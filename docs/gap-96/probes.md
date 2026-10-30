# Gap-96 oracle probes (JVM Clojure 1.13.0-alpha9, JDK 27)

Oracle for issue #96 (wire recorded agent executors through sends).
Run with `java -Duser.language=en -Duser.country=US -Duser.timezone=UTC
-cp /tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar
clojure.main docs/gap-96/<script>` (one fresh JVM per script — the
pool fields are process-global). Scripts + captured outputs sit
beside this file (`g96-route.*`, `g96-shutdown.*`,
`g96-nil-shutdown.*`).

Source facts (`core.clj`, confirmed by `javap` on `clojure.lang.Agent`):
the fields are `public static volatile ExecutorService pooledExecutor`
/ `soloExecutor`; the setters are `set!` over them (typed assignment);
`send`/`send-off` are `(apply send-via <field> …)` (read per send);
`future-call` is `(.submit soloExecutor …)` (read per call);
`shutdown-agents` is `Agent/shutdown` (shuts the current fields);
`await` barriers ride `(send agent count-down)`.

## R — routing (`g96-route.clj`)

- `r-send` / `r-off` → `[1 "g96-send"]` / `[1 "g96-off"]`: each
  install routes its own sends onto the supplied pool's threads.
- `r-future` → `"g96-fut"`: `future-call` runs on the installed
  solo pool.
- `r-persend` → `["g96-one" "g96-two"]`: the field is read per
  send — swapping between sends moves each action.
- `r-await` → `[nil 1]`: `await` completes with a custom send
  pool installed (barriers run there).
- `r-restored` → `true` (hygiene).

## N — nil + validation (`g96-route.clj`)

- `n-handler-send` / `n-handler-off` → `[:saw
  ["class java.lang.NullPointerException" "Cannot invoke
  \"java.util.concurrent.Executor.execute(java.lang.Runnable)\"
  because \"this.exec\" is null"] false 0 true]`: a nil field
  behaves EXACTLY like `send-via` nil (fence 3c P2) — the send is
  accepted, the handler sees the same NPE, follow-ups wedge
  (`await-for` → `false`, `@a` → `0`, `agent-error` → nil).
  Decision: installing nil maps to the nil-executor wedge (no
  restore-to-pool).
- `n-future` → `THROW java.lang.NullPointerException |
  "Cannot invoke
  \"java.util.concurrent.ExecutorService.submit(java.util.concurrent.Callable)\"
  because \"clojure.lang.Agent.soloExecutor\" is null"`:
  `future-call` over a nil solo field throws to the caller.
- `n-cce-str` / `n-cce-bool` (+ S10's keyword/long rows): a
  non-executor fails the `set!` checkcast to
  `java.util.concurrent.ExecutorService` with the standard JDK
  module verbiage; nothing installs.

## S — shutdown lifecycle (`g96-shutdown.clj`)

- `s-inflight` → `[nil true 41]`: shutdown returns at once with
  a custom action mid-flight; the action still finishes (drain).
- `s-post-send` / `s-post-off` → `[true :saw
  "class java.util.concurrent.RejectedExecutionException" false
  1]` (send; `2` for send-off): post-shutdown sends are
  accepted, notify the handler with a `RejectedExecutionException`,
  and never run.
- `s-post-future` → `THROW RejectedExecutionException` (message
  embeds pool identity hashes — port-fixed).
- `s-fresh-send` → `["g96-fresh" true 9]`, `s-fresh-future` →
  `"g96-freshfut"`: sides installed fresh AFTER shutdown are
  live again — shutdown shuts the CURRENT fields only (no latch).

## Z — nil shutdown (`g96-nil-shutdown.clj`)

- `z-nil-pooled-shutdown` / `z-nil-solo-shutdown` → the field
  NPEs: `Cannot invoke
  "java.util.concurrent.ExecutorService.shutdown()" because
  "clojure.lang.Agent.pooledExecutor" is null` (resp.
  `soloExecutor`).
- `z-both` → the SOLO NPE wins: shutdown runs the solo side
  first, then the send side, with no catch.
- `z-both-pools-live` → `[true true 1 1]`: the solo NPE lands
  before anything shuts.
- `z-pooled-npe-shut-solo` → `[true false 1 0]`: the pooled NPE
  lands AFTER the solo side shut (pooled side live).
- `z-nil-set-ret` → `true` (nil installs; the `set!` value is nil).

## Port decisions

- Slots are typed (unset / nil / custom); validation mirrors
  the `set!` checkcast (`ExecutorService` CCE, nil allowed).
- `send`/`send-off`/`await`-barriers resolve their field per
  send; `future-call` submits to the solo field per call; nil
  reuses the `send-via`-nil wedge + NPE texts.
- `shutdown-agents` shuts the current fields solo-first with the
  same partial effects (solo-NPE: nothing; pooled-NPE: solo side
  shut); fresh installs run again. The process-exit latch is the
  only infallible shutdown (nil sides skip) so exit always drains.
- Rejection texts stay port-fixed (pool identity hashes), as with
  the existing pool/future rejections.
