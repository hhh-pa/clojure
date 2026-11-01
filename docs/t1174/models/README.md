# S9D model-checking artifacts (T1174)

## `agent_queue.tla` — agent send/send-off queue discipline

TLA+ spec of one agent cell plus its submit/complete/restart/shutdown
protocol (`rust/crates/clj-value/src/agent.rs`,
`rust/crates/clj-eval/src/concurrency.rs`). It pins per-agent FIFO,
send/send-off non-blocking submission, failed-agent rejection,
restart-race serialization (the M3 under-lock re-check), and the
shutdown wedge — including the shutdown/reject paths the loom mirror
(`rust/crates/clj-value/tests/loom_agent.rs`) explicitly does not
model. The full conjunct-to-code map is the INVARIANT MAP comment in
the spec; deliberate abstractions are the G1–G7 list in its header.

Bounds (`agent_queue.cfg`): 2 senders × 2 direct sends (+1 nested
each), 2 racing restarters, both error modes and every pool
assignment (chosen in `Init`, fixed per behavior).

Toolchain pin: `tla2tools.jar` v1.7.4
(`https://github.com/tlaplus/tlaplus/releases/download/v1.7.4/tla2tools.jar`,
SHA-256 `936a262061c914694dfd669a543be24573c45d5aa0ff20a8b96b23d01e050e88`)
on Temurin 21. The SHA check in CI is the real pin; on bump, update
the URL + SHA in `.github/workflows/rust.yml` and re-record the
numbers below.

Last verified: 20,611,416 states generated, 7,110,400 distinct, depth
22, no error (TypeOK + 14 Safety conjuncts + Liveness + deadlock
check), in 2m17s wall on 10 workers (TLC 2.19, `-Xmx2g`). TLC's
fingerprint-collision estimate for that run: ~1e-6 (actual) — the
standard probabilistic caveat, comparable to any TLC run of this
size.

Local repro (from this directory, jar anywhere on disk):

```sh
java -Xmx2g -XX:+UseParallelGC -cp /tmp/tla2tools.jar tlc2.TLC \
  -workers auto -cleanup -config agent_queue.cfg agent_queue.tla
```

If the space outgrows the 15-minute CI budget, shrink `K`/`NESTED`
or split `:fail`/`:continue` into two configs rather than raising
the timeout.

## `channel_mailbox.tla` — core.async channel mailbox core (#188)

TLA+ spec of one channel plus its put/take/close protocol
(`rust/crates/clj-value/src/channel.rs`,
`rust/crates/clj-eval/src/async.rs` blocking drivers). It pins
fixed/dropping/sliding buffer accounting (including the
non-integral-sliding never-evicts quirk and the sliding-zero
`EmptyEvict`), rendezvous both directions, fixed-full park with
take-refill wakeup, per-mailbox FIFO across rendezvous/refill/close
(taken-values-are-subsequence-of-accepted), value conservation over
the puts queue / buffer / taken / dropped / evicted, and
close-release (close serves every queued taker but never resolves a
parked put). The full conjunct-to-code map is the INVARIANT MAP
comment in the spec; deliberate abstractions are the G1–G6 list in
its header. The loom mirror
(`rust/crates/clj-value/tests/loom_channel.rs`) owns the
park/wakeup schedule exploration this spec abstracts (G3).

Bounds (`channel_mailbox.cfg`): 3 putters × 2 puts, 3 takers × 2
takes (blocking scripts: the next op issues only after the previous
resolves), buffer CAP=1, queue bound MAX=2 (three threads per side
so the `qfull` arms stay reachable). The buffer kind and the
sliding `integral` flag are chosen in `Init`, fixed per behavior
(`integral` fixed `TRUE` off the sliding arm — it is unread there,
so no coverage is lost). `channel_mailbox_cap0.cfg` re-runs the
same spec at CAP=0 (fixed-0 queues everything, dropping-0 drops
everything, sliding-0-integral errors).

No liveness: channel ops block by design, so `Terminating` is an
unconditional stutter and the deadlock check is vacuous by design —
Safety carries all 17 conjuncts.

Last verified: CAP=1 leg 24,106,005 states generated, 8,611,085
distinct, depth 14, no error (TypeOK + 17 Safety conjuncts), in 44s
wall on 10 workers (TLC 2.19, `-Xmx2g`); CAP=0 leg 26,393,356
generated, 9,528,667 distinct, depth 14, no error, in 48s wall.
Fingerprint-collision estimates: ~1e-6 (actual) both legs — the
standard probabilistic caveat. (Local runs on Oracle JDK 26; CI
re-verifies on Temurin 21.)

Local repro (from this directory, jar anywhere on disk):

```sh
java -Xmx2g -XX:+UseParallelGC -cp /tmp/tla2tools.jar tlc2.TLC \
  -workers auto -cleanup -config channel_mailbox.cfg channel_mailbox.tla
java -Xmx2g -XX:+UseParallelGC -cp /tmp/tla-tools/tla2tools.jar tlc2.TLC \
  -workers auto -cleanup -config channel_mailbox_cap0.cfg channel_mailbox.tla
```

If the space outgrows the 15-minute CI budget, shrink `K`/`M` to 1
(the `qfull` arms stay reachable at 3×1) or split the buffer kinds
into per-kind configs rather than raising the timeout.

## `executor.tla` — single-thread executor cell (#188)

TLA+ spec of one executor pool plus its submit/shutdown/worker
protocol (`rust/crates/clj-value/src/executor.rs`, gap-96
drain-then-reject contract). It pins non-blocking submit, FIFO run
order, drain-after-shutdown, idempotent shutdown, and
post-shutdown rejection. The loom mirror
(`rust/crates/clj-value/tests/loom_executor.rs`) pins the
submit-vs-shutdown race schedule-by-schedule.

Bounds (`executor.cfg`): 2 submitters × 3 jobs, one racing
shutdown, one draining worker.

Last verified: 2,561 states generated, 1,341 distinct, depth 14, no
error (TypeOK + 8 Safety conjuncts + Liveness + deadlock check), in
under a second (TLC 2.19, `-Xmx2g`).

Local repro (from this directory, jar anywhere on disk):

```sh
java -Xmx2g -XX:+UseParallelGC -cp /tmp/tla-tools/tla2tools.jar tlc2.TLC \
  -workers auto -cleanup -config executor.cfg executor.tla
```
