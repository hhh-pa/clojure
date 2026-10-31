# Gap-123 slice 2: channel transducers (spec)

Oracle for issue #123, slice 2 (`chan` xform/ex-handler arities,
transducer stepping on the put path, completion on close and
closed takes). JVM Clojure 1.13.0-alpha9, core.async 1.8.741, JDK
27 — same classpath and fresh-JVM-per-script convention as
[probes.md](probes.md).

Run:

```sh
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp <jars> clojure.main docs/gap-123/g123-xform.clj
```

(`<jars>` as in probes.md.) Script + captured output sit beside
this file (`g123-xform.clj`, `g123-xform.out` — stderr interleaved,
so the default ex-handler traces are inline).

Port replay: `rust/crates/clj-eval/tests/async_slice2.rs`
(Clojure level) plus `channel.rs` xform unit tests (canned
steppers: the `None`-buffer shape, the evict→`Failed` route, and
throw propagation). Port implementation: `Xform`/`StepReq`/
`StepOut` and the `*_xform` methods in
`rust/crates/clj-value/src/channel.rs`, stepping + ex-handler
application in `rust/crates/clj-eval/src/async.rs`.

Design: the channel stores the composed step fn (`(xform conj)`,
built once at `chan`) behind a serialization gate. Each put
steps through it: filtered puts complete `true` untouched,
multi-emit puts park one registration holding every emit, and a
`reduced` step aborts (queued puts complete `true`, the channel
closes) once its emits buffer — or parks flagged and aborts at
refill. Step failures run the ex-handler path (nil drops, anything
else buffers as one untransformed replacement, a handler throw
escapes). The completion arity runs at `close!` (puts empty) and
on every closed-with-empty take.

## X — basic transforms

- `m-puts` → `[true true]`, `m-takes` → `[2 3]` (`(map inc)`).
- `f-puts` → `[true true true]`, `f-takes` → `[1 3]`
  (`(filter odd?)` — filtered puts still return `true`).
- `c-put` → `true`, `c-takes` → `[1 2]` (`(mapcat identity)` —
  one put emits twice).
- `k-puts` → `[true true true]`, `k-take` → `3` (`(comp (map
  inc) (filter odd?))` — only `2 → 3` survives).
- `d-put` → `true`, `d-takes` → `[1 [:empty :default]]`
  (dropping-1 + `mapcat`: the second emit drops).
- `s-puts` → `[true true]`, `s-take` → `3` (sliding-1 + `(map
  inc)`: `2` evicts).
- `u-puts` → `[true true true]`, `u-takes` → `[1 2]` (`dedupe`
  is stateful across puts).
- `p-take1` → `[1 2]`, `p-take2` → `[3]`, `p-take3` → `nil`
  (`partition-all`: the remainder flushes at `close!`).

The port's `partition-all` has no transducer arity (a core gap,
not async's), so the port flush tests use an equivalent
hand-rolled pair-buffer — same channel paths.

## R — `reduced` closes the channel

- `t-puts` → `[true true]`, `t-closed` → `true`, `t-takes` → `[1
  2 nil]`, `t-put-after` → `false` (`(take 2)` aborts on the
  second put).
- `t-park-takes` → `[1 2 nil]`, `t-park-f` → `true`,
  `t-park-closed` → `true`: the reduced put parks behind the
  full buffer; the take's refill consumes it and aborts.
- `t-g-takes` → `[1 2 nil]`, `t-g-f` / `t-g-g` → `true`: a put
  parked behind the reduced one completes `true` (its value is
  lost to the abort).
- `t-w-put` → `true`, `t-w-t1` → `1`, `t-w-t2` → `nil`: the
  abort serves queued takers from the buffer, then nil.
- `t-m-takes` → `[:a :b nil nil]`, `t-m-f` → `true`: `mapcat`
  short-circuits at `reduced`, so `:c` never emits (the port
  runs the same transducer code and agrees).

## E — ex-handler application

- `h-nil-put` → `true`, `h-nil-class` →
  `class clojure.lang.ExceptionInfo`, `h-nil-msg` → `"boom"`,
  `h-nil-data` → `{:n 1}`, `h-nil-take` → `[:empty :default]`
  (a nil result drops the value; data survives).
- `h-rep-put` → `true`, `h-rep-take` → `:fallback` (a
  non-nil result buffers as the replacement).
- `h-false-put` → `true`, `h-false-take` → `[false #object[…
  ManyToManyChannel …]]`: only nil drops — `false` buffers.
- `h-dflt-put` → `true`, `h-dflt-take` → `[:empty :default]`
  (the default handler prints a trace and drops; same for
  `h-false-exh-*` — a `false` ex-handler selects the default).
- `h-exh-throws` → `THROW clojure.lang.ExceptionInfo |
  "from-exh"` (a handler throw escapes the put).
- `h-cdone-nil` → `NO-THROW :done`, `h-cdone-throws` → `THROW
  clojure.lang.ExceptionInfo | "from-exh"` (a throwing
  completion routes through the handler too).
