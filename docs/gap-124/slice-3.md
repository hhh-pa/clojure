# Gap 124, slice 3 (finale) — tools.deps API namespace (source-only redesign)

Upstream oracle (text only, never executed): `org.clojure/tools.deps`
`0.31.1646` (`deps.clj`, `util/session.clj`, `util/concurrent.clj`),
read from the Maven Central jar; plus `clojure.set` (`set.clj`) and
`clojure.string` (`string.clj`, `blank?`/`join` only) from
`clojure/clojure` master for the two local set/string helpers.
No JDK/JVM anywhere in this slice. Slices 1 (`slice-1.md`) and 2
(`slice-2.md`) landed config + SPI + source manifests; this slice
ports the main `clojure.tools.deps` API namespace and closes #124 —
no further slices.

Naming note: the target namespace is `clojure.tools.deps`
(upstream's API ns name, `deps.clj`). Source compatibility requires
the exact name — `(require 'clojure.tools.deps)` must resolve — so
there is no separate `...deps.api` namespace.

No specification/TDD/BDD skills are installed in this workspace, so
this spec follows the standard spec-first shape: Problem / Solution /
BDD acceptance scenarios / TDD test plan, with every behavior pinned
to an upstream source file.

## 1. Problem

Slices 1–2 parse, validate, merge, canonicalize, and read manifests,
but nothing expands a dependency graph, computes a basis, or emits a
load path. The missing finale is the API layer: transitive expansion
(`resolve-deps` over the private `expand-deps` machine), the runtime
basis (`calc-basis`, `create-basis`), load-path emission
(`make-classpath-map`, `join-classpath`, `make-classpath`),
`print-tree`, the small pure utilities (`tool`, `resolve-added-libs`,
`prep-libs!` detection), the deprecated `edn` delegates, and the two
utility namespaces the API path needs (`util.concurrent`,
`util.session` scoping). Everything stays source-only: local dirs in,
source paths out, jars never.

## 2. Solution

Add the embedded `clojure.tools.deps` namespace (a near-verbatim
port of `deps.clj` minus the JVM), add `util.concurrent` (real
futures over `future-call`), finish `util.session`
(`with-session`, `retrieve-local`), and add three substrate natives
(`available-processors`, `path-separator`, `thread-id`). Two loader
rows in `ns.rs`. No other Rust.

### 2.1 Namespaces and functions

`clojure.tools.deps` (from `deps.clj` — public API):

| fn | upstream | port note |
|---|---|---|
| `slurp-deps`, `root-deps`, `user-deps-path`, `find-edn-maps`, `create-edn-maps`, `merge-edns`, `combine-aliases` | deprecated delegates to `edn` | faithful incl. `{:deprecated}` meta and the `find-edn-maps` key remap (`update-keys` is native) |
| `lib-location` | `ext/lib-location` delegate | faithful |
| `resolve-deps` (2- + 3-arity) | expand → cut-orphans → lib-paths → download-libs | faithful; executor is an opaque token, no shutdown call (C4) |
| `print-tree` (+ private `make-tree`) | tree print via `coord-summary` | faithful |
| `make-classpath-map` (+ privates `chase-key`, `flatten-paths`, `validate-paths`, `tree-paths`, `sort-paths`, `flatten-libs`) | classpath assembly | faithful; local `str-blank?` (C5), string paths (C12) |
| `join-classpath` | `str/join` over `File/pathSeparator` | faithful over `port/path-separator` + local `str-join` (C3, C6) |
| `make-classpath` | deprecated 3-arity | faithful incl. `{:deprecated}` meta |
| `tool` | replace project deps/paths | verbatim |
| `prep-libs!` (+ private `qualify-fn`) | prep detection + exec | detection faithful; exec refuses (R4); local `qualified-ident?` (C8) |
| `calc-basis` (1- + 2-arity) | `with-session` + resolve + cp map | faithful |
| `create-basis` | sources → argmap → merge → basis | faithful |
| `resolve-added-libs` | added/conflict over resolve | faithful (conflict arm throws for `:local`, as upstream — see §4) |
| `find-latest-version` | procurer scan, `:mvn` first | EXCLUDED (X1) |

`clojure.tools.deps` privates (all ported): `excluded?`,
`update-excl`, `add-version`, `select-version`,
`selected-version`, `selected-coord`, `selected-paths`,
`parent-missing?`, `deselect-orphans`, `dominates?`,
`include-coord?`, `canonicalize-deps`, `trace+`, `next-path`,
`expand-deps`, `cut-orphans`, `lib-paths`, `download-libs`.
Local private helpers (no upstream counterpart):
`q-peek`/`q-pop` (C1), `set-intersection`/`set-difference` (C2),
`str-blank?` (C5), `str-join` (C6), `split-dollar` (C7),
`qualified-ident?` (C8), `child-path` (C12, the B5 shape).

`clojure.tools.deps.util.concurrent` (from `util/concurrent.clj`):

| fn | upstream | port note |
|---|---|---|
| `processors` | `availableProcessors` | via `port/available-processors` (C4) |
| `new-executor` | fixed thread pool | opaque `{:threads n}` token (C4) |
| `submit-task` | bound-fn + pool submit | `future-call` (binding conveyance ≡, C4) |
| `shutdown-on-error` | shutdown + await | nil no-op (C4) |
| `thread-factory` | JVM `ThreadFactory` | not ported (no counterpart; `future-call` owns threads) |

`clojure.tools.deps.util.session` (finish of `util/session.clj`):

| fn | upstream | port note |
|---|---|---|
| `with-session` | macro swapping a fresh map | list-built expansion, fresh `(atom {})`, finally-restore (C9) |
| `retrieve-local` | thread-keyed `retrieve` | via `port/thread-id` (C10) |

Substrate (`util.port`): `available-processors` (0-arity →
positive long), `path-separator` (0-arity → `":"`/`";"`), `thread-id`
(0-arity → per-thread monotonic long). Extension loading: the API ns
`:require`s the `local`/`git`/`deps` extensions (upstream `load`s
`maven`/`local`/`git`/`deps`/`pom`; `maven`/`pom` never load — R1/R2).

### 2.2 Adaptation ledger (slice-3 C-series; A/B series still hold)

- C1 FIFO queues over vectors (no `PersistentQueue`): `q-peek` is
  `first` (nil on empty), `q-pop` drops the head (empty stays empty —
  queue-pop parity, unlike vector `pop`), `conj`/`into` enqueue.
  BFS order preserved exactly. Plain path vectors keep native
  `peek`/`pop` (stack ends, as upstream).
- C2 `clojure.set` is unported: local `set-intersection` /
  `set-difference` copy the `set.clj` 2-arity bodies verbatim,
  including the nil arms (`intersection(nil,X)` → nil,
  `difference(X,nil)` → X) that the same-version narrowing path
  depends on.
- C3 `File/pathSeparator` → the `port/path-separator` native
  (platform-exact — deliberately NOT an A7-style pin: the classpath
  string is consumed by external tools that split on the platform
  separator).
- C4 `util.concurrent` over `future-call`: `new-executor` returns an
  opaque `{:threads n}` token (no `ExecutorService`);
  `submit-task` is `future-call` (binding conveyance ≡ upstream's
  `bound-fn*` + submit); `shutdown-on-error` is a nil no-op
  (nothing to shut down — in-flight local reads complete
  harmlessly and every caller throws right after);
  `resolve-deps` drops the `.shutdownNow` line.
- C5 Local `str-blank?`: nil → true, else every char in the
  Java-whitespace subset U+0009–000D, U+001C–001F, U+0020. Exotic
  Unicode spaces read non-blank (pinned divergence; realistic
  override values are ASCII).
- C6 Local 2-arity `str-join` ≡ `(apply str (interpose sep coll))`
  (equivalent to `string.clj` for all inputs: empty → `""`,
  per-item `str`).
- C7 Local `split-dollar`: exact Java-split semantics for the
  single-char `$` delimiter including trailing-empty drop (the only
  `str/split` use is `(first (split ... #"\$"))` in `excluded?`).
- C8 Local `qualified-ident?` via `ident?` + `namespace` (the port
  lacks it; one use in `qualify-fn`).
- C9 `with-session` is built with `list` (no syntax-quote per the
  loader discipline): `let`/`try`/`finally` heads stay UNQUALIFIED
  (the analyzer matches `finally` clauses and real specials only
  unqualified), core vars fully qualified, the session var via
  `(var clojure.tools.deps.util.session/session)`; the fresh
  session is `(atom {})`; `alter-var-root` + `finally` restore the
  prior atom (same scoping contract as upstream).
- C10 `retrieve-local` keys on the `port/thread-id` native
  (thread-local monotonic ids: unique among live threads ≡ JVM,
  never reused ⊃ JVM).
- C11 `memoized-deps` (`ConcurrentHashMap`) → a local `(atom {})`
  with check-then-swap (B2-style benign races; values are
  content-derived).
- C12 All `File` ops → strings: `(jio/file root)` → `root`,
  `(jio/file root ensure)` → `child-path` join, `.exists` →
  `file-exists?`, `.getAbsolutePath` ∘ canonicalize →
  `dir/canonicalize` (already absolute), `(jio/file (or dir "."))`
  → `(or dir ".")`. Rust's Windows verbatim prefix (`\\?\`) is
  stripped in `canonical-path` (review round 2: verbatim form
  forbids the `/` joins, breaking manifest lookup; the JVM never
  emits it).
- C13 Type hints dropped (no classes); `{:deprecated}` and
  `:arglists` metadata kept.
- C14 Extension `load` → `:require` (`local`/`git`/`deps` only).

### 2.3 Refusals, deferrals, exclusions

- R4 (new, permanent): prep EXECUTION is refused — `exec-prep!`
  throws source-only ex-info naming the prep fn (upstream shells
  `java -cp ... clojure.main`; there is no JVM here). Detection
  stays faithful: default `:error` still reports unprepped libs,
  `:debug` logging and already-prepped libs never reach the exec.
- X1 (excluded): `find-latest-version` — version discovery across
  procurers starting with `:mvn`. In the port the `:mvn` probe
  throws "not loaded" before reaching any source procurer, so the
  fn could only ever throw; excluded rather than shipped dead.
  (The upstream `(comment ...)` REPL block is excluded too —
  dev scratch over JVM/`:mvn` examples.)
- N1 holds: full-sha git coords canonicalize, then `manifest-type`
  defers to the git-network slice (surfaced through `resolve-deps`
  and pinned).
- R1/R2/R3 hold: `:mvn` parses but throws "not loaded" from
  `manifest-type` during expansion; jar roots refuse via R2;
  port-canonical message texts where the JVM is quoted.

## 3. BDD acceptance scenarios

```gherkin
Feature: tools.deps API namespace (source-only)

  Scenario: resolve a local diamond once
    Given root deps {a {:local/root A}} where A and B both depend on {d {:local/root D}}
    When the client calls resolve-deps
    Then D appears once with canonical :paths and :dependents [a b]

  Scenario: same lib at two local roots is an error
    Given lib d reachable at two different :local/roots
    When the client calls resolve-deps
    Then it throws "No known ancestor relationship between local versions"

  Scenario: per-path exclusions narrow on re-encounter
    Given A excludes x from L while B depends on L plainly
    When the client calls resolve-deps
    Then x is included via B's path with :parents #{[b l]}

  Scenario: overrides and defaults shape the graph
    Given override-deps swapping a root and default-deps filling a nil coord
    When the client calls resolve-deps
    Then the override root wins and the default coord resolves

  Scenario: calc-basis assembles the runtime basis
    Given a merged edn with :paths ["src"] and one local dep
    When the client calls calc-basis
    Then :libs, :classpath, and :classpath-roots come back with "src" first

  Scenario: classpath overrides replace or remove
    Given a resolved lib map
    When the client overrides one lib with a path and one with ""
    Then the first lib's :paths swap and the second lib vanishes

  Scenario: alias chains resolve in extra-paths
    Given :extra-paths [:more] and aliases {:more ["m1" "m2"]}
    When the client calls make-classpath-map
    Then m1 and m2 lead the classpath with :path-key :extra-paths

  Scenario: create-basis runs end to end
    Given a scratch project dir with deps.edn plus an :extra-deps alias
    When the client calls create-basis with :dir and :aliases
    Then :libs, :classpath-roots, :argmap, and :basis-config all agree

  Scenario: print-tree draws the tree
    Given a resolved lib map with depth
    When the client calls print-tree
    Then each lib prints its coord-summary indented under its parent

  Scenario: trace records the expansion
    Given resolve-args with :trace true
    When the client calls resolve-deps
    Then the lib map carries :trace metadata with :include/:reason entries

  Scenario: prep detection without execution
    Given a lib with :deps/prep-lib and a missing :ensure dir
    When the client calls prep-libs! with default action
    Then it throws listing the lib; with :prep action it refuses source-only

  Scenario: maven and jars fail through the API
    Given :mvn and jar-root coords
    When the client calls resolve-deps on each
    Then :mvn throws "not loaded" and the jar root throws the R2 refusal
```

## 4. TDD test plan

- `rust/crates/clj-eval/tests/tools_deps_api.rs` (new, ~45 tests):
  loader rows (`clojure.tools.deps`, `util.concurrent` via require);
  substrate natives (`available-processors` pos-int, `path-separator`
  in `#{":" ";"}`, `thread-id` stable-per-thread + differs across
  `future`, each with an arity-error arm for patch coverage);
  `util.concurrent` (`processors` pin, executor token shape,
  `submit-task` value + `with-dir` binding conveyance,
  `shutdown-on-error` nil); `with-session` (fresh inside, prior
  restored, restore-on-throw) + `retrieve-local` (compute-once,
  thread-scoped); every BDD scenario above incl. the error rows
  (two-roots throw, `:mvn` "not loaded", R2 jar refusal, N1 git
  not-yet, `:prep` R4 refusal, unprepped `:error` listing,
  already-prepped silence); deprecated delegates (all seven incl.
  `find-edn-maps` key remap); `tool` rows; `join-classpath`
  (multi/single/empty); `resolve-added-libs` (added set; changed
  root throws — the `:local` compare has no "older", as upstream);
  `:threads` + `:trace` resolve-args; empty-deps resolve (`{}`)
  and the empty-trace `with-meta`-on-nil NPE parity edge.
- Set/map assertions use in-eval `=` (order-free), never pr-string
  order, for multi-element sets (`:parents`, `:paths` sets).
- `tools_deps_config.rs` / `tools_deps_spi.rs` (affected): no
  changes expected — purely additive slice; both suites must stay
  green untouched.
- Oracle pins: each test cites `deps.clj` (`expand-deps`,
  `resolve-deps`, `make-classpath-map`, `create-basis`, `tool`,
  `prep-libs!`), `session.clj`, `concurrent.clj`, `set.clj`,
  `string.clj`.
- Gates: `cargo fmt --check`, `cargo clippy -p clj-eval
  --all-targets -- -D warnings`, new + config + spi suites green
  (full `clj-eval` suite if budget allows); every new Rust line
  covered by construction (natives + arity arms), complexity
  ratchet holds (tiny fns), geiger unchanged (no unsafe).

## 5. Risks

- Futures are real threads: results stay deterministic because every
  task value is content-derived and queue order is single-threaded;
  tests never assert timing, only values. Each test boots its own
  `Evaluator`, so the global session var cannot leak across tests.
- The empty-trace NPE (`with-meta` on nil) is shared upstream/port
  behavior — pinned as parity, not fixed.
- Embedded-file discipline holds: no syntax-quote, no `::keywords`,
  `finally` unqualified (the analyzer requires it), core vars
  qualified in the `with-session` expansion.
- `sort` stability + insertion-ordered maps/sets (both verified in
  the port) keep `:classpath-roots` order JVM-identical; tests pin
  representative orders.
