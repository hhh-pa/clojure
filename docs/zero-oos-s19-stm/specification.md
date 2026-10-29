# Zero-oos S19: STM (issue #63) — Specification

- Status: spec + oracle probes (212 rows, all exit 0 and
  rerun-stable). No source edits; implementation is the
  owner's.
- Parent: out-of-scope reduction. Issue: #63. Branch:
  `zero-oos-s19-stm`. Probes:
  `docs/zero-oos-s19-stm/probes/{refs,txn,errors,portside}.{clj,out}`
  (174 spec-first rows + 38 review-committed `u-*` rows;
  oracle: 1.13.0-alpha10 jars under `/home/hhu/.m2`;
  invocation in each `.clj` header).
- Scope: 11 vars — `ref`, `dosync`, `commute`, `ensure`,
  `alter`, `ref-set`, `io!`, `ref-history-count`,
  `ref-max-history`, `ref-min-history`, `sync`. Fences per
  the issue: owns `docs/zero-oos-s19-stm/`,
  `rust/crates/clj-eval/tests/stm.rs`, the `Ref` value
  kind, the dosync machinery, the history natives. Matrix
  edits limited to the 11 rows + header counts (§1i).
- Row flips are NOT in this slice: the implementer flips
  the 11 rows in place from current `origin/master` (§1i,
  §2h.1).

## Part 1 — Contract

### 1a. The `Ref` value kind (`ref`, history accessors)

