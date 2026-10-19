# S5D oracle pins (7g9–7g14c + 7i–7i6 probes, decompiles, Formatter)

Working record for the S5D differential corpus and implementation.
Oracle: `java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main`
(the bare oracle drifts: machine locale is en_FR + Europe/Paris; 7h pins are en_US).
Probe files: `/tmp/s5d-gaps7g9.clj` … `/tmp/s5d-gaps7g14c.clj`, `/tmp/FmtProbe.java`,
`/tmp/FmtProbe2.java`, `/tmp/cq.clj`, `/tmp/loc.clj`, `/tmp/s5d-gaps7i.clj` …
`/tmp/s5d-gaps7i5.clj`, `/tmp/s5d-gaps7i6.clj` (7i6 = crash-cell recovery).

## Map classes (decompiled + probed)

- `canBePAM(init)`: array-len ≤ 16 → array-map; ≤ 128 with keys past the
  first 8 pairs ALL keywords → array-map; else hash-map. Used by `RT.map`
  and `RT.mapUniqueKeys` (literals). `error.rs::map_class_of` matches this.
- `(hash-map …)`: `PersistentHashMap/create` — ALWAYS hash-map (even 1 pair);
  `(hash-map)` → `{}` (array-map EMPTY).
- `(array-map …)`: `createAsIfByAssoc` — ALWAYS array-map (any size/content).
- Persistent `assoc` (new key): keyword key → promote iff size ≥ 64 pairs;
  non-keyword → promote iff size ≥ 8 pairs. Replace never promotes.
  `assoc-int-last` (39 kw then int) → HashMap. Promotion is one-way.
- Transient `assoc!`: capacity-gated. `(transient {})` starts cap 16 slots.
  Append while `len < cap` (no check). When full: promote to hash iff
  `len ≥ 128` OR key is non-keyword; else grow to
  `newCapacity = min(ceil(n*1.65),128)` made even, and retry.
  Consequences: `kw31-then-int9` → ArrayMap, `int9-then-kw31` → HashMap,
  `kw64` → A, `kw65` → H, `into-int9` → H, `into-kw9` → A.
- `dissoc` (persistent): order-preserving, class sticky (shrunk HashMap stays
  HashMap). `dissoc!` (transient): SWAP-LAST order
  (`(dissoc {:a 1 :b 2 :c 3} :a)` → `([:b 2] [:c 3])`,
  `dissoc!`+persistent → `([:c 3] [:b 2])`).
- `(empty m)`: kind-preserving (`empty` of big map → HashMap). `(empty "ab")`
  → nil, `(empty nil)` → nil, `(empty (transient …))` → nil.
- `(into {} [])` → ArrayMap; `(merge)` → nil; `(merge {} {})` → ArrayMap.
- Map seqs: array-map seq → counted+IDrop; hash-map seq → neither.
- `select-keys` from big → fresh small → ArrayMap.
- Transient class follows promotion: `(transient bigmap)` → TransientHashMap;
  array→hash promotion mid-build changes later error texts.

## `nth` (probed; RT.nth takes `(Object,int)` — index coerced FIRST)

- Index coercion (before coll dispatch): Number → long-range check,
  then int-range check, else truncate. Out-of-long-range (`1e100`,
  `±Inf`, huge BigInt) → IAE `Value out of range for long: X` (BigInt
  prints WITHOUT `N`); in-long/out-of-int (`5e9`, `-5e9`, MAX, MIN) →
  AE `integer overflow`; else truncate (`1.9`→1, `3/2`→1, `NaN`→0).
  nil → NPE `Cannot invoke "java.lang.Character.charValue()" because
  "x" is null`; anything else → CCE `X cannot be cast to
  java.lang.Number`. `(nth 5 "a")` → CCE, `(nth nil 1e100)` → IAE
  (coercion beats bad/nil coll). Direct + reflective paths identical.
- `(nth nil 0)` → nil; `(nth nil -1)` → nil; `(nth nil x :nf)` → `:nf`.
- Non-`Indexed` seqables walk in O(n): `(nth (map inc [1 2]) 1)` → 3.
  Negative → `IndexOutOfBoundsException` with EMPTY message; OOB with
  `:nf` → `:nf`.
- Strings: `(nth "ab" -1)` → `StringIndexOutOfBoundsException`
  `Index -1 out of bounds for length 2`.
- Unsupported (number/map/set/keyword…): `nth not supported on this type: X`
  with `getSimpleName` rule = strip package only (`Long`,
  `PersistentArrayMap`, `LazySeq`, `core$map` for core fns). `:nf` does NOT
  save unsupported types.
- `(nth (transient [1 2]) 1)` → 2 (transients support nth/get).

## Ranges (decompiled `LongRange`/`Range.create` + probed)

- `(range)` → `(iterate inc' 0)` (class `Iterate`, not a range).
- Dispatch: all-`int?` args → `LongRange/create`, else `Range/create`.
- `LongRange.create(s,e,st)`: `st>0`: `e≤s`→EMPTY else try
  `LongRange(s,e,st,toIntExact(rangeCount))` catch ArithmeticException →
  `Range.create` (LONG objects, double path); `st<0`: mirror; `st==0`:
  `e==s`→EMPTY else `Repeat.create(start)`.
- `rangeCount` = `((end-start)+step+(step>0?-1:+1))/step`, checked ops.
- `Range.create(s,e,st)`: pos-step + `start>end`→EMPTY; neg-step +
  `end>start`→EMPTY; `equiv(start,end)`→EMPTY; zero-step→`Repeat(start)`;
  else `Range` with `positiveStep`/`negativeStep` bounds check (`!(x≥end)`).
