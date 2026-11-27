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

### R4: the full boot prints scalars through the booted defaults

`(println "x" 1)` with scalar arguments took the generic path in the full boot: every value went through the `print-method` multimethod (dispatch, method lookup, an interpreted method call), where `BootMode::Lite` rendered the scalars directly. Programs that print per line paid that on each one; `hanoi` test 6 was 6.6× CPython with `native_print` and `core_read` its top self symbols.

The print snapshot now carries `defaults_only`: true in `BootMode::Lite` as before, and in the full boot while `print-method` and `print-dup` still hold the booted multis.

- **Guard:** the boot keeps both roots (`Evaluator::pristine_print`, shared by forks like the other registry-global baselines). The snapshot compares them with the live roots by `Arc::ptr_eq` on each refresh. Multis are immutable snapshots here (`defmethod`, `remove-method`, `prefer-method` install a new root), so the first `defmethod` ends the fast path for good, as does a replaced root.
- **Unchanged answers:** the fast path renders what the booted default methods render (`*print-readably*`, `*print-meta*`, `*print-length*`, `*print-level*`, `*print-dup*` and a custom `*out*` writer included). A `*print-dup*` snapshot takes the generic path, as in `BootMode::Lite`.

Effect (Linux perf vs CPython's h2h count): `hanoi` tc6 6.55× → 3.13×, tc5 2.07× → 1.06×.

Pinned by `crates/clj-eval/tests/print_fast_277.rs`: 25 JVM-oracle rows from `probes/print_fast.clj`, 16 of them while the multis are pristine and the rest after its `defmethod`s.

### R5 and R6. Vector equality and emulated collection dispatch

Vector and subvector equality checks lengths before comparing elements. Two packed integer windows compare their slices directly. Other vector elements use recursive Clojure equivalence, including its lazy realization order. The 100 rows in `probes/vector_equiv.clj` are checked against the JVM in the evaluator tests.

Java method dispatch borrows the method name. Deques and priority queues handle collection methods through their concrete types before the generic collection handler walks the Java class hierarchy.

In the workstation measurements, the vector change reduced instructions on `address_translation` tc7 from 4.69G to 2.04G. The collection dispatch change reduced `sliding_window_max` tc7 from 4.20 to 2.59 times the saved CPython instruction count. These are Linux perf measurements, with Pin remaining the CI gate.

### Destructuring, queue updates, and numeric traversal

Destructuring consumes its hidden VM input register. Persistent children with no other owners can then update in place. A consumed queue can also update its backing vector directly, and a front pop clones no backing storage until compaction.

The interpreter parses immutable string collections in one pass when `mapv` calls a unary Java numeric parser. It reuses the AOT parsing helper and falls back to the original call for invalid tokens or a redefined dispatch var. A reduction of mapped long products can consume two packed vector windows in one pass. It preserves checked arithmetic, with the ordinary reduction handling overflow and shared lazy inputs.

All 60 outputs matched the saved reference files in a macOS release comparison. Relative to the previous interpreter, retired instructions fell by 88% for `perceptron` tc7, 36% for `red_gateway` tc7, and 32% for `tree_diameter` tc7. The earlier Linux A/B measured `tree_diameter` tc7 at 4.33× CPython before the ownership changes and 3.12× afterward.

### Concatenation storage and nested vector reads

A concat stores its remaining inputs in a single shared slice. When a collector owns the concat, it releases the root before traversal and moves past unused inputs. Nested concat tails keep their resume order. Literal two-key reads of vectors of vectors borrow the intermediate row, including when the final cell holds nil. Other lookup types and wrapped indexes continue through the ordinary lookup path.

The macOS release comparison matched all 60 retry outputs. Peak RSS fell 16% for `hanoi` tc6, and the eight `rational_chebyshev` cases used 9–13% fewer instructions.

### Array reads in scalar VM loops

The interpreter's scalar loop tier reads invariant arrays through the existing locked `aget` accessor. The loop uses the normal boxed path for invalid indexes, unsupported values, and redefined vars. AOT continues to lower array reads through its general interop path.

In a macOS release A/B, `lfsr` tc4–tc7 used about 29% fewer retired instructions. The final interpreter matched the reference output on all 3,315 cases.

### PersistentQueue in AOT builds

The AOT constant pool resolves the canonical `PersistentQueue/EMPTY` by static field name. With the queue's JVM-style printed form, trying to read it back as data caused native compilation to fall back to embedded evaluation. The linked executable test checks native compilation, field identity, and FIFO operations.

### Fresh concat cells and two-key VM lookups

A fresh concat stores its inputs in the memo cell. Once traversal starts, it can use a cursor and a resume stack to remember its position. The VM borrows the collection and keys for a two-key `get-in` when all three operands can be read in place. It continues to construct the path for a redefined var.

On the Linux workstation, all 3,315 interpreter outputs matched the reference files. The largest `rational_chebyshev` cases used 24% fewer retired instructions, and `hanoi` tc6 peak RSS fell from 26,884 to 21,224 KiB.

### AOT ownership for `assoc`

When a three-argument `assoc` is a local collection's last use, the compiler evaluates the key and value before moving the collection. They can read it directly, and the update can reuse its storage when it is uniquely held. With LLVM 23 on the workstation, `wildcard_matching` tc5 fell from 45.6 billion retired instructions to 923 million, with the same output and peak RSS.
