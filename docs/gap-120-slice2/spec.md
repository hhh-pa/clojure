# Gap #120 slice 2: emulated construction

Part of #120. Port-native construction over the emulated lattice — no
JVM at runtime, ever (owner direction on the issue).

## 1. Scope

Slice 1 landed static `Class/forName` + fully-qualified class
resolution. Slice 2 adds the three construction spellings for an
allow-list of emulated classes whose values have **exact** lattice
classes, plus JVM-text ctor misses everywhere else:

- `(new Classname args...)` — the `new` special form, generalized
  beyond `(new Object)`. Non-`Symbol` heads and the bare `(new)`
  shape are compile errors with JVM text.
- `(Classname. args...)` — the trailing-dot spelling (same runtime
  mechanism as `new`).
- `(Classname/new args...)` — the static-call spelling (discovered
  during oracling: the JVM constructs through it, attributed to the
  head as written).

Allow-list (exact value semantics only):

- `java.lang.String` — the full port-representable overload matrix
  (§3): `()`, `(String)`, `(char[])`, `(byte[])`,
  `(char[],int,int)`, `(byte[],int,int)`, `(int[],int,int)`,
  `(byte[],int)` hibyte, `(byte[],String)`,
  `(byte[],int,int,String)`.
- `java.lang.Object` — 0-arg only (fresh `Fx4Object` per call;
  `(new Object)` reroutes from its analyze-once literal through the
  same runtime path).

Everything else keeps a precise miss: unknown overloads are
`No matching ctor found for class|interface X`; non-allow-listed
lattice classes (wrappers, collections, …) take the same uniform
miss even where the JVM succeeds (documented narrowing N4);
unresolvable names are `Unable to resolve classname: X` /
`ClassNotFoundException` at runtime (narrowing N1).

Out of scope (residue §6): wrapper/collection/Exception ctors,
`StringBuilder`/`StringBuffer` values, `Charset`-object overloads,
long-tail charsets, value-position `Class/new`, compile-time
HostDef arity checks.

## 2. Oracle

`probes/ctor{,2,3,4,5}.{clj,out}` — ~160 rows, run with:

```
CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:
   /tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -Dfile.encoding=UTF-8 -cp $CP clojure.main ctorN.clj > ctorN.out
```

on `java version "27" 2026-09-15` (HotSpot 27+35). Default charset
is UTF-8 (`file.encoding=UTF-8` row in every batch). Every pin in
`host_ctor.rs` cites its oracle label.

Key oracle findings (all pinned):

- Arity existence checks at **compile** time (`ctor-str-arity5`,
  `ctor-obj-arity`, `new-obj-arity`, `cn-arity5`, `m-obj-new-arity`);
  overload applicability within an existing arity fails at
  **runtime** (`ctor-str-arity2`, `ctor-str-badtype`,
  `ctor-str-coerce-double`, `cn-badtype`, `d-str-nil`, …).
- Nil overload ambiguity is a compile error on the JVM
  (`ctor-str-nil1/2/3`, `d-bytes-nil`, `d-nil-0-0-nil`,
  `cs-nil-name`, `cn-nil1`, `s-new-nil`); the port reports the same
  text at runtime (narrowing N2).
- `int` params match **Long only**, range-checked (`RT.intCast`:
  `ctor-str-coerce-oobig`, `c-hibyte-big` → `ArithmeticException |
  integer overflow`). Double/BigInt/BigDec/Ratio/Char/Bool never
  match (`ctor-str-coerce-double`, `c-bigint`, `c-bigdec`,
  `r-half`, `c-char`, `c-bool`). (`c-ratio` was vacuous: the reader
  reduces `1/1` to `Long`, oracle `r-class`.)
- `StringIndexOutOfBoundsException` text is the JDK
  `checkFromIndexSize` template, expression unreduced:
  `Range [5, 5 + 1) out of bounds for length 1`
  (`ctor-str-off-oob/neg`, `ctor-str-cnt-oob`, `cs-off-oob`).
- Every charset-name lookup failure — unknown, empty, illegal
  chars — is `java.io.UnsupportedEncodingException | <name>`
  verbatim, no trimming (`ctor-str-badcs/4`, `v-empty`,
  `v-badchars`, `v-colon`, `s-spcs`).
- Charset lookup precedes bounds/null checks (`e-badboth`,
  `e-nil-badcs` → UEE); null bytes NPE before bounds
  (`e-nil-badbounds`).
- The `(int[],int,int)` ctor throws `IAE | <int>` below 0 / above
  `0x10FFFF` (`ctor-str-badcp`, `cp-toobig`) and **embeds**
  unpaired surrogates (`sg-hex` → `d800`); the port maps those to
  U+FFFD (narrowing N5 — Rust `char` cannot hold surrogates; U+FFFD
  keeps `=` honest where `?` would not).
