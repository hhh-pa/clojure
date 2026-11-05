# Gap 124, slice 2 — tools.deps extension SPI + source manifests (source-only redesign)

Upstream oracle (text only, never executed): `org.clojure/tools.deps`
`0.31.1646` (`extensions.clj`, `extensions/local.clj`,
`extensions/git.clj`, `extensions/deps.clj`, `extensions/pom.clj`,
`util/session.clj`, `util/io.clj`), read from the Maven Central jar.
No JDK/JVM anywhere in this slice. Slice 1 (`slice-1.md`) landed the
config layer; the main `clojure.tools.deps` API namespace
(`expand-deps`/`resolve-deps`/`calc-basis`/`create-basis`/
`make-classpath-map`/`print-tree`, `util/concurrent`) is slice 3.

No specification/TDD/BDD skills are installed in this workspace, so this
spec follows the standard spec-first shape: Problem / Solution / BDD
acceptance scenarios / TDD test plan, with every behavior pinned to an
upstream source file.

## 1. Problem

Slice 1 ends at `:local lib-location`: coords parse, validate, merge,
and combine, but nothing canonicalizes a coord, detects a manifest, or
reads a nested `deps.edn`. The next minimal layer is the extension SPI
(the `ext/*` multimethods) with source-capable methods: `:local`
end-to-end (canonicalize → dep-id → manifest-type → manifest reads),
`:deps` manifest reading (`coord-deps`/`coord-paths`/usage/prep),
git coord canonicalization/validation (pure arms only), and the two
utility namespaces the manifest path needs (`util.io`, `util.session`).
Git network fetch, Maven resolution, and jar/pom consumption stay out
(permanent or deferred, per the ledger below).

## 2. Solution

Extend the embedded `extensions` shell to the full SPI, add three
extension namespaces (`local`, `git`, `deps`) plus `util.io` and
`util.session`, and add one substrate native (`is-file?`). No new Rust
beyond the native and five loader arms; everything else is Clojure.

### 2.1 Namespaces and functions

`clojure.tools.deps.extensions` (shell: full SPI + markers):

| fn | upstream | port note |
|---|---|---|
| `manifest-types` | `["deps.edn" :deps, "pom.xml" :pom]` | exact vector (B9) |
| `detect-manifest` | exists+isFile probe per entry, first wins | faithful over `file-exists?`+`is-file?`, nil-safe join (B5) |
| `coord-type-keys` defmulti + `:default` | `#{}` default | faithful; shell also carries the `:mvn` stub (B1) |
| `procurer-types` | method-table keys minus `:default` | fixed `#{:local :git :mvn}` (B1) |
| `coord-type` | registry-driven dispatch | keeps slice-1 hardcoded kind detection (B1); verdicts identical, anti-drift test |
| `canonicalize` defmulti + `:default` | `[lib coord]` passthrough | faithful |
| `dep-id`, `manifest-type` defmultis + `:default` | `throw-bad-coord` | faithful, incl. both map arms |
| `coord-summary` + `:default` | `(str lib " " (coord-type coord))` | faithful |
| `license-info`, `license-info-mf` + `:default` | nil | faithful |
| `compare-versions` + `:default` | "Unable to compare versions" throw | faithful; vector dispatch via exact table match (B6) |
| `find-versions` + `:default` | `throw-bad-coord` | faithful |
| `find-all-versions` | mapcat over procurer types | faithful (lazy; `doall` to force) |
| `coord-deps`, `coord-paths`, `manifest-file`, `coord-usage`, `prep-command` + `:default` | `throw-bad-manifest` | faithful, both arms |
| `lib-location` + 4 methods | — | UNCHANGED from slice 1 (B8), except the `:git` message now says "git-network slice" |
| `:pom` methods | `pom.clj` (XML) | coord-deps/coord-paths REFUSE (R1 corollary); manifest-file faithful path; usage/prep nil (upstream TBD); license via `:default` nil (B10) |

`clojure.tools.deps.extensions.local` (from `local.clj`):

| fn | upstream | port note |
|---|---|---|
| `coord-type-keys :local` | `#{:local/root}` | faithful |
| `dep-id :local` | `{:lib :root}` | faithful |
| `canonicalize :local` | canonical root + `ensure-file` throw | faithful (`dir/canonicalize` returns the string already) |
| `manifest-type :local` | explicit `:deps/manifest` wins; file root → `:jar`; else detect | faithful; file arm is the R2 REFUSAL |
| `find-versions :local` | nil | faithful |
| `compare-versions [:local :local]` | 0 or "No known ancestor" throw | faithful (B6) |
| `coord-summary :local` | `(str lib " " root)` | faithful |
| `license-info :local` | merge manifest-type, delegate to `-mf` | faithful |
| `:jar` manifest methods | `find-pom`/jar reads | coord-deps/coord-paths REFUSE (R2); manifest-file/usage/prep nil (upstream-exact); license via `:default` nil (B10) |

`clojure.tools.deps.extensions.deps` (from `deps.clj`, whole file):

