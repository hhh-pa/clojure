# S5 state introspection (zero-oos slice 5)

Fifteen upstream vars over existing machinery, no new value
kinds: `disj!` `reset-vals!` `swap-vals!` `error-handler`
`error-mode` `future-cancelled?` `get-validator`
`set-validator!` `find-var` `requiring-resolve`
`release-pending-sends` `await1` `agent-errors`
`clear-agent-errors` `replicate`.

Target universe rows come from PR #41's 691-row inventory
(unlanded): 10 from `Previously unlisted — accepted future
work`, 5 from `Previously unlisted — no slice yet`. This
slice adds its 15 rows to the wave-1 matrix in one fenced
`Previously unlisted — S5` block (PR #41 relative order),
flipped straight to `implemented`. The #41 land unions them
later.

## Semantics (oracle: `probes/state.{clj,out}`, alpha10)

- `error-handler` / `error-mode` — 1-arity reads of the
  agent cell (`error_handler` fn-or-nil, `error_mode`
  `:fail`/`:continue`). Nil is the `Agent.getErrorHandler()`
  / `getErrorMode()` NPE; others the `Agent` cast
  (`a-eh-*`, `a-em-*`).
- `future-cancelled?` — 1-arity over `FutureState::Cancelled`
  (new `Future::is_cancelled`; `get()` blocks so it cannot
  serve). Nil is the `Future.isCancelled()` NPE; others the
  `Future` cast (`b-*`).
- `get-validator` — 1-arity IRef read (atoms + agents, the
  house `as_iref` set shared with `add-watch`). Futures,
  delays, and non-refs are the `IRef` cast; nil is the
  `IRef.getValidator()` NPE (`c-get-*`).
- `set-validator!` — 2-arity IRef write in checkcast order:
  the receiver casts to `IRef` (nil passes through), then
  the fn casts to `IFn` (nil clears), then the null receiver
  trips the `IRef.setValidator(IFn)` NPE (`k-set-nil-badvf`
  is the `IFn` CCE, `k-set-bad-badvf` the `IRef` CCE); the
  install then validates the current value (`Invalid
  reference state`, old validator kept) and publishes,
  returning nil like the `void` setter (`c-set-*`). Vars are
  NOT accepted (see Non-goals).
- `reset-vals!` — `reset!` returning `[old new]` (validates,
  fires watches with `[old new]`). Nil is the
  `IAtom2.resetVals(Object)` NPE; others the `IAtom2` cast
  (`d-reset-*`).
- `swap-vals!` — `swap!` returning `[old new]` over the same
  CAS loop (shared helper). Nil is the arity-dependent
  `IAtom2.swapVals` NPE; nil `f` the `IFn.invoke` NPE
  (`d-swap-*`).
- `disj!` — 1-arity identity (no check, like the JVM decl),
  else `TransientSet::disjoin` over 1+ keys, returning the
  transient. Nil is the `ITransientSet.disjoin(Object)` NPE;
  persistent colls and transient maps/vectors are the
  `ITransientSet` cast; use-after-`persistent!` is the
  `IllegalAccessError` (`e-*`).
- `find-var` — 1-arity: qualified symbol to interned var
  (live namespaces only, refers blind — `f-refer-blind` is
  nil). Missing namespaces throw `No such namespace: X` and
  unqualified symbols `Symbol must be namespace-qualified`
  (both `IllegalArgumentException`); nil is the JVM field-read
  NPE (`Cannot read field "ns" ...`); non-symbols are the
  checkcast-shape `Symbol` cast (`f-*`).
- `requiring-resolve` — 1-arity: qualified symbols only
  (else `IllegalArgumentException Not a qualified symbol: X`,
  nil rendering empty); resolve in the current ns, else
  `require` the ns part and resolve again. Loader failures
  propagate in port shapes (the JVM's classpath
  `FileNotFoundException` has no port analog) (`g-*`).
- `release-pending-sends` — 0-arity: drains the running
  action's outbox (`Evaluator::agent_outbox`, armed over fn
  and watches) through the nested-release path and returns
  the dispatched count; outside an action returns 0
  (`h-outside`, `h-inside` is `[1 11]`, `h-ctrl` is `11`).
- `await1` — 1-arity: `(when (pos? queue-count) (await a))`
  returning the agent (new `Agent::queue_count`: queued plus
  running — the count INCLUDES the running head, per
  `k-await1-self-idle`). Empty queues skip the await — and
  its in-action guard — exactly like the JVM decl
  (`k-await1-idle-in-action`, `k-await1-busy-in-action`).
  Nil is the `Agent.getQueueCount()` NPE; others the `Agent`
  cast (`i-*`).
- `agent-errors` — core.clj `(when-let [e (agent-error a)]
  (list e))`, `{:added "1.0" :deprecated "1.2"}`.
- `clear-agent-errors` — core.clj `(restart-agent a @a)`,
  `{:added "1.0" :deprecated "1.2"}` (same unfailed throw
  and return value as `restart-agent`).
- `replicate` — core.clj `(take n (repeat x))`, `{:added
  "1.0" :deprecated "1.3"}` (`j-*` pins value- and
  meta-equivalence with the replacements).

All natives use the house arity guard (JVM-identical text,
pinned per var). Agent action validation now reads the
agent's CURRENT validator at completion (JVM `Action`
semantics); the send-time capture predates `set-validator!`
and goes stale once validators are writable.

## BDD scenarios

- Given an agent/atom/future in each state, each field read
  answers the oracle's value verbatim.
- Given validator set/get/veto/clear/nil-fn replays, atoms
  and agents answer the oracle's values and throws.
- Given `find-var`/`requiring-resolve` inputs (found,
  missing var, missing ns, unqualified, nil, non-symbol),
  resolution and throws replay the oracle; a lib load plus
  resolve round-trips through the port loader.
- Given nested sends inside an action, `release-pending-sends`
  dispatches them early and returns the count; outside an
  action it returns 0.
- Given each deprecated alias, values AND var meta
  (`:added`/`:deprecated`/`:arglists`) match the oracle.
- Given 0/2+ args (respective arities), each native throws
  the house arity text.

## Tests

`tests/zero_oos_s5_state.rs`: one test per var replaying its
probe labels verbatim, plus shared arity-miss coverage.
Matrix `notes` name the tests.

## Non-goals

- Var validators: `get/set-validator!` reject vars with the
  `IRef` cast — the same set `add-watch`/`remove-watch`
  accept today. Full var support (a `VarState` field plus
  enforcement across every root-install path) is a
  follow-up slice, not this one.
- `:static` meta on `await1` (JVM `i-static` is true): house
  natives never carry `:static`; unpinned.
- `requiring-resolve` missing-lib message parity (port
  loader shapes differ from the JVM classpath text).
- New value kinds (none needed); STM-adjacent `io!` guards
  (the port has no transactions).
