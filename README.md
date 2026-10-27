# clojure-rs — native Clojure in Rust

A self-contained native Clojure runtime, REPL, and LLVM AOT compiler
with no JDK required, tracking upstream Clojure `1.13.0-alpha9`
(`clojure-version` reports the ported release; tagged `v*` releases
ship per-platform binaries). This repository **is** the port: the original
Java implementation (sources, `pom.xml`, Maven workflows) was removed
in PR #11 once the port completed its specification; the Rust
workspace below is the sole implementation.

- Binary: `clojure-rs` (`eval`, `run`, `repl`, `build`).
- Language: a documented Clojure subset, differentially verified
  against JVM Clojure (same input, same printed output).
- Codegen: LLVM only, via inkwell 0.10.0 (`llvm22-1`, LLVM 22.1.8).
- License: EPL-1.0 (`epl-v10.html`).

Normative details live with the code: the
[core coverage list](rust/core-coverage.md) (machine-checked by CI),
the [design spec](docs/t1174/specification.md) and its contracts
(`docs/t1174/*-contract.md`), and the [workspace
README](rust/README.md) (LLVM setup, build commands).

## Quick start

Prerequisites: stable Rust ≥ 1.85 (MSRV). The default `aot` feature
needs LLVM 22 — see [LLVM setup](rust/README.md#llvm-setup-the-aot-feature);
without LLVM, build the interpreter with `--no-default-features`.

```sh
cd rust
cargo test --workspace --locked     # full suite
cargo build --release --locked      # ./target/release/clojure-rs
```

```console
$ clojure-rs eval '(+ 1 2 3)'
6
$ clojure-rs eval '(map inc [1 2 3])'
(2 3 4)
$ clojure-rs run hello.clj          # prints hello
$ clojure-rs repl                   # interactive, history at ~/.clojure-rs/history
$ clojure-rs build hello.clj -o hello && ./hello
42
```

`eval` takes `--edn` (EDN-only reader); `run` binds extra args to
`*command-line-args*`; `repl` takes `--history-file` / `--no-history`;
`build` takes `--emit-ir` / `--emit-obj` (skip linking) and
`--runtime-archive` (default: next to the binary — release archives
ship both). Exit codes: 0 success, 1 evaluation/compile error
(`file:line:col: message`), 2 usage error, 3 I/O error.

Prefer a download? Every CI run keeps per-platform
`clojure-rs-<target>.tar.gz` artifacts (binary + runtime archive),
and each `v*` tag attaches the same three archives to its GitHub
release.

## What is ported

**CLI.** All four entry points: `eval` (one form), `run` (script
file), `repl` (rustyline: history, `clojure.core` completion,
multi-line input, `*1`/`*2`/`*3`/`*e`, Ctrl-C cancels, Ctrl-D exits;
non-tty stdin degrades to streaming eval), `build` (LLVM AOT, below).

**Reader.** Symbols, keywords (incl. auto-resolve `::`), integers,
doubles, strings (escapes + unicode), chars, booleans, nil, lists,
vectors, maps, sets, quoting (`'`, `` ` `` with unquote/splicing),
deref `@`, meta `^`, `%` arg sugar, comments (`;`, `#_`), dispatch
macros (`#{`, `#"`, `#(`, `#'`, `#_`, `#?` reader conditionals with
`:cljr` features — `.cljc` never loads JVM branches). Every form
carries a span for diagnostics.

**Types (no bytecode).** `deftype defrecord reify proxy
definterface gen-interface gen-class` over a namespace registry
(`Type.` construction, `.-field` / `.method` interop, `->Type` /
`map->Type` factories); records are maps with the type name on
them (map ops, `hash` formula, metadata); methods dispatch
through per-type tables with `Object` fallbacks (`toString`,
record `hashCode`).

**Protocols.** `defprotocol` (map descriptors with `:on` / `:sigs`
/ `:impls`, variadic dispatch fns), `extend` / `extend-type` /
`extend-protocol` (named types, host classes, `nil`, and the
`IPersistentMap` fallback for maps/records, merged under the
var-alter lock), `satisfies?` / `extends?` (registry plus inline
claims; descriptors never satisfy), inline bodies on
`deftype` / `defrecord` / `reify` / `proxy`, the `Closeable`
contract, and `with-open` (reverse-order `finally` closes).
`String Long Double Boolean Object` resolve as class-name
strings (the port has no class literals).

**Multimethods.** `defmulti` (dispatch fn, docstring/attr head,
`:default` option only — `:hierarchy` is rejected until the
`derive` / `isa?` follow-up), `defmethod` (single- and
multi-arity tails), `prefer-method` (edges stored, never
consulted), `get-method` / `methods` / `remove-method` (exact
entry else default entry, whole-table view, missing-entry
no-op). Dispatch is exact-value plus the default entry; misses
fail with `No method in multimethod 'name' for dispatch value:
<pr>`; table updates install a new var root under the var-alter
lock, so values captured before an update keep their old table
(the JVM mutates the one object).

**Language core.** Special forms `def if do let letfn loop recur fn
fn* quote var throw try/catch/finally set!` (`set!` on dynamic vars
only); destructuring; `defn defmacro defonce comment assert`;
control flow (`cond condp case and or when when-not when-let if-let
if-some when-some doseq dotimes dorun doall for lazy-seq`); full
seq/collection/string I/O surface (`map filter remove mapv filterv
reduce take drop range repeat repeatedly iterate cycle interleave
concat apply comp partial juxt constantly identity complement ...`,
`slurp spit format`, `read-string`); transients (`transient`
/ `persistent!`, single-threaded by `&mut`); `eval load-file
load-string macroexpand gensym`.

**Namespaces, vars, macros.** `ns in-ns create-ns remove-ns find-ns
all-ns the-ns ns-name ns-aliases ns-resolve resolve alias refer use
require load loaded-libs`, `*ns* *file* *load-path*` (port-only
dirs vector), `intern declare ns-interns ns-publics ns-refers ns-map
ns-unmap ns-unalias`, roots + thread-local bindings
(`push-thread-bindings`, `with-bindings*`, conveyance to futures
and agent actions), `alter-var-root`, `with-redefs`, metadata
(`meta with-meta vary-meta alter-meta! reset-meta!`). Macros are
real: expansion evaluates the macro fn on unevaluated forms.

**Concurrency (no STM).** `atom` (validators, watches, `swap!`
/ `reset!` / `compare-and-set!`), `delay` / `force`, `future`
(binding conveyance, cached failures) / `promise` / `deliver`,
`pcalls` / `pvalues`, agents (`send` on a fixed pool, `send-off`
unbounded, `:fail` / `:continue` error modes, `await` / `await-for`,
`restart-agent`, `shutdown-agents`), `locking`, `add-watch` /
`remove-watch`, `realized?`.

**Core vars: 400 implemented, 2 divergent, 21 out of scope**
([`rust/core-coverage.md`](rust/core-coverage.md) is normative and
CI-enforced: listed vars must resolve, excluded ones must not).

**AOT (`build`).** Compiles the pure subset — top-level fixed-arity
`defn` over Int/Double/Bool/Str/Vector value flow, `if`/`do`/`let`
+ direct self-recursion, calls to a closed 13-primitive set
(`+ - * quot < <= > >= = count nth get conj`), `print` — to a
native binary linked against the shipped `clj-rt` runtime archive.
Anything else fails the compile with
`path:line:col: unsupported form in AOT subset: form (hint)`;
runtime failures print `path:line:col: Class: message` with the
true call-site span and exit 1. Host-only, no debug info, no
tail-call optimization (deep self-recursion overflows like the
JVM); `main` drops every slot and takes no arguments
(`*command-line-args*` is unconveyed). Primitives dispatch through
the interpreter's own natives, so AOT and `run` cannot drift.

## Divergences

Deliberate, documented, and pinned by the differential corpus —
each entry asserts its exact behavior, so drift fails CI:

- **No object identity.** Values are compared, never
  identical (`identical?` is out of scope): `swap!` /
  `compare-and-set!` publish-compare with `=`, where the JVM
  compares references.
- **Numbers.** `Int` is checked i64 — overflow raises a catchable
  arithmetic error, never wraps. `/` on ints is exact when
  divisible, else double division (no ratios — see below).
- **Equality/ordering.** `=` is same-category (`(= 1 1.0)` is
  false); `==` is the cross-category numeric comparison.
  `compare` matches JVM numeric ordering including NaN ties, but
  incomparable pairs fail with the stable `Cannot compare <A> to
  <B>` instead of JDK-version-dependent `ClassCastException` text.
- **Hashing is internal.** `hasheq` is only equiv-consistent —
  values never promise JVM-identical integers.
- **Printing.** `pr` round-trips EDN values through the reader;
  `*print-length*` / `*print-level*` truncate with `...`
  (`*print-dup*` is declared but ignored). Errors render as
  `file:line:col:` diagnostic chains, not JVM stack dumps.
- **Analyzer builtins.** `let loop fn binding letfn with-redefs
  with-bindings bound-fn locking` resolve as macros but
  `macroexpand-1` returns them unchanged (documented identity).
- **Agents.** `send-via` routes to the `send-off` pool (no custom
  executors); `await` on a failed agent throws instead of hanging;
  `shutdown-agents` blocks until drained (the JVM returns at
  once); post-shutdown `await` returns `nil` and post-shutdown
  `future` still spawns (the JVM wedges/rejects).
- **Safety pins.** Re-entrant `force` on a `delay` raises
  `IllegalStateException` (the JVM recurses to
  `StackOverflowError`, which would abort a Rust process);
  `locking` holds one global reentrant monitor (the JVM locks
  per-object). `alter-var-root` serializes per var like the
  JVM (same-thread nesting re-enters; the outer install
  wins).
- **Namespaces.** `ns-imports` is always `{}` (no host classes);
  `*loaded-libs*` is a plain set, not a ref.

## Out of scope

Absent from [`rust/core-coverage.md`](rust/core-coverage.md) means
out of scope, not silently missing:

- JVM interop (`new`, dotted access, `import`, `instance?`,
  `class`, `..`, `clojure.java.*`, `clojure.reflect`).
- STM (`ref dosync alter commute ensure ref-set`) — never;
  `*in*` machinery (`read read-line line-seq with-in-str`).
- Reader: `#=` eval reader, `#inst` / `#uuid` data readers,
  custom `data_readers.clj`.
- LLVM JIT, `clojure.spec`, `core.async`, and binary
  compatibility with JVM-compiled artifacts.

## Memory management

There is no tracing garbage collector. Values are immutable and
`Send + Sync`, shared by `Arc`; persistence plus reference
counting reclaims storage on drop:

- `Value` is one enum (`Nil Bool Int(i64) Double(f64) BigInt
  BigDec Ratio Str Char
  Symbol Keyword List Vector Map Set Regex TaggedLiteral
  ReaderConditional Var Unbound Throwable Transient Atom Delay
  Future Promise Agent Extension`). Heap payloads — strings,
  collections, metadata, extension values — sit behind `Arc`.
- Collections are thread-safe `rpds` persistent types with an
  `Arc` metadata slot plus insertion-order sidecars (small JVM
  maps/sets seq in insertion order, which is observable).
- Seqs are lazy and chunked (32, like the JVM); lazy thunks
  memoize through `OnceLock`, so they run once even under
  contention, and chunked side-effect order matches the JVM
  observably. Concurrency cells (atoms, futures, agents, …) are
  `Arc`-shared identity cells holding state, validators, and
  watches.
- The price of `Arc` without a collector: **self-referential
  cells leak** — `(reset! a a)` pins a cycle nothing collects
  (the JVM GCs it). Never store a ref in its own state.
- AOT binaries use the same representation across the FFI
  boundary. Generated LLVM code owns opaque `*mut Value` boxes
  and drives them through audited `rt_*` shims (`clj-rt`):
  arguments are consumed, results are owned (null on error),
  `rt_clone` / `rt_drop` manage sharing explicitly, and `main`
  drops every slot. Panics are caught inside the runtime and
  reported as `java.lang.Error` — unwinding never crosses FFI.
  Primitives run on a process-global booted evaluator behind a
  single non-nesting mutex (built binaries are single-threaded),
  which cannot deadlock by construction.

## Repository layout

```text
rust/                   Cargo workspace (binary + 7 crates, edition 2024)
  src/bin/clojure-rs/   CLI shell: eval/run/repl/build (clap)
  crates/clj-reader/    reader + printer, spans on every form
  crates/clj-value/     Value model, persistent collections, seqs, refs
  crates/clj-eval/      analyzer + tree-walking interpreter
  crates/clj-core/      namespaces/vars/macros + Rust-backed core subset
  crates/clj-repl/      rustyline REPL loop
  crates/clj-aot/       LLVM backend via inkwell (optional `aot` feature)
  crates/clj-rt/        AOT runtime archive (safe prim dispatch + rt_* shims)
  core-coverage.md      normative core var coverage (CI-enforced)
  tests/                integration suites + fixtures + vendored corpus inputs
docs/t1174/             design spec, per-area contracts, oracle pins,
                        differential corpora, TLA+ models
scripts/ci/             Go CI tooling + quality-gate container image
.github/workflows/     rust.yml (CI) + rust-release.yml (v* releases)
epl-v10.html            license text
```

## CI and releases

- `rust.yml` runs per PR and master push: 12 jobs on
  Linux/Windows (fmt+clippy, test, MSRV 1.85, LLVM-free unlinked
  tests, loom models, TLA+ model check, quality gates, Go
  CI-tools tests, release builds). Ordinary CI skips macOS for
  host capacity; `rust-release.yml` runs the full test suite plus
  builds on all three OSes for every `v*` tag (and on manual
  dispatch as the on-demand macOS check), attaching the three
  `clojure-rs-<target>.tar.gz` archives to the GitHub release.
- Quality gates: `cargo-deny` (licenses/bans/sources),
  `cargo-geiger` (unsafe audit), `cargo-udeps`, `cargo-audit`,
  complexity caps, and **100% line + patch coverage** floors
  (llvm-cov over nextest, JUnit test-results — both uploaded to
  Codecov).
- Concurrency is model-checked, not just tested: 18 loom
  schedule-exploration tests plus a TLA+ agent-queue model
  discharged by TLC in CI.
- Merging to master requires one approving review
  (`CODEOWNERS` auto-requests it) and verified signed
  commits (SSH: `gpg.format=ssh`, `commit.gpgsign=true`,
  key registered under GitHub Settings → SSH and GPG keys →
  Signing keys).

## Further reading

- [Design spec](docs/t1174/specification.md) — goals, non-goals,
  CLI/language contracts, slice plan (S1–S9, all landed).
- [Core coverage](rust/core-coverage.md) — every var's
  implemented/divergent/out-of-scope state.
- [Per-area contracts](docs/t1174/) — reader, evaluator, values,
  namespaces, vars, macros, concurrency (pinned divergences).
- [Workspace README](rust/README.md) — LLVM setup per platform,
  build/test commands, crate layout.
