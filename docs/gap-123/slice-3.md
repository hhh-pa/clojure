# Gap-123 slice 3: `go` blocks + parking ops (spec)

Oracle for issue #123, slice 3 (`go`/`go-loop`, parking
`<!`/`>!`, `alt!`/`alts!` inside `go`, plus the blocking
`alt!!`). JVM Clojure 1.13.0-alpha9, core.async 1.8.741, JDK
27 — same classpath and fresh-JVM-per-script convention as
[probes.md](probes.md).

Run:

```sh
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp <jars> clojure.main docs/gap-123/g123-go.clj
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp <jars> clojure.main docs/gap-123/g123-goalt.clj
```

(`<jars>` as in probes.md.) Scripts + captured outputs sit
beside this file (`g123-go.*`, `g123-goalt.*` — stderr
interleaved, so the dispatch-thread traces are inline; the
dispatch thread number varies run to run and is normalized to
`async-io-N`, and the absolute script path in the two
`macroexpanding` positions to the `docs/gap-123/`-relative
form. Go-failure probes take the channel, sleep a second so
the async trace flushes, then print — otherwise the trace
races the value line.)

Port replay: `rust/crates/clj-eval/tests/async_slice3.rs`
(Clojure level) plus `async.rs` unit tests (the rewrite
boundaries, the `do-alt` shape, the go-report line).
Port implementation: `rust/crates/clj-eval/src/async.rs`
(the `go`/`go-loop`/`alt!`/`alt!!` native macros plus the
`go-call` spawner); `channel.rs` is untouched — parking
rides the slices 1–2 mailbox.

Design: upstream compiles `go` bodies to a state machine
(`impl.go/go-impl`: tools.analyzer + SSA in `go.clj`,
transitions `<!`/`>!`/`alts!` in `ioc_macros.clj`, result
delivery in `return-chan`, failure close in
`run-state-machine-wrapped`). The port has no analyzer pass
to hang that transform on, so it adapts the strategy to its
thread primitives instead: `go` is a macro that rewrites the
body's visible parking calls to the blocking ops and runs
the rewritten body on its own thread (one thread per go
block, like `future-call`'s fork; deviation D9):

```clojure
(go body...) =>
(let [c (chan 1)] ; c is a gensym; every emitted head is fully qualified
  (go-call c (fn [] <rewritten body...>))
  c)
```

The rewrite is resolve-aware: an operator-position
`<!`/`>!`/`alts!`/`alt!` that resolves to the
`clojure.core.async` var becomes `<!!`/`>!!`/`alts!!`/
`alt!!`, following the TARGET var (aliases, refers, and
`:rename`s count). `fn`/`fn*` bodies bound the rewrite
(oracle `e-fn-boundary`), as do `quote` (`g-quote`) and
`var` (`g-var`) forms; locals shadow (from `&env` plus
the `let`/`loop` bindings on the walk path — `g-shadow`,
`g-shadow-in`). Parking ops hidden behind fn-creating
*macros* (`for`, `letfn`, …) are rewritten anyway and
execute (deviation D11 — upstream asserts, `e-for-fn`).

`go-call` (port-internal, public like upstream's
`ioc-alts!`; deviation D12) forks the evaluator with the
calling thread's bindings conveyed (like `future-call`;
`g-binding`), runs the thunk on a DETACHED thread (the
dispatch pool's threads are daemon — parked gos never hold
the runtime alive past main; no shutdown latch either),
puts a non-nil result on the channel and closes it
(`return-chan`: nil skipped — `g-nil-body` — but `false`
delivered — `g-false-body`); on a throw it closes the
channel and prints the D6 one-liner
(`run-state-machine-wrapped` minus the trace).
`alt!`/`alt!!` port `do-alt` (unbalanced assert, keyword
opts filtering, gensym port/val bindings, the `cond`
dispatch with binding-form result exprs); `alt!` expands
over `alts!`, so outside `go` it fails with the `alts!`
assert (`t-outside`), while inside `go` the rewrite
renames it to `alt!!` before it expands. `go-loop` is
`(go (loop …))`.

## G — go basics

