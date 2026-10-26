# Issue #32: S-rng-tower slice — Specification

- Status: ready (research complete: tower/regex/RNG surveys + 60
  oracle probes across `/tmp/probe.clj`, `/tmp/probe2.clj`,
  `/tmp/probe32b`–`/tmp/probe32i.clj`).
- Parent: out-of-scope reduction. Issue: #32. Branch: `32-rng-tower`.
  Probes: `docs/i32-rng-tower/probes/{re_rng,tower}.{clj,out}`
  (to be written; exit 0 required).

## Problem

Twelve `clojure.core` vars are `out-of-scope` with no slice and no
code in the tree: RNG (`rand`, `rand-int`, `shuffle`,
`random-sample`), regex matching (`re-find`, `re-matches`,
`re-seq`, `re-pattern`; the reader already reads `#\"...\"`
into `Value::Regex` raw source), and the big-number tower
(`bigint`, `bigdec`, `num`, `rationalize`, plus ratio values).
Any program touching stochastic choice, pattern matching, or
exact arithmetic fails at resolve time (`Unable to resolve
symbol`); ratio-producing paths cannot represent exact results.

## Solution

Three stages, each test-first against committed oracle probes:

1. **Regex** (`re.rs` + `RE_NATIVES`): `re-pattern` compiles
   (`regex` crate) or passes `Pattern` through; `re-find` /
   `re-matches` / `re-seq` match over `Value::Regex` source
   with JVM result shapes (string vs group vector, nil holes,
   `re-seq` nil-on-no-match). `PatternSyntaxException` error
   class added; look-around stays a documented divergence (the
   `regex` crate rejects it at compile time).
2. **RNG** (`rng.rs` + `RNG_NATIVES`, `rand` crate): `rand` /
   `rand-int` mirror the `Math/random` formulas exactly
   (including `(rand-int -1)` → `0` truncation and the nil NPE /
   string CCE texts); `shuffle` Fisher-Yates over
   Vector/List/Set with the `Collection` CCE and nil NPE texts;
   `random-sample` as a `core.clj` multi-arity `defn` over
   `filter` + `rand` (zero Rust, lazy semantics ride free).
3. **Tower** (clj-value + clj-reader + core.rs): new
   `Value::BigInt(num_bigint::BigInt)`,
   `Value::BigDec(BigDec)` (`unscaled: BigInt, scale: i64`),
   `Value::Ratio(num_rational::BigRational)` with print
   (`5N`/`5.0M`/`1/2` under pr, plain under str),
   scale-insensitive `=` (compareTo), exact cross-type `==` /
   `compare`, structural consistent hashing; reader literals
   `1/2`, `5N`, `5M` enabled (flipping the N3 deferrals);
   JVM promotion lattice (Double > BigDec > Ratio > BigInt >
   Int) for `+ - * / inc dec quot rem mod`, with the
   Int/Double-only wedges byte-identical (checked-overflow
   errors); inexact `Int`/`Int` `/` is a reduced ratio (`(/ 1 2)`
   → `1/2`, oracle `promo-mul` — not a Double); `int`/`nth` take
   the `RT.intCast` shape (truncate, `long` check with exact
   digits, then `toIntExact`; decimals narrow through `double`
   first — oracles `nth-*`, `conv-hugeint`, `conv-hugedecint`);
   `long` of decimals narrows through `double` too (oracle
   `conv-declong`); counts (`take`/`drop`/`repeat`) follow the
   `double` path (`ceil` for take/drop, truncation for repeat —
   oracles `index-*`, `seq-tower`); ranges step exactly with
   `Numbers.add` and compare exactly (oracles `range-*`);
   `number?`/`integer?` extended (`decimal?` does not exist —
   review round 1 correction); `short`/`byte`/`char` grow tower
   arms (review round 2);
   `bigint`/`bigdec`/`num`/`rationalize` coercion tables
   verbatim from the oracles (nil NPEs, `Long`-to-`IFn`-style
   CCEs, `No matching ctor` IAE, `Infinite or NaN` NFE,
   non-terminating `ArithmeticException`).

## Part 1 — User/functional specification

### 1.1 Goals

1. All twelve rows become `implemented`, with probe-backed notes.
2. `(rand-int 10)` stays in `[0,10)`; `(shuffle coll)` preserves
   elements; `(random-sample 1.0 coll)` / `(0.0 coll)` are
   deterministic (`(1 2)` / `()`).
3. `(re-seq #\"\d+\" \"a1b22\")` → `(\"1\" \"22\")`;
   `(re-find #\"a(x?)\" \"ax\")` → `[\"ax\" \"x\"]`;
   `(re-find #\"a(x)?\" \"a\")` → `[\"a\" nil]`;
   `re-matches` anchors (`#\"a*\"` vs `\"b\"` → nil).
