# I254: Bytecode tier for hot loops — Specification

- Status: slice 1 (bytecode dispatch) and the flat-scalar
  slice measured **no-go** and reverted (§1.5, §1.6); the
  32-byte `Value` (§1.7) and the unboxed loop tier (§1.8)
  landed.
- Issue: [#254](https://github.com/hhh-pa/clojure/issues/254).
  Branch: `perf/254-bytecode-tier`.
- Parents/context: #214 (performant JIT subset), #226
  (interpreter optimization sweep), #250 (landed perf stack).

## Part 1 — User/functional specification

### 1.1 Goals

1. Beat CPython across the board on the `h2h-cpu-mem` gate:
   zero instruction-count losses and zero peak-RSS losses at
   the 1.15x margin on both comparisons (`recipes` vs CPython
   and `recipes-rsrs` vs `master`), proven by an unwaived gate.
2. Preserve exact Clojure/JVM-observable semantics for every
   form the tier touches: values, errors, evaluation order,
   `recur` write-after-all-operands, last-use moves, direct-call
   redefinition behavior, transient mutation, and lazy/opaque
   subforms.
3. Keep the fallback boring: any loop body outside the bytecode
   subset executes exactly through today's fused, flat, or
   closure-tree paths in `clj-eval`.
4. Do not regress boot/compile footprint or small-case RSS.
   The remaining memory losses are marginal (about
   1.16-1.24x), so a tier that wins instructions by growing
   compiled artifacts or per-frame traffic can lose the gate.

### 1.2 Non-goals (explicit, slice 1)

- No whole-evaluator replacement and no AOT/JIT changes.
- No snippet algorithm rewrites in the runtime spike.
- No language-semantics changes, including weaker direct-site
  invalidation or different overflow/promotion behavior.
- No bytecode for straight-line function bodies until loop
  evidence says the dispatch wins there too. #250 measured
  that flat drivers for one-shot fn bodies regressed.

### 1.3 Acceptance criteria

- [ ] This spec is committed before implementation code.
- [x] Slice-1 go/no-go is recorded in this spec and on #254
  with before/after local instruction and RSS numbers for:
  empty/add loop floors, KMP/Boyer-Moore-shaped string loops,
  Dijkstra-shaped vector/transient loops, and the
  `count_strings` matrix inner loop. **No-go: see §1.5.**
- [ ] Go only if slice 1 reaches at least ~35% fewer local
  retired instructions per iteration on the representative
  loop benches without a small-case RSS increase. If it does
  not, the spike is reverted or kept behind an inert path only
  if measuring proves neutral-or-better everywhere.
  **Not met (§1.5): spike reverted, not kept inert.**
- [ ] Tests pin eligible loops, ineligible/fallback loops,
  destructuring bindings, nested loops, captured locals,
  opaque subforms (`case`, tree-evaluated escapes), errors,
  overflow/promotion misses, direct-site invalidation, and
  transient effect/value positions.
- [ ] `cargo test --workspace` passes from `rust/`; fmt,
  clippy `-D warnings`, complexity, coverage, and geiger
  gates stay green.
- [ ] Each widening slice attaches an unwaived
  `h2h-cpu-mem` dispatch and a stem diff against the
  16 MEM + 28 INS baseline from run 42749500221.
- [ ] Final: unwaived gate proves 0 MEM and 0 INS on both
  comparisons; only then remove the `waive-recipes`
  dependency for this work.

### 1.4 BDD scenarios

- Given `(loop [i 0 acc 0] (if (< i n) (recur (inc i)
  (+ acc i)) acc))`, when the tier is eligible, then the
  result, overflow behavior, and error behavior are identical
  to the flat/fused interpreter for small, boundary, and
  overflow-adjacent inputs.
- Given a loop body containing destructuring bindings,
  `try`, `case`, a tree-evaluated escape, an unsupported
  native, or a non-in-place operand shape, when compiled,
  then the loop either falls back as a whole or keeps that
  subform opaque; no partially rewritten semantics are
  observable.
- Given a hot loop calls a direct-linked core var that is
  redefined by an opaque call inside the loop, when later
  iterations execute, then they observe the same redefinition
  behavior as `Evaluator::site_direct_valid` gives today.
- Given a tiny script, when run under the bytecode build,
  then boot/eval output is unchanged and peak RSS does not
  grow beyond run-to-run noise.

### 1.5 Slice-1 go/no-go result (recorded 2026-11-10): NO-GO

A working spike was built on this branch (whole-loop compile
attempt with `body_mark` rewind, `Bc` dispatch over `BcCall`
fn-pointer ops reusing `vm_ops` `Op*::apply`/`test`, per-call
`site_direct_valid`, specialized 1–4 arity `recur`). All
`interp_loop` tests passed and snippet outputs matched the
baseline byte-for-byte (md5). It loses on instructions in
every placement, so per §1.3 it was reverted in full; no
inert path is kept. Local macOS retired instructions
(directional only; Linux Pin is authoritative), baseline is
`6d4d5217` (post-#250 master):

| Bench | Base | Bytecode-first | Fused-first |
| --- | --- | --- | --- |
| empty floor | 27.30B | 34.38B (+26%) | = base |
| add floor | 41.68B | 49.96B (+20%) | = base |
| vec1 floor | 67.99B | 122.58B (+80%) | = base |
| dijkstra | 2.068B | 2.471B (+19%) | 2.473B (+20%) |
| kmp | 1.812B | 1.965B (+8%) | 1.814B (+0.1%) |
| count_strings | 517.1M | — | 571.0M (+10%) |

Peak RSS was unchanged within noise everywhere (floors
~4.1 MB, dijkstra 23.2→22.8 MB, kmp ~11.0 MB,
count_strings 5.19→5.21 MB), so the artifact budget was not
the problem. The `count_strings` row uses a scaled input
(80-char literal, `l = 10^12`); the earlier `cs_mid` input
does not finish in 30 min on the baseline and is unusable
as a bench.

Why it loses:

- The fused/flat leaves are monomorphized shape closures
  that inline `Op*::apply` and read operands in place. The
  spike's fn-pointer op layer adds one non-inlinable call
  per op plus a per-instruction dispatch on top.
- Nested computed operands round-trip through 64-byte
  `Value` temp registers, where closures keep scalars in
  Rust locals; `vec1` (nested `rem`/`nth`/`+`) shows the
  cost most (+80%). §2.2 already banned this decomposition;
  the spike confirms the ban is load-bearing.
- On the floors the ceiling for any dispatch tier is fused
  parity: the fused empty loop is already two inlined calls
  per iteration (~227 ins). An enum-opcode variant of §2.2
  removes the fn-pointer layer but still pays a dispatch
  per instruction, so the −35% go criterion is not
  reachable by dispatch restructuring alone.

Consequence for #254's goal: the remaining gate losses are
not dispatch-bound at this tier granularity. The levers with
headroom left are unboxed loop state (int loop variables
promoted to Rust locals across iterations in the fused tier,
with overflow deopt), per-op intrinsic cost (`nth`, string
indexing, transient ops), and the small-case RSS footprint.
A future bytecode attempt should start from superinstruction
patterns (test+step+recur as one unboxed instruction), not
from a general dispatch loop.

### 1.6 Post-#250 levers and flat-scalar slice: measured

Lever 1 (unboxed-int stretch for fused loops, `f600cc38`) and
lever 2 (borrowed sort reads + index sort, `27bb6da3`,
`e2db802b`) landed and hold their local wins (floors −60%,
trie sort −13–15% locally). On the unwaived gate
([42795315827](https://github.com/hhh-pa/clojure/actions/runs/42795315827))
they are INS-neutral (28 losses, same-run rsrs median 1.0001,
rsrs 0+0) and cost ~+1.2% median RSS, flipping seven
knife-edge MEM cases (16 → 23): the gate stems' hot loops are
flat-tier (`let`/effects/`nth`), not fused-shaped.

The flat-tier follow-up — a scalar step program shadowing the
flat `Step` list (per-register `Int`/`Char`/`Bool` cache
mirrored to the frame, `nth`/`count`/casts, transient
`assoc!`/`conj!`/`pop!` effects, per-step deopt to
`run_flat_from`) — was then built and measured. Semantics
matched the base binary on every probe (overflow and
out-of-bounds after an in-iteration effect, frozen transient,
site redefinition, nested-loop boxed steps). Performance is
a **no-go**, worse than the bytecode spike: on an isolated
matrix inner loop (200M iterations) the scalar driver retires
438B instructions against the boxed flat driver's 213B
(2.06×); `count_strings` +57–70%, `dijkstrashortreach` +16%,
KMP neutral. Two evaluator shapes (recursive tree walk,
lowered postfix stack program) both lose: per-node generic
dispatch plus stack/cache traffic costs more than the
monomorphized shape closures it replaces, exactly as §1.5
found for the fused tier. The slice was reverted in full
(patch kept outside the tree); the branch stays at the
lever-1/2 state.

Remaining direction, unchanged: beat the shape closures at
their own granularity (wider fused shapes with unboxed state
across the whole body, superinstructions), or cut per-op
intrinsic cost and RSS footprint. A general interpreter loop
over the flat skeleton is now measured twice and excluded.

### 1.7 Value-size slice: 64 → 32 bytes (landed)

Both no-go slices fingered the 64-byte `Value` temp traffic,
so the next slice halved the enum: every payload over 24
bytes became a one-word `Arc` handle (`Symbol`, `Keyword`,
`List`, `Vector`, `Map`, `Set`, `Array`, `PrimitiveVector`
wrap an `Arc<…Data>`; `Value::Throwable` rides an
`Arc<Throwable>`; `Var`/`Unbound` shrink through `Symbol`).
Writers go through `Arc::make_mut` on the handle data first,
so aliasing and minted-id/entries-`Arc` identity rules are
exactly the old struct copy's; `Debug` output is unchanged
(pinned in `value_size.rs`, whose budget is now 32).

Local macOS A/B vs the pre-slice branch binary (identical
stdout md5 on every run): floors empty/add/vec1 −0.7% /
−0.4% / −7.4% INS with boot peak 4.11 → 3.82 MB; stems —
dijkstrashortreach −6.0% INS, kmp −6.0%, boyer_moore −5.3%,
count_strings −5.4% INS and −13.8% peak RSS, trie −6.5% INS
and −35.1% peak RSS (228.7 → 148.4 MB), knapsack −4.2%;
tiny-program peak RSS 3.49 → 3.26 MB. Semantics battery
(identity/aliasing/meta/transients/exceptions/sorts)
byte-identical old-vs-new. Gate verdict: MEM losses 22 → 0
(run 42839070153; INS 28 → 25).

### 1.8 Unboxed loop tier (`vm_scalar`, landed)

The gate's string loops (KMP and Boyer-Moore tables and scans)
and Dijkstra's heap and edge loops are flat-tier loops of
`nth`/compare/int ops, where each step pays a boxed `Value`
and a closure call (~80 instructions for a literal `let`,
~190 for a string `nth`). §1.5 and §1.6 showed that a dispatch
loop over the same boxed skeleton cannot win. This slice
removes the boxing instead, across the whole body.

- **Builder.** `vm_scalar::compile` builds a tree from the
  loop's `if`/`let`/`and`/`or`/nested-`loop` forms. Its
  scalars are `Int`/`Char`/`Bool` values carried as tagged
  `Sv` slots; it handles one- and two-argument int ops (and
  the n-ary folds of `+ - * min max bit-and/or/xor`),
  `int`/`long`, `nth`/`count` over invariant strings and
  (transient) vectors, `=`/`not=`/comparisons/`zero?`/`pos?`/
  `neg?`/`not`, and `assoc!`/`conj!` on transient vectors at
  a `recur` or exit. Anything else becomes a miss node.
- **Program.** The tree lowers to a flat register program:
  slot-to-slot steps, branches, nested loops as inline code,
  and constants and invariants loaded from a template at
  entry. One dispatch loop runs it (`exec`, out of line).
- **Exact behavior.** Every op answers exactly what its boxed
  op answers, or misses (overflow, a non-scalar, an
  out-of-range index, a redefined site, a moved direct
  snapshot). An iteration's effects run only after all of its
  values have computed and every effect target has checked
  out. A miss therefore writes the iteration's starting state
  back and resumes the boxed tiers, which re-run that
  iteration whole. One effect with collection-reading
  operands is allowed (they evaluate before it on either
  road); `recur`/exit values may not read collections when the
  leaf has effects.
- **Sites.** A loop's sites validate as one group, cached per
  evaluator in `vm_direct`, at entry; each iteration then only
  compares the direct snapshot.
- **Short loops.** After 32 entries, a loop averaging under
  two iterations per entry stops entering the tier: for 0–1
  trips, setup costs more than the boxed iterations it would
  replace. Measured on `find-edge`-shaped calls: with the
  give-up rule, 0- and 1-trip loops cost the same as before
  the tier, and 2-trip loops are 3% cheaper.

Local macOS retired instructions (pre-tier `c70215bf` build →
tier):

| Shape | Before | After |
| --- | --- | --- |
| KMP scan step | 675 | ~313 |
| BM inner compare | 723 | ~336 |
| BM table build step | 812 | ~352 |
| flat `let` + `recur` | 342 | ~172 |
| kmp tc11–15 (whole run) | — | −47..−48% |
| boyer_moore tc10–15 | — | −45..−47% |
| dijkstrashortreach tc3–7 | — | −15..−17% |
| suffix_tree tc9 | — | −10% |

Full local sweep: 1562 gate cases, 0 wrong outputs, total
instructions −15.5%, median peak RSS +0.4%. The semantics
battery is `tests/scalar_loop.rs`: 90 rows, each checked
against JVM Clojure 1.12 and run on both the VM and the tree
evaluator.

## Part 2 — Technical specification

### 2.1 Current execution tiers (source of truth)

`rust/crates/clj-eval/src/vm.rs` already has three loop
execution shapes behind `LoopCode`:

- closure tree: `compile_body` builds nested `Exec` closures;
  `run_loop` absorbs `Signal::Recur` and re-unpacks
  destructured bindings.
- fused: `try_fused_loop` / `run_fused` handle a single-`if`
  body with one terminal `recur`, using `Test` + `Operand`s.
- flat: `compile_flat_loop` / `run_flat` compile the
  `let`/`if`/`do` skeleton to `Step::{Eval,Set,Unpack,
  JumpUnless,Jump,Recur,Exit}`; leaf forms are still ordinary
  `Exec`/`Test`/`Operand` values compiled in exactly the
  generic allocation order.

Specialized leaves live in `rust/crates/clj-eval/src/vm_ops.rs`.
`Ctor::{One,Two,Three,N}` builds shape-specialized closures:
operands are classified by `shape` into `Reg`, `Up`, `Kst`,
or `Exec`; `ss`/`s1`/`sss` read in-place operands after
`Evaluator::site_direct_valid`, try the inline `Op*::apply`,
then fall back through `native*`, which re-validates and
re-resolves invalid sites. This is the semantic contract the
bytecode tier must preserve per direct call site.

### 2.2 Bytecode tier shape (slice 1)

Add a fourth `LoopCode` shape, attempted only where today a
loop would take fused or flat execution and only when the
whole body stays inside the slice-1 subset:

- `ByteLoop { code: Box<[Bc]>, sites: Box<[Site]>,
  consts: Box<[Value]> }` or an equivalent compact layout.
  Instructions name destination/source registers directly;
  constants are interned in `consts`; direct-call sites are
  interned in `sites` so invalidation checks stay per call.
- Instruction families:
  - control: `Jump`, `JumpUnless*` fused with common test ops
    (`<`, `<=`, `=`, `zero?`, `pos?`, etc.), `Recur`
    write-after-all, `Exit` for opaque tail code.
  - moves/loads: `Move`, `LoadConst`, `LoadUp` only if needed
    by eligible bodies; no temp decomposition for computed
    operands in slice 1 (computed operands make the call/form
    opaque or the loop ineligible, per §2.3).
  - int ops: `Inc`, `Dec`, `Add`, `Sub`, `Mul`, `Quot`,
    `Rem`, comparisons, with the same checked-inline-then-
    native behavior as `vm_ops` (`Add::apply` etc.).
  - collection ops: `Nth`/`Get` (2- and 3-arg), `Count`,
    `AssocBang`, `ConjBang`, `PopBang` in value and effect
    positions, reusing the existing `Op*::apply` /
    `apply_mut` implementations or wrappers with identical
    hit/miss behavior.
  - string ops only as far as `Nth`/`Eq` already cover them in
    slice 1; no new string representation.
- Execution is one Rust dispatch loop over `Bc` values. It
  must avoid: per-op `Box<dyn Fn>` calls, `Step`+flow double
  dispatch, `Operand::eval` enum traffic for in-place
  operands, and `Value` clones for scalar reads/writes.
  Registers are the existing `Frame.regs`; `set_reg`
  semantics (scalar forget) are preserved.

### 2.3 Eligibility and fallback

Slice 1 is whole-loop gated. A loop is eligible only if:

- bindings are plain symbols (no destructuring/unpacks);
- the skeleton is `let`/`if`/`do` with terminal `recur`s of
  loop arity, mirroring `flat_tail`/`flat_stmt` order;
- every value/test/effect leaf is either (a) a direct call
  with a bytecode op whose operands are all in-place
  (`Reg`/`Up`/`Kst` after the same `operand(..., false)`
  discipline `in_place_operands` uses), (b) a local/constant,
  or (c) an opaque compiled `Exec` in statement/tail
  positions where the flat tier already keeps it opaque;
- recur operands follow `operand(..., true)` exactly; if a
  recur operand needs a computed temp that is not itself an
  eligible bytecode op writing a fresh temp, the loop is
  ineligible rather than decomposed speculatively.

If anything is outside the subset, compilation must not have
mutated allocator state in a way that changes the fallback:
either pre-scan eligibility before compiling leaves, or
compile in the same order and discard the bytecode while
keeping the identical register allocation the generic path
would have produced. The implementation must state which
strategy it uses and test it with last-use/move-sensitive
loops (swaps, captured frames, nested loops).

Opaque leaves may still raise `Signal::Recur`: bytecode
execution absorbs empty and value-carrying recurs exactly
like `run_flat`/`run_loop` (`absorb_recur_values`).

### 2.4 Direct-site validity

Each bytecode direct op carries a `vm_ops::Site`. On every
execution it calls `site_direct_valid` before using the
inline op, with the same before-computed-operand snapshot
rule the `xs`/`sx`/`xx` closures use. On a miss or invalid
site it calls the same native fallbacks (`native1`/`native2`/
`native3`/`native`) so redefinition, arity, overflow
promotion (for `+'`/`-'`/`*'` spelled with plain ops), and
error messages are unchanged. Test ops return `bool` through
the same `AsTest` conversions as `test_ctor` where they are
fused into jumps; unfused tests may remain `Test` closures
in slice 1 only if measurements justify keeping them.

`vm_ops` may need small `pub(crate)` eval helpers or op-kind
tables for bytecode. Any parallel table must be pinned
against `UNARY`/`BINARY`/`TERNARY`/`NARY` by a test so the
bytecode op set cannot silently drift from the closure op
set. N-ary folds/chains stay opaque in slice 1 unless a
dedicated bytecode sequence proves faster.

### 2.5 RSS / artifact budget

- No new unsafe code; geiger count must not rise.
- Compiled bytecode for a loop should be no larger in live
  heap than the flat `Box<[Step]>` + leaf closures it
  replaces for the same body, or the slice must record the
  delta and show gate RSS still improves.
- No per-iteration allocation in the dispatch loop except
  allocations the executed Clojure code itself performs.
- Boot must not eagerly bytecode-compile cold code in a way
  that raises the small-case floor; eligibility checks stay
  at the existing compile points.

### 2.6 TDD plan

1. Failing tests first in `rust/crates/clj-eval/tests/
   interp_loop.rs` (or a new `bytecode_loop.rs` if the file
   shape fits better): result pins for eligible loops
   (arith, string `nth`, vector `nth`, transient build/mutate,
   wide frames, swaps), fallback pins (destructuring, `case`
   tail recur, `binding` escape, unsupported op, computed
   operand), error pins (nth out of bounds, bad transient,
   arithmetic on non-numbers), overflow/promotion pins, and
   direct-site redefinition observed across iterations.
2. Unit tests in `vm.rs`/`vm_ops.rs` for op-table parity,
   eligibility rejection reasons that are observable through
   behavior, jump patching, and recur write ordering.
3. Local benches before/after on the same binary where
   possible: `/tmp` scratch benches are evidence only; any
   durable regression bound belongs in `interp_loop.rs` in
   the existing style.
4. Full local gates from `rust/`: `cargo fmt --check`,
   clippy `-D warnings`, `cargo test --workspace`, coverage
   for touched files, complexity via
   `rust-code-analysis-cli` against
   `rust/complexity-baseline.json`.
5. Unwaived `h2h-cpu-mem` dispatch per widening slice; stem
   diff against run 42749500221 (16 MEM + 28 INS).

### Constraints

EPL-1.0 headers on new Rust files; MSRV 1.85 (no
let-chains); signed commits; work runs from `rust/` on
macOS; Linux Pin gate is authoritative for instruction
counts; local macOS numbers are directional only.

### CI impact

No workflow changes for slice 1. Existing `rust.yml`
quality gates and `h2h-cpu-mem.yml` judge the branch. The
final unwaive is a separate, later change once the gate is
clean unwaived.
