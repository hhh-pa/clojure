# I214: performant-JIT subset expansion, first four slices (spec)

Parent: [#214](https://github.com/hhh-pa/clojure/issues/214). Landed via
PR [#218](https://github.com/hhh-pa/clojure/pull/218) (`perf-214-jit-slices`).

## 1. Goal

The tree-walking interpreter is ~18x off JVM on interpreter-bound loops
(recipes dijkstra case 7: 58.4 s vs 3.16 s; profile: `Evaluator::eval`
38.8%, every other term <8%), and the AOT/JIT subset only covers the pure
core (13 prims, no `loop`/`recur`, no transients, no macro-forms), so none
of that code compiles. This spec covers the first four subset slices, in
dependency order: macro-forms + casts (A), transient prims (B),
`loop`/`recur` (C), `clj-rt` fast paths + unboxing spike (D).

Non-goal (explicit follow-up): `defn-`, `==`, leftover macros
(`if-let`/`when-let`/`case`/`for`/...), and lowering-side unboxed regions
(§5 records the proven design; codegen does not emit it yet).

## 2. Slice A: macro-forms + casts

The planner accepts `cond`/`and`/`or`/`when`/`when-not` by desugaring to
the existing `If`/`Do`/`Let` subset nodes — no new lowering, no new
runtime. The desugars carry core.clj's exact short-circuit semantics:

- `(and x ...)`: `Let`-bound nested `If`s over an anonymous slot (no user
  name can capture it); returns the deciding operand's value (falsy
  short-circuits, last operand wins). `(and)` is `true`; one operand is
  itself.
- `(or x ...)`: mirror image; `(or)` is `nil`.
- `(cond test expr ...)`: nested `If`s; `(cond)` is `nil`; `:else` needs
  no special case (any truthy test). An odd tail dies in planning with
  `Syntax error macroexpanding cond: cond requires an even number of
  forms`. Tests are never tails.
- `(when test body...)` / `(when-not ...)`: `If` over an implicit `Do`;
  body-less is `nil`. Branch tails inherit correctly (a tail `when` in a
  `loop` body keeps a branch `recur` in tail position; §4 pins this).

Macro/shadow discipline: `clojure.core/`-qualified heads are recognized; a
user `defn` or `let` binding named `cond`/`when`/etc. shadows the macro
(a shadowed call plans as a regular call or fails as higher-order, never
silently desugars). Classification lives in `classify_cond`,
`classify_when`, `classify_and_or` (`rust/crates/clj-eval/src/aot_subset.rs`).

Casts: `long`/`int` plan as `Prim::Long`/`Prim::Int` and lower through the
boxed ABI like every other closed prim (`rt_long`; `rt_int_cast` — `rt_int`
is already the integer-literal constructor).

## 3. Slice B: transient prims

`transient`/`conj!`/`assoc!`/`persistent!` plan as `Prim::Transient`/
`ConjBang`/`AssocBang`/`PersistentBang` and lower through the boxed ABI to
`clj-rt` shims that delegate to the interpreter's own transient natives
via the shared `call_prim` dispatch — zero semantic drift by construction,
including halfway states (any use of a spent transient errors exactly, as
in the interpreter). `nth`-on-transient needs no lowering change: the
shared `nth` native already reads transients.

Prim tables grow 13 → 19 with discriminants never shifting (new prims
append): `PRIM_TABLE` (name → `Prim`), `PRIM_NAMES` (discriminant → core
name for native resolution), and `prim_symbol` (discriminant → `rt_*`
symbol). All three are tables, not matches — one arm per prim would trip
the complexity gate. Pairings are pinned three ways: the `prim_symbol`
drift tests in `clj-aot`, the `prim_resolve_table` order test in `clj-rt`,
and one FFI differential test per shim (helper ≡ interpreter ≡ literal).

## 4. Slice C: `loop`/`recur`

Planner: `classify_loop` threads a `RecurCtx` (binding arity + tail flag)
through the body; children derive, never inherit blindly — `nontail()`
clears the tail flag for call heads, tests, prim args, and inits, while
`loop` inits clear the target outright. `classify_recur` fences three
shapes with plan errors (the analyzer fences them first, so these are
defense in depth): `recur` outside a `loop`, `recur` in non-tail position,
and arity mismatch against the loop bindings. Recur threads through the
§2 desugars structurally: a `recur` in a `cond`/`when`/`and` branch is a
tail exactly when the macro form is (pinned by
`recur_threads_through_macro_desugars`), while test position never is.

Lowering (`rust/crates/clj-aot/src/lower.rs`): `lower_loop` binds the
inits into flat frame slots once (like `let`), branches to a head block,
and lowers the body; a tail `recur` stores its args into the slots
(`store_slot` drops the old box first) and branches back to the head. A
`loop_stack` of head+slots targets handles nesting (innermost wins —
shadowing, never crossing). No phi nodes anywhere (alloca slots
throughout); every slot holds a valid owned box (params and slots
nil-initialized; every lowered value arrives +1 via `Local::clone`;
`lower_seq` drops non-finals).

One lowering fix belongs to this slice: `pack_args` emitted its argv
array at the current builder position, so every prim call in a loop body
reallocated stack per iteration — a 1M-iteration loop overflowed the 8 MB
stack. Argv allocas are hoisted to the entry block (dominate all uses,
allocate once per function); a 500K-iteration exact-sum JIT test pins it.

Corpus (`corpus/`, replayed by the `i214_*` tests in
`rust/tests/build_corpus.rs`): new `11-loop` case (countdown, 1..100
accumulation, rebind-order swap, nesting, `defn` countdown) replays
byte-identical across run/AOT/JIT/JVM; `12-when` graduates the old
unsupported `10-when` (interpreter == JVM 1.13.0-alpha10 byte-for-byte);
`unsupported/02-loop` keeps the still-unsupported destructuring loop
(plain-symbol loops compile). The `docs/spec/t1174` fixtures stay byte-frozen
— its `02-loop`/`10-when` files are superseded but kept by the
t1174-empty-diff rule.

## 5. Slice D: `clj-rt` fast paths + unboxing spike

Native string fast paths for `count`/`nth`: the `rt_count`/`rt_nth` shims
borrow the arg boxes, and on a string shape answer natively
(`count_str_fast`: UTF-16 units through the shared `str_utf16_len` scan
cache; `nth_str_fast`: index coerces FIRST through the shared `nth_index`
— nil-NPE quirk, tower truncation, range errors all shared — then the
shared `nth_str` lookup with scan cache, `not-found`, and
`StringIndexOutOfBounds` text); every other shape delegates to the shared
native via `prim_call`, so arity and error behavior stay shared, never
reimplemented. After a borrow-decided fast path the boxes still free
exactly once (`drop_args`). Needed only pure visibility lifts in
`coll.rs`/`core.rs` (no behavior change); the interpreter is untouched.

Recorded deltas (stderr prints, machine-dependent, never asserted):
string `count` ~305 ns vs vector-delegated ~489 ns, string `nth` ~429 ns
vs vector-delegated ~684 ns on the PR author's macOS/arm64 host.

Unboxing spike (designed, helper-proven, NOT emitted): `rt_is_int` (guard)
and `rt_unbox_int` (borrow-read) are thin shims over the safe
`is_int_flag`/`unbox_int` workers; `rt_i64_add`/`sub`/`mul` run one
checked step with an overflow out-flag (`i64_flagged`: value + `0` on
success, never-read zero + `1` on overflow — the caller then re-runs the
matching boxed helper, which reports the shared `long overflow`, since
Clojure longs never wrap). The designed region shape is
guard → unbox → raw-`i64` loop → `rt_int` re-box on exit, proven end to
end at helper level by a 1M-iteration loop-carried sum (`tests/i64.rs`:
recorded ~1.1 ns/iter unboxed vs ~152–438 ns/iter boxed on the author's
host). Lowering-side unboxed regions are the follow-up: no `clj-aot`
codegen emits guard/unbox sequences yet.

## 6. Integration and acceptance

- Unified Prim tables (13 → 19, one discriminant order), recur threading
  ported onto the restructured classifier, clippy arg-count bundling,
  complexity-test helper hoist with stale baseline entries pruned.
- Dijkstra now fails planning at `defn-` (15:1), not `loop` — the
  remaining gap is §1's follow-up list.
- Test contract: planner structural pins per desugar/fence; per-shim FFI
  differentials (helper ≡ interpreter ≡ literal, errors as `ERROR` with
  stderr bytes pinned by the S8D replay); safe-worker unit tests plus one
  boundary test per new shim; corpus replays (run/AOT/JIT/JVM); MSRV
  (`clj-eval` + `clj-rt` under 1.85) and the workspace nextest suite.
- Acceptance: full workspace suite green, clippy `-D warnings` clean,
  `cargo fmt` clean, complexity + geiger gates at their committed
  baselines, corpus 6/6 including the new `11-loop` replay.
