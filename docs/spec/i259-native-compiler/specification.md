# I259: Compile every function natively — Specification

Issue: hhh-pa/clojure#259. Builds on #256 (mixed builds) and #258 (one
evaluator). Toolchain follow-up: #260 (LLVM 23, helper inlining).

## Part 1 — goal and acceptance criteria

Goal: `clojure-rs build` compiles **every** top-level form and every
function of a program to native code. A built binary **outperforms
`clojure-rs run`** on the snippet sweep, while printing exactly what
`run` prints.

Today the mixed build lowers only the pure 19-prim subset: 81 of 2486
snippet `defn`s. Everything else is embedded source that is read,
macroexpanded, analyzed and interpreted at startup. AOT binaries
therefore cost about the same as `run`.

Acceptance criteria:

1. **Coverage.**
   - `build --report` prints `aot: native N/N defns, embedded 0
     top-level forms` for all 394 snippets.
   - Every top-level form and every `fn`, `letfn` and `defn` arity is
     native code.
   - The only residue allowed is runtime `eval` / `load-string` of data
     built at runtime, which is inherently interpreted.
2. **Parity.**
   - All 1754 snippet cases pass through built binaries.
   - The existing mixed-build BDD corpus (stdout, stderr and exit code
     equal to `run`) passes unchanged.
   - New BDD cases cover closures, `letfn`, `try`/`catch`/`finally`,
     `binding`, `lazy-seq`, `delay`, destructuring, `recur` in `fn`,
     multi-arity/variadic fns, and deep recursion.
3. **Performance.** In the snippet sweep on CI (Linux):
   - the `clojure-rs-aot` / `clojure-rs` geomean of cpu_s and of
     instructions is **< 1.0**;
   - maxrss is **≤ 1.0**;
   - no case is worse than 1.10× `run` on instructions.

   Locally on macOS, instruction counts (min of 3) on the sample set
   show the same.
4. **No startup reader.** Built binaries do not read or analyze program
   source at startup (residue forms excepted, and reported).
5. **Gates.** fmt, clippy `-D warnings`, 100% line + patch coverage,
   complexity ratchet, and the geiger baseline (justified FFI growth
   only).

## Part 2 — technical design

### 2.1 Build pipeline

1. **Live-equivalent build-time read.** A build evaluator reads the
   program one top-level form at a time with the live reader (data
   readers, `::kw`, `*ns*` as at runtime). After each form it applies
   that form's **environment effects** so later reads and expansions
   see them. Environment forms are `ns`, `in-ns`, `require`, `use`,
   `import`, `refer`, `refer-clojure`, `alias`, `defmacro`,
   `defprotocol`, `defrecord`, `deftype`, `definterface`, `defmulti`,
   `defmethod` and `extend*`. They are evaluated at build time; all
   other forms are only macroexpanded and analyzed (no side effects at
   build time, so `slurp *in*` never runs during `build`).
2. **Analysis.** Each top-level form (after `do` splitting) is
   macroexpanded and analyzed into `Expr` with the interpreter's own
   analyzer, so semantics come from one place.
3. **Lowering.** Each top-level form becomes a native thunk, and every
   `FunExpr` arity becomes a native function (§2.3). `main` boots the
   runtime, enters `run`'s binding frame, and calls the thunks in
   order. Uncaught errors print `run_diagnostic` and exit 1, exactly as
   in #256.
4. **Residue.** A form that cannot be analyzed at build time (for
   example a macro whose expansion depends on runtime-computed values)
   keeps the #256 embedded path for that one form, counted in
   `--report`. The target on the snippets is 0.
5. **Modes.** The default build tries the pure subset, then this
   native compiler, then the #256 mixed path. `build --strict` keeps
   its frozen S8A contract: it requires the pure 19-prim subset and
   never runs the native compiler (its help text says so).

### 2.2 Value ABI

- `clj_value::Value` gets `#[repr(u8)]`. Measured: the size stays 32
  and the bytes are identical to the current layout. The repr makes the
  layout a language guarantee: tag byte at offset 0 (`Nil`=0, `Bool`=1,
  `Int`=2, `Double`=3, … in declaration order), `Bool` payload at 1,
  `Int`/`Double` payloads at 8. A unit test pins the offsets and tags,
  and the AOT compiler embeds them.
- Generated code holds Values **by value** in 32-byte, 8-aligned stack
  slots (no per-value `Box`). Ownership: a slot owns its Value, and
  every slot is dropped on every exit path.
- **Helper ABI (`rt_n_*`, `clj-rt/src/nabi.rs`).** Every helper is a
  *safe* `extern "C" fn`. Single slots arrive as references
  (`&Value` in, `&mut Value` out, `Option<&Value>` nullable), which are
  ABI-identical to the pointers generated code passes. Arrays (slot
  runs, texts, index tables, arity descriptors, argv) arrive as
  `&CSlice<T>` — a `#[repr(C)] {ptr, len}` header with private fields
  that only generated code builds (constant globals for static data,
  entry-block headers for slot runs). The two `CSlice` accessors and
  the argv `CStr` read are the whole `unsafe` boundary (geiger +20,
  justified in the baseline).
- **Inline fast paths** need no helper: truthiness (`tag != 0 &&
  !(tag == 1 && byte1 == 0)`), clone/drop of POD tags (Nil, Bool, Int,
  Double, Char), int/double construction, and int fast paths for the
  analyzer's `FastArith` direct calls (`+ - * < <= > >= = inc dec`
  with an overflow check falling back to the helper).

