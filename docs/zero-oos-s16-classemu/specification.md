# Zero-oos S16: class emulation — Specification

- Status: spec + oracle probes written; no source edits.
- Parent: out-of-scope reduction. Issue: #60.
  Branch: `zero-oos-s16-classemu`.
- Probes: `docs/zero-oos-s16-classemu/probes/`
  `{lattice,imports,threads,beans}.{clj,out}`
  (523 pins, every file exit 0 and rerun-stable; the
  lattice section is mechanically closure-checked: every
  class name appearing in any `lb/lc/lj/lk/lm/ls` row has
  its own `bases` row).

## Problem

Twelve interop/class vars are `out-of-scope`
(`instance?`, `type`, `class`, `bases`, `supers`,
`import`, `..`, `bean`, `cast`, `class?`, `memfn`,
`identical?`, `PrintWriter-on`) and `ns-imports` is
`divergent` (always `{}`, `native_ns_imports` in
`rust/crates/clj-eval/src/ns.rs`). The port intercepts
narrow shapes at analyze time (`instance?`/`identical?`
"stay unresolved vars" per
`rust/crates/clj-eval/src/fx4_host.rs`; dotted
construction resolves "without an import table" per
`lookup_root` in `rust/crates/clj-eval/src/types.rs`),
and `jvm_class_of`
(`rust/crates/clj-eval/src/error.rs`) already names every
value's JVM class — but no `Class` values exist, no
import table exists, and no reflective member access
exists.

## Solution

Emulate the class system, oracle-pinned: a `Class`
value kind as a `Value::Extension` (`ClassValue`,
holding the JVM binary name) covering port types
(`HostDef` descriptors/instances), every scalar/coll
class in the `jvm_class_of` lattice, and the JDK
classes the oracle pins; `type`/`class`/`instance?`/
`class?`/`cast` over it; `bases`/`supers` lattice
walks over a mechanically transcribed 287-row table;
an `import` macro plus a real per-namespace import
table (seeded with the 106 JVM default imports)
feeding both `ns-imports` and unqualified
construction/member resolution (extending
`lookup_root`, keeping every existing dotted-name pin
green); a `..` macro over a new general `(. target
member)` dot shape; `bean` over the emulated member
surface; `memfn` expansion; a resolving `identical?`
var consistent with the existing analyze-time
`HostOp::Identical` rewrite (both are `=`); and
`PrintWriter-on` constructing the `java.io.PrintWriter`
`Writer` kind. Full design in §2.1.

## Part 1 — Normative contract

### 1.1 Conventions

- A probe id is `<prefix>-<name>` where the prefix
  names the file: `c/t/i/k/x/p/q/lb/ls/lc/lj/lk/lm/li`
  lattice, `m/n/d/e/r` imports, `x/d/m/f` threads,
  `b/w` beans.
- `THROW C|"m"` quotes the `.out` throw line verbatim
  (root cause).
- `(derived)` marks a port-test assertion derived from
  an oracle pin rather than quoted by it.
- Class pins transfer directly: the port now HAS
  `class`, so JVM `(class x)` pins become port
  `(class x)` pins (S12's `jvm_class_of` indirection
  is retired for this slice).
- `CNFE` = `ClassNotFoundException`, `CCE` =
  `ClassCastException`, `NPE` = `NullPointerException`,
  `IAE` = `IllegalArgumentException`, `ISE` =
  `IllegalStateException`.

### 1.2 Class values and the lattice

A `Class` value prints (pr/print) as its JVM binary
name, `=`-compares by name, and reports
`java.lang.Class` to `jvm_class_of`. The lattice is
the transcribed `(bases X)` table (§2.1 D2); every
ancestry query (`bases`, `supers`, `instance?`,
`cast`) walks it. Names outside the table (synthetic
port names: fn classes, future/promise reifies,
`clojure.core$iteration`, `clojure.core.Eduction`;
and untranscribed JDK names) hang directly under
`java.lang.Object`.

