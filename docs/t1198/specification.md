# T1198: Master-delta inventory past 1.13.0-alpha9 — Specification

- Status: draft for spec review (slice M1 under T1192).
- Parent: T1192 (REL ships the alpha9 equivalent; this seeds
  Phase 2 catch-up).
- Task: T1198. Branch: `T1198-master-delta`.

## Part 1 — User/functional specification

### 1.1 Goals

1. Every `clojure/clojure:master` commit past the
   `clojure-1.13.0-alpha9` tag is listed with subject, date,
   and files, each triaged to port / skip / already-have with
   evidence.
2. The port verdicts define the Phase 2 catch-up slice list
   (worked after REL ships); skips carry reasons so they are
   never revisited silently.

### 1.2 Non-goals (explicit)

- No behavior changes in this slice (inventory only).
- No alpha10 version bump (a later REL2 decision, not M1).
- No CX-zero or unrelated work.

### 1.3 Acceptance criteria

- [ ] `docs/t1198/inventory.md` committed: all 17 commits
  listed, each with verdict + reason + evidence pointer.
- [ ] Phase 2 slice list defined from the port verdicts.
- [ ] Task T1198 records the inventory commit.

## Part 2 — Technical specification

- Source: `gh api repos/clojure/clojure/compare/clojure-1.13.0-alpha9...master`
  (17 commits at inventory time) plus per-commit diffs.
- Verdicts: port (names the port file to change), skip
  (build/infra, pure revert with no residue, test-only,
  perf-only, or out-of-scope class with reason), already-have
  (port file:line or test evidence).
- Oracle spot-checks only where a verdict hinges on behavior;
  the catch-up slices do the full oracle work.
- CI impact: docs-only; no workflow changes.

### Constraints

EPL-1.0 headers where new files are code (docs need none);
signed commits (m-hu); no behavior change.
