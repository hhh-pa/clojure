# S5 slice plan: namespaces + core (T1174)

Branch `T1174-s5-core`. Charter: C5 (namespaces/vars/macros/loader),
`clj-core` (Rust-backed subset + embedded `.clj`), `rust/core-coverage.md`
(A5), differential corpus entries (A2). TDD: the coverage-matrix test
fails until each var lands.

## S5A. Namespace system + loader

- Multi-namespace registry (replace the single S4 `Namespace`): `ns`,
  `in-ns`, `create-ns`, `remove-ns`, `find-ns`, `all-ns`, `the-ns`,
  `ns-name`, aliases, refers, `alias`, `refer`, `use`, `require`,
  `refer-clojure`, `intern`, `declare`, `ns-interns`, `ns-publics`,
  `ns-refers`, `ns-resolve`, `resolve`, `*ns*`, `*file*`.
- Loader: embedded `clojure.core` (only embedded namespace in S5) plus
  file `load-string`/`load-file` and `require` over `*load-path*`
  (port extension: a vector of directories; embedded namespaces win).
  `*loaded-libs*`/`loaded-libs` for idempotent require.

## S5B. Var completeness (C5)

- `alter-var-root` (atomic root swap), `with-redefs` (scoped root
  rebinding), `bound?`, `thread-bound?`, `var-get`,
  `push-thread-bindings`/`pop-thread-bindings`/`clone-thread-bindings`/
  `get-thread-bindings`, `with-bindings*`/`with-bindings`, `bound-fn`
  (dynamic conveyance primitive; S6's `future`/`send` build on it).
- Meta: `meta`, `with-meta`, `vary-meta`, `alter-meta!`, `reset-meta!`.
- Out: var validators (no slice), STM (N4).

## S5C. Macros

- `defmacro` (mechanism TBD: special form vs macro), `macroexpand-1`,
  `macroexpand`, macro-fn invocation during analysis with `&form`/`&env`,
  `gensym`. No `macroexpand-all` (clojure.walk, not core).

## S5D. Core vars (`rust/core-coverage.md` is normative)

- Rust-backed: arithmetic, comparison, predicates, collections
  (incl. `nth`/`update-in`/`get-in`/`zipmap`), chunked-lazy seqs
  (`map`/`filter`/… over S3 `Seq`; eager `mapv`/`filterv`/…),
  higher-order fns, `str`/`keyword`/`symbol`/`name`/`namespace`,
  `format` (Formatter subset, oracle-pinned), `ex-info` family,
  print fns + `*out*`/`*err*`, `slurp`/`spit`, `read-string`,
  `with-out-str`, `eval`, transients (`conj!`…), `declare`,
  `var-set`. `delay`/`force`/`deref` stay in S6 per the S1 seed.
- Embedded `.clj`: `defn`/`defn-`, `and`/`or`, `when*`/`if-*`,
  `cond`/`condp`/`case`, threaders (`->`/`->>`/`as->`/`cond->*`),
  `dotimes`/`doseq`/`for`, `lazy-seq`/`lazy-cat`, `comment`, `assert`,
  `time`, `doto` (fn calls only; methods are N1 interop).
- Out of scope (coverage records each with its reason): transducers
  and xform arities, sorted collections, `format`, `rand-*`/`shuffle`,
  `instance?`/`type`, multimethods, `with-in-str`/`read`, `deftype`/
  `defrecord`/`reify`/`proxy`/`gen-class` (no type-system slice),
  `with-open`, protocols (`extend`/`satisfies?`), `*in*`,
  S6 (concurrency) and S7 (`*1`/`*2`/`*3`/`*e`, CLI) vars.
- A2 differential corpus: `rust/tests/` entries per 2.6 layout;
  in-subset entries byte-identical vs the JVM oracle, out-of-subset
  entries assert their divergence.

## TDD order

1. Coverage-matrix test (red: every `planned` var unresolved).
2. S5A registry + loader (namespace vars resolve).
3. S5B var fns, S5C macro machinery, S5D vars in dependency order
   (predicates → collections → seqs → HOF → .clj composites).
4. Differential corpus entries alongside each var.

## Acceptance

- Matrix test green; `core-coverage.md` has zero `planned` rows.
- Per-slice gates (fmt/clippy 3-OS/quality-gates/coverage) via PR CI.
- Spec updates in-slice (`Ref T1174` + `AI-Tool:` trailers per commit).
