# Gap-121 recon: LLVM JIT — just-in-time compile and execute

SPEC/RECON ONLY — no implementation. Part of #121 (ex-N5), funded
backlog epic #150, tracked under the zero-oos program (#42).

## 1. What "LLVM JIT" means for this port

Upstream JVM Clojure has no JIT API to port: it emits bytecode and
HotSpot JITs beneath it. So this is new surface, not a port. The
oracle is behavioral equivalence (same stdout / exit / diagnostics as
the AOT binaries and the interpreter), not API parity.

The port already has two execution paths:

1. **Interpreter** — `Evaluator::eval_source` / `eval_top_form`
   (`rust/crates/clj-eval/src/eval.rs`) over the full `Expr`
   (~40 variants, `rust/crates/clj-eval/src/expr.rs`). Full language,
   zero compile step. Semantically this already *is* "compile and
   execute".
2. **AOT** (`clj-aot`, S8) — `plan_source` (`aot_subset.rs`) →
   `lower::to_module` → `target::host_machine` object emission →
   `link` against the `clj-rt` archive → native binary with a C
   `main`. Subset-only: `SubsetExpr` (~15 variants) plus 13 closed
   `Prim`s that delegate to the interpreter's own natives, so
   semantics and error texts are shared, never reimplemented.

"JIT" here therefore means: **at runtime, compile (subset) source to
native code via LLVM and execute it in-process, with no user-visible
binary/link round-trip.** Consequences:

- **Ceiling is AOT-subset parity.** Lowering the full `Expr`
  (closures, protocols, STM, the FX4 macro families, …) is a
  multi-epic project, not slice 1. Anything outside `plan_source`
  keeps the interpreter-identical subset diagnostic.
- **Slice 1 buys capability, not speed.** The boxed-`ptr` ABI (every
  value an opaque box) plus prims delegating to interpreter natives
  through the process-global evaluator (`clj-rt/src/runtime.rs`)
  cap any speedup. An unboxed/performant JIT is a later epic.
  Slice-1 value: run subset programs without a toolchain round-trip
  (REPL/eval flows, embedding, single-binary distribution).
- **Precedent seams exist.** `compiler.rs` owns the `CompileDriver`
  hook / `native_compile` pattern (injected driver, LLVM-free
  `clj-eval`); the CLI owns `build` (`rust/src/bin/clojure-rs/main.rs`,
  `cmd_build`); the `aot` default feature plus the MSRV leg's
  LLVM exclusion (`rust.yml`, S9A D2) is the gating pattern any JIT
  crate must follow.

## 2. Ground facts (verified by reading; no build run)

- Pinned toolchain: inkwell 0.10.0, LLVM 22
  (`rust/crates/clj-aot/Cargo.toml`; `.github/actions/setup-llvm`
  asserts 22.1, default patch 22.1.8; llvm-sys 221.1.1 in the lockfile).
- inkwell 0.10 exposes a **MCJIT-era** JIT API only:
  `Module::create_jit_execution_engine` (which calls
  `LLVMCreateJITCompilerForModule`) plus
  `ExecutionEngine::{add_global_mapping, get_function_address,
  add_module, remove_module}`. Its ORC wrappers are vestigial (the
  IR-adding calls are commented out in `execution_engine.rs`).
  llvm-sys 221 ships raw **`orc2`** bindings as a separate path.
