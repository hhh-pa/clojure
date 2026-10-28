# S14 type internals (zero-oos slice 14)

Twenty `clojure.core` vars with no slice: five positional
factories for `clojure.lang` internals (`->ArrayChunk`,
`->Eduction`, `->Vec`, `->VecNode`, `->VecSeq`) plus
`EMPTY-NODE`; eight proxy helpers (`construct-proxy`,
`get-proxy-class`, `init-proxy`, `proxy-call-with-super`,
`proxy-mappings`, `proxy-name`, `proxy-super`,
`update-proxy`) plus `method-sig`; and five
protocol-introspection fns (`-cache-protocol-fn`,
`-reset-methods`, `find-protocol-impl`, `find-protocol-method`,
`extenders`).

Ground from #33: `HostDef`/`HostVal`/`HostKind`/`HostMethod`/
`HostInst` (`clj-value/src/host.rs`), `proxy` as `Object`-only
`__proxy*` instances (`takes_this: false`, `{ns}$proxy` names),
protocols as `:on`/`:sigs`/`:impls`/`:interface` maps with
`:impls` keyed by `jvm_class_of` strings.

Oracle: `probes/{factories,proxy,protocols,lattice}.{clj,out}`
(alpha10 jars). Probe labels below name the pins each behavior
replays.

## Factories and EMPTY-NODE

- `->Eduction [xform coll]` returns the port's native
  `Eduction` value (`xform.rs`, the same value `(eduction
  xform coll)` builds — the JVM `(Eduction. xform coll)`
  likewise). Probes `a-eduction-class`, `b-eduction-args`,
  `c-eduction-1`, `h-ed-seq`, `h-ed-first`.
- `->VecNode`/`->Vec`/`->VecSeq`/`->ArrayChunk` construct
  opaque handles: `HostType` instances of synthetic defs named
  `clojure.core.VecNode`/`Vec`/`VecSeq`/`ArrayChunk` (probe
  `a-*-class`), declared fields per the JVM deftypes
  (`[edit arr]`, `[am cnt shift root tail _meta]`,
  `[am vec anode i offset _meta]`, `[am arr off end]` — probes
  `b-*`, `f-*`), content keyed by field keyword so `.-field`
  reads work through the shared `field_hit` path. Printing is
  the shared `#object[Name 0xH "Name@H"]` fallback (probes
  `d-empty`, `d-vecnode`). Arities are the JVM positional
  counts (2/6/6/4, probes `c-*`); the house arity text answers
  misses.
- Handles carry a new `HostKind::Handle` variant with VALUE
  equality (same class name plus equal field content), so the
  BDD `(= (->VecNode nil) EMPTY-NODE)` holds. Records keep
  their map behavior; every other instance kind keeps identity
  equality. Hashing reuses the instance content mix, so
  value-equal handles hash equal.
- `EMPTY-NODE` is `(def EMPTY-NODE (->VecNode nil
  <32 nils>))` in `core.clj`, mirroring `(VecNode. nil
  (object-array 32))` (probes `a-empty-class`, `f-arr-count`).
- `->VecNode` also accepts 1 arg (`[edit]`, `arr` defaulting
  to the 32-nil vector): a port extension the BDD's
  `(->VecNode nil)` spelling requires. The JVM 1-arity call
  throws `ArityException` (probe `c-vecnode-1`); the port
  answers the defaulted handle instead. Pinned as a
  deliberate divergence.

Divergences (all pinned, with the JVM value in test
comments): handles are opaque, not live gvec values —
`(vector? (->Vec …))` is false (JVM true, probe `g-vec`),
`(count chunk)`/`(nth …)`/`(seq vecseq)` take the house
not-seqable errors (probes `h-chunk-count`, `h-chunk-nth`,
`h-vec-count`, `h-vecseq-seq`), and `(= EMPTY-NODE
(->VecNode nil (object-array 32)))` is port-true by value
(JVM false by identity, probe `e-fresh`). `->Eduction`
prints the port `#object[…]` shape (house-wide `eduction`
behavior, T1200), where the JVM prints the items readably
(probe `d-eduction`); likewise `sequential?` on eductions is
port-false (JVM true, probe `g-sequential-ed`) — both are
the established native-`Eduction` behavior, not this
slice's to change.

## Proxy helpers

- `proxy-name [super interfaces]` answers the JVM name
  string: munged `*ns*` + `.proxy` + `$`-joined
  super name, simple interface names (sorted set), and the
  lower-hex set hash (probes `a-name-*`). Inputs are
  class-name strings or `HostDef` values (anything answering
  a class uses class-name strings until S16's `Class` values
  land); anything else is the shared `cce_message` `Class`
  cast (the exact JVM text, probe `a-name-noclass`), nil
  included (the JVM NPEs, probe `a-name-nil` — the JDK text
  is unpinnable, so the cast stands).
