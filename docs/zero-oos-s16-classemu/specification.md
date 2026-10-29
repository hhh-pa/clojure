# Zero-oos S16: class emulation — specification (issue #60)

`Class` values, the emulated lattice (`bases`/`supers`/`instance?`),
`type`/`class`/`cast`/`class?`, the `import` macro plus a real
per-namespace import table (feeding `ns-imports`, `ns-map`, `resolve`,
and unqualified construction/member resolution), the `..`/`memfn`
macros over the emulated member surface, `bean`, a resolving
`identical?` consistent with the analyze-time rewrite, and
`PrintWriter-on` over port writers.

BDD (from the issue; each traces to oracle rows and to a
`classemu.rs` test named in §1.15):

- `(type "a")` → `String`, `(class "a")` →
  `String` (`c-type-plain`, `c-class-str`).
- `(instance? String "a")` → `true`, `(instance? String
  1)` → `false` (`c-inst-basic-t`, `c-inst-basic-f`).
- `(bases String)` → six classes in JVM declaration order,
  `(supers String)` → the six as a set (`c-bases-shape`,
  `c-supers-shape`, `l-java.lang.String`).
- `(import 'java.util.Date)` then `(ns-imports *ns*)`
  maps `Date` → the `Date` class (`i-ret`,
  `i-ns-imports-val`).
- `(.. "a" (toString))` → `"a"` (`m-thread-1`).
- `(bean x)` maps getters (`b-keys-*`, `b-cls-vals`).
- `(identical? [1] [1])` → `false` while `(=
  [1] [1])` → `true` (`d-eq-vs-ident-vec`).
- `(PrintWriter-on f nil)` wraps a port writer
  (`w-class`, `w-write-flush`).

## Part 1 — Contract

### 1.1 Conventions

Oracle: `clojure.main` on the alpha10 jars
(`org.clojure/clojure/1.13.0-alpha10`,
`spec.alpha/0.6.249`, `core.specs.alpha/0.6.148-alpha12`
under `/home/hhu/.m2`), invoked exactly as the header of
each probe says (`-Duser.language=en -Duser.country=US
-Duser.timezone=UTC`, from the repo root). Every probe
exits 0; every `.out` is byte-identical across re-runs
(verified by re-running each file and `cmp`ing).

| probe file | rows | prefix | covers |
|---|---|---|---|
| `probes/lattice.clj` | 227 | `l-` | §1.3 lattice: 214 static rows + 13 dynamic-rule rows |
| `probes/classes.clj` | 155 | `c-` | §1.3–§1.6: HostDef ancestry, `type`/`class`/`instance?`/`class?`/`cast`, class value semantics |
| `probes/import.clj` | 54 | `i-` | §1.7–§1.8: `import`, conflicts, `ns-imports` |
| `probes/interop.clj` | 257 | `m-`/`b-`/`t-` | §1.10–§1.12: `..`, member surface, `memfn`, `bean` |
| `probes/identical.clj` | 43 | `d-` | §1.9: `identical?` per-kind table, inline, arities |
| `probes/writers.clj` | 26 | `w-` | §1.14: `PrintWriter-on` |
| `probes/arrays.clj` | 18 | `a-` | S15 follow-up only (array classes; §2.2) |

Total: 780 oracle rows. `throwof` prints `label` +
`pr-str` of the value (or `THROW Class|message`);
`throwcause` prints the root-cause class + message with
`evalN`/`__N`/`--N`/`@L` scrubbed. Rows marked
oracle-only document JVM behavior the port deliberately
does not implement (each names its follow-up).

Error-chain convention below quotes the port's
`uncaught` rendering (`Class | message`, outermost
first); JVM `THROW` rows quote the oracle's
`(.getName (class t))` + `(.getMessage t)`.

### 1.2 Class values

New value kind `Value::Class` holding a `ClassVal`:

```rust
ClassVal { name: Arc<str>, iface: bool, bases: Arc<[Arc<str>]> }
```

- `name` is the binary name (`java.lang.String`,
  `user.Point`, `[I` in the S15 follow-up). Equality,
  hashing, and `identical?` are by `name` only (§1.9):
  classes are singletons by name, so two evaluations of
  `(class "a")` are `=` and `identical?` without any
  interning table.
- `iface` is `Class.isInterface` (lattice row, or the
  `HostKind::Interface` rule, or `false` for fn classes).
- `bases` is the ordered `bases` name list, materialized
  at construction from the static table (§1.3), the
  HostDef rule, or the fn rule. It is stored (not
  re-derived) because fn-class ancestry (fixed →
  `AFunction`, variadic → `RestFn`, `l-fn-fixed`,
  `l-fn-variadic`, `l-fn-multi-fixed`) is unknowable from
  the display name alone. `supers` walks names through
  the table plus the registry (§1.3), so `bases` entries
  stay names, not nested `ClassVal`s.

Why a new kind (§2.1): no existing kind answers the
lattice (`HostVal::Def` describes port-defined types
only; strings fail `class?`; vars deref). The lattice
needs interface-ness and ordered ancestry per value, so
the kind carries exactly `(name, iface, bases)`.

| probe IDs | pins |
|---|---|
| `c-eq-tt`, `c-eq-tf` | `=` is by name (`String`/`String` true, `String`/`Long` false) |
| `c-pr-str`, `c-print-str`, `c-with-out-str` | `pr`/`print` render the bare name (`java.lang.String`), interfaces too (`c-print-iface`) |
| `c-str-class`, `c-tostring`, `c-str-iface`, `c-tostring-iface` | `str`/`toString` render `class ` + name (`interface ` + name for interfaces) |
| `c-meta`, `c-with-meta` | no metadata; `with-meta` is the module-shape IObj CCE |
| `c-seq-err` | `(seq String)` is `Don't know how to create ISeq from: java.lang.Class` |
| `c-ifn?`, `c-fn?` | not invokable, not a fn |

Behaviors:

- `jvm_class_of(Value::Class(_))` is
  `java.lang.Class`. (This one arm makes `c-seq-err`,
  cast sources, and catch rendering fall out.)
- Print (`print.rs`): `pr`/`print` write `name`
  (lattice names print bare, exactly like the JVM's
  `print-method` for non-array classes, `c-pr-str`).
  Array classes (`[I` → `int/1`, `a-pr`) are the S15
  follow-up's print arm, not this slice's.
- `str` (`str_of_one`/`str_of_realized`): `class ` +
  name, `interface ` + name for interfaces
  (`c-str-class`, `c-str-iface`). This is a dedicated
  arm: `str` of a class is its `toString`, not its
  print form.
- `=` (`compare.rs`): same-kind by name; anything else
  false. `hash` (`hasheq.rs`): the name's string hash
  (required: `=`-equal classes share map buckets).
- `hashCode/0` member: Java `String.hashCode` of the
  name — a deterministic stand-in (§2.1; the JVM's is
  identity and unpinnable, so there is deliberately no
  oracle row).
- `identical?`: by name (§1.9).
- Metadata: none. `(meta C)` → `nil`;
  `(with-meta C m)` is the module-shape CCE
  (`cce_err(C, "clojure.lang.IObj")`, byte-equal to
  the oracle `c-with-meta`).
- Not seqable/counted/invokable: `seq`/`count` take the
  existing `iseq_err`/`count` paths with
  `java.lang.Class` (`c-seq-err`); `ifn?`/`fn?` false.

### 1.3 The lattice (registry)

