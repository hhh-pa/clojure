# Zero-oos S12: collections misc — Specification

- Status: spec + oracle probes written; no source edits.
  Awaiting owner review (leave uncommitted).
- Parent: out-of-scope reduction. Issue: #54.
  Branch: `zero-oos-s12-collmisc`.
- Probes: `docs/zero-oos-s12-collmisc/probes/`
  `{structmaps,chunks,mixhash,seque,iteration,fileseq,rematcher}.{clj,out}`
  (203 pins, every file exit 0 and rerun-stable; one redundant dissoc-class probe dropped at owner review).

## Problem

Twenty `clojure.core` vars do not resolve: the structmap
surface (`accessor`, `create-struct`, `defstruct`, `struct`,
`struct-map`), the chunk machinery (`chunk`, `chunk-append`,
`chunk-buffer`, `chunk-cons`, `chunk-first`, `chunk-next`,
`chunk-rest`, `chunked-seq?`), the hashing helpers
(`hash-combine`, `mix-collection-hash`), `seque`, `iteration`,
`file-seq`, and the matcher pair (`re-groups`, `re-matcher`).

## Solution

Oracle-pin the JVM behavior first (this spec), then implement:
structmaps as a `HostType` kind with map-identical `=`/hash/print
arms; an emulated single-use chunk buffer + chunk + chunked cons;
`hash-combine`/`mix-collection-hash` natives over the `hasheq.rs`
Murmur3 core (disjoint symbols from S10's wrappers); `seque` and
`iteration` as `ExtensionValue`s; `file-seq` as a lazy string-path
walk (documented divergence from `java.io.File`); a stateful
regex-crate matcher handle with find-then-groups sequencing.

## Part 1 — Normative contract

### 1.1 Conventions

- A probe id is `<prefix>-<name>` where the prefix names the file:
  `sm` structmaps, `ch` chunks, `mh` mixhash, `it` iteration,
  `sq` seque, `fs` fileseq, `rm` rematcher.
- `THROW C|"m"` quotes the `.out` throw line verbatim (root cause).
- `(derived)` marks a port-test assertion derived from an oracle
  pin rather than quoted by it.
- Class pins transfer without `class` (which is N1
  out-of-scope in the port): a JVM `(class x)` pin becomes a
  `jvm_class_of` mapping, observable through CCE from-names,
  ISeq error text, and `#object` print heads.

### 1.2 Structmaps

`defstruct` is a macro; the other four are fns. A basis object
(`create-struct`) is an identity-compared descriptor; an instance
is map-identical in print, `=`, and `hash`.

| Label | Contract |
| --- | --- |
| sm-basis-class | basis class is `PersistentStructMap$Def` |
| sm-basis-print | basis prints `#object[…$Def 0xH "…$Def@L"]` |
| sm-defstruct-expand | expands to `(def n (clojure.core/create-struct …))` |
| sm-defstruct-class | `defstruct` value is a `$Def` basis |
| sm-basis-eq | two identical bases are NOT `=` (identity) |
| sm-make-print | instance prints exactly like the plain map |
| sm-make-class | instance class is `PersistentStructMap` |
| sm-map? | `map?` is true |
| sm-eq-map | instance `=` the plain map with same entries |
| sm-hash-eq | `(hash instance)` `=` `(hash plain-map)` |
| sm-keys | keys in basis order, nil-defaults included |
| sm-vals | vals with nil for unset basis keys |
| sm-seq | seq of entries, nil-defaults included |
| sm-count | count includes nil-defaulted basis keys |
| sm-get-basis | unset basis key reads nil |
| sm-contains | unset basis key is `contains?` true |
| sm-extra | extra keys allowed, print after basis keys |
| sm-empty | no inits: all basis keys present as nil |
| sm-cross-basis | instances over different bases compare by map |
| sm-struct-print | positional ctor prints like the plain map |
| sm-struct-short | short positionals default the tail to nil |
| sm-struct-extra | THROW IAE `"Too many arguments to struct constructor"` |
| sm-assoc-print | assoc prints map-identical |
| sm-assoc-class | assoc of a basis key stays a structmap |
| sm-assoc-extra | assoc of an extra key prints map-identical |
| sm-assoc-extra-class | assoc of an extra key stays a structmap |
| sm-dissoc-basis | THROW RuntimeException `"Can't remove struct key"` |
| sm-dissoc-extra | dissoc of an extra key removes it |
| sm-dissoc-extra-class | dissoc of an extra key stays a structmap |
| sm-accessor | applied accessor reads the basis key |
| sm-accessor-miss | THROW IAE `"Not a key of struct"` |
| sm-accessor-plain | THROW CCE ArrayMap → StructMap |
| sm-accessor-fn? | accessor answers false to `fn?` (custom IFn) |
| sm-accessor-class | accessor class is `PersistentStructMap$1` |
| sm-accessor-print | accessor prints `#object[…$1 0xH "…$1@L"]` |
| sm-accessor-arity0 | THROW ArityException `(0)` passed to `…PersistentStructMap/1` |
| sm-accessor-arity2 | THROW ArityException `(2)` passed to `…PersistentStructMap/1` |
| sm-accessor-nil | THROW NPE `"Cannot read field \"def\" because \"m\" is null"` |
| sm-accessor-mismatch | THROW RuntimeException `"Accessor/struct mismatch"` (def identity, not key-set: same-keys other-basis throws) |
| sm-empty-basis | THROW IAE `"Must supply keys"` (zero keys) |
| sm-map-nonbasis | THROW CCE ArrayMap → `$Def` |
| sm-struct-nonbasis | THROW CCE String → `$Def` (cross-module shape) |
| sm-accessor-nonbasis | THROW CCE ArrayMap → `$Def` |

Derived port tests: `(chunk-first (struct-map …))` throws CCE
from `clojure.lang.PersistentStructMap` (pins sm-make-class
without `class`); `(chunk-first basis)` throws CCE from
`…$Def`; `(seq basis)` throws the ISeq error naming `…$Def`.

### 1.3 Chunks

Buffers are single-use builders: `chunk` takes the array and
kills the buffer. Chunks are indexed/counted but not seqable;
`nth` reads the raw capacity-sized array (nil holes, no
bounds check below capacity).

| Label | Contract |
| --- | --- |
| ch-buf-class | buffer class is `ChunkBuffer` |
| ch-buf-print | buffer prints `#object[ChunkBuffer 0xH …]` |
| ch-chunk-class | chunk class is `ArrayChunk` |
| ch-build | append/append/count/nth roundtrip `[2 :a :b]` |
| ch-print | chunk prints `#object[ArrayChunk 0xH …]` |
| ch-empty-count | empty chunk counts 0 |
| ch-nil | nil appends count and read back nil |
| ch-overflow | THROW AIOOBE `"Index 2 out of bounds for length 2"` |
| ch-bigcap | capacity 64 holds 40 items (no 32 ceiling) |
| ch-zero-cap | zero-capacity chunk counts 0 |
| ch-zero-append | THROW AIOOBE `"Index 0 out of bounds for length 0"` |
| ch-neg-cap | THROW NegativeArraySizeException `"-1"` |
| ch-seq | THROW IAE `"Don't know how to create ISeq from: …ArrayChunk"` |
| ch-nth-oob | nth past count, under capacity, is nil |
| ch-nth-cap | nth at capacity-1 hole is nil |
| ch-nth-cap-oob | THROW AIOOBE `"Index 32 out of bounds for length 32"` |
| ch-append-ret | append returns nil; chunk then counts 1 |
| ch-use-once | THROW NPE `"… \"this.buffer\" is null"` (append after chunk) |
| ch-chunk-twice | 2nd chunk is degenerate; count THROWs NPE `"… \"this.array\" is null"` |
| ch-cons-empty | empty chunk + rest returns rest identically |
| ch-cons-print | chunk-cons prints as its seq `(1 2 3)` |
| ch-cons-class | class is `ChunkedCons` |
| ch-cons-nil-print | nil rest prints `(1)` |
| ch-first-vec | vector-chunk head block holds 32 |
| ch-first-small | small head block count + nths `[3 1 3]` |
| ch-rest-vec | rest past the head 32 prints `(32 … 39)` |
| ch-next-vec | next past the head 32 prints `(32 … 39)` |
| ch-rest-end | rest at end is `()` and non-nil |
| ch-next-end | next at end is nil |
| ch-first-list | THROW CCE PersistentList → IChunkedSeq |
| ch-first-str | THROW CCE StringSeq → IChunkedSeq |
| ch-rest-cons | rest of a chunk-cons is the tail seq |
| ch-next-cons-end | next of a tailless chunk-cons is nil |
| ch-first-cons | first of a chunk-cons is its chunk |

`chunked-seq?` battery (one pin each; `true` rows first):

| Kind | `chunked-seq?` |
| --- | --- |
| vector seq | true (ch-cq-vec) |
| bounded range seq | true (ch-cq-range) |
| chunk-cons result | true (ch-cq-cons-chunk) |
| take-last of lazy | true (ch-cq-takelast) |
| drop of chunked | true (ch-cq-drop) |
| list/map/set/string seq | false (ch-cq-list/map/set/str) |
| repeat/cycle/iterate seq | false (ch-cq-repeat/cycle/iterate) |
| lazy/cons/concat/map seq | false (ch-cq-lazy/cons/concat/mapfn) |
| nil, vector value | false (ch-cq-nil/vecval) |
| rseq, subvec seq | false (ch-cq-rseq/subvec) |
| re-seq, tree-seq | false (ch-cq-reseq/treeseq) |
| `(range)`, `(repeat 3 :x)` | false (ch-cq-range-inf/repeat-n) |
| eduction seq | false (ch-cq-eduction) |
| seque, iteration seq, file-seq | false (sq-cq, it-cq, fs-cq) |

Port rule: true exactly for a bounded `RangeValue`, a
`RealizedSeq` over a `SeqKind::Chunked`/`SeqKind::ChunkedCons` head, or
the emulated chunked cons. `chunk-first`/`chunk-rest`/
`chunk-next` accept those same three shapes; anything else
throws CCE to `clojure.lang.IChunkedSeq`.

### 1.4 Hash combine and mix

`mix-collection-hash` is `Murmur3.mixCollHash` over int-checked
longs. `hash-combine` is `Util.hashCombine(seed, Util.hash(y))`
where `Util.hash` is the null-safe Java `.hashCode` — never
murmur (mh-util-spot, mh-comb-formula). Port: `hash_combine`
over `hash_code(y)` for every kind.

| Label | Contract |
| --- | --- |
| mh-mix-1-0 | `(mix 1 0)` → -2017569654 |
| mh-mix-0-0 | `(mix 0 0)` → -15128758 |
| mh-mix-1-1 | `(mix 1 1)` → 1978132887 |
| mh-mix-1-2 | `(mix 1 2)` → 1734656966 |
| mh-mix-neg | `(mix -1 3)` → -196466786 |
| mh-mix-big | `(mix 123456789 10)` → 1485565228 |
| mh-mix-max | THROW ArithmeticException `"integer overflow"` |
| mh-mix-vec | `(mix 1 0)` `=` `(hash [])` |
| mh-mix-set | `(mix 0 0)` `=` `(hash #{})` |
| mh-mix-arity | THROW ArityException (1 arg) |
| mh-mix-str | THROW CCE String → Number |
| mh-mix-double | 1.9 truncates to 1 |
| mh-comb-formula | equals `Util/hashCombine` over `Util/hash` |
| mh-util-spot | `Util/hash` of `[[1 2] :a {:a 1} nil]` → `[994 1013910569 1013910568 0]` |
| mh-comb-0-nil | → -1640531527 |
| mh-comb-0-0 | → -1640531527 |
| mh-comb-0-1 | → -1640531526 |
| mh-comb-1-1 | → -1640531461 |
| mh-comb-0-str | → -1640531430 |
| mh-comb-0-kw | → -626620958 |
| mh-comb-5-kw | → -626620634 |
| mh-comb-0-vec | → -1640530533 |
| mh-comb-0-map | → -626620959 |
| mh-comb-0-set | → -1640531526 |
| mh-comb-list | `(hash-combine 0 '(1 2))` → -1640530533 |
| mh-comb-neg | `(-1 "ab")` → 1640528486 |
| mh-comb-big | `(12345 nil)` → -1639734210 |
| mh-comb-double | `(0 1.5)` → -567313991 |
| mh-comb-negzero | `(0 -0.0)` → 506952121 (hashCode path) |
| mh-comb-bigint | `(0 Long/MAX)` → 506952121 (hashCode path) |
| mh-comb-char | `(0 \a)` → -1640531430 |
| mh-comb-bool | `(0 true)` → -1640530296 |
| mh-comb-seed-str | THROW CCE String → Number |
| mh-comb-bigseed | THROW ArithmeticException `"integer overflow"` (2^40) |
| mh-comb-arity | THROW ArityException (1 arg) |
| mh-tower-hash | `(hash [5N 1/2 1.5M])` → `[1740791543 3 466]` |
| mh-tower-util | `Util/hash` of same → `[5 3 466]` |
| mh-comb-bign | `(0 5N)` → -1640531522 (JVM magic; port diverges, §2.4) |
| mh-comb-dec | `(0 1.5M)` → -1640531061 (JVM magic; port diverges, §2.4) |
| mh-comb-ratio | `(0 1/2)` → -1640531524 (JVM magic; port diverges, §2.4) |
| mh-tower-agree | `(hash 5)` `=` `(hash 5N)` (equiv-consistent) |

### 1.5 Seque

`(seque s)` is `(seque 100 s)`; `n-or-q` is an int capacity
or a queue. Values, order, and nil-safety are pinned; feeder
timing and producer-error propagation are racy by design and
are NOT pinned (§2.5).

| Label | Contract |
| --- | --- |
| sq-basic | values in order `(1 2 3)` |
| sq-class | result class is `LazySeq` |
| sq-cq | `chunked-seq?` is false |
| sq-nil-elems | nil elements survive `(1 nil 3)` |
| sq-empty | empty input drains to `()` |
| sq-empty-seq | empty input seqs to nil |
| sq-nil-input | nil input drains to `()` |
| sq-cap | explicit capacity 2 drains 10 items |
| sq-queue | explicit queue drains (JVM reference; unrepresentable in port — no queue kind) |
| sq-zero-cap | THROW IAE `""` (empty message) |
| sq-neg-cap | THROW IAE `""` (empty message) |
| sq-cap-str | THROW CCE String → Character (int-cast path) |
| sq-cap-float | 2.9 truncates to capacity 2 |
| sq-take | partial consume `(0 1 2)` |
| sq-count | full count 50 |
| sq-inf | bounded take off an infinite producer |

### 1.6 Iteration

`iteration` builds a seqable/reducible stepper. Defaults:
`:somef some?`, `:vf identity`, `:kf identity`, `:initk nil`.
The seq path steps eagerly once, then lazily per cell; the
reduce path loops with `reduced` short-circuit.

| Label | Contract |
| --- | --- |
| it-basic | default opts walk `(1 2 3)` |
| it-take-inf | unbounded step takes `(1 2 3)` |
| it-empty | failing first `somef` seqs to nil |
| it-reduce-empty | failing first `somef` reduces to init |
| it-reduce | reduce sums to 6 |
| it-reduced | `reduced` short-circuits to 1 |
| it-vf | `:vf`/`:kf` pagination gives `([0] [1] [2])` |
| it-kf-stop | nil `:kf` stops after emitting `(3 2 1 0)` |
| it-somef | custom `:somef` stops at `(1 2)` |
| it-somef-skips | failing `somef` never calls `:vf` |
| it-seqable | result is `Seqable` |
| it-reducible | result is `IReduceInit` |
| it-cq | `chunked-seq?` of its seq is false |
| it-step-once | `seq` steps exactly once, eagerly |
| it-take-count | taking 2 steps exactly twice |
| it-twice | each consumption re-steps from `:initk` |
| it-reduce-count | reduce steps through the failing ret |
| it-unknown-opt | unknown opts are ignored |

### 1.7 File-seq

DECISION (spike resolved, §2.2): the port takes a STRING dir
and yields STRING paths — no `File` kind. JVM pins below are
the reference; starred rows are deliberate divergences.

| Label | Contract |
| --- | --- |
| fs-classes | JVM yields `java.io.File` only |
| fs-one | JVM head prints `#object[File 0xH "<ROOT>"]` |
| fs-count | 7-node fixture walks 7 entries |
| fs-names | sorted names match the fixture set |
| fs-root-first | root itself is first |
| fs-cq | `chunked-seq?` is false |
| fs-str-arg (*) | JVM THROWs CCE String → File; port ACCEPTS strings |
| fs-missing | missing root walks a singleton (no throw) |
| fs-file-root | file root walks a singleton |
| fs-empty-dir | empty dir walks a singleton |
| fs-seq? | result is a seq |

Port contract: `(file-seq dir-string)` is a lazy seq of path
strings, root first, children as root-joined paths without
canonicalization; non-dir and missing roots yield the
singleton `[dir-string]`; order is filesystem order (tests
sort, mirroring the JVM `listFiles` caveat); non-string args
throw IAE `Cannot open <x> as a directory.` (slurp-shaped).

### 1.8 Re-matcher and re-groups

`re-matcher` answers a stateful handle; `re-find` of one arg
advances it and returns the groups shape; `re-groups` reads
the last match. No match is `IllegalStateException`.

| Label | Contract |
| --- | --- |
| rm-class | handle class is `java.util.regex.Matcher` |
| rm-print | handle prints `#object[Matcher 0xH "…[pattern=a+ region=0,4 …]"]` |
| rm-find-groups | find then groups share `[full g1 g2]` |
| rm-find-plain | groupless find then groups share `"aa"` |
| rm-find-none | find with no match is nil |
| rm-groups-nomatch | THROW IllegalStateException `"No match found"` |
| rm-groups-fresh | THROW IllegalStateException `"No match found"` |
| rm-multi | successive finds advance `["a" "a" nil]` |
| rm-groups-each | groups track each find; nil holes kept |
| rm-exhaust | exhausted handle stays nil |
| rm-empty-advance | empty matches advance `["" "" nil]` |
| rm-bad-pattern | THROW CCE String → Pattern |
| rm-bad-subject | THROW CCE Long → CharSequence |
| rm-nil-subject | THROW NPE (CharSequence.length help text) |
| rm-groups-bad | THROW CCE String → Matcher |

Required companion change: `re-find` of one arg accepts the
S12 handle (find-next, groups shape) while still rejecting
every other kind with the #32-pinned Matcher CCE. This
touches `re.rs` (see §2.5).

### 1.9 Matrix edits

Flip these 20 rows in place in `rust/core-coverage.md`
(lines 693, 735–742, 748, 752, 760, 766, 778, 787, 804–805,
819, 828–829) from `out-of-scope` to `implemented` with the
notes below, and update the header to `692 vars: 430
implemented, 2 divergent, 260 out of scope, 0 planned`.
Test file: `rust/crates/clj-eval/tests/collmisc.rs` (fenced
to S12 by #54); test-fn names are advisory — the implementer
keeps them or renames consistently.

| var | notes |
| --- | --- |
| `accessor` | tests/collmisc.rs structmap_rows (S12; probes structmaps.out) |
| `create-struct` | tests/collmisc.rs structmap_rows (S12; probes structmaps.out) |
| `defstruct` | tests/collmisc.rs structmap_rows (S12 macro; probes structmaps.out) |
| `struct` | tests/collmisc.rs structmap_rows (S12; probes structmaps.out) |
| `struct-map` | tests/collmisc.rs structmap_rows (S12; probes structmaps.out) |
| `chunk` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-append` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-buffer` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-cons` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-first` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-next` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunk-rest` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `chunked-seq?` | tests/collmisc.rs chunk_rows (S12; probes chunks.out) |
| `hash-combine` | tests/collmisc.rs mixhash_rows (S12; probes mixhash.out; tower y structural, §2.4) |
| `mix-collection-hash` | tests/collmisc.rs mixhash_rows (S12; probes mixhash.out) |
| `seque` | tests/collmisc.rs seque_rows (S12; probes seque.out) |
| `iteration` | tests/collmisc.rs iteration_rows (S12; probes iteration.out) |
| `file-seq` | tests/collmisc.rs fileseq_rows (S12; probes fileseq.out; documented divergence: string paths in/out, JVM takes/yields java.io.File) |
| `re-groups` | tests/collmisc.rs rematcher_rows (S12; probes rematcher.out) |
| `re-matcher` | tests/collmisc.rs rematcher_rows (S12; probes rematcher.out) |

## Part 2 — Notes

### 2.1 Design decisions

- D1 (struct marker): new `HostKind::Struct`. The basis is a
  `HostVal::Def` (identity `=`, `#object[…$Def …]` print);
  the instance is a `HostVal::Inst` whose content map carries
  basis keys (nil-defaulted) plus extras. New arms: `=` and
  both hashes treat struct instances as their content map
  (equal to plain maps — unlike records); print renders the
  content map; `jvm_class_of` reports `…PersistentStructMap`
  for instances and `…PersistentStructMap$Def` for bases;
  map ops follow the record path except dissoc-of-basis-key
  throws and assoc always stays struct.
- D2 (accessor): dedicated `ExtensionValue` + IFn arms
  (callable, `fn?` false). Not a `NativeFn` (`fn?` is true
  for those). Print `#object[…$1 …]`, CCE target
  `…PersistentStructMap` on non-struct input. The accessor
  carries its basis def: applying it to a struct of any
  other def (even same keys) throws `Accessor/struct
  mismatch`; arity ≠ 1 names `…PersistentStructMap/1`
  (slash, per the JVM); nil input throws the pinned NPE.
- D3 (Chunk emulation): `ChunkBufferValue { slots,
  taken }`, `ChunkValue { slots, count }`; `chunk` copies
  the slots (observationally identical to shared `Arc`
  slots — the buffer is dead after the take). `chunk` flips `taken`; append-after-chunk and
  count/nth on a degenerate 2nd chunk throw the two pinned
  NPE texts. `nth` indexes raw slots (nil holes, AIOOBE at
  capacity). `jvm_class_of`: `…ArrayChunk` /
  `…ChunkBuffer`; both print the pinned `#object` shapes;
  `(seq chunk)` falls into the existing `iseq_err`.
- D4 (chunked cons): reuse `SeqKind::ChunkedCons` (empty chunks
  already collapse to the tail, matching ch-cons-empty).
  No new seq shape.
- D5 (hash natives): `pub` on the two existing `hasheq.rs`
  fns only — `hash_combine(seed: i32, hash: i32)` and
  `mix_coll_hash(h: i32, n: i32)` (no new `hash_combine`
  overload: Rust has no overloading, and renaming would
  touch S10-owned call sites). The native composes
  `hash_combine(seed, hash_code(y))` — behavior-identical,
  Seed/capacity int-checking throws ArithmeticException
  `"integer overflow"`; doubles truncate; non-numbers take
  the existing cast errors. Disjoint from S10's
  ordered/unordered wrapper symbols; second merger resolves.
- D6 (seque): `ExtensionValue` holding a feeder thread +
  bounded queue, drained through a lazy seq. The queue-object
  arity is unrepresentable (no port queue kind exists);
  integer capacities route through the shared int-cast
  helper (sq-cap-str/float/neg pins); anything else throws
  the int-cast error.
- D7 (iteration): `ExtensionValue` holding step + resolved
  opts; seq path via the lazy-cons machinery (first step
  eager); reduce path via seq citizenship (`walk_seq` —
  observably identical to an explicit loop, including
  `reduced` short-circuit per it-reduced/it-reduce-count).
- D8 (matcher handle): `ExtensionValue` holding the
  compiled `regex::Regex`, the subject text, the next
  offset, and the last `Captures` (or none). Find uses
  `captures_at`; after an empty match the next offset
  advances one char (rm-empty-advance). `jvm_class_of` is
  `java.util.regex.Matcher`; print is the port `#object`
  shape with the JVM `Matcher[…]` state string.

### 2.2 Spike resolutions

- file-seq strings-vs-path: STRINGS. The oracle (fs-one,
  fs-str-arg) shows the JVM takes and yields `java.io.File`,
  but the port has no File kind and `slurp`/`spit` take
  string paths. A new path kind would serve one var; string
  in/out is coherent and keeps the walk testable. The two
  starred §1.7 rows record the divergence.
- chunked-seq? on port seqs: YES for `SeqKind::Chunked` /
  `SeqKind::ChunkedCons` heads (the 32-item chunking already
  mirrors the JVM) plus bounded `RangeValue`s (ch-cq-range
  is true while ch-cq-range-inf is false, so infiniteness
  is the discriminator). Everything else answers false.

### 2.3 Grounding map

- `host.rs` (`HostVal`/`HostKind`, #33): D1 basis/instance.
- `clj-value/src/seq.rs` (`SeqKind::Chunked`/`ChunkedCons`, 32-item
  blocks): D4, §1.3 projectors, `chunked-seq?` rule.
- `hasheq.rs` (`hash_combine` private at :357,
  `mix_coll_hash`, `hash_code`): D5 (S10 owns the
  ordered/unordered wrappers in this file).
- `re.rs` (`pattern_source`, `match_text`, `groups_value`):
  D8 and the `re-find` 1-arity extension.
- `io.rs` (`native_slurp` string-path + `file_err`): §1.7
  arg convention and error shape.
- `seqs.rs`/`lazy.rs` (`seq_value`, `iseq_err`,
  `realized_value`, thunks): D6/D7 seq paths, ch-seq error.
- `error.rs` (`jvm_class_of`, `cce_message` + `cce_err` in
  `core.rs`): CCE/ISeq text for every new kind.

### 2.4 Divergences (all documented at the rows)

- file-seq takes/yields strings; JVM takes/yields Files
  (fs-str-arg*, fs-classes/fs-one reference only).
- `hash-combine` with tower `y` uses the port structural
  `hash_code`, not the JVM magic ints 5/3/466
  (mh-comb-bign/dec/ratio reference only). Consistent with
  the standing tower-hash policy in `hasheq.rs`.
- Seque/iteration/matcher internals (threads, queue class,
  handle class) are port machinery; only the pinned
  observable behavior transfers. The `BlockingQueue`-object
  `seque` arity is unrepresentable (no port queue kind);
  integer capacities only.

### 2.5 Risks and open points

- R1 (cross-slice touch): the `re-find` 1-arity extension
  edits `re.rs`, fenced to #32. The #32-pinned rejections
  stay for every non-handle kind; the owner must ack the
  shared edit or move the handle arm behind a hook.
- R2 (racy by design, unpinned): seque feeder timing and
  producer-error propagation (the drain can touch the eos
  sentinel before the agent records its error, so a
  producer throw sometimes surfaces and sometimes
  vanishes). Tests assert values only, on small inputs.
- R3 (`hasheq.rs` sharing): S10 owns the ordered/unordered
  wrappers in the same file. Symbols are disjoint; the
  second merger resolves textually.
- R4 (capacity errors): sq-cap-str pins CCE String →
  Character via the `(int x)` path — the implementer must
  route `seque` capacities through the same int-cast
  helper as `int`, not a bespoke cast.
- R5 (struct `=` integration): struct-vs-plain-map
  equality cuts across the `=` dispatch, not just
  `host_eq` (which only sees `HostVal` pairs). The
  implementer must route struct instances to map
  comparison wherever `=` partitions on `Value` kind.
- R6 (no `class`): six JVM class pins transfer as
  `jvm_class_of` mappings (§1.1); each needs its derived
  CCE/ISeq-text test from §1.2–1.3/1.8.

### 2.6 Conformance checklist (from #54 BDD)

- `(struct-map (create-struct :a) :a 1)` behaves as the
  JVM map (§1.2: sm-eq-map, sm-hash-eq, sm-seq).
- Chunk build/project roundtrip (ch-build, ch-first-vec,
  ch-rest-vec, ch-next-vec).
- `(chunked-seq? …)` matches the oracle per seq kind
  (§1.3 battery + sq-cq/it-cq/fs-cq).
- `(seque [1 2])` seqs (sq-basic, sq-class, sq-cq).
- `(iteration …)` reduces (it-reduce, it-reduced,
  it-reduce-empty, it-reduce-count).
- `(file-seq "<dir>")` walks (fs-count, fs-names,
  fs-root-first; string arg per §1.7).