- Decoders use REPLACE (`cs-ascii-high`, `cs-utf16-odd`,
  `u32-trunc`, `u32-badcp` → U+FFFD); `UTF-16`/`UTF-32` strip a
  BOM of either endianness, else decode big-endian
  (`cs-utf16le-bom`, `u32-bom`, `u32-le`, `s-u32lebom`,
  `bom-le/be`).
- `(Class/new …)` constructs on the JVM for every class —
  including deftypes by simple name (`cn-*`, `m-deftype-simple-new`
  — the var root holds the class, so the port consults
  descriptors for `member == "new"`); value position is an invoker
  fn (later slice, narrowing N7); instance position
  `(.. String (new "x"))` is a method miss against the
  *represented* class (narrowing N8).
- Same-form define+use works (`o-do-deftype-dot/new`,
  `o-do-import-use`): ctor class resolution stays **runtime**;
  compile-time checks fire only on order-independent knowledge
  (the `new` head shape + the allow-list arity table). Vars never
  participate on the JVM (`h-var-dot`); they must on the port
  (deftypes live in vars) — narrowing N3.
- `new` and `Class/new` ignore locals (`nl-new-shadow-str`,
  `h-local-static`, `h-local-new`, `cn-shadow-new`).

## 3. Contract

### 3.1 `new` lowering (analyzer, pre-shadow like `(new Object)`)

- `(new)` → compile error, `RuntimeException | wrong number of
  arguments, expecting: (new Classname args...)`, attributed `new`
  (`new-empty`).
- `(new head …)` with a non-`Symbol` head → compile error,
  `IllegalArgumentException | Unable to resolve classname:
  <value_of(head)>`, attributed `new` (`new-num-head`,
  `new-str-head`, `new-nil-head`, `new-list-head`, `new-kw-head`,
  `nl-new-numclass`, `m-new-qual*` render via the shared
  `value_of`: nil → `null`, else `str`).
- `(new Sym …)` → `(__new* 'Sym …)` (one runtime mechanism for
  all spellings). Before lowering, if `Sym` resolves through the
  import table (simple) or the shared lattice (dotted) to the
  allow-list, the arity must exist (§3.4) else the compile-time
  no-ctor error attributed `new`. Vars are deliberately not
  consulted (forward refs stay runtime).
- `(new Object)` reroutes from the analyze-once literal to
  `(__new* 'Object)` — fresh object per call. All existing
  `(new Object)` pins are single-evaluation and stay green.

### 3.2 `(Class. …)` (analyzer)

Unchanged lowering to `(__new* 'Class …)`, plus the same
allow-list arity check as `new`, attributed `new` (the JVM
attributes the dot spelling to `new` too: `ctor-str-arity5`).

### 3.3 `(Class/new …)` (analyzer, static-call path)

When the static head resolves (slice-1 rules: namespaces/aliases
win, FX4 exempt) and `member == "new"`:

- Resolve `ns_part` for the arity check only: import/lattice as
  in slice 1, PLUS var roots holding a `HostDef` descriptor
  (deftypes by simple name: `m-deftype-simple-new`). A var root
  holding anything else falls through to the ordinary var miss
  (`h-var-new` → `No such namespace`, no new code).
- Missing namespace parts keep the slice-1 rule: simple →
  `No such namespace` at compile (`cn-missing`); dotted →
  `ClassNotFoundException` at compile (`cn-missing-fq`).
  (Same-form define+use still works: top-level `do` splits
  like `Compiler.eval`, so each element analyzes after the
  previous evaluates — oracles `o-do-*`. Only a *nested*
  forward ref meets the eager rule.)
- Known allow-list + bad arity → compile-time no-ctor error
  attributed to the head as written (`cn-arity5`, `m-obj-new-arity`
  — `String/new`, `Object/new`, not `new`).
- Otherwise lower to `(__new* 'ns.part …)`; runtime resolves
  (symbol arm: var, then import, then the dotted lattice
  fallback).

### 3.4 Compile-time arity table

`ctor_arities`: `java.lang.String → {0,1,2,3,4}`,
`java.lang.Object → {0}`. Miss text (shared with runtime):
`No matching ctor found for class <FQN>` (`ctor-str-arity5`,
`ctor-obj-arity`, …). HostDef types keep baseline runtime arity
errors (residue R3).

### 3.5 Runtime dispatch (`__new*`)

