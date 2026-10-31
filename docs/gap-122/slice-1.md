# Gap-122 slice 1: `clojure.spec.alpha` minimal core

Part of #122 (clojure.spec port). This is the first thin slice: a
working `clojure.spec.alpha` namespace with the spec registry,
`def`, predicate specs, `s/and`, `valid?`, `conform`/`unform`, and
basic `explain`/`explain-str`.

## Source

Ported from upstream `spec.alpha-0.5.238`
(`~/.m2/repository/org/clojure/spec.alpha/0.5.238/`, file
`clojure/spec/alpha.clj`, 1996 lines). Upstream line references below
are against that file. The port lives in
`rust/crates/clj-eval/src/spec_alpha.clj`, served as an embedded
library by `resolve_source` (`rust/crates/clj-eval/src/ns.rs`), so
`(require '[clojure.spec.alpha :as s])` works with no files on disk.

## Scope (fixed)

In scope:

- Spec registry (`registry`, `get-spec`, `def-impl`).
- `s/def` (keyword and symbol keys, nil removes).
- Predicate specs (`s/spec`, `spec-impl`; fn, set, and ident preds).
- `s/and` (`and-spec-impl`, incl. lazy ident resolution).
- `valid?` (2- and 3-arity), `conform`, `unform`, `spec?`,
  `invalid?`, `regex?`.
- `explain-data`/`explain-data*`, `explain`, `explain-str`,
  `explain-out`, `*explain-out*`, `explain-printer`.
- `form`, `describe`, `abbrev` (explain-chain support).

Out of scope (later slices): generators (`gen`, `with-gen`, `gen*`
is declared on the protocol but unextended), `s/or`, `s/keys`,
`s/merge`, `s/every`/`coll-of`/`map-of`, `s/tuple`,
`s/multi-spec`, regex ops (`cat`/`alt`/`*`/`+`/`?`/`&`),
`s/fspec`/`s/fdef`, instrumentation, `s/conformer`,
`s/nilable`, `s/nonconforming`, `clojure.spec.gen.alpha`,
`clojure.spec.test.alpha`, `core.specs`.

## Behaviors pinned

Oracle: `docs/gap-122/probes/slice1.out` (from
`docs/gap-122/probes/slice1.clj` under
`clojure-1.12.6`/`spec.alpha-0.5.238`). Every behavior below is
pinned by `rust/crates/clj-eval/tests/spec_alpha.rs`.

- `valid?`/`conform`/`unform` over `s/def` predicate specs, bare
  predicates, set preds, and `#()` preds.
- `s/and` conform threading, `unform` reversal, 3-pred chains, and
  forward registry references (built before the target exists).
- `explain-str` byte format incl. the `spec: <name>` suffix, the
  `(< % 10)` second-pred line, and `Success!` on valid input.
- `explain-data` problem-map shape (`:path`/`:pred`/`:val`/`:via`/`:in`).
- `s/def` returns the key; `s/def` with nil removes the entry.
- `form`/`describe` output incl. fn-literal `unfn` restoration
  (`(fn [%] ...)` abbreviated to the body).
- Errors: unresolvable spec, non-namespaced `def` key (assert),
  non-fn pred under `explain`, regex-shaped input (slice guard).

## Port adaptations (all pinned by tests)

1. No syntax-quote and no `::`-keywords in `spec_alpha.clj`: the
   port reads a whole load batch before evaluating it, so both would
   qualify against the requiring namespace. Macros build expansions
   with `list` (fully-qualified heads); spec keywords are written
   out (`:clojure.spec.alpha/invalid`, ...).
2. Spec objects are `defrecord`s (`PredSpec`/`AndSpec`) with `Spec`
   extended to them, not `reify`: the port's reify instances carry
   no metadata, while `with-name`/`spec-name` store the registered
   name in metadata (as on the JVM).
3. `specize` is a `cond`, not the private Specize protocol: the port
   dispatches protocols on exact classes only (no `Object` default;
   a sorted set would miss a set entry). Same dispatch, same
   behavior.
4. `fn-sym` folds to nil (the port exposes no fn-name primitive):
   bare-fn specs carry the `:clojure.spec.alpha/unknown` form, so
   `(s/explain-str even? 5)` prints
   `5 - failed: :clojure.spec.alpha/unknown` where upstream prints
   `5 - failed: even?`.
5. `Exception.` construction is `ex-info` with the same message
   (still caught by `catch Exception`); the class reads
   `clojure.lang.ExceptionInfo` instead of `java.lang.Exception`.
6. `->sym` rebuilds the qualified symbol from the var's
   `:ns`/`:name` meta (the port's `symbol` takes no var).
7. `ns-qualify` reads names via `ns-name` (no `.name` interop; `str`
   of a namespace renders the object rep).
8. `spec`/`spec-impl`/`and-spec-impl`/`dt` drop the
   generator/conformer params (`gfn`, `cpred?`); `and-spec-impl`
   uses one conform loop (the per-count fast paths are
   behavior-identical).
9. `assert` in `def-impl` is bare, not `c/assert`: the port only
   treats the unqualified and `clojure.core`-qualified spellings as
   the special form.

## Known pre-existing port limits (not slice-1 work)

- Batch file reading resolves `::kw` against the requiring
  namespace for every form in the file, including forms after the
  `ns` form (upstream reads incrementally). Tests use explicit
  keywords or sequential evals.