- **Spike question S1 (expected pass, must be probed):** MCJIT
  still ships on LLVM 22.x (`llvm/lib/ExecutionEngine/MCJIT/`
  present on `release/21.x`, `release/22.x`, and `main`;
  `LLVMCreateJITCompilerForModule` still a real EngineBuilder path;
  LLVM 22 docs still ship "MCJIT Design and Implementation") but is
  deprecated/slated for removal ([RFC, July
  2024](https://discourse.llvm.org/t/rfc-removing-mcjit-and-runtimedyld/80464)).
  The probe (~30 lines: create an EE over an empty module) confirms
  `create_jit_execution_engine` works on the pin — and hence that
  Option A is directly executable. This recon host has no LLVM
  toolchain (`llvm-config` absent), so the spike runs in CI or a
  tooled dev box.
- Generated modules expose only C `main(argc, argv) -> i32`
  (`lower.rs`, `add_function("main", …)`). A JIT must either call
  `main` (observing stdout/exit code) or add a value-returning entry
  (e.g. `clj_run() -> ptr`, an owned box) to `lower.rs`.
- `rt_*` helpers are statically linked into AOT binaries. An
  in-process JIT must resolve them another way: EE global mappings
  (Option A), linking the temp artifact against the same `clj-rt`
  archive (Option B), or process-symbol lookup (Option C).
- AOT's error model null-checks into an abort block calling libc
  `exit(1)`. That is fine for a child binary and fatal in-process:
  every option needs a trap/return error path for JITted code.

## 3. Slice options

### Option A — In-process JIT via inkwell `ExecutionEngine` (S1 confirms)

- **Scope:** new `jit` module (inside `clj-aot` or a new `clj-jit`
  crate, same `aot`-style feature gating): `plan_source` →
  `lower::to_module` → `create_jit_execution_engine` → map `rt_*`
  via `add_global_mapping` → call the entry → return
  stdout/exit/value. CLI `jit` subcommand (or `--jit` eval flag)
  mirroring `cmd_build`. Subset-only. Tests mirror
  `clj-aot/tests/build_exe.rs` but run in-process, asserting
  byte-identical stdout/exit vs the AOT binary and the interpreter.
- **Effort:** S (days) — planner and lowerer reused untouched;
  only the EE runner plus entry/error-path work is new. (Spike S1
  confirms EE creation on the pin first.)
- **Risk:** deprecation, not availability — MCJIT works on the pin
  but is slated for removal upstream, so a future LLVM bump would
  force a move to Option C. No impact within the LLVM 22 pin.

### Option B — Link-and-load JIT: temp shared lib + `dlopen`

- **Scope:** same CLI/tests/contract as A, but the runner reuses the
  already-tested emission path: `target::emit_object` to a temp
  object file (`obj_path` via `std::fs::write`) → link a temp
  `.so`/`.dylib` against the `clj-rt` archive via the existing
  `link.rs` machinery with `-shared` → load with `libloading`,
  call the entry, remove the temps. Subset-only.
- **Effort:** S–M. Zero new LLVM API calls — every LLVM invocation
  is already exercised by the AOT tests. New work: `-shared` link
  variant, loader, temp lifecycle, entry/error-path (shared with A).
- **Risk:** low technical risk; accepted costs are temp files plus a
  `cc` invocation at JIT time (the hermetic `--emit` legs are
  unaffected), per-eval link latency (fine for run-once; hot-loop
  speed was never the slice-1 goal), platform `dlopen` quirks
  across the three OS legs, and a new `libloading` dependency
  (absent from all manifests and `Cargo.lock` today —
  license/MSRV/supply-chain review owed).

### Option C — ORCv2 LLJIT via llvm-sys `orc2` (future-proof in-process)

- **Scope:** same CLI/tests/contract, with a hand-rolled LLJIT
  session over the raw `orc2` FFI: add the lowered module, resolve
  `rt_*` against process symbols, look up the entry, call it.
  Subset-only.
- **Effort:** M (weeks): raw unsafe FFI with no inkwell safety net,
  ORC API churn across LLVM majors to track against the LLVM 22 pin.
- **Risk:** largest unsafe surface of the three; version lock-in;
  overkill for slice 1. Only justified if in-process *and*
  temp-free becomes a hard requirement.

## 4. Recommendation

**Slice 1 = Option A. Run spike S1 first (half-day, time-boxed)
to confirm EE creation on the pin; fall back to Option B only if
the probe fails. Defer Option C to a later epic.**

Rationale: A is the simplest runtime (in-process, no temp files, no
link step) and is directly executable — MCJIT ships on LLVM 22.x,
so the only risk is deprecation, which cannot bite within the fixed
LLVM 22 pin. B stays a live fallback (same CLI/tests/contract, no
new LLVM API calls) at the cost of temp files, a `cc` invocation
per eval, and a new `libloading` dependency. C remains the
future-proof answer for a post-MCJIT LLVM bump. The CLI spelling,
subset ceiling, entry/error-path design, and equivalence tests are
shared across options, so the spike loses nothing either way.

## 5. Non-goals (later slices/epics)

Full-`Expr` JIT; unboxed numerics / performance JIT; caching or
persisting JITted code; hot-path auto-JITting inside `eval`; new
`clojure.core` vars (any JIT entry point is tooling surface like
`build`, alongside `native_compile` — not core language).

## 6. Acceptance sketch for slice 1

- `clojure-rs jit file.clj` (exact spelling TBD in the slice spec)
  runs subset programs with byte-identical stdout/exit vs the AOT
  binary and the interpreter; subset violations surface the
  interpreter-identical diagnostic; no user-visible artifact remains.
- Gates per #42 conventions: three-crate suites, `fmt --check`,
  clippy zero warnings, complexity at baseline, llvm-cov clean in
  scope; feature-gating follows the `aot`/MSRV-leg precedent.
