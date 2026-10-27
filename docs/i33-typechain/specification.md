# Issue #33: S-typechain slice — Specification

- Status: stages 1–3 implemented (probes committed, 138/33/23
  pins, all exit 0 and rerun-stable; 84/54/20 tests).
- Parent: out-of-scope reduction. Issue: #33. Branch: `33-typechain`.
  Probes: `docs/i33-typechain/probes/{types,protocols,multimethods}.{clj,out}`
  (exit 0 required, verified by rerun).

## Problem

Nineteen `clojure.core` vars are `out-of-scope` with no slice and no
code in the tree: type definitions (`deftype`, `defrecord`, `reify`,
`proxy`, `gen-class`, `gen-interface`, `definterface`), protocols
(`extend`, `extend-type`, `extend-protocol`, `satisfies?`,
`extends?`, plus `with-open` which needs a close protocol), and
multimethods (`defmulti`, `defmethod`, `prefer-method`,
`get-method`, `methods`, `remove-method`). Any program defining its
own types, dispatching protocols, managing resources, or
multimethod-dispatching fails at resolve time (`Unable to resolve
symbol`).

## Solution

Three stages in strict dependency order (protocols build on the
type system; multimethods dispatch over both):

1. **Type system** (`deftype`, `defrecord`, `reify`, `proxy`,
   `gen-class`, `gen-interface`, `definterface` — 7 rows): new
   `Value::HostType` (type name, positional fields, inline method
   table). `defrecord` additionally behaves as a map (associative,
   countable, seqable, `assoc`/`dissoc`/`get`/`keys`/`vals`) with
   `->Type` positional and `map->Type` map factories.
   `gen-class`/`gen-interface`/`definterface` register named type
   and interface descriptors (constructable, no bytecode emission —
   documented divergence). `proxy` is fenced to `Object` as the
   only superclass (every other host class is interop N1,
   out of scope) plus port-defined interfaces.
2. **Protocols** (`extend`, `extend-type`, `extend-protocol`,
   `satisfies?`, `extends?`, `with-open` — 6 rows): a `defprotocol`
   helper (unlisted in the matrix — required as the protocol
   constructor; no row flip claimed for it), a global protocol
   registry (method impls per type, behind `RwLock` with loom
   coverage), `Value::ProtocolFn` dispatch at invoke time,
   `satisfies?`/`extends?` queries, and `Closeable` as a built-in
   protocol with `with-open` as a builtin macro (`try`/`finally`
   close; interop `.close` calls stay out of scope).
3. **Multimethods** (`defmulti`, `defmethod`, `prefer-method`,
   `get-method`, `methods`, `remove-method` — 6 rows):
   `Value::MultiFn` (dispatch fn, default, method table,
   preference table) with exact-value plus `:default` dispatch,
   preference recording, and the table-query natives. Hierarchy
   derivation (`derive`, `isa?`) is fenced as a follow-up
   (non-goal): no hierarchy rows exist yet, dispatch without
   derivation is the faithful subset, and ambiguity detection
   rides with that follow-up (ambiguity is unreachable without
   ancestors — preferences record only).

## Part 1 — User/functional specification

### 1.1 Goals

- `(defrecord Point [x y])` then `(->Point 1 2)`, `(:x p)`,
  `(assoc p :z 3)`, `(map->Point {:x 1 :y 2})` all behave like the
  JVM (field access, map-ness, factories).
- `(deftype Pair [a b] ...)` with inline methods dispatches on
  `this`, including overloaded arities.
- `(satisfies? P x)` is true after `(extend-protocol P T …)`;
  `(extends? P T)` tracks the registry; protocol fns called as
  ordinary fns `(m x args)` dispatch on the runtime type of `x`.
- `(with-open [w (proxy-closeable)] …)` closes on normal exit and
  on unwind (exception still propagates).
- `(defmulti area :shape)` + `(defmethod area :square [m] …)`
  dispatches; `(get-method m v)`, `(methods m)`, `(remove-method m
  v)` manage the table verbatim; `prefer-method` records edges
  (ambiguity resolution needs hierarchies — follow-up).

### 1.2 Non-goals (explicit)

- Bytecode emission for `gen-class` (no `.class` output, no AOT —
  divergence, documented at the row).
- `proxy` of any host class but `Object` (interop N1 owns the rest).
- Hierarchy derivation (`derive`, `isa?`, `:hierarchy` args to
  `defmulti`) — follow-up slice, not this one.
- Interop method calls (`.close`, `.foo`) — N1 owns them for
  host classes; `with-open` emits dot-close (`(.close x)`), which
  dispatches over inline `close` methods on port types (a registry
  protocol extension alone does not satisfy it — documented
  divergence).
- `defprotocol` claims no matrix row (unlisted helper).

### 1.3 Acceptance criteria

- All nineteen rows flip to `implemented`; `coverage_matrix`
  green; full gate suite green (fmt, clippy `-D warnings`, 100%
  line+patch, complexity ratchet, loom for the registries, TLA+
  where shared dispatch state warrants).
- Every new test pins a committed oracle probe row verbatim
  (command + output, exit 0).

