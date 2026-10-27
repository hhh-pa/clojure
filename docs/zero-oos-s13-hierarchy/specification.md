# Zero-oos S13: hierarchy-aware multimethod dispatch (issue #57)

Nine hierarchy/multifn vars go `implemented` — `ancestors`,
`derive`, `descendants`, `isa?`, `make-hierarchy`, `parents`,
`prefers`, `underive`, `remove-all-methods` — and dispatch
consults hierarchies plus `prefer-method` edges
(`method_for`: exact → best via `isa?` + prefers → default,
else the JVM ambiguity error). `get-method` goes
hierarchy-aware, `defmulti` gains `:hierarchy`, and
`prefer-method` learns the JVM conflict check (decision D1).

## BDD scenarios

- Given `(derive ::cat ::pet)` then `(isa? ::cat ::pet)`,
  the answer is `true`.
- Given methods on `::shape` and `::square` with
  `(derive ::square ::shape)`, dispatching `::square` runs
  the `::square` method (most specific wins).
- Given methods on two unrelated parents of the dispatch
  value, the call throws the JVM ambiguity text.
- Given `(remove-all-methods m)`, `(methods m)` and
  `(prefers m)` are both `{}` (probe `x-rmall`).

## Semantics

### Hierarchy maps

`make-hierarchy` answers
`{:parents {} :descendants {} :ancestors {}}` (probe
`h-make`; insertion order verbatim).

`derive` is a line port of `core.clj` (3-arity), with the
JVM touch order (probes `h-*`, `n-derive-*`):

- asserts (verbatim `AssertionError` texts): `(not= tag
  parent)` (`h-self`); tag class-or-`Named`
  (`h-tagstr`, `h-tagnil`); parent `Named`
  (`h-parent-str`, `h-parent-num`, `h-classparent`).
  Explicit-h derive needs no namespaces (`h-ns-ok`,
  `h-unnamed-ok`, `h-sym-ok`, `h-mixed-ok`); class tags
  are allowed (`h-classtag`), class parents are not.
- touch order (verbatim helpful-NPE texts when a key is
  missing): `(tp tag)` first — `because "tp" is null`
  (`h-badh-derive`); an already-present edge returns `h`
  here, before any other touch (`h-idem`,
  `n-derive-edgeonly`). Then `(ta tag)` (`because "ta"
  is null`, `n-derive-partial`) for the already-has
  error (`h-already`), `(ta parent)` for the cyclic
  error (`h-cycle`, `h-cycle3`), then the transitive
  build, whose first `:descendants` touch NPEs with
  `because "sources" is null` (`n-derive-partial2`).
- non-map relation values fail the call: `nil` NPEs
  (above); anything else is the `IFn` cast
  (`n-derive-badparents`, `n-isa-badanc` — the full
  loader text, via the house `cce_err`).
- the already-has / cyclic texts render through
  `print-str` semantics (probe `h-already`,
  `h-cycle`): `print` rendering, not `pr`.
- output key order is `:parents :ancestors
  :descendants` (probe `h-derive3`).

`derive` 2-arity asserts `(namespace parent)`
(`h-gparent-plain`) and tag class-or-namespaced-`Named`
(`h-gtag-plain`), alters the private
`#'clojure.core/global-hierarchy` root under the
var-alter lock, and answers `nil` (`h-gderive`).
`underive` rebuilds from the surviving parent edges
(`h-under`, `h-under-trans`); a missing edge returns `h`
(`h-under-miss`); garbage `h` NPEs with
`because "parentMap" is null` (`h-badh-under`);
rebuilding nothing answers a fresh empty hierarchy
(`n-under-partial`, key order `:parents :descendants
:ancestors`); the global arity answers `nil`
(`h-under-global`).

`isa?` evaluates its arms in order (`n-isa-eq-badh`,
`n-isa-class-badh`, `n-isa-vec-badh`, `n-isa-ne-badh`):

1. `=` (`h-isa-eq`, `h-isa-ne`);
2. both classes and assignable through the lattice
   (`h-isa-class`; interfaces are assignable to
   `Object`, `n-iface-obj`; `Object` is not assignable
   to interfaces, `n-obj-iface`);
3. `parent ∈ ancestors-map(child)` — touching a missing
   `:ancestors` NPEs with the bare
   `Cannot invoke "clojure.lang.IFn.invoke(Object)"`
   text (`h-badh-isa`);
4. class child: `parent ∈ ancestors-map(super)` for any
   transitive super (`h-isa-supermap`);
5. vectors: equal count and element-wise `isa?`
   (`h-isa-vec`).

