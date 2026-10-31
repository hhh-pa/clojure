# Gap 121, slice 1 — in-process LLVM JIT (Option A)

Part of #121 (ex-N5), funded backlog epic #150, zero-oos program
(#42). Follows [recon](recon.md): Option A (in-process JIT via
inkwell `ExecutionEngine`), spike S1 first, fallback B only if the
probe fails. This spec nails the two recon-deferred design
decisions (entry choice §2, error path §3) and the slice-1
contract. No specification/TDD/BDD skills are installed in this
workspace, so this spec follows the standard spec-first shape:
decisions / contract / CLI / tests / gates.

Upstream oracle: none (JVM Clojure has no JIT API to port —
recon §1). The oracle is behavioral equivalence: byte-identical
stdout / stderr / exit vs the AOT binary and the interpreter over
the S8D corpus.

## 1. Placement and gating

- New `jit` module inside `clj-aot` (not a new crate): it reuses
  `lower` directly, and `clj-aot` already carries the `aot`
  default feature plus the MSRV-leg LLVM exclusion (rust.yml S9A
  D2). No new cargo feature, no `rust.yml` change, no
  `Cargo.lock` change, no new dependency.
- `clj-eval` stays LLVM-free: the only touch is the
  `UNLINKED_JIT_LINE` const next to `UNLINKED_BUILD_LINE`.
- `clj-rt` untouched: the JIT resolves its `no_mangle` shims by
  address in-process (§4).

## 2. Decision D-entry: call C `main`, no `clj_run`

The JIT calls the module's existing C `main(argc, argv) -> i32`
through the EE (typed `JitFunction`, no manual transmute) and
returns its exit code. Rationale:

- Zero lowering changes for the entry: `-main`/argv handling,
  top-level load order, and exit codes stay structurally
  identical to the AOT binary, so the 3-way corpus differential
  (§7) pins parity instead of re-pinning semantics.
- A value-returning `clj_run() -> ptr` entry would need a second
  lowering of the tops (return-last vs discard-each) plus owned-box
  handoff across the JIT boundary — new ABI surface for no
  slice-1 acceptance gain (acceptance is stdout/exit parity).
- stdout/stderr flow to the process streams naturally, exactly
  like the AOT binary; the CLI adds no capture layer.

Consequence: `rt_argv` skips `argv[0]`, so the runner passes a
static dummy program name plus the CLI args (§6).

## 3. Decision D-error: return-propagation, not trap, not exit

AOT's abort block calls libc `exit(1)` — fatal in-process. The
JIT lowers in a return mode (`Lower::jit`): every abort site
drops its frame slots and returns instead of exiting —
null from a `defn` body, `1` from `main`. Callers already
null-check every `CallPrim`/`CallFn` result, so null propagates
frame by frame to `main`, which returns 1. The arity-error form
keeps the existing diverged/fresh-block structure (report, then
return; downstream builders emit dead-but-verifiable code that
never executes). Rationale:

- No trap machinery: no `longjmp`, no unwind through JIT frames,
  no signal handling — the propagation reuses the null checks
  the success path already pays for.
- Diagnostics are unchanged: fallible helpers print
  `path:line:col: Class: message` before returning null, so JIT
  stderr is byte-identical to the AOT abort stderr (true
  call-site spans — same planner, same baked path).
- Success-path IR is identical to AOT modulo abort-block
  bodies (snapshot-pinned: JIT IR contains `ret i32 1` and no
  `exit` call; AOT IR unchanged).

Accepted limitation (slice 1): a partially-built `VecLit`
accumulator on the error path leaks one box (it lives in SSA,
not in a frame slot, so the error return cannot drop it). The
leak is error-path-only, bounded by the error count, and freed
at process exit; fixing it (spilling the accumulator) is a
later slice. Frame slots are always dropped; prim-arg boxes are
already consumed by the failing helper before it returns null.

## 4. Contract: `clj_aot::jit`

```rust
pub enum JitError { Plan(String), Jit(String) }
pub fn run_str(source: &str, source_path: &str, args: &[String]) -> Result<i32, JitError>
```

Pipeline: `plan_source` → `lower::to_module_jit` (verified
before return, like AOT) → `create_jit_execution_engine(None)`
(self-initializes the native target and MCJIT) → map all 28
`rt_*` declarations via `add_global_mapping` → look up `main`
→ call with `(argc, argv)` → return the exit code (0 or 1).

- `OptimizationLevel::None`: slice 1 buys capability, not
  speed; no opt passes, fastest JIT turn.
- Mapping table (single source of truth): 13 prim entries pair
  `cljrt::prim_symbol(prim)` with the shim address (no
  duplicated symbol strings); 15 helper entries pair the FFI
  name with the shim address. No exhaustive `Prim` match (a
  13-arm match trips the cyclomatic gate — the table is a
  branchless array literal). `cljrt::X as *const ()` (clippy
  `function_casts_as_integer` forbids direct `as usize`).
- `exit` stays declared but unmapped: no JIT codegen path emits
  an `exit` call, and MCJIT never resolves unreferenced
  declarations. A drift test fails if any other declaration
  lacks a mapping.
- `JitError::Plan` carries the verbatim planner diagnostic
  (interpreter-identical, includes the `path:line:col:` span);
  `JitError::Jit` carries a `jit: …`-prefixed message
  (distinguishes backend failures at the CLI). One `Display`
  or-pattern line serves both (patch-coverage: the Plan path
  covers it; LLVM failures are environmentally untriggerable).
