# S15 Java arrays (zero-oos slice 15)

Thirty-seven `out-of-scope` vars with no array values behind
them: a new `Array` value kind (typed, fixed-length, mutable,
nine JVM element types plus arbitrary nesting) with full
`seq`/`count`/`nth`/`=`/`hash`/print integration; thirty-four
fns over it (ctors, coercions, accessors, `bytes?`); the
`amap`/`areduce` macros as `core.clj` ports; and `vector-of`
as its own immutable kind (`PrimitiveVector`).

Target rows (all flipped straight to `implemented` in the one
alpha table, ASCII order): `aclone` `aget` `alength` `amap`
`areduce` `aset` `aset-boolean` `aset-byte` `aset-char`
`aset-double` `aset-float` `aset-int` `aset-long` `aset-short`
`boolean-array` `booleans` `byte-array` `bytes` `bytes?`
`char-array` `chars` `double-array` `doubles` `float-array`
`floats` `int-array` `into-array` `ints` `long-array` `longs`
`make-array` `object-array` `short-array` `shorts` `to-array`
`to-array-2d` `vector-of`.

## Oracle inventory

`docs/zero-oos-s15-arrays/probes/` (alpha10 jars, same header
convention as `docs/i33-typechain/probes/multimethods.clj`):

- `ctors.{clj,out}` (108 lines) — every ctor arity, zero-init,
  seq-fill/truncate/pad, size+init dispatch, `make-array`
  dims/classes, `to-array`/`to-array-2d`/`into-array`, `aclone`
  (incl. 2d-shallow), `alength`, class shapes `h-*`, boxed
  reads `h-boxed-*`.
- `access.{clj,out}` (90 lines) — `aget`/`aset`/8×`aset-*`
  incl. bounds/store/arity errors, the eight casts
  (pass-through + `nil` + CCE matrix), `bytes?`.
- `interop.{clj,out}` (108 lines) — `seq`/`count`/`nth`/`first`
  /`map`/`reduce`, non-seq fns over arrays (`conj`/`assoc`/
  `get`/`contains?`/`find`/`empty`/predicates/`rseq`/`meta`/
  `compare`), `=`/`hash` identity, print shapes `d-*`,
  `amap`/`areduce` behavior + `macroexpand-1`, `vector-of`.
- `edge.{clj,out}` (126 lines) — `into-array` cast matrix,
  2-arity scalar-vs-seq, multi-index tails, `Array.setX`
  widening, direct-`aset` matching, `get`/`contains?` edges,
  `to-array-2d` edges, scalar coercions (port comparison).
- `match.{clj,out}` (66 lines) — widen retry, raw-idx paths,
  `into-array` gaps, `to-array` acceptance (+`identical?`),
  size coercion, `vector-of` gaps.
- `scalar.{clj,out}` — checked-int paths, ctor narrowing,
  2-arity fill dispatch, `Vec`/vector invoke, runtime-nil.

Mechanism sources (read, not vendored): `RT.java`
(`seqToTypedArray`, `toArray`, `object_array`, `intCast`/
`longCast` families, `aset`/`aget` overloads),
`Reflector.java` (`paramArgTypeMatch`, `widenBoxedArgs`,
single-match leniency), `Compiler.java` (`emitUnboxArg` =
`checkCast(Number)` + `intCast`, static literal types),
`core_print.clj` (`print-tagged-object`), `gvec.clj` (`Vec`
interfaces incl. 1-arity `IFn.invoke`).

## BDD scenarios (issue #59)

- `(aget (int-array [1 2]) 1)` reads `2`; `(aset a 0 9)`
  mutates in place (all handles see it).
- `(= (int-array [1]) (int-array [1]))` is false (identity).
- `(amap a i ret (* 2 (aget a i)))` loops over a clone.
- `(alength (make-array Long/TYPE 2 3))` is `2` (port:
  `(make-array :long 2 3)` — keywords, see §8).

## Design: `Value::Array`

New `clj-value/src/array.rs`: `Array { id, desc, cell }`.

- `desc: ArrayDesc { base: BaseType, dims: u32 }` where
  `BaseType` is the eight primitives or `Object(Option<JVM
  class name>)`. `dims >= 1`. Descriptor string: `[` × dims
  + `Z|B|C|S|I|J|F|D` or `L<class>;` (`[I`, `[[I`,
  `[Ljava.lang.Object;`, `[Ljava.lang.Long;`, …). The
  `Object(Some)` tag records `into-array` inference
  (`Long[]`, `String[]`, …) for later `ArrayStoreException`
  checks; explicit ctors use `Object(None)` (= `Object[]`).
