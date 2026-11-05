# S5D collection pins (coll probes + sources)

Oracle batches `/tmp/s5d-gapscoll{,2,3,4}.clj` + `s5d-gapscoll{5,6,7,8}.clj`
(`c`/`c2`-`c8`), JVM `core.clj` + `RT`/`APersistent*`/`Persistent*`
sources. Locale `en_US`, zone `UTC`.

## Vector assoc/get/contains (`RT`/probed)

- `assoc` on vectors: key must satisfy `Util.isInteger`
  (`Long`/`Integer`/`BigInt`/`BigInteger` — `Double` never, even
  `1.0`); else `IAE "Key must be integer"`. Index is `intValue`
  (wraps); at-count appends; anything else is `IOOBE` with an
  empty message.
- `get` on vectors nil-puns bad keys (`(get [1 2] :a)` → `nil`,
  `(get [1 2] 1.0)` → `nil`); OOB is `nil`/`not-found`.
  `contains?` on vectors is `false` for bad keys (never throws).
- `assoc` with odd varargs: `IAE "assoc expects even number of
  arguments after map/vector, found odd number"` (NOT the
  `No value supplied` text — that one is `array-map`/`hash-map`
  only). `assoc!` with a trailing key silently assocs `nil`.

## `get`/`contains?`/`find` (`RT.get`/`contains`/`find`)

- `get` on strings is bounds-checked (`OOB` → `nil`/`not-found`,
  never throws); non-`Number` keys answer `nil`/`not-found`.
- `contains?` on a string with a non-`Number` key throws `IAE
  "contains? not supported on type: java.lang.String"` (falls
  through the `Number` gate); otherwise `IAE "contains? not
  supported on type: <full class>"` for lists/seqs/numbers/etc.
- `find` is 2-arity only; misses are `nil`; anything but
  nil/Associative/transient-assoc is `IAE "find not supported
  on type: <full class>"`. Entry keys read back as passed
  (`(find [1 2] 0)` → `[0 1]`).
- `get`/`find` on transient vectors nil-pun bad keys like the
  persistent ones; transient sets answer `get` (`ILookup`) but
  not `find`.

## `nth` (`RT.nth`, index coerces first)

- `(nth [1 2] 5)` → `IOOBE` empty (bounds-checked, NOT `AIOOBE`);
  `:nf` saves everything except unsupported types.
- Strings: `SIOOBE "Index N out of bounds for length L"` (JDK
  text, UTF-16 length).
- Huge indices: `intCast` checks — `Long` overflow is
  `ArithmeticException "integer overflow"`, huge `Double` is `IAE
  "Value out of range for long: <java-double>"`.
- Unsupported transient maps/sets: `UOE "nth not supported on
  this type: TransientArrayMap"` (Java `getSimpleName` — member
  classes strip the owner, unlike `simple_class_of`).

## `conj` (`APersistentMap.cons` shapes)

- `(conj nil 1 2 3)` → `(3 2 1)` (plain reduce, NOT `list*`).
- Map + vector requires exactly 2 (`IAE "Vector arg to map conj
  must be a pair"`); map + seqable loops `assoc` over `RT.seq`
  casting each element to `Map.Entry` (`(conj {} (seq [[1 2]]))`
  → `CCE PersistentVector→java.util.Map$Entry`); empty seqs and
  nil pass the map through.
- Map + map assocs every pair (no cast involved).
- `conj` 1-arity passes anything through (`(conj 5)` → `5`).

## `merge`/`merge-with`/`into` (`core.clj` shapes)

- `merge` is `into`-shaped: `([x y] (if (and x y) (into x y)
  (or x y)))`, left-folded (`(merge {:a 1} [:b 2])` → `IAE`
  via `conj` of `:b`; `(merge nil {:a 1})` → `{:a 1}`).
- `merge-with` 1-arity → `nil`, 2-arity → `x`; maps AND vectors
  (incl. subvectors) take the `kvreduce` path (vector keys are
  indices); anything else seqs and `key`/`val`s its elements
  (`CCE →Map$Entry` on the first).
- `into` is `reduce conj` (`(into nil [1 2])` → `(2 1)`); the
  transient/kvreduce fast paths are unobservable.

## `peek`/`pop` (`RT.peek`/`RT.pop`)

- Both nil-pun (`nil` → `nil`); only lists/vectors/subvectors/
  entries are stacks (`peek` reads the val, `pop` answers the
  1-vector — T1192 FX2) — every seq value (`LazySeq`, `Cons`, ranges,
  repeats, cycles, iterates, chunked/string/entry seqs),
  strings, maps, sets, and transients are `CCE
  →IPersistentStack`.
