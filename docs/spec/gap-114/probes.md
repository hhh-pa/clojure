# Gap #114 oracle probes (JVM Clojure 1.12.3)

All run with `java -cp /tmp/clojure-1.12.3.jar:/tmp/spec.alpha.jar:/tmp/core.specs.jar clojure.main -e '…'`,
one JVM per probe (an unbalanced `monitor-enter` poisons its
continuation with an uncatchable message-less IMS — see Quirk B —
so probes never share a process with a leak).

Issue: `monitor_key` keyed `Symbol` by content, so a
distinct-but-equal symbol *released* another symbol's monitor
where the JVM throws. Fix: key symbols by minted id (fence 4a);
keywords stay content-keyed (globally interned — content key
*is* the identity key). Pinned by `locking_object_kinds` (still
green), `locking_symbol_exit_matches_by_identity`, and the
`monitor_key_covers_every_kind` symbol/keyword asserts in
`rust/crates/clj-eval/{tests/concurrency.rs,src/concurrency.rs}`.

Class note: the JVM throws
`java.lang.IllegalMonitorStateException` (message-less) for a
failing exit; the port raises `IllegalStateException` ("attempt
to exit a monitor not held by this thread"), pinned by the
fence-3b `fx4-monitor-exit` tests. That class mapping is
pre-existing and out of scope — these probes pin *which* exits
fail, not the class.

## P1 — exit matches by identity (the issue)

- `(do (monitor-enter 'g114-a) (monitor-exit 'g114-a) :survived)`
  → dies with `IllegalMonitorStateException` (distinct-but-equal
  symbols are distinct monitors).
- `(do (monitor-enter :g114-a) (monitor-exit :g114-b) :survived)`
  → dies with `IllegalMonitorStateException` (distinct keywords
  likewise).
- `(do (monitor-enter :g114-k) (monitor-exit :g114-k) :survived)`
  → `:survived` (interned keywords share identity across
  occurrences, so content keys match — keywords need no fix).
- `(let [s 'g114-sym] (monitor-enter s) (monitor-exit s) :ok)` →
  `:ok` (same binding exits cleanly).

## P2 — balanced `locking` over symbols/keywords

- `(locking 'g114-sym :ok)` → `:ok`.
- `(let [k :kw] (monitor-enter k) (monitor-exit k) :ok)` → `:ok`.
- `(do (monitor-enter :kw) (monitor-exit :kw) :ok)` → `:ok`.

## P3 — identity premises (`identical?`)

- `[(identical? 'g114-sym 'g114-sym) (let [s 'g114-sym] (identical? s s)) (identical? :g114-kw :g114-kw)]`
  → `[false true true]`.

## Quirk A — `monitor-exit` inside `try` always throws (do not pin this shape)

Every try-wrapped exit threw, even on a held identical monitor:

- `(let [s 't] (monitor-enter s) (try (monitor-exit s) :released (catch IllegalMonitorStateException e :threw)))`
  → `:threw`, and the monitor stays held.
- `(let [k :k] (monitor-enter k) (try (monitor-exit :k) :released (catch IllegalMonitorStateException e :threw)))`
  → `:threw` (contrast P1: the same exit bare releases).

Bare exits are always correct (P1/P2). Mechanism not chased;
consequence: JVM pins for exit matching must use try-free
shapes (die-vs-survive), never try/catch. The port's
`locking_symbol_exit_matches_by_identity` still uses try/catch
around the distinct exit — valid there, because the port's
`try` is plain control flow over its own stripe table (no
bytecode quirk), and the test must balance the enter in-`let`
(a leaked guard would poison a stripe for other tests).

## Quirk B — unbalanced enter poisons the continuation (observed, unexplained)

- `(do (monitor-enter 'x) :tail)` → uncatchable message-less
  `IllegalMonitorStateException` (a `try/catch` around the
  enter does not catch it either).
- `(do (let [s (Object.)] (monitor-enter s) :entered) (+ 1 1))`
  → same uncatchable IMS; the `(+ 1 1)` never evaluates.

Reproduced with symbol and plain-`Object` lockees, in `let`,
`do`, `fn`, single- and multi-form scripts. No mechanism
proposed; recorded so future probers isolate (balanced-only)
monitor shapes one JVM per probe.
