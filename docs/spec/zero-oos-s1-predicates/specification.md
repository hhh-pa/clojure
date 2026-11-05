# S1 kind predicates (zero-oos slice 1)

Twelve upstream vars over existing machinery, no new value
kinds: `NaN?` `infinite?` `nat-int?` `neg-int?` `pos-int?`
`any?` `delay?` `ffirst` `find-keyword` `indexed?`
`map-entry?` `special-symbol?`.

Target universe rows come from the landed 692-row inventory
(`rust/core-coverage.md`, via PR #41); this slice flips its 12
rows to `implemented` in place (header counts = actual).

## Semantics

- `any?` — `(constantly true)`; 1-arity.
- `delay?` — `Value::Delay` only.
- `ffirst` — core.clj `(defn ffirst [x] (first (first x)))`.
- `map-entry?` — `Value::MapEntry` only (never 2-vectors).
- `indexed?` — oracle settles membership (vectors for sure;
  strings? maps?).
- `find-keyword` — 1-arity (kw-or-string) and 2-arity
  (ns-str-or-nil, name-str); oracle settles whether lookup
  interns (JVM `Keyword.find` does not) and the
  nil-ns / non-string shapes.
- `special-symbol?` — oracle pins the exact set
  (`def if do let quote var fn* loop* recur throw try`
  `monitor-enter monitor-exit . new set!` plus `&`? `fn`?).
- `NaN?` — oracle pins the shape (expected: float kind and
  NaN payload; BigDec NaN? Ratio?).
- `infinite?` — expected: float kind and ±Inf payload.
- `nat-int?` / `neg-int?` / `pos-int?` — defined over the
  port's `int?` (see below); oracle pins BigInt inclusion
  and the zero boundary (`neg-int? 0` false, `pos-int? 0`
  false expected).
- `int?` boundary (existing `native_intp`, NOT redefined
  here): oracle probes record what `int?` answers for
  BigInt/BigDec/Ratio/float so the three `*-int?` fns
  compose correctly; if the port's `int?` disagrees with
  the oracle, that is a follow-up slice, not this one.

All natives are 1-arity (`pred1!` + registry row); `ffirst`
is core.clj; `find-keyword` is a 1-or-2-arity native with
the house arity guard. Arity misses throw the house
arity text (pinned).

## BDD scenarios

- Given each battery value (nil, bools, ints, floats incl
  ##NaN/##Inf, ratios, bigints, bigdecs, chars, strings,
  keywords, symbols, lists, vectors, maps, sets, fns,
  delays, atoms, map entries, multifns, records), each
  predicate answers the oracle's boolean verbatim.
- Given 0 or 2+ args, each native throws the arity text.
- Given `(find-keyword "nope")` twice, the oracle's
  intern-or-nil behavior replays (no observable table
  growth either way).
- Given every `special-symbol?` candidate, membership
  matches the oracle set exactly.

## Tests

`tests/predicates.rs`: one test per var replaying its
probe labels verbatim, plus a shared arity-miss test.
Matrix `notes` name the tests.

## Non-goals

`int?` redefinition, BigDec tower gaps, `bytes?` (needs
the Array kind — S15), `find-var`/`requiring-resolve`
(S5), print-dup behavior (S8).
