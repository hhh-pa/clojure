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