- `(range 0 ##NaN)` → infinite `Range` `(0 1 2 …)`; `(range 5 5 0)` → `()`;
  `(range 5 4 0)` → `(repeat 5)`; empty ranges → `EmptyList` (never an
  empty range object).
- `LongRange`: Counted + IDrop + chunked. Double `Range`: neither. `seq` of
  either returns self; `next` shifts (unchecked `start+step`); `drop`
  shifts; `take-last` shifts. `(count (range 10))` → 10 via Counted.
- 1-arg double path: `Range.create(end)` = `isPos(end) ? Range(0,end,1) :
  EMPTY`. `(range nil)`/`(range "a")` → isPos errors (corpus-pinned).

## Repeat / cycle / iterate / cons (probed)

- `(repeat n x)`: strict (`repeat_count` eager, intValue truncation);
  `n≤0` → `EmptyList`. `(repeat 2.5 :x)` → 2 items.
- `(repeatedly n f)` = `(take n (repeatedly f))` — LAZY `n` via take
  (ceil rule! `(repeatedly 2.5 f)` → 3 calls).
- `Repeat`: seq, IDrop, NOT counted, NOT chunked (one per step).
  `seq`/`next`/`drop`/`take-last` preserve `Repeat`.
- `(cycle xs)` → class `Cycle` (lazy path for drop); `(iterate f x)` →
  class `Iterate` with LAZY `more`: `rest`/`next` force the CURRENT
  val only (fresh: 0 calls; `rest`-of-`rest`: 1); `first`-of-`rest`
  → 1 call; `(nth it 2)`/`first`-of-`(drop 2 it)` → 2 (7i counters;
  the old "strict more, rest calls f once" pin was WRONG).
  `(cons h t)` → class `Cons`. All three `seq` to self.
- `conj`: range/lazy → `Cons`; list → `PersistentList`; nil → `List`.

## take-last / butlast / drop / take-nth (probed)

- `take-last`: `(take-last 0/-1/empty …)` → nil. Result kind-preserving:
  vector→ChunkedSeq, list→PersistentList, LongRange→LongRange,
  double-Range→Range, Repeat→Repeat, string→StringSeq(counted!),
  small-map→ArrayMap-Seq(counted!), set/big-map→KeySeq, lazy→ChunkedCons.
  `n` validated eagerly through `(seq (drop n s))`.
- `butlast`: always ChunkedSeq (counted) or nil (input < 2 items). Eager.
- `drop`: IDrop inputs (vector, LongRange, Repeat, StringSeq, small-map
  SEQ, small MAP itself, ChunkedSeq, take-last/butlast results) validate
  + drop EAGERLY; everything else lazy. Past-end eager → `EmptyList`.
  `(drop 0 v)` → seq-kind; `(drop 1 "ab")` → LazySeq (String not IDrop);
  `(drop 0 '())` → LazySeq. DoubleRange NOT IDrop (lazy).
- `take-nth`: EVERY force = `seq` + `first` + `(drop n s)` (the drop runs
  eagerly each time, so IDrop inputs throw on first force with bad `n`).
  `(take-nth 0 coll)` replays head forever. `(interleave)` → `()`;
  `(concat)` → LazySeq.

## sort (probed)

- Result: `ArraySeq`, counted TRUE, IDrop FALSE, no metadata; empty/nil →
  `EmptyList`. Sort is stable. `sort-by` calls `(comp (keyfn x)
  (keyfn y))` DIRECTLY (no int mapping). Comparator protocol
  (`AFunction.compare`): `true`→less; `false`→consult reversed call;
  numbers→sign of intValue/trunc; else CCE to Number. Keyfn called per
  comparison.

## `conj`/`assoc`/`dissoc` error pins

- `(conj (transient [1]) 2)` → CCE TransientVector→IPersistentCollection.
- `(assoc (transient {:a 1}) :b 2)` → CCE →Associative.
- `(dissoc (transient {:a 1}) :a)` → CCE →IPersistentMap (full module text).
- `(assoc! tvec :a 1)` → IAE `Key must be integer`. Persistent assoc on
  vector with non-int key → same (corpus-verified). `assoc` on nil → map.
- `pop` on empty → ISE `Can't pop empty vector` (also `pop!`).

## Format (FmtProbe/FmtProbe2/Clojure pins + `s5d-fmt*` batches; locale en_US, zone UTC)

- Parse: flags `[-#+ 0,(<]*` (duplicates fail DURING parse, every flag
  including `<` → `DuplicateFormatFlagsException` `Flags = 'X'`);
  width digits (overflow → `IllegalFormatWidthException`
  `-2147483648`, constant); precision = `.` + 1+ digits (overflow →
  `IllegalFormatPrecisionException` `-2147483648`; a `.` not followed
  by a digit is NOT precision); conversion must be `[a-zA-Z%]`.
- Parse failure (end of string or a non-conversion char in the
  conversion slot) → `UnknownFormatConversionException` with the char
  AFTER `%` (`%5.3`→`5`, `%`→`%`, `%.2`→`.`, `%.2+s`→`.`,
  `%-1$s`→`-`, `%_s`→`_`), even mid-string. `%0$` → argument-index
  `Illegal format argument index = 0`; unparseable/`>i32::MAX` index
  → `Format argument index: (not representable as int)`.