- `g-ret-class` → `…impl.channels.ManyToManyChannel`,
  `g-ch-pr` → its stock `#object` rendering.
- `g-val` → `42`; `g-closed-after` → `[nil true]` (the
  channel closes after delivering the value).
- `g-nil-body` → `[nil true]` (a nil result is skipped);
  `g-false-body` → `[false nil true]` (`false` delivers);
  `g-empty` → `[nil true]`; `g-multi` → `:c` (last value).
- `g-buf1` → `[:v true nil true]` (the go channel
  buffers one value, then closes).
- `g-thread-diff` → `true` (the body runs off-thread).
- `g-quote` → `(a/<! 1)` (quoted parking ops are data);
  `g-squote` → `(user/got :q)` (unquoted ones run).
- `g-var` → `#'clojure.core.async/<!`.
- `g-shadow` → `:shadowed`, `g-shadow-in` → `:in` (a
  local named `<!` is an ordinary call).
- `g-async` → `:v` (go returns before the body runs).
- `g-binding` → `:bound` (thread bindings are conveyed).

## P — parking takes/puts

- `p-take-then-put` → `:v`; `p-put-then-take` →
  `[:w true]` (`p-put-ret` → `true` — a parked put
  completes `true`).
- `p-seq-takes` → `3`, `p-let-take` → `42`, `p-if-take`
  → `:t`, `p-if-take-nil` → `:f` (parking in arg, let,
  and test positions).
- `p-multi-expr` → `:done`; `p-doseq` → `:drained`
  (`doseq` expands to a loop — no fn boundary).
- `p-nested-go` → `:inner` (blocking takes work inside
  go); `p-nested-go-chan` → `:in`.
- `p-try` → `:v`, `p-try-throw` → `:caught`.
- `p-fan-in` → `[(0 … 7) (true × 8)]`; `p-scale16` →
  `(0 … 15)` (sixteen parked gos all complete).

## C — close interplay

- `c-take-closed` → `nil`; `c-put-closed` → `false`.
- `c-park-released` → `nil` (a parked `<!` is released
  by close).
- `c-put-full-then-close` → `[:a :b true]` (a put
  parked behind a full buffer survives the close).
- `c-drain` → `[:x :y nil]`.

## B — buffer interplay

- `b-full-park` → `[:a :b true]` (a `>!` to a full
  fixed buffer parks until a take frees room).
- `b-drop` → `:keep`; `b-slide` → `:new`.

## X — transducer interplay

- `x-map` → `42`; `x-filter` → `3`; `x-mult` → `[1
  2]`.
- `x-reduced` → `[true 1 2 nil true]` (`(take 2)`
  aborts through a go put).
