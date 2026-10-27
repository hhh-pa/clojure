# S11 extended arith (zero-oos slice 11)

37 upstream vars completing the tower's arithmetic surface:
4 auto-promoting (`+'` `-'` `*'` `dec'`, plus the `inc'`
in-slice promotion fix), 21 `unchecked-*`, 12 bitwise.
Oracle: alpha10 jars, probes in `probes/`
(`autopromote*.out`, `unchecked*.out`, `bitwise*.out`,
`s11gaps.out`).

Target universe rows come from PR #41's inventory (unlanded);
this slice adds its 37 rows to the wave matrix at mirrored
positions, flipped straight to `implemented`. (On the wave-2
shared branch the header counts every slice's rows; counts
must equal actual.)

## Semantics

### Auto-promote: `+'` `-'` `*'` `dec'` (+ `inc'` fix)

- `+'` — 0-arity `0`; 1-arity `(cast Number x)` (nil passes
  through as nil, numbers pass through identical, anything
  else `CCE | Cannot cast {from} to java.lang.Number` —
  oracles `ap-ar1s`, `ap-ar1nil`, `a2-addchar`, `a2-addtrue`,
  `g-mulnil`); 2+ arity folds left with PROMOTING `Int` arms
  (overflow becomes exact `BigInt`, never throws) and the
  checked `add` lattice/ranks/`*math-context*` consultation
  everywhere else (oracles `ap-max1`, `ap-fold`,
  `ap-mixed-*`, `mc-addp`).
- `-'` — 0-arity is the arity error; 1-arity promoting
  negate (`(-' MIN)` → `9223372036854775808N`, oracles
  `sp-negmin`, `sp-ar1*`; `BigDec` rounds to the bound context like
  checked negate — oracles `n-negmc`, `n-negmc10`); 2+ arity folds left with
  promoting `Int` arms, checked `sub` lattice elsewhere
  (`sp-min1`, `sp-fold`, `mc-subp`).
- `*'` — 0-arity `1`; 1-arity the same `cast` as `+'`
  (`mp-ar1s`, `g-mulnil` — note `a2-mulnil`'s NPE is a
  PROBE-HARNESS artifact: `classof` calls `.getName` on the
  nil result; the fn answers nil); 2+ arity folds left
  with promoting `Int` arms, checked `mul` lattice
  elsewhere (`mp-max2`, `mp-min-1`, `mc-mulp`).
- `dec'` — 1-arity promoting step (`(dec' MIN)` →
  `-9223372036854775809N`; tower/double kinds keep the
  `dec` rules incl. context rounding; nil is the numbers
  NPE, non-numbers the `Number` CCE — oracles `dp-*`).
- `inc'` IN-SLICE FIX — the port's `native_inc_prime`
  currently overflows on `MAX` (it calls the checked
  step); the oracle promotes (`ip-max` →
  `9223372036854775808N`). Retarget it at the promoting
  step; every other `inc'` arm already matches (`ip-*`).

### Unchecked long ops (tower-checking, wrapping `Int` arms)

`unchecked-add`/`-subtract`/`-multiply` (2-arity),
`unchecked-inc`/`-dec`/`-negate` (1-arity): the checked
`add`/`sub`/`mul`/step/negate lattice — `Double` >
`BigDec` > `Ratio` > `BigInt` folds, `*math-context*`
consulted (`xm-addu`, `xm-mulu`, `xm-incu`) — EXCEPT the
`Int` arms wrap (`wrapping_*`, oracles `ua-wrap`,
`us-wrap`, `um-wrap`, `um-minneg`, `ui-wrap`, `ud-wrap`,
`un-min`; negate rounds `BigDec` to the bound context —
oracles `n-unegmc`, `n-unegmc10`). Tower/double results are value-identical to
the checked ops (`ua-big`, `ua-rat`, `ua-dec`, `um-rat`
→ `1N`); nil is the numbers NPE, non-numbers the
`Number` CCE. `BigInteger` absorbs like everywhere
(`xc-addbi` → `3N`).

### Unchecked `-int` ops (checked intcast, wrapping `i32` ops)

`unchecked-add-int`/`-subtract-int`/`-multiply-int`/
`-divide-int`/`-remainder-int` (2-arity),
`unchecked-inc-int`/`-dec-int`/`-negate-int` (1-arity):
each arg coerces through `intcast` (CHECKED — oracles
`uai-long`, `xi-*`, `g-int*`), then the op runs in
wrapping `i32` and answers a port `Int` (no `Integer`
kind exists):

