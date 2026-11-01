# Gap-123 slice 1 oracle probes (JVM Clojure 1.13.0-alpha9, core.async 1.8.741, JDK 27)

Oracle for issue #123, slice 1 (channels + blocking ops only:
`chan` with fixed/dropping/sliding buffers, `>!!`/`<!!`,
`close!`/`closed?`, `alts!!`). Go blocks and parking ops (`go`,
`>!`, `<!`, `alt!`, `alts!`) are slice 2 and are not probed except
for their outside-`go` failure text.

Run with `java -Duser.language=en -Duser.country=US
-Duser.timezone=UTC -cp <jars> clojure.main docs/gap-123/<script>`
(one fresh JVM per script), where `<jars>` is
`/tmp/clj113/clojure-1.13.0-alpha9.jar`,
`/tmp/clj113/spec.alpha-0.6.249.jar`,
`/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar`,
`~/.m2/repository/org/clojure/core.async/1.8.741/core.async-1.8.741.jar`,
`tools.analyzer-1.2.0.jar`, `tools.analyzer.jvm-1.3.2.jar`,
`tools.reader-1.6.0.jar`, `core.memoize-1.1.266.jar`,
`core.cache-1.1.234.jar`, `data.priority-map-1.2.0.jar` (same
group dir), and `org/ow2/asm/asm/9.9.1/asm-9.9.1.jar`. Scripts +
captured outputs sit beside this file (`g123-chan.*`,
`g123-block.*`, `g123-alts.*`, `g123-bound.*`, `g123-edge.*`).

Port replay: `rust/crates/clj-eval/tests/async_slice1.rs`
(Clojure level) plus `channel.rs` unit tests (queue depths,
shared-waiter races) and the `ceil_to_i64` tower test. Port
implementation: `rust/crates/clj-value/src/channel.rs` (mailbox),
`rust/crates/clj-eval/src/async.rs` (the `clojure.core.async`
namespace, installed lazily by `require`).

## C — shapes and classes (`g123-chan.clj`)

- `c-unbuf-class` → `clojure.core.async.impl.channels.ManyToManyChannel`.
- `c-unbuf-pr` / `c-fixed-pr` / `c-nil-pr` / `c-buf-pr` → the
  stock `#object[… 0xHEX "…@HEX"]` rendering of that class.
- `c-drop-pr` → `#object[…DroppingBuffer …]`; `c-slip-pr`
  → `#object[…SlidingBuffer …]`.
- `c-dropbuf-class` / `c-slipbuf-class` / `c-fixbuf-class` →
  `…impl.buffers.DroppingBuffer` / `SlidingBuffer` / `FixedBuffer`.
- `c-eq-self` → `true`, `c-eq-other` → `false` (identity `=`),
  `c-hash-eq` → `true`.
- `f-put1` / `f-put2` → `true`, `f-take1` / `f-take2` →
  `:a` / `:b` (fixed FIFO).
- `d-puts` → `[true true true]`, `d-takes` → `[1 2]`
  (dropping keeps the first n).
- `s-puts` → `[true true true]`, `s-takes` → `[2 3]`
  (sliding keeps the last n).
- `x-close-ret` → `nil`, `x-closed?` → `true`, `x-put-closed` →
  `false` (puts after close fail), `x-drain` → `:x`
  (close-then-drain), `x-take-closed` → `nil`, `x-close-again` →
  `nil` (idempotent), `x-closed?-still` → `true`.
- `u-closed-take` → `nil`, `u-closed-put` → `false`.
- `e-nil-put` → `THROW
  java.lang.IllegalArgumentException | "Can't put nil on
  channel"`.
- `e-buf0` / `e-bufneg` / `e-chan0` → `THROW
  java.lang.AssertionError | "Assert failed: fixed buffers must
  have size > 0\n(pos? n)"`.