- `cell: Arc<RwLock<ArrayStore>>` (mutation is shared, like
  `Atom`'s cell; `Send + Sync`). `ArrayStore`: one variant
  per base (`Bools(Vec<bool>)`, `Bytes(Vec<i8>)`,
  `Chars(Vec<char>)`, `Shorts(Vec<i16>)`, `Ints(Vec<i32>)`,
  `Longs(Vec<i64>)`, `Floats(Vec<f64>)`, `Doubles(Vec<f64>)`,
  `Objs(Vec<Value>)`, `Arrays(Vec<Option<Array>>)`).
- Nesting: `dims > 1` stores `Arrays` whose slots are
  `None` (nil) until set, except rectangular `make-array`
  which pre-fills every level. Inners carry `dims - 1`
  with the same base; storing checks the full inner
  descriptor (`ArrayStoreException` with the stored value's
  descriptor, oracle `b-store-2d`).
- `float` stores `f64`, never `f32` (port-`float`
  precedent: range-checked, exact double; the `[F`
  descriptor still distinguishes the kind). Reads box:
  bool→`Bool`, byte/short/int/long→`Int`, float/double→
  `Double`, char→`Char`, ref→as-is. (JVM boxes to
  `Byte`/`Short`/`Integer`/`Float`/…; the port collapses
  narrow numbers to `Int`/`Double` exactly like its
  `byte`/`short`/`int`/`float` natives do.)
- `=` is identity (`Arc::ptr_eq` on the cell — same cell
  is `=`, anything else is not, oracle `c-ident-*`,
  `c-vs-seq`, `c-vs-vec`). `hash` is `id_hash(id)`
  (stable across mutation, distinct per array, oracle
  `c-hash-*`). No metadata (`meta` → nil, `with-meta` →
  CCE →`IObj`, oracle `b-meta`, `b-with-meta`).
- Allocation guard: payload bytes (full product over
  dims) over 256 MiB, or a `try_reserve` failure, throws
  `OutOfMemoryError` (empty message) instead of aborting.
  Negative sizes throw `NegativeArraySizeException`
  (message = the size, oracle `d-neg-size`, `e-neg-dim`).
- `seq` snapshots elements into a `RealizedSeq` (nil when
  empty). Divergence D1: mutate-between-`seq`-and-realize
  sees stale values (JVM `ArraySeq` reads live); a lazy
  array-seq would need ~10 new dispatch arms for a
  pathological corner — pinned as port behavior instead.

## Design: `Value::PrimitiveVector`

`vector-of` is its own immutable kind, not `Array`-backed:
`Array` is fixed-length and mutable while `Vec` is
persistent (every `conj`/`assoc`/`pop` answers a new
value) — one flag cannot bridge that. New
`clj-value/src/pvec.rs`: `PrimitiveVector { elem: Elem8,
items: Arc<Vec<Value>>, meta: Option<Arc<Map>> }` where
`Elem8` is the eight primitive keywords. Items hold the
*coerced* `Value`s (`Int`/`Double`/`Char`/`Bool`); the
unboxed storage is unobservable in the port (reads box the
same values the JVM boxes, modulo the narrow-number
collapse above).

- `conj`/`assoc` coerce through the JVM `ArrayManager`
  fn for the type — i.e. the port's `int`/`long`/`float`/
  `double`/`byte`/`short`/`char`/`boolean` natives
  (range errors included: `f-conj-bad`, `j-byte-vec`,
  `j-bool-num` truthiness, `j-conj-nil`).
- `=`, `hash`, print, `seq`, `count`, `nth`, `get`,
  `find`, `contains?`, `peek`, `pop`, `empty`, `rseq`,
  `subvec`, `reduce`, predicates, and 1-arity `invoke`
  all behave exactly like vectors/`Vec` (oracle §F/§G):
  sequential `=` incl. cross-type (`f-eq-vo`), vector
  `hash` (`f-hash-eq`), `[…]` print with `^meta` under
  `*print-meta*` (`c-vo-meta`), `empty` stays typed
  (`j-empty-cls`, `j-empty-conj`), `subvec` answers a
  plain subvector (`j-subvec-cls`), `pop` of empty →
  `IllegalStateException` "Can't pop empty vector",
  `dissoc` → CCE →`IPersistentMap` (`g-dissoc`), invoke
  gates on `Util/isInteger` (`Long`/`Integer`/`BigInt`/
  `BigInteger` only — `Short`/`Double`/nil → IAE "Key
  must be integer", OOB → `IndexOutOfBoundsException`
  with empty message, 2-arity → `AbstractMethodError`
  verbatim).
- `counted?`/`sequential?`/`indexed?`/`reversible?`/
  `seqable?`/`vector?`/`coll?`/`associative?`/`ifn?`
  are true; `list?`/`map?`/`set?`/`seq?`/`record?`/
  `fn?` false (oracle `f-*`, `b-*`).

## Semantics

### Constructors (`Numbers` 1- and 2-arity)

1-arity `Object` form: `Number` → size via `intValue`
(narrow: `Int` low-32, `Double` saturate/`NaN`→0,
`BigInt`/`Ratio`/`BigDec` low-32 of truncation; negative
→ `NegativeArraySizeException`), else `RT.seq` the arg
(nil → empty, non-seqable → IAE "Don't know how to
create ISeq from: X", oracle `b-int-nil`,
`b-int-true`, `b-int-dsize`, `f-int-dsize-len`) and fill
from the seq (oracles `b-*`).

2-arity `(int, Object)` form: size through
checkCast+`intCast` (nil → NPE-charValue, non-`Number` →
CCE→`Number`, `Double`-huge → IAE *long*, oracle
`f-int-dsize2`, `f-int-ssize2`, `a-tail-hugedbl`), then:

- `int`/`long`/`float`/`double`: `Number` init →
  scalar fill with `intValue`/`longValue`/`floatValue`/
  `doubleValue` (narrowing, no checks); anything else →
  seq-fill truncated/padded with zeros (oracles
  `c-int-init`, `c-int-initseq`, `b-int-longseq`,
  `d-int-dblinit`, `d-int-nilinit`, `d-int-strinit`,
  `d-int-boolinit`, `b-long-scalar`, `b-float-scalar`,
  `b-double-scalar`).
- `byte`/`short`/`char`/`boolean`: exact-wrapper init
  (`Byte`/`Short`/`Character`/`Boolean` — in the port:
  that means *no* `Int` fill; `Int` inits take the seq
  path and fail IAE-seq) fills, anything else seq-fills
  (oracles `c-byte-init`, `b-byte-seq`,
  `b-byte-longseq`, `b-short-scalar`, `b-char-str2`,
  `b-char-num`, `b-bool-scalar1`, `b-bool-seq`,
  `c-char-init`, `c-bool-init`, `d-byte-fill`,
  `d-short-fill`, `d-bool-nilinit`).
- Element fill: ints/longs/floats/doubles/shorts/bytes
  via `xxxValue` narrowing/wrapping (oracles
  `b-byte-trunc`, `b-long-mixed`, `b-float-seq`,
  `b-short-wrap`, `b-int-wrap`, `b-long-huge`,
  `b-int-hugedbl`, `b-float-inf`, `b-byte-neg`,
  `e-short-huge`, `e-byte-huge`); chars/booleans via
  strict `(Character)`/`(Boolean)` casts (oracles
  `d-char-of-int`, `d-char-of-str`, `b-bool-seq`,
  `b-bool-nil`, `b-double-nil`, `b-int-bool`,
  `b-long-str`, `e-char-nil` — NPE texts verbatim).
- `object-array` is 1-arity only (`RT.object_array`:
  `Number` → size, else `RT.seq`; oracle
  `c-object-init`, `b-obj-size`, `b-obj-nil`, `oa-*`).

### `make-array` / `into-array` / `to-array` / `to-array-2d`

- `make-array` takes a class designator + dims. The port
  has no classes (S16): it accepts the nine keywords
  `:boolean :byte :char :short :int :long :float :double
  :object` (extension, see §8) and otherwise mirrors the
  JVM `^Class` hint — nil → NPE with empty message,
  anything else → CCE →`java.lang.Class` (oracles
  `e-nil-class`, `e-kw-class`, `e-num-class`). Dims go
  through core-`int` (port `int`: huge-`Long` → AE,
  huge-`Double` → IAE-*int*, `String` → CCE→`Character`,
  char → code, nil → NPE-charValue, oracles
  `e-bad-dim`, `e-extra-bad-dim`, `f-make-dlen`,
  `f-make-nillen`, `f-make-chardim`, `a-make-huge`).
  Multi-dim allocates rectangularly (oracles `e-int-2d-*`,
  `e-int-3d`, `e-obj-2d-nil`, `g-str-2d-inner`).
- `into-array` seqs its input (any seqable incl. lazy,
  oracle `c-lazy-*`, `c-int-lazy`, `c-obj-lazy`). 1-arity
  infers the component from the first element's class
  (nil/empty → `Object`), then checks every element —
  mismatch → IAE "array element type mismatch" (NOT
  `ArrayStoreException`; nil always fits; oracles
  `f-into-plain`, `f-into-mixed`, `f-into-empty`,
  `a-into-nil-elem`, `a-into-empty-cls`). 2-arity with
  an explicit type: `Integer`→core-`int`, `Byte`→`byte`,
  `Short`→`short`, `Float`→`float`, `Character`→`char`
  per element (the port natives, verbatim); `Long`/
  `Double`/`Boolean` and reference types use raw
  `Array.set` rules — exact-wrapper unbox + JLS widen
  (`char`→`long`/`double` ok; `Double`→`long`,
  `Integer`→`int` n/a, `Long`→`boolean` all "argument
  type mismatch"; nil into primitive → IAE empty
  message; oracles `a-int-*`, `a-long-*`, `a-byte-*`,
  `a-char-*`, `a-bool-*`, `a-double-*`, `a-float-*`,
  `a-short-*`, `a-double-char`, `c-long-nil`,
  `c-double-nil`, `c-long-double`, `c-float-str`,
  `c-short-str`, `c-short-char`, `c-int-empty-cls`,
  `a-into-huge`, `f-into-*`). The type arg follows the
  `make-array` keyword rule (oracle `f-into-kwtype`,
  `f-into-niltype`).
- `to-array`: nil → THE shared empty `Object[]`
  (`identical?` true, oracle `e-to-nil-ident`,
  `g-to-nil-len`); `Object[]` dims-1 → same handle
  (oracle `d-ident`); other arrays → boxed copy
  (`f-to-arr`, `d-prim-cls`); `String` → chars
  (`f-to-str`, `d-str-cls`); maps → entries (`f-to-map`,
  `d-map2`); vectors/lists/sets/seqs/`MapEntry`/PV →
  elements (`f-to-vec`, `f-to-list`, `d-range`,
  `d-seq`, `d-lazy`, `d-set`); anything else →
  `RuntimeException` "Unable to convert: class X to
  Object[]" with `jvm_class_of` (oracles `g-to2d-num`,
  `d-num`, `d-kw`).
