# Zero-oos S10: cells, taps, and helpers (issue #53) — Specification

- Status: spec + oracle probes (95 rows, all exit 0 and
  rerun-stable). No source edits; implementation is the
  owner's.
- Parent: out-of-scope reduction. Issue: #53. Branch:
  `zero-oos-s10-cells`. Probes:
  `docs/zero-oos-s10-cells/probes/{cells,taps,helpers,executors}.{clj,out}`
  (oracle: 1.13.0-alpha10 jars under `/home/hhu/.m2`;
  invocation in each `.clj` header).
- Scope: 13 vars — `volatile!`, `volatile?`, `vreset!`,
  `vswap!`, `add-tap`, `remove-tap`, `tap->`, `tap>`,
  `comparator`, `hash-ordered-coll`, `hash-unordered-coll`,
  `set-agent-send-executor!`,
  `set-agent-send-off-executor!`. Fences per the issue:
  owns `docs/zero-oos-s10-cells/`,
  `rust/crates/clj-eval/tests/cellstaps.rs`, the `Volatile`
  kind, the tap registry, `deref`'s volatile arm,
  `comparator`, the ordered/unordered hash natives, the
  executor vars. `hasheq.rs` is symbol-shared with S12
  (§2e). Matrix edits limited to the 13 rows + header
  counts (§1f).

## Part 1 — Contract

### 1a. Volatile cells (`volatile!`, `volatile?`, `vreset!`, `vswap!`, `deref` arm)

New `Value::Volatile` cell grounded in `atom.rs`'s
shared-cell pattern (`Arc<Mutex<…>>` + print id), minus
watches, validators, and meta. Single-threaded semantics
need no lock protocol beyond the mutex itself (the
established interior mutability — `xform.rs` counters).

| probe id | pins (port matches the oracle row) |
|---|---|
| `c-new` | `(volatile! 0)` constructs; JVM prints `#object[clojure.lang.Volatile 0xH {:status :ready, :val 0}]` (§2f for the port print) |
| `c-new-nil` | `(volatile! nil)` holds nil |
| `c-new-coll` | cell holds collections verbatim |
| `c-pred-t` | `volatile?` true on a cell |
| `c-pred-atom/int/nil/str` | `volatile?` false on atom, number, nil, string |
| `c-deref` | `(deref v)` → current value (new arm in `native_deref`) |
| `c-deref-reader` | `@v` → current value |
| `c-deref-timeout` | `(deref v 100 :t)` THROWs `ClassCastException` to `java.util.concurrent.Future` — volatiles are not blocking; they fall into `deref_timeout`'s existing `other` arm, no new arm |
| `c-deref-nil` | control: `(deref nil)` NPE text (already pinned; unchanged) |
| `c-vreset` | `(vreset! v 9)` returns newval AND stores it (`[9 9]`) |
| `c-vreset-nil` | reset to nil stores nil |
| `c-vreset-nonvol` | `(vreset! 1 2)` THROWs CCE to `clojure.lang.Volatile` |
| `c-vreset-nil-vol` | `(vreset! nil 2)` THROWs NPE naming `Volatile.reset` |
| `c-vswap` | `(vswap! v + 5)` returns the swapped-in value (`[5 5]` — the issue BDD) |
| `c-vswap-multi` | extra args applied (`[7 7]`) |
| `c-vswap-noargs` | single-arg f (`[2 2]`) |
| `c-vswap-nonfn` | non-fn f THROWs CCE to `clojure.lang.IFn` at the call |
| `c-vswap-nonvol` | non-volatile vol THROWs CCE to `clojure.lang.Volatile` (the `^Volatile` tag checkcast) |
| `c-shape*` | `macroexpand-1` of `(vswap! v + 5)` is exactly `(.reset v (+ (.deref v) 5))` (multi/noargs analogous) |
| `c-double-eval` | vol is evaluated TWICE (`[2 2]` — side-effect counter) |
| `c-eq-same/distinct` | identity equality only (`true` / `false`) |
| `c-identical` | same cell is `identical?` |
| `c-meta` | `(meta v)` → nil (no meta support) |
| `c-ifn` / `c-counted` | both false |

