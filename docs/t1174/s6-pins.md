# S6 oracle pins (JVM truth, en_US/UTC)

Probes: `/tmp/s6-probe{,2,3,4}.clj` against the repo-built
`clojure-1.13.0-master-SNAPSHOT` jar. `THROW C | m` = class + message.

## P1 delay/force/realized?

- `realized?` before force → `false`; force throwing body →
  rethrows the SAME exception; body runs ONCE (counter stays 1:
  failures are cached, never re-run).
- `realized?` AFTER a throw → **`true`** (surprise: throwing
  still realizes).
- `(force 42)` → `42` (non-delays pass through).
- `(realized? (atom 1))` → `ClassCastException` to
  `clojure.lang.IPending` (atoms are not pending).

## P2 atom/watches/validators

- Ctor options `:validator`/`:meta` work; `(meta (atom 1 :meta
  {:k :v}))` → `{:k :v}`.
- Validator failures (`swap!`/`reset!`/`compare-and-set!`, AND
  ctor initial value): `IllegalStateException | Invalid
  reference state`. CAS validates too.
- `compare-and-set!`: `true`/`false`; on non-atom →
  `ClassCastException` to `clojure.lang.IAtom`.
- `swap!` takes extra args (`(swap! (atom 0) + 1 2 3)` → `6`).
- Watches fire UNCONDITIONALLY (surprise: `(swap! b identity)`
  fires with `[old old]`); `add-watch`/`remove-watch` return
  the ref; watch fn gets `[key ref old new]`.

## P3 future

- Failed future: `realized?`/`future-done?` → `true`; `deref`
  throws `ExecutionException | java.lang.Exception: boom`
  (cause class+message embedded), cached across derefs.
- Bindings convey: `(binding [*dx* 2] @(future *dx*))` → `2`.
- `(deref fut-with-slow-body 50 :t)` → `:t`.
- `future-cancel` → `true`; deref of a cancelled future
  (EVEN with timeout val) throws `CancellationException`
  (empty message). `future-done?` → `true` after cancel.
- `future?`: true of futures only (not delays/values).

## P4 promise

- First `deliver` returns the promise object; second → `nil`;
  value is the first delivery; `realized?` → `true`.
- Timeout deref on pending promise → timeout val (promises
  are Futures).

## P5 agents (+ timeout-deref casting)

- `await` → `nil`; `await-for` ok → `true`, timeout → `false`.
- `:continue`: failed action leaves state unchanged, error
  CLEARED (`agent-error` → `nil`), later sends process.
- `:fail`: `await`/`await-for`/`send` all throw
  `RuntimeException | Agent is failed, needs restart`; state
  parked; `agent-error` non-nil.
- `restart-agent` returns the NEW state (`10`); clears the
  error; `:clear-actions true` drops the queue.
  Bad arity → standard `ArityException`.
- Nested send order: `[:outer :inner]` (nested runs after).
- `send` returns the agent; `shutdown-agents` → `nil`.
- Validator veto inside a `:continue` action behaves like an
  action error (state kept, error cleared, await nil).
- `*agent*` is bound in the action fn, the watches, and the
  error handler; only the validator runs bare.
- 3-arity `deref` casts to `java.util.concurrent.Future`
  FIRST: atoms AND delays throw `ClassCastException` to
  `Future` (only futures/promises accept timeouts).

## P6 setters/locking (`/tmp/s6d-probe{,2}.clj`)

- Both setters return **`nil`** (the `void` Java methods), never
  the agent — including the nil-accepting arities.
- `set-error-handler!` accepts nil (clears); a non-fn is the
  `IFn` CCE (the single overload compiles to a checkcast, NOT a
  Reflector mismatch). The installed handler fires on the next
  failure with `[agent error]`.
- `set-error-mode!` accepts nil (mode reads nil, behaves as
  `:fail` — the worker compares against `CONTINUE`); `:bogus`
  assigns fine and behaves as `:fail`; a non-keyword is the
  `Keyword` CCE.
- Nil agents: `NullPointerException |
  Cannot invoke "clojure.lang.Agent.setErrorHandler(clojure.lang.IFn)"
  because "a" is null` (resp. `setErrorMode(clojure.lang.Keyword)`);
  non-agents: the `Agent` CCE.
- `(locking nil 1)` → `NullPointerException | Cannot enter
  synchronized block because "locklocal__5902__auto__171" is null`
  — the lock name is a GENSYM (PORT-FIXED: the port reports
  `"lock"`). `(locking)` → `ArityException | Wrong number of
  args (0) passed to: clojure.core/locking`; empty body → `nil`;
  multi-body → last value.
- CAUTION: `await` on a `:fail` agent whose barrier won the race
  hangs the JVM (the barrier parks behind the latched error);
  probes must `sleep` for the latch or use `await-for`.