- All flag messages sort flags in canonical order (`-`, `#`, `+`,
  space, `0`, `,`, `(`): pair, mismatch, flags-error, and the
  width-required reconstruction alike.
- Check orders (all before argument lookup; each probed): numerics
  (`d`/`o`/`x`/`e`/`f`/`g`/`a`) = width-required → pair → mismatch →
  precision; `%s` = dash-width → mismatch → precision; `%b`/`%h` =
  dash-and-zero-width → mismatch → precision; `%c`/`%t` = mismatch
  → width → precision; `%n` = precision → width-forbidden → pair →
  flags; `%%` = precision → pair → flags → width-required.
- The `+`/space pair (all flags, canonical) fires only for numerics
  and `%n`/`%%` (`%+ x`→`'+ '`, `%+ #d`→`'#+ '`, `% +n`→`'+ '`);
  `s`/`b`/`h`/`c`/`t` report both inside the mismatch (`%+ s`→`+ `).
- Mismatch (`Conversion = {c}, Flags = {bad}`, `%t` names the
  SUFFIX): `s`/`a` drop a non-sole `#`; `t`/`c` list all canonical;
  `b`/`h` truncate after the first `#`; `x`/`o` collapse to a
  present `,` else list all. Allowed sets: `s`/`b`/`h`/`c`/`t` =
  `-` only; `d` = `- + space 0 , (`; `o`/`x` = `- # 0`; `e`/`f` =
  all eight; `g` = all but `#`; `a` = `- + space # 0` (`-` needs
  width everywhere).
- Width-required (`-`/`0` without width) → `MissingFormatWidthException`
  with `%` + canonical flags + index + precision + conversion
  (`%-0s`, `%-1$s`, `%-0.2d`, `%-tY`, `%#0d`). `%n` forbids width
  (`IllegalFormatWidthException` with the number) and precision;
  `%%` allows width (pads) and requires it for `-`. `%n`/`%%`
  take no argument and disturb neither index.
- Precision illegal for `d`/`o`/`x`/`c`/`t`/`n`/`%` (message = the
  number); legal for `s`/`b`/`h` (truncate) and `e`/`f`/`g`/`a`.
  `%b/%B/%h/%H` ALLOW precision (= max chars, truncate).
- `%c`: accepts ONLY Character (Clojure Long → `IllegalFormatConversion
  c != java.lang.Long`!). The old range-check/codepoint pin is
  UNREACHABLE in the port (no `Integer` type; Rust `char` is always
  valid); lone-surrogate output stays a known divergence.
- `%h` = Java `.hashCode` hex (see hasheq.rs `hash_code`; seqs realized
  first; `(%h [1 2])`→`3e2`). `%n` = line separator (`\n` Unix,
  `\r\n` Windows — cfg-gated).
- `%a` = `Double.toHexString` (`0x1.8p0`, `-0x0.0p0`, `NaN`,
  `Infinity`, subnormal `0x0.0000000000001p-1022`); `%A` uppercases
  (`0X1.8P0`, `NAN`); precision = hex digits after the point,
  `max(1, p)`, half-up in hex (`%.2a`→`0x1.80p0`; the old
  "precision 0 = full" pin was WRONG — `%.0a`≡`%.1a`). Long arg →
  `a != java.lang.Long`; null → `null`.
- `%tY` of Long = millis since epoch (NOT an error!); null → `null`;
  non-Long → `IllegalFormatConversion` (`Y != java.lang.Double`).
  Unknown suffix → `UnknownFormatConversion` with BOTH chars (`tV`,
  `tq`, trailing `t`). Suffix set: H I k l M S L N p z Z s Q B b h A a
  C Y y j m d e R T D F c (+ uppercase prefix, which uppercases).
  NO V/U/W/G/g/K/O/E. Zone: render UTC, document divergence (oracle
  pinned with TZ=UTC). `%te`/`%tk`/`%tl` are UNPADDED (not
  space-padded!); `%ts` truncates toward zero (`-1`→`0`).
- `%e`/`%f`/`%g` render the shortest decimal `HALF_UP`
  (`%.2f 2.675`→`2.68`, `%.0f 2.5`→`3`); `%g` precision is
  `max(1, p)`, scientific iff the decimal exponent is `< -4` or
  `>= p`, and NEVER strips (`%.10g 123.456`→`123.4560000`,
  `%g 0.0`→`0.00000`); `NaN` ignores sign flags, infinity honors
  them (`%(f -Inf`→`(Infinity)`).
- `%d` of Double → `d != java.lang.Double` (no coercion); `%f` of Long
  → error; `%05d` of -42 → `-0042`; `%(05d` → `(042)`; `%#08x` →
  `0x0000ff` (zeros after the prefix); `%#x` of 0 → `0x0`;
  `%#o` of 0 → `00`.
- `%s`/`str` of a LazySeq do NOT print contents (old "realizes first"
  pin was WRONG): `clojure.lang.LazySeq@<hex>` with hex = Java List
  CONTENT hash (deterministic, not identity: `()`→`@1`, `(2 3)`→`@402`,
  `(\b \c)`→`@1002`). The hash walk realizes (counter-verified), so
  infinite lazies hang — take-bound or startsWith. `%s` honors
  precision-then-width.

## Print / str

- `print-sequential`: level dec-first then neg→`#` (current code correct).
- Negative `*print-length*` = UNLIMITED (never `zero?`); fractional
  non-integral = unlimited; NaN = unlimited; integral double = that int.
  Integral negative level → `#` path per dec (verify `−0.5` in corpus).
