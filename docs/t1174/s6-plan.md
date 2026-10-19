# S6 slice plan: concurrency (T1174)

Branch `T1174-s6-concurrency`. Charter: C6 minus STM (`atom` CAS
loops with validators + watches; `future` binding conveyance +
cached deref exceptions; `agent` pools, error modes, `await`,
nested-send ordering), `delay`/`promise` IDeref kin, the S6
`core-coverage.md` flips (A5), differential corpus entries (A2).
TDD: stress/determinism tests first (spec §2.5).

## S6A. Atoms + delay (synchronous core, no threads)

- `Value::Atom` (`Arc`-shared mutable cell: current value,
  `:validator`, `:meta`, watches map) + `atom` (ctor options
  `:validator`/`:meta`), `deref`, `swap!` (CAS loop: apply fn,
  validate, publish; watches fire only on change), `reset!`,
  `compare-and-set!`, `add-watch`/`remove-watch` (`IRef` watch
  fns keyed by keyword), `realized?`.
- `delay`/`force`: run-once thunk with realized flag; exception
  caching and `realized?`-after-throw are oracle-probe items
  (JVM truth first, then pin).
- `memoize` (unlocked by `atom`; coverage row flips here).
- Validators: ctor `:validator` option only (no
  `set-validator!`/`get-validator` rows — stays unlisted).
- Out: `ref`/`dosync` (STM, N4).

## S6B. Futures + promises (first threads)

- `future-call` (takes a zero-arg fn; conveys the calling
  thread's bindings via the S5B `bound-fn` machinery),
  `future` (macro over `future-call`), `deref` on futures
  (blocking; 3-arity timeout `msec timeout-val`),
  `future?`/`future-done?`/`future-cancel` (+ `realized?`),
  `promise`/`deliver` (single-shot latch; `deref` blocks).
- Exceptions: a failed future caches the failure; `deref`
  rethrows (`ExecutionException` shape per oracle probe).
- Threading model (spike first, §Design below): one
  `std::thread` per future vs a shared pool; per-thread
  `Evaluator` sharing registry state.
- `shutdown-agents` also joins future threads (JVM joins both
  pools; probe the exact rendezvous).

## S6C. Agents (pools + error modes)

- `agent` (state + `:validator`/`:error-mode` (`:continue`/
  `:fail`)/`:error-handler`), `send` (fixed CPU-bound pool),
  `send-off` (unbounded pool), `send-via` (caller executor —
  port: routes to the `send-off` pool; pinned divergence),
  `await`/`await-for` (calling-thread rendezvous),
  `shutdown-agents`, `agent-error`, `restart-agent`
  (`:clear-actions` semantics per probe).
- Nested sends from inside an action run after the current
  action (ordering test with a shared log atom).
- Validator failures inside actions follow the error mode
  (probe: failed action + `:fail` parks the agent).
- Out: `set-agent-send-executor!` et al (unlisted), `pvalues`/
  `pcalls` (composites; follow-up if cheap, else S9).

## Design (spike before S6B)

D1. `Evaluator` across threads: `Send+Sync` VERIFIED (probe
    test, removed after). Remaining: the SHARING shape for
    per-thread evaluators (shared registry? atomic counters?).
    Options: (a) clones over a shared `Arc<Mutex<Registry>>`;
    (b) single-owner evaluator with message-passing eval
    requests; (c) `thread::scope` borrowing. The S6B spike
    picks one; the plan updates with the choice + rationale.
D2. Deterministic tests: scripted interleavings via
    promise/latch rendezvous (no sleeps); stress tests use
    fixed iteration counts and generous timeouts, Linux CI
    only where timing-sensitive.
D3. `deref` timeout clock: wall-clock millis; flake budget is
    one retry-free assertion shape (`(deref p 50 :t)` on an
    undelivered promise is deterministic).

## TDD order

1. Oracle probes (delay-throw caching, future `deref`-throw
   shape, agent error-mode/restart semantics, nested-send
   order) → `s6-pins.md`.
2. `concurrency-contract.md` (IRef/IBlockingDeref/IPending
   semantics, error shapes, pool sizes).
3. S6A unit/model tests (CAS-loop accounting, watch-no-fire-
   on-same-value, validator veto shapes) then impl.
4. S6B determinism tests (latch-scripted) + stress (N-thread
   swap!/deliver counts) then impl.
5. S6C ordering/error-mode/await tests then impl.
6. Coverage flips (`out-of-scope|S6` → `implemented`) +
   matrix green; corpus entries alongside each fn.

## Acceptance

- Zero `out-of-scope | S6 concurrency` rows; matrix test green.
- Stress tests green and non-flaky (3 consecutive local runs
  + CI 3-OS).
- Per-slice gates (fmt/clippy/quality-gates) via PR CI.
- Spec + contracts update in-slice (`Ref T1174` + `AI-Tool:`
  trailers per commit).
