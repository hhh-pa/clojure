# T1199 specification — M2 destructuring catch-up slice

Upstream scope: clojure/clojure commits 9e51d6dc (`:req!` directive,
`req!` getter) and 4610bb46 (namespaced-destructuring element
precedence), both landed on master after 1.13.0-alpha9. M1 inventory
(`docs/t1198/inventory.md`) triaged them as the single PORT slice;
all other master deltas are SKIP.

Oracle: clojure 1.13.0-alpha10 jars (features postdate alpha9, so the
alpha9 jars reject them); probes in `docs/t1199/probes/m2-deltas.clj`
(value behavior) and `m2-errors.clj` (error shapes), with captured
output in `m2-deltas.out` / `m2-errors.out`. Upstream tests:
`7d3c9332` (req! directive tests in
`test/clojure/test_clojure/data_structures.clj`) and the `4610bb46`
test patch, fetched via `gh api`. Upstream binding spec
(`::req-bindings`, `::req!`) read from core.specs.alpha
0.6.148-alpha12.

## Part 1 — behavior

1. `:req!` takes a map of local to key: `(let [{:req! {a :long-a}}
   {:long-a 1}] a)` binds `a` to `1`. Missing keys throw
   `IllegalArgumentException` with message `Missing required key: <key>`.
   Works in `let`, `loop`, and `fn` params.
2. `:req!` composes with `:keys`/`:keys!`/`:or`/`:missing`/nested maps:
   `(let [{:keys! [b], :keys [d], :req! {a :long-a}, :or {d 42}}
   {:long-a 1, :b 2}] [a b d])` gives `[1 2 42]`.
3. `:req!` entries reject `:or` defaults: supplying a default for a
   required local or key throws `Exception` (`Can't supply default
   value for required key: <key>`; two defaults for one key throw
   `Multiple :or defaults for same key`). Required + `:missing`
   records absent keys (`{:long-a nil}`) instead of throwing.
4. Nested maps work as `:req!` locals: `(let [{:req! {{:keys! [c]}
   :nested}} {:nested {:c 3}}] c)` gives `3`. Locals are
   local-names or nested map forms only (upstream `::req-bindings`).
5. Post-`&` keys in a namespaced `:keys`/`:keys!` directive resolve
   against the directive namespace: `(let [{:foo/keys! [aa & :bb]
   :select sel} {:foo/aa 1, :bb 2, :foo/bb 3}] sel)` selects `:foo/bb`,
   not `:bb`. Qualified post-`&` keys keep their own namespace.
   Without a directive namespace nothing changes.
6. Pre-`&` translation keeps element-namespace precedence, and
   `(namespace %)` now always runs: strings under a qualified
   directive take the `Named` cast (was `Unsupported binding form`),
   `nil` always takes the `getNamespace` NPE (was `getName` under a
   qualified directive).

Out of scope: spec-level rejection of namespaced elements under a
namespaced directive (upstream `clojure.spec` rejects
`{:foo/keys [::a]}` at macroexpansion; this port has no binding-spec
validation layer and does not add one), `:foo/req!` (namespaced
`req!`, unpinned upstream).

Known divergences (documented, not fixed here):

- Upstream `let` spec-asserts its bindings before expanding
  (failures read `macroexpanding clojure.core/let`); this port
  expands first. So malformed `let` shapes surface the expansion
  error here (`Unexpected error macroexpanding let` + cast) vs the
  spec error upstream (`Syntax error macroexpanding
  clojure.core/let`). Same stage, same outcome (compile error);
  pre-existing architectural class (e.g. `(let [{:keys [{:a 1}]}
  ...])` already pins this). `loop` matches upstream exactly —
  both expand first (`loop-req-bad-full`, `loop-req-num` pins).
- `(fn [{:req! {b :b}, :or {b 42}} ...])`: the walk reports `Can't
  supply default value` where upstream's inner-`let` spec reports
  non-conformance. Same wrapper, more precise cause.
- `:select` projection order is source order here, JVM hash order
  upstream (pre-existing §E12 divergence).

## Part 2 — implementation

This port has two destructure engines (upstream has one shared
`destructure`): the macro layer (`destructure_expand.rs`, serving
`let`/`loop`, running before validation) and the analyzer layer
(`expr.rs` validation + `eval.rs` binding, serving `fn`/`let*`).
Both are updated.

- `destructure_expand.rs`: `walk_entry` routes an exact `:req!`
  keyword to a new `walk_req` (map entries bind required with raw
  keys via `push1(..., true)`; non-maps mirror the JVM's
  expansion-stage `ISeq` / `Map$Entry`-cast errors). `post_amp`
  qualifies post-`&` keys through a new `post_key` (upstream `xfk`).
  `translate` flips to element-namespace precedence (upstream `xf`);
  `named_parts` always reports the `getNamespace` NPE.
- `expr.rs`: `spec_valid_directive` accepts `:req!` via a new
  `spec_valid_req` (map of local-name/nested-map to any key, per
  upstream `::req-bindings`). `walk_map_pair` validates `:req!`
  through new `walk_req_list`/`walk_req_entry` (required `:or`
  semantics, same expansion-stage errors for `fn`/direct-analysis
  parity). `key_for` flips to element precedence; new `post_amp_key`
  qualifies analyzer post-`&` keys; `key_cast_err` drops the
  qualified-string short-circuit and the `getName` variant.
  `MapPattern.entries` gains a required flag; `build_map_pair`
  builds `:req!` entries required.
- `eval.rs`: `bind_entries` passes the entry required flag into
  `map_binding` (existing `req!`/`:missing` machinery).

Tests: oracle-backed rows in `eval_semantics.rs::destructure_values`
(`let`/`loop`/`fn` value pins), expansion errors in
`fx4_macroexpand.rs::expand_errors`, validation matrix in
`analyzer_contract.rs::m2_req_walk_rows` plus updated
`keys_element_cast_matrix` rows for the new cast/NPE behavior.

CI impact: none (no workflow changes). Quality gates unchanged: fmt,
clippy `-D warnings`, full workspace tests, stable-toolchain llvm-cov
line + patch gates.