- `to-array-2d`: `^Collection` hint — nil passes the
  hint then NPEs in `.size` (message "Cannot invoke
  \"java.util.Collection.size()\" because \"coll\" is
  null", oracle `g-to2d-nil`); non-`Collection`
  (maps, strings, arrays, scalars) → CCE
  →`java.util.Collection` (oracles `t2d-map`,
  `t2d-str`); otherwise `[to-array …]` per element
  (nil element → empty row; non-convertible →
  `RuntimeException`; oracles `f-to2d`,
  `f-to2d-ragged-nil`, `f-to2d-empty`, `g-to2d-str-elem`,
  `g-to2d-lens`).

### Access

`alength`/`aclone` (1-arity): nil → IAE "More than one
matching method found: {alength,aclone}"; non-array →
IAE "No matching method {…} found taking 1 args"
(reflector texts verbatim; oracles `g-clone-nonarray`,
`g-clone-nil`, `g-len-nonarray`, `g-len-nil`,
`g-len-str`). `aclone` is a shallow copy (fresh
identity, same descriptor; oracles `g-clone-eq`,
`g-clone-mut`, `g-clone-2d-shallow`, `g-clone-obj`,
`h-clone`).

`aget` 2-arity (direct, `RT.aget` overloads): nil →
"More than one matching method found: aget"; non-array
→ "No matching method aget found taking 2 args"
(oracles `a-nonarray`, `a-nil`, `a-str`); idx through
core-`int` (port `int`: `Double` 1.0 → 1, `String` →
CCE→`Character`, nil → NPE-charValue, huge → AE /
IAE-*int*; oracles `a-idx-type`, `a-idx-nil`,
`a-idx-str`, `a-idx-long-obj`, `a-arity1`,
`a-aget-huge`, `a-aget-hugedbl`); OOB → AIOOBE "Index
{i} out of bounds for length {n}" (oracles `a-neg-idx`,
`a-oob`, `a-oob-neg`).

