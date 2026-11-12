# I258: Mixed AOT binaries share one evaluator — Specification

Issue: hhh-pa/clojure#258 (follow-up of #256 / #257).

## Part 1 — goal and acceptance criteria

Goal: a mixed AOT binary never boots a second interpreter, and it runs
the shared runtime the way `clojure-rs run` does. As a result,
`clojure-rs-aot` costs no more than `run` before #259's native work
starts paying off.

Acceptance criteria:

1. **One boot.** Native `defn`s called from embedded code run their prim
   shims on the program's evaluator. The global `clj-rt` runtime never
   boots in mixed binaries; it remains for pure-subset binaries.
2. **Same runtime profile as `run`.**
   - The runtime archive uses the same global allocator as the CLI
     (mimalloc off Linux). The allocator now lives in `clj-eval`, the
     crate every artifact links.
   - On Linux, AOT links use the CLI's `-no-pie` and hot-text section
     order (`hot-text.ld` shipped next to the archive).
3. **Measured.** On local macOS arm64, mixed binaries are within about
   2.5% of `run` in instructions on the sample snippets (was +14..+20%).
   In the snippet sweep, `clojure-rs-aot` / `clojure-rs` maxrss and
   cpu_s geomeans are at most about 1.0.
4. **No behavior change.** Nested native calls (native → prim →
   interpreter callback → native) restore the enclosing evaluator.

## Part 2 — design

- **`runtime::ActiveScope`.** A thread-local `ACTIVE: *mut Evaluator`
  is set only inside the native-call wrapper (`mixed::invoke`), from the
  `&mut Evaluator` the interpreter hands it. The wrapper does not touch
  that reference until the scope drops. `call_prim` reborrows it when
  set, otherwise it takes the global lock (pure binaries). The prim table
  for active calls is resolved once (`SHARED_PRIMS`), since the natives
  are the same `clojure.core` fns in every evaluator.
- **`clj_aot::link::layout_args(archive, linux)`.** On Linux it returns
  `-no-pie`, plus `-Wl,-T,<archive dir>/hot-text.ld` when that file
  exists. The release tarball ships `hot-text.ld` on Linux, and the
  snippet sweep copies it next to its archive.
- **Unsafe.** One new reborrow (`call_prim`), with a SAFETY comment;
  the geiger baseline is updated.

## TDD

- `active_scope_runs_prims_on_the_callers_evaluator_258` (prims on the
  active evaluator, nested restore, null after).
- `layout_args_follow_linux_and_hot_text`.
- The existing mixed BDD corpus continues to pin stdout, stderr and exit
  code against `run`.

## CI impact

The release workflow adds `hot-text.ld` to the Linux tarball. No new
gates.