- No filesystem interaction: no temp files, no archive, no
  linker — nothing to clean up, nothing hermetic legs forbid.
- No `run_file`: the CLI owns the IO gate (exit 3), mirroring
  `cmd_build`, and calls `run_str` — same split as
  `compile_str`/`compile_file`.

## 5. Lowering changes (`lower.rs`, AOT behavior frozen)

- `Lower` gains `jit: bool` (return mode) and `in_main: bool`
  (current function returns `i32` vs `ptr`), set at function
  entry. `Lower::new` stays AOT; `Lower::new_jit` flips the
  mode. `to_module_jit` mirrors `to_module` (deliberate small
  duplication, `driver.rs` precedent).
- `null_check` takes the frame slots (JIT error blocks drop
  them); `emit_abort` takes the slots and branches on the mode
  (AOT arm byte-identical IR).
- Every touched AOT line stays covered by the existing snapshot
  tests; every new JIT line executes under the JIT tests (§7).

## 6. CLI: `clojure-rs jit file.clj [args...]`

- `Jit { file, args }` subcommand mirroring `Build`/`Run`:
  `file` positional, trailing `args` passed to the JITted
  `-main` (after the dummy `argv[0]`), so `-main` argv parity
  with the AOT binary holds (`jit f.clj a b` ≡ `prog a b`).
- Exit codes per C8, mirroring `cmd_build`: missing file /
  directory / unreadable → `jit: …`, exit 3 (IO gate runs on
  every configuration); plan failure → verbatim diagnostic,
  exit 1; LLVM failure → `jit: …`, exit 1; success → the
  program's exit code (0/1). Unlinked (`--no-default-features`)
  → `UNLINKED_JIT_LINE`, exit 1.
- Single error arm in `cmd_jit_compile` (all errors render
  through one `eprintln!`): the LLVM-failure arm is
  environmentally untriggerable (no temp files to break, unlike
  `build`'s TMPDIR probe), so arm-sharing with the
  test-triggerable Plan path is what keeps patch coverage at
  100%.

## 7. Tests (all execute on the LLVM legs; file-level `cfg(aot)`
gating where the crate needs it)

- **S1 probe** (`jit.rs` unit test): create an EE over an empty
  module. Expected pass on the LLVM 22 / inkwell 0.10 pin
  (recon §2). A failure here — not a code bug — is the
  trigger for fallback B (link-and-load, recon §3).
- **IR shape** (unit): JIT lowering of an erroring program
  contains `ret i32 1` and no `exit` call; AOT lowering of the
  same source still contains the `exit(1)` abort.
- **Drift guard** (unit): every declaration in a lowered JIT
  module resolves in the mapping table, except `exit`;
  `exit`/bogus resolve to `None`.
- **In-process execution** (`clj-aot/tests/jit.rs`, serialized
  by a process-local mutex — MCJIT-era thread caution):
  build_exe-shaped corpus (print/control-flow/cond-let/
  keywords) exits 0; top-level prim error, in-`defn` error, and
  arity error exit 1 (each error shape pins its return arm);
  `-main` with args exits 0; subset violation returns the
  verbatim `f.clj:1:1: unsupported form…` diagnostic. stdout
  flows to the test log (captured by the runner); byte-identity
  is asserted by the CLI legs below.
- **3-way corpus differential** (`rust/tests/build_corpus.rs`
  jit legs): every S8D replay case also runs `jit` and asserts
  stdout/stderr/exit equal to the AOT binary (goldens for
  supported/argv cases; `{path}:2:8: Class: message` stderr
  for error programs; stderr-equality with `build` for
  unsupported/malformed/nesting cases — same planner, same
  path, same text).
- **CLI contract** (`rust/tests/cli.rs`): `jit` in `--help`;
  missing/dir/unreadable inputs exit 3 (`jit: …`); a linked
  run prints and exits 0; a plan error prints verbatim, exit 1;
  unlinked reports the shared line, exit 1. `main.rs` unit
  test mirrors `build_unlinked_reports_shared_line`.

## 8. Gates accounting

- Patch + floor coverage stay 100: every new lib line executes
  under the LLVM legs (single-line error closures /
  single-arm renders for environmentally-untriggerable
  failures); every new bin line executes in instrumented
  children (the `cli.rs`/`build_corpus.rs` spawns).
- Complexity: no function over the ratchet (table literals and
  shared helpers instead of big matches).
- Geiger: exactly one new `unsafe` block (typed entry lookup +
  call); baseline +2 (crate + rollup fold). No new
  `unsafe` in tests.
- MSRV 1.85: `main.rs`/`compiler.rs`/`cli.rs` additions stay
  1.85-clean (no let-chains); `clj-aot` keeps its LLVM
  exclusion. `fmt --check` and clippy `-D warnings` clean.

## 9. Non-goals (recon §5, restated)

Full-`Expr` JIT; unboxed numerics / performance work;
caching or persisting JITted code; hot-path auto-JIT inside
`eval`; new `clojure.core` vars (the `jit` subcommand is
tooling surface like `build`); a value-returning entry
(revisit if REPL/eval embedding needs it — D-entry records
why `main` suffices for slice 1).