- `x-exh` → `:fb`; `x-dflt-exh` → `[:empty :default]`
  (the default handler's trace prints on the go thread).

## E — go errors (value `[nil true]` + a dispatch trace)

- `e-throw` → `ExceptionInfo: boom {:n 1}`.
- `e-fn-boundary` → `AssertionError: Assert failed: <!
  used not in (go ...) block` (fn bodies are opaque).
- `e-bang-nil` → `IllegalArgumentException: Can't put
  nil on channel` (same text as `>!!`).
- `e-take-nonchan` → the `:take!`/`ReadPort` protocol
  error for `java.lang.Long`.
- `e-arity` → `ArityException: Wrong number of args
  (2) passed to:
  …ioc-macros/take!` (the port reports the `<!!`
  arity instead — deviation D10).
- `e-for-fn` → the `<!` assert (`for` hides the call
  behind a lazy-seq fn — deviation D11).

## S — `alts!` inside go

- `s-take` → `[:ready true]`; `s-put` → `[true true
  :v]`; `s-closed` → `[nil true]`; `s-put-closed` →
  `[false true]`.
- `s-default` → `[:d :default]`; `s-default-nil` →
  `[nil :default]`; `s-default-expr` → `[:dflt
  :default]`.
- `s-ready-beats-default` → `[:ready true]`;
  `s-priority` → `[:first true]`; `s-priority-rev` →
  `[:second true]`.
- `s-woken` → `[:late true]`; `s-put-woken` → `[:pv
  true true]`.
- `s-set` → `[:in-set true]`; `s-unknown-opt` → `[:x
  true]`.
- `s-empty` → the empty-ports assert; `s-nil-put` →
  the nil-put assert; `s-nonchan` → the `:take!`
  protocol error; `s-odd-opts` →
  `IllegalArgumentException: contains? not supported on
  type: clojure.lang.Keyword` (the `ioc-alts!`
  destructuring — the port reports the `alts!!` text
  instead, deviation D10).

## T — `alt!` inside go

- `t-take` → `[:got :ready]`; `t-put` → `:wrote`;
  `t-multi` → `[:got :second true]`;
  `t-bind` → `[:v true]`.
- `t-default` → `42`; `t-priority` → `:first`;
  `t-default-take` → `:d`.
- `t-nested-take` → `:v2` (a `<!` in the result expr
  parks after the win); `t-putval-take` → `:x` with
  `t-putval-took` → `:dv` (a `<!` in the put value
  runs before the choice).
- `t-outside` → `THROW AssertionError | "Assert
  failed: alts! used not in (go ...) block\nnil"`.
- `t-unbalanced` → `THROW
  Compiler$CompilerException | "Unexpected error
  macroexpanding a/alt! at (…:94:33)."` (the port
  blames the same operator through its own wrap).
- `t-empty-default` → `[nil true]` + the empty-ports
  assert (zero op clauses still pass `[]` to `alts!`).

## U — `alt!!` (blocking, outside go)

- `u-take` → `[:got :ready]`; `u-put` → `[:wrote
  :v]`; `u-multi` → `[:got :second true]`;
  `u-bind` → `[:v true]`; `u-mixed` → `[:got :v
  true]`.
- `u-default` → `42`; `u-priority` → `:first`;
  `u-dup-default` → `2` (later pairs win).
- `u-badport` → the `:take!` protocol error for
  `java.lang.Long` (a non-vector clause is a take).
- `u-unbalanced` → the `CompilerException`/
  `macroexpanding a/alt!!` shape (same port wrap
  note as `t-unbalanced`).
- `u-empty-default` → `THROW AssertionError |
  "Assert failed: alts must have at least one channel
  operation\n(pos? (count ports))"` (even with
  `:default` — the call passes `[]`).

## L — `go-loop`

- `l-drain` → `[:a :b :c]` (take until nil, `recur`
  across the parking point); `l-count` → `15`;
  `l-single` → `[:got :only]` (`alt!` in the loop).

## Port deviations (D1–D8 as in slice-2.md, plus)

- D9: one thread per go block (no fixed dispatch
  pool): parking scales to the OS thread limit, and
  blocking ops inside `go` always work (upstream
  stalls them past pool saturation — same values
  until then).
- D10 (trace text only — values agree): go-internal
  failures print the D6 one-liner, and two arities
  report through the blocking layer: zero-arg `<!`
  reports `(0) … core.async/<!!` instead of `(2) …
  ioc-macros/take!`, and odd `alts!` opts report `No
  value supplied for key: :default` instead of the
  `ioc-alts!` destructuring failure.
- D11: parking ops hidden behind fn-creating *macros*
  (`for`, `map`, `letfn`, user macros) execute
  (blocking) instead of asserting (`e-for-fn`). Only
  literal `fn`/`fn*` bodies bound the rewrite — a
  macroexpanding rewrite would misblame inner macro
  errors at the `go` form, so the port rewrites the
  raw body deliberately.
- D12: `go-call` is a public port-internal var (the
  `go` expansion's spawn + completion protocol —
  upstream's expansions likewise call the public
  `ioc-alts!`).

## Slice boundary (what remains of #123)

Slice 3: this directory's `g123-go.*` + `g123-goalt.*`
+ `slice-3.md`, the `go`/`go-loop`/`alt!`/`alt!!`
macros and the `go-call` spawner in `async.rs`. Still
to do: `promise-buffer`, `put!`/`take!` callbacks +
the dispatch executor, `timeout`/`thread`, `offer!`/
`poll!`, `pipeline`/`merge`/`mix`/`pub-sub`, and
docstring/arglist metadata on the async vars.