**`vswap!` decision (in-spec):** a builtin macro (in
`builtin_expand.rs`, beside the other core macros)
expanding to `(vreset! VOL (F (deref VOL) ARGS…))` —
the direct analog of the JVM shape, with the port's
own `vreset!`/`deref` in place of `.reset`/`.deref`.
VOL is deliberately NOT gensym-bound: the JVM
evaluates it twice (`c-double-eval`), and single-eval
would be an observable behavior change. The
`^Volatile` tag has no port analog (no host hints);
the equivalent checkcast is `vreset!`'s CCE
(`c-vswap-nonvol`).

**Printing decision (in-spec):** the port prints
volatiles with the transient `#object` convention
established for atoms (`print.rs`), i.e.
`#object[clojure.lang.Volatile 0x{id:x}
"clojure.lang.Volatile@{id:x}"]` — a pinned
divergence from the JVM's `{:status :ready, :val …}`
rendering (§2f), same rationale as atoms.

### 1b. Taps (`add-tap`, `remove-tap`, `tap>`, `tap->`)

| probe id | pins (port matches the oracle row) |
|---|---|
| `t-no-taps` | `(tap> :lonely)` → true with no taps registered |
| `t-add-ret` | `add-tap` returns nil (and lazily starts the loop) |
| `t-add-nonfn-ret` | `add-tap` of a non-fn returns nil, no throw |
| `t-remove-ret` | `remove-tap` returns nil; removing twice still nil |
| `t-remove-never` | removing a never-added fn returns nil, no throw |
| `t-deliver` | roundtrip: add, `tap>` `:x`, tap receives `:x` (the issue BDD) |
| `t-tap-ret-with-tap` | `tap>` → true with taps present |
| `t-thread` | delivery runs on a thread named exactly `clojure.core/tap-loop` |
| `t-nil` | `(tap> nil)` delivers nil (`[:got nil]`) |
| `t-add-twice` | set semantics: added twice, delivered ONCE (`1`) |
| `t-removed-misses` | tap removed before take never fires (`:TIMEOUT`) |
| `t-throw-safe` | a throwing tap is swallowed; other taps still fire |
| `t-tap->-ret` | `(tap-> 5)` returns `5` |
| `t-tap->-delivers/nil/false` | `tap->` returns x AND delivers it (`[:threaded :threaded]`, `[nil [:got nil]]`, `[false [:got false]]`) |
| `t-full` | full-queue offer counts: 1500 offers against a blocked consumer → `[1024 476]` (capacity 1024; `false` past it) |

**Dispatcher threading model (in-spec):** one daemon
dispatcher thread named `clojure.core/tap-loop` per evaluator
lineage (the `AgentSystem` precedent: one runtime is one
lineage; forked evaluators share the system, separate
runtimes do not — the JVM's single loop is per classloader,
and the port has no shared-global equivalent),
started lazily on the first `add-tap`/`tap>` (the
`force tap-loop` analog); one global bounded FIFO
queue of capacity **1024**; one global registry set
(add = conj, remove = disj). The loop takes one item,
snapshots the registry, and calls each tap inside a
per-tap catch-all — exactly the JVM structure
(`core.clj`: `tapset` atom of `#{}`, `tapq`
`ArrayBlockingQueue(1024)`, single `tap-loop`
`Thread`). `tap>` is a non-blocking offer returning
bool. No nil sentinel is needed in the port
(`Value::Nil` is a first-class queue item); the
observable nil roundtrip (`t-nil`) is what the tests
pin.

On the issue Solution's "one delivery thread each
like the JVM's per-tap loop": not honored literally
— the JVM has a SINGLE shared loop thread, not one
per tap (corroborated by the `core.clj` source, one
`Thread.` over a `doseq`, and by `t-thread` pinning
one name). The single-dispatcher model above is the
faithful reading (§2c).

