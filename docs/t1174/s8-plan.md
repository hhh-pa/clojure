# S8 slice plan: LLVM AOT (T1174)

Branch `T1174-s8-llvm-aot`. Charter: spec 1.3/1.4/2.4
(`build <file.clj> -o <out>` over the `clj-aot` stub; the pure
subset compiles to a native binary that prints what `run`
prints; out-of-subset input fails `build` with a diagnostic
naming the first unsupported form). TDD: subset corpus first
(spec §2.5).

## S8A. Subset planner (LLVM-free, `clj-eval`)

- New `clj-eval/src/aot_subset.rs`: `plan_source`
  (read + analyze on a throwaway `boot()` evaluator, never
  `eval`) → `SubsetProgram { fns, main }` or `SubsetError
  { span, form, hint }`.
- Surface-form classification (the `defn` macro's expansion
  contains `alter-meta!`/`var`, so the checker matches
  `(defn ...)` BEFORE expansion): single-clause fixed-arity
  `defn` (+ optional docstring/attr-map, ignored) over
  `if`/`do`/`let` (symbol bindings only), literals
  (Int/Double/Bool/Str/Vector-of-subset), direct calls to
  sibling `defn`s (self-calls are plain calls — no TCO,
  exactly like the JVM) or the closed primitive set
  (`+ - * quot < <= > >= = count nth get conj`, unqualified
  or `clojure.core/`-qualified), and the `print` intrinsic.
  Everything else is `SubsetError` naming the operator.
- `main` = non-`defn` subset tops in load order (values
  discarded; program output is `print` output only, like
  `run`'s no-echo rule). Forward references fail in the
  analyze pass exactly like the interpreter (compile in
  order, no pre-declare).
- Analyzer errors surface with direct file spans (better
  than `run`'s load-wrapper synthesis); reader errors reuse
  reader spans. Unit-test matrix: accept/reject per form,
  first-error-wins in source order, span per case.

## S8B. Runtime archive (`clj-rt`, new crate)

- `crate-type = ["staticlib", "rlib"]`, deps
  `clj-eval + clj-value` only (never inkwell): the
  `extern "C"` helper ABI over opaque `*mut Value` boxes.
- Zero semantic drift by construction: every primitive
  helper resolves its `clojure.core` native from a
  process-global booted evaluator and calls THE SAME
  `NativeImpl` (`chain_cmp` order, `quot` zero/NFE rules,
  `nth`/`get`/`conj` edges, arity texts — all shared).
- Ownership: args consumed, return owned; `rt_clone`
  borrows; `rt_drop` frees; locals are owned slots.
  Single-threaded binaries; no shadow stack. Errors:
  helpers print `path:line:col: Class: message` (spans
  baked at the call site) and exit 1 — no unwinding
  across FFI, every shim wrapped in `catch_unwind`.
  The one `unsafe` module is the audited FFI shim
  (geiger delta recorded with justification).
- Unit tests call the `extern "C"` fns directly (no LLVM).

## S8C. Lowering + link (`clj-aot`, inkwell)

- `compile_str/file` → plan (S8A) → inkwell module
  (opaque `ptr` value type; one LLVM fn per `defn`;
  `@main` drives tops, returns C8 codes) → object file
  via host `TargetMachine` (x86/aarch64 init only —
  never `target-all`) → platform link (`cc` on Unix,
  `link.exe`/`clang` on Windows) against the `clj-rt`
  archive (exe-relative lookup + `--runtime-archive`
  override; release archives ship the archive next
  to the binary).
- `--emit-ir` / `--emit-obj` skip linking (hermetic
  tests, linker-less machines). No debug info in S8;
  no cross-compilation; host-only.
- IR snapshot tests (`contains`, not full goldens).

## S8D. CLI + corpus + packaging

- `cmd_build` replaces the stub: IO checks first (exit 3
  `build: ...`, identical under `--no-default-features`),
  then plan/emit/link; exit 1 `path:line:col: msg` naming
  the first failure; clap keeps exit 2. Unlinked-`build`
  message/test stays intact.
- `s8-corpus/`: supported programs differential
  (`build`+run vs `run`, byte-identical stdout + exit 0);
  `unsupported/` cases pin `build` exit 1 + naming
  diagnostic AND `run` exit 0 (the BDD "still work under
  `run`" clause); runtime-error programs are ours-pinned
  (AOT bakes true call-site spans; `run` synthesizes
  1:1 — documented divergence).
- `rust/tests/build.rs` (smoke: exits, diagnostics,
  execute-output) + `rust/tests/build_corpus.rs`
  (replay every corpus file); old stub asserts move
  out of `cli.rs`. Release packaging ships
  `libcljrt.a` / `cljrt.lib` per platform.
- Spec §2.4 update in-slice: rooting strategy, `main`
  semantics, S8 limits (host-only, no debug info, no
  TCO, `*command-line-args*` unconveyed).

## Design (decided, recorded)

D1. Boxed-everything lowering (opaque `ptr`, runtime
    helpers per primitive call). No unboxing fast paths:
    the subset has no loops; boxing cost is acceptable.
D2. Primitives delegate to the interpreter's own natives
    (registry-resolved from a global booted evaluator) —
    never reimplemented. `=`/`compare` drift is impossible
    by construction.
D3. Self-calls are PLAIN calls (no `tail` flag): the JVM
    stack-overflows deep self-recursion, and differential
    parity beats cleverness.
D4. `recur` is OUT (needs loop/fn TCO machinery); `loop`,
    `try`, `throw`, `var`, `set!`, laziness, concurrency,
    `println`/`pr`/`prn`, and all non-`defn` macros
    (incl. `when`, `ns`) are OUT, each naming its form.
D5. Prepost conditions are OUT (ignoring them would
    diverge); docstrings/attr-maps on `defn` are
    accepted-and-ignored (no runtime effect in-subset).
D6. Local LLVM loop (official tarball, NOT CI's brew):
    `LLVM_SYS_221_PREFIX=$HOME/llvm22` +
    `RUSTFLAGS="-C link-arg=-Wl,-lto_library,
    $HOME/llvm22/lib/libLTO.dylib -C link-arg=-Wl,
    -rpath,$HOME/llvm22/lib -C link-arg=$HOME/llvm22/
    lib/libc++abi.dylib"`. The tarball's static archives
    are LTO bitcode (Apple's libLTO 21 can't parse v22),
    and `-lc++` must resolve to the tarball's libc++ with
    its libc++abi (never `-lunwind`: it hijacks Rust
    panics — observed SIGABRT in the concurrency suite).
    CI needs none of this (brew llvm@22).

## TDD order

1. Oracle probes → `s8-pins.md` (subset boundary per
   form, `print` shape, error/exit rules).
2. `s8-corpus/` programs + expected outputs (seeded
   from the interpreter, JVM cross-checked) + MANIFEST.
3. S8A planner + unit matrix (LLVM-free).
4. S8B runtime + direct-call unit tests.
5. S8C lowering + IR snapshots + obj emission.
6. S8D CLI + link + integration replay + packaging.
7. Spec §2.4 update + matrix green.

## Acceptance

- BDD 1.3 AOT scenario passes: `./f` exits 0, prints
  what `run` prints; out-of-subset fails `build`
  naming the form, still works under `run`.
- Corpus replay green; per-slice gates
  (fmt/clippy/quality-gates) via PR CI.
- Spec + contracts update in-slice (`Ref T1174` +
  `AI-Tool:` trailers per commit).