New `Value::Ref(Ref)` in `clj-value` (new `ref.rs`),
grounded in `atom.rs`'s shared-cell pattern: a print id
plus `Arc<Mutex<RefState>>`. `RefState` holds the current
value, its commit point (`u64`), the history chain
(`VecDeque<(point, value)>`, newest first), `min_history`
/ `max_history` (`i64`, always int-range by construction —
§1a conversion), the `:validator` fn, the `:meta` map, and
the watches map. Equality is shared-cell identity plus
equal meta (`cell_meta_eq`, like atoms); hashing is a new
tag plus the id (beside `hash_value_rest`'s atom arm).

| probe id | pins (port matches the oracle row) |
|---|---|
| `r-new` / `r-new-nil` / `r-new-coll` | `(ref x)` constructs over any value; JVM prints `#object[clojure.lang.Ref 0xH {:status :ready, :val …}]` (§2f for the port print) |
| `r-meta-opt` / `r-meta-none` | `:meta` option installs; absent reads nil |
| `r-min-default` / `r-max-default` | defaults are 0 / 10 |
| `r-min-opt` / `r-max-opt` | `:min-history` / `:max-history` options install through the §1a conversion |
| `r-min-set` / `r-max-set` | setters return THE REF (`identical?`, `[true 3]` / `[true 7]`) |
| `r-max-below-min` / `r-min-above-max` | NO cross-constraint: max 3 over min 5 reads `[5 3]`; min 9 over max 7 reads `[9 7]` |
| `r-min-negative` / `r-max-negative` | NO lower bound: `-1` installs and reads back |
| `r-history-init` / `r-history-after` / `r-history-after2` | count starts 0; stays 0 after commits under default bounds |
| `r-history-min-keeps` / `r-history-min3-many` | with min 5 / min 3, the count keeps 2 after 2 commits / 3 after 8 commits |
| `r-history-max-trims` / `r-history-default-many` | min 0 trims to 0 even with max 1; defaults trim to 0 after 15 commits |
| `r-deref` / `r-deref-reader` | `deref` / `@` answer the current value |
| `r-deref-timeout` | `(deref r 100 :t)` THROWs `ClassCastException` to `java.util.concurrent.Future` — refs fall into `deref_timeout`'s existing `other` arm, no new arm |
| `r-realized` | `(realized? r)` THROWs `ClassCastException` to `clojure.lang.IPending` — refs are not pending; the existing `other` arm covers it, no new arm |
| `r-realized-atom` / `r-realized-num` | controls: atoms and numbers behave as today (unchanged) |
| `r-eq-same` / `r-eq-distinct` / `r-identical` | identity equality only |
| `r-counted` | `counted?` false (default arm, no new code) |
| `r-ifn` | `ifn?` TRUE — refs are callable (§1d; atoms are not: `r-atom-ifn`, `r-atom-invoke`) |
| `r-invoke0` / `r-invoke1` | invoking a non-fn-valued ref THROWs `ClassCastException` naming the VALUE's class to `IFn` (`Long cannot be cast to …IFn`) |
| `r-invoke-fnval` / `r-invoke-variadic` | invocation derefs and applies: `((ref (fn [a b] …)) 1 2)` → `3`; `(apply (ref +) (range 1 22))` → `231` |
| `r-odd-opts` | `(ref 0 :validator)` THROWs `IllegalArgumentException` `No value supplied for key: :validator` (reuse `atom_options`) |
| `r-bad-opt-key` | unknown options are silently ignored (`(ref 0 :bogus 1)` constructs) |
| `r-validator-nil-opt` | falsy `:validator` never installs (like `setup-reference`) |
| `r-validator-construct` / `r-validator-reject` | the initial value is validated at construction (`Invalid reference state`); commit validates the final value and aborts on veto, leaving the old value |
| `r-str` | `(str r)` → `clojure.lang.Ref@L` (join the `toString` arm in `core.rs`) |
| `r-with-meta` | `(with-meta r m)` THROWs `ClassCastException` to `clojure.lang.IObj` (generic `_` arm, no new code) |

**History-bound conversion (setters and options share one
helper).** The oracle ladder, in order:

1. `nil` → `NullPointerException` `Cannot invoke
   "java.lang.Character.charValue()" because "x" is null`
   (`e-min-nil`, `e-max-nil` — the JVM reflection path;
   the port reproduces the TEXT).
2. Non-`Number` → `ClassCastException` to
   `java.lang.Number` (`e-min-nonnum`, `e-max-nonnum`,
   `e-min-char`, `e-min-bool`, `e-min-str`).
3. `Double`: `NaN` → 0 (`e-min-nan`); out of long range →
   `IllegalArgumentException` `Value out of range for
   long: {Double.toString}` (`e-min-huge`, `e-min-negbig`
   — render with the existing `java_double_string`);
   else truncate toward zero (`e-min-double` /
   `e-max-double`: 3.9 → 3,
   `e-min-negdouble` −3.9 → −3).
4. `BigInt`: huge → `IllegalArgumentException` `Value out
   of range for long: {plain-decimal}` (`e-min-bigint-big`);
   else its long value (`e-min-bigint`).
5. `Ratio` / `BigDec`: truncate toward zero (`e-min-ratio`
   7/2 → 3, `e-min-negratio` → −3, `e-min-bigdec` → 7).
6. The resulting long outside int range →
   `ArithmeticException` `integer overflow` (`e-min-longbig`,
   `e-min-doublebig`).

Getters answer `Value::Int` of the stored bound. The nil-ref
and non-ref receiver checks precede everything: nil →
`NullPointerException` naming `Ref.getHistoryCount()` /
`Ref.getMinHistory()` / `Ref.getMaxHistory()`
(`e-count-nil`, `e-min-get-nil`, `e-max-get-nil`); non-ref
→ `ClassCastException` to `clojure.lang.Ref`
(`e-min-nonref`, `e-max-nonref`, `e-count-nonref`). Arities
are exactly `[ref]` for the count and `[ref]` / `[ref n]`
for min/max (`e-count-arity0/arity2`, `e-min-arity0/arity3`,
`e-max-arity0/arity3`, via the established arity-error
convention).

**History-count semantics.** The count is the history-chain
length after the commit trim (§1g): every commit trims the
chain to `min_history` (drop oldest while longer), so
single-threaded counts are fully determined by the rows
above. `max_history` has no single-threaded observable
role; its concurrent role (force-trim past pinned readers)
is specified in §1g.

### 1b. `dosync` / `sync` / `io!` (join-or-begin, join-or-`io!`, gate)

`dosync` is a builtin macro (`builtin_expand`, the
`vswap!` precedent: in `BUILTIN_MACROS`, no `SpecialOp`)
expanding to `(sync nil ~@body)` — byte-identical to the
JVM expansion (`t-shape-dosync`). `sync` and `io!` are new
`SpecialOp`s (analyzer + evaluator support, the `locking`
precedent): added to `SPECIAL_OPS`, `is_shadowable_macro`
(JVM macros shadow like `locking`), and `BUILTIN_MACROS`
(so the vars resolve), but NOT to `is_expand_macro` —
`macroexpand-1` leaves them alone (PINNED DIVERGENCE from
`t-shape-sync` / `t-shape-io`: the port has no host calls
to expand to; §2h.3).

| probe id | pins (port matches the oracle row) |
|---|---|
| `t-body-value` / `t-empty` | body value answers (`2`); empty body → nil |
| `t-commit` | `(dosync (alter r inc) (alter s + 5))` commits both (`[1 15]`) |
| `t-atomic-abort` | a throwing body commits nothing (`[:caught 0 10]`) |
| `t-deref-local` | `deref` inside sees uncommitted values (`5`) |
| `t-retry-invisible` | single-threaded body runs exactly once (`[:done 1]`) |
| `t-nested` | nested `dosync` joins: inner answers the alter value, outer commits once (`[2 2]`) |
| `t-nested-join` | joining commits once: one watch fire across nesting |
| `t-sync-in` | `(sync nil …)` inside `dosync` joins (same machinery — `dosync` IS `sync`) |
| `t-sync-out` / `t-sync-empty` | `(sync nil :x)` → `:x`; `(sync nil)` → nil |
| `t-sync-arity0` | `(sync)` THROWs `ArityException` `Wrong number of args (0) passed to: clojure.core/sync` |
| `t-sync-flags-ignored` | the flags form is NEVER evaluated (`[:body 0]`) — the analyzer drops it before analysis |
| `t-sync-flags-need-not-be-nil` | any flags value is accepted |
| `t-sync-runs-tx` | `sync` outside `dosync` still RUNS a transaction (`(sync nil (io! :x))` throws `I/O in transaction`) |
| `t-io-in` | `(dosync (io! :x))` THROWs `IllegalStateException` `I/O in transaction` — the gate fires BEFORE body evaluation (structural pins: `t-shape-io`, `t-shape-io-multi`) |
| `t-io-out` / `t-io-multi` / `t-io-empty` | outside a transaction the body runs and its value answers (`:x`, `3`, nil) |
| `t-shape-dosync` | `(macroexpand-1 '(dosync …))` is EXACTLY `(clojure.core/sync nil …)` |
| `e-dosync-passthrough` | body throws propagate unwrapped (`["boom" {:k 1}]`) |

**Join rule.** A `sync`/`dosync` with a running transaction
on this evaluator joins it (depth + 1, same `Transaction` —
one commit, one watch round, one outbox); otherwise it
begins one (§1g). `(macroexpand '(dosync x))` stops at
`(sync nil x)` (DIVERGENCE from the JVM's full expansion —
a consequence of the special-form decision).

**Recur fence (DIVERGENCE).** `sync` fences `recur` like
`try` (`Cannot recur across try`): the JVM instead binds
the recur to the sync macro's synthetic 0-arity fn — an
`IllegalArgumentException` arity mismatch when the counts
differ, a silent compile (recurring the body fn, not the
loop) when they coincide (oracle `u-recur-across`,
`u-recur-sync-zero`; the port's loud fence beats both the
confusing IAE and the silent infinite loop). An inner loop
recurs freely and `io!` never fences (`u-recur-inside`,
`u-recur-io`).

### 1c. `alter` / `commute` / `ref-set` / `ensure` (check order, composition)

One check order for all four, oracle-pinned step by step:
arity → ref cast → (alter/commute only) fn cast →
transaction gate. The ref cast precedes the gate
(`e-alter-nil-outside` is the NPE, NOT `No transaction
running`); the fn cast precedes the gate too
(`e-alter-nonfn-outside`, `e-commute-nonfn-outside`) AND
precedes nil-dispatch (`e-alter-nil-nonfn` is the `IFn`
cast, not the NPE).

| probe id | pins (port matches the oracle row) |
|---|---|
| `e-alter-outside` / `e-refset-outside` / `e-commute-outside` / `e-ensure-outside` | outside a transaction: `IllegalStateException` `No transaction running` |
| `e-alter-nil-in` / `e-refset-nil-in` / `e-commute-nil-in` / `e-ensure-nil-in` | nil ref → `NullPointerException` naming `clojure.lang.Ref.alter(clojure.lang.IFn, clojure.lang.ISeq)` / `Ref.set(Object)` / `Ref.commute(clojure.lang.IFn, clojure.lang.ISeq)` / `Ref.touch()` (inside AND outside — `e-alter-nil-outside`, `e-alter-nonref-outside`) |
| `e-alter-nonref-in` / `e-refset-nonref-in` / `e-commute-nonref-in` / `e-ensure-nonref-in` | non-ref → `ClassCastException` to `clojure.lang.Ref` (inside AND outside) |
| `e-alter-nonfn` / `e-commute-nonfn` | non-fn `fun` → `ClassCastException` to `clojure.lang.IFn` |
| `e-alter-arity` / `e-commute-arity` / `e-refset-arity` / `e-ensure-arity0` / `e-ensure-arity2` | `alter` / `commute` need `[ref fun & args]`; `ref-set` needs `[ref val]`; `ensure` needs exactly `[ref]` (established arity-error texts) |
| `t-alter-returns` / `t-refset-returns` / `t-commute-returns` / `t-ensure-returns` | answers: the new value / the set value / the new in-tx value / the current value (`3`, `9`, `5`, `7`) |
| `t-commute-reruns` | the commute fn runs EXACTLY twice single-threaded (once in-tx, once at commit): `[1 2]` |
| `t-commute-order` | multiple commutes re-run at commit IN ISSUE ORDER (`[10 [:f :g :f :g]]`) |
| `t-commute-alter` / `t-commute-refset` | alter/ref-set BEFORE commute compose (`11`): at commit, non-commute writes apply to latest first, then commutes re-run on top |
| `t-commute-alter-rev` / `t-commute-refset-rev` | alter/ref-set AFTER a commute on the same ref THROWs `IllegalStateException` `Can't set after commute` |
| `t-commute-set-immediate` | the `Can't set after commute` throw fires AT the call (later body never runs: `[:t 0 0]`) |
| `t-commute-abort-atomic` | it aborts the whole transaction atomically (`[:t 0 0]`) |
| `t-commute-ensure` / `t-ensure-commute` | `ensure` composes with commute both ways (`1`; after commute it sees the tx-local value) |

**Semantics.** `alter` applies `fun` to the tx-local value
plus args DURING the transaction; `ref-set` replaces the
tx-local value; `commute` applies `fun` during the
transaction (for the tx-local value) AND records
`(ref, fun, args)` for the commit re-run; `ensure`
read-locks (records the read version; commit revalidates —
write-skew prevention, observable only under contention;
single-threaded it answers the value). At commit, each
ref's commutes re-run in issue order over (latest
committed ⊕ that txn's earlier non-commute writes). A ref
touched by `commute` rejects later in-txn `alter`/`ref-set`
with `Can't set after commute`, thrown immediately.

### 1d. Generic-fn arms for refs (deref, invoke, meta, watches, validator)

Refs join the existing generic arms; several need NO new
code (the fallthrough already matches — the implementer
verifies each with the cited pin):

| site | change | evidence |
|---|---|---|
| `native_deref` 1-arity | NEW `Value::Ref` arm: tx-local value when this txn wrote it, else the committed value (§1g read path) | `t-deref-local`, `t-commit` |
| `deref_timeout` | none (existing `other` arm) | `r-deref-timeout` |
| `is_ifn` | NEW arm → true | `r-ifn` |
| invoke dispatch | NEW `Value::Ref` arm: tx-aware deref, then invoke the VALUE with the same args (any arity, incl. 0); a non-fn value takes the existing `IFn` cast error, which names the value's class | `t-invoke-in-txn` (`50`), `r-invoke-fnval`, `r-invoke-variadic`, `r-invoke0`, `r-invoke1` |
| `native_realized` | none (existing `other` → `IPending` cast; `as_pending` answers None for refs) | `r-realized` |
| `is_counted` | none (default false) | `r-counted` |
| `str` (`core.rs`) | join the `toString` arm → `clojure.lang.Ref@{id:x}` | `r-str` |
| `meta_of` + `value_meta` | NEW arms → the cloned cell meta | `r-meta-opt`, `r-meta-none` |
| `as_iref` | NEW `WatchedRef::Ref` (same nil/CCE shapes as atoms); `add-watch` / `remove-watch` answer the ref | `t-addwatch-returns`, `t-removewatch-returns`, `t-addwatch-in-txn` |
| `as_ref_target` (`alter-meta!`, `reset-meta!`) | NEW `RefTarget::Ref` | `u-altermeta`, `u-resetmeta` (`vary-meta` keeps the `IObj` cast — `u-varymeta`; `set-validator!` takes the separate `watched_or_nil` path and admits atoms/agents/refs — §1e) |
| `as_iobj` (`with-meta`) | none (generic `IObj` cast) | `r-with-meta` |
| print: `ref_token` (+ `TOK_REF`), cells arm, engine body | NEW arms: the transient `#object[clojure.lang.Ref 0x{id:x} "clojure.lang.Ref@{id:x}"]` convention (PINNED DIVERGENCE from the JVM's `{:status :ready, :val …}` rendering — same rationale as atoms, §2f) | `r-new` (divergence record) |
| `jvm_class_of_host` | NEW arm → `clojure.lang.Ref` | every `ClassCastException` row naming Ref |
| `native_await` / `native_await_for` | NEW tx gate FIRST (before the in-action check): `IllegalStateException` `await in transaction` / `await-for in transaction` — even with zero agents | `t-await-in-dosync`, `t-awaitfor-in-dosync`, `t-await-action-tx`, `t-awaitfor-action-tx` (tx message wins), `t-await-noargs-in`, `t-await-noargs-out` (control: nil) |

### 1e. Validators and watches fire AT COMMIT

| probe id | pins (port matches the oracle row) |
|---|---|
| `t-validator-calls` | the validator runs on the initial value at construction AND once at commit on the FINAL value only (`[0 2]` — the intermediate `1` is never validated) |
| `r-validator-reject` / `e-validator-msg` | a veto aborts the commit with `IllegalStateException` `Invalid reference state`; nothing is installed |
| `t-setvalidator-in-txn` | in-txn install governs that same commit (the oracle's commit veto): `set-validator!` admits refs (S5 checkcast extended); install validates the tx-local value like `deref` and writes the slot non-transactionally (JVM `ARef.setValidator`: plain volatile write — survives an abort, `v-ref-abort-keeps`) |
| `t-watch-commit` | watches fire ONCE per commit with (old committed, new committed): `[[0 2]]` |
| `t-watch-abort` | aborted transactions fire no watches (`[]`) |
| `t-addwatch-in-txn` | a watch installed in-txn fires at that commit (`[[0 1]]`) |
| `t-nested-join` | joining nests fire once total |
| `t-watch-order*` / `t-watch-shuffled*` / `t-watch-keys4/8` / `t-watch-strkeys` / `t-watch-mixedkeys` / `t-watch-samefn` / `t-key-hashes` | the JVM order is hash-trie order (insertion-independent, key-determined, non-monotonic in hash — §2d). The port fires watches in INSERTION order (port atom convention) — PINNED DIVERGENCE |

Commit order across refs (validate + notify) is ascending
Ref id — port-defined and deterministic (the JVM order is
hash-trie and unpinned for multi-ref commits). Nil watch
fns install but never fire (atom convention).

### 1f. `send` deferral (the `concurrency.rs` behavior change)

`send`, `send-off`, AND `send-via` (shared `send_action` —
consistent by construction) issued inside a running
transaction validate EAGERLY (arity + agent cast —
`t-send-badagent-in-txn` throws immediately), enqueue into
the transaction's outbox, and answer the agent
(`t-send-returns-agent`). On successful commit the outbox
dispatches IN ORDER, AFTER watches fire (§1g step order —
watches-then-sends is port-defined; the JVM relative order
is unpinned). The deferred action observes COMMITTED state
(`t-send-defer` / `t-sendoff-defer` / `t-sendvia-defer` all
`[5 5]` — the action fn derefs the committed `5`, which
PROVES post-commit dispatch). A failed attempt (body throw,
veto, conflict retry) DISCARDS its outbox: only the
winning attempt's sends dispatch, and an aborted
transaction dispatches none (`t-send-abort` → `[:caught
[0 0]]`).

Outside a transaction, `send_action` is byte-identical:
the deferral branch is conditioned ONLY on the tx field
being `Some` (§2f proof strategy).

### 1g. Transaction context, retry, barge bound, MVCC algorithm

**Context.** The running transaction lives in an
`Evaluator` field (`tx: Option<Transaction>`), NEVER in
`Bindings` (§1h). `Transaction`: `start_point: u64` (the
commit clock at begin), `depth: u32` (nesting), `reads:
{refid → point}`, `writes: {refid → value}`, `commutes:
[(refid, fun, args)]` (issue order), `commuted: {refid}`
(the §1c set-guard), `ensures: {refid}`, `outbox:
[(agent, item)]` (issue order).

**Clock and registries.** The commit clock is a
process-global `AtomicU64`: refs are `Send + Sync` values
that cross evaluator lineages (agent state, promise
delivery), so a per-lineage clock would corrupt snapshot
isolation. Active-reader start points live in a global
`Mutex<Vec<u64>>` (pushed at begin, removed at end) for
the trim-retain rule below.

**Read path** (`deref`, `ensure`, invoke, commute/alter
base): own write (incl. in-txn commute applications) wins;
else the newest committed version with `point ≤
start_point`; record `reads[refid]` (first read wins).
Resolving with NO version `≤ start_point` (only possible
under contention after a force-trim) raises internal
`Retry`.

**Commit path** (outermost `sync` exit only; nested exits
just decrement depth):

1. Empty writes + commutes + ensures → return the body
   value with NO clock bump (read-only fast path;
   unobservable, avoids contention).
2. Re-run each ref's commutes IN ISSUE ORDER over (latest
   committed ⊕ this txn's earlier non-commute writes to
   that ref) (§1c composition) — lock-free, before any
   guard is held. A commute recorded DURING the re-run
   applies tx-locally but never reruns that commit
   (port-defined: the JVM fail-fasts
   `ConcurrentModificationException` — oracle
   `u-latecommute-cme`; pinned in `stm_portside_pins`).
3. Validate every written ref's final value (a veto ABORTS
   with `Invalid reference state` — never retries) —
   likewise lock-free. User code must never run under held
   guards: a validator (or rerun fn) resolving a locked
   target would block on its own transaction's guard
   (single-threaded self-deadlock), or a concurrent
   committer's (cross deadlock).
4. Lock the write-set + ensure-set refs in ASCENDING Ref-id
   order: spin phase first (at most `MAX_SPIN_RETRIES =
   64` full `try_lock` rounds over the set, releasing all
   on any failure), then the BARGE phase (blocking
   acquisition in the same ascending order — deadlock-free
   by the global order, since the user-code steps above
   hold nothing). §2c justifies the bound. The target set
   snapshots here, so rerun- and validator-phase joins
   are locked members.
5. Revalidate: every `reads` entry's current point still
   equals the recorded point (else drop locks, `Retry`).
   Ensured refs are covered by the same check (their
   read-locking), and the rerun base pins through the
   reads its first commute recorded (a base that shifted
   under the lock-free rerun retries like any conflict).
   Read-only refs try briefly — a held cell retries
   rather than blocking into a lock cycle.
6. Bump the clock ONCE; install every new current with the
   fresh point; push each old current onto its history.
7. Trim each touched history: drop oldest while `len >
   min_history`, EXCEPT stop when `len ≤ max_history` AND
   the oldest is the sole remaining version `≤ S` for some
   OTHER active reader's start point `S` (then it is
   retained). Past `max_history` the drop is forced and
   the pinned reader retries on next access. (With no
   other active readers this is exactly trim-to-min —
   every `r-history-*` row.)
