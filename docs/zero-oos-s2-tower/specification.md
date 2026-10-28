# S2 tower math (zero-oos slice 2)

Nine upstream vars completing the BigInt/BigDec/Ratio tower plus
the BigDec precision context: `abs` `biginteger` `decimal?`
`denominator` `numerator` `ratio?` `rational?`
`*math-context*` `with-precision`.

Target universe rows come from the landed 692-row inventory
(`rust/core-coverage.md`, via PR #41); this slice flips its 9
rows to `implemented` in place. (On the wave-1 shared branch
the header counts the sibling slices' rows too; counts must
equal actual.)

## Semantics

- `abs` — `Numbers/abs` over all five numeric kinds
  (`Int`/`Double`/`BigInt`/`BigDec`/`Ratio`), `BigInteger`
  absorbing into `BigInt` (oracle `r9-absbi`); 1-arity.
  `Long/MIN_VALUE` is a fixed point, doubles take `abs`
  (`-0.0` → `+0.0`, `NaN`/`Inf` ride through); `BigDec`
  rounds to the bound context (oracle `wp-abs`); nil is the
  numbers NPE, anything else the `Number` CCE.
- `biginteger` — the upstream `cond` chain
  (`BigInt`→widen, `BigDec`→trunc, `Double`→`valueOf`→trunc,
  `Ratio`→trunc, `Int`→widen, `String`→`BigInteger(String)`,
  nil→`String.length` NPE, else `No matching ctor` IAE);
  1-arity. Results are the NEW `Value::BigInteger` kind
  (parent DECISION on #44, option (a)): value-identical to
  `Value::BigInt` (same `=`/`hash`/arithmetic — ops absorb
  it, oracles `r8-add`..`r8-typeop`), but printing bare
  (`5`, never `5N` — oracles `bi-pr`/`bi-tostr`).
- `decimal?` — `Value::BigDec` only; 1-arity (`pred1!`).
- `ratio?` — `Value::Ratio` only; 1-arity (`pred1!`).
- `rational?` — upstream `(or (integer? n) (ratio? n)
  (decimal? n))`, i.e. `Int`/`BigInt`/`BigInteger`/`Ratio`/
  `BigDec` (decimals INCLUDED — oracle `pred-rat-m`);
  1-arity (`pred1!`).
- `numerator` / `denominator` — `Ratio` accessors returning
  `Value::BigInteger` (reduced form, so the denominator is
  always positive); 1-arity. Non-`Ratio` args take the
  house checkcast CCE to `clojure.lang.Ratio` (nil NPEs name
  the field: oracles `nd-nil`/`r2-den-nil`).
- `*math-context*` — an `RT.java`-style dynamic root
  (`def_dynamic_root`, nil default, no `:dynamic` meta —
  oracle `mc-meta`). KNOWN DIVERGENCE (documented, pinned):
  upstream binds it (to nil) in the `clojure.main` thread
  frame (oracle `mc-thread2`), so `thread-bound?` is true
  there; the port boots a minimal frame (`*ns*` only —
  pinned by `vars.rs`), so here it is false.
- `with-precision` — core.clj `defmacro` over `binding`
  (upstream shape, incl the `:rounding MODE` form) plus a
  port-only `math-context` ctor native (installed but never
  referred — the `__defmethod*` shape, since a `:private`
  mark would reject the macro's own cross-ns expansion).
  The ctor mirrors `(MathContext. precision [rounding])`
  verbatim (negative → `Digits < 0`, wrong types → `No
  matching ctor`, nil → `null String` NPE, huge `Int` →
  `integer overflow`, strings → the strict
  `MathContext(String)` grammar, string+mode → the `Number`
  CCE of oracles `r7-strmode`/`r8-strmodedirect`). Bad
  `:rounding` modes fail the `RoundingMode.valueOf` text
  (upstream fails at compile with a path-laden
  `CompilerException` — mapped, documented); the port also
  accepts keyword/string modes by exact name (deliberate
  superset, documented).
- Context conveyance: `+ - * /` read the visible
  `*math-context*` (`deref_core`) and, when bound, run the
  `BigDec`-involved pairs as JDK-context ops (precision
  significant digits, `HALF_UP` default, all 8 modes;
  precision 0 = unlimited). `+`/`*` post-round;
  binary `-` is `add(x, negate(y, mc))` — the subtrahend
  rounds FIRST (oracles `r2-sub3`/`r5-selfsub`/`r6-subceil`,
  so `(- x x)` need not be zero); `/` divides with guard
  digits (exact-and-short keeps the stripped exact value);
  `quot` rounds the preferred-scale integral quotient
  (oracles `r2-quot`/`r7-quotmix`/`r8-quotscale`); `inc`/
  `dec`/negate/`abs` round their `BigDec` results;
  `bigdec`-of-`Ratio` consults via its upstream `/`
  definition. `rem`/`mod` convert ratios through the
  context (oracles `r7-remread`/`r7-remmix`) but never
  round their results (oracles `wp-rem`/`r3-rem`).
  `UNNECESSARY` failures carry `Rounding necessary`.
  Reads are lazy: non-`BigDec` ops never touch the var
  (oracle `r5-garbage-int`); garbage bindings fail the
  `MathContext` CCE where consulted (oracles `r5-garbage*`).
- The context value is a `MathContextVal` extension value
  (NOT a new `Value` kind): `#object[java.math.MathContext
  0x… "precision=N roundingMode=M"]` under `pr` (oracle
  `wp-mc`), the bare `toString` under `str`/`print`
  (oracle `wp-str`, via a defaulted `print_str` hook),
  structural `=`/`hash` like `MathContext.equals`
  (oracles `wp-eq2`/`wp-eq3`/`r2-eqrm*`).
- `BigInteger` membership (oracles `r8-*`/`r9-*`):
  `integer?`/`number?`/`rational?` true; `int?`/`ratio?`/
  `decimal?`/`float?` false. `int?` says false, so S1's
  `Int`-only `nat/neg/pos-int?` stand AS IS (no S1 amend);
  S1's `NaN?`/`infinite?` needed one `BigInteger` arm in
  `double_predicate` (else the wildcard CCEs a number).
  No `bigint?` var exists upstream (verified absent in
  alpha10 core.clj, PR #41, and master), so that
  constraint is vacuous — the `5`-vs-`5N` print pin is the
  class-distinction regression test instead.

All fns are 1-arity (`pred1!` or the house arity guard);
`numerator`/`denominator`/`abs`/`biginteger` are registry
natives, `with-precision` is core.clj. Arity misses throw
the house arity text (pinned).

## BDD scenarios

- Given each battery value (nil, bools, ints incl
  `Long/MIN_VALUE`, floats incl `-0.0`/`##NaN`/`##Inf`,
  ratios, bigints, bigintegers, bigdecs, chars, strings,
  keywords, symbols, lists, vectors, maps, sets, fns,
  delays, atoms, map entries, multifns, records), each
  predicate and coercion answers the oracle verbatim.
- Given `(with-precision 3 (/ 1M 3M))`, then `0.333M`;
  given `(with-precision 3 (+ 1.234M 5.678M))`, then
  `6.91M`; given precision 0, `(/ 1M 3M)` still throws
  non-terminating.
- Given each `:rounding` mode, results round per the JDK
  mode; given `UNNECESSARY` with an inexact result, then
  `Rounding necessary`.
- Given `rem`/`mod` under a context, then the exact
  (unrounded) remainder of context-converted operands.
- Given `(- 5.678M 5.678M)` under precision 3, then
  `-0.002M` (subtrahend rounds first).
- Given a throwing body, `*math-context*` still restores
  (binding semantics ride free).
- Given 0 or 2+ args, each native throws the arity text.

## Tests

`tests/zero_oos_s2_tower.rs`: one test per var replaying
its probe labels verbatim, plus a shared arity-miss test,
a context-restoration test, and the `BigInteger` print
regression test. Matrix `notes` name the tests.

## Non-goals

`int?` changes, auto-promote arith (`+'` family — S11),
`type`/`class` on tower values (S16), print-dup behavior
(S8). KNOWN PRE-EXISTING GAP (reported, not fixed — #35's
fence): no-context `(quot 5.678M 2.5M)` is `2M` in the
port but `2.00M` upstream (oracles `r8-quotscale0`/
`r8-quotz`); S2's context path uses the preferred scale.