**`tap->` (in-spec):** the verbatim JVM one-liner in
`core.clj` (S1 `ffirst` precedent; `doto` exists at
`core.clj:315`): `(defn tap-> [x] (doto x tap>))`. No
native needed.

### 1c. `comparator` (core.clj one-liner)

`(defn comparator [pred] (fn [x y] (cond (pred x y)
-1 (pred y x) 1 :else 0)))` — the verbatim JVM body.
Call convention matches `coll.rs` `compare_sort_keys`
(`SortSpec::Custom` calls `(cmp a b)`;
`afunction_order` consumes the ints), so `sort`,
`sort-by`, and `sorted-map-by` interoperate with no
`coll.rs` change.

| probe id | pins (port matches the oracle row) |
|---|---|
| `k-lt/gt/eq` | `((comparator <) 1 2)` → `-1`; flipped → `1`; equal → `0` |
| `k-flip/flip2` | `(comparator >)` mirrors (`1` / `-1`) |
| `k-sort` | `(sort (comparator >) [1 2 3])` → `(3 2 1)` (the issue BDD shape) |
| `k-sorted-map` | `(keys (into (sorted-map-by (comparator >)) {1 :a 2 :b 3 :c}))` → `(3 2 1)` |
| `k-truthy-lt/eq` | truthy (non-bool) pred results order via `cond` (`-1` / `0`) |
| `k-str` | length-pred sort → `("a" "bb" "ccc")` |
| `k-arity` | 1-arg call THROWs `ArityException` (`Wrong number of args (1) … comparator/fn--N`) |
| `k-nonfn-pred` | non-fn pred THROWs CCE to `clojure.lang.IFn` when called |

### 1d. Hash helpers (`hash-ordered-coll`, `hash-unordered-coll`)

Natives over the existing `hasheq.rs` Murmur3 steps
(`hash_ordered`/`hash_unordered`, today private —
expose to the natives; disjoint from S12's
`hash_combine`/`mix_collection_hash` ownership, §2e).
Single-arity `[coll]`; the arg must be a port
collection (vectors, lists, maps, sets, seqs —
whatever the port seqs). Strings and numbers are NOT
accepted even though some are seqable: the JVM takes
`Iterable`, and `String`/`Long` are not `Iterable`
(`h-ord-string`, `h-ord-num` CCE). Map args iterate
entries (`h-ord-map`, `h-unord-map`).

| probe id | pins (port matches the oracle row verbatim) |
|---|---|
| `h-ord-123` | `(hash-ordered-coll [1 2 3])` → `736442005` |
| `h-ord-empty` | `[]` → `-2017569654` |
| `h-ord-single` | `[42]` → `1127286853` |
| `h-ord-list` | `'(1 2)` → `156247261` |
| `h-ord-eq-vec-list` | vec/list agree → true |
| `h-ord-eq-hash` | `= (hash [1 2 3])` → true |
| `h-ord-str-els` | `["a" "b"]` → `-757264838` |
| `h-ord-nested` | `[[1] #{2} {:a 1}]` → `1393116264` |
| `h-ord-map` | `{:a 1}` → `1242899341` |
| `h-ord-neg` | `[-1 0]` → `-1894195675` |
| `h-ord-nil` | nil THROWs NPE naming `Iterable.iterator()` |
| `h-ord-string/num` | THROW CCE to `java.lang.Iterable` |
| `h-unord-set` | `#{1 2 3}` → `439094965` |
| `h-unord-empty` | `#{}` → `-15128758` |
| `h-unord-single` | `#{42}` → `-1234344236` |
| `h-unord-eq-hash` | `= (hash #{1 2 3})` → true |
| `h-unord-map` | `{:a 1 :b 2}` → `161871944` |
| `h-unord-map-eq-hash` | `= (hash {:a 1 :b 2})` → true |
| `h-unord-vec-order` | order-insensitive → true |
| `h-unord-vec-val` | `[1 2]` → `460223544` |
| `h-entry` | `(hash entry)` = `(hash-ordered-coll [k v])` → `[-1809393901 -1809393901 true]` |
| `h-unord-nil` | nil THROWs the same NPE |