8. Pop the tx (drop locks BEFORE running user code —
   reruns and validators ran lock-free at steps 2–3;
   watches and sends run unlocked like the JVM).
9. Fire watches in ascending Ref-id order, each ref's
   watches in insertion order (§1e).
10. Dispatch the outbox in issue order through the normal
    `enqueue` path (§1f).
11. Answer the body value.

Body throws, `Retry` exhaustion (none — retries are
unbounded in COUNT like the JVM; only the SPIN phase is
bounded), and vetoes all discard the attempt's outbox and
reads/writes; `Retry` re-runs the body with a FRESH
`start_point` (re-read the clock per attempt). Retry is
user-invisible: no throwable escapes for conflicts
(`t-retry-invisible`).

**Loom test plan** (same budget discipline as
`loom_var_lock.rs`: 2 threads, bounded steps, models in
`docs/t1174/models/` IF the harness needs a new TLA
config — reuse the existing agent-queue style otherwise):

- `stm_racing_increments_commit`: mirror the commit lock
  acquisition (ascending-id try_lock rounds + barge with
  the bound as a MODEL PARAMETER set to 2, so both paths
  run on every schedule) around a shared counter; assert
  both txns commit (counter hits 2 on every schedule —
  no lost update, no deadlock).
- `stm_ensure_blocks_write_skew`: tx A ensures R then
  writes S; tx B writes R; assert A either commits with
  B's write visible-after or retries — i.e. no schedule
  commits A blind to B (the revalidation check mirrored
  branch-for-branch).
