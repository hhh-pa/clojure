# S20 oddballs (zero-oos slice 20)

Five inventory rows, three surviving vars: the two guard rows
(`clone-thread-bindings`, `pprint`) resolve by removal on probe
evidence of absence, and `*repl*` `StackTraceElement->vec`
`Throwable->map` land as `implemented`.

Target universe rows come from PR #41's inventory (unlanded);
this slice adds its 3 rows to the wave-2 matrix's single alpha
table (`### Previously unlisted — accepted future work`), ASCII
sorted by var cell, flipped straight to `implemented` with test
notes. The #41 land unions them later.

## Semantics

- Guard verdicts (probes `guards.out` `g-clone`,
  `g-pprint-public`, `g-pprint-var`): `find-var` returns nil
  for `clojure.core/clone-thread-bindings`, and `pprint` is
  neither in `clojure.core` publics nor interned there. Both
  rows are deleted as inventory repair plus the header
  recount — the only two rows in the program allowed to
  resolve by removal (issue #55 guard rule).
- `*repl*` — dynamic var, root false, `:dynamic` meta true
  (probes `r-root`, `r-dynamic`, `r-bound`). `Session::new`
  resolves it and pushes a session binding of true that lives
  as long as the session; a non-booted evaluator (no interned
  var) skips it like the existing four handles.
- `Throwable->map` — native over the port `Throwable` chain.
  The oracle (alpha10) rewrote this fn versus the 1.11 shape
  the issue text sketches; every rule below is probe-pinned:
  - Top map: `:via` and `:trace` always; `:cause` present iff
    the root message is non-nil (probes `g-keys-*`,
    `l-keys-n`); `:data` present iff the ROOT link is an
    `ExceptionInfo`, valued at the root's ex-data (probes
    `g-data-*`, `j-top-data`, `k-top-keys`).
  - `:cause` is the root (deepest) message (probes `b-cause`,
    `j-cause`, `k-cause`).
  - `:via` walks the cause chain outermost-first, one entry
    per link: `:type` (symbol of the full JVM class name) and
    `:at` always; `:message` present iff non-nil (probes
    `c-via0-keys`, `l-via0-keys-n`); `:data` present iff the
    link is an `ExceptionInfo` (probes `j/k-via-data-keys`).
  - `:trace` is the ROOT cause's stack, element-wise (probe
    `l-trace-is-root-stack`), so `(first (:trace m))` is the
    root link's `:at` (probes `h-*`, `j/k-trace0-which`).
  - `ExceptionInfo` data normalizes nil to `{}`: the
    normalization lives in alpha10 `ex-info` itself (probe
    `l-raw-data` reads `{}` straight off `.getData`), while
    non-`ExceptionInfo` ex-data stays nil (probe
    `l-exdata-plain`). The port's `ex-info`/`ex-data` still
    store/answer nil (owned by an earlier slice — converging
    them is out of this fence), so the S20 fns apply the
    nil→`{}` rule at the `:data` projection: `:data` values
    are always maps, matching the oracle exactly. (Visible
    seam: port `(ex-data (ex-info "m" nil))` answers nil while
    `(:data (Throwable->map (ex-info "m" nil)))` answers `{}`.
    Both are pinned; the former converges when `ex-info`
    does.)
- `StackTraceElement->vec` — native. The port has no
  `StackTraceElement` values (no JVM stack) and no span
  values, so the port widens the domain to throwables: it
  answers its argument's emulated `:at` frame (same helper as
  `Throwable->map`). The widening is the documented,
  deterministic emulation — no new divergent rows.
- Emulated frame for one chain link (N3 stable-form: no
  method names exist in port diagnostics, so volatile slots
  take stable values): `[type-sym method-sym file line]`
  where `type-sym` is the link class's JVM name as a symbol,
  `method-sym` is the stable symbol `unknown`, `file` is nil
  (a JVM-legal nullable slot), and `line` is the link
  ex-data's `:clojure.error/line` when it is an `Int`
  (reader diagnostics ride there — the diagnostic span
  chain), else `-1` (the JVM unknown-line convention).
  Frame slot classes stay JVM-shaped: symbol, symbol,
  string-or-nil, int (probes `a-via0-at`, `e-ste`).
- `:trace` emulation: the per-link `:at` frames root-first,
  so the oracle's `(first (:trace m))` = root's `:at` rule
  holds by construction.
- Errors mirror the JVM hints: both fns are 1-arity (house
  arity text, probes `f-map-arity*`, `f-ste-arity0`);
  non-throwable input fails the cast (`java.lang.Throwable`
  / `java.lang.StackTraceElement` targets through the house
  `cce_message`, probes `f/g-*-cce`); nil is the verbatim NPE
  (probes `g-map-nil`, `g-ste-nil`), quoted exactly like the
  other house NPE texts.

## BDD scenarios

- Given the alpha10 oracle, `(find-var
  'clojure.core/clone-thread-bindings)` is nil and `pprint`
  is absent from `clojure.core`, so both matrix rows are
  removed and the header counts the smaller universe.
- Given a booted core outside any session, `*repl*` reads
  false; given a session transcript evaluating `*repl*`, it
  reads true.
- Given `(Throwable->map (ex-info "o" {:a 1} (ex-info "i"
  {:b 2})))`, the map carries `:cause "i"`, top `:data {:b
  2}`, a 2-entry `:via` with per-link `:data`, and a `:trace`
  whose first frame is the root link's `:at`.
- Given a reader-failure throwable (ex-data
  `:clojure.error/line`), the emulated frame carries that
  line; given a plain `(ex-info "m" {})`, the frame is
  `[clojure.lang.ExceptionInfo unknown nil -1]`.

## Tests

`tests/oddballs.rs`: one test per probe group replaying its
labels verbatim (`guard_*` as negative resolve pins after
removal, `repl_*`, `throwable_map_*`, `ste_vec_*`), plus the
documented nil-data seam pin and shared arity-miss tests. The
session-true leg of `*repl*` lives in
`clj-repl/tests/session.rs` (`s20_repl_*`: transcripts, not
units — a `Session` cannot be built from `clj-eval` tests
without a dependency cycle). Matrix `notes` name the tests.
