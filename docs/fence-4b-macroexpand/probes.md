# Fence 4b (macroexpand audit) — oracle probes

Slice: `fence-4b-macroexpand`. Oracle: stock Clojure **1.13.0-alpha9**
(jar from Maven Central; run lines below). Committed scripts +
outputs live in `probes/` next to this file — every checkable claim
here traces to a row there or to a vendored-source line.

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
error), `fn-or-vec1` (Map$Entry CCE) — which the port matches exactly
(restored `non_map_or`, restored `:or []` eval pin). The `let`-spec
gap predates this slice and is out of scope.

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

## B3 pushback: nil-held `monitor-exit` is IMS, not NPE

The monitor nil matrix (all rows also verified under `-Xint` and
`-Xcomp`):

- `monitor-exit-nil-held`, `-nillocal-held`, `-callnil-held` → IMS
  (nil message), for literal, local, and call nils alike — including
  the review's exact `(do (monitor-enter x) (monitor-exit nil))`.
- `monitor-exit-nil-empty`, `-nillocal-empty` → NPE naming the form
  (`"null"`, `"n"`, `IFn.invoke(Object)` templates verbatim).
- `monitor-enter-nil` → NPE (enter null-checks first).

HotSpot checks ownership before null on the exit path, so the
review's JVMS reading (null-check-first) does not hold for
MONITOREXIT; the port keeps the state-dependent rule.

## Pinned as-is

- `with-redefs` odd-arity truncation and `letfn` garbage shapes are
  JVM-verbatim pass-through (reviewer-confirmed).
- `:keys!` / `:select` / `:excess` / `:missing` / `:all` / `:defaults`
  are 1.13 map-spec directives — the port implements the `:select` /
  `:all` outlets and pins the rest, unchanged by this slice.
- The `fn`-nested analyzer path names the fixed lock local (`"lock"`)
  instead of the expansion gensym (reviewer A6: pre-existing, honestly
  documented in `locking_rows`).
- `with-redefs` string bindings still diverge (reviewer A4:
  pre-existing, narrowed by F5).
- `letfn*` CCE-vs-body-error precedence edge (reviewer A3): raising
  the init CCE before analyzing tails is the implemented order.
- Nested `locking`/`monitor-enter` re-entrancy (reviewer A5): the
  JVM's mechanism for the expanded form is unknown; no port model.
- Unbalanced `monitor-enter` at eval end needs per-object migration
  onto the 3b monitor infra; deferred to the 4b rebase (needs 3b).
