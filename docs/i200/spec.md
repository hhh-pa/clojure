# I200: re-entrant `delay` force throws the JVM's `StackOverflowError` (spec)

Parent: [#200](https://github.com/hhh-pa/clojure/issues/200). Landed via
PR [#224](https://github.com/hhh-pa/clojure/pull/224)
(`fix-batch-7-issues`).

## 1. Behavior

A delay body that forces its own delay (same-thread re-entry) throws
`java.lang.StackOverflowError` with an empty message — the JVM shape,
raised directly instead of recursed into, since real Rust stack recursion
would abort the process. The throw is cached like any thrown body: the
delay reads realized, and a re-force rethrows the same failure without
re-running. It is an `Error`, so `Exception` does not catch it but
`StackOverflowError`/`Throwable` do, and the message reads nil.

Fellow-thread forces still block on the cell condvar until the owner
publishes, then share the outcome; only same-thread re-entry throws.

## 2. Supersedes (frozen `docs/t1174` statements)

`docs/t1174` is frozen, so its older text stands as written; this spec
supersedes the following statements:

- `docs/t1174/concurrency-contract.md`: re-entrant force is no longer the
  `IllegalStateException | Delay is already being forced` pinned
  divergence — it is the JVM's empty-message `StackOverflowError`.
- `docs/t1174/s6-corpus/MANIFEST.md` (delay self-force entry) and the
  `docs/t1174/s6-corpus/02-delay.clj` header: the port now matches the
  JVM outcome (empty-message `StackOverflowError`), pinned by the Rust
  tests below instead of the corpus (the JVM's HotSpot stderr warning is
  still too fragile to pin, so the corpus entry stays dropped).

## 3. Implementation and tests

- `take_delay_thunk` (`rust/crates/clj-eval/src/concurrency.rs`): the
  `Forcing(owner)` same-thread arm answers
  `ThrowClass::StackOverflowError` with an empty message.
- `delay_reentrant_force_throws_stack_overflow`
  (`rust/crates/clj-eval/tests/concurrency.rs`): exact-class catch,
  nil message, `Exception`-does-not-catch / `Throwable`-catches,
  caching + evaluator-healthy-afterwards. `delay_throw_cached` in the
  same file pins the `uncaught` shape.
