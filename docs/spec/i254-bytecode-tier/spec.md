# I254: Bytecode tier for hot loops — Specification

- Status: slice 1 (bytecode dispatch) and the flat-scalar
  slice measured **no-go** and reverted (§1.5, §1.6); the
  32-byte `Value` (§1.7), the unboxed loop tier (§1.8), its
  inlined calls (§1.9), and effects in source order with an
  undo log (§1.10) landed.
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

### 1.9 Inlined calls, conditional effects and effect tails (landed)

(The effect rules below kept §1.8's order by moving work: tests
and temps above effects, late re-reads. §1.10 replaced them with
effects in source order and an undo log; inlining stands.)

After §1.8 the gate's tightest INS case was suffix_tree tc9
(1.127x). Its Ukkonen build loop — about 149k iterations and
80% of the run — never entered the unboxed tier and ran on
the flat tier at ~7.9k instructions an iteration. It needed
four things the builder did not lower: a call to the
top-level `find-edge`; `(when (not= last-new -1) (assoc! …))`
effects; a `let`/`if` tree between the effects and the
`recur`s; and a `recur` operand, `(nth link-v active)`, that
reads a transient the iteration's effects just wrote.

- **Inlined calls.** A call through a var that holds a VM
  closure inlines into the program. The closure must have no
  conveyed bindings, and its fixed arity for the arg count must
  bind symbols only — the arity `fixed_target` would run. Params
  bind as slots; an outer local passes as itself, so a
  collection param reads it in place. The body lowers in the
  callee's own lexical frame, where a captured local or the fn's
  own name misses.
  - The program enters only while each inlined var has no thread
    binding and still holds the closure (the direct-site check,
    cached with the site group).
  - It also enters only while the calls it stands for fit under
    the call-depth limit, because an inlined call skips
    `guarded_call`'s check.
  - A fn never inlines into its own inlining, so recursion stays
    a real call. Nesting stops at 4, and one loop inlines at most
    16 calls; unbounded, the recursive `splay` cost splay_tree
    +10% load time.
- **Conditional effects.** `(when test eff…)` and `when-not`
  in effect position. The test computes with the iteration's
  values, and the guarded effects and their operands run only
  under it.
- **Effect tails.** Effects may be followed by a `let`/`if`/`do`
  tree ending in `recur`s or exits. The tree's tests and temps
  compute before the effects, which is allowed only when none
  reads a collection, and every leaf runs the effects first.
- **Late reads.** A `recur` operand that is an `nth`/`count`
  whose own operands read no collection reads after the
  iteration's effects, as the source orders. It also reads
  before them, which checks it. Effects only store scalars
  inside vectors or append to them, so the second read cannot
  miss.
- **Cheaper steps.** `nth` over vectors and transient vectors
  reads the item in place (`read_at`: no `Value` clone or
  drop). Batched `assoc!`/`conj!` of ints keep packed buffers
  packed. A program's collections no longer spill to the heap
  at entry.

Every miss keeps §1.8's contract: nothing observable happens
before an iteration's effects, and a miss hands the iteration
back to the boxed tiers whole.

Local macOS retired instructions (`21e8ff4d` → this slice):

| Cases | Before | After |
| --- | --- | --- |
| suffix_tree tc9 | 1462M | 973M (−33%) |
| suffix_tree tc4–8 | — | −32..−39% |
| z_function tc9–15 | — | −20..−35% |
| dijkstrashortreach tc3–7 | — | −4..−5% |
| kmp tc10–15, boyer_moore tc10–15 | — | −2..−3% |
| suffix_array tc4–9 | — | +0.8..+1.7% |

suffix_array is the one loss: its last inner loop (a `when`
effect plus a late read) now enters the tier, averaging two
iterations an entry, where entry costs about what the
iterations save. Its CPython ratio stays near 0.3.

