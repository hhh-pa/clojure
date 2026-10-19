# T1174: Native Rust port of the Clojure runtime, REPL, and compiler — Specification

- Task: https://orsay.hhh.pa/T1174
- Branch: `T1174-rust-port`
- Status: draft for spec review (slice 0)
- Binary: `clojure-rs`, Cargo workspace at `rust/`
- Codegen: LLVM via inkwell, pinned LLVM 22 (Cranelift rejected)

This spec has two parts: Part 1 is the user/functional specification
(scenarios, behavior, acceptance criteria); Part 2 is the technical
specification (contracts, constraints, slices, TDD plan, CI impact).
The spec on this branch always describes this branch; implementation
slices update it when behavior changes.

## Part 1 — User/functional specification

### 1.1 Goals

G1. A single self-contained native binary, `clojure-rs`, that evaluates
    a documented subset of Clojure with no JDK installed.
G2. Four entry points: `eval` (expression), `run` (script file),
    `repl` (interactive), `build` (LLVM AOT of the pure subset).
G3. Behavior compatible with stock JVM Clojure on the implemented
    subset, verified by a differential corpus (same input, same
    printed output).
G4. CI parity with `../arcanist`: fmt+clippy, tests on Linux/macOS/
    Windows, quality gates, per-commit release artifacts, and `v*`
    tag releases with attached archives.
G5. The JVM build (`pom.xml`, Maven workflows) keeps working untouched.

### 1.2 Non-goals (explicit)

N1. JVM interop: dotted method/field access, `new`, `definterface`,
    `genclass`, `proxy`, `reify` host members, `clojure.java.*`,
    `clojure.reflect`, `import` of JVM classes.
N2. Full `core.clj` compatibility. `core.clj` is JVM-coupled; the port
    ships a Rust-backed core subset with a published coverage list.
N3. Big-number tower in this task: ratios, BigInt, BigDecimal are a
    follow-up. Implemented numbers: 64-bit integers (checked,
    overflow is an error, not silent wrap) and IEEE-754 doubles.
N4. STM (`ref`/`dosync`/validators on refs) in this task; `atom`
    (with validators), `delay`, `future`, `agent` are in.
N5. LLVM JIT, `clojure.spec`, `core.async`, tools.deps, data readers
    needing I/O (`#=` eval reader), and binary compatibility with
    JVM-compiled artifacts.

### 1.3 BDD scenarios

```gherkin
Feature: eval a single expression
  Scenario: integer arithmetic
    Given the `clojure-rs` binary built for my platform
    When I run `clojure-rs eval '(+ 1 2 3)'`
    Then the exit code is 0 and stdout is exactly "6\n"

  Scenario: error reporting
    Given the `clojure-rs` binary
    When I run `clojure-rs eval '(/ 1 0)'`
    Then the exit code is non-zero
    And stderr contains "Divide by zero" and the failing form's position

Feature: run a script file
  Scenario: hello world script
    Given a file `hello.clj` containing `(println "hello")`
    When I run `clojure-rs run hello.clj`
    Then the exit code is 0 and stdout is exactly "hello\n"

  Scenario: missing file
    Given no file `nope.clj`
    When I run `clojure-rs run nope.clj`
    Then the exit code is non-zero and stderr names the missing file

Feature: interactive REPL
  Scenario: persistent session state
    Given I start `clojure-rs repl`
    When I enter `(def x 41)` then `(+ x 1)`
    Then the second result prints `42`
    And command history persists across restarts (history file)

  Scenario: multi-line input and completion
    Given I start `clojure-rs repl` on a tty
    When I enter an unbalanced form and press Enter
    Then the REPL continues the form on the next line instead of erroring
    And Tab completes known `clojure.core` names

Feature: AOT build of the pure subset
  Scenario: compile and run
    Given a file `f.clj` with a pure function over integers
    When I run `clojure-rs build f.clj -o f`
    Then `./f` exits 0 and prints the same result as `clojure-rs run f.clj`
    And forms outside the pure subset fail `build` with a diagnostic
      naming the unsupported form (they still work under `run`)

Feature: no JDK required
  Scenario: bare machine
    Given a machine with no JDK and no Clojure jar
    When I download the release archive for my platform and extract it
    Then `clojure-rs eval '(+ 1 2)'` prints `3`
```

### 1.4 CLI contract