| Label | Contract |
| --- | --- |
| p-pr | `(pr-str String)` is `"java.lang.String"` |
| p-print | `(print-str String)` is `"java.lang.String"` |
| p-str | (divergence D24.3) `(str String)` is `"java.lang.String"` (JVM: `"class java.lang.String"`) |
| p-pr-class | `(pr-str (class 1))` is `"java.lang.Long"` |
| p-eq/p-eq2/p-eq3 | class `=` is by name |
| p-printdup | `(binding [*print-dup* true] (pr-str String))` is `"#=java.lang.String"` |
| p-name | `((memfn getName) String)` is `"java.lang.String"` |
| p-simple | `((memfn getSimpleName) String)` is `"String"` |
| p-tostring | `((memfn toString) String)` is `"class java.lang.String"` |
| p-iface-tostring | `((memfn toString) Comparable)` is `"interface java.lang.Comparable"` |
| lb-* (138) | `(bases X)` ordered transcription (superclass first, then interfaces) |
| lc-* (47) | second-order closure transcription |
| lj-* (15) | port runtime classes transcription |
| lk-* (12) | third-order closure transcription |
| lm-* (76) | default-import coverage transcription |
| li-all/li-all2/li-all3 | `isInterface` flags for all 287 keys |
| ls-* (19) | `(supers X)` spot checks (sorted names; port returns a set) |
| lb-object/ls-object/ls-nil/lb-nil | `(bases Object)`/`(supers Object)`/`(supers nil)`/`(bases nil)` are nil |
| c-class/c-class2 | `(class String)` is `Class` |

### 1.3 type, class, instance?, class?, cast

`type` is `:type`-meta-or-class; `class` is the
`jvm_class_of` name as a `Class` (nil for nil).
`instance?`/`cast` take a class DESIGNATOR (D3): a
`Class` value or, transitionally, a class-name
string (the S-typechain `(def String
"java.lang.String")` vars flow here; the JVM rejects
strings — divergence D24.2).

| Label | Contract |
| --- | --- |
| c-long/c-double/c-bigint/c-bigdec/c-ratio | tower classes |
| c-bool/c-char/c-string/c-nil | scalar classes; nil → nil |
| c-keyword/c-symbol/c-vec/c-mapentry | `Keyword`/`Symbol`/`PersistentVector`/`MapEntry` |
| c-pam/c-phm/c-ptm/c-phs/c-pts | size/sort-dependent map/set classes |
| c-elist/c-list | `PersistentList$EmptyList`/`PersistentList` |
| c-pattern/c-var | `Pattern`/`Var` |
| c-atom/c-delay/c-agent/c-volatile | `Atom`/`Delay`/`Agent`/`Volatile` |
| c-future/c-promise | (derived) `clojure.core$future_call$reify` / `clojure.core$promise$reify` (JVM mints fresh reify classes) |
| c-fn/c-ns | (derived) fn class is the munged display; ns is `Namespace` |
| c-lazyseq/c-cons/c-rangeseq/c-strseq/c-vecseq | seq-kind classes |
| c-multifn/c-exinfo/c-throwable | `MultiFn`/`ExceptionInfo`/throwable classes |
| t-long/t-nil | `(type 1)`/`(type nil)` |
| t-meta-class/t-meta-kw/t-meta-str | `:type` meta wins, any value |
| i-str/i-obj/i-num/i-long-exact | true cases |
| i-int-miss/i-nil-target | false cases (incl nil target) |
| i-charseq/i-comparable/i-ifn/i-seqable | interface cases |
| i-class-of-class/i-class-miss | `(instance? Class String)` true; `(instance? String String)` false |
| i-vec-ipm/i-map-ipm | vector false / map true for `IPersistentMap` |
| i-nil-class | THROW NPE `"Cannot invoke \"java.lang.Class.isInstance(Object)\" because \"c\" is null"` |
| i-str-class/i-num-class | (divergence D24.2) strings accepted as designators; non-strings take the port CCE to `java.lang.Class` |
| k-string/k-str/k-nil/k-class | `class?` is `instance? Class` (strings are not classes) |
| x-ok/x-nil | cast hit returns x; cast of nil is nil |
| x-fail/x-fail2 | THROW CCE `"Cannot cast java.lang.Long to java.lang.String"` / `"Cannot cast java.lang.String to java.lang.Integer"` |
| x-nilclass | THROW NPE `"Cannot invoke \"java.lang.Class.cast(Object)\" because \"c\" is null"` |
| x-nonclass | (divergence D24.2) like i-str-class |

### 1.4 bases, supers

Both take a designator (D3) or nil (→ nil).
`bases` answers an ordered list (super first, like
`cons`); `supers` answers a set (`not-empty`).
Non-designators take the port CCE to
`java.lang.Class` (JVM: lb-nonclass long-form CCE).