### 2.3 Functions, closures and calls

- **Native function ABI.**
  `i32 f(ctx*, Value* ret, const Value* args, u64 argc, const Value* captured)`
  returns 0 on success and 1 when an exception is pending in the
  runtime (pending throwable stored by the helper).
- **Closures.** `fn` creates a runtime closure object: a code table
  per arity (fixed count, variadic flag) plus a captured-values array
  computed by closure conversion of the arity bodies' free locals.
  The object is callable from the interpreter (`map`, `reduce`, …)
  and from native code, and it prints, compares and has metadata like
  interpreted fns.
- **Named fns and `letfn`.** Self and mutual references use a
  patchable captured cell, mirroring `Slots::Patchable`.
- **Calls.**
  - A `DirectNative` with `FastArith` is inlined (§2.2), and the
    covered cases of `nth`/`get`/`count`/`contains?`/`=`/`not=` and the
    int predicates/bit ops run as intrinsics.
  - Another `DirectNative` calls its native entry, resolved once per
    var (`rt_n_core_mv`).
  - Inlining, intrinsics and the resolved entries assume the program
    cannot rebind a core var. A program that mentions a rebinding
    form — `with-redefs`, `alter-var-root`, `with-redefs-fn`, `intern`,
    `ns-unmap`, `eval`, a `def` into `clojure.core`, or a var-resolving
    fn (`resolve`, `ns-resolve`, `find-var`, `requiring-resolve`,
    `ns-interns`, `ns-publics`, `ns-map`) — compiles every such call
    through the var's current root instead (`rt_n_native_mv`).
  - A call through a var holding a compiled `defn` of this program
    goes direct native→native, guarded by a cheap check that the var
    root is still that fn (the guard keeps `def`/`with-redefs`/
    `binding` semantics).
  - Everything else goes through `rt_invoke`.
- **Deep recursion.** Native frames check a stack budget (stack
  pointer vs. limit in ctx) at function entry. When it is exhausted
  they raise `StackOverflowError`, like the interpreter, so recursive
  `defn`s go native again.

### 2.4 Expressions

| Expr | lowering |
|---|---|
| `Literal` / `Quote` | constant pool slot (built once at startup by constructor helpers, no reader) → clone |
| `VecLit` / `MapLit` / `SetLit` | helper builders over native slots |
| `Global` / `VarRef` | var handle slot (interned by qualified name at startup) → deref helper with a per-thread value cache keyed by (registry epoch, binding version) — the interpreter's own validity rule |
| `Local` | native slot (frame-local, captured array, or arg) |
| `Def` / `Declare` / `DefMacro` / `Ns` | helpers that perform the same registry effect as the interpreter |
| `If` / `Do` / `Let` / `Loop` / `Recur` | native control flow; destructuring `Pattern`s through helpers (`nth`/`get`/`seq`/rest) |
| `Fn` / `LetFn` / `BoundFn` | closures (§2.3) |
| `Throw` / `NewThrowable` / `Try` | pending-exception protocol: helper failure → branch to the innermost handler (class match helper) or cleanup + return 1; `finally` on every exit |
| `Binding` / `WithRedefs` / `WithBindings` / `Locking` / `Sync` / `Io` / `WithOutStr` / `WithInStr` / `Time` / `Assert` | push/pop helpers around natively compiled bodies, popped on every exit |
| `LazySeqStar` / `DelayStar` | body compiled as a closure, wrapped by the runtime constructor |
| `CaseStar` | native switch on the helper-computed hash/int key |
| `Invoke` / `DirectNative` | §2.3 |

### 2.5 Phasing

1. **S1 — complete native lowering via generic helpers.** Coverage
   100%, parity on all 1754 cases, and no startup reader. This removes
   read and analyze from startup, the first measurable win.
2. **S2 — fast paths.** Inline int/double/truthiness, direct
   native→native calls, specialized collection helpers (vector `nth`,
   `count`, `get`, `assoc`, transients), and argument passing without
   array copies. Driven by sweep profiles until criterion 3 holds.
3. **S3 (#260) — LLVM 23 helper inlining** for the remaining helper
   overhead.

### 2.6 TDD

- `clj-eval` unit tests for every `native_abi` helper (safe Rust over
  `&mut Evaluator`).
- `clj-aot` IR snapshot tests per Expr kind.
- BDD replay (`rust/tests/native_corpus.rs`): every program in
  `docs/spec/i259-native-compiler/corpus/` runs through `jit` (the full
  native compiler, in-process) and must match `run` byte for byte —
  stdout, stderr and exit code; `err_*` programs abort with exit 1.
  The programs cover every Expr kind and the AC2 edge cases, plus
  core-var rebinding through `resolve`, var redefinition, and
  non-finite double constants.
- The local all-snippet sweep (`aotsweep.py`) and the CI sweep's
  `clojure-rs-aot` leg.

### Status (this branch)

- AC1, AC2, AC4 and AC5 hold: every snippet builds fully natively, all
  1754 cases pass, and the gates are green.
- AC3 is not met yet. Locally (macOS instructions, one case per
  script) the AOT/run geomean is 0.945 and 359/394 scripts are faster,
  but 17 scripts are > 1.10× `run` (worst: string scanning and DP
  loops, up to 2.7×). The remaining work is the slow tail: unboxed
  scalar locals, cheaper collection helpers, and #260's helper
  inlining.

### CI impact

- No new jobs; build+run tests run in the existing aot test and
  coverage jobs.
- The snippet sweep measures the result; no snippet-repo change is
  needed.
