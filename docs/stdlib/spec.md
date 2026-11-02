# Stdlib-all: embed the entire upstream `src/clj` stdlib

Program issue: #204. One integration PR. Pure Rust; upstream source is the
oracle (read-only); no JDK/JVM anywhere in product or tests.

## 1. Goal

Every one of the 49 upstream `clojure/clojure` `src/clj/**/*.clj` files must
be usable from clojure-rs under its canonical namespace, with its public API
intact. Strategies per file: **verbatim** (byte-identical vendor, EPL header
preserved), **port** (same API, JVM calls replaced by native equivalents),
**native** (Rust implementation behind the same namespace), **redesign**
(documented native equivalent where the JVM mechanism cannot exist).

Already done: `string`, `set` (verbatim); `java.shell`, `reflect`
(facsimiles); `java.io` (native `jio.rs`); `deftype`/`defrecord`/`defprotocol`
(prelude + natives). Out of scope: nothing except binary JVM compat.

## 2. Per-file verdicts

- W1 verbatim+pure: `template`, `walk`, `zip`, `data`, `datafy`.
  Scan each for JVM imports first; vendor verbatim if clean, else minimal
  port. Keep EPL headers.
- W2 test framework: `test`, `test/junit`, `test/tap`. Port. `test.clj` is
  the highest-value file: it turns upstream test suites into oracles.
- W3 pprint: `pprint` + `pprint/cl_format`, `column_writer`, `dispatch`,
  `pprint_base`, `pretty_writer`, `print_table`, `utilities`. Port as a set.
- W4 entry/IO: `repl`, `main`, `edn`, `core_print`, `stacktrace`. Port.
  `repl/deps` belongs to W7.
- W5 small libs: `math`, `uuid`, `instant`, `javadoc`, `browse`, `process`
  (`clojure.java/process`), `gvec`. Port; new natives only where no
  equivalent exists.
- W6 runtime-ish: `core/server`, `core/reducers`, `parallel`,
  `core_proxy`, `xml`. Port (`xml` over an existing in-tree XML path or a
  small new native; no new crates without asking).
- W7 tools track: `java/basis`, `java/basis/impl`, `tools/deps/interop`,
  `repl/deps`. Port against the already-embedded tools.deps namespaces.
- W8 core parity + redesigns: audit every public `def`/`defn`/`defmacro`
  in upstream `core.clj` against prelude + Rust natives; implement gaps
  (pure → prelude appendices, interop → natives). Redesigns: `genclass` →
  register emulated classes in the class lattice (no bytecode);
  `inspector` + `java/browse_ui` → same public API over text rendering
  (no Swing). `core_deftype`/`core/protocols` gaps (if any) also land here.

## 3. Layout and wiring contract

- Mirror upstream paths: `rust/crates/clj-eval/src/stdlib/<upstream path>`
  (e.g. `stdlib/clojure/walk.clj`, `stdlib/clojure/pprint/dispatch.clj`),
  exactly like the existing `stdlib/clojure/{string,set}.clj`.
- Slice agents may touch `ns.rs` in exactly two delimited blocks: one
  `include_str!` const block next to the existing `STDLIB_*` consts, one
  `EMBEDDED_SOURCES` entry block at the table end. Mark both
  `// W<N> (<stream>) — do not merge by hand, integrator resolves`.
  Everywhere else: NEW files only under assigned paths (stdlib sources,
  new `*.rs` modules + their `mod` line in a marked block, test files).
  `lib.rs` otherwise untouched; the integrator resolves block conflicts.
- Only W8 touches `src/core.clj` (prelude appendix sections, clearly
  delimited); all other workstreams keep hands off.
- Ports keep public names/arglists; every JVM divergence gets a brief
  code comment citing the upstream form. No README changes (integrator
  decides at the end).

## 4. Test contract (per namespace)

- A smoke test that `require`s the namespace and exercises its primary
  entry points against upstream-documented behavior (docstrings + source
  semantics are the oracle; never shell to a JVM).
- New Rust natives: colocated cover tests per repo convention — the 100%
  coverage floor applies to every new Rust line.
- Mirror the nearest existing test file layout; BDD scenarios only where
  user-visible behavior is new (W2/W4/W8).
- Full `cargo nextest -p clj-eval`, `cargo fmt --check`, `cargo clippy`
  with repo lints, and the complexity gate must pass on every slice
  branch before handoff.

## 5. Acceptance

All 49 namespaces load via `require`; per-namespace smoke tests green;
nextest + snip 1754/1754 green; coverage floor 100%; clippy/complexity
clean; single squash PR closing #204.