- `clojure-rs --help` exits 0 and documents all subcommands.
- `clojure-rs --version` exits 0 and prints `clojure-rs <cargo-version>`.
- `clojure-rs eval <form> [--edn]` evaluates one form; `--edn`
  restricts the reader to EDN. Exit 0 with the printed result on
  success; non-zero with a `file:line:col: message` diagnostic on
  failure.
- `clojure-rs run <file.clj> [args...]` loads the file with `*command-line-args*`
  bound to `[args...]`. Exit 0 on success; non-zero with a diagnostic
  naming file/line/col on failure.
- `clojure-rs repl` starts the interactive REPL (see 1.5). Non-tty
  stdin degrades to streaming eval (one form per result, no line
  editing, no history write).
- `clojure-rs build <file.clj> -o <out>` AOT-compiles the pure subset
  (see 2.4). Exit non-zero with a diagnostic naming the first
  unsupported form when the input leaves the subset.

### 1.5 REPL behavior

Maps `clojure.main/repl` essentials: print prompt, read one form,
eval, print result, loop; `*1`, `*2`, `*3` bound to recent results;
`*e` bound to the last error. Ctrl-C cancels the current input;
Ctrl-D (empty line) exits 0. History file:
`~/.clojure-rs/history` (overridable by `--history-file`,
disable with `--no-history`). Interrupt/error never kills the
session; the error value prints and the loop continues.

### 1.6 Language subset (functional definition)

Reader: symbols, keywords (auto-resolve `::` in loaded namespaces),
integers, doubles, strings (escapes + unicode), chars, booleans,
nil, lists, vectors, maps, sets, quoting (`'`, `` ` `` with
unquote/unquote-splicing), deref `@`, meta `^`, arg `%` sugar,
comments (`;`, `#_`), dispatch macros (`#{`, `#"`, `#(`, `#'`,
`#_`, `#?` reader conditionals with `:cljr` features). Out:
`#=` eval reader, `#inst`/`#uuid` data readers (follow-up),
custom `data_readers.clj`.

Special forms: `def`, `if`, `do`, `let`/`letfn`, `loop`/`recur`,
`fn` (multi-arity, variadic, pre/post), `quote`, `var`, `throw`,
`try`/`catch`/`finally`, `set!` (dynamic vars only), `monitor-enter`
is out, `new`/`.`/`import` out (N1), `deftype`/`defrecord` (methods
without host interop) in, `defprotocol`/`extend-protocol`/
`extend-type` in, `defmulti`/`defmethod` in.

Core subset (seed list; the coverage file `rust/core-coverage.md`
is normative and grows per slice): `+ - * / quot rem mod inc dec
< <= > >= = == not not= nil? some? true? false? boolean? number?
integer? string? keyword? symbol? fn? ifn? coll? sequential? map?
vector? set? list? count empty? seq first second rest next cons conj
assoc dissoc get contains? keys vals select-keys merge merge-with
zipmap vec vector list list* hash-map hash-set sorted-map sorted-set
map filter remove mapv filterv reduce reductions take drop take-while
drop-while range repeat repeatedly iterate cycle interleave interpose
concat apply comp partial juxt constantly identity
complement every? some every-pred some-fn keep keep-indexed
map-indexed distinct dedupe partition partition-all partition-by group-by
frequencies sort sort-by max min max-key min-key str print println pr prn
newline read-string slurp spit format subs count get subs keyword symbol
name namespace gensym macroexpand macroexpand-1 eval load-file require
use in-ns ns create-ns the-ns ns-name find-ns all-ns refer binding push-thread-bindings
pop-thread-bindings with-redefs declare intern ns-interns ns-publics resolve
var-get var-set alter-var-root bound? thread-bound? defn defmacro defonce
comment assert doseq dotimes dorun doall for when when-not when-let if-let
if-some when-some cond condp case and or lazy-seq delay force future
deref realized? add-watch remove-watch atom swap! reset! compare-and-set!
send send-off await shutdown-agents pprint clojure-version *command-line-args*
*ns* *out* *err* *in* *print-length* *print-level* *1 *2 *3 *e`.

Semantics on the subset must match JVM Clojure observably (printed
representation, equality, ordering, truthiness, nil-punning,
laziness/chunking observability via side-effect order, dynamic
binding conveyance to futures/agents, agent error modes).

### 1.7 Acceptance criteria

A1. All scenarios in 1.3 pass on Linux, macOS, and Windows CI artifacts.
A2. The differential corpus (Part 2, 2.7) passes at 100%: every corpus
    entry on the implemented subset prints byte-identical output on JVM
    Clojure and `clojure-rs`; out-of-subset entries assert their
    documented divergence.
