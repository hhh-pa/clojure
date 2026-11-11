# I256: `clojure-rs build` compiles every snippet — Specification

Issue: hhh-pa/clojure#256. Consumer: hhh-pa/clojure-code-snippet#66
(the `clojure-rs-aot` sweep leg).

## Part 1 — User/functional specification

### 1.1 Goals

- `clojure-rs build x.clj -o x` succeeds for **every** script in
  `hhh-pa/clojure-code-snippet` (394 scripts), and `./x < input`
  prints byte-for-byte what `clojure-rs run x.clj < input` prints,
  with the same exit code, for all 1754 shared cases.
- Programs inside the existing pure subset still build to the same
  fully native binary as today (no behavior or IR change).
- Programs outside the subset build to a **mixed** binary: every
  top-level `defn` the subset planner accepts is lowered to LLVM.
  Every other top-level form is embedded and evaluated at startup
  by the linked `clj-rt` runtime, which uses the same evaluator as
  `run`, so the two cannot drift.
- `build --report` prints native-vs-embedded coverage so later
  widening (S2) is measurable per snippet.

### 1.2 Non-goals (S1)

- Lowering new forms natively (closures, maps, interop, seqs, …).
  That is S2, driven by sweep data.
- Avoiding the reader at startup. S1 embeds source text, read live
  like `run` (D4). Skipping reads for forms with no read-time
  dependencies is an S2 lever once the `clojure-rs-aot` leg shows
  reader cost.
- Cross-compilation, debug info, Windows-specific work beyond
  keeping the existing MSVC link path compiling.

### 1.3 Acceptance criteria