| Label | Contract |
| --- | --- |
| lb-string | `(Object Serializable Comparable CharSequence Constable ConstantDesc)` |
| lb-long | `(Number Comparable Constable ConstantDesc)` |
| lb-* (all) | exact ordered transcription incl `()` → nil cases |
| ls-* (all) | exact supers sets (port: set values) |
| lb-nonclass/lb-nonclass2/ls-nonclass | (derived) port CCE shape |

### 1.5 import, ns-imports, and class resolution

`import` is a macro emitting `(do
(clojure.core/__import* "FQN") …)`; `__import*` is a
hidden native validating against the known-class set
(lattice keys + default-import FQNs +
`java.util.Calendar` + dotted port-type descriptors)
and installing simple-name → FQN in the current ns.
Every ns is seeded with the 106 JVM default imports
(n-fresh-full). Value-position symbols resolve:
locals → vars → imports → fully-qualified known
names (D6).

| Label | Contract |
| --- | --- |
| m-single/m-quoted | `(do (clojure.core/__import* "java.util.Date"))` (derived: `import*` → `__import*`) |
| m-list/m-unquoted-list | one `__import*` per class |
| m-multi/m-empty | multi and `(do)` shapes |
| m-importstar | `clojure.core/import*` stays unresolvable (port: `__import*` is the hidden native) |
| n-fresh | fresh ns has exactly the 106 default simple names |
| n-fresh-full | exact simple → FQN pairs (seeding transcription) |
| n-fresh-val | `(get (ns-imports 'fresh) 'String)` is `String` |
| d-return | `(import 'java.util.Date)` answers the `Date` class |
| d-table/d-keys | table maps `Date` → `Date` |
| d-value/d-class-of/d-eq | `Date` evaluates to the class; `(class Date)` is `Class`; `=` to FQCN |
| d-listform | `(import '(java.util Calendar))` works |
| d-repeat/d-javalang | re-import and default re-import are no-ops |
| d-shadow | (divergence D24.8) port `def` does not check imports (JVM: `RuntimeException "Expecting var, but Date is mapped to class java.util.Date"`) |
| d-unmap/d-unmap-default | `ns-unmap` drops explicit and default imports |
| d-empty | `(import)` is nil, imports nothing |
| e-missing/e-lowercase | THROW CNFE `"no.such.Klass"` / `"java.util.date"` |
| e-conflict/e-conflict2 | THROW ISE `"Date already refers to: class java.util.Date in namespace: <ns>"` |
| e-number/e-string | (divergence D24.15) port derived errors (JVM: spec `ExceptionInfo`) |
| r-fqcn/r-short/r-short2 | FQCN and default-imported shorts evaluate to classes |
| r-resolve-short/r-nsresolve-short/d-resolve/d-nsresolve | (divergence D24.7) port `resolve`/`ns-resolve` return nil for classes (JVM: the class) |
| d-nsmap-has | (divergence D24.7) port `ns-map` excludes imports (S5A owns `ns-map`) |

### 1.6 General dot, .., memfn, and the member surface

New analyzer shape `(. target (method args…))`
(method-only, missing → `No matching method`) and
`(. target member)` (field-first, missing → `No
matching field`). `..`/`memfn` are verbatim macro
ports. The emulated surface (D9): `toString/0`,
`getClass/0`, `hashCode/0`, `equals/1` on every
value; `flush/0`, `close/0`, `write/1`, `write/3`
on writers; `getName/0`, `getSimpleName/0`,
`toString/0`, `getSuperclass/0` on classes. Host
targets keep their tables (method-only for the list
shape). Nil target + emulated member → NPE
`Cannot invoke "Object.getClass()" because "target"
is null` (derived uniform; cf d-nil).