### 1e. Executor vars (`set-agent-send-executor!`, `set-agent-send-off-executor!`)

| probe id | pins |
|---|---|
| `e-set-ret` / `e-off-ret` | returns the executor itself (`set!` value semantics; `identical?` → true) — PORT MATCHES |
| `e-routes` / `e-off-routes` | JVM runs subsequent actions on the supplied pool's threads (`[1 "s10-probe-pool"]`) — PINNED DIVERGENCE (below) |
| `e-nil-set` / `e-off-nil-set` | nil accepted at set time (`:restored`) — PORT MATCHES |
| `e-nil-send` | JVM send under a nil pool silently drops the action (returns the agent, value stays `0`, no error) — PINNED DIVERGENCE |
| `e-non-exec` / `e-off-non-exec` | JVM rejects non-executors with CCE to `ExecutorService` — PINNED DIVERGENCE |
| `e-non-exec-pool-ok` | JVM pool untouched after rejection (`[:caught 1]`) — port analog: set never disturbs dispatch |
| `e-cross` | pools independent on the JVM → true — holds trivially in the port |
| `e-restored` | oracle hygiene (defaults restored; `shutdown-agents` ends the run) |

**Emulation decision (in-spec): ACCEPT-AND-RECORD.**
Each var stores its arg in a per-pool config slot and
returns it. Dispatch ALWAYS uses the fixed pools
(`send` → send pool, `send-off` → send-off pool);
the recorded value is never consulted. No validation:
any value including nil is accepted. This is the
`send-via` precedent verbatim ("executor accepted and
ignored; routes to the fixed pool" — `concurrency.rs`
`native_send_via`, matrix row `send-via`): the port
has no host executors (N1), so custom routing is
unimplementable, and it has no `ExecutorService` host
type to check against, so the JVM's set-time CCE
(`e-non-exec`) is unimplementable too — a typecheck
would accept everything or nothing. Recording (rather
than pure ignore) keeps the `set!` observable for any
future reader at trivial cost. Consequences, all
pinned by `cellstaps.rs exec_record` tests: post-set
sends always complete on the fixed pools (even after
setting nil or `:kw`); no CCE, no silent drop — the
two deliberate divergences from `e-routes` and
`e-nil-send`/`e-non-exec`.

### 1f. BDD scenarios + matrix edits

BDD (from the issue Plan; each traces to oracle rows
and to a `cellstaps.rs` test):

- `(let [v (volatile! 0)] (vswap! v + 5) @v)` → `5`
  (`c-vswap`).
- `(do (add-tap f) (tap> :x))` delivers `:x` to `f`
  (`t-deliver`).
- `(sort (comparator >) [1 2])` → `(2 1)` (`k-sort`
  shape).
- `(hash-ordered-coll [1 2 3])` → `736442005`,
  `(hash-unordered-coll #{1 2 3})` → `439094965`
  (`h-ord-123`, `h-unord-set`).

Matrix (`rust/core-coverage.md`): the 13 rows already
exist at their mirror positions (scattered across
lines 696–869; #41 landed, so no rows are added —
§2h), all `out-of-scope`. The implementation flips
each in place to `implemented` with `cellstaps.rs`
test notes per area (`volatile_*`, `tap_*`,
`comparator_*`, `hash_*`, `exec_record`), and the
header counts move from 410/2/280/0 to **423
implemented, 2 divergent, 267 out of scope, 0
planned** (692 total; both endpoints re-verified by
counting status rows in the file — §2a).

## Part 2 — Notes

### 2a. Derivation story

95 oracle rows (cells 31 + taps 17 + helpers 35 +
executors 12 — line counts re-verified against the
committed `.out` files), every row exit 0 and
rerun-stable; no source edits. Each Part 1 pin
traces to one oracle row; error rows pin class +
message via `throwcause` (root cause, eval counters
scrubbed). Three `core.clj` reads ground the
structural claims: the tap block (lines 8313–8329),
`tap->` (8357–8361), `comparator` (3076–3082) — all
from the alpha10 jar under `/home/hhu/.m2`, the same
oracle the probes ran against. Header math (§1f) was
re-verified by counting status rows in the file
(410/2/280/0 here), not by trusting the header.

