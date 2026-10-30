# Fence 4b (macroexpand audit) — JVM differential probes

Slice: `fence-4b-macroexpand`. Method: E1 differential — each of the nine
analyzer builtins (`let`, `loop`, `fn`, `binding`, `letfn`, `with-redefs`,
`with-bindings*`, `bound-fn`, `locking`) expanded on JVM Clojure and on the
port, shapes compared. Probe scripts: `/tmp/e1*.clj` (retained in session
scratch, not committed). Pins: `tests/fx4_macroexpand.rs`,
`tests/analyzer_contract.rs`.

## F1 — `seq?` (not `instance?`) in destructure seqable test

JVM `destructure` tests sequential binding forms with `seq?`. The port used
an `instance?` check, which misfires on seqable-non-sequential values.
Fix: `fx4_host.rs` emits `seq?`; hardcoded `instance?` pins replaced.

## F2 — `:or` defaults inlined at use site

The port hoisted `:or` defaults into a `default__` map, breaking
env-visibility (defaults could not see earlier bindings). JVM inlines each
default at its use site. Fix: `destructure_expand.rs` drops the hoisting and
inlines; map-spec validation is recursive at every map level.

## F3 — `letfn` fn-specs must be seqable

JVM `letfn` rejects non-seqable fn-specs during destructuring; the port
accepted garbage shapes silently. Fix: `destructure_expand.rs` validates
seqability of each fn-spec. (Genuinely garbage shapes — e.g. odd element
counts — remain JVM-verbatim pass-through; the JVM also accepts them and
fails later.)

## F5 — `with-redefs` MapEntry binding

JVM `with-redefs` destructures its binding vector entries as MapEntry
pairs; the port mishandled the entry shape. Fix in `builtin_expand.rs`.
Odd-arity binding vectors still truncate JVM-verbatim (pass-through, not a
port bug — the JVM does the same).

## F7 — `bound-fn` template + `letfn*` ObjExpr CCE

- `bound-fn`: expansion template corrected to the JVM shape.
- `letfn*`: inits now compile-then-cast to ObjExpr so non-fn inits raise
  the JVM-verbatim class-cast; the fn-headed fast path is preserved so
  named-fn inits still wrap in `LetFn` (a named-fn `FunExpr` must not lose
  its wrapper).

## F8b — `locking` nil lockee + unmatched `monitor-exit`

- `(locking nil)` names the gensym lock local in the NPE path
  (`locklocal__2__auto__` at fresh boot; pinned).
- Unmatched `monitor-exit` raises `IllegalMonitorStateException` with an
  empty message; nil lockee names the expanded form via the quoted second
  arg. New `ThrowClass::IllegalMonitorStateException` parented under
  `IllegalStateException` in both throw tables (`extension.rs`,
  `typeinternals.rs`, `ns.rs` name map).

## Pinned as-is (verified JVM-verbatim or deferred)

- `:keys!` / `:select` / `:excess` are 1.13 map-spec directives — left
  pinned, not implemented.
- Nested `locking`/`monitor-enter` forms stay divergent: the JVM's
  re-entrancy mechanism for the expanded form is unknown; no port
  implementation without a verified model.
- The `fn`-nested analyzer path names the fixed lock local (`"lock"`)
  instead of the expansion gensym — a port artifact (the JVM macroexpands
  first, so the path does not exist there). Pinned in `locking_rows`;
  unifying it means teaching the analyzer to expand first, out of scope.
- Unbalanced `monitor-enter` at eval end needs per-object migration onto
  the 3b monitor infra; deferred to the 4b rebase (needs 3b landed).
- `with-redefs` odd-arity truncation and `letfn` garbage shapes are
  JVM-verbatim pass-through (probed, pinned).