| Label | Contract |
| --- | --- |
| x-doc/x-single/x-three | single-level `..` expansions (verbatim shapes) |
| x-bare | `(.. "a" toString)` → `(. "a" toString)` |
| x-arity1 | (derived) port macro-arity error |
| d-tostring/d-long-tostring/d-vec-tostring/d-map-tostring/d-keyword-tostring | `toString` is `str` |
| d-getclass | `(.. "a" (getClass))` is `String` |
| d-hashcode-str/d-hashcode-long/d-hashcode-bool | 97 / 1 / 1231 |
| d-equals-t/d-equals-f | boolean equality |
| d-equals-numtower | (divergence D24.5) port `(.. 1 (equals 1.0))` is true (JVM: false) |
| d-chain | threading composes |
| d-nil | NPE shape (derived uniform text) |
| d-missing0 | THROW IAE `"No matching method noSuchMethod found taking 0 args for class java.lang.String"` |
| d-missing1 | THROW IAE `"… taking 2 args …"` |
| d-bare-missing | THROW IAE `"No matching field found: noSuchField for class java.lang.String"` |
| d-length/d-substring | (not emulated) port `No matching method` (derived) |
| m-plain/m-args | `memfn` expansion shapes (derived gensym) |
| m-arity0 | (derived) port macro-arity error |
| f-tostring/f-map/f-length | `memfn` calls thread |
| f-substring | `((memfn substring beg end) "abc" 0 1)` is `"a"` |
| f-missing | THROW IAE method error |

### 1.7 bean

`bean` answers a plain snapshot map (documented
emulation: the JVM answers an `APersistentMap`
proxy). `:class` is always present. Menus: String
(`:blank` `:class` `:empty`; `:bytes` omitted —
S15); vector (`:class` `:empty` `:first` `:last`);
map (`:class` `:empty`); keyword/symbol (`:class`
`:name` `:namespace`); class (`:class` `:name`
`:simpleName`); port record/struct/deftype
instances (declared fields); everything else
`:class`-only. Nil → NPE `Cannot invoke
"Object.getClass()" because "x" is null`.

| Label | Contract |
| --- | --- |
| b-str-keys/b-str-empty/b-str-class | `(:blank :bytes :class :empty)` less `:bytes`; `:empty` false; `:class` String |
| b-str-blank/b-str-blank2 | false / true |
| b-str-bytes | (not emulated) no `:bytes` key (S15 owns arrays) |
| b-long-keys/b-long-class/b-bool-keys | `:class`-only |
| b-vec-keys/b-vec-first/b-vec-last/b-vec-empty | full vector menu |
| b-map-keys/b-map-empty | map menu |
| b-kw-keys/b-kw-name/b-kw-ns | keyword menu |
| b-sym-keys/b-sym-name/b-sym-ns | symbol menu |
| b-count/b-contains/b-get-missing/b-get-default/b-seq-first | map citizenship (derived shapes) |
| b-bean-class | (divergence D24.6) port class is `PersistentArrayMap` |
| b-nil | NPE shape verbatim |

### 1.8 PrintWriter-on

`(PrintWriter-on flush-fn close-fn [autoflush?])`
constructs the `java.io.PrintWriter` writer kind
around an accumulating buffer. Writes append;
`flush` calls `flush-fn` with the accumulation iff
nonempty, then clears; `close` flushes, then calls
`close-fn` iff non-nil, and answers nil.
Autoflush is accepted and stored but inert
(w-autoflush: embedded newlines do not flush —
matches the JVM write path). Usable as `*out*`
(write + `flush` route through).

| Label | Contract |
| --- | --- |
| w-basic/w-accumulate | accumulation + single flush callback |
| w-empty-flush | no callback on empty flush |
| w-close-flushes/w-close-fn/w-close-noflushfn-call | close flushes first, then close-fn; nil-safe |
| w-class | class is `java.io.PrintWriter` |
| w-write-offlen | `write/3` appends the slice |
| w-autoflush/w-autoflush-then/w-noautoflush | no implicit flush; explicit flush delivers all |
| w-arity1 | (derived) port arity error |
| w-print-binding | print+flush through a bound `*out*` |

### 1.9 identical?

Resolving var, exactly `=` (same code path as the
pinned `HostOp::Identical` analyzer rewrite, which
is unchanged). JVM identity record (q-*) is kept
for the divergence file: values clone, so identity
is equality (divergence D24.1).

| Label | Contract |
| --- | --- |
| q-nil/q-small-long/q-str-lit/q-kw/q-bool | true (agree) |
| q-big-long/q-str-built/q-vec/q-sym/q-mixed | (divergence D24.1) port answers `=` |

### 1.10 Matrix edits