A3. `rust.yml` + `rust-release.yml` green; a `v*` dry-run produces 3
    archives as run artifacts (nothing attaches without a tag; the publish
    path stays untested until the first tag); JVM workflows untouched and green.
A4. Quality gates (fmt/clippy/deny/geiger/udeps/audit/complexity/
    coverage) green with no new warnings.
A5. `rust/core-coverage.md` lists every implemented var with its
    conformance state; anything absent is out of scope, not silently
    missing.

## Part 2 — Technical specification

### 2.1 Architecture

```text
rust/
  Cargo.toml            workspace: clojure-rs (bin) + clj-* libs
  core-coverage.md      normative core var coverage list (see A5)
  deny.toml             license/bans/sources policy (seeded from arcanist)
  src/bin/clojure-rs/   CLI: eval/run/repl/build (clap)
  crates/
    clj-reader/         reader + printer, spans on every form
    clj-value/          Value model, persistent collections (rpds), seqs
    clj-eval/           analyzer + tree-walking interpreter
    clj-core/           Rust-backed clojure.core subset + embedded .clj
    clj-repl/           rustyline REPL loop
    clj-aot/            LLVM backend via inkwell (slice 8 only)
  tests/                integration suites + fixtures/ + differential corpus
```

Crate split keeps the interpreter buildable/testable without LLVM;
`clj-aot` is an optional dependency of the binary, enabled by
default but disableable (`--no-default-features`) so `cargo test -p`
on the other crates never needs an LLVM install.

Value model: a single `Value` enum (`Nil Bool Int(i64) Double(f64)
Str Char Keyword Symbol List Vector Map Set Fn Var Atom ...`),
immutable and `Send+Sync`, shared by `Arc` (no tracing GC: values
are persistent; drops reclaim). Interop surface for AOT: the
native side boxes through the same `Arc`-shared representation;
rooting strategy is designed in the slice-8 spec update.

### 2.2 Contracts

C1. Numbers: `Int` is i64 with checked arithmetic; overflow raises a
    catchable arithmetic error (never wraps). Mixed int/double ops
    promote to double. `/` on ints is exact only when divisible,
    else double division (documented deviation from ratio behavior
    until the big-number follow-up; the corpus pins it).
C2. Equality: `=` is same-category equality (`(= 1 1.0)` is
    false — `Numbers.equal` gates on category, probe17; `(==
    1 1.0)` is the cross-category comparison, S4); `==` is
    numeric equality across categories. Ordering is JVM
    `compare` (numeric across Int/Double with NaN-is-0,
    within-type otherwise, incomparable pairs error).
C3. Printing: `pr` round-trips through the reader for EDN values;
    `print`/`println` are human-readable; `*print-length*`/
    `*print-level*` truncate with `...` when bound to a
    non-negative int or integral double (nil, negative,
    fractional non-integral, and NaN are unlimited; a
    non-positive level prints `#`). Stack traces are Rust-side
    diagnostic chains (`file:line:col: message`, `:cause` chains),
    not JVM stack dumps — a documented deviation.
C4. Seqs: lazy, chunked (32), with nil-punning; side-effect order of
    `map`/`filter` over chunked seqs matches JVM observably.
C5. Namespaces/vars: intern, root + thread-local bindings, dynamic
    conveyance to `future`/`send` bodies, `alter-var-root`
    atomicity, `with-redefs` scope. Macroexpansion is interpreter
    evaluation of the macro fn on unevaluated forms.
C6. Concurrency: `atom` CAS loops with validators + watches;
    `future` conveys bindings and caches exceptions for `deref`;
    `agent` fixed pool, `:fail`/`:continue` error modes, `await`,
    nested-send ordering. STM excluded (N4).
C7. Reader conditionals use `:cljr` (+ `:default`); `.cljc` loads
    JVM branches never.
C8. CLI exit codes: 0 success; 1 evaluation/compile error; 2 usage
    error; 3 I/O error (missing file etc.).

### 2.3 Constraints

- Toolchain: stable Rust, MSRV 1.85 (inkwell floor). `cargo fmt
  --check` and `cargo clippy --workspace --all-targets -- -D warnings` clean.
- LLVM: pinned major 22 everywhere (dev + CI + gate image);
  `inkwell` single feature `llvm22-1` (inkwell 0.10.0 supports up to LLVM 22); CI asserts the major.
