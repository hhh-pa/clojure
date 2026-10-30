# Gap #125: `#=` eval reader honoring `*read-eval*` (forward spec)

Forward spec for #125. It supersedes nothing: the past spec
(`docs/t1174/reader-contract.md`, frozen per owner rule) §16 and
§24 item 2 keep their S2-era deferral wording; this doc specifies
the #125 behavior delta (§5 records the conformance status the
frozen catalog cannot carry).

## §1 Gate semantics

`EvalReader` checks the `*read-eval*` gate first (the live binding
on eval paths, `ReaderOpts.read_eval` standalone):

- `false` ⇒ the denial triple — `RuntimeException`,
  `EvalReader not allowed when *read-eval* is false.`, entry span
  (col right after `#=`) — **without consuming the following
  form**. Probe1 `#=(+ 1 2)` pins `(1,3)`; nested `[#=(+ 1 2)]`
  pins `(1,4)` by the same entry-span rule plus position
  tracking (`eval_reader_denies_before_reading`).
- `true` ⇒ read the operand, then apply the evaluator hook to
  it. Hookless contexts (the standalone reader) report `ReadEval`
  (`Eval reader #= requires an evaluator hook`); `clj-eval`
  installs the live hook, which resolves the head and applies it
  to the UNEVALUATED operand forms (S7 probes `c-*`/`d-*`),
  reporting failures with the post-operand span
  (`eval_reader_applies_through_the_hook`).

## §2 Traveling-throwable class names

`ReaderError::jvm_class()` reports `getSimpleName` for the
traveling throwable (`EvalFailed::Failed`), via the new
`ThrowClass::simple_name` (strips the package and any outer
`$` class). Eval-side `throw_class_for` (`ns.rs`) strips
packages itself, so the FQ→simple move is behavior-preserving
for its `io`/`ns`/`reader` consumers.

## §3 Harness alignment

The corpus drives `read_eval: false`, mirroring the oracle
driver's per-case `(binding [*read-eval* false])`
(`tests/corpus/driver.clj:143`) — this fixes a latent
harness/oracle mismatch. The `#=` case compares the denial
triple against the oracle (the `:diverge :n5-readeval` marker
and the stale `ours.edn` snapshot are removed, not edited).

## §4 Oracle derivation (no JVM execution)

- Denial triple: the committed oracle `expectations.edn` entry
  for `#=(+ 1 2)` plus the upstream `LispReader.EvalReader`
  text (denial fires before the operand is touched).
- Nested span: extrapolation of the pinned probe1 entry-span
  rule (no new corpus case — additions need JVM regen).
- Hook application: pre-existing S7 `c-*`/`d-*` probes.

## §5 §24-item-2 status

CONFORMANT as of #125. Pins: the corpus flip (§3), the
`eval_reader_denies_before_reading` triple + nested asserts
(§1), and the `jvm_class` simple-name asserts. Issue #125 asks
to flip the §24 catalog entry itself; the entry text stays
frozen per the past-spec rule — this section is the flip
record. (If the owner prefers the catalog edited, it is a
5-line change referencing this doc.)