Flip these 13 rows in place in
`rust/core-coverage.md` (lines 63, 481–485,
504–505, 574, 690, 713, 730, 743, 785) from
`out-of-scope` (and one `divergent`) to
`implemented` with the notes below, and update the
header to `692 vars: 464 implemented, 0 divergent,
227 out of scope, 0 planned`. Test file:
`rust/crates/clj-eval/tests/classemu.rs` (fenced
to S16 by #60); test-fn names are advisory.

| var | notes |
| --- | --- |
| `ns-imports` | tests/classemu.rs import_rows (S16; probes imports.out; sorted simple→Class map) |
| `instance?` | tests/classemu.rs lattice_rows (S16; probes lattice.out; Class-or-string designators, §2.4) |
| `type` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `class` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `bases` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `supers` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `import` | tests/classemu.rs import_rows (S16 macro; probes imports.out; closed world, §2.4) |
| `..` | tests/classemu.rs dot_rows (S16 macro; probes threads.out; general dot shape) |
| `identical?` | tests/classemu.rs identical_rows (S16; probes lattice.out; `=`-consistent per the pinned analyzer rewrite, §2.4) |
| `PrintWriter-on` | tests/classemu.rs printwriter_rows (S16; probes beans.out) |
| `bean` | tests/classemu.rs bean_rows (S16; probes beans.out; snapshot map, §2.4) |
| `cast` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `class?` | tests/classemu.rs lattice_rows (S16; probes lattice.out) |
| `memfn` | tests/classemu.rs memfn_rows (S16 macro; probes threads.out) |

## Part 2 — Notes

### 2.1 Design decisions

- D1 (Class values): `Value::Extension` holding a
  new `clj_value::ClassValue { name: Arc<str> }`
  (`rust/crates/clj-value/src/hostclass.rs`). No new
  `Value` variant (that would touch every exhaustive
  match — hostile to concurrent slices). `class_name`
  is `java.lang.Class` (so `jvm_class_of`,
  `simple_class_of`, and CCE from-names work
  untouched); `print` is the bare FQN (p-pr/p-print
  via the existing `print-method` default, mirroring
  the JVM's `print-method Class` which writes
  `getName`); `=`/`hash` are by name;
  `dup_shape` is `#=<FQN>` (p-printdup).
- D2 (lattice): static
  `BASES: &[(&str, Option<&str>, &[&str])]` in
  `rust/crates/clj-eval/src/classemu.rs`
  (FQN, superclass, interfaces), MECHANICALLY
  generated from `lattice.out` (script in §2.3;
  never hand-edited): 287 keys, order-exact,
  superclass split by the `li-*` flags (first
  element iff not an interface; `Object` has
  neither). `supers` is the fixpoint walk;
  `instance?`/`cast` are membership tests. Unknown
  names (synthetic port names, untranscribed JDK
  names) default to direct `Object` children —
  consistent by construction, approximate at the
  edges (§2.4). Port-defined (`HostDef`) classes
  resolve dynamically: superclass `Object`,
  interfaces from the def.
- D3 (designators): `instance?`/`cast`/`bases`/
  `supers` accept a `Class` or a class-name string.
  The 5 core string vars (`String`, `Long`,
  `Double`, `Boolean`, `Object` in `core.clj`)
  are S-typechain's items and stay strings, so the
  BDD `(instance? String "a")` flows a string;
  rejecting it would break S-typechain pins or
  force touching their vars. Transitional
  divergence (§2.4); `class?` stays strict.
- D4 (import table): `Namespace.imports:
  HashMap<String, Arc<str>>` (simple → FQN) with
  additive methods; `Namespace::new` seeds the
  106 `n-fresh-full` pairs (ONE seeding site —
  every ns funnels through `ensure_ns_in`).
  `unmap` drops imports too (d-unmap, additive
  line). Conflicts throw the verbatim ISE
  (e-conflict).
- D5 (import validation): the port has no class
  loader, so `__import*` validates against the
  closed known-class set (lattice keys +
  default-import FQNs + `java.util.Calendar` +
  dotted port-type descriptors); anything else is
  CNFE (e-missing shape). The `import` macro is a
  verbatim port modulo `import*` → `__import*`
  and `reduce1`/`into1` → `reduce`/`into` (port
  lacks the `1` privates; same semantics here).
- D6 (value-position resolution): additive
  fallback in `analyze_symbol` AFTER var
  resolution: current-ns import hit, else
  fully-qualified known name, else the existing
  errors. Vars shadow imports (forced by the
  string defs; the JVM instead forbids `def`
  over imports — §2.4). Compile-time, like the
  JVM (a `do`-split evaluates earlier elements
  first, so sequential top-level imports work).
- D7 (`lookup_root`): additive import fallback
  after `var_root`: unqualified miss +
  import hit → dotted FQN re-resolves as
  ns+name (port types construct unqualified
  after import); foreign FQNs answer their
  `Class`, so `__new*` reports the honest
  `Not a type: <sym>`. Every existing
  dotted-name pin is preserved (fresh evaluators
  carry only default imports; the 5 string vars
  still resolve first).
- D8 (general dot): additive `try_dot_form` arm
  for head `.` exactly: 3-lists with a list
  member → new hidden `__methcall*`
  (method-only, missing → `No matching method`
  at any arity, d-missing0); with a symbol
  member → existing `__meth*` (field-first,
  d-bare-missing). Anything else keeps the
  unresolvable error. Host targets dispatch
  through a new additive method-only helper
  (same tables, no field-first).
- D9 (member surface): ONE shared helper serving
  both `__meth*` (additive non-host arm — the
  only `types.rs` behavior touch besides D7)
  and `__methcall*`, so prefix and general
  shapes agree: `toString/0` = `str`,
  `getClass/0` = `class`, `hashCode/0` =
  `hash_code`, `equals/1` = `=` (§2.4);
  writers gain `flush/0`, `close/0`, `write/1`,
  `write/3`; classes gain `getName/0`,
  `getSimpleName/0`, `toString/0` (`class`/
  `interface` prefix by lattice role),
  `getSuperclass/0`. Nil + emulated member →
  the uniform NPE. Existing `foo`/`nosuch`/
  `close`-on-`Long`/`x` pins are untouched
  (none of those names are emulated).
- D10 (bean): snapshot map per §1.7 menus.
  Declared-fields-only for port instances
  (mirrors JVM declared getters; extension
  content stays invisible).
- D11 (PrintWriter-on): new additive
  `WriterKind::Callback` + additive arms in
  `class_of` (`PrintWriter`), `write_to_writer`
  (buffer; autoflush inert per w-autoflush),
  `flush_out` (invoke iff nonempty), and a new
  `flush_writer`/`close_writer` pair also used
  by the D9 member surface. `check_writer`
  accepts it untouched (downcast).
- D12 (`identical?`): native delegating to the
  `=` code path after a 2-arity check. The
  analyzer rewrite is byte-identical behavior
  and stays.
- D13–D15 (fns): per §1.3–1.4; nil rules and
  NPE/CCE texts verbatim where the JVM pins
  them, port-CCE shape for non-designators.
- D16/D17 (macros): verbatim `..`/`memfn`/
  `import` ports in a marked `core.clj`
  section.
- D18 (`ns-imports`): body rewrite over the
  import table (sorted, symbol keys, `Class`
  values); arg validation untouched. The one
  stale S5A pin (`ns_vars.rs`, `(ns-imports
  'fx)` → `{}`) is updated to the seeded
  table — it asserts this slice's row.
- D19 (matrix): §1.10.
- D20 (CNFE): additive `ThrowClass::
  ClassNotFoundException` + `jvm_name` arm +
  `throw_class_for` arm (so `catch` by name
  resolves IF named — but the §E6
  `is_curated_fqn` fence is untouched, so
  users catch it via `Exception`; §2.4).
- D21 (files): new `hostclass.rs`,
  `classemu.rs`, spec dir, `tests/classemu.rs`;
  additive edits listed in §2.3. No changes to
  `fx4_host.rs` (rewrite pinned), `multi.rs`,
  `protocol.rs`, `satisfies?`/`extends?`, the
  `ns` special form (`:import` unsupported —
  §2.4), `resolve`/`ns-resolve`/`ns-map`
  (§2.4), or `def` (§2.4).
- D22 (complexity): lattice as data (linear
  search, no giant match); dispatch in small
  chained fns; baseline extended only for pure
  dispatch tables with justification.
- D23 (seams): S13/S14/S15 are unlanded on
  this base, so all three seams are one-sided
  by necessity: S13's class-pair logic should
  ride `instance?`/`supers`; S14's
  string-answering fns gain `Class` values to
  upgrade to (the 5 core defs stay strings —
  §2.5); S15's array classes land as
  `Object`-children until S15 extends the
  table, and `bean` `:bytes` awaits S15.
  Shared pins when they land (landing owner
  coordinates).

### 2.2 Spike resolutions

- Class representation: EXTENSION. A new
  `Value` variant would touch every exhaustive
  match across three crates (hostile to
  concurrent slices); `ExtensionValue` carries
  `class_name`/`print`/`dyn_eq`/`dyn_hash`/
  `dup_shape` exactly as needed, and
  `token_of` falls through to `Object`
  (correct default printing).
- `identical?` semantics: EQUALS. The pinned
  analyzer rewrite lowers qualified
  `identical?` to `=`, and #60 demands a var
  "consistent with the rewrite" — the only
  consistent reading is the same code path.
- `import` target: HIDDEN NATIVE. The JVM's
  `clojure.core/import*` is compiler magic
  (`resolve` → nil, m-importstar); the port's
  `__import*` hidden native follows the
  `TYPES_NATIVES` precedent.
- `ns` `:import`: OUT. The `ns` special form
  is S5A's (refer-clojure/require/use only);
  extending it is a different slice's item.
  Users call `import` directly.
- Static interop: OUT. The port has no
  statics; class-valued targets answer the
  D9 instance members (JVM would compile-error
  `(. String (getName))` — §2.4).
- String/collection JDK methods (`length`,
  `substring`, …): OUT. Only the `Object`
  tetrad is emulated; the probes record the
  JVM values for a follow-up.
- `bean` depth: SNAPSHOT MAP with the §1.7
  menus. Proxy-map laziness is unobservable
  in the pins; `:bytes` needs S15 arrays.
- `resolve`/`ns-resolve`/`ns-map`/`def`:
  UNTOUCHED (all §2.4 gaps with JVM records).

### 2.3 Grounding map

- Oracle sources: alpha10 jars
  (`clojure-1.13.0-alpha10.jar` + specs),
  `clojure/core.clj` (`instance?`, `type`,
  `class`, `bases`, `supers`, `import`, `..`,
  `identical?`, `cast`, `class?`, `memfn`,
  `ns-imports` bodies), `clojure/core_proxy.clj`
  (`bean`), `clojure/core_print.clj`
  (`PrintWriter-on`, `print-method`/`print-dup`
  on `Class`).
- Table generator (run at implement time, kept
  as `docs/zero-oos-s16-classemu/probes/
  gen_tables.py`; inputs `lattice.out` +
  `imports.out`): parses every
  `l{…}-* ("a" "b" …)` row into
  `(key, super-or-None, [ifaces])` using the
  `li-*` flags (asserts: flags cover all keys
  except `Guard` — empty-bases non-`Object`
  implies interface; `Object` is the only
  empty-bases class; every ancestor is a key),
  emits `BASES` sorted by key; parses
  `n-fresh-full` into `DEFAULT_IMPORTS`
  (asserts 106 pairs). The `.rs` tables carry
  a "generated — do not hand-edit" header
  naming the script + sources.
- New code: `rust/crates/clj-value/src/
  hostclass.rs` (`ClassValue` + downcast
  helpers), `rust/crates/clj-eval/src/
  classemu.rs` (tables, `CLASS_EMU_NATIVES`:
  `type` `class` `instance?` `bases` `supers`
  `cast` `class?` `bean` `PrintWriter-on`
  `identical?` + hidden `__import*`
  `__methcall*`; `plain_member` surface;
  `methcall` host helper glue).
- Additive edits: `clj-value/src/lib.rs`
  (export), `clj-value/src/extension.rs`
  (CNFE), `clj-eval/src/lib.rs` (`mod
  classemu`), `core.clj` (S16 macro section),
  `ns.rs` (boot chain, `ns-imports` body,
  `throw_class_for` arm), `env.rs` (imports
  field/methods/seeding/unmap line),
  `expr.rs` (`analyze_symbol` fallback, `.`
  arm), `types.rs` (`lookup_root` fallback,
  non-host arm call-out, method-only helper),
  `io.rs` (`Callback` variant + arms +
  ctor + flush/close helpers),
  `tests/ns_vars.rs` (1 stale assertion),
  `rust/core-coverage.md` (13 rows +
  header), maybe
  `rust/complexity-baseline.json`.
- Tests: `rust/crates/clj-eval/tests/
  classemu.rs` (`lattice_rows`,
  `import_rows`, `dot_rows`, `memfn_rows`,
  `bean_rows`, `printwriter_rows`,
  `identical_rows`), every assertion citing
  its probe label.

### 2.4 Divergences (all documented at the rows)

- D24.1 `identical?` is `=` (q-* record the
  JVM): values clone, and the pinned
  analyzer rewrite is `=`.
- D24.2 class designators accept strings
  (i-str-class/x-nonclass/lb-nonclass record
  the JVM CCE): transitional, until the 5
  core string defs are upgraded (owner TBD —
  not S14's fence, not this slice).
- D24.3 `(str <Class>)` is the bare FQN
  (p-str): the port's `str`/print share the
  extension print path; the `class `/`interface
  ` prefix survives only on the `toString/0`
  member.
- D24.4 unknown lattice names hang under
  `Object` (fn classes, reify classes,
  iteration/Eduction handles,
  untranscribed JDK names).
- D24.5 `equals/1` member is port `=`
  (d-equals-numtower).
- D24.6 `bean` is a snapshot map, not a
  proxy; `:bytes` omitted (S15); menus per
  §1.7 (b-bean-class/b-str-bytes).
- D24.7 `resolve`/`ns-resolve` return nil
  for classes and `ns-map` excludes imports
  (d-resolve/d-nsresolve/r-resolve-short/
  d-nsmap-has): those fns are S5A's items.
- D24.8 `def` over an import does not fail
  (d-shadow): `def` is not this slice's.
- D24.9 no `ns` `:import` clause: `ns` is
  S5A's; call `import`.
- D24.10 class-valued targets answer
  instance members (JVM compile-errors
  `(. String (getName))`); no statics.
- D24.11 `(. host (field))` list-shape:
  NONE — method-only is implemented
  properly (kept here so the quirk is
  never reintroduced silently).
- D24.12 closed import world: only
  lattice keys + default FQNs +
  `java.util.Calendar` + dotted port
  types import; everything else is CNFE.
- D24.13 autoflush inert (w-autoflush*):
  matches the JVM write path (newlines do
  not flush); `println`-triggered flush
  has no port routing.
- D24.14 `getSimpleName` follows the
  port's `simple_name_of` (`$`-lowercase
  keeps the full name; JVM anonymous
  classes answer `""`).
- D24.15 no import arg specs
  (e-number/e-string): port derived
  errors.
- D24.16 `(catch ClassNotFoundException …)`
  unsupported (§E6 fence untouched); catch
  via `Exception`.

### 2.5 Risks and open points

- R1 (5 string defs): `(class? String)` is
  false and `(= (class "a") String)` is
  false until the defs are upgraded. No
  fenced slice owns the upgrade (S14's
  fence is factories/proxies/protocol
  introspection); this spec keeps strings
  working as designators so the upgrade is
  behavior-adding, not behavior-fixing.
  Owner TBD at landing.
- R2 (`ns_vars.rs` stale pin): the
  `(ns-imports 'fx)` → `{}` assertion is
  updated to the seeded table. It asserts
  this slice's row; no other S5A
  assertion changes.
- R3 (unmap collisions): `unmap` now drops
  imports; existing unmap pins are
  re-verified by the suite (any pin
  unmapping a default-imported name is
  inspected, not silently kept).
- R4 (bare-name captures): default imports
  make 106 short names resolvable; pins
  asserting "Unable to resolve" for such
  names would break — the suite re-verifies
  (recon found none; all use `nope*`/
  `undeclared*` shapes).
- R5 (S14/S13/S15 landing order): any order
  works — seams are one-sided and
  additive. If S14 lands first with more
  string-answering fns, they ride D3.
- R6 (`first` on non-seqables): `(import 5)`
  behavior depends on the port's `(first 5)`
  — pinned derived at implement time.

### 2.6 Conformance checklist (from #60 BDD)

- [ ] `(import 'java.util.Date)` then
  `(ns-imports *ns*)` maps `Date` to its
  class (d-table).
- [ ] `(instance? String "a")` is true
  (i-str, via the string designator).
- [ ] `(supers String)` contains `Object`
  (ls-string).
- [ ] `(.. "a" (toString))` threads
  (d-tostring).
- [ ] `(bean x)` maps getters (b-* menus).
- [ ] `(identical? a a)` matches the
  analyzer rewrite (D12; q-* record).
- [ ] All thirteen rows flipped (§1.10);
  `coverage_matrix` green; full gate
  suite green.
