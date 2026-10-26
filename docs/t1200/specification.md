# T1200: Transducers and transducer arities — Specification

- Status: draft for spec review (slice D1 under T1192; first slice of
  the divergence/out-of-scope program).
- Parent: T1192 (divergence/out-of-scope reduction). Task: T1200.
  Branch: `T1200-transducers`.

## Part 1 — User/functional specification

### 1.1 Goals

1. The 8 divergent rows that are pure transducer fallout become
   `implemented`: 1-arg `map`/`filter`/`remove`/`keep`/`keep-indexed`
   and 0-arg `distinct`/`dedupe` return transducer fns instead of
   raising `ArityException` (today's D13 divergence), and `into`
   gains its 3-arg xform arity.
2. The 9 out-of-scope transducer-core rows become `implemented`:
   `transduce`, `sequence`, `eduction`, `completing`,
   `ensure-reduced`, `unreduced`, `preserving-reduced`, `halt-when`.
   (`reduced`/`reduced?` already exist in `core.rs` and stay as-is.)
3. Every new behavior is pinned by JVM-oracle probes (alpha10 jars)
   plus differential-corpus rows, so drift fails CI.

### 1.2 Non-goals (explicit)

- No `clojure.spec`, `core.async`, reader, or type-system work.
- No change to seq (non-xform) arities or laziness behavior.
- No shared mutable transducer state: transformers are
  per-reduction values, so no loom/TLA+ models are expected
  (stated upfront; revisited if the implementation proves it wrong).
- The 10 analyzer-builtin `macroexpand-1` divergences belong to
  slice D-macroexpand (later task), not this slice.

### 1.3 Acceptance criteria

- [ ] `map`/`filter`/`remove`/`keep`/`keep-indexed` 1-arg and
  `distinct`/`dedupe` 0-arg return 1-arg fns whose 0/1/2-step
  protocol matches the JVM (probe-pinned).
- [ ] `transduce` (2/3/4-arg), `sequence`, `eduction`,
  `completing` (1/2-arg), `ensure-reduced`, `unreduced`,
  `preserving-reduced`, `halt-when` (1/2-arg), and 3-arg `into`
  behave like the JVM, including early termination via `Reduced`
  and completion-arity calls.
- [ ] Oracle probes committed under `docs/t1200/probes/` with
  `.clj` + `.out` pairs recorded from the alpha10 jars.
- [ ] `core-coverage.md` updated: the 17 rows move to
  `implemented` (counts 336→353 implemented, 19→11 divergent,
  65→56 out of scope).
- [ ] 100% line + patch coverage held; full gate suite green.

## Part 2 — Technical specification

- Source of truth: `clojure/clojure` `core.clj` transducer section
  (`completing`, `transduce`, `eduction`, `sequence`, `halt-when`,
  per-xform `fn [rf]` builders) at the alpha10 tag, confirmed by
  running each probe against the alpha10 jars.
- Port shape: new transducer builders in `clj-eval` (likely
  `seqs.rs` for xform arities + a small `xform` module for the
  step-fn protocol), reusing `NativeFn` closures (as `seqs.rs`
  `native_fn` does), the existing `Reduced` value (`core.rs`),
  and `walk_seq`-style head walking for `transduce`. `sequence`
  stays lazy over existing thunks; `eduction` is seqable +
  reducible without count.
- Contracts to pin: `completing` 0/1/2-step delegation
  (`([x] x)` vs `([x] (cf x))`); `transduce` completion call and
  `unreduced` unwrap; `preserving-reduced` vs `ensure-reduced`
  wrapping; `halt-when` 1-arg (`haltWhen` sentinel + `reduced`)
  vs 2-arg (`retf` on halt); per-xform init/complete passthrough
  and step transformation (map-f, filter-if, distinct-seen,
  dedupe-prev, §2.1 scenarios).
- Error shapes (oracle TBD, probes `err-*`): bad xform callee,
  non-reducible input to `transduce`, `sequence`/`eduction` on
  non-seqables — each recorded `.out` verbatim.
- TDD plan: unit tests per builder (init returns `(rf)`,
  complete calls `(rf result)`, step transforms + `Reduced`
  propagation); integration tests counting side effects to prove
  early termination and laziness; `into`-xform equivalence with
  the same `transduce` call; error-shape tests against probe
  outputs. Failing tests first, then the builders.
- CI impact: no workflow changes; `rust.yml` gates apply
  (fmt, clippy `-D warnings`, 100% line+patch floors).

### Constraints

EPL-1.0 headers on new Rust files; MSRV 1.85 (no let-chains);
signed commits (m-hu) with `Ref T1200`; green-before-push;
macOS work runs from `rust/`; oracle command + output committed
for every new behavior test.

### 2.1 BDD scenarios (user-observable)

- Given `(def xf (comp (map inc) (filter odd?)))`, when
  `(transduce xf + (range 10))`, then the JVM sum (probe `sum`).
- Given an infinite source, when `(transduce (comp (map f)
  (halt-when pred)) conj [] src)`, then evaluation terminates
  having realized only the consumed prefix (probe `halt`).
- Given `(sequence xf coll)`, when only the head is realized,
  then later elements' mapping fns have not run (probe `lazy`).