- `h-evict-put` → `true`, `h-evict-take` → `[:empty :default]`
  (the sliding-zero evict crash swallows through the default
  handler).
- `h-evict-rep-put` → `THROW
  java.util.NoSuchElementException | nil` (the replacement
  buffers outside the try, so the re-crash escapes).
- `h-rep1-put` → `true`, `h-rep1-class` →
  `class clojure.lang.ExceptionInfo`, `h-rep1-take` → `:r`.

## C — completion arity

- `c-parked` → `0`, `c-parked-t` → `1`, `c-pre` → `0`: puts
  and parked takes run no completion.
- `c-after-close` → `1`, `c-after-take1` → `2`,
  `c-after-take2` → `3`, `c-after-take3` → `4`: `close!` runs
  the completion once, and every closed-with-empty take runs
  it again.
- `c-f3` / `c-h5` → `:timeout` (zero-emit puts park behind a
  full buffer — the port completes them at once; deviation
  D4), `c-flush` → `[[1 2] [3 4] [5] nil]`, `c-g` → `true`:
  `close!` skips the completion while puts are queued, and a
  later closed take flushes `[5]`.
- `c-red-takes` → `[1 nil]`: a `reduced` completion is not
  an abort.

## B — errors and edges

- `e-chan-42` → `THROW java.lang.ClassCastException | "class
  java.lang.Long cannot be cast to class clojure.lang.IFn
  (java.lang.Long is in module java.base of loader
  'bootstrap'; clojure.lang.IFn is in unnamed module of
  loader 'app')"` (a non-fn transducer fails when applied).
- `e-nil-xf` → the slice-1 `buf-or-n` assert (unchanged).
- `e-degen-put` → `true`, `e-degen-take` → `[:empty
  :default]` (a step that never calls its `rf` buffers
  nothing, but the put completes).
- `b-full-drop` → `:pending` (a filtered put behind a full
  buffer parks — deviation D4).
- `b-closed-put` → `false`, `b-closed-steps` → `1` (puts on
  a closed channel neither step nor buffer).

## A — `alts!!` with transducers

- `a-full-dflt` → `[:d :default]` (a put to a full
  transducer channel parks, so `:default` wins).
- `a-closed` → `[nil #object[… ManyToManyChannel …]]`.
- `a-stale` → `[:d :default]`, `a-stale-takes` → `[1 [:empty
  :default]]` (a defaulted multi-emit put never resurrects).
- `a-w-take` → `1`, `a-w-f` → `[true #object[…]]`,
  `a-w-take2` → `42` (a parked put-alts is woken by a take).

## Q — stepping granularity

- `q-d-pre` → `1`, `q-d-t1` → `1`, `q-d-mid` → `1`,
  `q-d-t2` → `nil`, `q-d-end` → `2`: draining the buffer
  runs no completion.
- `q-m-takes` → `[1 2 3]`, `q-m-f` → `true`: a parked
  multi-emit put refills whole.
- `q-f0-parked` → `:pending`, `q-f0-take` → `1`, `q-f0-f` →
  `true`: a zero-emit put parks, then completes `true` at
  refill (deviation D4 completes it at arrival instead).
- `q-calls-parked` → `[0]`, `q-take1` → `0`,
  `q-calls-after` → `[0 1]`, `q-f` / `q-g` → `true`,
  `q-takes` → `[1 nil]`: the JVM steps queued puts at
  refill and never steps the aborted one (deviation D5
  steps at arrival: `[0 1 2]` / `[0 1 2]` — values agree).

## Port deviations (all pinned in `async_slice2.rs`)

- D1–D3 (from slice 1, unchanged): public `closed?`, eager
  `chan` validation, sliding-zero `NoSuchElementException`
  on transducer-less channels. (The slice-1 D4, the xform
  refusal, is lifted by this slice.)
- D4: puts step at arrival, so a filtered/zero-emit put
  behind a full buffer completes `true` at once instead of
  parking until a refill steps it (same value, only timing
  differs).
- D5: same cause — a put later lost to an abort still ran
  its transducer (the JVM never steps it). Values agree.
- D6: the default ex-handler prints one `Class | message`
  line (the JVM prints the full stack trace).
- D7 (source-derived, not probed): stepping runs without
  the channel lock, so a close racing a slow step wins
  (the put reports `false`); the JVM holds its mutex
  across the step, so the put would win there instead.

## Slice boundary (what remains of #123)

Slice 2: this directory's `g123-xform.*` + `slice-2.md`,
the `Xform` mailbox support in `channel.rs`, and stepping /
ex-handler application in `async.rs`. Still to do:
`promise-buffer`, `put!`/`take!` callbacks + the dispatch
executor, `timeout`/`thread`, `go`/`go-loop`
(state-machine compilation), parking `<!`/`>!`/`alt!`/
`alts!`, `offer!`/`poll!`, `pipeline`/`merge`/`mix`/
`pub-sub`, and docstring/arglist metadata on the async
vars.
