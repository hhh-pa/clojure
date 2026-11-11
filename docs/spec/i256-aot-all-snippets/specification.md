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

- Lowering new forms natively (`loop`, closures, maps, interop, …).
  That is S2, driven by sweep data.
- Avoiding the reader at startup. S1 embeds source text. A
  pre-read image (serialized forms) is an S2 lever once the
  `clojure-rs-aot` leg shows reader cost.
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

### 2.2 Design decisions

- **D1 — per-top-level classification.** `plan_mixed(text)` reads
  all forms and plans them one at a time. A `defn` that plans
  cleanly becomes a native `SubsetFn`. Every other form (non-`defn`
  forms, and `defn`s that fail planning) becomes an
  `Embedded { text, span }` item, holding its exact source slice.
  Load order is kept as a single `Vec<TopItem>`.
- **D2 — native eligibility is closed over natives.** A native
  `defn` may only call native siblings and the 19 prims (the
  planner already enforces this). It is demoted to embedded when
  its name is (re)defined more than once in the file, when an
  earlier embedded form defines it, or when it calls a demoted fn.
  Demotion iterates to a fixpoint, so the native set is closed.
  This preserves JVM var-indirection semantics for every name a
  program could rebind (`with-redefs`/`binding` on a native var is
  out of scope: native defns are not `^:dynamic`, and direct
  sibling calls are what the existing pure build already does).
- **D3 — `-main`.** The implicit argv `-main` call happens only
  when the whole program is pure (today's contract, unchanged).
  Mixed programs follow `run`, which never calls `-main`
  implicitly. This is documented in the README AOT paragraph.
  Every snippet calls `-main` explicitly, so this is what keeps
  them from running twice.
- **D4 — embedded image.** The module gets one private constant
  holding the source path and the embedded items. Each item is a
  `(offset, line, col)` header plus UTF-8 text, preceded by a
  magic/version word (`CLJAOT01`). `main` calls
  `rt_run_embedded(image_ptr, image_len)` once per contiguous run
  of embedded items in load order. Native defns are interned
  between runs by `rt_intern_native(name, fnptr, arity)`.
  Startup-checked: a bad magic/version prints
  `internal: bad AOT image` and exits 70.
- **D5 — runtime frame.** The first `rt_run_embedded` call enters
  the same binding frame `cmd_run` builds (shared helper moved
  into `clj-eval` so `run` and `clj-rt` use one definition) and
  requires `clojure.string`. Each item is evaluated with the
  reader positioned at its recorded line/col, so spans and
  `*file*` match `run`. On error the runtime prints the
  `run_diagnostic` rendering (also shared) and returns failure.
  Generated code then exits 1 after joining drainers, as `run`
  does.
- **D6 — re-entrancy (no deadlock).** Embedded code can call a
  native defn (through its var), and that native code calls `rt_*`
  shims while the evaluator is already borrowed by
  `rt_run_embedded`. The runtime therefore tracks an *active
  evaluator* in a thread-local:
  - `rt_run_embedded` takes the lock once, publishes
    `&mut Evaluator` in the thread-local for its dynamic extent,
    and clears it on exit, including on panic (via a guard).
  - Shims use the active evaluator when one is set, and otherwise
    lock as today.
  - The `NativeFn` wrapper created by `rt_intern_native` receives
    `&mut Evaluator` from the interpreter and republishes it for
    the native call's extent.

  Single-threaded binaries still never contend. Futures and agents
  run on other threads and see no active evaluator, so they take
  the lock as today.
- **D7 — native fn values.** `rt_intern_native` wraps the C fn
  pointer in a `NativeFn` (display `ns/name`, arity-checked with
  the interpreter's own arity error text) and interns it into the
  current namespace. Boxed args are cloned in, ownership follows
  the S8 D7/D8 rules, and a null return becomes the
  already-printed error (`NativeError` carrying the rendered
  diagnostic, so the outer `run_diagnostic` does not print it
  twice).
- **D8 — CLI.** `build` tries `plan_source` first; on success it
  runs the existing path, unchanged. On `PlanError::Subset` it
  falls back to `plan_mixed`. Reader and analyzer errors still fail
  the build with the same diagnostics as today. `--report` is
  accepted in both modes. `--strict` keeps the old
  fail-on-unsupported behavior for anyone relying on it (the
  existing unsupported-form tests move under `--strict`).
- **D9 — runtime archive discovery** is unchanged
  (`--runtime-archive` or next to the binary). Release archives
  already ship it, so the snippet CI copies `libcljrt.a` next to
  `clojure-rs` from the cargo `target/release` dir.

### 2.3 Slices

- **S1 (this PR):** D1–D9; local all-snippet sweep green; tests.
- **S2 (follow-up issues, measured):** native `loop`/`recur`, `fn`
  closures, maps/sets/keywords-as-fns, destructuring, `str` /
  `println`. Lower top-level non-`defn` forms natively when they
  fit. Pre-read image (skips the reader at startup). Each follow-up
  is filed from `--report` totals plus `clojure-rs-aot` vs
  `clojure-rs` sweep ratios.

### 2.4 TDD plan

1. `aot_subset.rs` unit tests (no LLVM): `plan_mixed` classification
   and load order, demotion fixpoint (redefinition, call into a
   demoted fn, embedded-before-native definition), spans of
   embedded slices.
2. `clj-rt` unit tests: image decode (good, bad magic, truncated),
   `run_embedded` frame parity (`*command-line-args*`,
   `clojure.string` preloaded, `*file*`), error rendering equals
   `run_diagnostic`, active-evaluator re-entry (native → shim →
   embedded callback → native), and panic clears the thread-local.
3. `clj-aot` integration tests (LLVM): the BDD scenarios above as
   build+run tests, the IR-identity test for a subset program,
   `--strict` keeps the old error, and `--report` lines.
4. Sweep check (local, not CI): scratchpad script that builds every
   snippet and diffs every case (acceptance criterion 1).

### Constraints

- No `unsafe` beyond the existing `abi.rs` FFI surface. Each new
  `extern "C"` shim takes its pointer and length with the same
  documented safety contract (geiger baseline updated with
  justification).
- The interpreter-only build (`--no-default-features`) is
  unaffected, because `build` stays behind the `aot` feature.

### CI impact

- New tests run in the existing LLVM-enabled test and coverage jobs.
  No workflow changes are needed in this repo.
- The snippet repo (#66) switches its clojure-rs build to the `aot`
  feature and adds the `clojure-rs-aot` leg.
