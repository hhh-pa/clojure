# Fence 4b (macroexpand audit) — oracle probes

Slice: `fence-4b-macroexpand`. Oracle: stock Clojure **1.13.0-alpha9**
(jar from Maven Central; run lines below) on Oracle JDK 27+35
(aarch64); the monitor matrix is additionally reproduced on
Temurin OpenJDK 21.0.13 LTS. Committed scripts + outputs live in
`probes/` next to this file — every checkable claim here traces
to a row there or to a vendored-source line.

Run (from `probes/`):

```sh
CP=<1.13.0-alpha9 jars: clojure, spec.alpha, core.specs.alpha>
java -cp "$CP" clojure.main or-semantics.clj   # -> or-semantics.out
java -cp "$CP" clojure.main kept-fixes.clj     # -> kept-fixes.out
```

Vendored-source oracle: the corpus input
`rust/crates/clj-reader/tests/corpus/inputs/src/clj/clojure/core.clj`
(bit-identical to the fork HEAD per the reviewer).

## Reverted: F1/F2 contradicted the oracle (review B1/B2)

The first 4b push claimed `seq?` emission and `:or` inlining from a
flawed differential; the oracle refutes both, so both reverted whole
(no iteration):

- `instance-emission`: upstream emits
  `(clojure.core/instance? clojure.lang.ISeq gmap)` (vendored
  `core.clj:4563`), not `seq?`.
- `or-hoist`: upstream hoists each default into a `default__` gensym
  (vendored `core.clj:4544`, `4558-4560`); getters and `:select`/`:all`
  outlets reference the gensym (vendored `:4675-4686`).
- `or-multiple`: local+key defaults raise `Multiple :or defaults`
  (vendored `:4587-4590`).
- `or-keyside`: `:or {:x 7}` yields `7`.
- `or-double-eval`: `[1 {:a 1}]` — the default evaluates once; the
  reverted inlining evaluated it twice (`[2 {:a 2}]`).
- `or-appear-only`: stray `:or` keys under `:select` raise
  `keys #{:y} appear only in :or` (vendored `:4676-4679`).

## Layer split: `let`-spec vs `destructure`-fn (pre-existing gap)

