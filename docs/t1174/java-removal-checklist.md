# Java-removal readiness checklist (S9E handoff, T1174)

Post-S9 follow-up — removal itself is NOT this slice. The Rust port
is fully independent of the JVM tree: no `rust/` or `scripts/` source
references `src/jvm`, `src/clj`, or `test/clojure` (verified by grep
at S9E time).

## Delete set (all tracked)

- `src/jvm/` (2.5M), `src/clj/` (908K), `test/` (924K)
- `codegen/` (36K — unreferenced: named in neither `build.xml`
  nor `pom.xml`, no generated-markers in `src/jvm`)
- `pom.xml`, `build.xml`
- `.github/workflows/test.yml` (JVM matrix),
  `snapshot.yml`, `release.yml` (on-demand JVM releases),
  `doc-build.yml` (JVM API docs via setup-clojure)

## Keep set (looks JVM, stays)

- `epl-v10.html` — every Rust EPL header points at it.
- `docs/t1174/**` — contracts/pins/goldens are the checked-in
  oracle record; they outlive the tree they were seeded from.
- The `tla` CI job's Temurin JRE (`rust.yml`) — a model-checker
  runtime, not the Clojure JVM build. Stays.
- Root `.gitignore` JVM lines (`*.jar`, `target`, maven/idea) —
  harmless; `target/` still ignores the local oracle jars below.

## Oracle strategy post-deletion

All differential goldens are checked in, so deletion loses no
signal. The local oracle jars (`target/*.jar`, gitignored build
artifacts) keep working until deleted. For future oracle needs
with no repo JDK: fetch the upstream release jar from Maven
(`org.clojure/clojure`) — no repo JVM build required.

## CI effect

Deleting `test.yml` frees the JVM matrix (17/21/25 ×
temurin/corretto × direct/no-direct on ubuntu) — the capacity
competitor behind the macOS starvation. `rust.yml` + `rust-release.yml`
are the only workflows left.

## Order

Land S9 first (review-loop to APPROVED), then remove in one commit:
tree + workflows together, so no run ever sees a half-deleted JVM.