| fn | upstream | port note |
|---|---|---|
| `deps-map` (private) | session-cached `merge-edns [root read]` | faithful; session key is the canonical path (B3) |
| `coord-deps :deps` | `(seq (:deps ...))` under `with-dir` | faithful |
| `coord-paths :deps` | canonicalize entries, warn when external, strings vec | faithful; single canonicalize (B4) |
| `manifest-file :deps` | absolute path or nil | canonical path or nil (B3) |
| `coord-usage`, `prep-command :deps` | usage map / prep+exec-args or nil | faithful |

`clojure.tools.deps.extensions.git` (from `git.clj`, pure arms only):

| fn | upstream | port note |
|---|---|---|
| `auto-git-url`, `full-sha?` | pure | faithful |
| `coord-type-keys :git` | `#{:git/url :git/sha :git/tag :sha}` | faithful |
| `canonicalize :git` | validation + tag/sha resolution | pure arms faithful (both-shas/both-tags/unqualified-lib/no-url/missing-sha/prefix-sha/full-sha); any tag → NOT-YET (B11) |
| `dep-id`, `coord-summary :git` | via `to-canonical` | faithful (pure for full-sha coords) |
| `license-info :git` | via `manifest-type` | faithful shape (throws not-yet via manifest-type) |
| `compare-versions [:git :git]` | 0 / ancestor check | equal shas → 0; else NOT-YET (B11) |
| `manifest-type`, `find-versions :git` | procure / list tags | NOT-YET (B11) |

`clojure.tools.deps.util.io` (from `io.clj`): `printerrln` only
(`read-edn`/`slurp-edn`/`write-file` deferred — nothing in this slice
needs them; the `edn` ns owns reading). Private `join-space`
(`clojure.string` is unported, B7).

`clojure.tools.deps.util.session` (from `session.clj`): `session` atom
+ `retrieve` (1- and 2-arity). `with-session`/`retrieve-local` arrive
with the API ns in slice 3 (B2).

Substrate (`util.port`): one new native, `is-file?` (path string →
bool, false when missing — upstream `File/isFile`).

### 2.2 Adaptation ledger (slice-2 B-series; slice-1 A-series still holds)

- B1 Fixed extension registry. The port's defmulti has no
  `getMethodTable`, so `procurer-types` is the fixed set
  `#{:local :git :mvn}` and `coord-type` keeps slice-1's hardcoded
  kind detection (same verdicts as upstream's registry-driven
  version). `coord-type-keys` methods live upstream-layout (:local in
  `local`, :git in `git`, :mvn stub in the shell); an anti-drift test
  pins that every published key dispatches its kind.
- B2 `session` is an `(atom {})`; `retrieve` is check-then-swap
  (a repeated compute under races is benign — values are
  content-derived). No `with-session`/`retrieve-local` yet.
- B3 Session keys and `manifest-file` use canonical paths (upstream:
  absolute). Same file, stronger key stability.
- B4 `coord-paths :deps` canonicalizes once (the port's
  `canonicalize` returns strings; upstream's File→string final pass
  is unneeded).
- B5 `child-path` nil-guard: a nil dir joins to the bare file name
  (upstream `(jio/file nil f)` parity); `""` falls out of `str`.
- B6 Vector dispatch (`[:local :local]`, `[:git :git]`) resolves by
  exact table match; cross-type pairs fall to `:default` through
  element-wise `isa?` (port `method_for` parity).
- B7 `join-space` is duplicated in `util.io` (no `clojure.string`;
  mirrors upstream, which also has two `printerrln`s — `edn.clj`
  cannot depend on `io.clj`).
- B8 Layout rule: the shell owns ALL of `lib-location` (it must host
  the `:mvn` refusal, which has no namespace); extension namespaces
  carry the remaining SPI methods. Zero duplication, shell stays
  self-sufficient.
- B9 `manifest-types` keeps the upstream-exact vector including
  `"pom.xml" :pom`; `:pom` coord methods refuse (R1 corollary).
- B10 Unreadable-manifest rule: `coord-deps`/`coord-paths` refuse for
  `:jar`/`:pom` (empty deps would lie; jars must never reach the load
  path); `manifest-file` returns nil for `:jar` (upstream-exact) and
  the path for `:pom` (no XML involved); usage/prep return nil
  (upstream TBD-exact); license-info falls to `:default` nil
  ("unknown").
- B11 Git network deferral: any arm needing `gitlibs` (tag validity,
  tag/sha resolution agreement, procure, tag listing, ancestor check)
  throws not-yet ex-info naming the git-network slice. All pure arms
  are upstream-exact.
- B12 `file-separator` stays `/` and `nl` stays `\n` (A7 continued).

### 2.3 Refusals (permanent) and deferrals (temporary)

- R1 (slice 1, holds): `:mvn` parses but is refused at
  `lib-location`. Corollary this slice: `:pom` manifests are detected
  but `coord-deps`/`coord-paths :pom` refuse source-only; other `:mvn`
  dispatches land on upstream-exact `:default`s ("not loaded").