`parents` / `ancestors` / `descendants` tolerate
key-missing `h` (`h-badh-views`, `n-par-partial`,
`n-anc-partial`, `n-desc-partial`): class tags union the
lattice bases / transitive supers with the map entries
(`h-p-string`, `h-a-string`, `h-p-kw`, `h-p-vec`,
`h-p-amap`, `h-p-hset`, `h-p-union`, `h-a-union`,
`n-par-badh-class`, `n-par-ifn`); `descendants` of a
class throws `UnsupportedOperationException`
`Can't get descendants of classes` (`h-d-string`);
empty answers `nil` (`h-empty`).

`prefers` answers the multifn's prefer table
(`x-prefers-view`, `x-prefer-bare`); non-multifns fail
the sibling `MultiFn` cast.

### Dispatch (`method_for`)

Per candidate entry in port table order: an exact hit
answers immediately; otherwise every entry whose key
the dispatch value `isa?`s competes — a more specific
key displaces the best, a less specific one keeps it,
else preference edges decide, else the call throws
`Multiple methods in multimethod 'NAME' match dispatch
value: DV -> CHALLENGER and BEST, and neither is
preferred` (`x-amb`, `x-get-amb`; challenger-first is
pinned against observed table order by `n-amb-order`).
No candidate falls back to the default entry
(`x-def-hier` proves hierarchy beats default);
exhausted, invocation throws the existing no-method
error and `get-method` answers `nil`. Transitive
matches (`x-trans`), most-specific (`x-specific`,
`x-class-spec`), vector dispatch (`x-vec`), class
dispatch through the lattice (`x-class`), prefer
resolution in both directions (`x-prefer`,
`x-prefer-flip`), hierarchy-aware `get-method`
(`x-get-hier`), hierarchy edits visible with no cache
(`x-cache-inv`), and exact-key removal under a
hierarchy (`x-remove-hier`) are all pinned.

`prefer-method` keeps its record shape and answers the
multifn, but now validates under the install lock: a
direct reverse edge throws `IllegalStateException`
`Preference conflict in multimethod 'NAME': OTHER is
already preferred to PREF` (`x-prefer-conflict`;
decision D1). Self-preference is allowed
(`x-prefer-self`); transitive cycles are allowed —
the check is direct-edge only (`n-prefer-trans`).

`remove-all-methods` is a macro over a hidden native
(like `remove-method`): it clears the method table
*and* the prefer table (`x-rmall`), answers the
multifn, rejects non-multifns with the sibling cast
(`x-rmall-bad`), and the multifn stays usable after
(`x-rmall-reuse`, `x-rmall-call`).

`defmulti :hierarchy` evaluates its value: `nil` (or
absent) is the live global hierarchy; a var, atom, or
agent holding a hierarchy map is dereferenced live at
dispatch (`x-atom-miss`, `x-atom-live`, `x-hier-var`,
`x-hier-iso`); anything else is the verbatim
`clojure.lang.IRef` cast (`x-hier-map`, `x-hier-5` —
decision D2: JVM-exact, bare maps rejected). A `nil`
hierarchy reference NPEs when dereferenced
(`x-hier-nil` → the house deref-nil NPE); a
non-hierarchy value behaves like garbage `h` (exact
hits still work, `x-hier-badval`; hierarchy consults
NPE, `n-badval-miss`).

### The class lattice

`class?` in the port is a tabled class-name string or
a `HostDef` descriptor. The lattice is data, not code:
per-class `{is-interface, bases[], supers[]}`
transcribed from the `l-*` / `n-range` / `n-unbound`
probes (35 classes: `Object`, `String`, `Long`,
`Double`, `Boolean`, `Character`, `Number`,
`BigInteger`, `BigDecimal`, `BigInt`, `Ratio`, `Symbol`,
`Keyword`, `Var`, `Var$Unbound`, `Atom`, `Agent`,
`Delay`, `LazySeq`, `PersistentList`,
`PersistentList$EmptyList`, `PersistentVector`,
`APersistentVector$SubVector`, `MapEntry`,
`PersistentArrayMap`, `PersistentHashMap`,
`PersistentTreeMap`, `PersistentHashSet`,
`PersistentTreeSet`, `Pattern`, `TaggedLiteral`,
`ReaderConditional`, `Range`, `LongRange`,
transient vector / array-map / hash-set, `MultiFn`,
plus interfaces `CharSequence`, `IFn`, `ISeq`).
Assignability: same name, or parent in the child's
transitive supers, or interface child with parent
`Object`. `parents` answers bases, `ancestors`
answers supers (each unioned with map relations).
Names outside the table are not classes (spec seam
for S16 — unknown-name pins use never-real names
only). `HostDef` children: `deftype` / `record` /
`gen-class` descriptors claim their interface names
plus `Object`; interface descriptors claim their name
only (no `Object` — interfaces have no superclass);
clause 4 walks the claimed interfaces. `derive`
accepts class tags (tabled names, `HostDef`s) and
rejects class parents, like the JVM asserts.