One static table maps binary name → `(iface,
ordered-bases)`. It is closed under `bases`+`supers`
(every name any row mentions has its own row — verified
mechanically over `lattice.out`), covers every
`jvm_class_of`/`class_name` name the port reports
(§1.4), every `ThrowClass` name (the nonexistent
`java.util.MissingFormatConversionException` is
synthesized below; `ClassNotFoundException` is tabled
for the new variant §1.7 adds — shared with wave1,
which added the same variant;
`java.util.NoSuchElementException` is tabled for the
new variant §1.10 adds, live-JVM ancestry
`[RuntimeException]`), every S13 table key (all 41 full
rows agree
as sets, §2.2), plus `java.lang.Class`, `java.util.Date`
(the issue's BDD), `java.math.BigInteger` (S13 parity),
and the fn-ladder (`AFn`, `AFunction`, `Fn`, `RestFn`,
`Comparator`, `Future`, `IBlockingDeref`). `supers` is
computed by a visited-set walk (cycle-safe: adversarial
redefinition can link HostDefs cyclically, §2.5),
never stored.

| probe IDs | pins |
|---|---|
| `l-<name>` (214 rows) | `[interface? ordered-bases-or-nil sorted-supers-or-nil]` per static name |
| `l-fn-fixed`, `l-fn-multi-fixed`, `l-fn-variadic`, `l-fn-name` | fn rule: any `rest` arity → `[RestFn]`, else `[AFunction]` |
| `l-native-fixed`, `l-native-rest` | JVM truth (`first` → `AFunction`, `map` → `RestFn`); the port answers `[AFunction]` for all natives (§2.1) |
| `l-future`, `l-future-name`, `l-promise`, `l-promise-name` | reify rows; the port reports the stable counter-free names |
| `l-reify-shape` | `[Object, <ifaces...>]` shape behind the HostDef rule |
| `l-iteration`, `l-iteration-name` | ancestry the synthetic `clojure.core$iteration` row copies |
| `c-rec-bases/supers/claimed`, `c-det-bases/claimed` | record/deftype implicit interfaces and claimed-first order |

Static rows (214, `lattice.clj`): `bases` keep JVM
declaration order (superclass first — `String` puts
`Comparable` before `CharSequence`, `l-java.lang.String`;
S13 stores ASCII-sorted bases and sorts at the view, so
sets agree and order differs by design, §2.2). `supers`
is a set (the probe sorts; the port returns a `Set` and
tests compare as sets). `(bases Object)` and `(supers
Object)` are `nil` (`l-java.lang.Object`); interfaces
have no superclass (`l-clojure.lang.ISeq`).

Import-only rows (4): the port never *reports* these
names, but they are tabled so `__import*` resolves them
instead of `CNFE`ing on real classes:
`PersistentHashMap$ArrayNode$Seq`
(`c-name-seq-bigmap-oo`) and
`PersistentTreeMap$Seq` (`c-name-seq-sortedmap-oo`,
§1.4 pins the port's divergent simplifications),
plus `java.util.Calendar` (`[Object, Serializable,
Cloneable, Comparable]`, required by
`i-import-multi-ret`) and `java.sql.Date`
(`[java.util.Date]`, required by
`i-conflict-class`) — both ancestries verified live on
the JVM by the §2.6 method. More exotic `NodeSeq`
variants stay unmapped (follow-up if ever needed,
§2.2).

Synthetic rows (2, port-only, documented here — the
oracle cannot produce them):

- `clojure.core$iteration`: the JVM reports a
  per-call-site `clojure.core$iteration$reify__N`
  (`l-iteration-name`); the port reports the stable
  `clojure.core$iteration` (S12's choice). Ancestry is
  copied from the JVM reify row
  (`[Object, IReduceInit, Seqable, IObj]`,
  `l-iteration`).
- `java.util.MissingFormatConversionException`: this
  class does not exist on the JDK (`Class/forName`
  throws `ClassNotFoundException` — verified live), but
  the port's `ThrowClass` has the variant
  (pre-existing, out of fence). Ancestry is synthesized
  from its `throw_parent` entry plus its siblings:
  bases `[IllegalFormatException]`, supers the
  `IllegalFormatException` closure. The port keeps
  throwing it where it does today; S16 only lets the
  lattice answer for it.

Dynamic rules (applied at `Class` construction and by
the `supers` walk; the static table never holds these
names):

- HostDef names (`user.Point`, …): `iface` is
  `kind == Interface`. Bases are `[Object]` ++ claimed
  `interfaces` in declared order (the port has no
  `:extends`, so the superclass is always `Object`;
  interface defs list their `extends` with no
  superclass, mirroring `l-clojure.lang.ISeq`), plus
  kind implicits appended AFTER the claimed list:
  - `Record`: `IRecord, IHashEq, IObj, ILookup,
    IKeywordLookup, IPersistentMap, Map, Serializable`
    (`c-rec-bases`; `c-rec-claimed` proves claimed
    interfaces sort before the implicits;
    `c-rec-supers` pins the full 15-name closure).
  - `Type` (deftype): `IType` (`c-det-bases`;
    `c-det-claimed` proves the claimed-first order).
  - `Interface`, `GenClass`: no implicits
    (`l-reify-shape` evidences the bare
    `[Object, ifaces...]` shape).
  - `Struct`: no dynamic rule — struct instances report
    `PersistentStructMap` and struct defs report
    `PersistentStructMap$Def` (§1.4 carve-outs), both
    static; struct descriptors do not fold (§1.5).
  The walk recurses into claimed names via the registry
  (static row if tabled, HostDef rule if defined) and
  skips names that no longer resolve (documented
  leniency, §2.4). The implicits are honest: the port's
  records take `with-meta` (`IObj`) and are maps; its
  deftypes reject `with-meta`, exactly like the JVM
  (which throws `CCE` there — no `IObj` in
  `c-det-bases`).
- Closures: any arity with `rest` → bases `[RestFn]`,
  else `[AFunction]` (`l-fn-fixed`,
  `l-fn-multi-fixed`, `l-fn-variadic`). One exact-display
  exception: `clojure.lang.PersistentStructMap$1` (the
  S12 accessor display, a real JVM class) uses its
  static row (`[AFn]`, `l-clojure.lang.PersistentStructMap$1`).
- NativeFns: bases `[AFunction]`, unconditionally.
  This diverges for variadic natives (`map` is
  `RestFn`-based on the JVM, `l-native-rest`) — the
  only faithful fix is per-native shape flags across
  every slice's tables, which the fence forbids (§2.1).
  Observable only via `(bases (class <variadic-native>))`
  and `RestFn` membership in its `supers`.
- Future/Promise kinds: the static reify rows keyed by
  the port's stable names (`clojure.core$future_call$reify`
  → `[Object, IPending, IBlockingDeref, Future, IDeref,
  IObj]`, `l-future`; `clojure.core$promise$reify` →
  `[Object, IPending, IBlockingDeref, IDeref, IFn,
  IObj]`, `l-promise`).
- Importing a fn/reify display name is `CNFE`: dynamic
  names are values, never registry entries (§1.7).

`bases`/`supers` natives (mirror `core.clj` shapes):

- `(bases C)`: `nil` for `nil` (`c-bases-nil`) and for
  empty ancestry (`c-bases-object`, `c-bases-iface`);
  else `(cons superclass (seq ifaces))` when a superclass
  exists, else `(seq ifaces)` — a seq (`seq?` true,
  `c-bases-shape`), in lattice order. Non-class args are
  the module-shape CCE (`c-bases-nonclass`);
  `HostVal::Def` is not an error (it folds, §1.5). Wrong
  arities are runtime `ArityException`s (`c-bases-arity0`,
  `c-bases-arity2`, `c-supers-arity0`).
- `(supers C)`: `nil` for `nil`/empty (`c-supers-nil`,
  `c-supers-object`, `c-supers-iface`); else a set
  (`set?` true, `c-supers-shape`). Non-class args are
  the module-shape CCE (`c-supers-nonclass`).
- `HostVal::Def` folds to its name at the boundary
  (same rule as `instance?`, §1.5 — S13 coherence).

The implementation test pins all 214 static rows
(verbatim from `lattice.out`) plus one test per dynamic
rule (§1.15).

### 1.4 `type` and `class`

`(class x)` answers the `Class` of `x`'s `jvm_class_of`
name (every name in §1.3, via the construction rules
there); `(type x)` is `(or (:type (meta x)) (class x))`,
exactly like `core.clj`.

| probe IDs | pins |
|---|---|
| `c-class-*` (41 rows) | class per kind, incl. `nil` → `nil`, `String` → `Class`, `*out*` → `OutputStreamWriter`, `*err*` → `PrintWriter`, `()` → `EmptyList`, `(Object.)` → `Object` |
| `c-name-*` (29 rows) | port-reported names verified against the oracle (transients ×4, seq subkinds ×13, chunks, matcher, structs, reader, natives ×2; future/promise/iteration/static-native shapes scrubbed) |
| `c-name-seq-bigmap-oo`, `c-name-seq-sortedmap-oo` | oracle-only: JVM names the port does NOT report (divergent simplifications, below) |
| `c-type-*` (9 rows) | the `:type` rule: plain, `nil` → `nil`, meta value returned as-is (even non-classes), falsy `:type` falls through to `class` |
| `c-class-arity0/2`, `c-type-arity0/2` | runtime `ArityException`s (`clojure.core/class`, `clojure.core/type`) |
| `c-class-cons-nil`, `c-class-cons` | `(cons x nil)` is a `PersistentList`; `(cons x seq)` is a `Cons` |

Behaviors:

- `(class nil)` → `nil` (`c-class-nil`); `(type nil)`
  → `nil` (`c-type-nil`).
- `(class C)`/`(type C)` for a class `C` → the `Class`
  class (`c-class-class`, `c-type-class`).
- `(type x)` with `(:type (meta x))` truthy returns it
  as-is — keyword, string, anything (`c-type-meta`,
  `c-type-meta-str`, `c-type-sym-meta`); falsy (`nil` or
  `false`) falls through to `(class x)`
  (`c-type-meta-nil`, `c-type-meta-false`). Kinds without
  metadata take the `class` branch (`c-type-nometa`).
- `(class x)` is the `jvm_class_of`/`class_name` name
  (§1.3), with two carve-outs where the port's internal
  display name is not the JVM's class: struct instances
  → `PersistentStructMap` and struct defs →
  `PersistentStructMap$Def` (`c-name-struct`), even
  though `type_name` says `user.qs`/`user.qs$Def`.
  Everything else reports exactly what the port already
  reports: transients are origin- and hashiness-aware
  (`c-name-tvec/tarr/tset/thash`), sorted collections
  report the tree names (`c-class-sortedmap/sortedset`),
  writers report per-kind (`c-class-out/err`),
  `iteration`/`future`/`promise` report the stable
  counter-free names (`c-name-iteration/future/promise`),
  and seq subkinds report their `SeqKind` (`c-name-keys/
  vals/rseq/subvec-seq/sort/str-seq/vec-seq/seq-smallmap/
  seq-set/seq-sortedset`, all agreeing with the JVM).
- Two seq-subkind names DIVERGE by pre-existing port
  design (the `SeqKind` enum, out of fence): big-map
  seqs report `APersistentMap$KeySeq` (JVM:
  `PersistentHashMap$ArrayNode$Seq`,
  `c-name-seq-bigmap-oo`) and sorted-map seqs report
  `PersistentArrayMap$Seq` (JVM:
  `PersistentTreeMap$Seq`, `c-name-seq-sortedmap-oo`).
  The port names rule `class`/`bases`/`instance?`; the
  JVM names stay importable via the §1.3 import-only
  rows.
- Native displays are counter-free
  (`clojure.core$map`, `c-name-native`); the JVM's
  `:static` natives carry a build counter
  (`clojure.core$first__N`, `c-name-native-fixed`) that
  the port never reports.
- Wrong arities are runtime `ArityException`s naming
  `clojure.core/class` / `clojure.core/type`
  (`c-class-arity0/2`, `c-type-arity0/2`) — no analyzer
  involvement.

### 1.5 `instance?`

A resolving binary native plus two analyzer rules. The
JVM's `instance?` is compiler magic over a real var
(`c-inst-resolve` → `#'clojure.core/instance?`, whose
root is an anonymous fn, `c-inst-var-class`); direct
calls compile to the check, other positions use the var
(`c-inst-var-apply`, `c-inst-var-apply-f`,
`c-inst-ho`). The port mirrors that split: the analyzer
owns direct qualified calls, the var owns everything
else — and both agree everywhere.

| probe IDs | pins |
|---|---|
| `c-inst-basic-t/f` | the BDD (`String` vs `"a"`/`1`) |
| `c-inst-iseq-t/f` | agrees with the `seq?` rewrite (`()` true, `[]` false) |
| `c-inst-object/nil-x/nil-class` | `Object` admits everything but `nil`; `nil` class is NPE (`Cannot invoke "java.lang.Class.isInstance(Object)" because "c" is null`) |
| `c-inst-nonclass` | non-class in class position is the module-shape CCE |
| `c-inst-iface/serial/num/comparable/class` | interface/superclass/`Class` membership |
| `c-inst-arity1/3` | JVM: runtime `ArityException` (`.../instance?--N`); the port rejects at compile time (§2.4) |
| `c-inst-macroexpand/resolve` | no macroexpand; resolves (so the port's var is faithful) |
| `c-inst-order` | class position evaluates first (`[:c :x]`) |
| `c-inst-var-*`, `c-inst-ho` | the var agrees with the check, including higher-order use |

Behaviors:

- `(instance? C x)`: `C` a class → membership of
  `x`'s class in `C`'s subclass closure (via §1.3,
  including HostDef and fn rules); `x` `nil` → `false`
  (`c-inst-nil-x`); `C` `nil` → the NPE above
  (`c-inst-nil-class`); `C` a non-class non-descriptor →
  `cce_err(v, "java.lang.Class")`
  (`c-inst-nonclass`).
- `HostVal::Def` in class position folds to its name
  (S13 coherence: S13's `derive` already accepts
  descriptors as class tags). This keeps the natural
  `(instance? Point (Point. 1))` working in the defining
  namespace, where `Point` names the def var, not an
  import (§2.1). Exception: `Struct` descriptors do not
  fold (they are not types — structs construct via the
  `struct` fn, and on the JVM a struct def is a `$Def`
  object, not a class, so class position there is the
  module-shape CCE). A folded name with no registry
  entry (neither static row nor live HostDef) is likewise
  the CCE, not an error about the registry.
- Analyzer (`fx4_host.rs`, surgical): keep the
  `SeqQ` rewrite (`(instance? clojure.lang.ISeq X)` →
  `(seq? X)` — the pins, plus `c-inst-iseq-t/f`
  proving the native agrees); 2-arg calls otherwise
  fall through to the var. Reject any other arity of a
  direct qualified call at compile time with
  `Wrong number of args (N) passed to:
  clojure.core/instance?` — this keeps the
  `fx4_macroexpand.rs` `compile_err` pins green
  (`(clojure.core/instance? 1)` must not compile); it
  phase-diverges from the JVM's runtime `ArityException`
  (§2.4).
- `(clojure.core/instance? String 1)` stays a compile
  error via unresolvable `String`: there are no default
  imports (§1.7), so the pin needs no special case.
- First-class use (`apply`, `map`, `resolve`) goes
  through the var with standard runtime arity errors
  (`clojure.core/instance?`, no `--N` — the JVM's
  `--N` is a defn artifact, §2.4).

### 1.6 `cast` and `class?`

`(cast C x)` is `Class.cast` over the lattice;
`(class? x)` is `(instance? Class x)`.

| probe IDs | pins |
|---|---|
| `c-cast-ok/nil-x/iface/class` | identity on success; `nil` passes through; interfaces and `Class` work |
| `c-cast-miss` | `Cannot cast java.lang.Long to java.lang.String` (exactly `NativeError::cast`) |
| `c-cast-nil-c` | NPE (`Cannot invoke "java.lang.Class.cast(Object)" because "c" is null`) |
| `c-cast-nonclass-c` | non-class `c` is the module-shape CCE |
| `c-cast-arity1`, `c-cast-order` | runtime arity; `c` evaluates first |
| `c-class?-t/f-str/f-nil/f-iface` | true for classes (interfaces included), false otherwise |
| `c-class?-arity0/2` | runtime `ArityException`s |

Behaviors:

- `(cast C x)`: `C` a class and `x` a member (or
  `nil`) → `x`; member mismatch → `NativeError::cast`
  (`Cannot cast {from} to {to}` with `jvm_class_of`
  names — byte-equal to `Class.cast`, `c-cast-miss`);
  `C` `nil` → the NPE above; `C` a non-class
  non-descriptor → `cce_err(v, "java.lang.Class")`.
  Descriptors fold (§1.5). Evaluation order is
  left-to-right (`c-cast-order`).
- `(class? x)`: `true` iff `x` is a `Value::Class`
  (interfaces are classes, `c-class?-f-iface`).
  Descriptors are not classes (`false`); classes of
  descriptors are (`(class? (class (Point. 1)))` →
  `true`).

### 1.7 `import`

A port `core.clj` macro (JVM-faithful: redefinable,
referable, excludable) expanding to a `do` of
`__import*` calls. (`clojure.core/import*` is compiler
magic on the JVM — it does not resolve,
`i-import*-resolve`, `i-import*-resolve-q` — so the
port uses a hidden `__import*` native in the standing
of `__new*`: unlisted in the matrix, callable
cross-namespace so macro expansions analyze.)

| probe IDs | pins |
|---|---|
| `i-expand-quote/bare/list/list-bare/multi` | quote-stripping; `(pkg C1 C2)` fan-out; multi-spec `do` |
| `i-expand-str`, `i-import-nil/num/badlist/lonely/empty/slash/qual-pkg/qual-cls/list-str` | validation: each spec is an unqualified symbol or a sequential of ≥2 unqualified symbols, else the spec `ExceptionInfo` |
| `i-import-exdata` | the JVM's ex-data is spec internals (`:clojure.spec.alpha/*`); the port carries `nil` |
| `i-ret`, `i-import-multi-ret`, `i-dup` | value is the last class; same-class re-import is a no-op |
| `i-unknown`, `i-unknown-list`, `i-import-package` | `ClassNotFoundException` with the name |
| `i-conflict-class` | different class, same simple name: `IllegalStateException` (`Date already refers to: class java.util.Date in namespace: imp2`) |
| `i-conflict-def`, `i-import-over-refer` | over a var mapping: module-shape CCE (`Var` → `Class`) |

Behaviors:

- Expansion: `(import 'java.util.Date)` →
  `(do (clojure.core/__import* "java.util.Date"))`
  (`i-expand-quote`); `(import '(java.util Date
  Calendar))` fans out per class (`i-expand-list`);
  multiple specs concatenate in order
  (`i-expand-multi`). Quote-stripping matches `core.clj`
  (`(and (seq? %) (= 'quote (first %)))`).
- Validation (in the macro, before emitting): each
  quote-stripped spec must be an unqualified symbol
  (namespace-qualified symbols are rejected,
  `i-import-slash`) or a sequential of ≥2 symbols whose
  every element is an unqualified symbol (a qualified
  package, `i-import-qual-pkg`, a qualified class,
  `i-import-qual-cls`, or a non-symbol element,
  `i-import-list-str`, fails it, as do
  `i-import-lonely`, `i-import-empty`, `i-import-badlist`).
  Anything else — strings, numbers, `nil` — throws
  `(ex-info "Call to clojure.core/import did not
  conform to spec." nil)` (`i-expand-str`,
  `i-import-nil`, `i-import-num`), the port's `ns`
  spec-error shape with nil data (the JVM's data is
  spec internals, `i-import-exdata`, unportable by
  design).
- `__import*` (one string arg): resolve the FQN —
  static table first, then a HostDef of that qualified
  name (§1.8) — and record `simple-name → Class` in
  the current namespace, returning the class. Unknown
  names (including packages, `i-import-package`) are
  `ClassNotFoundException` with the bare name
  (`i-unknown`, `i-unknown-list`). This needs a
  `ThrowClass::ClassNotFoundException` variant (the
  analyzer already names the class for catch clauses;
  the runtime lacks it).
- Conflicts: same simple name bound to an equal class
  → no-op success (`i-dup`); bound to a different
  class → `IllegalStateException` (`{name} already
  refers to: {existing} in namespace: {ns}` where the
  existing class renders via `str`, i.e. `class ` /
  `interface ` prefixed, `i-conflict-class`); bound to
  a var (def or refer) → `cce_err(var,
  "java.lang.Class")` (`i-conflict-def`,
  `i-import-over-refer`). One principled exception: a
  var holding a `HostVal::Def` of the SAME qualified
  name is a no-op success returning the class (§2.1 —
  importing a type in its defining namespace must not
  CCE when the JVM no-ops).
- No default imports: a fresh namespace's import table
  is empty. The JVM installs ~100 (`i-defaults` pins
  the whole list); the port installs none because the
  pinned `(clojure.core/instance? String 1)` compile
  error (§1.5) requires unqualified `String` to stay
  unresolvable in a fresh `ns` (§2.1). `ns-imports` of
  a fresh namespace is therefore `{}` (implemented with
  this fenced difference, §1.15).
- `ns` keeps rejecting `:import` (the `macros.rs`
  `ns_errors` pin, §2.3): only the direct macro exists
  (§2.4).

### 1.8 The import table

Each namespace carries `imports: Map<simple-name,
ClassVal>`, consulted LAST (after interns, then
refers) by every unqualified resolver.

| probe IDs | pins |
|---|---|
| `i-ns-imports*` (table, values, coercions) | the `ns-imports` var — specified in §1.13, not here |
| `i-resolve`, `i-ns-resolve`, `i-value-pos` | imports resolve to the class (not a var) in `resolve`/`ns-resolve`/value position |
| `i-construct` | `(Date. 0)` constructs through the import (oracle constructs; the port answers `Not a type`, below) |
| `i-ns-map`, `i-ns-publics`, `i-ns-interns` | imports are in `ns-map`, not in `ns-publics`/`ns-interns` |
| `i-def-over-import`, `i-def-over-import-after` | `def` over an import is a compile error (`Expecting var, but Date is mapped to class java.util.Date`); nothing is interned |
| `i-intern-over-import`, `i-refer-over-import`, `i-refer-removes-import`, `i-refer-warn` | `intern`/`refer` replace the import (removal, not shadow; the port stays silent, §2.4) |
| `i-unmap` | `ns-unmap` drops the import |
| `i-deftype-import`, `i-deftype-construct` | HostDef FQNs import and construct |
| `i-fqn-eval` | JVM-only: dotted FQNs evaluate to classes (the port keeps rejecting them, §2.4) |
| `i-ns-clause` | JVM-only: `ns` `:import` works (the port keeps the spec error — see §1.13) |

Behaviors:

- Consumers (all consulted after interns+refers):
  `resolve`/`ns-resolve` (the `do_resolve` path),
  analyzer value/operator resolution, `lookup_root`
  (unqualified construction `(Point.)` and interface
  position), `ns-map` (layered under refers/interns),
  `ns-imports` (the table alone, as a sorted map).
- `ns-unmap` drops the import entry alongside
  intern/refer entries (`i-unmap`); `resolve` then
  misses.
- `def` over an imported name is a compile error:
  `parse_def` checks the import table and fails with
  cause `RuntimeException | Expecting var, but {name}
  is mapped to class {fqn}` (`i-def-over-import`),
  leaving no var (`i-def-over-import-after`).
- `intern`/`refer` over an import replace it (the entry
  is removed: `i-intern-over-import` proves removal for
  `intern`, and `i-refer-removes-import`
  (`[#'impg/Date nil]`) proves it for `refer` —
  `ns-imports` drops the name and `resolve` answers the
  var). The JVM warns on
  `*err*` (`i-intern-over-import`, `i-refer-warn` capture
  the texts); the port replaces silently like all its
  other replaces (§2.4).
- No auto-import: `deftype`/`defrecord`/`definterface`/
  `gen-class` do not touch the table (the JVM does;
  the port's existing `ns-imports`/`ns-map` pins forbid
  it, §2.1). `def` keeps interning the descriptor var;
  `(Point.)` keeps resolving through it.
- `deftype` does not consult the table beyond the
  `def` rule above; `catch` keeps its curated
  `resolve_classname` rules (imports do not feed
  `catch` — follow-up, §2.2).
- Dotted bare names (`java.lang.String`,
  `foo-bar.R2`) stay unresolvable in value position
  (`i-fqn-eval` is oracle-only; the
  `stage1_descriptor_level` pin holds). Classes are
  named via imports, via `type`/`class` values, or via
  `(import ...)` return values — never via bare FQNs.
- `__new*` gains a `Value::Class` arm: resolve the
  name to a `HostDef` (dotted split + registry, reusing
  the import-or-var lookup) and construct; a JDK name
  with no `HostDef` is `IAE | Not a type: {name}`
  (mirroring `not_a_type`); an interface def is the
  existing `No matching ctor found for class {name}`.
  Interface position (`resolve_iface`) gains the same
  `Class` arm with its existing miss shapes.

### 1.9 `identical?`

A resolving binary native. The analyzer keeps owning
direct qualified calls: the 2-arg rewrite now lowers to
the var (was: `=`), and wrong arities are compile
errors. Consistency between the rewrite and the var is
by construction (one code path); the value pins stay
green because immediates compare by value.

| probe IDs | pins |
|---|---|
| `d-macroexpand`, `d-inline-arities`, `d-has-inline` | no macroexpand; `:inline-arities #{2}` — the JVM's 2-arg-only inline is the rewrite's warrant |
| `d-nil`–`d-lazy-same` (28 rows) | per-kind identity table (below) |
| `d-arity0/1/3` | JVM: runtime `ArityException`s (`clojure.core/identical?`); the port rejects direct calls at compile time (§2.4) |
| `d-apply-t/f`, `d-apply-arity`, `d-ho-str/vec` | the var path (runtime arities, higher-order) |
| `d-eval-order` | left-to-right (`[:x :y]`) |
| `d-eq-vs-ident-*` | `=` vs `identical?` splits (vec, big long, built string) |

Behaviors:

- Analyzer (`fx4_host.rs`, surgical):
  `classify_identical` keeps matching 2-arg direct
  qualified calls, but `lower_host` now emits the
  resolving var call instead of `=`. The
  `(identical? 1 1)` → `true` / `(identical? 1 2)` →
  `false` pins stay green (integer immediates). Any
  other arity of a direct qualified call is a compile
  error (`Wrong number of args (N) passed to:
  clojure.core/identical?`), keeping the two
  `compile_err` pins green; it phase-diverges from the
  JVM's runtime `ArityException` (§2.4).
- The native (`identical?` var): two rules by kind.
  Value kinds compare by value (`=`): `Nil`, `Bool`,
  `Int`, `Double`, `BigInt`, `BigDec`, `Ratio`, `Char`
  (immediates — no boxes exist), `Str`, `Symbol`,
  `Keyword` (the port never interns, so pointer
  identity would be meaningless), and the boxed
  singletons `MapEntry`, `TaggedLiteral`,
  `ReaderConditional` (no stable allocation to point
  at; `=` gives the right answer for the same binding
  and only over-approximates distinct-but-equal).
  Identity kinds compare allocation/cell identity:
  `List`/`Vector`/`Map`/`Set` (new per-value ids, see
  below — `rpds` exposes no pointer), `Regex`
  (`Arc::ptr_eq`), `Var`/`Unbound`/`Throwable` and all
  refs/transients (existing ids — same answers as
  `=`), `Extension`/`HostType`/`MultiFn`
  (`Arc::ptr_eq` — NOT `dyn_eq`: seq equality is
  value-based, but seq identity is not), `Class`
  (by name — classes are singletons, §1.2).
- Persistent collections need fresh identity ids: add
  a `u64` id field (static `AtomicU64` counters, the
  `ATOM_ID` precedent) to the port's
  `List`/`Vector`/`Map`/`Set`, assigned at every
  construction, preserved by `Clone` (derived). This
  is the only shared-struct change in the slice; it is
  additive and behavior-neutral except through
  `identical?` (§2.3).
- JVM divergences, all forced and all pinned: big
  longs, doubles, uncached chars, bigints/bigdecimals,
  ratios (`d-int-big`, `d-double`, `d-char-uncached`,
  `d-bigint`, `d-bigdec`, `d-ratio` — the JVM is
  cache-dependent, the port compares values);
  built strings (`d-str-built` — the port has no
  interning); distinct-but-equal entries/tagged
  (over-approximation, above); symbols (`d-sym` —
  value-compared). `d-str-lit`/`d-kw` match (value
  comparison agrees with interning on literals).
- First-class use resolves to the var with runtime
  `ArityException`s (`d-apply-arity`); higher-order
  and eval order match the JVM (`d-ho-str`,
  `d-ho-vec`, `d-eval-order`).

### 1.10 `..` and the member surface

A port `core.clj` macro (JVM-faithful: redefinable,
referable, excludable). It cannot emit the JVM's `.`
forms — the port has no general `.` (bare `.` stays
unresolvable) — so it emits the hidden-native calls the
dot-forms read as: `(.. x (m a))` →
`(clojure.core/__meth* 'm x a)`, `(.. x f)` →
`(clojure.core/__field* 'f x)`, threading nests left.
Same evaluation, different macroexpand text (pinned
divergence, §2.1).

| probe IDs | pins |
|---|---|
| `m-expand-1/2/full/field` | JVM expansion shapes (oracle; the port emits `__meth*`/`__field*` calls) |
| `m-expand-arity0/1`, `m-arity1` | JVM arity errors (`.../-DOT--DOT-`); the port uses its standard macro-arity shape naming `clojure.core/..` |
| `m-thread-1/2/field` | threading + field/call chaining |
| `m-nil-tostring/hash/eq/getclass` | nil + resolving member → NPE (`Cannot invoke "Object.getClass()" because "target" is null`) |
| `m-nil-miss0/miss1/field` | JVM NPEs uniformly; the port keeps miss shapes here (forced by the `(.foo nil)` pin, below) |
| `m-miss-1`, `m-cls-miss`, gate misses (`m-isempty-long-miss`, `m-first-str-miss/map-miss`, `m-named-var-miss*`, `m-realized-long-miss`, `m-isempty-deftype-miss/class-miss`, `m-first-sortedset-miss`) | `No matching method {m} found taking {n} args for class {C}` (IAE, `no_method`) |
| `m-miss-0` | JVM method-shape; the port answers field-shape for 0-arg misses (the `dtor_method_on_other_values` pin) |
| `m-miss-field` | `No matching field found: {f} for class {C}` (IAE, `no_field`) |
| `m-cls-symbol-target` | oracle-only: the JVM treats a class-name symbol target as a static call (`No matching method getName ... for class java.lang.String`); the port has no statics, so an imported `String` target takes the instance path (`m-cls-getname`) |
| `m-deftype-field/method`, `m-record-field` | HostDef fields + methods via the existing `__field*`/`__meth*` dispatch |
| `m-strlen-oo`, `m-substring-oo`, `m-docstring-oo` | oracle-only: real JDK members outside the emulated surface (follow-up, §2.2) |

Member surface (consulted by `__meth*` before the
host dispatch; `__field*` is unchanged — fields stay
host-only):

- Universal (`Object`): `toString` (≡ `str`, below),
  `equals` (below), `hashCode` (≡ `(hash x)` —
  `m-hash-str/int/vec/long` pin String/Long/Vector
  agreement), `getClass` (≡ `(class x)` — `m-getclass`,
  `m-cls-getclass`).
- `Class` (target a `Value::Class`): `getName`
  (`m-cls-getname`), `getSimpleName` (reuse
  `simple_name_of`, §2.3 — `m-cls-simple`,
  `m-cls-simple-nested`, `m-cls-simple-fn`; the
  synthetic reify names keep their `$`-segment —
  no JVM oracle exists for the port-invented stable
  names),
  `getSuperclass` (first base unless interface or
  baseless → `nil`; `m-cls-super/super-iface/
  super-object`), `getInterfaces` (remaining bases as
  a vector, lattice order — `m-cls-ifaces-order`; the
  JVM returns an array, the port a vector),
  `isInterface` (`m-cls-isiface-t/f`), `isArray`
  (always `false` — array classes have no values yet,
  S15 follow-up §2.2; `m-cls-isarray`), `isInstance`
  (membership; `nil` arg
  → `false`, `m-cls-isinstance-t/f/nil`),
  `isAssignableFrom` (lattice closure; non-class arg →
  short CCE, `m-cls-assign-nonclass`; `nil` arg → NPE
  with empty message — the JVM's message is nil,
  `m-cls-assign-nil`, §2.4), `cast` (≡ `cast`, §1.6 —
  `m-cls-cast-ok`, `m-cls-cast-miss` byte-matches
  `c-cast-miss`).
- `String`: `isEmpty`, `isBlank` (`m-isempty-str-*`,
  `m-isblank-*` incl. whitespace `m-isblank-ws`).
- `Named` (keywords, symbols): `getName`,
  `getNamespace` (nil-namespace → `nil`; vars are not
  `Named` — `m-named-*`).
- `List` (every lattice `List` member: vectors, lists,
  subvecs, ranges, conses, lazies, entries, all seq
  subkinds): `getFirst`/`getLast` by position
  (`m-first-vec/list/range/subvec/cons/lazy`,
  `m-last-vec`, `m-first/last-entry`); empty →
  `NoSuchElementException` with empty message (new
  `ThrowClass` variant; the JVM's message is nil,
  `m-first-empty`, §2.4). Sets, maps, strings miss
  (`m-first-str-miss/map-miss/sortedset-miss`).
- `Collection`/`Map` (everything with a count, incl.
  records, structs, entries, sorted): `isEmpty`
  (`m-isempty-*`; records/structs are `Map`s —
  `m-isempty-record-f/struct-f`; deftypes and classes
  miss — `m-isempty-deftype-miss/class-miss`).
- `MapEntry`: `getKey`/`getValue` (`m-entry-key/value`).
- `Throwable`: `getMessage` (nil-able — `t-msg-nil`),
  `getCause` (nil-able — `t-cause-nil`), `getLocalized-
  Message` (≡ message, no i18n — `t-loc`);
  `t-cause-msg/class` pin cause chains.
- `ExceptionInfo`: `getData` (`m-exinfo-data`).
- `IPending` (delays, futures, promises, lazy seqs):
  `isRealized` (`m-realized-*`).
- Writers: `close`/`flush` (0-arg, void — the §1.14
  probes call `(.close pw)`/`(.flush pw)`; never bean
  keys, §1.11). On other writers both are silent
  no-ops returning nil (`(.. *out* (flush))` never
  flushes stdout, unlike `(flush)`).
- HostDef instances/descriptors: the existing dispatch
  (fields, methods, descriptor misses, gen-class
  forwarding) — `m-deftype-field/method`,
  `m-record-field`. A 0-arg `__meth*` on a host target
  keeps reading field-first (pre-existing, JVM-same).
  Universals shadow host members: `equals`/`hashCode`/
  `getClass` answer the surface even when a host type
  defines the member (a deftype `(equals [this x] …)`
  never runs — pinned divergence, no oracle rows
  define custom universals; deferring to exact host
  matches is a follow-up, §2.2). `toString`
  re-dispatches through `str`, so custom `toString`
  methods run (`m-deftype-method`) while gen-class
  `toString` sigs answer the default.

Nil target (R1 rule): resolve `(member, arity)`
against the surface IGNORING the target's kind — a hit
NPEs with the oracle text (`m-nil-tostring/hash/eq/
getclass`); anything else keeps today's miss shapes
(`null` as the class: 0-arg → field-shape, n-arg →
method-shape). So `(.. nil (isEmpty))` NPEs (member
real) while `(.. nil (nosuch_xyz))` field-misses and
`(.. nil nosuch_xyz)` field-misses — all three NPE on
the JVM (`m-nil-miss0/miss1/field`), divergences forced
by the `(.foo nil)` pin (§2.1). Fields on nil always
miss (no field is universal).

`toString` ≡ `str_of_one` (one rule, no second
renderer, §2.1). Oracle values per kind:

| rows | oracle | port |
|---|---|---|
| `m-str-string/int/double/bool/char/bigint/bigdec/ratio/kw/sym` | the scalar prints | MATCH |
| `m-str-vec/map/set/list/entry/subvec/sortedmap/sortedset/range/cons/struct` | the print forms | MATCH (structs via the `host_str` map arm) |
| `m-str-var` | `#'clojure.core/map` | MATCH |
| `m-str-class` | `class java.lang.String` | MATCH (§1.2) |
| `m-str-lazy` | `LazySeq@L` | MATCH (shape; the port's hash is content-derived, `lazy_seq_str`) |
| `m-str-atom/delay/promise/future/agent/volatile` | `Class@L` | MATCH (shape; via `java_debug`) |
| `m-str-multifn` | `MultiFn@L` | MATCH (shape; via `multifn_str`) |
| `m-str-record/deftype` | `user.QS2@L` (default rendering — records get no map `toString`) | MATCH (shape; via `fallback_str`) |
| `m-deftype-method` | custom `toString` runs (`MM!`) | MATCH (`host_str` runs the method) |
| `m-str-regex` | `x` (bare source) | DIVERGES (`#"x"`, `print_regex` always quotes) |
| `m-str-fn` | `display@L` | DIVERGES (`#object[...]`, Extension pr) |
| `m-str-ns` | `user` | DIVERGES (`#object[Namespace ...]`) |
| `m-str-exinfo` | `class: message data` | DIVERGES (`#error {...}`, `print_throwable`) |
| `m-str-tagged/readercond` | `Class@L` | DIVERGES (print forms `#a 1`, `#?(...)`) |
| `m-str-transient-vec/map` | `Class@L` | DIVERGES (`#object[...]`, default arm — unlike the atom family) |
| `m-str-reader/chunk/matcher` | default / custom `Matcher[...]` | DIVERGES (`#object[...]`) |
| `m-str-eduction/reduced/iteration/accessor` | `Class@L` / reify | DIVERGES (Extension pr forms) |

`equals` is three rules, in order (`m-eq-*`):

1. Either side numeric-tower (Long/Double/BigInt/
   BigDec/Ratio): Java-exact — same class plus Java
   equality. Cross-type → `false` (`m-eq-cross`,
   `m-eq-bigint-long`, `m-eq-long-bigint` — where port
   `=` says true); `NaN` equals `NaN` (`m-eq-nan`);
   `+0.0` ≠ `-0.0` (`m-eq-negzero`); `BigDecimal`
   compares scale-sensitively (`m-eq-bigdec-scale`
   → `false`); ratios compare normalized
   (`m-eq-ratio` → `true`).
2. Else either side a record instance: map-content
   equality — record→content, map→self,
   struct→content, else `false`. Record-vs-map
   (`m-eq-record-map`), map-vs-record
   (`m-eq-map-record`), cross-record-type
   (`m-eq-record-diff-type`), and record-vs-struct
   (`m-eq-record-struct`) are all `true` on the JVM —
   where port `=` says `false` (pinned by `rec-eq-map`).
3. Else port `=` (`m-eq-tt/tf/nil-arg/vec/vec-list/
   entry-vec/lazy-list/range-vec/lazy-vec/cons-vec/
   set-sorted/map-sorted/record-record/record-diff/
   deftype-same/deftype-diff` all agree with it —
   including deftype identity).

Arity misses on real members keep `no_method` shapes
(`m-eq-arity0/2`).

### 1.11 `bean`

`(bean x)` is a 1-arity native returning a plain eager
map: `:class` plus one entry per 0-arg getter in the
§1.10 surface, minus `getClass`. Keys are decapitalized
(`getCount` → `:count`, JavaBeans rule: a leading
all-caps run is preserved — `getURL` → `:URL`,
`m-bean-cap`). Getter values are computed EAGERLY at
`bean` time (the JVM re-invokes per access —
`b-lazy` → `[1 2]`; the port answers `[1 1]`, §2.1).
`nil` → NPE naming `x` (`Cannot invoke
"Object.getClass()" because "x" is null` — note: NOT
the `..` `target` text, `b-nil`). Eagerness throws at
`bean` time: `bean` of an empty `List` throws
`NoSuchElementException` (empty message) evaluating
`:first`/`:last` — the JVM returns a 4-key proxy map
and throws `InvocationTargetException` (wrapping the
NSEE) only when the keys are touched (pinned
divergence, §2.4). Missing keys read
`nil`/default, `contains?`/`count`/`assoc`/`meta`
behave as for plain maps (`b-missing/contains/count/
assoc/meta`).

| probe IDs | pins |
|---|---|
| `b-keys-str`, `b-empty-t/f`, `b-bytes`, `b-class-val` | string keys; `:bytes` is oracle-only (needs byte arrays — S15, §2.2) |
| `b-keys-long` | scalars → `:class` only |
| `b-keys-kw/sym` | `:class :name :namespace` (`:namespace` present even when nil) |
| `b-keys-vec/map`, `b-keys-lazy/range/cons/subvec/entry/sortedmap/sortedset` | `:class :empty` + List `:first :last` (+ `:realized` for lazy only — ranges/conses are not `IPending`) |
| `b-keys-class/iface`, `b-cls-vals` | oracle 48 reflection keys; the port keeps the 7 computable ones (below) |
| `b-record`, `b-keys-struct/structdef/deftype` | records/structs → `:class :empty`; deftypes/structdefs → `:class` |
| `m-det-bean-keys`, `b-lazy` | HostDef 0-arg `get*`/`is*` methods become keys (`:count`), called eagerly |
| `m-bean-cap` | `:URL` — full JavaBeans decapitalize |
| `b-keys-sym/var/ns/pattern/matcher/transient/reader/future/promise/delay/atom/agent/volatile/exinfo/multifn/eduction/reduced/chunk/iteration/accessor` | per-kind oracle keys; the port column (below) keeps the computable subset |
| `b-nil` | the `x` NPE text |

Port key table (oracle → port):

- String: 4 → `:blank :class :empty` (`:bytes`
  needs S15 arrays).
- Scalars (int/double/bool/char/bigint/bigdec/ratio):
  `:class` (`b-keys-long`; the rest rule-derived — no
  scalar has a `get*`/`is*` getter).
- Keywords/symbols: `:class :name :namespace` (all
  three always).
- Vectors/lists/subvecs/ranges/conses/all seq
  subkinds: `:class :empty :first :last`.
- Lazy seqs: + `:realized` (`b-keys-lazy`).
- Maps/records/structs/sets/sorted: `:class :empty`.
- Entries: `:class :empty :first :key :last :value`
  (`b-keys-entry`).
- Classes/interfaces: `:array :class :interface
  :interfaces :name :simpleName :superclass`
  (`:interfaces` a vector of Classes; the other 41
  reflection keys need a real JVM — follow-up, §2.2).
- Throwables: `:cause :class :localizedMessage
  :message` + `:data` for `ExceptionInfo` only
  (`:stackTrace`/`:suppressed` need real stacks —
  follow-up).
- Delays/futures/promises: `:class :realized`.
- Writers (`*out*`/`*err*`/buffers/`PrintWriter-on`):
  `:class` — `close`/`flush` are void members, never
  keys.
- Atoms/agents/volatiles/reduced/multifns/patterns/
  matchers/transients/readers/eductions/chunks/
  iterations/accessors/fns/namespaces/vars/structdefs/
  deftypes-without-getters: `:class`. (The oracle's
  `:validator`/`:watches`/`:error*`/`:methodTable`/
  `:aliases`/`:mappings`/`:lineNumber`/`:done` and
  friends are follow-up surface, §2.2; the JVM's
  `bean`-on-Var `CCE` (`b-keys-var`) is an accidental
  getter — the port answers `:class`.)
- HostDef instances (any kind incl. records): `:class`
  (+ `:empty` for records) plus one key per 0-arg
  `get[A-Z]*`/`is[A-Z]*` method, values called eagerly
  (`m-det-bean-keys`, `b-lazy`). Fields are never keys
  (`b-keys-deftype` → `(":class")` despite `[x]`).

### 1.12 `memfn`

A port `core.clj` macro (same standing as `..`):
`(memfn name & args)` → `(fn* [target args...]
(__meth* 'name target args...))`, where `target` is a
gensym carrying `(meta name)` — a `^String` hint lands
as `:tag` on the target symbol
(`m-memfn-hint-meta` → `{:tag String}`), exactly like
the JVM. The JVM emits `(. target (name args...))`
(`m-memfn-expand/expand-args/expand-hint`); the port
emits the `__meth*` call (pinned divergence, §2.1 —
same evaluation). Calls, arities, and the 0-arg
`ArityException` mirror the JVM (`m-memfn-call/
call-args/arity0`).

| probe IDs | pins |
|---|---|
| `m-memfn-expand/expand-args/expand-hint` | JVM shapes (oracle; the port emits `__meth*`) |
| `m-memfn-hint-meta` | the hint rule (`:tag` on the target) |
| `m-memfn-call/call-args` | calls through the surface |
| `m-memfn-arity0` | the 0-arg `ArityException` |

### 1.13 `ns-imports`

`(ns-imports ns)` answers the §1.8 table as a sorted
map `{simple-symbol → Class}`, coerced through the
`ns-*` family's `the_ns_name` (ns-object ok, string →
short CCE, nil → the JVM-exact `key` NPE, missing →
`No namespace: X found`). A fresh namespace answers
`{}` (no default imports — the §1.7 fenced
divergence; the JVM's ~100 defaults are oracle-only).

| probe IDs | pins |
|---|---|
| `i-ns-imports`, `i-ns-imports-val` | the table as a sorted map; values are Classes |
| `i-ns-imports-nsobj` | ns-object arg works |
| `i-ns-imports-str` | string arg → short CCE (`String` → `Symbol`; the oracle shows the module shape) |
| `i-ns-imports-nil` | nil arg → NPE (`Cannot invoke "Object.hashCode()" because "key" is null` — byte-exact via `NIL_KEY_NPE`) |
| `i-ns-imports-missing` | missing ns → `Exception \| No namespace: nosuchnsxyz found` (via `the_ns_name`) |
| `i-defaults` | oracle-only: the JVM's ~100 default imports (the port installs none) |
| `i-ns-clause` | oracle-only: the JVM's `ns` `:import` feeds the table (the port keeps the `ns` spec error, §1.7) |

Behaviors:

- Return shape: a sorted map (port-sorted; the probe
  sorts, `i-ns-imports`), values the `Class` objects
  (`i-ns-imports-val`).
- Coercion is exactly `the_ns_name` — no new error
  paths (§2.3).
- Fresh-ns `{}` flips the `ns-imports` matrix row
  from `divergent` to `implemented` with the fenced
  difference (§1.15).

### 1.14 `PrintWriter-on`

A new `WriterKind` wrapping a string accumulator: writes
append to a buffer; flush delivers the buffer to
`flush-fn` (skipped when empty); close flushes then
calls `close-fn` once. `autoflush?` is accepted and
stored but never consulted — exactly like the JVM at
the Clojure-printing level (`w-noflush-auto` proves the
JVM ignores it there too; it only fires on direct
`.println` interop calls, `w-direct-println-auto`,
which the port has no surface for).

| probe IDs | pins |
|---|---|
| `w-class`, `w-arity3-class`, `w-print` | `java.io.PrintWriter`, `#object` print shape |
| `w-arity1/4` | runtime `ArityException`s (`clojure.core/PrintWriter-on`) |
| `w-write-flush`, `w-println`, `w-prn-to`, `w-flush-call` | writes buffer; flush/`(flush)`/println-flush deliver |
| `w-flush-empty` | no call on empty buffer |
| `w-close`, `w-close-nil` | flush-then-close; nil `close-fn` ok |
| `w-flush-default`, `w-noflush-noauto/flush-noauto` | println flushes iff `*flush-on-newline*` (default true) |
| `w-noflush-auto`, `w-print-newline-auto` | `autoflush?` ignored at Clojure level |
| `w-direct-println-auto/noauto`, `w-direct-print-auto` | oracle-only: the interop mechanism the port lacks |
| `w-nonfn-construct/flush` | construction ok; flush-time CCE (port: short `Cannot cast ... to clojure.lang.IFn`, §2.4) |
| `w-close-twice` | `close-fn` runs once |
| `w-write-after-close` | writes after close are silently dropped |
| `w-flush-ret/close-ret` | both return `nil` |

Behaviors:

- Constructor: 2-arity defaults `autoflush?` to false.
  The writer's class is `java.io.PrintWriter`
  (`w-class`) and it prints as
  `#object[java.io.PrintWriter 0xH
  "java.io.PrintWriter@L"]` (`w-print`) — the existing
  `Writer` print path with the new kind, no new code.
- `flush_out` (and hence `flush`, `println` with
  `*flush-on-newline*`, `prn`) must fire `flush-fn`
  for this kind — the one integration point (§2.3).
  Empty buffer → no call (`w-flush-empty`).
- Close: flush first, then `close-fn` unless nil
  (`w-close`, `w-close-nil`); a second close is a
  no-op (`w-close-twice` — track a closed flag). If
  `flush-fn` throws during close, `close-fn` is
  skipped and the writer stays closed (a second close
  still no-ops).
  Writes after close are dropped, never throw
  (`w-write-after-close`). Flush and close return
  `nil` (`w-flush-ret`, `w-close-ret`).
- A non-fn `flush-fn` fails at flush time with the
  port's standard invoke error (`Cannot cast {from} to
  clojure.lang.IFn`, short shape — the JVM's is the
  module shape, `w-nonfn-flush`, §2.4).
- `autoflush?` true is stored and observable nowhere:
  `w-noflush-auto` (`[]`) and `w-print-newline-auto`
  (`[]`) pin the JVM behavior the port replicates by
  construction (all port printing funnels through
  string writes, which never autoflush on either
  system).

### 1.15 Matrix edits (row-flip plan — NOT APPLIED HERE)

NOTE (count correction): the issue says "twelve
out-of-scope rows" flipped "plus the ns-imports
divergent row (thirteen rows)". The issue's own name
list has thirteen out-of-scope names, and the current
`rust/core-coverage.md` (verified by grep, same file in
this worktree and `origin/master`) holds all thirteen
as `out-of-scope` plus `ns-imports` as `divergent`.
The flip is FOURTEEN rows, one per var below. (The
issue's "mirror from PR #41" is likewise stale: PR #41
landed long ago — the implementer mirrors row positions
from current `origin/master`, where every row below
already exists in place.)

| # | var | line | now | covering tests (`classemu.rs`) |
|---|---|---|---|---|
| 1 | `ns-imports` | 63 | divergent | `ns_imports_*`: explicit table (sorted), fresh-ns `{}`, coercions, conflicts |
| 2 | `instance?` | 481 | out-of-scope | `instance_*`: BDD, lattice membership, nil/nonclass/descriptor, var use, analyzer arities |
| 3 | `type` | 482 | out-of-scope | `type_*`: `:type` rule (incl. falsy fall-through), per-kind, arities |
| 4 | `class` | 483 | out-of-scope | `class_*`: per-kind incl. names, `nil`, class-of-class, arities |
| 5 | `bases` | 484 | out-of-scope | `bases_*`: order/shape/nil/CCE/arity + the 214-row lattice test + dynamic rules |
| 6 | `supers` | 485 | out-of-scope | `supers_*`: set-shape/nil/CCE/arity, HostDef walks, visited-safety |
| 7 | `import` | 504 | out-of-scope | `import_*`: expansion, validation, return, conflicts, CNFE, no-defaults |
| 8 | `..` | 505 | out-of-scope | `dotdot_*`: expansion, threading, nil rule, arities, BDD |
| 9 | `identical?` | 574 | out-of-scope | `identical_*`: per-kind table, rewrite agreement, var use, arities |
| 10 | `PrintWriter-on` | 690 | out-of-scope | `printwriter_*`: shape, buffer/flush/close, autoflush-dead, non-fn |
| 11 | `bean` | 713 | out-of-scope | `bean_*`: per-kind key tables, eagerness, nil, map-ness |
| 12 | `cast` | 730 | out-of-scope | `cast_*`: ok/miss/nil/nonclass/descriptor/order/arity |
| 13 | `class?` | 743 | out-of-scope | `classq_*`: true/false table, interfaces, descriptors, arities |
| 14 | `memfn` | 785 | out-of-scope | `memfn_*`: expansion (`__meth*` call), hint meta, calls, arity |

Each flips in place to `implemented` with a test note
naming its `classemu.rs` tests (house style, §1.1).
Header math (endpoints at spec time: 451/1/240/0 —
re-verify by counting on the merge base; thirteen
`out-of-scope` rows leave and one `divergent` row
leaves, so the header gains fourteen `implemented`).
This spec task edits no matrix rows.

## Part 2 — Notes

### 2.1 Design decisions

1. **New `Class` kind (vs reuse).** A string fails
   `class?`; a var derefs; a `HostVal::Def` covers only
   port-defined types. The lattice must answer
   interface-ness and ordered ancestry for JDK names,
   HostDef names, and fn displays alike — hence
   `ClassVal { name, iface, bases }`. The stored `bases`
   (not re-derived) is forced by fn classes: fixed vs
   variadic is unknowable from the display name.
2. **`bases` declaration order (vs S13's sorted).**
   `(bases String)` returns `Comparable` before
   `CharSequence` (JVM declaration order,
   `l-java.lang.String`). S13 stores ASCII-sorted bases
   because `isa?`/`parents` only need sets (its probes
   sort). Both are right for their consumer; the merge
   follow-up (§2.2) unifies on declaration order.
3. **Descriptors fold (vs CCE).** `instance?`/`cast`/
   `bases`/`supers` accept `HostVal::Def` by name.
   Alternative (strict CCE) would break the natural
   `(instance? Point (Point. 1))` in the defining
   namespace, where `Point` names the def var. S13
   already folds descriptors for `derive` — S16 is
   consistent with the published seam.
4. **No default imports (vs JVM's ~100).** Forced by
   the pinned `(clojure.core/instance? String 1)`
   compile error: any default `String` mapping would
   let it compile. `ns-imports` flips to `implemented`
   with this fenced difference (fresh ns → `{}`).
5. **No auto-import from `deftype` etc. (vs JVM).**
   The JVM maps the type name to the class; the port
   interns a descriptor var (existing pins:
   `stage1_descriptor_level`, `ns-map`/`ns-imports`
   shapes). Auto-importing would collide with the var
   and flip those pins red. Consequence: importing a
   type in its defining ns hits the same-name var —
   resolved as a no-op success (same-type leniency,
   §1.7) rather than the var-CCE, matching the JVM's
   outcome.
6. **`import`/`..`/`memfn` as `core.clj` macros (vs
   analyzer builtins).** Real macros are redefinable,
   referable, and excludable like the JVM's; analyzer
   builtins (`vswap!` precedent) are not. Their
   expansions use existing surface (`__import*` hidden
   native; `__meth*`/`__field*` hidden calls — the same
   calls the `.m`/`.-f` dot-forms read as), so no new
   special forms are needed and bare `.` stays
   unresolvable.
7. **`..`/`memfn` expand to `__meth*`/`__field*` calls
   (vs JVM `.` forms).** The port has no general `.`
   (bare `.` is pinned unresolvable), so `..` cannot
   emit `(. x (m))`. It emits the equivalent hidden
   calls (`(clojure.core/__meth* 'm x a)` /
   `(clojure.core/__field* 'f x)`) — same evaluation,
   different macroexpand text (pinned divergence).
8. **Rewrite `identical?` → var (vs keeping `=`).**
   Keeping the `=` lowering would define `identical?`
   as value equality (`(identical? [1] [1])` → true,
   contradicting `d-eq-vs-ident-vec`). Lowering to the
   resolving var keeps the scalar pins green and makes
   rewrite/var consistency structural.
9. **`identical?` value-vs-identity split.** Immediates
   have no boxes (FX4 pins force `(1 1)` → true);
   strings/symbols/keywords are never interned (so
   pointer comparison would be meaningless noise);
   boxed singletons (entry/tagged/readercond) have no
   stable allocation. Everything else compares
   allocation identity — which needs the new
   collection ids (`rpds` exposes no pointer) but
   reuses existing ids/`Arc`s elsewhere.
10. **`..`-`toString` ≡ `str` (vs JVM `toString`).**
    The port's `str` already diverges from JVM
    `toString` for fns/regex/namespaces/throwables
    (pre-existing, other slices' pins). Teaching `..`
    a second rendering would fork the port's string
    story; following `str` keeps one rule
    (`m-str-*` rows document both sides per kind).
11. **Nil-target rule (NPE on hits, miss shapes on
    misses).** The JVM NPEs uniformly
    (`m-nil-*`); the port's `(.foo nil)` field-error
    pin forbids full uniformity. Resolution failures
    keep miss shapes; successful resolutions on `nil`
    NPE with the oracle text — diagnostics match the
    JVM exactly where the member exists.
12. **Emulated member surface (§1.10).** Universal-4
    (`toString` ≡ `str`, `equals`, `hashCode`,
    `getClass`) + Class-9 + lattice-gated getters
    (`String`×2, `Named`×2, `List`×2,
    `Collection`/`Map`×1, `MapEntry`×2, `Throwable`×3,
    `ExceptionInfo`×1, `IPending`×1) + HostDef
    (existing). Included iff shared across
    kinds (or `Object`/`Class` itself) and exactly
    representable, plus `getKey`/`getValue`/`getData`
    to keep the bean-keys ≡ surface-getters identity
    (§2.1.19). Kind-specific stateful getters
    (`Future`, `Reader`, `Namespace` maps, atom/agent
    tables, multifn tables, `:bytes`, `:stackTrace`)
    go to the follow-up with oracle rows already
    pinned.
13. **`bean` is eager (vs JVM lazy).** The JVM
    re-invokes getters per access (`b-lazy` → `[1
    2]`); the port builds a plain map once (`[1 1]`).
    Eager gives full map semantics (`assoc`, defaults,
    `count`) for free — the JVM's own `assoc` on beans
    returns a plain map anyway (`b-assoc`).
14. **`autoflush?` stored-but-dead (vs println-hooked).**
    `w-noflush-auto` proves Clojure-level printing
    never triggers it on the JVM either (only direct
    `.println` interop does, which the port cannot
    express). Hooking it to `println` would DIVERGE.
15. **NativeFn ancestry `[AFunction]`-always (vs
    per-native flags).** Exactness needs a shape flag
    on ~600 install rows across every slice's tables
    (fence-prohibited churn). The divergence is
    confined to `(bases (class <variadic-native>))`
    and `RestFn` membership.
16. **`import` spec error with nil data (vs spec
    internals).** The JVM's ex-data is
    `:clojure.spec.alpha/*` internals (`i-import-exdata`)
    with no port equivalent. Message matches; data is
    `nil` — same observability as the port's `ns` spec
    error (class + message in the chain).
17. **Record/deftype implicits emulated (vs bare
    `[Object]+claimed`).** The JVM adds 8 interfaces to
    every record and `IType` to every deftype
    (`c-rec-bases`, `c-det-bases`). Emulating them is
    exact (oracle-pinned, claimed-first order pinned by
    `c-rec-claimed`/`c-det-claimed`), honest (the port's
    records take `with-meta`, its deftypes reject it —
    matching the JVM), and cheap (two static rows,
    `IRecord` + `IKeywordLookup`). The alternative
    (bare) would make `(instance? IPersistentMap
    record)` false — a visible lie about the port's own
    map-like records.
18. **`..`-equals as `=` + two carve-outs (vs a
    from-scratch `equals`).** Port `=` already agrees
    with JVM `.equals` on 15 rows; the carve-outs are
    exactly where it cannot: numerics (Java-exact —
    cross-type `false`, `NaN` self-equal, `±0`
    distinct, `BigDecimal` scale-sensitive) and records
    (map-content — where port `=` deliberately says
    `false` per `rec-eq-map`). Reusing `=` keeps one
    equality engine; the carve-outs are total (every
    pair hits exactly one branch).
19. **Bean keys ≡ surface getters (vs JVM's
    reflection).** On the JVM, `bean` keys and 0-arg
    `..` members coincide (same reflection). The port
    preserves the identity: every bean key is a §1.10
    member (`getKey`/`getValue`/`getData` were added to
    the surface FOR this), every 0-arg surface member
    is a bean key. Exclusions (`:bytes`, `:stackTrace`,
    `:methodTable`, …) are the same follow-up list on
    both sides.
20. **No `__meth0*` (vs JVM-exact 0-arg miss shapes).**
    The JVM answers method-shape for 0-arg call misses
    (`m-miss-0`); the port's `dtor_method_on_other_
    values` pin mandates field-shape there. A parallel
    native would buy JVM-exact `..` errors at the cost
    of two dispatches with different miss conventions —
    error-path text only. One dispatch, one convention.

### 2.2 Seams and follow-ups

S16 lands complete alone: every behavior in Part 1 is
specified against `origin/master` plus the S16
implementation itself. Cross-slice upgrades become a
follow-up issue (filed by the implementer at land
time), not S16 scope:

- **S13 (hierarchy, landed in wave2):** S16's lattice
  walk and S13's `isa?`/`parents` overlap. S16 vendors
  its own visited-set walk; at the wave2 merge S13's
  `class_of`/`hier_key` learned S16 class values (fold
  by name through the shared lattice — all 41 shared
  static rows re-verified agreeing as sets). The
  follow-up unifies the two walks on declaration-order
  bases (S13 sorts today) and reconciles HostDef
  implicits (S13 encodes none; §1.3's record/deftype
  lists are the JVM-exact input).
- **S14 (names, landed in wave2):** proxy dispatch and
  `proxy-name`/`get-proxy-class` read class values by
  name (merged); `bean` getters dispatch through the
  live proxy cell like `__meth*`. S14 owns display-name
  policy; S16's fn/reify display rules (§1.3) are the
  consumer input.
- **S15 (arrays, landed in wave2):** `isArray` still
  answers `false` unconditionally (`m-cls-isarray` —
  array classes have no values yet); `bean`'s `:bytes`
  stays dropped (`b-bytes` oracle-only); the `a-*`
  probes (18 rows) pin JVM array-class behavior. The
  follow-up gives arrays class values and flips
  `isArray`/adds `:bytes`; S16 needs no changes.
- **Static surface** (`Integer/MAX_VALUE`,
  `Integer/parseInt`, `System/getProperties` —
  `i-static-field/call`, `m-docstring-oo`,
  `m-strlen-oo`, `m-substring-oo`): a later slice.
  `m-cls-symbol-target` pins the JVM's static-call
  treatment of class-name symbols; the port takes the
  instance path (no statics).
- **`catch` class resolution** keeps its curated
  `resolve_classname` rules; imports do not feed
  `catch` (follow-up).
- **Extended member/bean surface:** namespace
  (`:aliases`/`:mappings`/`:name`), reader
  (`:lineNumber`/`:columnNumber`/`:string`,
  `m-reader-line-oo`), future (`:done`/`:cancelled`,
  `m-isdone-future-oo`), atom/agent
  (`:validator`/`:watches`/`:error*`), multifn
  (`:methodTable`/`:preferTable`), throwable
  (`:stackTrace`/`:suppressed`), Class (the other 41
  reflection keys), exotic `NodeSeq` imports, deferring
  universals (`equals`/`hashCode`/`getClass`) to exact
  host member matches (§1.10). Oracle
  rows for all are committed (`b-keys-*`,
  `m-isdone-*`, `m-reader-line-oo`).
- **Queues:** the port has no queue kind, so
  `PersistentQueue` stays out of the lattice; a queue
  slice adds the row (ancestry: `Obj`, `List`, … —
  oracle at that time).

### 2.3 Integration points (for the implementer)

- New `Value::Class` kind + `ClassVal { name, iface,
  bases }` (§1.2); singletons keyed by name
  (equality/hash/`identical?` by name).
- New collection ids on `List`/`Vector`/`Map`/`Set`
  for `identical?` (§1.9) — the only shared-struct
  change; additive (`ATOM_ID` precedent).
- Static lattice table (214 rows verbatim from
  `lattice.out`) + visited-set `supers` walk (§1.3).
- 14 row-flip natives/macros (§1.15), three hidden
  natives (`__import*`, `__meth*`/`__field*`
  extensions — same names, widened dispatch),
  `core.clj` macros (`import`, `..`, `memfn`).
- `WriterKind::PrintWriterOn` + `flush_out` hook
  (§1.14).
- Import table per ns + resolver layering
  (interns→refers→imports) + `parse_def` check (§1.8).
- Analyzer: `instance?` arity gate (keep `SeqQ`,
  2-arg falls through, else compile error, §1.5);
  `identical?` 2-arg lowers to the var (§1.9).
- Reuse: `simple_name_of` (`error.rs`) for
  `getSimpleName`; `NativeError::cast` for
  cast-misses; `jvm_class_of`/`class_name` for
  `class` (plus the two struct carve-outs, §1.4);
  `str_of_one` for `..`-`toString`; `hash` for
  `..`-`hashCode`; port `=` for `..`-`equals`
  branch 3.
- New `ThrowClass` variants: `ClassNotFoundException`
  (§1.7), `NoSuchElementException` (§1.10).
- Pins that must stay green (surveyed): `instance?`/
  `identical?`/`ns`-`:import`/`ns-map`/`ns-imports`
  shapes, dotted-name rejects, `str`/`print` goldens,
  `with-meta` CCEs, `(.foo nil)`/`(.foo 5)` miss
  shapes (the `dtor_method_on_other_values` comment
  stays true for misses — the implementer may note
  the narrowed nil rule), `rec-eq-map`.

### 2.4 Error-shape adaptations (port conventions)

Where the JVM's text is unportable, the port keeps the
class and adapts the message by house rule:

- Short-shape cast failures (`Cannot cast {from} to
  {to}`, `NativeError::cast` — the JVM's module shape is
  unportable here): `i-ns-imports-str`,
  `m-cls-assign-nonclass`, `w-nonfn-flush`. (The other
  class-position CCEs — `c-inst-nonclass`,
  `c-bases/supers-nonclass`, `c-cast-nonclass-c`,
  `c-with-meta`, `i-conflict-def`, `i-import-over-refer`
  — render the JVM's module shape via `cce_err`,
  byte-equal to the oracle; `b-keys-var` is not an
  error — the port answers `:class`, §1.11.)
- Spec `ExceptionInfo` data → `nil`
  (`i-import-exdata`, `i-expand-str`,
  §2.1 decision 16).
- Nil-message exceptions → empty message:
  `NoSuchElementException` (`m-first-empty`) and the
  `isAssignableFrom(nil)` NPE (`m-cls-assign-nil`).
- Eager `bean` of an empty `List` throws the bare
  `NoSuchElementException` at `bean` time (empty
  message); the JVM returns a 4-key proxy and throws
  `InvocationTargetException` (wrapping NSEE) only
  when `:first`/`:last` (or `keys`/`vals`/print) is
  touched (§1.11).
- `..` 0-arg call misses → field-shape (the
  `dtor_method_on_other_values` pin); the JVM answers
  method-shape (`m-miss-0`).
- Nil misses keep miss shapes (R1 rule); the JVM NPEs
  (`m-nil-miss0/miss1/field`).
- `..` with a class-name symbol → instance path; the
  JVM tries a static call (`m-cls-symbol-target`).
- `import`/`..`/`memfn`/`bean`/`class`/`type` arity
  errors name the port macro/native (`clojure.core/..`,
  not `.../-DOT--DOT-`; `clojure.core/instance?`
  without the JVM's `--N`).
- `refer`/`intern` over imports replace silently (no
  `*err*` warning — port-wide convention,
  `i-intern-over-import`, `i-refer-warn`).
- HostDef leniency: the `supers` walk skips claimed
  names that no longer resolve (redefinition deleted
  the target); `import` of a stale name is `CNFE`.

### 2.5 Risks

- `hashCode` for classes has no oracle row (identity
  hashes are unpinnable): the implementer uses the
  Java String hash of the name (§1.2) — deterministic
  and `=`-consistent, but a reviewer may ask why not
  identity. Answer: determinism (same argument as
  `record_hasheq`/`inst_hasheq`).
- `identical?` collection ids touch the shared
  struct: additive `u64`s, but every constructor must
  mint them — miss one and two empty vectors compare
  `identical?`-true... no: distinct ids compare
  false; the failure mode is a missed id defaulting
  (e.g. 0) and colliding. Tests must cover
  freshly-built collections of every kind.
- Cycle-safe `supers` is load-bearing: adversarial
  `definterface` cycles (`A extends B extends A`)
  must terminate — the visited set is not optional.
- The `bean` HostDef-method rule calls user code
  eagerly at `bean` time (the JVM calls lazily per
  access): throwing getters fail `bean` itself, not
  the key lookup — a phase difference tests must pin
  as specified, not as JVM-identical.
- Oracle JDK sensitivity: `getInterfaces` order,
  `String` bases, the `:static` native counters
  (`first__N`), and the `NodeSeq` class names depend
  on the JDK + Clojure build. All are scrubbed or
  order-pinned where fragile; a JDK upgrade means
  re-running the probes, not rewriting the spec.
- `ns-imports` fresh-ns `{}` is a fenced divergence
  from the JVM's ~100 defaults — explicit in §1.7 and
  the row-flip plan, but the most likely review
  question. The forcing pin:
  `(clojure.core/instance? String 1)` must not
  compile.

### 2.6 Probe inventory (780 rows, all byte-identical on re-run)

| probe | rows | covers |
|---|---|---|
| `lattice.clj/.out` | 227 | 214 static `l-<name>` + 13 dynamic (`l-fn-*`, `l-native-*`, `l-future/promise/iteration`, `l-reify-shape`) |
| `classes.clj/.out` | 155 | `c-class-*` 41, `c-name-*` 29, `c-type-*` 9, `c-inst-*` 24, `c-class?-*` 6, `c-cast-*` 9, value semantics 15, bases/supers shapes 13, arities 4, HostDef ancestry 5 |
| `import.clj/.out` | 54 | expansion 8, behavior + coercions + conflicts + validation + defaults 44, static oracle-only 2 |
| `interop.clj/.out` | 257 | `..` shape/threading/nil/miss (§§A–B) 19, `toString` (§C) 48, `equals`/`hashCode`/`getClass` (§§D,L) 37, Class reflection (§E) 28, `memfn` (§F) 7, `bean` (§§G,N) 54, HostDef (§H) 4, gates (§§I,J,K,M,O) 57, oracle-only trailer 3 |
| `identical.clj/.out` | 43 | per-kind table + rewrite agreement + var use |
| `writers.clj/.out` | 26 | `PrintWriter-on` incl. 3 direct-interop oracle-only rows |
| `arrays.clj/.out` | 18 | oracle-only array classes for the S15 follow-up |

Re-run convention (header of every `.clj`): alpha10
jars on `CP`, `en/US/UTC` locale flags,
`java ... clojure.main <probe> > <run>`, `cmp` against
the committed `.out`.

### 2.7 Conformance checklist (for the reviewer)

- Every Part 1 behavior cites a committed probe row;
  every probe row is cited or marked oracle-only/
  follow-up.
- Every `Class` the port reports (§1.4 table) has a
  static row or a dynamic rule; the table is
  mechanically closed (checked in §1.3).
- Every divergence names its forcing pin or fence
  (§§2.1, 2.4) — no silent gaps.
- The 14 row-flips list line numbers against current
  `origin/master` (§1.15); the PR #41 mirror note in
  issue #60 is stale (PR #41 landed).
- S16 suites import nothing from peer branches;
  cross-slice work is itemized in §2.2, not assumed.