- R2 (lands this slice): a `:local/root` pointing at a file is
  refused in `manifest-type :local`, and explicit-`:jar` coords are
  refused in `coord-deps`/`coord-paths :jar`. One message, three
  sites: `"Source-only port: cannot use jar manifest for {lib}
  (source directories only)"`, data `{:lib :coord}`.
- N1 (temporary): `:git` lib-location (shell message, reworded to
  "git-network slice"), manifest-type, find-versions, tag
  canonicalization, cross-commit compare — all deferred to the
  git-network slice (needs the network decision from issue #124).

## 3. BDD acceptance scenarios

```gherkin
Feature: tools.deps extension SPI + source manifests

  Scenario: local canonicalize absolutizes
    Given coord {:local/root "rel/lib"} under a project dir
    When the client calls canonicalize on it
    Then it returns [lib coord] with an absolute :local/root

  Scenario: missing local lib is refused
    Given coord {:local/root "/no/such/dir"} that does not exist
    When the client calls canonicalize on it
    Then it throws "Local lib my/lib not found: /no/such/dir"

  Scenario: jar root is refused at manifest detection (R2)
    Given coord {:local/root "/abs/lib.jar"} pointing at a file
    When the client calls manifest-type on it
    Then it throws source-only ex-info naming the lib

  Scenario: deps manifest deps are read
    Given a local dir whose deps.edn has :deps {a/a {:local/root "../a"}}
    When the client calls coord-deps with manifest :deps
    Then it returns the ([a/a {...}] ...) entries

  Scenario: deps manifest paths resolve
    Given a local dir whose deps.edn has :paths ["src"]
    When the client calls coord-paths with manifest :deps
    Then it returns the canonical [".../src"] vector
    And a :paths entry outside the root warns on *err*

  Scenario: git full-sha canonicalizes
    Given lib my/lib with {:git/url "https://example.com/r.git" :git/sha "<40 hex>"}
    When the client calls canonicalize on it
    Then it returns the standard {:git/url :git/sha} form

  Scenario: git validation rejects bad coords
    Given libs/coords with both :sha and :git/sha, missing sha, prefix sha, or an unqualified lib
    When the client calls canonicalize on each
    Then each throws its upstream-exact message

  Scenario: git tags defer to the network slice (N1)
    Given a coord with :git/tag
    When the client calls canonicalize on it
    Then it throws not-yet ex-info naming the git-network slice

  Scenario: mvn never resolves
    Given coord {:mvn/version "1.2.3"}
    When the client calls manifest-type, dep-id, or compare-versions on it
    Then it throws the upstream-exact "not loaded" / "unable" errors
    And lib-location still refuses source-only (slice 1)

  Scenario: pom manifests are detected but refused
    Given a local dir with pom.xml and no deps.edn
    When the client calls detect-manifest then coord-deps :pom
    Then detection returns :pom and coord-deps refuses source-only
```

## 4. TDD test plan

- `rust/crates/clj-eval/tests/tools_deps_spi.rs` (new, ~35 tests):
  substrate `is-file?` rows; `manifest-types` pin; `detect-manifest`
  rows (deps/pom/both/empty); `coord-type-keys` + `procurer-types` +
  anti-drift rows; every `:default` method row (canonicalize
  passthrough, bad-coord/bad-manifest arms, summary, license nils,
  compare throw, find-all-versions doall-throws); `:pom` refusal +
  faithful rows; local canonicalize/dep-id/manifest-type(R2)/
  find-versions/compare/summary/license rows; `:jar` refusal + nil
  rows; deps coord-deps/coord-paths(+*err* warn)/manifest-file/usage/
  prep rows; session retrieve rows; io printerrln pin; git
  auto-git-url/full-sha?/canonicalize matrix/dep-id/summary/compare/
  not-yet rows.
- `tools_deps_config.rs` (affected): one assertion update — the `:git`
  lib-location message now says "git-network slice".
- Oracle pins: each test cites the upstream file/behavior
  (extensions.clj SPI, local.clj `canonicalize`/`manifest-type`,
  deps.clj manifest fns, git.clj validation, session.clj/io.clj).
- Gates: `cargo fmt --check`, `cargo clippy -p clj-eval --all-targets
  -- -D warnings`, the new suite plus the config suite green;
  llvm-cov floor holds (`is-file?` fully covered), complexity ratchet
  holds (small fns only), geiger unchanged (no unsafe).

## 5. Risks

- Session caching is process-global with no `with-session` scoping
  yet: tests must use unique scratch dirs per case (the slice-1
  `scratch()` pattern) so keys never collide.
- `find-all-versions` is lazy and set-ordered: tests `doall` it and
  accept either deterministic throw (git not-yet vs mvn not-loaded).
- Embedded files keep the spec_alpha.clj discipline: no
  syntax-quote, no `::keywords` (the loader reads the whole file
  before evaluating).
