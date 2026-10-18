# S2 oracle probes

Clojure scripts that pin JVM `LispReader` behavior for the S2 reader
contract (`../reader-contract.md`). Each `(t "...")` / `(tm "...")` call
prints one `>>>` line with either the pr-str of every form read or the
root-cause exception (class | message | ex-data line/col).

- `probe.clj`: numbers, chars, strings, symbols, `##`, dispatch basics,
  conditionals, ns-maps, meta, fn, syntax-quote, delims, EOF, whitespace.
- `probe2.clj`: appended to `probe.clj` (`probe-all.clj`); ends in a
  deliberate top-level reader error (dup map literal), exit nonzero.
- `probe3.clj`: position state machine (`POS ...`), CR normalization,
  boundaries, backquote atoms, splice shapes, char codes, ns-map print,
  print-meta (dies partway on a deliberate `^:k "s"` error; earlier
  sections are the evidence).
- `probe4.clj`: dup-check basics, `#?@` positions, print-meta short
  forms, regex `\Q` char codes.
- `probe5.clj`: NaN map keys, radix overflow, big-set dup, print-meta
  `:tag` falsy / `:type`, regex qmode branch.
- `probe6.clj`: dup-check order dependence (`[1]`/`(1)` asymmetry).
- `probe7.clj`: `hash` values, nested-double dups, big-scale dups,
  `ReaderConditional` dup messages.
- `probe8.clj`: set nested-double dups, big-map new-reporting,
  `TaggedLiteral` dup messages.
- `probe9.clj`: big-map first-vs-new (programmatic), `TaggedLiteral`
  nested-double keys (OK), big-map 1/1.0 (OK both).
- `probe10.clj`: small/big threshold (16 forms), all-keyword path,
  big-map seq-cross and map-nested dups.
- `probe11.clj`: set map-nested dups (unifies set/big-map rule),
  big 1/1.0 coexistence.
- `probe12.clj` (S2 review): radix-10 digit errors omit `under radix`;
  greedy radix digits swallow `N` (`36r1N`⇒59, `2r1N` errors on `"1N"`).
- `probe13.clj` (S2 review): bare seq-cross dups BOTH orders in small
  maps (no asymmetry); length-mismatched pairs stay OK.
- `probe14.clj` (S2 review): same-length mixed-seq nested doubles DUP
  (patched recursion everywhere); mixed numerics stay OK.
- `probe15.clj` (S2 review): `TaggedLiteral`/`ReaderConditional`
  nested-double keys stay OK in sets (plain-form rule).
- `probe16.clj` (S2 review): splice leftovers dropped across top-level
  forms (`'#?@(:clj (1 2)) X` ⇒ `(quote 1)`, `X`).
- `probe17.clj` (S3): equality categories (`=` same-category only),
  reader ints always `Long`, `compare` matrix incl. NaN-is-0 and
  CCE shapes, numeric hashing, `sorted-map` compare-collapse.
- `probe18.clj` (S3): chunked-seq realization counts (32/2/32/64/32/16,
  vec10 10), mid-chunk remainder (31 from 1), nil-punning (C4).
- `probe19.clj` (S3): transient printing, `IllegalAccessError` rules,
  `assoc!`/`conj!`/`pop!` edges.
- `probe20.clj` (S4): analyzer error cause chains (if/quote/let/loop/fn/
  var/throw/set!/try/catch/recur), fn arity texts, nil/number/keyword/
  vector invocation, unbound-var uses, unknown ns/var, pre/post failure,
  `def` return shape.
- `probe21.clj` (S4): closure/shadow/loop/recur/letfn values, destructure
  matrix (incl. `:or`-nil stays nil, variadic0 rest is nil), pre/post
  pass, `def` value/type/meta, `set!` roundtrip, `(do)`/`quote`.
- `probe22.clj` (S4): try/catch/finally values, `ex-info`/`ex-data`,
  `throw`/`finally` interplay.
- `probe23.clj` (S4): try/def/fn/let/loop/letfn shape matrix,
  variadic legality rule, `pre` expansion, interop sanity.
- `probe24.clj` (S4): `set!` targets, `:or` evaluation, full `IFn`
  invoke table (keywords/symbols/vectors/maps/sets/vars/unbound),
  arity-name rules, CCE from-names, index coercion.

Run: `./probe.sh [outdir]`. Gensym ids (`p1__243#`), identity hashes
(`@7fc19ced`), and inst renderings vary between runs; the contract
records only the stable shapes.