- `Int` → `toIntExact` (`AE | integer overflow` past
  `i32`, oracle `uai-long`).
- `Double` → the `long` range check first (`NaN` is 0,
  past-`i64` is `IAE | Value out of range for long: {d}`
  with Java double rendering — oracles `xi-nan`,
  `xi-inf`, `xi-hugedbl`, `xi-hugedec`), then `toIntExact`.
- `BigInt`/`BigInteger` → the `long` stop with EXACT
  digits, then `toIntExact` (oracles `xi-bigwrap`,
  `g-intbighuge`, `g-intbighugneg`).
- `Ratio` → truncate toward zero, then the `long` stop
  with exact digits, then `toIntExact` (oracles `xi-rat`,
  `xi-hugerat`, `g-intrathuge`).
- `BigDec` → through `double` (oracle `xi-hugedec`
  renders `1.0E20`; `g-intdechuge` renders `1.0E19`).
- nil → the `charValue` NPE quirk (oracles `uai-nil`,
  `udv-nil`, `urm-nil`); chars/strings/anything else →
  the `Number` CCE (oracles `uai-str`, `xi-char`,
  `xd-char` — note chars do NOT convert here, unlike
  `int`).

`unchecked-divide-int`/`-remainder-int` by zero throw
`AE | / by zero` (oracles `udv-zero`, `urm-zero`);
`MIN/-1` wraps to `MIN` (oracle `udv-minneg`, Java `/`).

### Unchecked coercions (truncating, never range-fail)

Results are port `Int`s (widened from the target width)
except `unchecked-double`/`-float` (`Double`) and
`unchecked-char` (`Char`); no `Integer`/`Short`/`Byte`/
`Float` kinds exist.

- `unchecked-int` — `Int` low 32 bits; `Double` the Java
  `(int)` cast (truncate, `NaN`→0, saturate — oracles
  `uci-dbl`, `uci-nan`, `uci-inf`, `uci-hugedbl`);
  `BigInt`/`BigInteger` `intValue` (low 32 — oracles
  `uci-big`, `xc-bi`); `Ratio` `(int)doubleValue`
  (SATURATING — oracle `xr-intwrap` → `MAX`, `xr-intneg`
  → `MIN`); `BigDec` `intValue` (truncate + low 32 —
  oracle `uci-hugedec` → `1661992960`); `Char` → its
  code; nil → the `charValue` NPE; anything else → the
  `Character` CCE (oracles `uci-char`, `uci-nil`,
  `uci-str`, `xc-intbool`).
- `unchecked-long` — `Int` identity; `Double` the Java
  `(long)` cast (oracles `ucl-dbl`, `ucl-nan`, `ucl-inf`,
  `ucl-hugedbl`); `BigInt`/`BigInteger` low 64 bits
  (oracles `ucl-big`, `ucl-negbig`); `Ratio` low 64 of
  the exact trunc (oracles `xr-longwrap`, `xr-longneg`
  → `0`); `BigDec` `longValue` (truncate + low 64 —
  oracle `ucl-hugedec`); `Char` → the `Number` CCE
  (oracle `ucl-char`!); nil → the `longValue` NPE;
  anything else → the `Number` CCE.
- `unchecked-float` — PORT MAPPING (no `Float` kind):
  answers the EXACT `Double` (mirrors `float` minus the
  range check, so common values print identically —
  oracle `ucf-dbl` prints `0.1` either way); out-of-`f32`
  values stay exact instead of becoming `Inf` (oracle
  `ucf-huge` → `##Inf` upstream, `1.0E300` here —
  documented, pinned). `Char` → the `Number` CCE
  (oracle `xc-fchar`); nil → the `floatValue` NPE;
  anything else → the `Number` CCE.
- `unchecked-double` — extensionally `double` (numbers
  narrow, `Char` → the `Number` CCE, nil → the
  `doubleValue` NPE — oracles `ucd-*`, `xc-dchar`).
- `unchecked-short` — `Int` low 16 bits; `Double` the
  Java `(short)(int)` composition (`NaN`→0, `Inf`→-1,
  `-Inf`→0 — oracles `ucs-dbl`, `ucs-nan`, `xc-sinf`,
  `xc-sneginf`); `BigInt`/`BigInteger` low 16 (oracle
  `ucs-big`); `Ratio` `(short)(int)doubleValue`
  (oracles `g-shortrat` → `-1`, `g-shortratneg` → `0`);
  `BigDec` `shortValue` (low 16 of `intValue` — oracle
  `ucs-dec`); `Char` → the `Number` CCE (oracle
  `ucs-char`); nil → the `shortValue` NPE; anything
  else → the `Number` CCE.
