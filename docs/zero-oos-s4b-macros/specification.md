# S4b macro machinery (zero-oos slice 4b)

Nine upstream vars exposing the macro-expansion helper surface
and JVM name munging: `definline` `selector` `destructure`
`seq-to-map-for-destructuring` `unquote` `unquote-splicing`
`munge` `namespace-munge` `with-local-vars`.

Target universe rows come from the landed 692-row inventory
(`rust/core-coverage.md`, via PR #41); this slice flips its 9
rows to `implemented` in place (header counts = actual).

## Semantics

- `definline` — verbatim core.clj port of the upstream macro
  (docstring, `split-with` decl parse, `eval`-built specimen
  body). Design decision (the issue's open hook): `:inline`
  is RECORDED, never honored — the port is an interpreter and
  inlining is a compiler concern. Calls evaluate the specimen
  body exactly like the upstream non-inlined path; `(:inline
  (meta #'f))` answers the inline fn. Specimen-evaluation
  errors surface the port's own native texts (S5D-owned, e.g.
  `inc` on a symbol); the throw phase (expansion, not call)
  matches.
- `selector` — verbatim core.clj port (`defn-`
  `selector-impl` + `defmacro selector`); the two
  `IllegalArgumentException` sites throw through the house
  `throw-illegal-arg!` (no `new` exists). Runtime rides the
  existing `:select`/`:all`/`:excess`/`:missing` outlet
  machinery (verified working on this tree); expansion
  qualifies `some-vals` (already resolves, PR #41 row).
- `destructure` — 1-arity native over
  `destructure_expand.rs`: partition the seqable into pairs
  (trailing odd form dropped, like `partition`), return the
  input unchanged when every pair starts with a symbol, else
  reduce pairs through the shared `bind_one` starting from
  `[]`. Non-seqables throw the `ISeq` IAE; bad binding forms
  throw `Unsupported binding form: X` (shared helper, same as
  `let` expansion). `MapEntry` patterns take the vector path
  (`MapEntry` is a vector on the JVM — `dt-entry-*`); the
  shared `bind_one` gains that arm (unreachable from `let`,
  which has no entry literals). The internal `let`-path
  `destructure()` is untouched.
- `seq-to-map-for-destructuring` — 1-arity native: even 2+
  elements ⟹ assoc pairs in order (later dupes win,
  insertion order); exactly 1 element ⟹ `first` as-is (even
  non-maps); empty/nil ⟹ `{}`. Odd 3+ elements follow the
  oracle's `createAsIfByAssocComplexPath`: assoc the even
  prefix pairs, then `conj` the tail (map merges, 2-vector /
  entry assocs, nil is identity; anything else throws
  `conj`'s error — `Vector arg to map conj must be a pair`,
  the `ISeq` IAE, or the `Map$Entry` CCE — all pinned, no
  `to-array` needed, S15 stays out).
  Non-seqables throw the `ISeq` IAE (`next`'s). Fully
  realizes, like `to-array` (infinite seqs hang on both).
- `unquote` / `unquote-splicing` — bare `(def ...)` unbounds
  in core.clj (no `:declared`, matching upstream lines
  13–14). They resolve so reader-emitted
  `clojure.core/unquote` symbols stop failing at resolve
  time; the reader/analyzer keeps owning syntax-quote
  expansion untouched.
- `munge` — 1-arity native: `(str s)` through the true
  `Compiler.munge` char table (oracle-pinned), returning a
  symbol when given a symbol (namespace destroyed by
  escaping, like the JVM) else a string. This table DIFFERS
  from the `ext.rs` display `munge()` (`/` → `_SLASH_`, not
  `$`, plus `\` and the `"` arms the display path never
  needed): a new `compiler_munge()` carries it; display
  printing is untouched.
- `namespace-munge` — 1-arity native: `(str ns)` with `-` →
  `_`, nothing else (upstream `core_deftype.clj:13`).
- `with-local-vars` — core.clj macro, upstream shape minus
  interop: `let` over 0-arity `__local-var*` calls (fresh
  anonymous dynamic vars, see below), qualified
  `push-thread-bindings` of the `(hash-map ~@name-vals-vec)`
  form, `try`/`finally` `pop-thread-bindings` (the `finally`
  clause is built as a concrete list — the `try` clause
  matcher rejects syntax-quote seqs — printing exactly like
  the upstream shape). Arg checks go through the house
  private `assert-args` (same call shape as upstream).
  Expansion necessarily differs from the JVM's
  (`Var/create`/`pushThreadBindings` interop has no port
  spelling); evaluation order and scoping match.
- `__local-var*` (unlisted dunder helper, `__multifn*`
  precedent): allocates an anonymous dynamic var — no-ns
  registry state (`root` none, `dynamic` true, meta `{:ns
  nil, :name nil}` per the oracle), printing `#<Var:
  --unnamed-->` in both `pr` and `str`.

All four fns are 1-arity with the house arity guard; arity
misses throw the house `ArityException` text (same shape as
the JVM's). The three macros keep upstream arglists
(`definline` `[name & decl]`, `selector` `[m]`,
`with-local-vars` `[name-vals-vec & body]`).

## BDD scenarios

- Given each battery input (vectors, lists, maps, strings,
  nil, keywords, odd vectors), `destructure` answers the
  oracle's expansion-or-throw verbatim (modulo gensym
  suffixes, normalized in tests).
- Given 0/1/2/3+ element seqables, `seq-to-map-for-
  destructuring` answers map/first/`{}`/throw per the oracle.
- Given every ASCII punctuation char plus nil/numbers/
  keywords/symbols, `munge`/`namespace-munge` answer the
  oracle's strings verbatim, with symbol-in/symbol-out for
  `munge`.
- Given `(definline f [x] \`(inc ~x))`, calls evaluate the
  specimen (`(f 5)` → `6`), `(:inline (meta #'f))` is the
  inline fn, and malformed decls/bodies fail at expansion.
- Given 1- and N-directive maps, `selector` expands to the
  single-gensym/multi-`some-vals` shapes (normalized) and
  the selecting fn answers the oracle's outlet values;
  non-maps and directive-less maps throw the oracle texts.
- Given `(with-local-vars [a 1 b 2] ...)`, `var-get`/
  `var-set`/`thread-bound?` replay inside the body, bindings
  pop on return and on throw, sequential uses are
  independent, and no var leaks into any namespace;
  non-vector/odd bindings throw the `assert-args` texts.
- Given `` `~x `` outside syntax-quote, evaluation fails as
  on the JVM; `(resolve 'unquote)` is truthy and `(bound?
  #'unquote)` false.

## Tests

`tests/zero_oos_s4b_macros.rs`: one test per var replaying
its probe labels (gensym/line/hex normalized where the
house already normalizes), plus a shared native arity-miss
test and an unbound-`resolve` test for the `~` pair. Matrix
`notes` name the tests.

## Non-goals

`:inline` honoring (no compiler); outlet-semantics fixes
(outlets verified working — any gap found later belongs to
that slice, analyzer stays untouched); unbound-`var-get`
throwing (the S5B sentinel is inherited, recorded in
probes); `to-array`/arrays (S15); `eval` extensions (used
as-is); print dispatch (`print-method` on vars stays the
`pr`/`str` core paths, S8).