- `get-proxy-class [& bases]` answers the proxy class-name
  string (S16 upgrades answers to `Class` values). The first
  base is the superclass unless it is an interface def, in
  which case the superclass defaults to `Object` (probes
  `b-default-super`, `b-name-iface`); a first STRING names
  the superclass only when it is exactly
  `"java.lang.Object"`, otherwise it counts as an interface
  (strings cannot report `isInterface`, so the Object fence
  decides — a non-Object superclass string would be illegal
  there anyway). Non-Object superclass slots are the
  `__proxy*` `Cannot extend …: proxy supports Object only`
  error (the JVM raises `IncompatibleClassChangeError` for
  final `String`, probe `b-final` — unreachable here since
  only `Object` passes). Interface slots accept interface
  defs and (unverifiable) strings; non-interface defs are
  `Not an interface: …`, anything else casts to
  `java.lang.Class` (probe `b-nonclass`). Zero bases answers
  the `Object`/no-interface name: the docstring's "class
  defaults to Object" honored (the JVM NPEs, probe `b-none`
  — treated as the accident it is).
- `construct-proxy [c & ctor-args]` mints a proxy instance
  whose def is named `c` (probe `c-class`): `takes_this:
  false`, no claimable interfaces (the name does not parse
  back), empty method table, and a NIL fn map (probe
  `c-mappings-fresh`). `c` must be a class-name string
  (else the shared `Class` cast, probe `c-nonclass`); any
  ctor args are `No matching ctor found for class {c}`
  (probe `c-ctor-args`).
- Proxy instances grow a `proxy_map: Option<Atom>` cell on
  `HostInst` (present exactly on proxies — `__proxy*`
  installs it, `construct-proxy` installs nil-held). The
  cell holds the `{string-name → fn-or-nil}` map VERBATIM
  (JVM shape): `__proxy*` pre-populates single-overload
  methods as their fn and multi-overload methods as the
  `{argc → fn}` submap (a port shape for per-arity fns,
  pinned); `init-proxy` replaces the map, `update-proxy`
  assocs onto it (`nil` values stay visible, probe `d-nil`;
  over a nil map `assoc` builds from `{}`, like the JVM).
  `proxy-mappings` reads the cell (nil until initialized).
  Non-proxy first args are the shared `cce_message`
  `IProxy` cast; non-map mappings are the
  `IPersistentMap` cast (exact JVM texts — probes
  `d-init-nonproxy`, `d-init-nonmap`, `d-update-nonproxy`,
  `d-update-nonmap`, `d-mappings-nonproxy`). Both mutators
  return the identical proxy (probe `d-init`).
- Dispatch consults the cell for proxies (`takes_this ==
  false`): a fn (or any non-map non-nil value, called
  as-is) wins; a `{argc → fn}` submap dispatches per arity;
  nil/missing falls to the `Object` super/default path —
  never to the baked `def.methods` table, so `update-proxy`
  removal truly reverts to super (JVM semantics). Macro
  `proxy` behavior is unchanged (the cell starts as the
  baked methods, probe `d-macro-map`). `str`/`toString`
  consults the cell the same way.