- `Symbol`: `resolve_def` hit → construct / interface no-ctor
  (unchanged, incl. `No matching ctor found for class <HostDef>`
  for interfaces — residue R4). Miss because unresolvable →
  dotted names try the shared lattice (FQ JDK classes), then
  `ClassNotFoundException` (`ctor-missing-fq`, `new-missing-fq`);
  simple/qualified names → `IllegalArgumentException | Unable to
  resolve classname: <sym>` (`ctor-missing-simple`,
  `new-missing-simple`, `m-new-qual*`). Bound non-descriptors keep
  `Not a type` (baseline).
- `Class` value: by name — allow-list builds; `HostDef` names
  construct (interface check first); anything else the uniform
  miss `No matching ctor found for class|interface <FQN>`
  (interface iff the lattice says so: `ctor-iface` vs
  `ctor-abstract`, `ctor-private`).
- `HostType`/other heads: unchanged (`Not a type`, CCE).

### 3.6 String overloads (runtime)

Arity 0: `""`. Arity 1: `(Str)` fresh copy; `(char[])` collect;
`(byte[])` UTF-8 lossy; nil → ambiguity error; else no-ctor.
Arity 2: `(bytes,Int)` hibyte; `(bytes,Str)` named decode;
`(bytes,nil)` / `(nil,nil)` → ambiguity; `(nil,Int)` /
`(nil,Str)` → unique match, then bytes-NPE; else no-ctor. Arity 3:
`(chars|bytes|ints,Int,Int)`; `(nil,Int,Int)` → ambiguity; else
no-ctor. Arity 4: `(bytes,Int,Int,Str)`; `(nil|bytes,Int,Int,nil)`
→ ambiguity; else no-ctor. Other arities → no-ctor (surface
callers can't reach these — the analyzer gates them).

- Ambiguity text: `More than one matching method found:
  java.lang.String` (verbatim, incl. `method` for ctors).
- Int coercion: `Value::Int` only, `i32` range-checked →
  `ArithmeticException | integer overflow`.
- Check order per overload: structural match → int coerce →
  charset lookup (UEE) → null-bytes NPE → bounds (SIOOBE) →
  build.
- Bounds: `Range [<off>, <off> + <count>) out of bounds for
  length <len>`.
- Bytes-NPE (port-pinned, house style):
  `Cannot invoke "java.lang.String.<init>(byte[])" because "bytes"
  is null` (JDK text is an internal deref message and varies).
- Named charsets: UTF-8 (+`utf8`), US-ASCII (+`ascii`),
  ISO-8859-1 (+`iso8859-1`, `latin1`), UTF-16 (+`utf16`,
  `unicode`), UTF-16BE (+`unicodebig`), UTF-16LE
  (+`unicodelittle`), UTF-32 (no aliases probed —
  case-insensitive core name only), matched
  case-insensitively. All other names → UEE.
- Fresh allocation on every path: `(identical? (String. …)
  <literal>)` is false (`ctor-str-ident`, `fr-e`, `fr-empty`,
  `fr-chararr`) — never return the argument `Arc`.

### 3.7 Charset alias evidence

Shipped aliases are exactly the probed ones: `utf8` (`cs-alias`,
`a-*` batch: `ASCII`, `Latin1`, `ISO8859-1`, `ISO-8859-1`,
`UTF16`, `Unicode`, `UnicodeBig`, `UnicodeLittle`, `utf-16`,
`utf-16be`, `Utf-8`). `UTF-32` core names probed (`u32-*`,
`s-u32lebom`); no `UTF-32` alias probed, so UTF-32 ships with no
aliases beyond case-insensitivity. Anything else (incl. valid
JDK names like `windows-1252`, oracle `tail-cp1252`) → UEE
(narrowing N6).

### 3.8 Object

0 args → fresh `Fx4Object`; else no-ctor (runtime; the analyzer
gates surface arities).

## 4. Narrowings (all oracle-evidenced, all pinned)

- N1 (phase): unresolvable class names fail at runtime, not
  compile (`ctor-missing-*`, `new-missing-*`, `h-var-dot`).
  Forced by same-form define+use (`o-do-*`): the analyzer must
  not resolve classes. Texts match the JVM exactly
  (`Unable to resolve classname: X` / CNFE).
- N2 (phase): nil ambiguity fails at runtime, not compile
  (§2 list). Same text as the JVM.
- N3 (vars participate): a var root that is not a descriptor is
  `Not a type: X` where the JVM says `Unable to resolve
  classname` (`h-var-dot`); vars must participate or deftypes
  can't construct. Baseline behavior, kept.
- N4 (allow-list): non-allow-listed lattice classes miss
  uniformly (`No matching ctor found for class|interface X`)
  even where the JVM constructs (`ctor-int*`, `ctor-long-str`,
  `ctor-bool-str`, `cn-int`). Names outside the lattice
  (`HashMap`, `ArrayList` — oracles `ctor-hashmap*`,
  `ctor-arraylist`) CNFE, exactly like `import` of them does;
  lattice expansion is the future "import of real classes"
  slice. Same table-gated philosophy as slice-1 statics.
- N5 (surrogates): lone surrogates from `(int[],int,int)` and
  `(byte[],int)` become U+FFFD; the JVM embeds U+D800+ (`sg-hex`).
  Unrepresentable in Rust `char`; U+FFFD keeps `=` exact.
- N6 (charset long tail): valid-but-unimplemented charset names
  (e.g. `windows-1252`) throw UEE like unknown names
  (`tail-cp1252`). A full `java.nio.charset` port is its own epic.
- N7 (value position): `String/new` in value position keeps
  `No such namespace: String` (slice-1 narrowing stands; JVM
  yields an invoker fn, `cn-val`). `resolve` of it is nil on both
  (`cn-resolve`).
- N8 (instance position): `new` through `..`/`.`, `memfn` on a
  class value misses against `java.lang.Class` at runtime (the
  slice-1 runtime-miss convention); the JVM misses against the
  represented class at compile time (`m-inst-new`).
- N9 (UTF-8 edge): lossy UTF-8 follows Rust `from_utf8_lossy`
  (REPLACE); pathological malformed sequences could differ from
  the JDK decoder in U+FFFD count. No divergence observed on
  probed inputs.

## 5. Pin flips (oracle-justified, cf. slice 1's three)

- `classemu i-construct`: `(Date. 0)` — `Not a type:
  java.util.Date` → `No matching ctor found for class
  java.util.Date` (uniform miss N4; the old pin documents itself
  as forced divergence).
- `classemu construct_via_class_values`: `(__new* String "x")`
  — `Not a type` → constructs `"x"`.
- `classemu c-class-object`: `(Object.)` — `Not a type:
  java.lang.Object` → constructs (class `java.lang.Object`).
- `types dtor_unresolvable_type`: `(Nosuch. 1)` — `RE |
  Unable to resolve symbol: Nosuch in this context` → `IAE |
  Unable to resolve classname: Nosuch` (JVM text+class; phase
  narrows per N1).

## 6. Residue (later slices, NOT this one)

- R1: wrapper ctors (`Integer/Long/Double/Boolean/…`) —
  class-exactness problem (`(Integer. 5)` must have class
  `java.lang.Integer`, but the port's only int is `Long`).
- R2: collection ctors (`HashMap`, `ArrayList`, …) — same
  class-exactness problem.
- R3: HostDef arity checks at compile time (baseline: runtime).
- R4: HostDef interface miss reads `for class`; JVM reads
  `for interface` (baseline pin kept).
- R5: `StringBuilder`/`StringBuffer` values + their ctors (need
  an observable value type + `str`/`print`/method story).
- R6: `(byte[],Charset)` / `(byte[],int,int,Charset)` overloads
  (no `Charset` values exist).
- R7: `Exception`/`ex-info` ctors (throwable construction is its
  own surface).
- R8: value-position `Class/new` invoker fns (with value-position
  statics generally).
- R9: `String.new` / FQ-member edge: none — covered.
- R10: #163 items (forName arg order, R1 nil rule, 0-arg
  miss-shape) — untouched by this slice.

## 7. Implementation notes

- New module `rust/crates/clj-eval/src/ctor.rs`: the
  `ctor_arities` table, `emulated_ctor` dispatch, the String
  overload matrix, charset decoders, bounds/NPE helpers.
- `fx4_host.rs`: `classify_new` covers all `(new …)` shapes
  (`NewEmpty` / `NewCtor(&Symbol)` / `NewBadHead(&Value)`);
  `HostOp::NewObject` + analyze-time `fresh_object` retire
  (rerouted through `__new*`).
- `expr.rs`: `new` lowering (+ allow-list arity check attributed
  `new`), dot-form allow-list arity check (attributed `new`),
  `Class/new` static arm (§3.3, attributed to the head).
- `types.rs`: `native_new` — `Class`-arm emulated dispatch,
  dotted lattice fallback + CNFE, unresolvable → `Unable to
  resolve classname`; `not_a_class` retires.
- `clj-value/src/extension.rs`: `ThrowClass::
  UnsupportedEncodingException` (`java.io`, parent
  `java.io.IOException`).
- Gates: `cargo fmt --all -- --check`, `cargo clippy --locked
  -p clj-eval -p clj-value --all-targets -- -D warnings`,
  `cargo test --locked -p clj-eval` (incl. new `host_ctor`
  suite) + `-p clj-value`. MSRV 1.85: no let-chains.