- Dependencies: mirrored deny policy (MIT/Apache-2.0/BSD-3/BSL/
  Unicode/Unlicense/Zlib + LLVM-exception); `multiple-versions =
  "deny"`; `rpds`/`rustyline`/`clap` approved pending gate pass.
- Unsafe: no new `unsafe` outside FFI shims; geiger ratchet starts
  at the vendored baseline and only tightens.
- New sources carry the repo-standard EPL-1.0 header (verbatim
  block, `//` style); vendored Apache-2.0 CI files keep theirs.
- Zero changes to JVM sources, `pom.xml`, `build.xml`, Maven
  workflows. New CI files only: `rust.yml`, `rust-release.yml`
  (`release.yml` name is taken by the Maven release).
- `v*` tags are the Rust release namespace (existing tags all use
  `clojure-`); no tag is cut in this task (dry-run only).

### 2.4 AOT pure subset (slice 8)

Compilable: top-level `defn` of fixed-arity fns over Int/Double/
Bool/Str/Vector value flow, `if`/`do`/`let`/`loop`-free straight
code + direct self-recursion, calls to a closed primitive set
(`+ - * quot < <= > >= = count nth get conj` on the value types).
Everything else (vars, laziness, macros at runtime, concurrency,
I/O beyond a `print` intrinsic) is a compile error naming the
form. Linking uses the platform linker against the `clj-value`
runtime archive. JIT is a follow-up, not this task.

### 2.5 Slices (bind the plan's steps 1–9)

S1 scaffold+CI: workspace, `--help`/`--version`, vendored CI,
  LLVM provisioning, green on 3 OSes. TDD: CLI smoke tests first.
S2 reader+printer: reader per `LispReader.java` behavior
  (`EdnReader` out of S2 scope per reader contract §24 item 11),
  printer per C3, corpus harness. TDD: round-trip
  property tests + differential entries first.
S3 values+collections: `Value`, rpds-backed types, transients,
  seqs (C4), equality/ordering (C2). TDD: model tests + threaded
  sharing test first.
S4 evaluator: analyzer + special forms (1.6), invocation, errors
  (C8 diagnostics). TDD: per-form suites + differential first.
S5 namespaces/core: C5, embedded core sources, coverage file.
  TDD: coverage-matrix test fails until each var lands.
S6 concurrency: C6 minus STM. TDD: stress/determinism tests first.
S7 repl+cli: 1.4/1.5, packaging smoke on artifacts.
  TDD: PTY-less REPL driver tests + artifact smoke first.
S8 aot: 2.4, differential vs interpreter. TDD: subset corpus first
  (unsupported-form diagnostics asserted too).
S9 hardening: corpus growth, ratchet floors, docs, follow-up tasks.

Each slice lands as its own `T1174:` commit series + PR; the spec
updates in-slice when behavior changes.

### 2.6 TDD plan

- Unit tests live with each crate (`cargo test -p clj-*`).
- Property tests (reader round-trip, collection model conformance)
  run in CI on Linux; fixed seeds, no nondeterminism.
- Integration tests in `rust/tests/` mirror arcanist layout:
  `common/` harness, `fixtures/`, one file per area.
  Crate-local harnesses with crate-local fixtures (e.g. the S2
  reader corpus under `clj-reader/tests/`) use the same layout
  in place.
- Differential oracle: stock JVM jar built once per run from the
  untouched Maven build; corpus entries are `(input, expected)`
  pairs where `expected` is JVM-printed output captured at seed
  time and checked in. Mismatches fail CI with a diff.
- Coverage: llvm-cov line floor ratchets from the S1 baseline;
  patch coverage is deferred to S9 hardening (not enforced per-slice).

### 2.7 CI impact

New: `.github/workflows/rust.yml` (fmt+clippy / test×3 /
gates / build-release×3), `.github/workflows/rust-release.yml`
(`v*` → build×3 → attach archives), `.github/actions/
profile-runner`, `scripts/ci/go`, `scripts/ci/
quality-gates.Dockerfile` (+LLVM 22), `codecov.yml`,
`rust/deny.toml`, geiger/complexity baselines. Secrets/vars to
provision (non-blocking: local-only image, token-less upload):
`GATE_IMAGE` namespace, `DOCKERHUB_*`, `CODECOV_TOKEN`.
Untouched: `test.yml`, `release.yml`, `snapshot.yml`,
`doc-build.yml`, all Maven files. No Jenkinsfile/.arclint
changes needed (GitHub-hosted repo).