`aget`/`aset` multi-index: the first step uses the
direct rule; every further step uses the raw rule —
nil array → NPE `getComponentType` text (`aget`, oracle
`a-nil-inner`) or NPE empty (`aset`, oracle
`b-aset-nilinner`); non-array → IAE "Argument is not an
array" (oracles `a-deep-nonarray`, `b-deep-nonarray`);
idx through checkCast+`intCast` (`Double` → `long` stop
first: huge → IAE-*long*, `NaN` → 0; non-`Number` →
CCE→`Number`; nil → NPE-charValue; oracles
`c-aget-tail-idx`, `c-aget-str-tail`, `b-aget-nidx`,
`b-aget-cidx`, `a-tail-hugedbl`, `a-tail-nan`).

`aset` direct 3-arity (`RT.aset` overloads, idx via
core-`int`, oracle `b-idx-type`): nil/non-array mirror
`aget`'s pair with `aset` names (oracles `b-nonarray`,
`b-nil`, `b-arity2`); OOB → AIOOBE "Index…" (oracle
`b-oob`); val matching per array type:
`int[]`←`Int`-toIntExact (AE past `i32`, oracle
`e-direct-int-huge`), `long[]`←`Int` exact,
`short[]`/`byte[]`←`Int` with `short`/`byte` range
checks (DIVERGENCE D2 — the JVM only matches the
`Short`/`Byte` wrappers and throws "No matching
method" for `Long` literals; the port erased the
wrappers, so it stores in-range values and range-errors
the rest; without this `(aset s 0 (short x))` could
never work), `char[]`←`Char`, `boolean[]`←`Bool`,
`double[]`←`Double`, `float[]`←`Double` (the
static-`double` rule: JVM literals resolve statically
and narrow, storing `Inf` past range — the port stores
the exact `f64` per its float posture; oracles
`e-float-double`, `e-direct-float-huge`), `Object[]`/
narrowed/`dims>1`←anything with the store check
(`ArrayStoreException` naming `jvm_class_of(val)`;
nil ok; oracles `b-obj`, `b-store-obj`, `b-store-2d`,
`e-obj-nil`, `e-obj-any`). Every other val →
"No matching method aset found taking 3 args" (oracles
`b-narrow`, `b-narrow-int`, `b-strval-prim`,
`b-nilval-prim`, `b-boolval-int`, `e-int-double`,
`e-double-long`, `e-float-double`(literal rule),
`e-char-long`, `e-bool-long`, `e-long-double`,
`a-short-byte`, `a-byte-short`, `a-int-bigint`,
`a-long-bigint`, `a-double-bigint`, `a-int-ratio`,
`a-double-bigdec`, `a-int-char`, `a-float-long`,
`a-double-intcall`; widen successes `a-int-short`,
`a-int-byte`, `a-long-int`, `a-long-short`,
`a-double-float`, `e-int-intobj`, `e-long-long`,
`e-double-double`, `e-float-float`, `e-byte-byte`,
`e-short-short` all store via the port rules above).
Returns the original val.

