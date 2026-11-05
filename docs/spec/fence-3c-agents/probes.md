# Fence 3c oracle probes (JVM Clojure 1.12.3)

All run with `java -cp clojure-1.12.3.jar:spec.alpha-0.5.238.jar:core.specs.alpha-0.4.74.jar clojure.main`.
Each result is pinned by a test in `rust/crates/clj-eval/tests/concurrency.rs`
(or `classemu.rs` for ancestry).

## P1 — `send-via` validation

`(send-via <ex> a f)` for `ex` in `[nil 42 "x" :e]`:

- `nil` → SENT (returns the agent; the action never runs — see P2).
- `42` → `ClassCastException | class java.lang.Long cannot be cast to class
  java.util.concurrent.Executor (java.lang.Long and java.util.concurrent.Executor
  are in module java.base of loader 'bootstrap')`.
- `"x"` → same shape with `java.lang.String`.
- `:e` → `ClassCastException | class clojure.lang.Keyword cannot be cast to
  class java.util.concurrent.Executor (clojure.lang.Keyword is in unnamed
  module of loader 'app'; java.util.concurrent.Executor is in module
  java.base of loader 'bootstrap')`.

Pins: `agent_send_shapes` (keyword + long CCE rows).

## P2 — nil-executor consequences

With an `:error-handler` delivering `[class message]` into a promise:

- Handler sees `NullPointerException | Cannot invoke
  "java.util.concurrent.Executor.execute(java.lang.Runnable)" because
  "this.exec" is null`.
- A follow-up `(send a ...)` never runs: `(await-for 500 a)` → `false`,
  `@a` → `0` (the head wedges the queue permanently).

Pin: `send_via_nil_wedges_with_handler_npe` (exact mirror).

## P3 — single-thread executor ancestry

`(Executors/newSingleThreadExecutor)` (JDK 27 reports the `AutoShutdown`
subclass; the port fixes the classic name):

- class: `java.util.concurrent.Executors$AutoShutdownDelegatedExecutorService`
- supers: `#{ExecutorService Object DelegatedExecutorService Executor
  AutoCloseable}`
- bases: `(DelegatedExecutorService)`

Port-fixed class: `...Executors$FinalizableDelegatedExecutorService`
(pre-`AutoShutdown` JDKs), supers minus `AutoCloseable`.
Pin: `executor_ancestry_rows` (classemu).

## P4 — `send-via` error order

`(send-via 42 1 inc)` → the *agent* CCE wins:
`ClassCastException | class java.lang.Long cannot be cast to class
clojure.lang.Agent (...)`. The `^Agent` hint checkcasts first.

Pin: `agent_send_shapes` (order row).

## P5 — await / shutdown matrix (earlier battery, re-stated)

- `send-via` with a custom executor runs the action there (observed
  `pool-1-thread-1`).
- `shutdown-agents` returns in ~0.09ms with actions pending.
- `await` on an already-failed agent throws `RuntimeException | Agent is
  failed, needs restart` (not a hang, not the action error).
- Post-shutdown: `await` hangs; `future-call` throws
  `RejectedExecutionException`; `send` is accepted-but-never-runs;
  agent creation works.

Pins: `send_via_custom_executor_survives_shutdown`,
`await_fail_during_hangs_until_restart`, `await_post_shutdown_hangs`,
`future_rejected_after_shutdown` (pre-existing).

## P6 — in-txn `send-via` (deferral preserves the executor)

`(dosync (alter r (constantly 5)) (send-via nil a (fn [_] @r)))`
then `[(await-for 500 a) @r @a]`:

- commit succeeds: `@r` → `5`;
- handler sees the P2 NPE;
- `(await-for 500 a)` → `false`, `@a` → `0` (wedges at
  commit-dispatch, like the out-of-txn send).

With `(Executors/newSingleThreadExecutor)` instead of `nil`:
`(await a) [@r @a]` → `[5 5]` (runs at commit).

Pins: `send_deferral` (`t-sendvia-defer`, `t-sendvia-custom-defer`).
