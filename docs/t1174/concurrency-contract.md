# Concurrency contract (S6, T1174)

C6 minus STM, pinned by `s6-pins.md` (Pn refs). `THROW C | m`
shapes are exact JVM text (en_US/UTC).

## IRef (atom/agent)

- State + `:validator` + `:meta` + watches (`{kw fn}`).
  Validator runs at construction AND every publish (P2/P5):
  veto → `IllegalStateException | Invalid reference state`.
- Watches fire on EVERY publish, even value-unchanged (P2:
  `(swap! b identity)` fires `[old old]`), with
  `[key ref old-val new-val]`. `add-watch`/`remove-watch`
  return the ref. Watch fns run on the publishing thread
  (documented: JVM order across threads is unspecified; the
  port fires synchronously under the publish).
- `deref` (1-arity) returns current state for atoms, agents,
  delays, futures, promises. `realized?` requires `IPending`
  (delay/future/promise); atoms throw the `ClassCastException`
  (P1).
- 3-arity `deref` casts to `Future` first: atoms/delays throw
  `ClassCastException` to `java.util.concurrent.Future` (P5);
  only futures/promises honor timeouts.

## Atom

- `swap!` CAS-loops `(apply f old args*)`; `reset!` publishes
  directly; `compare-and-set!` returns bool (validates on
  match). Extra `swap!` args supported (P2).

## Delay

- Run-once (failures cached, never re-run); throwing still
  realizes (P1). `force` passes non-delays through.
- Re-entrant force (a delay forcing itself) is
  `IllegalStateException | Delay is already being forced`
  (PINNED DIVERGENCE: the JVM recurses until
  `StackOverflowError`; a Rust stack overflow aborts the
  process, so the port guards instead).

## Future/promise

- `future-call` conveys calling-thread bindings (S5B
  `bound-fn`); failures cache; `deref` rethrows
  `ExecutionException | <cause-class>: <cause-msg>` (P3).
- Cancelled futures: `deref` (any arity) throws
  `CancellationException`; `future-done?`/`realized?` true.
- `promise`/`deliver`: first delivery wins (returns the
  promise), later → `nil`; pending `deref` blocks.

## Agents

- `send` (fixed pool, #cores), `send-off` (unbounded pool),
  `send-via` routes to the `send-off` pool (PINNED
  DIVERGENCE: no custom executors). All return the agent.
- `:continue`: failed action keeps state, clears the error,
  queue proceeds. `:fail`: `send`/`await`/`await-for` throw
  `RuntimeException | Agent is failed, needs restart` until
  `restart-agent` (returns new state, clears error;
  `:clear-actions` drops the queue) (P5).
- `await` → `nil` (throws when failed); `await-for` →
  bool. Nested sends run after the enclosing action (P5);
  nested sends from a FAILED action are discarded (the JVM's
  unreleased `nested` buffer — never resurrected by a later
  restart without `:clear-actions`).
- `*agent*` bound inside actions. `shutdown-agents` → `nil`,
  joins pools + future threads (BLOCKING — DIVERGENCE: the
  JVM returns at once and drains async; timing-only).
- After shutdown, sends return the agent but their actions
  never run (like the JVM's swallowed executor rejection —
  the queue wedges, `await-for` times out `false`, plain
  `await` hangs on both runtimes). Submission failures
  notify the error handler with a
  `RejectedExecutionException` first, like the JVM's
  `execute` catch (the message is port-fixed: the JVM's
  embeds pool identity).

## Threads (port rules)

- One `std::thread` per future; agent pools are
  `std::thread` sets over `mpsc` queues (no new crates).
- Per-thread `Evaluator` over shared state (D1 spike picks
  the sharing shape; `Value` is already `Send+Sync`).
- `alter-var-root`'s read-call-install is NOT atomic across
  threads (PINNED DIVERGENCE: the JVM serializes per-var; the
  port cannot hold its non-reentrant `Mutex` across the
  evaluating call, so concurrent alters of one root may lose
  an update). Single publishes (`intern`, atom/agent
  publishes) are atomic.
- Print rendering of refs (`#object[...]`) is covered by
  existing `java_debug` conventions, not re-pinned here.
