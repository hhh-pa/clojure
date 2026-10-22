# T1192: Port completeness — divergences fixed, missing features built — Specification

- Task: https://orsay.hhh.pa/T1192
- Branch: `T1192-fix-divergences`
- Status: draft for spec review (slice FX0)
- Binary: `clojure-rs`, Cargo workspace at `rust/`
- Parent: T1174 (S1–S9 landed, JVM implementation removed in PR #11)

This spec has two parts: Part 1 is the user/functional specification
(scenarios, behavior, acceptance criteria); Part 2 is the technical
specification (inventories, verdicts, slices, TDD plan, CI impact).
The spec on this branch always describes this branch; fix slices
update it when behavior changes.

## Part 1 — User/functional specification

### 1.1 Goals

G1. Every eliminable divergence from JVM Clojure is eliminated:
    `rust/core-coverage.md` carries zero `divergent` rows except the
    structural floor (Part 2, D-inventory), and every contract pin
    marked DIVERGENCE either conforms or moves to the structural
    record with its mechanism.
G2. Every real `clojure.core` var resolves: the coverage file's
    `out-of-scope` rows shrink to the two non-vars
    (`clone-thread-bindings`, which does not exist on the JVM, and
    `pprint`, which is not in core). Type system, protocols,
    multimethods, transducers, sorted collections, RNG, regex, the
    number tower, STM, `*in*`, reader extensions, JIT, `spec`,
    and `core.async` are all in scope.
G3. Newly covered surface is JVM-observable-conformant, verified by
    oracle probes plus differential corpus entries at 100%.

### 1.2 Non-goals (explicit)

N1. JVM interop stays out: `new`, dotted access, `import`,
    `instance?`, `class`, `bases`, `supers`, `..`,
    `clojure.java.*`, `clojure.reflect`. (Types/protocols below
    are pure-Rust; nothing names a host class.)
N2. Binary compatibility with JVM-compiled artifacts stays out;
    `gen-class` defines a named type but emits no bytecode (no
    consumer exists without a JVM).
N3. JVM-volatile texts stay stable-by-design: JDK/vendor-varying
    messages (CCE shapes, NPE texts, `#object` hexes, impl-name
    `--NNNN` suffixes) keep their pinned stable forms — matching
    a moving target is not conformance.
N4. CI runs no JVM Clojure: probes execute locally against the
    oracle jar; CI enforces the checked-in pins only.

### 1.3 BDD scenarios

```gherkin
Feature: divergences fixed
  Scenario: macroexpand conforms
    When I run `clojure-rs eval '(macroexpand-1 (quote (let [a 1] a)))'`
    Then stdout is the JVM expansion `(let* [a 1] a)`

  Scenario: transducer arities work
    When I run `clojure-rs eval '(into [] (map inc) [1 2])'`
    Then stdout is `[2 3]`

  Scenario: map entries behave
    When I run `clojure-rs eval '(key (first {:a 1}))'`
    Then stdout is `:a` with JVM failure modes on non-entries

  Scenario: pre/post enforced
    When I run `clojure-rs eval '((fn [x] {:pre [(pos? x)]} x) -1)'`
    Then the exit code is non-zero and stderr carries the JVM assertion text

Feature: number tower
  Scenario: exact division
    When I run `clojure-rs eval '(/ 1 3)'`
    Then stdout is `1/3` and `(+ 1/3 1/3 1/3)` prints `1`

  Scenario: big promotion
    When I run `clojure-rs eval '(* 9223372036854775807 2)'`
    Then stdout is the full bigint, not an overflow error

Feature: collections and seqs
  Scenario: sorted access
    When I run `clojure-rs eval '(subseq (sorted-map :a 1 :b 2) > :a)'`
    Then stdout is `([:b 2])`

  Scenario: transducers reduce
    When I run `clojure-rs eval '(transduce (comp (map inc) (filter odd?)) + [1 2 3])'`
    Then stdout matches the JVM value

Feature: types, protocols, multimethods
  Scenario: record dispatch
    Given a record type with a protocol method
    When the program calls the method through the protocol
    Then the result matches the JVM

  Scenario: multimethod dispatch
    Given a `defmulti` on `:type` with two `defmethod`s
    When each is invoked
    Then results match the JVM, including hierarchy preference

Feature: state and IO
  Scenario: STM retries
    Given two futures `alter`ing one ref in `dosync`
    When both complete
    Then the ref holds a serializable result and no update is lost

  Scenario: input reads
    When I run `clojure-rs eval '(with-in-str "1 2" (read))'`
    Then stdout is `1`

Feature: platform surface
  Scenario: regex groups
    When I run `clojure-rs eval '(re-find #"(\d+)-(\d+)" "a12-34")'`
    Then stdout is the JVM group vector

  Scenario: random in range
    When I run `clojure-rs eval '(rand-int 100)'`
    Then stdout is an integer in [0, 100)

  Scenario: data readers
    When I run `clojure-rs eval '#inst "2024-01-01"'`
    Then stdout round-trips through the reader

  Scenario: spec conforms
    When a spec for even numbers conforms `4` and rejects `5`
    Then results match `clojure.spec.alpha`

  Scenario: async roundtrip
    Given a channel with one `>!` and one `<!`
    When the program runs
    Then the taken value equals the put value

  Scenario: JIT parity
    When I run the JIT over a ground expression
    Then its printed result equals the interpreter's
```

### 1.4 Acceptance criteria

A1. All scenarios in 1.3 pass on Linux and Windows CI artifacts
    (macOS via the `v*` tag path per PR #15).
A2. The differential corpus passes at 100%, including every new
    fix/feature entry; pre-existing entries keep passing unmodified
    (any intentional behavior change migrates its entry with the
    oracle output in the same commit).
A3. `rust/core-coverage.md`: zero `divergent` rows outside the
    structural floor, and `out-of-scope` rows exactly
    {`clone-thread-bindings`, `pprint`}; the coverage-matrix test
    enforces both.
A4. `docs/t1192/divergences.md` records every structural item with
    its forcing mechanism and pinning tests; the root README's
    ported/divergence sections are refreshed to match.
A5. Quality gates green throughout (fmt/clippy/deny/geiger/udeps/
    audit/complexity/100% line+patch coverage); loom/TLA+
    coverage extends over STM/async/concurrency changes.

## Part 2 — Technical specification

### 2.1 Divergence inventory and verdicts

Source: 21 `divergent` coverage rows + `*-contract.md` DIVERGENCE
pins + spec C1/C2/C3 deviations + s5d D13/coll-pins. Verdicts:
FIX (slice id), STRUCTURAL (forcing mechanism), INVESTIGATE
(probe first, verdict in-slice).

FIX:
- D-macroexpand: 9 analyzer builtins with `macroexpand-1`
  identity (`let loop fn binding letfn with-redefs
  with-bindings bound-fn locking`) → FX4. Oracle smoke done:
  JVM `(macroexpand-1 '(let [a 1] a))` is `(let* [a 1] a)`.
- D-xform: 7 seq-fn transducer arities + `into` plain-only →
  FT7 (needs the transducer protocol).
- D-mapentry: no `MapEntry` (`key`/`val` via `nth`, map seqs
  and `find` yield vectors, conj-over-pairs CCE) → FX2.
- D-prepost: `:pre`/`:post` silently ignored → FX3.
- D-printdup: `*print-dup*` declared but ignored → FX4b.
- D-streamprint: str realization up-front vs JVM in-flight
  interleaving → FX4b.
- D-astral-fmt: `%s` precision counts scalars, JVM counts
  UTF-16 units → FX4b.
- D-tsneg: `%ts` negatives truncate → FX1.
- D-time: `time` millis digits approximate → FX1.
- D-emptymeta: seq `empty` drops meta (inferred) → FX1
  (oracle-confirm first).
- D-cce-shape: `update`-on-transient / `keys`-of-non-map JVM
  bare/checkcast CCEs asserted as port full-CCEs → FX1.
- D-caseacc: hand-written `case*` shapes are port spec errors
  where the JVM raises deterministic IAEs → FX1 (NPE accidents
  stay structural, below).
- D-postfuture: post-shutdown `future` spawns (JVM rejects
  with `RejectedExecutionException`) → FX1 (shutdown flag).
- D-shutdrain: `shutdown-agents` blocks (JVM returns at once,
  drains async) → FX1 (async drain).
- D-avaroot: concurrent `alter-var-root` on one var may lose
  an update → FX1 (reentrant/sequence protocol).
- D-argv: AOT `main` drops `*command-line-args*` → FX1 (argv
  conveyance + spec update).
- D-charf: `float` returns exact doubles (JVM rounds via f32)
  → FT1 (with the tower's coercions).
- D-pctpop: `%c` rejects integers (JVM accepts int-range via
  Integer) → FT1 (accept int-range Longs; full Integer/Long
  split stays structural, below).
- D-div: `/` exact-or-double instead of ratios → FT1 (tower).
- D-zones: `format` zones UTC-only → FX6 (tz-data dependency
  decision in-slice).
- D-phase: eval-in-macro phase gaps assert port runtime errors
  instead of JVM compile errors → FX5
  (investigate-first; may downgrade).

INVESTIGATE (probe, then FIX or STRUCTURAL in-slice):
- I-nthf64: `nth` with f64 past 2^53, unprobed → FX1.
- I-huge: huge-long parse/print edges, untested → FX1.
- I-smapread: `sorted-map` "reads as `{}`" (D13; mechanism
  unclear) → FT6 (dies with sorted collections or pins).
- I-hashinf: `hash` over infinite lazies hangs, marked
  JVM-true → FX1 (confirm hang==JVM, then conformant).

STRUCTURAL (go to `divergences.md` with mechanism + pin):
- S-identity: `swap!`/`compare-and-set!` compare with `=`
  (values have no object identity; `identical?` unimplementable).
- S-delay: re-entrant `force` guards (JVM recurses to
  StackOverflowError, which aborts a Rust process).
- S-sendvia: `send-via` routes to the `send-off` pool (no
  host executors exist to name — N1).
- S-awaitfail: `await` on a failed agent throws (JVM blocks
  forever); S-postawait: post-shutdown `await` returns nil
  (JVM wedges). Intentional liveness deviations: reproducing a
  hang is not conformance.
- S-nsimports: `ns-imports` is always `{}` (no host classes).
- S-aritytxt: arity texts strip `--NNNN` impl suffixes
  (compile counters; unmatchable by construction).
- S-surrogate: `char` rejects surrogates; lone surrogates
  unrepresentable (Rust `char`/`str` cannot hold them).
- S-intlong: no Integer/Long split (reader yields Long;
  same-`=`/dup behavior already matches; only class
  introspection differs, and that is N1).
- S-astralix: astral seq/get/nth index scalars, JVM UTF-16
  units (yielding a lone surrogate is S-surrogate).
- S-volatile: E8/E10 stable texts (CCE shapes, NPE texts,
  `#object` hexes, `loop requires` suffix) — the JVM texts
  vary by JDK/vendor (N3).
- S-npeacc: `case*`/ex-data NPE accidents (undefined
  behavior, not a contract).
- S-exdata: spec-problem/quote ex-data payloads (spec is a
  feature; quote payload revisited in FT13 if applicable).

### 2.2 Feature inventory (from `out-of-scope` rows)

- F-tower: ratios, BigInt, BigDecimal (`bigint bigdec num
  rationalize`), reader/arithmetic/promotion/printing. Touches
  `=`, `compare`, `hash`, every arithmetic native: lands
  before numeric divergence fixes close.
- F-reader: `#=` eval reader, `#inst`/`#uuid`, `data_readers.clj`.
- F-in: `*in*`, `with-in-str`, `read`, `read-line`, `line-seq`
  (PushbackReader over stdin/strings).
- F-rng: `rand`, `rand-int`, `shuffle` (RNG choice in-slice:
  hand-rolled SplitMix64 vs `rand` crate).
- F-regex: `re-find`, `re-matches`, `re-seq`, `re-pattern`
  (`regex` crate; reader already reads `#"..."`).
- F-sorted: `sorted-map`, `sorted-set`, `-by` variants,
  `subseq`, `rsubseq` (ordered-map design in-slice; `rpds`
  has no sorted map).
- F-xduce: `transduce`, `sequence`, `eduction`, `completing`,
  `ensure-reduced`, `unreduced`, `preserving-reduced`,
  `halt-when` + all xform arities (closes D-xform).
- F-multi: `defmulti`, `defmethod`, `prefer-method`,
  `get-method`, `methods`, `remove-method` + hierarchy
  (`isa?`, `derive`, `underive`, `parents`, `ancestors`,
  `descendants` — added to coverage if absent).
- F-proto: `defprotocol`, `extend`, `extend-type`,
  `extend-protocol`, `satisfies?`, `extends?` (dispatch +
  method caching).
- F-types: `deftype`, `defrecord`, `reify`, `definterface`
  (fields, methods, metadata; records seq as maps).
- F-proxy: `proxy`, `gen-class`, `gen-interface`, `with-open`
  (proxy over F-types interfaces; gen-class named-only per
  N2; with-open over a Closeable protocol).
- F-stm: `ref`, `dosync`, `alter`, `commute`, `ensure`,
  `ref-set` + validators/history (MVCC; loom + TLA+ models
  required by the concurrency house rule).
- F-spec: `clojure.spec.alpha` (`s/def`, `s/fdef`, conform,
  explain; generator-dependency decision in-slice).
- F-async: `core.async` (channels, `go`, `<!`/`>!`/`alt`;
  runtime decision in-slice: std threads vs tokio).
- F-jit: LLVM JIT evaluation parity (inkwell execution
  engine; ground expressions first).
- `type` joins F-types (returns the type tag; class-based
  `instance?`/`class`/`bases`/`supers` stay N1).

### 2.3 Slices (order binds dependencies)

FX0 spec (this file) + oracle smoke (done: jar + probe work).
FX1 small deterministic fixes (D-tsneg/D-time/D-emptymeta/
  D-cce-shape/D-caseacc/D-postfuture/D-shutdrain/D-avaroot/
  D-argv + I-nthf64/I-huge/I-hashinf probes).
FX2 MapEntry (D-mapentry).
FX3 pre/post validation (D-prepost).
FX4 analyzer builtins as real macros (D-macroexpand: `let*`/
  `fn*`/`loop*` special forms + core macro defs + analyzer).
FX4b printer/writer (D-printdup/D-streamprint/D-astral-fmt).
FT1 number tower (F-tower; closes D-div; D-charf/D-pctpop
  land here; S-intlong documented).
FT2 reader extensions (F-reader). FT3 `*in*` (F-in).
FT4 RNG (F-rng). FT5 regex (F-regex). FT6 sorted (F-sorted;
  closes I-smapread). FT7 transducers (F-xduce; closes D-xform).
FT8 multimethods (F-multi). FT9 protocols (F-proto).
FT10 types (F-types; `type` flips here).
FT11 proxy/gen-class/with-open (F-proxy).
FT12 STM (F-stm). FT13 spec (F-spec). FT14 core.async (F-async).
FT15 JIT (F-jit). FX5 phase gaps (D-phase). FX6 zones (D-zones).
FX7 structural record (`divergences.md`) + coverage ratchet +
  README refresh (closes A3/A4).

Each slice lands as its own `T1192:` commit series + PR with
`/review-loop` to acceptance; landing stays explicit-only.

### 2.4 TDD and oracle plan

- Oracle: the root `clojure.jar`
  (`1.13.0-master-SNAPSHOT`, same build as the T1174 pins;
  untracked, local-only) driven by system `java` with
  `-Duser.language=en -Duser.country=US -Duser.timezone=UTC`.
  Fresh environments fall back to stock `org.clojure/clojure`
  1.12.6 from Maven Central; the jar version is recorded in
  every new pin, and disagreements resolve in-slice.
- Per item: oracle probe first (new `docs/t1192/probes/`),
  failing test/corpus entry second, fix third, pin fourth.
  Property tests where the shape fits (tower laws, sorted
  model conformance, regex oracle differential on corpora).
- Concurrency house rule applies: STM/async/concurrency
  changes ship loom schedule tests + TLA+ models alongside
  unit tests, in the same slice.
- Coverage: the 100% line + patch floors hold for every new
  line; `divergences.md` + coverage-file flips are reviewed
  artifacts, not drive-bys.

### 2.5 Constraints

- Stable Rust, MSRV 1.85; fmt + clippy `-D warnings` clean.
- LLVM stays pinned major 22; inkwell single feature.
- Every new dependency passes `cargo-deny` review in its
  slice (license + duplicate-source policy) with an MSRV
  check; std-only solutions win ties.
- EPL-1.0 headers on new sources; unsafe only in audited
  FFI-style shims with geiger review.
- Branch protection is live: every slice PR needs the
  michael-h-hu approval to merge; CODEOWNERS auto-requests it.

### 2.6 CI impact

No workflow changes expected: fixes ride `rust.yml` (12 jobs,
Linux/Windows) and releases ride `rust-release.yml` (3-OS on
`v*`); growth is corpus/coverage/pins only. New crates enter
the workspace only with their gates (geiger/udeps/audit pass,
coverage floors). No Jenkinsfile/.arclint changes needed
(GitHub-hosted repo).
