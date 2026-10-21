# S9 slice plan: hardening (T1174)

Branch `T1174-s9-hardening`. Charter: spec §2.5 ("corpus growth,
ratchet floors, docs, follow-up tasks") plus the review residuals
from R1/R2 and the standing model-checking requirement. No new
language surface except the deferred S6/reader follow-ups below;
everything else is gates, robustness, proofs, and docs. TDD:
corpus/pins first wherever behavior is observable (spec §2.6).

## S9A. Ratchet floors (gates)

- Patch coverage: spec §2.6 defers per-PR patch coverage to S9.
  Wire a diff-aware patch floor (100%, same as the line floor)
  into `quality gates` so new/changed lines can never land dark.
  Extends `scripts/ci/go/cmd/coverage-floor` (or a sibling tool)
  with base-diff scoping; goldens + `go ci-tools tests` cover it.
- MSRV: add `rust-version = "1.85"` to every crate (s7-plan
  follow-up) plus a CI leg pinning the 1.85 toolchain that proves
  the floor. The leg excludes LLVM-dependent compilation
  (`clj-aot` needs a newer rustc via inkwell) and covers the rest
  of the workspace; exact scope settled in implementation.
- No-default-features CI leg: the `#[cfg(not(feature = "aot"))]`
  unlinked path (`cli.rs:591`) is proven only locally today (R1
  residual). Add an LLVM-free `--no-default-features` test leg to
  the OS matrix — cheap on all three runners.

## S9B. Deferred language follow-ups (oracle-pinned)

- `pvalues`/`pcalls` composites (s6-plan: "follow-up if cheap,
  else S9"). Oracle-pin JVM behavior first; implement if cheap,
  else defer with a recorded reason — no silent drops.
- Reader extras (reader-contract §24 item 11): `print-dup`,
  `data_readers.clj`, `load-reader` extras (`EdnReader` surface
  already covered via `--edn`). Oracle-pin each; implement or
  catalogue as a divergence with cause. Corpus entries land with
  the behavior, per §25.

## S9C. Robustness residuals (R1/R2)

- `to_module` verification: map LLVM's module verify failure to a
  clean `Emit` diagnostic instead of an abort (R1 residual).
  Hermetic unit test pins the error shape (no link step).
- Deep-nesting guards: `build` on pathological nesting overflows
  the machine stack at compile time today (shared with the
  pre-existing reader/analyzer; R1 residual). Budget recursion at
  the `build` entry points (`compile_str`/`compile_file`) with a
  clean diagnostic; readers/analyzers keep their shape — guard,
  don't redesign. Oracle check: JVM behavior on the same input
  recorded in the plan.

## S9D. Model checking (standing requirement)

The concurrency code ships with argument-only validation today
(single-lock reasoning, stress tests). Deliver real artifacts:

- Loom models for the `Future`/`Promise` outcome-cell protocol
  (the wake-not-timeout back-edge S9 must pin by construction,
  not by sleeper timing) and the agent action-queue protocol,
  run under `cfg(loom)` in CI.
- One TLA+ spec for agent `send`/`send-off` ordering (in-order
  per-agent sends, `send-off` non-blocking, shutdown/cancel
  semantics), model-checked in CI via the `tla2tools` jar or a
  container step; the spec lives under `docs/t1174/models/`.
- Keep the Relaxed-ordering arguments (`OBJ_SEQ`, shutdown flags)
  as written proofs next to the code; anything with a
  multi-variable invariant gets a model instead of prose.

## S9E. Corpus growth + docs

- Differential corpus growth for every S9-covered behavior (S9B
  items, verify/nesting diagnostics, MSRV/unlinked legs where
  observable); goldens JVM-seeded and checked in.
- Docs: S9 notes (`s9-plan.md` deltas recorded here), dual-path
  doc updates where behavior changed, and the Java-removal
  readiness checklist (what still assumes a JDK: oracle-based CI,
  dual-path docs) as the post-S9 handoff — removal itself is a
  follow-up, not this slice.

## Design (decided, recorded)

- D1. Patch floor 100% from day one of S9A: the tree is already at
  100% line coverage, so the patch gate starts green and only
  bites on regressions.
- D2. MSRV leg excludes `clj-aot` compilation: inkwell tracks
  newer rustc; the 1.85 promise covers our code, not LLVM's.
- D3. Follow-ups resolve, never linger: each S9B item ends the
  slice implemented+tested or deferred-with-reason in this plan.
- D4. Models run in CI or they don't exist: loom cfg + TLA+
  check are gates steps, not local-only scripts.
- D5. No behavior change without an oracle pin or a catalogued
  divergence entry — S9 included.

## TDD order

1. S9A patch-floor tool goldens first (failing gate on a dark
   patch, passing on a lit one), then wiring.
2. S9B oracle probes before implementation (`-Duser.language=en
   -Duser.country=US -Duser.timezone=UTC`, per standing rule).
3. S9C diagnostic-shape tests before guards (hermetic, no link).
4. S9D model properties before/while the code they pin lands;
   failing model runs reproduced in CI config first.
5. S9E corpus entries seeded from the JVM jar at implementation
   time, never hand-written.

## Acceptance

- Every S9A–S9E item landed or deferred-with-reason in this plan.
- `quality gates` holds 100% line + 100% patch on the slice head;
  MSRV leg + no-default leg green in the matrix.
- Loom + TLA+ steps green in CI; specs checked in.
- Full matrix (test/release × 3 OSes, fmt+clippy, go-tools, JVM)
  green; review-loop to APPROVED; land.
- `docs/t1174/progress.md` flips S9 to landed on the follow-up
  slice's opening commit (house pattern); the Java-removal
  checklist in S9E is the post-S9 handoff.

## Deltas (recorded as they land)

- S9A rustyline pin: rustyline 19 needs rustc 1.88+
  (`cfg_select`, `file_lock`, let-chains in its own sources) and
  its minimal-feature builds don't compile, so `clj-repl` pins
  `=17.0.2` (default features; keeps `home` 0.5.11-compatible).
  All repl tests pass unmodified on 17. Revisit if rustyline
  declares an MSRV.
- S9A own-code fallout from the 1.85 floor, all fixed: `as_chunks`
  → `chunks_exact` (io.rs), two let-chains → nested `if`s
  (expr.rs, lazy.rs) in coverage-neutral form.
- S9B resolutions (D3): pvalues/pcalls behavior already ported
  (S6D verbatim); added the ExecutionException error-propagation
  pin. `:static` meta unmodeled globally (even `map`) — deferred
  as a compile hint with no runtime semantics. `*print-dup*`
  dispatch stays a catalogued divergence (atom witness pinned:
  JVM throws under `true`, we print). set-agent-send-executor!
  et al rejected (no custom executors); load-reader rejected
  (N1-adjacent, needs java.io); data_readers.clj stays spec'd-Out
  (no classpath); clojure.edn deferred (needs a value-producing
  EDN reader entry; `--edn` covers interactive use). Absence
  pinned for load-reader, the executor setters, and the
  clojure.edn namespace.
- S9 infra (Windows setup-llvm cache): the archives-only cache
  keyed on `action.yml` alone went stale forever after one vcpkg
  baseline drift (exact hits never re-save; vcpkg then reported
  `Restored 0 package(s)` and rebuilt libxml2 from source, ~7
  min, every run). The cache now keys on the vcpkg baseline
  commit, covers the built installed tree plus archives and
  downloads (msys2 tool + source tarballs), and skips the vcpkg
  install on an exact hit (guarded by an `xml2` presence check);
  the SDK transplant still runs every time. Follow-up fix:
  `VCPKG_INSTALLATION_ROOT` renders empty on GitHub runners
  (verified in the log: cache held only the ~10 MB archives),
  so the paths use the baseline step's resolved `dir` output;
  verified by rerun: exact hit on the 77 MB full cache prints
  `reusing cached vcpkg installed tree` and vcpkg never runs
  (cold 6.9–8.3 min → warm ~0 s; even archives-only fallback
  restores in 317 ms). Note: actions/cache versions entries by
  the path list, so the path fix itself invalidated the old
  entry — and the fleet carries multiple vcpkg baselines at
  once, hence one slow rebuild per baseline before warm.