`let-or-nil` / `let-or-empty-vec`: JVM `let` spec-rejects a
non-map `:or` (`did not conform to spec`). The port has no `let`-spec
and accepts these through the lenient `destructure`-fn layer —
`fn-or-nil`, `fn-or-empty-vec` (plain expansion), `fn-or-5` (ISeq
error), `fn-or-vec1` (Map$Entry CCE — same class pair; the JVM's
message carries a module clause the port's `Cannot cast` shape
omits, per the port's CCE convention) — which the port otherwise
matches (restored `non_map_or`, restored `:or []` eval pin). The
`let`-spec gap predates this slice and is out of scope.

## Kept fixes (reviewer-verified genuine)

- F3 `letfn-list/map/set-fnspecs`: fnspecs seq (lists, maps, sets —
  not just vectors).
- F5 `wrd-map-odd-drop` / `wrd-map-paired`: map bindings seq to
  entries — a lone entry odd-drops, paired entries fail the `var`
  `Symbol` cast as `MapEntry` (CCE class pair verbatim; the port's
  CCE message shape is its own convention).
- F7 `letfn*-nonfn-init`: a non-`fn` init is the CCE to
  `Compiler$ObjExpr` (the JVM names the compiler-internal
  `NumberExpr`; the port names the source form's class — no port
  counterpart exists). The `bound-fn` template change is pin-only
  (byte-identical expansion); the pre/post pin stays.
- F8b-core `locking-nil`: the NPE names the expansion's lockee
  gensym (JVM leaks `locklocal__5902__auto__170`-shaped names; the
  port pins its own counter). Unmatched `monitor-exit` is
  `IllegalMonitorStateException` with an empty message
  (`monitor-exit-unheld` shows a nil message).

## B3: nil `monitor-exit` is always NPE (JVMS null-first)

The port implements the JVMS rule: a nil lockee on the exit path is
the form-naming NPE whether or not monitors are held (JVMS SE 21
orders the MONITOREXIT null check before the ownership / IMS rule;
`MonitorExitExpr.emit` is a bare MONITOREXIT, so there is no
compiler-level null check to model). Unmatched non-nil exits stay
IMS with an empty message.

Observed-variance record (preserved, not followed): Oracle 27+35 /
Temurin 21.0.13 produce a held→IMS / empty→NPE split instead, at
both the Clojure level (`kept-fixes.out` rows below, byte-identical
under `-Xint`, re-verified 2026-10-30 on stock 1.13.0-alpha9) and
the pure-bytecode level (`probes/monitor-matrix-jvms.txt`,
unanimous across both vendors × default / `-Xint`; full repro —
generator, driver, javap listings, run script — in
`probes/jvm-matrix/`, re-run 2026-10-30 with identical output).
The mechanism is unexplained (the template-interpreter source
shows a leading `null_check`, which predicts NPE-always), so the
port follows the spec text and keeps the repro for re-testing.

## Pinned as-is

- `with-redefs` odd-arity truncation and `letfn` garbage shapes are
  JVM-verbatim pass-through (reviewer-confirmed).
- `:keys!` / `:select` / `:excess` / `:missing` / `:all` / `:defaults`
  are 1.13 map-spec directives — the port implements the `:select` /
  `:all` outlets and pins the rest, unchanged by this slice.
- The `fn`-nested analyzer path names the fixed lock local (`"lock"`)
  instead of the expansion gensym (reviewer A6: pre-existing, honestly
  documented in `locking_rows`).
- `with-redefs` string bindings (reviewer A4, closed by #106):
  strings seq to chars — upstream `RT.seqFrom` takes the
  `CharSequence` arm (`StringSeq`), the macro feeds each char to
  `` `(var …) `` (vendored `core.clj:7925`), and
  `TheVarExpr.Parser.parse` casts straight to `Symbol`
  (`Compiler.java`), so the first char is the CCE
  `Character`→`Symbol`. The analyzer now seqs strings the same
  way (the eval path already did); a lone char odd-drops and
  `""` is empty, like the macro's `zipmap`.
- `letfn*` CCE-vs-body-error precedence (reviewer A3, closed by
  #106): upstream `LetFnExpr.Parser.parse` analyzes inits then
  the body with no cast, and only casts each init to `ObjExpr`
  in `emit` (`Compiler.java`) — so a body analysis error beats a
  non-`fn` init CCE, while an init analysis error still beats a
  body one. The port now analyzes inits, analyzes the body, then
  casts, in that order.
- Nested monitor forms (reviewer A5, pinned by #106): the input
  is a monitor form as a lockee. `(monitor-exit (monitor-enter
  x))` is the exit NPE here (call-form message); the observed
  JVMs throw IMS — the inner enter yields nil (upstream
  `MonitorEnterExpr.emit` ends in `NIL_EXPR`), so the outer exit
  is nil-with-held, exactly the unexplained B3 split condition
  (same mystery, not a new mechanism; the port holds the JVMS
  rule). The sibling `(monitor-enter (monitor-exit x))` agrees
  (enter NPE, empty stack). Both shapes are pinned in
  `monitor_nested_forms`.
- Exit matching uses 3b's keys: content-hashed kinds (symbols,
  Box clones) match across distinct objects, minted-id kinds
  (vectors, cells) behave identity-like. Full JVM identity is
  unimplementable for cloning Box kinds, and 3b's landed Box-clone
  exit test requires key matching (4b-rebase fallout).
- Unbalanced `monitor-enter` at eval end needs per-object migration
  onto the 3b monitor infra; deferred to the 4b rebase (needs 3b).