- `*print-namespace-maps*` EXISTS, root TRUE with common-ns lifting
  (`{:a/x 1 :a/y 2}` → `#:a{:x 1, :y 2}`; the old root-FALSE pin was
  WRONG — re-verified by the 11-corpus oracle 2026-10-18). Add the
  var (new matrix row); gate `lift_ns`.
- `pr-str`/`print`/`pr`: bounded-realize with `*print-length*`
  (limit+1 then print, so `...` shows and infinite seqs terminate).
  Realization happens up-front (interleaving with in-flight prints is a
  known divergence). `str` is NOT in this family (old pin lumped it
  in): per-arg `.toString` — LazySeq → `clojure.lang.LazySeq@hash`
  (realized or not; never contents), strict colls print readably
  (`["a" :b]` keeps quotes, StringSeq shows `(\a \b)`).
- `subs` index: Double truncates (reflector int-unbox); nil → NPE
  getClass (pinned); OOB message per JDK (`Range [b, e) out of bounds
  for length N` — verify JDK text in corpus).

## Predicates / classes

- `(counted? nil)` → FALSE; `(counted? "ab")` → FALSE (current code
  wrongly true). Counted: strseq/smallmap-seq/LongRange/sort-result/
  vector-chunked/take-last-vector/butlast + collections + transients.
- `simple_class_of`: strip package ONLY (`LazySeq`, `core$map`,
  `PersistentList$EmptyList`). Extensions included.
- `fn?` includes natives (correct). `record?` always false.
- 32-bit ints: `(int 65)` is Long here (documented divergence; `%c`
  rejects all Ints).

## Control / builtins

- `assert`: expansion-time DYNAMIC `*assert*` check (a `binding`
  around `eval` disables); `(assert x)` →
  `AssertionError "Assert failed: <pr-str-of-form>"`; 2-arity →
  `"Assert failed: <msg-str>\n<pr-str-of-form>"` (message str'd,
  evaluated only on failure); success yields nil. Analyzer builtin
  (no `new` available to macros); when off, the test is neither
  analyzed nor evaluated. 0/3+ args → bare macro arity.
- `time`: analyzer builtin; `(prn (str "Elapsed time: " millis
  " msecs"))` shape (prn-quoted), value preserved. `with-out-str`:
  analyzer builtin delegating to the `binding` push/pop discipline;
  nesting rebinds, throws still pop.
- `lazy-seq`: keep core.clj macro → `(lazy-seq* body)`; `lazy-seq*` is an
  ANALYZER form (no var; JVM has no such var). `lazy-cat` macro kept.
  Body values seq-coerce (IAE, not a cast: `(first (lazy-seq 5))`
  is `Don't know how to create ISeq from: Long`); the body is a
  zero-arg `fn` boundary for `recur`.
- `condp`: verbatim core.clj macro (pure expansion, `:>>`
  ternary); no-match (incl. zero clauses) is IAE `No matching
  clause: <str-of-expr>` at runtime. `(pred test expr)` order:
  `(condp < 5 10 :a :d)` is `:d`, `(condp < 10 5 :a :d)` is `:a`
  (probed).
