# T1198: master-delta inventory past 1.13.0-alpha9

Source: `clojure/clojure` `clojure-1.13.0-alpha9` (`8a8d06b7`)
`...master` — 17 commits, inventoried 2026-10-26. Head at
inventory time includes the `clojure-1.13.0-alpha10` release
(`8ff45962`, 2026-10-22); the port tracks alpha9 per the REL
slice, so post-alpha10 WIP belongs to the future alpha11 line.

Verdicts: PORT (Phase 2 catch-up slice named) / SKIP (reason) /
ALREADY-HAVE (evidence). Evidence paths are port-relative.

## Behavior deltas (PORT — one Phase 2 slice: M2)

- `9e51d6dc` (2026-10-17, WIP `:req!` directive, core.clj
  +37/-29): the `:req!` MAP directive merges required entries
  into map destructuring. The port has the `!`-suffix path
  (`destructure_expand.rs` `required` plumbing, `req!` getter
  emission) but no `:req!` directive handling (no literal
  match). → PORT (M2, `destructure_expand.rs`).
- `4610bb46` (2026-10-18, explicit ns dominates directives,
  core.clj + tests): element namespaces now beat directive
  namespaces (`(or (namespace %) mkns)`), plus `xfk` qualifying
  post-`&` keys. The port keeps directive-wins
  (`destructure_expand.rs` `translate`: `kind.ns...or(elem_ns)`).
  → PORT (M2, `destructure_expand.rs`).

Phase 2 slice M2 (proposed): alpha10 destructuring deltas —
both commits, one oracle probe each, `destructure_expand.rs`
only. A later REL2 slice (not M1) decides the alpha10 version
bump once M2 lands.

## Skips (13 + 2 net-zero pairs)

Build/infra (no runtime behavior, JVM tree only):

- `aef2e692` — maven-release-plugin next-iteration
  bookkeeping, pom-only. SKIP.
- `e2424b3c` — revert of the mvn 3.9.16 wrapper pin
  (workflows/.mvn/mvnw). SKIP.
- `0e5c5a1f` — central-publishing-plugin 0.12.0 bump,
  pom-only. SKIP.
- `9dd120ef` — post-alpha10 maven bookkeeping, pom-only.
  SKIP.
- `b1431ec6` — reproducible-build timestamp infra,
  pom-only. SKIP.
- `ac6c9905` — core.specs dep bump, pom-only. SKIP.

Reverts with no residue (net zero with their counterpart):

- `7ee7bbaa` — keyword-callsite optimization commented out
  (Compiler.java +2/-1, comment only). SKIP.
- `4c29c0ac` — reverts `7ee7bbaa`. SKIP (pair nets zero).
- `807285bf` / `7a78eef5` — callsite tweak + exact revert
  (+1634/-1592, then back). SKIP (pair nets zero).

Release/test markers:

- `8ff45962` — `clojure-1.13.0-alpha10` release marker.
  SKIP (REL2 tracks the version line, not M1).
- `7d3c9332` — `:req!` tests only (data_structures.clj).
  SKIP (behavior triaged under `9e51d6dc`).

Perf / out-of-scope class (unobservable in the port):

- `903eee79` — `KeywordLookupSite2` callsite caching
  (Compiler.java +49/-5, no error-path changes). The port has
  no callsites (direct lookup). SKIP (perf-only).
- `73471ccd` — `get-in` `:inline` expansion + null-tolerant
  inliners. The port implements no `:inline` expansion at all
  (pre-existing gap, not a delta); null-tolerance is
  unobservable without inliners. SKIP (out-of-scope class).
- `8aa45d12` — type-hint meta propagation on macroexpansion
  (`(identity exp)` with form meta). Values identical; only
  host reflection metadata differs, which the port does not
  implement. SKIP (out-of-scope host machinery).

## Already-have (0)

None: the delta window contains no behavior the port already
covers. (`req!` getter calls and `!`-suffix directives predate
the window and stay as-is.)

## Tally

17 commits: 2 PORT (1 slice), 15 SKIP, 0 ALREADY-HAVE.