- `(pop '())` → `ISE "Can't pop empty list"`, `(pop [])` →
  `ISE "Can't pop empty vector"`; list pops keep the tail's own
  metadata (reaching empty restores the popped list's).

## Subvectors (`RT.subvec` + `APersistentVector$SubVector`)

- `start == end` answers plain `PersistentVector.EMPTY` (never a
  subvector); metadata is dropped (ctor gets `null`); nested
  windows flatten; bad int coercions match `nth` exactly
  (nil-`NPE`, `CCE →Number`).
- `conj`/`assoc` stay subvectors (`assoc` at `end` appends;
  negative indices write the backing at the absolute index,
  silently when before the window); `pop` to one element and
  `empty` fall back to plain vectors (popping to empty drops
  the metadata).
- `(seq sv)` is `APersistentVector$Seq`: counted but NOT
  `IDrop` (so `drop` over subvectors is lazy); subvectors
  invoke like vectors except the arity error names
  `APersistentVector/SubVector` (slash).
- `replace` over a subvector and `vec` of a subvector stay
  subvectors (`vec` still strips the metadata).

## `vec`/`empty`/`select-keys`/`zipmap`/`array-map`

- `vec` of a vector strips the metadata and keeps the value
  (subvectors stay subvectors); everything else realizes into
  a plain `PersistentVector` (no `LazilyPersistentVector`
  anywhere in modern Clojure).
- `empty` keeps the metadata (`.empty()` re-tags); non-collections
  answer `nil` (strings included).
- `select-keys` works on any `get`-able (`(select-keys [1 2]
  [0])` → `{0 1}`) and restores the input map's metadata.
- `zipmap` seqs keys before vals, stops at the shorter side,
  interleaves (never over-realizes an infinite side).
- `array-map` never promotes (forced array class); duplicate
  keys: last value wins at the first position.

## Transients (probed)

- `(transient x)` on lists/strings/numbers/transients → `CCE
  →IEditableCollection`; on `nil` → `NPE asTransient "coll"`.
- `conj!` 0-arity → empty transient vector, 1-arity passes
  through; `assoc!` needs 3+ args; `dissoc!`/`pop!` hint
  `ITransientMap`/`ITransientVector` (`CCE` on the wrong
  transient kind); `conj!` on a transient map takes pairs like
  the persistent one.
- `nil` bangs name the hinted method + local (`conj`,
  `assoc`, `without`/`"map"`, `pop`); use after `persistent!`
  (incl. `count` and second `persistent!`) is always
  `IllegalAccessError "Transient used after persistent! call"`.
- Metadata round-trips (`persistent!` restores, `meta` of a
  transient is `nil`).

## Arity errors (probed `c3`)

- `defn` fns report bare `clojure.core/name` (no suffix).
- Bootstrap `def`+`fn*` fns (`assoc`, `first`, `second`,
  `cons`, `seq`, `rest`, `next`, `last`) report
  `clojure.core/name--NNNN` (build-specific counter —
  stripped here, documented divergence).
- `count` arity-mismatches fail at COMPILE time
  (`--inliner--NNNN` suffix — phase divergence here: runtime
  `ArityException` instead).
- Set invoke takes 1–2 args (get/get-or-not-found); map invoke
  1–2; vector/subvector exactly 1.

## `counted?` per type (probed `c4`/`c7`)

True: collections, subvectors, transients, long ranges,
`seq` of vector/string/small-map, sort results.
False: `nil`, strings, double ranges, repeats (even finite),
cycles, iterates, `keys`/`vals`, conses (`Cons` is never
`Counted`, not even over a vector — fixprobe7 `:cc-cons`
reverses the old `c4` read).

## `drop` strictness (sources + `c5`/`c6`)

`IDrop` implementors: `LongRange`, `PersistentArrayMap` (+ its
`Seq`), `PersistentVector` (+ `ChunkedSeq`), `Repeat`,
`StringSeq`. `IDrop` inputs step strictly (`(or … ())`,
`n ≤ 0` answers `(seq coll)`); everything else (incl.
subvectors, lists, strings, hash maps, `APersistentVector$Seq`)
is `(lazy-seq …)` — including at `n ≤ 0`.

## `count` (probed)

`count` of strings is the UTF-16 length (`(count "😀")` → `2`);
transients count via `Counted`; anything else uncountable is
`UOE "count not supported on this type: <simple>"`.

## Known divergences (no `MapEntry` type, contract-blessed)

- Map seqs/`find` yield `[k v]` vectors (class `Vector`, NOT
  `MapEntry`); `conj` map+map merges entry-wise (same values);
  `conj`/`merge-with` over seqs-of-pairs `CCE →Map$Entry`
  where true entry seqs would assoc (rare; documented).
- Astral strings: `seq`/`get`/`nth` index Unicode scalars
  (lone surrogates are unrepresentable as `Char`); `count`
  and `contains?`-bounds use UTF-16 units.
- Vector-family `assoc`/`conj!` wrap huge keys through `i32`
  (`intValue`); `nth`/`subvec` check them (`intCast`).