- The mirror tracks the real commit steps 4–5
  branch-for-branch; drift is a bug (same header contract
  as `loom_var_lock.rs`).

### 1h. `bound-fn` conveyance (mechanism proof)

The transaction context MUST live outside `Bindings`, and
that placement PROVES both conveyance behaviors by
mechanism (`env.rs` + `eval.rs`):

- SAME-thread `bound-fn` invocation sees the running
  transaction (`t-bound-fn` → `1`): `apply_closure`
  (`eval.rs`) re-pushes conveyed bindings with
  `push_frame` / `truncate` around the call and touches
  NOTHING else on the evaluator — an `Evaluator`-level tx
  field trivially survives. (`bound-fn*` snapshots only
  `clone_bindings` — `fx4_host.rs` — so it cannot disturb
  the tx either.)
- CROSS-thread conveyance (futures, agent actions,
  `send_action`'s `fork_for_thread`) NEVER carries the
  transaction (`t-future-no-convey` → `:no-tx`):
  `fork_for_thread` takes ONLY `Bindings` and constructs
  every other field fresh — a tx field arrives `None`,
  matching the JVM (where `LockingTransaction` is a
  thread-local the binding conveyor does not copy).

Storing the tx INSIDE `Bindings` would be WRONG both ways:
`apply_closure` replays only `pairs()` (flattened
`(Var, Value)` — a hidden field would not survive), and
`fork_for_thread` would leak open transactions across
threads. `get-thread-bindings` never shows the tx (it is
not a var binding).

### 1i. BDD scenarios + row flips (covering-test plan; no matrix edits)

Issue BDD, each with its covering test in
`rust/crates/clj-eval/tests/stm.rs`:

1. `(dosync (alter r + 1) (alter s + 1))` commits both or
   neither → `atomic_commit` (`t-commit`,
   `t-atomic-abort`, `t-commute-abort-atomic`).
2. `(alter r f)` outside `dosync` throws the JVM text →
   `outside_tx_gate` (all four `e-*-outside` rows +
   check-order rows).
3. Conflicting concurrent `dosync` blocks both commit (one
   retries) → the §1g loom tests (real-thread
   stress stays OUT — loom only, per the issue's budget).
4. `send` inside `dosync` runs after commit →
   `send_deferral` (`t-send-defer*`, `t-send-abort`,
   `t-send-returns-agent`, `t-send-badagent-in-txn`).
5. `(io! :x)` inside `dosync` throws → `io_gate`
   (`t-io-in`, `t-io-out/multi/empty`, `t-sync-runs-tx`).

Remaining covering tests: `ref_ctor_opts` (§1a rows),
`history_accessors` (conversion ladder + arities),
`history_trim` (`r-history-*`), `macro_shapes`
(`t-shape-dosync` exact; `sync`/`io!` identity),
`nesting_join` (`t-nested*`), `commute_compose`
(`t-commute-*`), `stm_portside_pins` (the `u-*` rows in
`probes/portside.out`), `stm_retry_under_contention` +
`stm_revalidate_retry` (genuine conflict/revalidation
retries — port-side pins with no oracle rows),
`validator_watch_timing` (§1e rows),
`invoke_and_predicates` (§1d rows), `await_gates` (§1d
await rows), `boundfn_conveyance` (`t-bound-fn`,
`t-future-no-convey`), `print_and_str` (`r-str` + port
`#object` form), `seam_suites_green` (§2f — runs the
existing agent/future/atom assertions unmodified).

Row flips (implementer, from current `origin/master` —
§2h.1; verify line numbers at flip time):

| row (`rust/core-coverage.md`) | master line | flip to + test note |
|---|---|---|
| `ref` | 551 | `implemented`; `stm.rs ref_ctor_opts` |
| `dosync` | 552 | `implemented`; `stm.rs atomic_commit, nesting_join, macro_shapes` |
| `commute` | 553 | `implemented`; `stm.rs commute_compose` |
| `ensure` | 554 | `implemented`; `stm.rs atomic_commit` (+ loom skew test) |
| `alter` | 555 | `implemented`; `stm.rs atomic_commit, outside_tx_gate` |
| `ref-set` | 556 | `implemented`; `stm.rs atomic_commit, commute_compose` |
| `io!` | 776 | `implemented`; `stm.rs io_gate` |
| `ref-history-count` | 809 | `implemented`; `stm.rs history_accessors, history_trim` |
| `ref-max-history` | 810 | `implemented`; `stm.rs history_accessors` |
| `ref-min-history` | 811 | `implemented`; `stm.rs history_accessors` |
| `sync` | 830 | `implemented`; `stm.rs nesting_join, macro_shapes` |

Header counts are recomputed from the file at flip time
(current header: 451 implemented / 240 out of scope; +11 /
−11 → 462 / 229 — the implementer verifies, never adds
deltas to a stale base).

## Part 2 — Notes

### 2a. Derivation story

Surveyed before writing: `CONC_NATIVES` + `send_action` /
`native_deref` / `native_realized` (`concurrency.rs`),
the `Atom` cell (`atom.rs`), the `Bindings` frame +
`apply_closure` / `fork_for_thread` conveyance (`env.rs`,
`eval.rs`, `fx4_host.rs`), every `Value::Atom` match arm
in the eval crates (§1d table), and `loom_var_lock.rs` +
`docs/t1174/models/`. The oracle then overturned several
first guesses: `sync` takes `[flags & body]` with flags
unevaluated (the first `t-sync-*` probes were wrong and
rewritten); `realized?` on a ref THROWS (refs are not
`IPending`); refs are `IFn` and delegate invocation to
their value (atoms are neither); history setters return
the ref with NO constraints; `await`/`await-for` throw
inside transactions (tx check beats the in-action check);
`set-validator!` in-txn governs that commit;
alter-after-commute throws `Can't set after commute`;
commit-time watch order is hash-trie order (§2d). Each
surprise became probes before it became spec text.

### 2b. MVCC rationale (trim rule, reads, locks)

The trim rule is DERIVED, not assumed. Candidate A (trim
to min only when exceeding max) predicts count 4 after 15
default-bounded commits (1…10, trim at 11, then 4 more);
the oracle says 0 (`r-history-default-many`). Candidate B
(trim to `min_history` on every commit) predicts every
single-threaded row exactly: defaults → 0, min 3 after 8
commits → 3, min 5 after 2 commits → 2, min 0 max 1 → 0.
So B is the rule, and `max_history`'s ONLY consistent
role is the concurrent one: bound retention for pinned
readers (§1g step 7). The retain condition (sole remaining
version `≤ S` for another active reader) is the weakest
rule that keeps snapshot reads total: dropping any other
entry cannot break a reader (a newer `≤ S` version
remains), and force-dropping past `max_history` converts
to a reader `Retry`, which is exactly what the bound is
for.

Reads resolve newest-`≤ start_point` (snapshot isolation);
writes are blind (last committer wins after revalidation —
there is no write-write conflict check beyond the lock,
matching the JVM's bargain semantics for `alter`). Locks
are the cells' own mutexes in ascending-id order (no
separate lock table to drift). Validators run under the
commit locks (they must see the final values atomically);
watches and sends run after lock release (user code never
holds cell locks — the `atom.rs` short-critical-section
discipline).

### 2c. Barge bound justification

`MAX_SPIN_RETRIES = 64`, then blocking acquisition in
ascending-id order. The SHAPE mirrors the JVM (spin, then
barge by blocking); the NUMBER is port-defined: each spin
round is nanoseconds (failed `try_lock` + release), so 64
rounds absorb every realistic contention burst without a
context switch, while the bound guarantees a spinner can
never starve against barging committers — the failure mode
of unbounded spinning. The blocking phase cannot deadlock
(the global id order) and always makes progress (some
holder always commits and releases), so retries stay
unbounded in COUNT like the JVM while every individual
wait is bounded. Rejected: unbounded spinning alone
(starvation under sustained contention); immediate
blocking without a spin phase (a context switch per
contended commit — pure overhead single-threaded… which
is unobservable, but the spin phase also keeps the loom
model's fast path schedule-tight). The loom model takes
the bound as a parameter (2) so both phases run on every
schedule.

### 2d. Watch-order divergence (full evidence)

The JVM fires ref (and atom) watches in hash-trie order,
PROVEN key-determined and neither insertion nor sorted:

| probe | insertion | observed | kills |
|---|---|---|---|
| `t-watch-order` | a, b | b, a | insertion |
| `t-watch-order3` | a, b, c | c, b, a | — |
| `t-watch-order-shuffled` | b, a, c | c, b, a | reverse-insertion |
| `t-watch-shuffled2` | c, a, b | c, b, a | reverse-insertion (again) |
| `t-watch-keys4` | a, b, c, d | c, b, d, a | both sorted orders |
| `t-watch-keys8` | k1…k8 | k8,k5,k7,k6,k3,k4,k1,k2 | numeric hash sort (non-monotonic in `t-key-hashes`) |
| `t-watch-strkeys` | "a","b","c" | a, b, c | (consistent: small-string bucket order) |
| `t-watch-samefn` | a, b, c (one fn) | c, b, a | fn-identity involvement |

Only hash-trie bucket order survives. Reproducing it
needs a `PersistentHashMap` trie replica — wildly out of
proportion for an order Clojure never documents
(`add-watch` promises no order). DECISION: insertion
order, shared with the port's shipped atoms — and NOTE
the same divergence pre-exists for atoms
(`t-watch-order-atom` → `[:b :a]`, while `atom.rs` fires
insertion order): S6's `watch_rows` pins only
single-watch shapes, so multi-watch order was never
pinned port-side. Out of this slice's fence; flagged for
the owner, not fixed here.

### 2e. History-bound conversion notes

The ladder (§1a) is `RT.longCast` then checked
`long→int`, read off the oracle: doubles render Java-style
in the `IAE` message (`1.0E300` — reuse
`java_double_string`, never Rust's `{}`), bigints render
plain decimal, ratios/bigdecs truncate toward zero like
`longCast`, and the `nil` case takes the JVM's bizarre
reflection path (`Character.charValue` NPE — reproduced
as TEXT). Huge-ratio / huge-bigdec inputs follow the same
`IAE` shape by derivation (unpinned — the helper falls
out of the shared long-then-int structure). The stored
bound is always int-range, so getters never re-check.

### 2f. Seams + proof strategy

| seam | why it stays green | proof |
|---|---|---|
| agent suite | deferral adds ONE branch (`if tx.is_some()`) at the top of `send_action`'s dispatch section, AFTER the existing arity/cast validation; `None` falls through to the identical code path | existing `concurrency.rs` agent tests run UNMODIFIED in `seam_suites_green`, plus a new assertion that `send` outside `dosync` dispatches immediately (pre-deferral behavior) |
| future / promise / delay suites | untouched code paths; tx never crosses `fork_for_thread` (§1h) | same unmodified runs; `t-future-no-convey` pins the isolation |
| atom suite | untouched arms; refs reuse (never alter) `as_iref`/`as_ref_target` shapes via NEW enum variants | unmodified runs; `r-realized-atom` / `t-watch-order-atom` controls document current behavior |
| `deref` on non-refs | one NEW match arm; all existing arms byte-identical | unmodified runs + `r-deref-timeout` (falls through to the old arm) |
| `await` outside tx | the tx gate is a PRE-check returning early only when `Some` | unmodified runs + `t-await-noargs-out` control |
| print of non-refs | new arms only; the transient `#object` convention is cited from the atom path, not changed | unmodified print tests |

The port print divergence (`#object[…]` without the
deref-as-map body) additionally DODGES a cycle hazard:
a ref containing itself (or a cycle through collections)
prints finitely because the body never renders the value
— the JVM's `{:status :ready, :val …}` rendering has no
such guard to copy.

### 2g. Open risks

1. **Commit-fn complexity.** Steps 2–7 want one function
   but the complexity ratchet will not take it — the
   implementer splits per step (lock, revalidate,
   commute-rerun, validate, install-trim) with the
   narrowest signatures that keep 100% patch coverage
   honest.
2. **`(sync)` arity surfacing.** The `ArityException` for
   0-arg `sync` must surface through the analyzer's
   `SpecialOp` path — the implementer wires it through
   the established analyzer-error mapping and pins
   `t-sync-arity0` exactly.
3. **Loom model drift.** The §1g mirrors must track
   commit steps 4–5 branch-for-branch (the
   `loom_var_lock.rs` header contract); the bound-as-a-
   parameter keeps both phases under exploration.
4. **Unpinned JVM corners.** Multi-ref commit order,
   watches-vs-sends order, huge-ratio/bigdec history
   inputs, and over-`max_history` reader behavior under
   real contention are port-defined (§1e/§1f/§2e/§1g) —
   each is the smallest deterministic rule consistent
   with every pinned row.
5. **Global clock + test parallelism.** The `AtomicU64`
   is contention-free in practice (one CAS per writing
   txn); its VALUES are never user-visible, so parallel
   tests cannot observe each other through it.

### 2h. Issue-Solution deviations + follow-ups

Deviations from the issue #63 Solution, each forced by
evidence:

1. **Mirror-position row adds → flip in place.** The
   Solution assumed PR #41 positions; the rows long ago
   landed as `out-of-scope` at §1i lines — the
   implementer flips the status field plus notes and
   header counts from current `origin/master`.
2. **Watch order is a pinned divergence** (§2d), not
   JVM-identical — the JVM order is hash-trie and
   unreproducible without a trie replica.
3. **`sync` / `io!` are special forms, not interop
   expansions** (§1b) — the port has no host calls;
   `macroexpand-1` is identity for both (divergence),
   while `dosync` keeps the JVM expansion exactly.
4. **`await` / `await-for` gain tx gates** (§1d) — the
   issue never mentioned them; the oracle forced both
   texts plus the gate-first ordering.
5. **`send-via` defers too** (§1f) — the issue names
   `send`/`send-off`, but all three share `send_action`
   on both sides (`t-sendvia-defer` pins the JVM side).
6. **`max_history`'s concurrent role is port-defined**
   (§1g step 7) — no single-threaded oracle row can pin
   it; the rule is the weakest consistent with every
   row that does pin.

**Follow-ups (not this slice):** the `stm.rs` + `Ref`
kind + dosync implementation itself (owner's); the §1g
loom tests; the 11 row flips + header recompute; any
future decision on the pre-existing atom watch-order
divergence (§2d).