4. `(rationalize 0.5)` → `1/2`; `(rationalize 5.1M)` →
   `51/10`; `(rationalize 5.0M)` → `5N`; `(= 5N 5)` → true;
   `(= 5.0M 5.00M)` → true; `(sort [5M 1/2 5N 1 0.5 2])` →
   `(1/2 0.5 1 2 5M 5N)`.

### 1.2 Non-goals (explicit)

- No exact-draw RNG pins (draws differ across runtimes by
  design); determinism pins use seeded shapes/ranges plus the
  `0.0`/`1.0` `random-sample` edges.
- No look-around/possessive regex features (engine divergence,
  documented in the row note).
- No tower bit-ops, no `short`/`byte`/`char` tower
  conversions, no overflow-to-BigInt promotion (the
  checked-overflow wedge is protected and stays).
- No `re-matcher`/`re-groups` rows (need a Matcher host
  object; interop-blocked — the coverage header's
  "unlisted = file bug" applies, noted as follow-up).
- No JVM-magic hash values for the new types (structural,
  `=`-consistent hashes; documented).

### 1.3 Acceptance criteria

- [ ] Twelve rows flipped; probe-backed notes.
- [ ] All probes pinned by tests, verbatim outputs (RNG:
  shape/range only).
- [ ] `coverage_matrix` green; 100% line+patch; full gates green.

### 1.4 BDD scenarios (user-observable)

- Given `(rand-int 10)` sampled repeatedly, then every draw is
  an integer with `0 <= d < 10`; given `(rand-int -1)`, then
  `0` (truncation, probe `randint-neg1`).
- Given `(shuffle [3 1 2])`, then `(sort Tap)` is `(1 2 3)`;
  given `(shuffle nil)`, then the `Collection.toArray` NPE;
  given `(shuffle "ab")`, then the `String`-to-`Collection` CCE.
- Given `(re-seq #\"\d+\" \"a1b22\")`, then `(\"1\" \"22\")`;
  given `(re-seq #\"x\" \"abc\")`, then nil; given a String
  pattern, then the `Pattern` CCE.
- Given `(rationalize 0.5)`, then `1/2`; given
  `(bigdec 1/3)`, then the non-terminating
  `ArithmeticException`; given `(bigint nil)`, then the
  `String.length` NPE.
- Given `(str 5N)`, then `\"5\"` (no suffix); given
  `(pr-str 5N)`, then `\"5N\"`.

## Part 2 — Technical specification

- Source of truth: `core.clj` at the alpha10 tag (`rand` =
  `Math/random`, `rand-int` = `(int (* n (rand)))`,
  `shuffle` = `Collections/shuffle` over `ArrayList`,
  `random-sample` = `filter` over `rand`; `re-*` shapes;
  `bigint`/`bigdec`/`num`/`rationalize` coercions) plus
  `java.util.regex` result shapes, confirmed by the probes.
- Promotion lattice (oracle-pinned, rank Double > BigDec >
  Ratio > BigInt > Int): `(+ 1/2 5)` → `11/2`,
  `(+ 1/2 5N)` → `11/2`, `(+ 1/2 5M)` → `5.5M`,
  `(+ 5N 5M)` → `10M`, `(+ 5N 5)` → `10N`,
  `(+ 5M 0.5)` → `5.5` (Double), `(+ 1/2 0.5)` → `1.0`,
  `(/ 5N 2)` → `5/2`, `(/ 1 2)` → `1/2` (inexact `Int`
  division is a reduced ratio, oracle `promo-mul`),
  `(quot 7N 2)` → `3N`, `(quot 7 2N)` → `3N`,
  `(quot 5M 2)` → `2M`, `(quot 1/2 1)` → `0N`.
- Port shape: `re.rs`/`rng.rs` natives + tables chained in
  `ns.rs` beside `COLL_NATIVES`; `random-sample` appended to
  `core.clj`; tower `Value` variants in
  `clj-value/src/value.rs` with `print.rs` / `compare.rs` /
  `hasheq.rs` arms; reader `NumLit` deferrals flipped to
  values; `Num` enum + arithmetic/predicate/conversion
  extensions in `core.rs`; `ThrowClass::PatternSyntaxException`
  in `clj-value/src/extension.rs`.
- TDD plan: oracle probe files first (alpha10 jars; RNG
  shape/range only), stage tests red (unresolvable vars),
  then regex → RNG → tower implementation, then
  `random-sample` composition, then row flips +
  `coverage_matrix`.
- CI impact: new Rust code under `rust/**` plus `regex`,
  `rand`, `num-bigint`, `num-rational` deps (all MSRV-clean
  for 1.85, all in the registry cache); full gate suite
  applies (fmt, clippy `-D warnings`, 100% line+patch floors,
  complexity ratchet ≤15, loom/TLA only if shared dispatch
  state appears). No workflow changes needed.

### Constraints

EPL-1.0 headers on new Rust files; MSRV 1.85 (no let-chains);
signed commits (m-hu) with `Ref #32`; green-before-push;
macOS work runs from `rust/`; oracle command + output committed
for every new behavior test.
