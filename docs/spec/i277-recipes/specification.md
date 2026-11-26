# #277: no clojure-rs vs CPython loss on the refreshed suite

## Goal

The snippet repos' T1263 merges grew the h2h suite from 1754 to 3315 cases. On the new cases, `clojure-rs run` lost the enforced `recipes` leg (interpreter vs CPython 3.11, strict 1.15 tie margin on median Pin instructions and median peak RSS): 61 cases in 33 new programs.

The requirement (user, 2026-11-26): no clojure-rs vs CPython loss in any case, fixed in #277 before it merges.

## Method

Each fix must be generic: no snippet-specific paths. Each case is profiled on Linux with `perf record -e instructions:u` (see `scripts`-free tooling notes in the PR). The worst ratios came from algorithmic gaps in the runtime, where an operation that is O(1) or O(log n) on the JVM was O(n) here; those go first.

## Slices

### R1: `clojure.lang.PersistentQueue` is a real FIFO

Before, a queue was a vector with a `:clojure-rs/queue` metadata mark. `pop` rebuilt the vector without its front, so a BFS over n nodes was O(n²). `tree_serialize` ran 20.7G instructions where CPython ran 231M (88×); `tree_diameter` test 7 ran 168 s, past the 1500 s Pin budget on every clojure-rs leg.

`clj_value::QueueValue` is a window `items[start..]` over a persistent vector:

- **`pop`** advances `start` in O(1). The dead prefix compacts once it outgrows the live window, so popping is O(1) amortized and the prefix never holds more than the live items.
- **`conj`** is the vector's `push_back`: in place for a uniquely held queue (the VM's owned `conj`/`pop` ops handle a moved loop accumulator in place), otherwise a persistent push.
- **JVM semantics**, where the marked vector diverged:
  - `class` is `clojure.lang.PersistentQueue`, and `vector?` is false;
  - `get` answers nil, and `contains?` throws;
  - printing uses the JVM's `#object[clojure.lang.PersistentQueue 0x… "clojure.lang.PersistentQueue@<hashCode>"]` (the identity hash is arbitrary on both);
  - `str` is `Object.toString`.
- **Unchanged answers:** `=` and `hash` follow the `Sequential` contract (equal to vectors and lists with the same items). `seq`, `count`, `nth`, `into`, `reduce`, `vec`, `empty` and `meta` answer as before.

Pinned by `crates/clj-eval/tests/persistent_queue_277.rs` (40 rows from `probes/queue.clj` against `probes/queue.out`, JVM Clojure 1.13.0-alpha10 on JDK 26) and a `clj-value` unit test (compaction, in-place ops, equality and hashing).

Result (Linux, perf):

| case | before | after | CPython |
| --- | --- | --- | --- |
| `tree_serialize` tc6 | 20.7G instructions (88×) | 651M (2.8×) | 231M |
| `tree_diameter` tc7 | 168 s (Pin timeout) | 3.92G (4.3×) | 906M |

The remaining gap there is persistent-data overhead, which later slices address.

### R2: sorted sets insert in place

Two costs hit every `conj`/`disj` on a sorted set used as a priority queue.

- **Storage:** past 8 members a sorted set used the hash-indexed large storage. Each insert re-cloned every member into a new sorted sidecar and updated the `rpds` index, which sorted sets never read: they find positions by comparator binary search. Sorted sets now keep the flat sorted slice up to 512 members (`SORTED_SMALL_SET_MAX`).
- **Ownership:** the sorted `conj`/`disj` helpers borrowed the set and cloned it before inserting, so even a uniquely held set (a moved loop accumulator) copied every member. They now take the set by value. A uniquely held set inserts or removes with one `memmove`.

Effect (Linux perf vs CPython's h2h count):

| case | before | after |
| --- | --- | --- |
| `k_way_merge` tc6 | 2.55× | 0.99× |
| `k_way_merge` tc7 | 4.46× | 1.31× |
| `makespan` tc7 | 4.38× | 1.52× |

### R3: literal-path `get-in` / `assoc-in` in the VM

`(get-in m [i j])` and `(assoc-in m [i j] v)` built the path vector on every call and stepped it generically. Matrix code calls them in its inner loops. The VM now compiles a vector-literal path of 1-4 keys into the keys as operands:

- **`get-in`** nests `lookup_present` steps, equivalent to the JVM's `get-in` for vector paths (`(get nil k nf)` is `nf`), not-found included.
- **`assoc-in`** is an update op: a last-use collection register moves out, and each level takes its child out of an owned parent, so a uniquely held nest updates in place.
- **Redefinition:** a redefined var (an invalid site) builds the vector and calls through the var, like every direct call here.

Effect: `rational_chebyshev` 2.0-6.8× → 1.16-3.4×, `lup` tc7 4.4× → 2.8×, `motion_planning` tc6 3.7× → 3.0×.

Pinned by `crates/clj-eval/tests/literal_paths_277.rs`: 29 JVM-oracle rows from `probes/literal_paths.clj`, covering every operand shape and the sorted-set cases, plus the redefinition behavior.