- `proxy-call-with-super [call this meth]` assocs `meth` to
  nil, invokes `(call)`, and restores the map in a `finally`
  (probe `f-super`, `f-restored`); `this` must be a proxy
  (same `IProxy` cast). `meth` is unvalidated (like the
  JVM's `assoc`).
- `proxy-super [meth & args]` is a macro expanding to
  `(proxy-call-with-super (fn [] (.meth this args…)) this
  "meth")` — the JVM expansion modulo the call shape (probe
  `f-macro` shows `(. this meth …)`, whose `.`-head
  interop form does not exist on the port). It captures
  `this`, so on the port — where proxy method fns exclude
  `this` (#33) — it resolves wherever the caller binds
  `this` (pinned via an explicit binding; probe
  `f-supermacro` shows the JVM call working).
- `method-sig [triple]` ports the JVM triple shape
  `[name-string params-seq-or-nil return]` (probes
  `e-tostring`, `e-valueof`, `e-shape`). The JVM input
  (`java.lang.reflect.Method`) cannot exist behind the
  interop fence, so the port input is a descriptor triple
  `[name params return]` (vector or seq): malformed input
  is `IllegalArgumentException: method-sig requires a
  [name params return] triple`, a non-string name is the
  shared `cce_message` `String` cast, non-nil non-sequential
  params are `IllegalArgumentException: method-sig params
  must be nil
  or sequential`. The answer re-seqs params (`[]` → nil).

## Protocol introspection

- `find-protocol-impl [protocol x]` ports the JVM walk over
  the `:impls` registry: a non-protocol first arg is the
  house `… is not a protocol` (the JVM NPEs with JDK-only
  text, probes `a-nonproto`/`b-nonproto` — unpinnable, and
  consistent with `satisfies?`/`extends?`). An `x` whose
  instance def claims the protocol's `:on` answers `x`
  itself (probe `a-inline`); otherwise the class walk runs —
  exact `jvm_class_of`, then superclass chain (excluding
  `Object`), then `pref`-reduction over implemented
  supers (excluding `Object`), then `Object` — over the
  probed lattice (`lattice.out`: chains plus transitive
  supers for every port class, interfaces included for
  `pref`). Unknown names (port types, arbitrary strings)
  default to `[Object]` with their listed interfaces (port
  `deftype`/`defrecord` instances also claim
  `clojure.lang.IType`, probe `l-deftype`); `nil` walks
  nothing but still hits an explicit `"null"` entry (probe
  `f-nil-call`). Port registry entries (`{str → {argc →
  fn}}`) normalize to the JVM `{keyword → fn}` shape (probe
  `a-exact-keys`): empty overload maps are skipped;
  multi-arity methods answer the SMALLEST-arity fn
  (documented, pinned). A miss answers nil (probes
  `a-miss`, `a-nil`).
- `find-protocol-method [protocol methodk x]` is literally
  `(get (find-protocol-impl protocol x) methodk)` (probes
  `a-exact-call`, `a-long-call`, `a-long-missing`,
  `a-super-call`, `a-iface-call`, `a-object-call`,
  `b-inline-nil`, `b-str-key`, `b-missing-k` — including
  exact-entry-wins-wholesale and inline→nil).
- `extenders [protocol]` answers `(keys (:impls protocol))`
  as class-name strings (S16 upgrades to classes): map
  iteration order, nil when empty (probes `c-list`,
  `c-empty`). Total like the JVM: non-protocols answer nil
  (probes `c-nonproto`, `c-nil`).
- `-cache-protocol-fn [pf x c interf]` ports the cache-fill
  step; the port keeps no `MethodImplCache` (every protocol
  call reads the live registry), so it performs the lookup
  and returns `f` without storing. `defprotocol` method
  fns register `(closure-id → var, method-name)` in the
  evaluator via a hidden `__protocol_method*` call the
  macro emits after each `defn`. Checks run left to right
  (probe `G`): a non-fn `pf` is the shared `AFunction` cast (exact JVM text);
  an unregistered fn is `IllegalArgumentException: Not a
  protocol fn` (the JVM NPEs reading the null cache, probe
  `d-badpf` — JDK text unpinnable); `c` takes class-name
  strings and defs (else the shared `Class` cast); a
  non-IFn `interf` is the shared `IFn` cast. An `x`
  implementing `c` (lattice membership) answers `interf`
  (probe `d-interf`); otherwise the `find-protocol-method`
  hit answers (probe `d-hit`), and a miss throws the
  oracle `No implementation of method: :m of protocol:
  #'p found for class: C` (nil `x` prints `nil` — probe
  `d-nil`; note the pre-existing `__protocol_invoke*` path
  prints `null` there, #33's fence, left alone).
- `-reset-methods [protocol]` answers nil; total like the
  JVM (probes `e-ok`, `e-nonproto`, `e-nil`) — there is no
  method cache to rebuild. Arity 1 (probe `e-arity`).

## Seams

- Anything answering a class uses class-name strings until
  S16's `Class` values land; S16 owns that upgrade
  (`get-proxy-class`, `construct-proxy`'s `c`,
  `proxy-name`'s inputs, `extenders`, `-cache-protocol-fn`'s
  `c`). Pins here name strings.
- Derive hierarchies are S13 (multifn dispatch), a separate
  path from the protocol class walk; the walk never consults
  hierarchies.
- Protocol dispatch through proxy instances reads the baked
  method table (`inline_impl`, #33), not the live proxy map
  cell: `update-proxy` removal does not affect protocol
  calls on proxies. Pinned as a known limitation.

## BDD scenarios

- Given `(->VecNode nil)`, it builds a handle `EMPTY-NODE`
  equals.
- Given `(init-proxy p {"toString" (fn [] "hi")})`,
  `(proxy-mappings p)` round-trips the map and `(str p)`
  is `"hi"`; `update-proxy` removal restores super
  behavior.
- Given `(extend-type String P …)`, `(find-protocol-method
  P :m "x")` finds the impl up the class walk (exact,
  superclass, interface, and `Object` steps).
- Given the same, `(extenders P)` lists the extended
  class-name strings.

## Tests

`rust/crates/clj-eval/tests/typeinternals.rs`: one test
group per var replaying its probe labels verbatim, plus
shared arity-miss tests and the seam/limitation pins.
Matrix `notes` name the tests.

## Matrix

Twenty rows in the `### Previously unlisted — accepted
future work` alpha table, ASCII-sorted by var cell,
flipped straight to `implemented` with test notes; header
counts recounted to actuals.