### 1.4 BDD scenarios (user-observable)

- Given a `defrecord` type, when positionally constructed, then
  field keywords read back and `assoc` returns the same type.
- Given a protocol extended to a type, when its method is called
  on an instance, then the impl runs; when called on an
  unextended type, then a no-impl `IllegalArgumentException`
  names protocol, method, and type (oracle text).
- Given `with-open` over a closeable, when the body throws, then
  close still runs and the original throw propagates.
- Given a multifn with two methods and a `prefer-method`, when
  dispatched, then exact-or-default wins and the preference is
  recorded (ambiguity resolution needs hierarchies — the
  ambiguity-error scenario rides with that follow-up).

## Part 2 — Technical specification

### Stages and slices

- **Stage 1** (`types.rs` + `TYPE_NATIVES`, `Value::HostType`,
  map-ops integration in `coll.rs`/`lazy.rs`): `deftype`
  (positional fields, inline fns with `this`), `defrecord`
  (plus map-ness + both factories), `reify` (anonymous
  `HostType`), `proxy` (fenced superclass check + method table),
  `gen-class` (name/constructors/methods descriptor +
  constructor fn), `gen-interface`/`definterface` (method-signature
  descriptors usable by `proxy` and dispatch). Plus a **fenced
  port-type call surface** using faithful JVM syntax, dispatching
  only on port-registered types (host interop stays N1):
  `(TypeName. args)` positional constructors (wrong count →
  `No matching ctor found for class <name>`, oracle
  `dtor-arity1`), `(.-field inst)` positional field reads
  (oracle `dtor-field`), `(.method inst args)` method-table
  dispatch (oracle `dtor-meth`, `reify-meth`, `proxy-iface`).
  Anything else in `.`/`new` position keeps the existing
  unresolved error. Oracle findings baked in: `definterface`
  sigs exclude `this` (it would become a phantom parameter)
  with one arglist per entry (repeat the name for overloads);
  `deftype`/`reify` bodies include `this`; `proxy` bodies
  exclude `this`; record `=` is same-type-only (oracle
  `rec-eq-map` is false); runtime top-level `gen-class` is not
  constructable on the JVM (oracle `genclass`
  ClassNotFoundException), so the port's constructable
  descriptor is a documented divergence.
- **Stage-1 implementation deltas** (found while building; all
  pinned in `rust/crates/clj-eval/tests/types.rs`, 84 tests):
  `parse-type-specs` kept the interface list across method specs
  (the recur slot was `triples`, wiping ifaces); proxy fns run
  without `this` (`HostDef::takes_this == false`, honored by both
  `native_meth` and `host_str`); proxy arities count the full
  argv (no phantom `this`); factories are named closures
  (`user/evalN/->Point--M`, oracle `rec-arity` shape);
  `map->` accepts the captured descriptor value, not just a
  symbol; `with-meta`/`meta` work on records (deftypes stay
  non-`IObj`); `empty` on records is `UnsupportedOperationException`
  (oracle `h-empty`); default `toString`/`hashCode` resolve with
  no interface entry (oracle `j-tostring-dt`,
  `l-rec-hashcode`); `gen-class`/`gen-interface` `:methods`
  entries may be vectors (as the oracle writes them); `first`
  and stepping seq records through content entries; `..` is a
  pre-existing gap (no port macro, out of stage scope), as are
  `.toString`/`.hashCode` on non-host values, bare dotted
  references (constructors resolve; `str` of one does not), and
  `hash-code` (private; probes use `.hashCode`). Invoke-as-fn
  failures use the port's stable analyzer-cast text (the
  oracle's `DynamicClassLoader` clause is unreachable with one
  loader — documented in-test).