### Global hierarchy

A private `core.clj` var,
`(def ^:private global-hierarchy (make-hierarchy))`
(the JVM var is private too, `h-private`, so no
matrix row — same standing as the other private
`core.clj` helpers). Global arities alter it under the
var-alter lock; default multifns dereference it live.

## Error catalog

| site | class | text | probe |
| --- | --- | --- | --- |
| ambiguity | `IllegalArgumentException` | `Multiple methods in multimethod 'N' match dispatch value: DV -> C and B, and neither is preferred` | `x-amb` |
| prefer conflict | `IllegalStateException` | `Preference conflict in multimethod 'N': O is already preferred to P` | `x-prefer-conflict` |
| derive self | `AssertionError` | `Assert failed: (not= tag parent)` | `h-self` |
| derive tag | `AssertionError` | `Assert failed: (or (class? tag) (instance? clojure.lang.Named tag))` | `h-tagstr` |
| derive parent | `AssertionError` | `Assert failed: (instance? clojure.lang.Named parent)` | `h-parent-str` |
| global parent | `AssertionError` | `Assert failed: (namespace parent)` | `h-gparent-plain` |
| global tag | `AssertionError` | `Assert failed: (or (class? tag) (and (instance? clojure.lang.Named tag) (namespace tag)))` | `h-gtag-plain` |
| already ancestor | `Exception` | `TAG already has PARENT as ancestor` (print rendering) | `h-already` |
| cyclic | `Exception` | `Cyclic derivation: PARENT has TAG as ancestor` | `h-cycle` |
| bad `:hierarchy` | `ClassCastException` | full `IRef` loader text | `x-hier-map`, `x-hier-5` |
| hierarchy NPEs | `NullPointerException` | `Cannot invoke "clojure.lang.IFn.invoke(Object)"[ because "tp"/"ta"/"sources"/"parentMap" is null]` | `h-badh-*`, `n-derive-partial*` |
| map-call CCE | `ClassCastException` | full `IFn` loader text | `n-derive-badparents` |
| class descendants | `UnsupportedOperationException` | `Can't get descendants of classes` | `h-d-string` |
| arity | `ArityException` | `Wrong number of args (N) passed to: clojure.core/NAME` | `h-arity1` |

## Divergences from the JVM

- Ambiguity key order follows port table order, not
  JVM hash order (same `DV -> challenger and best`
  shape; `x-amb`, `n-amb-order`). With 3+ matching
  methods and partial preference relations the
  throw-vs-return outcome is deterministic here and
  hash-dependent there.
- The mutating multifn macros quote non-symbol targets
  (pre-existing port pattern): only vars/symbols ride
  the locked install path.
- Unknown class names are not classes (fixed S13
  lattice; S16 extends it without touching these
  pins — they use never-real names).

## Seams

- S14's `find-protocol-impl` class walk is a separate
  code path; this slice never touches protocol
  dispatch.
- S16 lands `Class` values; the lattice table grows
  then, and these pins stay green.

## Tests

`rust/crates/clj-eval/tests/hierarchy.rs` replays
every probe label (`h-*`, `l-*`, `x-*`, `n-*`) plus
arity/CCE guards, `HostDef` relations, and
never-real unknown-class names. All 23 existing
multimethod pins (`tests/multimethods.rs`,
`docs/i33-typechain/probes/multimethods.out`,
re-verified byte-identical against alpha10 for this
slice) stay green; only the `:hierarchy` arm of
`multi_options_rejected` changes (now the `IRef`
cast — the other arms are untouched).

## Matrix

Nine rows in the `Previously unlisted — accepted
future work` table, ASCII-sorted by var cell,
flipped straight to `implemented` with test notes;
header counts recounted to actuals. The
`prefer-method` note gains "edges consulted at
dispatch (S13)".

## Decisions

- D1: `prefer-method` conflict validation is in,
  although the issue never names it: without it the
  port admits preference states the JVM forbids,
  leaving dispatch behavior with no oracle analog.
  Five lines in an owned file, zero parallel-slice
  risk, oracle-pinned.
- D2: `:hierarchy` is JVM-exact (var/atom/agent or
  nil; bare maps rejected with the verbatim CCE).
  No snapshot extension: users wrap explicit
  hierarchies in an atom, exactly like the JVM.
- D3: the global hierarchy is a real private
  `core.clj` var (not evaluator state), so
  `:hierarchy #'clojure.core/global-hierarchy` and
  inspection work verbatim.
- D4: NPE/CCE/assert texts mirror the oracle
  verbatim (house precedent for helpful-NPE shapes).