`aset` tail stores use strict `Array.set` (exact-wrapper
unbox + widen — the port reads `Int` as JVM `Long`:
`int[]`←`Int` THROWS "argument type mismatch" while
`long[]`←`Int` stores; oracles `b-2d`,
`c-aset-int2d-f`, `c-aset-obj2d`, `c-aset-long-tail`,
`e-tail-int-long`, `e-tail-long-long`,
`e-tail-int-int`, `c-aset-tail-idx`).

`aset-*` (def-aset): val through the matching core
coercion (port natives verbatim, oracles `c-int-str`,
`c-int-nil`, `c-byte-str`, `c-float-str`,
`c-short-nil`, `c-char-str`, `c-bool`, `c-char`,
`c-float`, `c-double`, `c-short`, `c-byte`,
`a-setint-huge`, `a-setlong-huge`); idx through
checkCast+`intCast` (oracles `b-setint-didx`,
`b-setint-sidx`, `b-setint-nidx`); non-array → IAE
"Argument is not an array", nil array → NPE empty
(oracles `e-setint-vec`, `e-setint-nilarr`);
non-primitive array (`Object[]`, any `dims>1`) → IAE
"Argument is not an array of primitive type" (oracles
`c-int-objarr`, `d-setbool-obj`, `b-setint-2d`,
`b-setbool-2d`); then the JLS `setX` widening table
(`setInt`→int/long/float/double,
`setLong`→long/float/double, `setShort`→short↑,
`setByte`→byte↑, `setChar`→char/int/long/float/double,
`setFloat`→float/double, `setDouble`→double,
`setBoolean`→boolean; miss → "argument type mismatch";
oracles `c-int`, `c-int-2d`, `c-long`, `c-int-wrongarr`,
`c-long-wrongarr`, `c-bool-intarr`, `d-setint-*`,
`d-setlong-*`, `d-setshort-*`, `d-setbyte-*`,
`d-sethcar-int`, `d-setfloat-*`, `d-setdouble-float`,
`d-setbool-bool`); OOB → AIOOBE empty message
(oracles `c-int-oob`, `c-double-oob`). Multi-arity
recurses like `aset` (oracle `c-int-2d`). Returns the
original val.

