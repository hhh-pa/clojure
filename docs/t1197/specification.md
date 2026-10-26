# T1197: Release the Rust 1.13.0-alpha9 equivalent — Specification

- Status: draft for spec review (slice REL under T1192).
- Parent: T1192 (PR #22 landed; port tracks 1.13.0-alpha9 behaviorally).
- Task: T1197. Branch: `T1197-release`.

## Part 1 — User/functional specification

### 1.1 Goals

1. The port reports the tracked language version: `*clojure-version*`
   (and `clojure-version`) reflect 1.13.0-alpha9 exactly as the JVM
   oracle does (exact key set — no `:interim` — plus qualifier
   spelling).
2. Crate versions and a changelog name the 1.13.0-alpha9 equivalent:
   one consistent version across the workspace plus a root changelog
   entry describing what the release contains and what it diverges on.
3. The existing `rust-release.yml` mechanics stay the delivery path:
   a release PR lands through the normal review loop; the
   release-candidate tag is created only on explicit user go, and the
   tag CI (three-OS tests + archives) must be green before any
   1.13.0-final follow-up.

### 1.2 Non-goals (explicit)

- The 1.13.0-final catch-up and later alpha/beta tracking (later
  slices; this slice freezes the alpha9 line).
- The M1 master-delta inventory and the CX-zero program (sibling
  slices, untouched).
- crates.io publishing (the release workflow attaches GitHub
  archives only; no publish step exists or is added here).
- General host `new` and other out-of-scope language gaps (they
  stay divergences, recorded where they already are).

### 1.3 BDD scenarios

- Given a fresh evaluator, When `(select-keys *clojure-version*
  [:major :minor :incremental :qualifier])` is evaluated, Then it
  equals the oracle's map for 1.13.0-alpha9 (probe before changing).
- Given the release commit, When `cargo metadata` is read, Then
  every workspace crate reports the same release version.
- Given the release commit, When the changelog is read, Then it
  names the 1.13.0-alpha9 equivalent, its behavioral scope, and
  the known-divergent list with pointers.
- Given the release PR, When CI finishes, Then every gate is
  green (fmt/clippy/deny/geiger/complexity/100% line+patch,
  loom/TLA+) exactly as on ordinary slices.

### 1.4 Acceptance criteria

- [ ] Oracle probe for `*clojure-version*` + `clojure-version`
  committed (`.clj`/`.out`), values match post-change behavior.
- [ ] All workspace crates share the release version; no stale
  `0.1.0` remains in manifests or version strings.
- [ ] Root changelog exists with scope + divergence pointers.
- [ ] Release PR reviewed to APPROVED through the loop and
  landed; tag CI dry-run (`workflow_dispatch`) green.
- [ ] No tag is created in this slice without explicit user go.

## Part 2 — Technical specification

### 2.1 Change inventory

- `rust/crates/clj-eval/src/ns.rs` `clojure_version_map`
  (currently `{:major 0, :minor 1, :incremental 0, :qualifier
  nil, :interim true}`) → oracle values (the `:interim` key
  goes away, matching the JVM map); check `core.rs`
  `native_clojure_version` renders from the same source.
- `rust/Cargo.toml` + `rust/crates/*/Cargo.toml` versions →
  the release version (single value, workspace-wide).
- New root changelog (path: `CHANGELOG.md`), first entry for
  this release.
- Tests: version-map pins (existing harness + oracle probe);
  no new natives, no behavior changes beyond the version map.
- Tag name is settled here (`v1.13.0-alpha9` form) but the tag
  itself is created only on explicit user go (never from the
  loop, never from this slice's commits).

### 2.2 TDD and oracle plan

1. Probe the oracle (`*clojure-version*`, `(clojure-version)`)
   on 1.13.0-alpha9 jars; commit `.clj`/`.out`.
2. Failing version-map test first, then the one-line change.
3. Manifest + changelog edits; full CI contract before commit.
4. Release PR through the review loop; `workflow_dispatch`
   dry-run green as the tag-readiness proof.

### 2.3 Constraints

- EPL-1.0 headers on new files; MSRV 1.85 (no let-chains).
- Signed commits (m-hu authors); 100% line+patch coverage holds
  (changed lines covered by the version pins).
- No behavior change beyond the version map; every other value
  stays oracle-identical.

### 2.4 CI impact

No workflow changes (`.github/workflows/rust-release.yml`
untouched); version/changelog edits ride `rust.yml` + gates.
The `workflow_dispatch` dry-run is read-only w.r.t. releases.
