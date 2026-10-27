# S2 tower math (zero-oos slice 2)

Nine upstream vars completing the BigInt/BigDec/Ratio tower plus
the BigDec precision context: `abs` `biginteger` `decimal?`
`denominator` `numerator` `ratio?` `rational?`
`*math-context*` `with-precision`.

Target universe rows come from PR #41's 692-row inventory
(unlanded); this slice adds its 9 rows to the master matrix
at mirrored positions, flipped straight to `implemented`
(standalone: 432 rows, 414/2/16). The #41 land unions them
later. (On the wave-1 shared branch the header counts the
sibling slices' rows too; counts must equal actual.)

## Semantics

- `abs` — `Numbers/abs` over all five numeric kinds
  (`Int`/`Double`/`BigInt`/`BigDec`/`Ratio`); 1-arity.
  Oracle pins the doc edges (`Long/MIN_VALUE` fixed point,
  double `-0.0` → `+0.0`, `±Inf` → `Inf`, `NaN` → `NaN`),
  tower behavior (exact, never context-rounded), and the
  non-number errors (nil NPE vs `Number` CCE per kind).
- `biginteger` — the upstream `cond` chain
  (`BigInt`→self, `BigDec`→trunc, `Double`→`valueOf`→trunc,
  `Ratio`→trunc, `Int`→widen, `String`→`BigInteger(String)`,
  else the `No matching ctor` IAE); 1-arity. The port has
  one integer-tower kind, so results are `Value::BigInt`.
  Oracle settles the nil/bool BadStr shapes AND the print
  question (upstream `BigInteger` prints bare `5` while the
  port's `Value::BigInt` prints `5N`): if the oracle shows
  bare printing, `biginteger` needs a distinct
  representation — that is a no-new-Value-kinds fence
  break, escalated to the parent before implementing.
- `decimal?` — `Value::BigDec` only; 1-arity (`pred1!`).
- `ratio?` — `Value::Ratio` only; 1-arity (`pred1!`).
- `rational?` — upstream `(or (integer? n) (ratio? n)
  (decimal? n))`, i.e. `Int`/`BigInt`/`Ratio`/`BigDec`
  (decimals INCLUDED); 1-arity (`pred1!`). Oracle confirms
  the decimal arm.
- `numerator` / `denominator` — `Ratio` accessors returning
  `Value::BigInt` (reduced form, so the denominator is
  always positive); 1-arity. Non-`Ratio` args take the
  house checkcast CCE to `clojure.lang.Ratio` (nil NPE
  text pinned by the oracle).
- `*math-context*` — an `RT.java`-style dynamic root
  (`def_dynamic_root`, nil default): bindable, with the
  oracle pinning whether `:dynamic` meta is present.
- `with-precision` — core.clj `defmacro` over `binding`
  (upstream shape, incl the `:rounding MODE` form) plus a
  private port-only `math-context` ctor native (matrix
  precedent: `throw-illegal-arg!` et al are unlisted
  privates). The ctor mirrors `(MathContext. precision
  [rounding])` validation verbatim (negative → `Digits
  < 0`, wrong types → `No matching ctor`, bad mode → the
  oracle's `NoSuchFieldError`/IAE shape mapped to the port).
- Context conveyance: `+ - * /` read the visible
  `*math-context*` (`deref_core`) and, when bound, run the
  `BigDec`-involved pairs as `op(decimal, mc)` (precision
  significant digits, `HALF_UP` default, all 8 modes;
  precision 0 = unlimited). Oracle settles the exact op
  set (expected: `+ - * /` only — NOT `quot`/`rem`/`mod`,
  negate, `inc`/`dec`, `abs`, comparisons) and the
  mixed-kind pairs; `bigdec`-of-`Ratio` consults via its
  upstream `/` definition. `UNNECESSARY` failures carry
  `Rounding necessary`.
- The context value is a `MathContextVal` extension value
  (NOT a new `Value` kind): `#object[java.math.MathContext
  ...]` print (exact quoted part pinned by the oracle),
  structural `=`/`hash` iff the oracle shows
  `MathContext.equals` value semantics.

All fns are 1-arity (`pred1!` or the house arity guard);
`numerator`/`denominator`/`abs`/`biginteger` are registry
natives, `with-precision` is core.clj. Arity misses throw
the house arity text (pinned).

## BDD scenarios

- Given each battery value (nil, bools, ints incl
  `Long/MIN_VALUE`, floats incl `-0.0`/`##NaN`/`##Inf`,
  ratios, bigints, bigdecs, chars, strings, keywords,
  symbols, lists, vectors, maps, sets, fns, delays,
  atoms, map entries, multifns, records), each predicate
  and coercion answers the oracle verbatim.
- Given `(with-precision 3 (/ 1M 3M))`, then `0.333M`;
  given `(with-precision 3 (+ 1.234M 5.678M))`, then the
  oracle's 3-significant-digit sum; given precision 0,
  `(/ 1M 3M)` still throws non-terminating.
- Given each `:rounding` mode, results round per the JDK
  mode; given `UNNECESSARY` with an inexact result, then
  `Rounding necessary`.
- Given `quot`/`rem`/`(- x)`/`inc` under a context, then
  the exact (unrounded) value.
- Given a throwing body, `*math-context*` still restores
  (binding semantics ride free).
- Given 0 or 2+ args, each native throws the arity text.

## Tests

`tests/zero_oos_s2_tower.rs`: one test per var replaying
its probe labels verbatim, plus a shared arity-miss test
and a context-restoration test. Matrix `notes` name the
tests.

## Non-goals

`int?` changes, auto-promote arith (`+'` family — S11),
`type`/`class` on tower values (S16), print-dup behavior
(S8), `BigInteger`-vs-`BigInt` print duality beyond the
oracle verdict (escalated, not silently diverged).
