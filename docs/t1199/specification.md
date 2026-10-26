# T1199 specification — M2 destructuring catch-up slice

Upstream scope: clojure/clojure commits 9e51d6dc (`:req!` directive,
`req!` getter) and 4610bb46 (namespaced-destructuring element
precedence), both landed on master after 1.13.0-alpha9. M1 inventory
(`docs/t1198/inventory.md`) triaged them as the single PORT slice;
all other master deltas are SKIP.

Oracle: clojure 1.13.0-alpha10 jars (features postdate alpha9, so the
alpha9 jars reject them); probes in `docs/t1199/probes/m2-deltas.clj`
with captured output in `m2-deltas.out`. Upstream tests: `7d3c9332`
(req! directive tests in `test/clojure/test_clojure/data_structures.clj`)
and the `4610bb46` test patch, fetched via `gh api`.

## Part 1 — behavior

1. `:req!` takes a map of local to key: `(let [{:req! {a :long-a}}
   {:long-a 1}] a)` binds `a` to `1`. Missing keys throw
   `IllegalArgumentException` with message `Missing required key: <key>`.
2. `:req!` composes with `:keys`/`:keys!`/`:or`/`:missing`/nested maps:
   `(let [{:keys! [b], :keys [d], :req! {a :long-a}, :or {d 42}}
   {:long-a 1, :b 2}] [a b d])` gives `[1 2 42]`.
3. `:req!` entries reject `:or` defaults: supplying a default for a
   required local or key throws `Exception` (`Can't supply default
   value for required key: <key>`; two defaults for one key throw
   `Multiple :or defaults for same key`). Required + `:missing`
   records absent keys (`{:long-a nil}`) instead of throwing.
4. Nested maps work as `:req!` locals: `(let [{:req! {{:keys! [c]}
   :nested}} {:nested {:c 3}}] c)` gives `3`.
5. Post-`&` keys in a namespaced `:keys`/`:keys!` directive resolve
   against the directive namespace: `(let [{:foo/keys! [aa & :bb]
   :select sel} {:foo/aa 1, :bb 2, :foo/bb 3}] sel)` selects `:foo/bb`,
   not `:bb`. Without a directive namespace nothing changes
   (`{:keys! [aa & :bb]}` still selects `:bb`).
6. Pre-`&` translation keeps element-namespace precedence: element ns
   wins over directive ns (matches upstream `xf`), keyword elements
   keep their own ns under a namespaced directive.

Out of scope: spec-level rejection of namespaced elements under a
namespaced directive (upstream `clojure.spec` rejects
`{:foo/keys [::a]}` at macroexpansion; this port has no binding-spec
validation layer anywhere and does not add one here), `:foo/req!`
(namespaced `req!`, unpinned upstream), non-map `:req!` values
(upstream throws a cast error; this port reports a clean error).

## Part 2 — implementation

File: `rust/crates/clj-eval/src/destructure_expand.rs` only.

- `walk_entry` routes an exact `:req!` keyword (no namespace) to a new
  `walk_req`, mirroring the upstream splice of `:req!` entries back
  into the binding loop with required semantics: per `(local, key)`
  entry, outlet injection (`inject_all`), `:select` accounting
  (`sel_add`), token projection (`btok_put` for symbols), then
  `push1(..., req = true)`. Keys are used raw (no `translate`), as
  upstream. Non-map `:req!` values are a clean `Exception`.
- `post_amp` qualifies post-`&` keyword keys through a new `post_key`
  helper (upstream `xfk`): under `Keys` + directive ns, an unqualified
  keyword takes the directive ns, a qualified one keeps its own; all
  other shapes pass through. The qualified key feeds both `:select`
  accounting and the required-`push1` getter.
- `translate` flips to element-namespace precedence (upstream `xf`):
  `elem_ns.or(kind.ns)`.

Tests: oracle-backed rows in `eval_semantics.rs::destructure_values`
(value pins + `uncaught` error pins from `m2-deltas.out`), including a
non-map `:req!` error pin for the new error branch.

CI impact: none (no workflow changes). Quality gates unchanged: fmt,
clippy `-D warnings`, full workspace tests, stable-toolchain llvm-cov
line + patch gates.