- `unchecked-byte` — the `unchecked-short` shape at
  8 bits (`(byte)(int)` doubles, low 8 integers,
  `byteValue` NPE/CCE — oracles `ucb-*`, `g-byterat`,
  `xc-binf`).
- `unchecked-char` — `Char` passes through; numbers
  take `(char)longValue` (low 16 of the `unchecked-long`
  composition — oracles `ucc-dbl`, `ucc-nan`, `ucc-big`,
  `ucc-rat`, `ucc-dec`, `xc-charhuge` → `\uffff`,
  `xc-charneg`, `xr-char` → NUL); nil → the `longValue`
  NPE; anything else → the `Number` CCE. Lone
  surrogates (e.g. `(unchecked-char 55296)`) are
  unrepresentable in Rust `char`: they fail the house
  `char` range error (the checked-`char` surrogate
  divergence, shared).

### Bitwise (Long-only lattice)

All 12 fns take integral operands ONLY: on the JVM
`Long`/`Integer`/`Short`/`Byte` (oracles `jt-*`),
everything else — `BigInt`, `Double`, `Ratio`,
`BigDecimal`, `Character`, strings, collections — fails
`IAE | bit operation not supported for: class {X}`
(oracles `ba-big`, `ba-dbl`, `ba-rat`, `ba-dec`,
`jt-andchar`, `ao-kw`, `ao-vec`, `ao-bool`, `ba-str`).
The port has no `Integer`/`Short`/`Byte` kinds, so the
port rule is: `Int` works, nil is the `getClass` NPE,
anything else is that IAE with `class {jvm_class_of}`
(the house names already match upstream verbatim for
every probed class). Operands validate left to right;
the FIRST offender is named (oracles `g-order`,
`g-order2`, `va-andbad3`).

- `bit-and`/`bit-or`/`bit-xor`/`bit-and-not` — variadic
  `[x y & more]` left folds (`&`, `|`, `^`, `x & ~y`);
  0/1 args are the arity error (oracles `ba-ar1`,
  `ba-ar3`, `va-and0`, `va-and4`, `va-or0`, `va-or3`,
  `va-xor0`, `va-xor3`, `g-andnot0`, `g-andnot1`,
  `g-andnot4`, `ban-plain`).
- `bit-not` — 1-arity complement (`bn-*`).
- `bit-shift-left`/`bit-shift-right`/
  `unsigned-bit-shift-right` — 2-arity Java shifts (the
  count masks to 6 bits, negatives included — oracles
  `bsl-mask`, `bsl-negsh`, `bsr-mask`, `bsr-negsh`,
  `ubs-mask`, `ubs-negsh`, `ev-hugesh`, `ubs-plain`,
  `ev-ubs`).
- `bit-set`/`bit-clear`/`bit-flip` — 2-arity
  (`x | (1<<n)`, `x & ~(1<<n)`, `x ^ (1<<n)`, masked —
  oracles `bst-neg`, `bcl-neg`, `bfl-neg`).
- `bit-test` — 2-arity boolean (`(x & (1<<n)) != 0`,
  masked — oracle `btt-neg` → `false`).

## Port mappings (pinned, deliberate)

- No `Integer`/`Short`/`Byte`/`Float`/`BigInt`-bitwise:
  see above; each divergence is pinned by an `extarith`
  test row naming the upstream value.
- Arity failures are always the runtime `ArityException`
  (`Wrong number of args (N) passed to:
  clojure.core/{name}`); upstream raises `CompilerException`
  ("Syntax error compiling …") for the `:inline`d fns
  (`inc'`/`dec'`/every `unchecked-*`/shifts/`bit-not` —
  oracles `ip-ar0`, `ua-ar1`, `bn-ar2`, `fx-shl3` …).
  The port has no inliner; the runtime shape matches the
  non-inlined siblings (`sp-ar0`, `ba-ar1`, `fx-set3`).
- CCE texts use the house `cce_message` (no JVM module
  suffix); the `Cannot cast X to Number` shape reuses
  `NativeError::cast` verbatim.
- `unchecked-char` surrogates share the checked-`char`
  divergence (house range error).

## Tests

`rust/crates/clj-eval/tests/extarith.rs`, following the
`tower.rs` shape (`run`/`run_last`/`uncaught` + `pr`/
`realized` helpers): one `#[test]` per fn family, every
oracle label replayed (value + class + throw shape),
plus the port-mapping pins above.
