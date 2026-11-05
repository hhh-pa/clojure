# Vars contract (S5B, normative)

Var completeness: roots, thread bindings, and metadata. Oracle:
probe37 (140 tx). Wrappers are the S4 ones; only the rules below
are new. `clone-thread-bindings` does not exist on the JVM
(probe37 `clone-missing`) and is out of scope.

## Roots (`alter-var-root`, `var-get`, `var-set`)

- `alter-var-root` applies `f` to the current root (never the
  thread-local value, probe37 `alter-root-sees`) consed with the
  extra args, then installs the result as the new root and returns
  it (`avr-basic`, `avr-args`). An unbound root reaches `f` as the
  `Unbound` object, not a throw (`avr-unbound-arg`); a throwing `f`
  leaves the root untouched (`avr-fail-keeps`). Non-var first arg
  is the `Var` cast, non-callable `f` the `IFn` cast (`avr-non-var`,
  `avr-non-fn`). Concurrent alters of one var serialize on the
  var monitor while unrelated vars proceed in parallel;
  same-thread nesting re-enters and the outer install wins
  (`fx1-alter` probes, T1192 FX1).
- `var-get` is thread-local-or-root and never throws: unbound
  reads the `Unbound` object (`vget-unbound`) — like `deref`
  (`vget-vs-deref`). Non-var is the `Var` cast (`vget-non-var`).
- `var-set` writes the top thread frame and returns the value
  (`vset-basic`); without a frame — including a never-bound var, a
  non-dynamic var, or another thread's frame — it is
  `IllegalStateException | Can't change/establish root binding of:
  {name} with set` with the UNQUALIFIED name (`vset-unbound-thread`,
  `vset-nondynamic`, `vset-cross-thread`), the same shape as `set!`.
  Non-var is the `Var` cast (`vset-non-var`).

## Predicates (`bound?`, `thread-bound?`)

- Both take 0+ vars and AND the results: 0 args is true
  (`bound-none`, `tb-none`), any unbound is false (`bound-mixed`).
  Each arg casts to `Var` in order, short-circuiting on the first
  false like `every?` (`bound-non-var`, `tb-non-var`).
- `bound?` is root-or-thread-local (`bound-thread-local`);
  `thread-bound?` is thread-local only (`tb-inside`). `with-redefs`
  binds neither (`tb-redefs`).

## Thread-binding frames

- `push-thread-bindings` validates the whole map before pushing
  anything: non-`Associative` arg is the `Associative` cast
  (`push-non-map`), nil is the `NullPointerException` off
  `Associative.seq` (`push-nil`), non-var keys are the `Var` cast
  (`push-non-var-key`), and non-dynamic vars are
  `IllegalStateException | Can't dynamically bind non-dynamic var:
  {qualified}` (`push-non-dynamic`, `push-unbound-dynamic`). A
  mixed map binds nothing (`push-atomic`). Empty maps push an empty
  frame (`push-empty`); nested pushes shadow (`push-nested`). A
  vector arg is an `Associative` whose elements must be map
  entries: empty pushes nothing, non-empty fails the first
  element's `IMapEntry` cast (unprobed corner, JVM `Frame` ctor).
- `pop-thread-bindings` pops one frame and returns nil
  (`pop-ret`); an empty stack is `IllegalStateException | Pop
  without matching push` (`pop-empty-fresh-thread`). Base frames
  pop like any other (main-thread pop rule probe).
- `get-thread-bindings` snapshots the visible Var→value map, a
  fresh object per call (`gtb-snapshot`), `{}` when empty
  (`gtb-fresh-thread`).
- `with-bindings*` pushes its map (same validation, `f` never runs
  on failure, `wbs-non-dynamic`), calls `f` with the extra args,
  and pops in a `finally` (`wbs-basic`, `wbs-args`, `wbs-throw`).
  Arity is 2+ (`arity-wbs-1`).

## Binding forms (builtins, vars land in S5C)