Coercions (`booleans`/`bytes`/`chars`/`shorts`/`ints`/
`longs`/`floats`/`doubles`): nil → nil; exact
descriptor (`dims` = 1, matching base) → same handle;
else CCE with full module text — arrays count as
`java.base`/`bootstrap` (fix `module_of` for `[`;
oracles `d-*`, `h-ints-2d`, `h-ints-clone-same`,
`h-bytes-clone-same`).

`bytes?`: 1-arity; true iff dims-1 `byte[]`, false for
nil and everything else (oracle `e-bytes-*`).

### Seq/count/nth/get/find/contains?/empty

- `seq` (snapshot, nil when empty; oracles `a-seq`,
  `a-seq-empty`, `a-seq-obj`, `a-seq-2d`), `first`/
  `rest`/`next`/`map`/`reduce`/`nthrest`/`last` ride it
  (oracles `a-first`…`a-last`). `count` answers the
  length (oracle `a-count`). `nth` coerces `Number`
  keys (`Double` 1.5 → 1, `BigInt` too; `String` →
  CCE→`Number`; nil → NPE-charValue — the checkCast+
  `intCast` rule; oracles `a-nth`, `a-nth-nf`,
  `e-nth-float`, `n-arr-str`, `n-arr-nil`), answers
  not-found on OOB, else AIOOBE empty message (oracle
  `a-nth-oob`).
- `get` coerces `Number` keys (`Double` 1.5 → 1;
  non-`Number`/nil key → nil — lenient; OOB →
  not-found/nil; oracles `b-get`, `b-get-nf`,
  `f-get-str`, `f-get-nil`, `f-get-neg`, `f-get-obj`,
  `e-get-float`, `e-get-bigint`).
- `contains?`: `Number` key → bounds check; anything
  else → IAE "contains? not supported on type: {desc}"
  (oracles `b-contains`, `f-contains-oob`,
  `f-contains-str`, `f-contains-neg`,
  `e-contains-nil`).
- `find` → IAE "find not supported on type: {desc}"
  (oracle `b-find`). `empty` → nil (oracle `b-empty`).
  `conj` → CCE →`IPersistentCollection`, `assoc` →
  CCE →`Associative`, `rseq` → CCE →`Reversible`,
  `with-meta` → CCE →`IObj`, `compare` →
  CCE →`Comparable` (oracles `b-conj`, `b-assoc`,
  `b-rseq`, `b-with-meta`, `b-compare`,
  `b-compare-vec`). `meta` → nil.
- Predicates: only `seqable?` is true for arrays;
  `counted?`/`sequential?`/`indexed?`/`reversible?`/
  `coll?`/`vector?`/`list?`/`associative?`/`ifn?` are
  false (oracles `b-counted?`…`b-list?`, `f-arr-assoc`,
  `f-arr-ifn`).

### Print / `str`

- pr: `#object["{desc}" 0x{id:x} {desc}@{id:x}]`
  (class-name quoted, rep bare — oracle `d-pr-*`,
  `d-pr-in-vec`; the quoting asymmetry is pinned, the
  JVM mechanism behind it is not reproduced).
- print (`readably` false): both bare (oracle
  `d-print-str-obj`). `str`: `{desc}@{id:x}` (oracle
  `d-str-int`). `id` is the port identity (atoms print
  the same way).