- **Stage 2** (`protocol.rs` + `PROTOCOL_NATIVES`, `core.clj`
  macros, no new `Value` variant): `defprotocol` emits a
  `definterface` var plus a protocol-map var (`:on` the interface
  name, `:sigs` keyword → `{:arglists}` with `this` included,
  `:impls` class-name → method → argc → fn, `:interface` the
  descriptor) plus one variadic dispatch fn per method (every call
  reaches `__protocol_invoke*`, so its oracle arity error fires,
  never the fn's own `ArityException`); `extend`/`extend-type`/
  `extend-protocol` merge into `:impls` under the port's var-alter
  lock (read-modify-install, no lost update); `satisfies?`/
  `extends?` read the registry plus inline claims; inline
  `deftype`/`defrecord`/`reify`/`proxy` bodies resolve protocols
  through the shared interface path (the `:interface` descriptor
  carries the `:on` claim and past-`this` sigs, so no new check
  or registration path was needed) and dispatch through the
  instance's own method table; `Closeable` is a built-in
  interface descriptor (not a protocol); `with-open` is a
  `core.clj` macro nesting `let`/`try`/`finally` (reverse-order
  closes); `String`/`Long`/`Double`/`Boolean`/`Object` resolve as
  class-name strings (the port has no class literals). Dispatch
  keys types by `jvm_class_of` (port types and host classes share
  one space; `nil` is `null`); the only interface fallback is
  `IPersistentMap` for maps/records (oracle `c-map`); descriptors
  never dispatch or satisfy (like the JVM's classes). The write
  path reuses the loom-modelled var-alter lock (`loom_var_lock.rs`
  covers the primitive; the merge is pure under it).
- **Stage-2 implementation deltas** (found while building; all
  pinned in `rust/crates/clj-eval/tests/protocols.rs`, 54 tests):
  `:arglists` are lists (JVM shape), not vectors; the `extend`
  map form stores bare fns (one per method — each covers every
  declared arity, dispatching itself) with keyword keys (macros
  emit string keys with argc maps); `extend-protocol` accepts
  `nil` group targets (null extensions) and passes keywords
  through to the native's `is not a protocol` error (oracle
  `e-ext-nonproto`); dotted symbols (`clojure.lang.IPersistentMap`)
  ride as-is instead of gaining a `java.lang.` prefix; a bound
  string in type position names its class (the core class-name
  vars); class-name vars moved one stage-1 rejection from
  unresolvable to the accurate `Not an interface` (a bound
  non-descriptor IS a type — except instances, which are not
  types at all and keep `Not a type`); a later `extend-type`
  replaces the inline impl (JVM order — registry reads before
  the instance table); multi-argvec sigs (`(m [this] [this x])`)
  parse to all leading vectors; `class` stays out of scope
  (oracle `a-fnclass` untestable until type reflection lands).
- **Review-driven deltas (rounds 1–2)** (all pinned alongside
  the above): a
  protocol map is only valid with the full constructor shape
  (`:on`, `:sigs`, `:impls`, and the `:interface` descriptor —
  partial hand-built maps are `not a protocol`, never a dispatch
  panic); dot-method calls on descriptors miss with the
  field/method error (descriptors carry no instance methods);
  unresolvable non-dotted type names stay unresolvable (no
  phantom `java.lang` extension — the bound class-name vars
  cover the host values that exist).
- **Stage 3** (`multi.rs` + `MULTI_NATIVES`,
  `Value::MultiFn`): `defmulti` (dispatch fn, default, hierarchy
  slot reserved), `defmethod` (arities incl. multi-arity),
  `prefer-method` (dominance), `get-method`/`methods`/
  `remove-method` (table views). Ambiguity errors arrive with
  the hierarchy follow-up (unreachable without ancestors).
- **Stage-3 implementation deltas** (found while building; all
  pinned in `rust/crates/clj-eval/tests/multimethods.rs`, 20 tests):
  table updates install a new var root under the var-alter lock
  (read-modify-install, no lost update), so captured values keep
  old tables — a documented divergence from the JVM's mutated
  object; mutating natives take a var or a call-time-resolved
  symbol, anything else failing the `MultiFn` cast (oracle
  `e-prefer-nonmulti`), and multifn-valued non-symbol expressions
  are rejected there too (a bare value has no var to install to
  — documented divergence); dispatch is exact entry else default
  entry (`method_for`), prefers stored but never consulted, so
  ambiguity errors are unreachable; misses name the unqualified
  multifn and the dispatch value's print form (oracle
  `e-nomethod`); `get-method` falls back to the default entry
  before nil (`b-get-miss`); `methods` includes the `:default`
  entry (`b-default-entry`); `remove-method` on a missing entry
  is a no-op (`c-remove-miss`); all three mutating forms answer
  the updated multifn; multifns compare by identity, print the
  `#object[clojure.lang.MultiFn …]` shape with name-derived
  stable hex, and are never maps; `defmulti` reuses
  `finish-type-def` (docstring/attr onto the var — the attr map
  now evaluates, like the JVM, fixing the shared helper for the
  stage-1 definers too), accepts only `:default` (`:hierarchy`
  rejected), and answers the var; `defmethod` validates tails
  (`defmethod requires a function tail`, `Invalid defmethod
  arity`) and resolves symbol targets through `(var …)`, so
  missing multifns fail in the analyzer; `class` stays out of
  scope (the `f-*` pins are oracle-only — the class-name vars
  ride as dispatch keys instead).

### TDD plan

- Oracle probes first per stage (`types.clj`, `protocols.clj`,
  `multimethods.clj` against the alpha10 jars, exit 0), committed
  before implementation; row flips only with green gates.
- Split across isolated-worktree workers per stage if schedule
  demands (parent owns `core.rs`/`ns.rs`/`value.rs` merges, as in
  #32); stages order strictly, so stage 2+ starts after stage 1
  lands in the branch.

### Constraints

- LLVM only; EPL-1.0 headers; MSRV 1.85 (no let-chains); signed
  commits with `Ref #33`; 100% line+patch coverage on new code;
  complexity ratchet (rca 0.0.25, cyclo ≤ 15); arm-free asserts
  only in `#[cfg(test)]`; oracle command+output for every new
  test; review 5-round cap with the two-account loop; never
  land/close from the loop — land only on explicit approval.
- New `Value` variants ride `=`/hash/print/compare only via
  explicit arms with equiv-consistency tests (no silent
  catchalls).