Full local sweep: 1562 gate cases, 0 wrong outputs, total
instructions −5.3%. `tests/scalar_loop.rs` now holds 115
rows plus 15 setup rows that call top-level fns, every one
checked against JVM Clojure 1.12 and run on both the VM and
the tree evaluator. A separate test pins the call-depth limit
against a loop that makes the real call.

### 1.10 Effects in source order with an undo log (landed)

§1.9's effect rules kept the §1.8 contract — every value of an
iteration computes before its first effect — by moving work
around the effects. Two gate shapes stayed out of reach.
convex_hull's monotone chain pops from a transient inside a
nested loop (`(loop [] … (pop! h) (recur))`) before its outer
iteration's `conj!`, and suffix_array's construction runs effects
inside nested loops and inlined calls. convex_hull tc4 had become
the tightest INS case (1.114x on `2948ad16`).

- **Undo log.** Effects run where the source puts them: as
  statements anywhere in a body, under `if`/`when`/`let`, in
  nested loops and in inlined calls. Each logs its inverse: an
  append undoes by a pop, a replaced item is put back, a popped
  item is pushed back. The log clears at each outer iteration's
  head. A miss undoes the iteration's effects, newest first, and
  hands the iteration to the boxed tiers whole, as before. Nothing
  else can observe a transient while an iteration runs, because
  the program calls no code of its own.
- **More forms.**
  - `pop!` joins `assoc!`/`conj!`.
  - Statements may be `if`/`let` over statements, nested loops
    (value dropped), and scalar expressions computed for their
    misses.
  - `do`/`let` bodies with statements work in value position.
  - Calls run for effect inline as statements.
- **Unlogged effects.** A backward pass over the finished program
  marks every effect after which no step can miss before the
  iteration ends; those need no undo. Effects that end a body over
  a tail reading no collection run at the tail's leaves, after the
  leaf's values. A single trailing `assoc!`/`conj!` therefore runs
  unlogged, as cheaply as before.
- **Cheap logging.**
  - Undo entries are plain data: an `Int` item inline, any other
    in a side pool.
  - Runs of appends to one transient share an entry.
  - A packed-int `assoc!` swaps in place
    (`TransientVector::swap_int`).

  A logged effect costs ~20 instructions more than an unlogged
  one.
- **Heavy loggers leave.** A program leaves the tier when it fills
  the 1024-entry log, or when its iterations average more than 32
  undo entries over all its runs once 1024 iterations have run.
  Nested loops of many effects (edit-distance rows,
  insertion-sort shifts) run cheaper on their own programs. One
  long run is not judged midway: a suffix tree's breadth-first
  walk logs heavily near the root and lightly after.

Local macOS retired instructions (`2948ad16` → this slice):

| Cases | Before | After |
| --- | --- | --- |
| convex_hull tc4 | 255.1M | 207.9M (−18.5%) |
| suffix_array tc4–9 | — | −21..−31% |
| suffix_tree tc4–9 | — | −7..−13% |
| dijkstrashortreach tc3–7 | — | −8..−17% |
| knapsack tc1 | — | −31.5% |
| boyer_moore tc10–15 | — | +3..+7% |
| edit_distance tc3–5 | — | +5% |
| kmp, z_function, automata | — | +1..+4% |

The losses are the price of the new dispatch and logging paths in
loops that already ran unboxed; none of those cases is near the
gate (boyer_moore stays under 0.96x CPython). Full local sweep:
1562 gate cases, 0 wrong outputs, total instructions −3.6%.
`tests/scalar_loop.rs` now holds 138 rows plus 21 setup rows,
all checked against JVM Clojure 1.12 on the VM and the tree
evaluator. They cover every undo (append, replace, append at the
count, pop, a run of appends, a non-`Int` item, aliased
transients, a full log) and the heavy-logging give-up; mutation
checks confirmed that dropping the rollback or logging nothing
fails them.

### 1.11 Int-tuple sort, `let` destructuring and lazy compile (landed)