- `case`: verbatim core.clj macro + `case*` ANALYZER form (the
  expansion target must exist). Prep minus the shift-mask search
  (no bit ops; always 0/0, `:sparse` when not fits-table?),
  sorted-map (plain `{}` — imap order is eval-irrelevant),
  `int`/`Integer` casts (identity/literals, pr-identical),
  `into1`/`reduce1`/`ffirst` (`into`/`reduce`/inlined); dispatch
  hashing is `Util/hash` = Java `.hashCode` (NOT murmur —
  `hash(:a)=1013910569`, `hash(:b)=1013910508`). Modes: all
  int-sized integers → `:ints` (non-`Number` exprs miss, then an
  `equiv` post-check — `(case 97 \a …)` and `(case 1.5 1 …)` miss);
  all keywords → `:hash-identity` (post-check is `equiv` here —
  keywords are not interned); else `:hash-equiv`. Dup detection at
  expansion: IAE `Duplicate case test constant: <str-of-form>`
  (`(str nil)` = `""`, strings unquoted). `(case 1 () :a :b)`:
  `()` is a zero-const group (`(seq? '())` is TRUE), vacuous
  `:ints`, then bare `(max)` arity (Q1, 7g15 closed — no
  "non-const" error exists; any test form is a const, lists are
  groups). Test consts are LITERAL DATA in `case*` (probed:
  `(case 'a 'a :x :d)`, `(case 'a (a) :x :d)`, `(case '(1 2)
  ((1 2)) :x :d)` all hit; a `def`'d name matches as the symbol,
  never evaluated). The macro emits consts unquoted (imap `{1 [1
  :a]}`, collisions quote inside the `condp` testexprs); the skip
  slot is nil without collisions, a set with. Q1's bare arity
  escapes UNTOUCHED: a non-own `ArityException` during expansion
  rethrows as-is (the wrapper previously rebuilt it with the macro
  call's argc — fixed). Expr evaluates once (let-bound `ge`).
  No-match without default is IAE `No matching clause: <str>`;
  `(case 5)` throws it, `(case 1 1)` yields `1`. `(case 1 1 :a 1.5
  :b :d)` is `:a` (probed). `case*` shape:
  `(case* ge shift mask default imap switch-type test-type
  skip?)`; hand-written `case*` beyond the macro shape is a
  port-level spec error (the JVM NPEs/IAEs depending on position —
  out of scope).
- Macro-error classes are exact via two private natives (macros
  have no `new`): `throw-illegal-arg!` (cond-odd, assert-args,
  defn-name, for-keyword, case-dup, case/condp no-match) and
  `throw-npe!` (`doseq` invalid keywords reproduce the JVM
  `steppair` NPE verbatim, JDK 26 re-verified — a JVM bug the
  corpus pins). `hash-code` (private) exposes `.hashCode` for
  `case` prep. Expansions emit private calls as `(#'...)` (the
  `var` form bypasses the privacy check — probed P1; a qualified
  call from another ns is ISE even in expansions). Bodies call
  them bare (resolved once, at `def` in `clojure.core`).
- Sequential destructuring delegates non-`nil`/vector/list/string
  colls to `nth` (lazy seqs force and walk, subvectors index —
  `(split-at …)` take-thunks in `condp`'s emit need this); the
  `recur`-mismatch chain is `Syntax error compiling recur` + IAE
  cause (probed P2).
- `trampoline`: plain native loop. `ex-info` family: natives over
  `Value::Throwable`.
- Roots: `*print-meta*` FALSE (not nil); `*print-readably*` TRUE;
  `*print-dup*` FALSE; `*assert*` TRUE; `*unchecked-math*`,
  `*warn-on-reflection*`, `*allow-unresolved-vars*` FALSE;
  `*print-length*`/`*print-level*` nil; `*print-namespace-maps*`
  TRUE. Dynamic SLOT set for all — but the `RT.java` interns
  (`*assert*`, the three flags, `*out*`/`*err*`, the print triad)
  carry NO `:dynamic` meta (corpus 11/15 pin `false`), while the
  `core.clj` `^:dynamic` trio (`*print-length*`/`*print-level*`/
  `*print-namespace-maps*`) carry meta `true`.
- `odd?`/`even?`: low-bit; non-integers (incl. `2.0`, strings,
  nil) are IAE `Argument must be an integer: <str>`.
- `hash`: `hasheq` after a full realize; unbounded `Repeat`/
  `Range`, zero-step `Range`, any `Cycle`/`Iterate` — top-level or
  nested in realized collections — throw bare UOE (the JVM
  `hasheq` overrides; checked before realizing, and again after
  forcing reveals a typed infinite). Forcing a genuinely infinite
  lazy thunk hangs, like the JVM (`LazySeq.hasheq` iterates).
  `(repeat 0 x)` is `()`, so it hashes (`-2017569654`).
- `eval`/`load` failures now wrap in a real `CompilerException`
  with the structured cause (S5A's flat shape restructured — the
  corpus `pe` chains and `(catch CompilerException …)` require
  it); bare macro-arity failures still escape unwrapped. The
  `clojure.lang.Compiler$CompilerException` FQN resolves in
  `catch` (the short stays unresolvable, JVM-true).

## Corpus rules

- Oracle flags: `-Duser.language=en -Duser.country=US -Duser.timezone=UTC`.
- Avoid: identity hashes (`(hash (transient …))`, `#object[…0x…]`
  — normalize hex or avoid), wall-clock `time` output (normalize or
  match shape), zone-sensitive `%t` outside UTC-pinned runs.
- Set/big-map order: normalize inside entries (`sort-by pr-str`).
- Drop expected-output files: entries print tagged lines; runner diffs
  normalized stdout.

## 7i range ctors/coercions (7i, 7i4, 7i6)

- 1-arg: `(range nil)` → NPE getClass; `(range "a")` → CCE
  String→Number; `(range (int 5))` → `(0 1 2 3 4)`; `(range 5.0)` →
  class `Range`, elements all Long.
- 3-arg: ANY non-Number in ANY slot (incl. step 0) → CCE/NPE:
  `(range 1 2 "a")`, `(range 1 "a" 1)`, `(range "a" 2 1)`,
  `(range "a" "b" 0)`, `(range 1 "a" 0)`, `(range "a" 1 0)`,
  `(range "a" "b" 1)`, `(range nil nil)`, `(range 1 nil 1)`,
  `(range "a" "a" 1)`, `(range nil nil nil)` all throw (String→CCE,
  nil→NPE getClass). Zero-step saves nothing.
- `(range 0 5 -0.0)` → `Repeat` (neg-zero step = zero);
  `(range 0.0 5 0.0)` → `Repeat` of Doubles; `(range 5 5 0)` → `()`;
  `(range 5 4 0)` → `(repeat 5)`.
- Overflow: `(range 0 MAX 1)`/`(range -5 MAX 2)` → class `Range`
  (count overflow → double path); `(range (MAX-1) MAX 5)` → take 2 →
  `((MAX-1))`; `(range MIN MAX -1)` → `()`.
- NaN: `(range 0 ##NaN)` → infinite `Range` `(0 1 …)`;
  `(range ##NaN ##NaN)` → `Range`, take 1 → `(##NaN)` (equiv false →
  not empty; infinite NaNs); `(range 1 1.0)`/`(range 1.0 1)` → `()`
  (cross-type equiv → empty; class `EmptyList`).
- NaN step: `(range 0 5 ##NaN)` → take 3 → `(0 ##NaN ##NaN)`;
  `(range 0.0 5 ##NaN)` → `(0.0 ##NaN ##NaN)`; class `Range`.
- Repeat: `(repeat 2.5 :x)` → 2 items (TRUNCATE — repeat is in the
  truncate family with nth/subvec/subs); `(repeat nil :x)` → NPE
  `Number.doubleValue()`; `(repeat "a"/true :x)` → CCE;
  `(repeat MAX :x)` → `Repeat`, take 1 → `(:x)`;
  `(take 1 (repeat :x))` → `LazySeq`.

## 7i iterate/cons/cycle/interleave (7i, 7i5, 7i6)

- `(iterate 5 0)` → CCE Long→IFn; `(first (iterate nil 0))` → 0;
  `(rest (iterate nil 0))` → `Iterate`, no throw;
  `(first (rest (iterate nil 0)))` → NPE `this.f` null.
- Counters (fresh `(iterate f 0)`): `first`→0 calls; `rest`/`next`→0;
  `first`-of-`rest`/`next`→1; `rest`-of-`rest`/`next`-of-`rest`→1;
  `(nth it 2)`/`first`-of-`(drop 2 it)`→2. Rule: `rest`/`next`
  force the CURRENT val only; the new val stays lazy.
- `(cons 1 nil)` → `PersistentList` `(1)`; `(cons 1 [2])` → `Cons`,
  rest → `ChunkedSeq`; cons never forces tail (counter 0).
- `(cycle nil)`/`(cycle [])` → `EmptyList` (class + value);
  `(cycle [1])` → `Cycle`, rest → `Cycle`.
- `(interleave)` → `()` (NOT nil); `(interleave [1 2])` → `LazySeq`,
  vec → `[1 2]`; `(concat)` → `LazySeq`, NOT nil.

## 7i drop/nthrest coercions (7i–7i4, 7i6)

- `n` rule: non-positive/NaN → return as-is with NO coercion
  (`-1e100`, `##NaN`, MIN, `-1`, `-0.5` on `[1 2 3]` → `"(1 2 3)"`,
  class `ChunkedSeq`); positive → CEIL, then long-check (IAE `Value
  out of range for long: X` for `1e100`), then int-check (AE
  `integer overflow` for MAX, `5e9`, `(double MAX)`, 2^32, 2^32+1).
- Fractional rounds UP: `drop` 2.4/2.5/2.6 → drop 3, 0.4/0.5 →
  drop 1. Same ceil for `take` (2.4/2.5 → 3 items, 0.5 → 1) and
  `nthrest` (1.5 → drops 2). Truncate-family = nth/repeat/
  subvec/subs (`2.5`→2); ceil-family = drop/take/nthrest (`2.5`→3).
- `nil`/`"a"` n on IDrop → NPE getClass / CCE AT CONSTRUCTION; on
  non-IDrop (incl. `(drop nil 5)`, transient vector) construction
  returns `LazySeq`, throw on force: `str` → IAE `Don't know how
  to create ISeq from: java.lang.Long` (resp. `...TransientVector`).
- Huge-n on seqs walks to end silently (no AE/IAE): `(drop 1e100
  '(1 2 3))`/`(drop 1e100 "abc")`/`(drop MAX "abc")` → `LazySeq@1`.
- `nthrest`: 0/neg → coll ITSELF (`[1 2 3]` vector, never seqed);
  vec+MAX → AE; list+MAX → `()`; `"a"` → CCE on vec AND list
  (coerces before coll dispatch).
- `subvec`/`subs`: MAX → AE (all 3 slots); 1.5 truncates (`[2 3]`,
  `"bc"`); `"a"` → CCE; `nil` → NPE charValue (nth-family path,
  NOT getClass).

## 7i keys/vals tails + seq metas (7i3, 7i6)

- `(keys {})/(vals {})/(keys nil)/(vals nil)` → nil.
  `(keys {:a 1})` → `APersistentMap$KeySeq`; `(first {:a 1})` →
  `MapEntry`.
- Construction never validates: `(keys [10 20])` → `KeySeq`; only
  entry-force throws (`first` → CCE Long/Vector→`Map$Entry`).
- Tails never validate: `(rest (keys [[1 2]]))` → `EmptyList`;
  `(first (rest …))` → nil; `(next (keys [[1]]))` → nil;
  `(next/(rest (vals [1 2])))` → `ValSeq` (first-of-tail → CCE).
- `(rest (keys {:a 1}))` → `EmptyList` `"()"`;
  `(first (rest (keys {:a 1 :b 2})))` → `:b`;
  `(next (keys {:a 1 :b 2}))` → `KeySeq` (meta dropped → nil);
  `drop` on KeySeq → `LazySeq` (KeySeq NOT IDrop);
  `counted?(rest …)` → false.
- `meta(next (with-meta s m))`: `StringSeq` KEEPS (`rest` too);
  vector-chunked, range, repeat, cycle, iterate, list,
  drop-1, sort → nil. `with-meta` preserves the seq class.

## 7i take-last/butlast/drop-last (7i3, 7i6)

- `(take-last 2 [1 2 3 4])` → `(3 4)` `ChunkedSeq`; list →
  `PersistentList`; `"abcd"` → `StringSeq` `(\c \d)`; 0/-1 → nil;
  `nil` n → NPE getClass (EVEN on lazy — validation is eager);
  `"a"` → CCE; `(range 10)` → `LongRange`; double-range → `Range`;
  `(map …)` → `ChunkedCons`; `(repeat 3 :x)` → `Repeat` `(:x :x)`;
  `1e100` → IAE long-range.
- `butlast`: `[1]`/`nil`/`[]` → nil; `[1 2]` → `(1)`; always
  `ChunkedSeq` (string in → chunked out; `(butlast "abc")` →
  `"(\a \b)"`). All 7i3 butlast cells duplicate corpus 05.
- `drop-last` returns `LazySeq` (`(drop-last 1 [1 2])` →
  `LazySeq@20` = `(1)`).

## 7i partition step (7i5, 7i6; no earlier pin existed)

- Construction fully lazy (`(partition 2 "a" [1 2])` → `:ok`);
  FIRST force coerces step via STRICT `nthrest` on ANY coll: `"a"`
  → CCE (vec AND list, `partition` AND `partition-all`); `nil` →
  NPE getClass.
- MAX step: vector → AE `integer overflow` (ChunkedSeq IDrop);
  list → single window `((1 2))` (`@401`, walk stops at end);
  `partition-all`+vector → AE.
- `(partition 2 0 [1 2 3])` → `((1 2) (1 2))` (`@8001`); n/step 0
  or `partition-all`-neg → infinite empties (take 2 → `[() ()]`);
  `(partition -1 …)` → `()` (corpus 06); pad:
  `(partition 2 2 [9] [1])`/`(partition 3 3 [9] [1])` → `((1 9))`.

## 7i str/tostring + hash (7i2–7i4, 7i6)

- `str` never prints lazy CONTENTS (realized or not):
  `LazySeq@<hex-Java-List-content-hash>` — deterministic, NOT
  identity: `()`→`@1`, `(2 3)`→`@402`, `(\b \c)`→`@1002`,
  `((1 2) (1 2))`→`@8001`, `(1 1)`→`@3e1`, `(1 2)`→`@3e2`, `(1)`→
  `@20`. Same via `%s`. The hash walk REALIZES (counter 2/2) —
  infinite lazies hang; corpus uses take-bounded/startsWith.
- `print-str`/`pr` DO print contents (`(map inc [1])` → `"(2)"`).
- `str` of strict colls/seqs prints readably: `[1 2]`, `(1 2)`,
  `{:a 1}`, `#{1}`, `(:a)`, `(0 1 2)`, `(:x :x)`, `["a" :b]`
  (quotes kept), `(\a \b)` for StringSeq (chars escaped).
- `seq` returns self: LongRange/Repeat/Cycle/Iterate/Cons/KeySeq.
- `(hash <infinite>)` → UOE with EMPTY message (cycle, iterate,
  repeat, `(range 0 5 0)`); finite seqs hash fine
  (`(take 1 (repeat :x))` → 1469441678, `(map inc [1 2])` →
  -1504369821, `(range 10)` → 804834377, `(filter odd? [1 2 3])`
  → -1179299920).

## 7i `=`/equiv early-exit (7i6)

- `(= 1 1.0)` → FALSE (int/double not equiv); `(= 1N 1)` → true;
  `(= ##NaN ##NaN)` → false; `(= nil nil)` → true,
  `(= nil false)` → false.
- Sequential equiv crosses types: `(= (range 3) [0 1 2])` and
  `(= [1 2] '(1 2))` → true.
- Pairwise + early exit (counter-verified): `(= (repeat :x) [:y])`
  → false (terminates); `(= (range) [0 1])`/`(= [0 1] (range))` →
  false (terminate at the shorter end); mismatch at pair 2 forces
  exactly 2+2; count-mismatch forces the longer side +1.
- `(= {:a 1} {:a 2})`/`(= {:a 1} {:b 1})` → false;
  `(= #{1} #{1})`/`(= \a \a)` → true.

## 7i transients in access position (7i6)

- `(get (transient [1 2]) 0)`/`(get (transient {:a 1}) :a)` → 1;
  miss+`:nf` → `:nf`; `(nth … 5)` → IOBE `""`;
  `(count (transient [1 2]))` → 2; `counted?` → true;
  `(contains? (transient {:a 1}) :a)` → true;
  `(find (transient [10 20]) 1)` → `[1 20]`.
- Transients are NOT seqable: `seq`/`first`/`drop`-force → IAE
  `Don't know how to create ISeq from: …TransientVector`.

## 7i probe anomalies

- 7i: `i-drop-wrap` whole-fn AE (2^32+1 eager-AE killed the tail:
  2^32/2.5/-1 recovered in 7i6); `i-drop-err` print-time IAE
  (`Long→ISeq` from uncapped `(drop nil 5)`) killed the script —
  all later 7i labels lost (re-probed in 7i2–7i4).
- 7i2: `i2-keys` print-time CCE (`doall` returns the UNFORCED
  KeySeq; `pr-str` forces it) killed the script — later 7i2
  labels lost (re-probed in 7i3/7i4).
- 7i3: `i3-drop-err` whole-fn IAE (uncapped `str` of 1e100-drop);
  `i3-partition-big` print-time AE (`cap` can't catch lazy `take`
  — `toIntExact` blew in `pr-str`) killed the script;
  range-nanstep/subvec/nthrest/tostring lost (re-probed in 7i4).
- 7i4: `i4-partition-big` whole-fn CCE (uncapped `str` of
  bad-step `take`).
- `/tmp/s5d-out7i5.txt` was missing — 7i5 run by hand (same
  oracle flags) during doc work.
- Lesson (3rd instance): never print a bare lazy value in probes
  — force inside `cap` (`str`/`vec`); `take`, `doall`-of-KeySeq
  and `drop`-of-bad-coll all defer throws to print.

## Fixbatch (coverage-audit fixes; fixprobe 1–7, same oracle flags)

- `not` is falsiness, not nilness: `(not false)` → true,
  `(not 0)`/`(not "")`/`(not '())` → false (`:not-*`).
- `max`/`min`: ties keep the LATTER with its type (`(max 1 1.0)`
  → `1.0`, `(max 1.0 1)` → `1`); `NaN` poisons from either side
  (`(max 1 ##NaN)` and `(max ##NaN 1)` → `##NaN`); 1-arg passes
  through unchecked (`(max :a)` → `:a`); 0-arg is ArityException
  (`:max-tie`, `:max-nan1/2`, `:max-1kw`, `:max-0`).
- `float?` ≡ `double?` here (`(float? 1.0)` → true; the old
  always-false body contradicted golden `01-preds.out:32`).
- `==`/`<`/`>`/`<=`/`>=`: 0-arg is ArityException; 1-arg passes
  UNCHECKED (`(== :a)`, `(< "s")`, `(> nil)` → true); 2+ args
  cast to `Number` (verbose CCE, `:eqeq-1kw`, `:lt-1str`).
- `quot`/`rem`/`mod` int paths: `quot`/`rem` zero says
  `AE | / by zero`, `mod` zero says `AE | Divide by zero`
  (`:quot-i0`, `:mod-i0`); `/` int zero stays `Divide by zero`
  (`:div-i0`), `/` double zero stays `Inf`/`NaN` (`:div-d0`).
- `quot`/`rem`/`mod` double paths (uniform prefix: `y == 0.0`
  → `AE | Divide by zero` FIRST, even over `NaN`
  (`:quot-nan-0`); non-finite `x` or `NaN` `y` →
  `NFE | Infinite or NaN`): `quot` truncates toward zero with
  `-0.0`→`+0.0` (`:quot-negfrac`, `:quot-2-neginf`) and a
  non-finite quotient is NFE (`:quot-ovf`); `rem` with an
  infinite divisor is `NaN`, NOT `x` (`:rem-num-inf`); `mod`
  runs `((x%y)+y)%y` (infinite divisors fall out as `NaN`,
  `:mod-num-inf`).
- `empty` (`RT.empty`, `:empty-*`, corpus 08): maps class-sticky
  (array→array, hash→hash), vectors/lists/sets emptied in kind,
  subvectors → plain `[]`, seqs/lazies/ranges → `()` (bare, no
  meta — inferred from `RT.empty`), everything else (nil,
  strings, scalars, keywords, chars, bools, transients) → nil;
  metadata kept on the strict kinds (`:empty-meta-*`).
- `vec` on a non-seqable is `RT.toArray`, not a seq failure:
  `RuntimeException | Unable to convert: class X to Object[]`
  (`:vec-num` Long, `:vec-trans` TransientVector).
- `Cons` is never `Counted` (`:cc-cons`, `:cc-range`,
  `:cc-lazy` — reverses the old `c4` read; coll-pins fixed).
  `(count cons)` still walks to the length.
- `mapcat` splices each outer item (`(apply concat (map …))`):
  nil/empty items vanish, non-seqables fail `IAE | Don't know
  how to create ISeq from: X` (the old port shape never
  concatenated — caught by the `not`-fix ripple in `for`).
- Ident fns (fixprobe8): 1-arg `keyword` else-arm is nil
  (`(keyword 5)` → nil; only the 2-arg arity casts to `String`);
  `(symbol :a)` → `a`, `(symbol nil)`/`(symbol 5)` →
  `IAE | no conversion to symbol`; `(name nil)` →
  `NPE | Cannot invoke "clojure.lang.Named.getName()" because
  "x" is null` (`getNamespace` likewise); `name`/`namespace`
  on mistypes are verbose `Named` CCEs.
- Print limits (fixprobe9): length AND level share the
  nil/negative/fractional/NaN→unlimited, integral-double→that-int
  rules; level 0/negative prints `#` (`:lev-0`, `:lev-neg`,
  `:len-negdbl`, `:lev-frac`, `:lev-nan`). `*print-meta*` prints
  the `^meta` prefix (`:meta-true`), `*print-readably*` is honored
  via `pr`/`print`, and `*print-namespace-maps*` gates the
  `#:ns` lift (`:nsmaps-false`) — the old "ignored by the
  printer" notes for `-meta`/`-readably` were stale.
- Coercions (fixprobe10–12): `int` stays `Long` but
  range-checks (longs past i32 → `AE | integer overflow`,
  doubles past it → `IAE | Value out of range for int: D`
  checked BEFORE truncating, `NaN` → 0); `long` likewise
  with the i64 range; `short`/`byte`/`char` IAE on `NaN`
  (unlike `int`/`long`); `float` range-checks and returns the
  exact double (`NaN` passes, `Inf` is IAE);
  `int`/`long`/`short`/`byte` take Characters (97),
  `double`/`float` CCE them; nil trips `charValue` (`int`),
  `doubleValue` (`long`/`double`/`float`/`short`/`byte`) or
  `longValue` (`char`) NPEs; mistypes CCE→`Character` (`int`)
  or →`Number` (the rest). Out-of-range doubles render via
  `Double.toString` (`1.0E100`, `Infinity`, `NaN`). `char`
  truncates (65.9→`\A`), rejects negatives/past-`0x10FFFF`
  and — port-divergent — surrogates (the JVM accepts them
  unprintably, printing `\?`).
- `disj` 1-arg passes through; 2+ args nil-pun, else
  CCE→`IPersistentSet` (same-module shape, `:disj-vec`).
  `set` goes through `seq` (`:set-5` is the seq-failure IAE,
  not `Unable to convert`).