1. **All snippets build and pass.** A local sweep (`build` each
   snippet once, then run every case) has 0 build failures, 0
   output mismatches and 0 exit-code mismatches against the
   expected outputs. The snippet CI `clojure-rs-aot` leg (#66)
   confirms this on Linux.
2. **Pure programs are unchanged.** Every existing `build`/`jit`
   test passes unmodified, and `--emit-ir` output for subset
   programs is byte-identical to `master`.
3. **Mixed programs behave like `run`.** Top-level forms run in
   source order. `*command-line-args*`, the `clojure.main`
   thread-binding frame, the up-front `(require 'clojure.string)`,
   `*file*`, error rendering (`path:line:col: Class: message`) and
   exit codes (1 on an uncaught error) all match `run`.
4. **No source at runtime.** The built binary runs after the `.clj`
   file is deleted or moved.
5. **Coverage report.** `build --report` prints one line,
   `aot: native <n>/<m> defns, embedded <k> top-level forms`, to
   stderr. It does not affect the exit code.
6. **Quality gates.** fmt, clippy `-D warnings`, 100% line + patch
   coverage, geiger/deny/udeps/audit unchanged or justified.

### 1.4 BDD scenarios

- **Given** a snippet that reads stdin with `(slurp *in*)` and uses
  `clojure.string`, regex and `loop`/`recur`,
  **when** I run `clojure-rs build s.clj -o s`, then `./s < in.txt`,
  **then** stdout equals `clojure-rs run s.clj < in.txt` and the
  exit code is 0.
- **Given** a program whose `(defn sq [x] (* x x))` is in the subset
  and whose top-level `(println (map sq [1 2 3]))` is not,
  **when** it is built,
  **then** `sq` is lowered natively, the `println` form is embedded,
  the output is `(1 4 9)`, and `--report` says
  `native 1/1 defns, embedded 1 top-level forms`.
- **Given** a mixed program whose embedded form throws
  `(nth [] 5)` at line 3,
  **when** the binary runs,
  **then** stderr shows the same `path:3:col: ...` diagnostic as
  `run`, and the exit code is 1.
- **Given** a subset-only program,
  **when** it is built before and after this change,
  **then** the emitted IR is identical.
- **Given** a program that defines `-main` and calls
  `(-main (slurp *in*))` at top level,
  **when** the mixed binary runs,
  **then** `-main` runs exactly once (the top-level call), as
  under `run`.

## Part 2 — Technical specification

### 2.1 Current state (source of truth)

- `clj_eval::plan_source` (`aot_subset.rs`) plans the whole file on
  a throwaway evaluator. The first unsupported form fails the
  whole program (`PlanError::Subset`).
- `clj_aot::compile_str` goes plan → `lower::to_module` → object →
  link against `libcljrt.a` (`clj-rt`, which links `clj-eval` +
  `clj-value`, so the full interpreter is already in every built
  binary).
- `clj-rt::runtime` holds one booted `Evaluator` behind a global
  `Mutex`. Each `rt_*` shim locks it, calls the interpreter's own
  `NativeImpl`, and unlocks.
- Pure-subset `main` runs the tops, then calls an arity-1 `-main`
  with the argv list (T1192 FX1 D-argv).

### 2.2 Design decisions (as built)

- **D1 — per-top-level classification** (`clj_eval::plan_mixed`,
  `aot_subset/mixed.rs`). Read all forms with their char ranges
  (`Evaluator::read_forms_ranged`). Plan each top-level
  `(defn name ...)` alone on the throwaway planner, rolling back
  the planner on any failure. Every other form, and every `defn`
  that fails planning (subset *or* analyzer error), stays embedded.
- **D2 — native eligibility is closed over natives.** A native
  `defn` may call only native siblings and the 19 prims. It is
  demoted when:
  - its name is defined by more than one top-level def-like form,
    or by a failed `defn`;
  - it uses `print` (native `print` would bypass `*out*`
    bindings);
  - its form cannot hold the in-place replacement on its first
    line (see D4);
  - it calls a demoted sibling. This is iterated to a fixpoint.

  Nothing goes native when any form mentions `alter-var-root`,
  `with-redefs(-fn)`, `intern`, `ns-unmap`, `alter-meta!` or
  `(:)refer-clojure`, the ways a program can make var identity or
  prim resolution observable. The same applies when the source
  fails to read or exceeds the nesting limit. Demotion never
  changes behavior: the fallback is `run`'s own path.
- **D3 — `-main`.** The implicit argv `-main` call stays a
  pure-subset rule (unchanged). Mixed programs follow `run`, which
  never calls `-main` implicitly. Every snippet calls `-main`
  explicitly, so it runs once.
- **D4 — embedded source (masked text, not an image).** The
  embedded source is the `\r\n`-normalized program text. Each
  native `defn` is rewritten in place: its first chars become
  `(clj.aot/n K)` and every other non-newline char of the form
  becomes a space. Every embedded form keeps its line and column,
  so spans, `*file*`-relative diagnostics and read-time effects
  (ns changes, data readers, `::kw`) behave exactly as under `run`,
  because the binary reads the text live, form by form. A pre-read
  image was rejected because `run` interleaves reads with
  evaluation.
- **D5 — runtime frame.** The binary's `main` calls
  `rt_register_native(K, name, arity, trampoline)` per native, then
  returns `rt_run_program(source, path, argv)`. The runtime boots a
  fresh evaluator and installs `clj.aot/n` and `clj.aot/load`. It
  then evaluates `clj_eval::script::run_script(args,
  "(clj.aot/load)")`, the exact wrapper `cmd_run` now shares, where
  `load` is `load_text_as_file` (the body of `load-file` minus the
  file read). Failures print `script::run_diagnostic` (shared with
  `run`) and exit 1. Drainers join, and stdout is flushed
  explicitly, because C `main` returning to libc skips std's
  at-exit flush.
- **D6 — no re-entrancy deadlock, by separation.** The program
  evaluator is never the global `RUNTIME` evaluator that the prim
  shims lock. A native called from embedded code therefore enters
  the shims freely. No thread-local evaluator pointer and no
  `unsafe` aliasing are needed.
- **D7 — native fn values and errors.** `(clj.aot/n K)` installs a
  `NativeFn` named `ns/name` into the current namespace. Its closure
  arity-checks with the interpreter's `NativeError::arity` text,
  boxes clones of the args, and calls the trampoline. Natives are
  lowered in JIT return mode (a failure returns null instead of
  exiting). While a native runs, shim failures (`runtime::fail`,
  including `rt_arity_error`) are stashed in a thread-local instead
  of printed, and the wrapper returns the stashed `NativeError`.
  `try`/`catch` around native calls therefore works, and uncaught
  failures render through `run_diagnostic`.
- **D8 — CLI.** `clj_aot::build_str(.., BuildMode)` plans the pure
  subset first. On success the existing pure path runs, with IR
  byte-identical to before. On *any* plan error, `BuildMode::Mixed`
  (the CLI default) builds mixed. Reader, analyzer and nesting
  failures then surface at runtime exactly as under `run`.
  `BuildMode::Strict` (`build --strict`, and the library's
  `compile_str`) keeps the old first-failure diagnostics, which the
  existing corpus tests now pin under `--strict`. `--report` prints
  the coverage line in both modes.
- **D9 — runtime archive discovery** is unchanged
  (`--runtime-archive` or next to the binary).

### 2.2.1 Measured S1 coverage (local, 2026-11-11)

- All 394 snippets build mixed, and all 1754 cases pass through the
  built binaries (local sweep; stdout normalized like `snip test`,
  exit code 0).
- `--report` totals: 81 of 2486 top-level `defn`s go native, in 74
  of 394 scripts. Most snippet helpers use forms outside the subset
  (`str`, interop, seqs, maps), so S2 widening is where AOT speed
  comes from.

### 2.3 Slices

- **S1 (this PR):** D1–D9; local all-snippet sweep green; tests;
  geiger baseline +54 (FFI shims, trampoline GEP, boundary tests).
- **S2 (follow-up issues, measured):** native `fn`
  closures, maps/sets/keywords-as-fns, destructuring, `str` /
  `println`. Lower top-level non-`defn` forms natively when they
  fit. Pre-read forms where no read-time effects can intervene. Each follow-up
  is filed from `--report` totals plus `clojure-rs-aot` vs
  `clojure-rs` sweep ratios.

### 2.4 TDD plan

1. `aot_subset.rs` unit tests (no LLVM): `plan_mixed` classification
   and load order, demotion fixpoint (redefinition, call into a
   demoted fn, embedded-before-native definition), spans of
   embedded slices.
2. `clj-rt` unit + FFI-boundary tests: frame parity
   (`*command-line-args*`, `clojure.string` preloaded, `*file*`),
   interning, rethrow of stashed failures under `try`/`catch`, arity
   text, bad intern indices, and silent trampoline failures.
3. `clj-aot` integration tests (LLVM): the BDD scenarios above as
   build+run tests, the IR-identity test for a subset program,
   `--strict` keeps the old error, and `--report` lines.
4. Sweep check (local, not CI): scratchpad script that builds every
   snippet and diffs every case (acceptance criterion 1).

### Constraints

- `unsafe` only at the FFI boundary: the two new `abi.rs` shims,
  reclaiming a trampoline's owned return, and the IR GEP that loads
  trampoline args (geiger baseline updated with justification).
- The interpreter-only build (`--no-default-features`) is
  unaffected, because `build` stays behind the `aot` feature.

### CI impact

- New tests run in the existing LLVM-enabled test and coverage jobs.
  No workflow changes are needed in this repo.
- The snippet repo (#66) switches its clojure-rs build to the `aot`
  feature and adds the `clojure-rs-aot` leg.