### 2b. Volatile rationale

`Value::Volatile` copies `atom.rs`'s shared-cell
pattern (`Arc<Mutex<…>>` + print id) minus watches,
validators, and meta — the probes pin the minus:
`c-meta` nil, identity-only equality
(`c-eq-same`/`c-eq-distinct`, `c-identical`), neither
IFn nor counted (`c-ifn`, `c-counted`). `vswap!` is
a builtin macro (not a native) because the JVM's is
a macro with observable double evaluation: `c-shape*`
pins the exact `(.reset v (f (.deref v) …))`
expansion and `c-double-eval` pins the two
evaluations, so the port expands to its own
`vreset!`/`deref` and keeps the double eval. The
`^Volatile` tag checkcast has no port analog (no
host hints); `vreset!`'s CCE (`c-vswap-nonvol`,
`c-vreset-nonvol`) is the equivalent gate. `deref`
needs only the value arm: `c-deref-timeout` proves
volatiles are not `Future`s, so the timeout path
stays in `deref_timeout`'s existing `other` arm.

### 2c. Tap single-dispatcher corroboration

The issue Solution read the JVM as one delivery
thread per tap; the source says otherwise.
`core.clj:8313–8329` defines ONE `tapset` atom, ONE
`tapq` (`ArrayBlockingQueue`, capacity 1024), and
ONE `tap-loop` — a `delay` forcing a single daemon
`Thread` named `clojure.core/tap-loop` that `take`s,
snapshots `@tapset`, and `doseq`s over the snapshot
inside a per-tap `(catch Throwable …)`. Four oracle
rows corroborate the four structural facts
independently of the source read: `t-thread` pins
the exact thread name; `t-full` pins capacity 1024
(`[1024 476]`); `t-add-twice` pins set semantics
(delivered once); `t-throw-safe` pins the per-tap
catch-all (a throwing tap neither kills the loop nor
starves later taps). `t-tap->-ret`/`-delivers` pin
`tap->` as deliver-then-return-x, matching the
verbatim `(doto x tap>)` one-liner at
`core.clj:8357–8361`. The port therefore builds one daemon dispatcher per
lineage, one bounded FIFO (1024) per lineage,
one registry set — the faithful reading, not the
per-tap loop (§2h).

### 2d. Comparator and hash-helper derivation

`comparator` is the verbatim JVM body
(`core.clj:3076–3082`): `(fn [x y] (cond …))`. The
`cond` (not `if`) matters — `k-truthy-lt` and
`k-truthy-eq` pin truthy non-bool pred results
ordering. The call convention `(cmp a b)` → ints is
already what the port's `SortSpec::Custom` /
`afunction_order` consume (§1c), so no `coll.rs`
change; `k-sort` and `k-sorted-map` pin the interop
end to end. The hash natives expose the existing
private Murmur3 steps (`hasheq.rs`: `hash_ordered`,
`hash_unordered`) over port collections. The JVM arg
contract is `Iterable`, pinned negatively: strings
and numbers THROW CCE to `java.lang.Iterable`
(`h-ord-string`, `h-ord-num`) even though some are
seqable, nil THROWs the NPE naming
`Iterable.iterator()` (`h-ord-nil`, `h-unord-nil`),
and maps iterate entries with values pinned
verbatim (`h-ord-map`, `h-unord-map`). Entry hash
equals the ordered hash of `[k v]` (`h-entry`).

### 2e. `hasheq.rs` symbol-sharing with S12

S10 and S12 both touch
`rust/crates/clj-value/src/hasheq.rs`, but the
symbol sets are disjoint: S10 exposes
`hash_ordered` / `hash_unordered` (today private
fns) to the new natives and touches nothing else
in the file; S12 owns `hash_combine` (also private
today) plus the `mix_collection_hash` step it adds.
Neither slice renames or reshapes the other's
symbols, so the merge is textual (visibility
markers plus adjacent new fns) and whoever lands
second resolves it — the same second-merger rule as
#40/#33 on `core-coverage.md` (see the #40 spec
§2d). No shared test file either: S10 pins via
`cellstaps.rs hash_*`.