- `e-take-nonchan` → `THROW
  java.lang.IllegalArgumentException | "No implementation of
  method: :take! of protocol:
  #'clojure.core.async.impl.protocols/ReadPort found for class:
  java.lang.Long"` (same shape for `e-put-nonchan`
  (`:put!`/`WritePort`/`clojure.lang.Keyword`), `e-take-nil`
  (`… found for class: nil`), `e-close-nonchan` and
  `e-closed-nonchan` (`:close!`/`:closed?`/`Channel`/
  `java.lang.String`), `e-chan-str` (`:full?`/`Buffer`/
  `java.lang.String` — raised on first use, since construction
  stores the garbage).
- `e-bang-outside-go` / `e-rbang-outside-go` → `THROW
  java.lang.AssertionError | "Assert failed: <! used not in (go
  ...) block\nnil"` (resp. `>!`).

## B — blocking rendezvous (`g123-block.clj`)

- `r-put-ret` → `true`, `r-take-val` → `42` (taker first);
  `p-take-val` → `:v`, `p-put-ret` → `true` (putter first).
- `w-take-val` → `nil` (a parked take is released by close).
- `k-take-val` → `7`, `k-put-ret` → `true`, `k-take-empty` →
  `nil`: a parked put SURVIVES the close — the next taker gets
  the value and the put completes `true`.
- `b-drain` → `[:first :second nil]`, `b-put-ret` → `true`:
  same through a full fixed buffer (the drain refills from the
  parked put).
- `m-takes` → `(nil :only)`: the close hands each parked taker
  one buffered value, the rest nil.
- `n-puts` → eight `true`, `n-takes` → `(0 … 7)`;
  `u-takes` → `(0 1 2 3)` (many-to-many fan-in, buffered and
  unbuffered).

## A — alts!! (`g123-alts.clj`)

- `a-take` → `[:ready :c1]`; `a-put` → `[true :c2]` with
  `a-put-took` → `:v`.
- `a-closed` → `[nil :c1]` (a closed channel completes
  immediately); `a-put-closed` → `[false :c2]`.
- `a-default` → `[:dflt :default]`; `a-default-nil` → `[nil
  :default]` (`:default` is presence, not truth).
- `a-ready-beats-default` → `[:ready :c1]`.
- `a-priority` → `[:first :c1]`, `a-priority-rev` → `[:second
  :c2]` (`:priority` tries in order).
- `a-woken` → `[:late :c2]`; `a-put-woken-take` → `:pv`,
  `a-put-woken` → `[true :c1]` (parked alts released by the
  peer op).
- `e-alts-empty` → `THROW java.lang.AssertionError |
  "Assert failed: alts must have at least one channel
  operation\n(pos? (count ports))"`.
- `e-alts-nil-put` → `THROW java.lang.AssertionError |
  "Assert failed: can't put nil on channel\n(some? (port
  1))"`.
- `e-alts-nonchan` → the `:take!`/`ReadPort` protocol error for
  `java.lang.Long`; `e-alts-list-op` → the same for a list op
  (`clojure.lang.PersistentList` — only vectors are put-pairs).
- `e-alts-odd-opts` → `THROW
  java.lang.IllegalArgumentException | "No value supplied for
  key: :default"`.
- `e-altsbang-outside-go` → `THROW java.lang.AssertionError |
  "Assert failed: alts! used not in (go ...) block\nnil"`.
- `a-set` → `[:in-set true]` (ports vec first — sets work);
  `a-nil` → the empty-ports assert (`(vec nil)` is `[]`);
  `e-alts-str` → the `:take!`/`ReadPort` protocol error for
  `java.lang.Character`.

## Q — queue bounds (`g123-bound.clj`)

- `e-take-bound` → `THROW java.lang.AssertionError | "Assert
  failed: No more than 1024 pending takes are allowed on a
  single channel.\n(< (.size takes) impl/MAX-QUEUE-SIZE)"`
  (no windowed-buffer hint on the takes side).
- `e-put-bound` → `… 1024 pending puts … Consider using a
  windowed buffer.\n(< (.size puts) impl/MAX-QUEUE-SIZE)`.
- `q-takes-released` → `true`; `q-puts-drained` → `[0 1023
  1024]`, `q-puts-released` → `true` (the parked puts drain
  after the close, in order).

## E — buffer-size edges (`g123-edge.clj`)

- `e-drop-str` / `e-slide-str` → the JDK `String → Number`
  checkcast verbiage; `e-chan-str2` shows `(chan "x")`
  constructs fine and fails on first use (see `e-chan-str`).
- `e-chan-15` constructs fine and behaves as cap 2 (`(>= size
  1.5)`); `drop0-put` / `dropneg-put` → `[true]` (dropping
  zero/negative accept and drop everything); `slideneg` → `1`
  (sliding-negative never evicts).
- `slide0-put`: `(sliding-buffer 0)` then `>!!` crashes in
  `LinkedList.removeLast` — but the crash runs inside the
  channel's transducer `try/catch`, whose default ex-handler
  prints the trace (captured in `g123-edge.out`) and swallows
  it, so the put returns `true` and the value is lost. The
  port surfaces this as a `NoSuchElementException` instead
  (documented deviation — there is no JVM trace to print).
- `e-chan-vec` → the `:full?`/`Buffer` protocol error for
  `clojure.lang.PersistentVector`.

## Port deviations (all pinned in `async_slice1.rs`)

1. `closed?` is public in the port's `clojure.core.async` (the
   JVM keeps it in `impl.protocols`) — the slice requires it.
2. `(chan <non-number non-buffer>)` fails eagerly with the
   `:full?`/`Buffer` protocol error for that class (the JVM
   stores the garbage and fails on first use).
3. Sliding-zero puts throw `NoSuchElementException` (nil
   message) instead of printing a swallowed trace (see E).
4. (Lifted by slice 2 — see `slice-2.md`.) In slice 1,
   `(chan buf xform)` refused with
   `UnsupportedOperationException | unsupported: channels with
   transducers (gap-123 slice 2)`; `(chan nil xform)` keeps
   the JVM's `Assert failed: buffer must be supplied when
   transducer is\nbuf-or-n`. An ex-handler without a
   transducer is accepted and ignored (unobservable on the
   JVM too).

## Slice boundary (what remains of #123)

Slice 1: this directory's `g123-*.clj` / `g123-*.out` (below
`xform`) + `channel.rs` + `async.rs` + the `require` hook in
`load_one`. Slice 2 (`slice-2.md` + `g123-xform.*`) adds
channel transducers (`chan` xform/ex-handler arities).
Slice 3 (`slice-3.md` + `g123-go.*` + `g123-goalt.*`) adds
`go`/`go-loop` (thread-per-block), parking `<!`/`>!`/
`alt!`/`alts!`, and the blocking `alt!!`.
Still to do: promise-buffers, `put!`/`take!` callbacks +
the dispatch executor, `timeout`/`thread`, `offer!`/
`poll!`, `pipeline`/`merge`/`mix`/`pub-sub`, and
docstring/arglist metadata on the async vars.