After §1.10 the two tightest gate margins were kruskal tc5 (1.032x
CPython, the worst `recipes` INS on `edb88162`) and
shortest_path tc8/tc0 (1.129x master, the worst `recipes-rsrs` INS:
a 23M startup-bound case).

- **Int-tuple sort.** `sort` with the default comparator over
  vectors of one length, at most 8 `Int`s each (kruskal's
  `[w u v]` edges), compares flat `i64` keys in a stable index sort
  and permutes the vectors. The order is the general lane's (a unit
  test compares the two).
- **`let` destructuring in the tier.** A `let` binding a vector of
  symbols over a collection item (`(let [[w u v] (nth edges i)] …)`)
  reads the item's fields in place (`Co::Nth2`) when each is a
  scalar. Loop bindings stay symbol-only.
- **Lazy compile.** Every eligible loop used to compile its program
  when its fn was defined: ~1.0M instructions of the dijkstra
  snippet's load, mostly for loops that run briefly or never
  (1,524M over 1,500 copies of the snippet). A loop now compiles
  once it is hot:
  - after 64 boxed `recur`s over its entries, when the rest of that
    entry runs on the program (an iteration boundary holds the
    whole state in the loop's registers, as entry does, and a miss
    hands back from where it left the state, as before);
  - or at its 32nd entry, when its boxed history (entries, trips)
    is as long as the trip rule judges: too short a trip compiles
    nothing, and the loop stops counting.

  The compile reads the loop's scope from a snapshot taken when
  the loop compiled boxed (the registers of the visible locals, the
  `letfn` names, the captures; every local the body reads has its
  place by then), and the var roots when it runs. Only a cold loop
  counts its iterations: the boxed roads are monomorphized, and the
  plain ones run the code they ran before.
  `Evaluator::set_eager_tier` compiles at first entry instead (the
  tier tests drive it with single runs).
- **Judging.** The trip rule starts from the boxed history, so it
  judges a loop's first entries, as before, however late the
  program compiles. A new rule covers what the history cannot see:
  a program whose first 32 runs all miss within 8 iterations leaves
  the tier (count_strings' matrix rows miss at their first nonzero
  cell). One long run among them keeps the program: boyer_moore's
  shift loop misses at once on most entries and runs thousands of
  iterations on a few.

Local macOS retired instructions (`edb88162` → this slice; master
for reference):

| Case | Master | Before | After |
| --- | --- | --- | --- |
| kruskal tc5 | 192.5M | 182.6M | 118.9M (−34.9%) |
| dijkstrashortreach tc8 | 45.9M | 46.2M | 45.1M |
| dijkstra snippet load ×1500 | 16,303M | 17,827M | 16,303M |
| suffix_array tc8 | 2,043M | 1,457M | 1,355M (−7.0%) |
| count_strings tc8 | 1,156M | 1,083M | 1,117M (+3.1%) |
| boyer_moore tc9 | 221.8M | 129.5M | 129.6M |

A late compile shifts which entries some judged loops sample, so a
few programs land on the other side of the trip rule (count_strings
tc8 +3.1%, suffix_array tc8 −7.0%). Full local sweep against the
eager build: 1562 cases, 0 wrong outputs, total instructions
+0.06%, median ratio 1.000. Peak RSS rose on a few macOS runs with
big growing transients (suffix_array tc7 23.8 → 30.7 MB, master
29.7 MB): the eager build with only the loop body moved behind an
`Arc` reproduces it, and a variant of the snippet peaks at 30.6 MB
on either build, so it tracks allocation layout rather than the
tier. On `edb88162` the Linux gate measured these cases at
0.94–1.01x master (18.3 MB on suffix_array tc7); the gate decides.

`tests/scalar_loop.rs` holds 155 rows and 23 setup rows run with
the eager setting, plus 12 lazy rows on the default: hand-overs
that finish and that miss on flat and fused bodies, a hot body with
no program, the 32nd-entry decision both ways, the miss rule, and
outer locals reaching the late compile through registers, captures
and the fn's own name. All are checked against JVM Clojure 1.12.

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