- PV prints exactly like a vector incl. `*print-length*`
  and `^meta` (`f-ctor`, `f-empty`, `c-vo-meta`).

### `jvm_class_of` and error classes

Extend `jvm_class_of` (`error.rs`) with descriptors:
`[Z [B [C [S [I [J [F [D [L…;` (`[[I`, …) and
`clojure.core.Vec`. New `ThrowClass` variants:
`ArrayIndexOutOfBoundsException` (→`IOOBE`),
`ArrayStoreException` (→`RuntimeException`),
`NegativeArraySizeException` (→`RuntimeException`),
`OutOfMemoryError` (→`VirtualMachineError`→`Error`),
`AbstractMethodError` (→`IncompatibleClassChangeError`
→`LinkageError`→`Error`), with `throw_parent` links
and `throw_class_for` arms (S7-table format). The
`catch` name table already lists every name.

## Keywords (§8 extension) and the S16 seam

`make-array`/`into-array` accept the nine keywords
above; the JVM takes `Class` objects the port cannot
name yet (S16 class emulation). S16 extends the
accepted designators to class values and adds
narrowed-component `make-array` (`String[]`, …);
keyword pins stay green. Likewise `type`/`class`/
`instance?`/`..` stay out-of-scope: S15 pins
descriptors through `jvm_class_of` (error texts) and
print shapes only; S16 keeps those pins green. No
existing kind's `=`/`hash`/print changes (full suites
stay green unmodified except new tests).

## `amap` / `areduce`

Verbatim ports of the JVM `core.clj` macros except
`unchecked-inc`/`unchecked-inc-int` → `inc` (those
vars do not exist in the port; the port's own
`dotimes` already substitutes `inc` the same way;
array indices cannot overflow in practice).
Expansions pinned against oracle `e-expand-amap`/
`e-expand-areduce` modulo that substitution and
gensym numbering. Behavior oracles `e-amap`,
`e-amap-long`, `e-amap-cls`, `e-amap-orig`,
`e-areduce`, `e-areduce2`, `e-amap-nonarray`.

## Spike verdict

- Multi-dim: the nested `Arrays(Vec<Option<Array>>)`
  design carries rectangular allocation, nil inners,
  ragged rows, depth-n recursion, and descriptor store
  checks with no special cases beyond the descriptor.
  VALIDATED (oracles `e-*`, `a-2d`, `a-3d`, `c-*`,
  `g-clone-2d-shallow`, `b-store-2d`).
- Store/load typing: every matrix decoded end to end —
  `Numbers` narrowing ctors vs `RT` checked paths vs
  `Array.set` unbox rules vs reflector overload
  matching (`paramArgTypeMatch` + `widenBoxedArgs` +
  static-literal types + single-match leniency +
  `checkCast`+`intCast` unboxing), with the residual
  divergences D1–D4 enumerated below. VALIDATED.
- `vector-of`: own immutable kind, not `Array`-backed
  (persistence vs fixed-mutability are irreconcilable);
  the `Vec` parity surface (invoke gate included) is
  fully pinned. VALIDATED.

Divergences (all pinned as port behavior in
`tests/arrays.rs` with oracle notes): D1 `seq`
snapshot (above); D2 short/byte direct-`aset` stores
(above); D3 `float` storage is `f64` (out-of-`f32`
magnitudes stay finite instead of `Inf`, oracle
`b-float-inf`, `e-direct-float-huge`); D4 nil arrays
always take the stable "More than one matching
method" rule — the JVM's inner/runtime-nil NPEs
depend on HotSpot method order and vary even within
one JVM (oracles `e-aset-nil-multi` vs `g-nil-*`;
unmirrorable, unpinnable). Narrow reads collapse to
`Int`/`Double` (port-wide posture, not a divergence).

## Tests

`rust/crates/clj-eval/tests/arrays.rs`: one test per
var replaying its probe labels verbatim (oracle label
→ port assertion, quoting the `.out` line), plus
port-only pins (D1–D4, keyword designators, OOM
guard). Matrix `notes` name the tests.

## Non-goals

`type`/`class`/`instance?`/`..` (S16), `unchecked-*`
arithmetic (whoever claims it), lazy array-seqs (D1),
explicit narrowed `make-array` (S16), `*unchecked-math*`
honoring (pre-existing port posture).