### 2f. Print and emulation precedents

The volatile print is a pinned divergence with an
established rationale, not an approximation: the
JVM renders `#object[… {:status :ready, :val …}]`
(`c-new*`), but the port has no `{:status …}` map
machinery for cells, so volatiles take the
transient `#object` convention already established
for atoms and delays (`print.rs`:
`#object[clojure.lang.Atom 0x{id:x} "…@{id:x}"]`).
Same class-name/identity-hash shape, same
rationale — one convention for all cells.

The executor rows earn `implemented` (not
`divergent`) by the `send-via` precedent:
`concurrency.rs:887` documents "executor accepted
and ignored … pinned divergence", `native_send_via`
accepts any executor and routes to the fixed
send-off pool, and the matrix row `send-via` is
`implemented` with that note. S10's accept-and-
record is the same shape — the arg is stored and
returned, dispatch stays on the fixed pools — so
the two rows flip to `implemented` with
`exec_record` notes naming the pinned divergences
(`e-routes` custom routing, `e-nil-send` silent
drop, `e-non-exec` set-time CCE), exactly as
`send-via` carries its divergence inside an
`implemented` row.

### 2g. Open risks

1. **Accepted executor divergences.** `e-nil-send`
   (JVM silently drops the action under a nil pool;
   the port completes it on the fixed pool) and
   `e-non-exec` / `e-off-non-exec` (JVM set-time
   CCE to `ExecutorService`; the port has no host
   executor type to check against, so it accepts
   everything) are deliberate and pinned — but they
   are observable behavior gaps a strict
   differential corpus would flag. Contained by the
   `send-via` precedent (§2f); still, any future
   host-executor work must revisit these rows
   first.
2. **`t-full` analog flake-avoidance.** The oracle
   row is timing-shaped (block the consumer, flood
   1500 offers, count `[1024 476]`) with 500 ms
   sleeps. The `cellstaps.rs` analog must not
   assert exact timing — gate on the blocked
   consumer (promise/rendezvous) before flooding,
   then assert the capacity counts, so loaded CI
   cannot flake it.
3. **S12 merge coordination.** Disjoint symbols
   (§2e) keep the `hasheq.rs` merge textual, but
   both slices also flip `core-coverage.md` header
   counts — the second merger recomputes the
   header from the file, never by adding deltas to
   a stale base.
4. **Daemon/lazy dispatcher + `shutdown-agents`
   oracle note.** The JVM tap loop is daemon +
   lazy (`delay`/`force`, `.setDaemon true`); the
   port dispatcher must be too, or tap tests hang
   the suite at exit. Oracle hygiene note: the
   agent pools are NON-daemon, which is why
   `executors.clj` ends with `(shutdown-agents)` —
   without it the oracle JVM never exits. The
   port's `cellstaps.rs` needs the symmetric
   discipline: daemon tap dispatcher, joined /
   shutdown agent pools.

### 2h. Issue-Solution deviations + follow-ups

Two deviations from the issue #53 Solution, both
forced by evidence landed since:

1. **Per-tap loop → single dispatcher.** The
   Solution specified one delivery thread per tap
   "like the JVM"; the JVM has a single shared
   loop (§2c). Corrected, not honored literally.
2. **Mirror-position row adds → flip in place.**
   The Solution assumed the 13 rows were missing
   and had to be added at mirror positions; #41
   has since landed them all as `out-of-scope`
   (verified present, lines 696–869), so S10 only
   flips the status field plus notes and header
   counts (§1f).

**Follow-ups (not this slice):** the `cellstaps.rs`
implementation itself (owner's); any future
host-executor support revisiting the §2g.1
divergences; S12's `hasheq.rs` half (§2e); no
`planned` / `divergent` semantics change.