- `with-redefs` is a PARALLEL ROOT SWAP, not thread bindings:
  init values evaluate against the old roots, roots install, the
  body runs, and the saved roots (or rootlessness) restore in a
  `finally` — even on throw (`wrd-basic`, `wrd-parallel`,
  `wrd-throw`, `wrd-unbound`). Inside the body the vars are not
  thread-bound (`tb-redefs`) and an outer thread binding still
  shadows (`wrd-thread-shadow`). A repeated var keeps the last
  value (map semantics); an odd trailing binding silently drops,
  like the macro's `zipmap` (`wrd-odd`). Empty bindings run the
  body (`wrd-empty`). Binding targets resolve exactly like `var`:
  non-symbols are the `Symbol` cast and misses are `Unable to
  resolve var`, both under `Syntax error compiling var`
  (`wrd-num`, `wrd-missing`).
- `with-bindings` evaluates its map expression (computed maps
  work, `wb-computed-map`) and runs the body over one pushed frame
  (`wb-basic`, `wb-throw`); empty body is nil (`wb-empty-body`).
- `bound-fn` snapshots the thread bindings at CREATION and
  re-pushes them as one frame around every invocation — on any
  thread (`bf-cross-thread`), including `*ns*` (`bf-ns-convey`),
  and even on the creating thread outside the original bindings
  (`boundfn-outside` probe). Inner `binding` still shadows
  (`bf-inner-rebind`).

## Metadata

- Fresh interns carry `{:ns <ns>, :name <sym>}`
  (`fresh-intern-meta` probe); `def` adds `:line`/`:column`/`:file`
  plus user meta and `:doc` (S4, now also `:ns`). Boot vars stamp
  `:name`/`:ns` (`:dynamic`/`:private` where apt).
- `meta` reads Symbols, lists/vectors/maps/sets, functions, vars
  (nil once cleared), and namespaces (nil when fresh, `meta-ns`);
  every scalar, keyword, `nil`, var-value, and default fn reads nil
  (`meta-scalars`, `meta-fn`). Reader `:line`/`:column` rides
  lists only (`meta-colls`, S2 already).
- `with-meta` targets `IObj` only (collections, symbols,
  functions): nil is the `withMeta` NPE (`wm-nil-obj`), anything
  else non-`IObj` — numbers, vars, namespaces — is the `IObj` cast
  (`wm-num`, `wm-var`, `with-meta-ns` probe). The meta arg casts to
  `IPersistentMap` (`wm-non-map`); nil clears (`wm-nil`); the
  result keeps value, type, and callability (`wm-basic`, `wm-fn`).
  The object checks before the meta arg (receiver first).
- `vary-meta` applies `f` to `(meta, args...)` and `with-meta`s
  the result: nil meta reaches `f` as nil (`vm-nil-meta`), a
  non-map result is the `IPersistentMap` cast (`vm-args`,
  `vary-nonmap-ret` probe), non-`IObj` is the `IObj` cast
  (`vm-non-meta`). Arity is 2+.
- `alter-meta!`/`reset-meta!` target `IReference` (vars and
  namespaces; atoms don't exist yet): anything else is the
  `IReference` cast (`am-non-ref`, `rm-non-ref`). `alter-meta!`
  applies `f` to `(current-or-nil, args...)` and installs the
  result — nil stays nil (`am-nil-meta`), non-map non-nil is the
  `IPersistentMap` cast (`alter-nonmap-ret` probe) — computing
  before installing so a throwing `f` changes nothing.
  `reset-meta!` requires a map or nil (`rm-nil`, `rm-non-map`).
  Both return the new meta (`am-basic`, `rm-basic`) and wipe
  `:line` when it isn't carried over (`rm-basic`).

## Fences

- `declare`/`ns` stay `planned` for S5C (real macros over
  `with-redefs`-style internals; probe37 `declare-*`/`ns-*` pin
  their behavior, including the `ns` expansion shape).
- `with-redefs`/`with-bindings`/`bound-fn` analyze and evaluate in
  S5B; their vars resolve (as macros, like `binding`) in S5C.
- Validator fns (`set-validator!`) don't exist yet: roots and
  bindings install unchecked.
- Bindings conveyance is same-thread snapshotting; real threads
  arrive later (the single-threaded port has no thread id).
