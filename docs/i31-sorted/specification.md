# Issue #31: F-sorted slice — Specification

- Status: ready (first S-slice of the out-of-scope program).
- Parent: out-of-scope reduction. Issue: #31. Branch: `31-sorted`.
  Probes: `docs/i31-sorted/probes/sorted.{clj,out}` (19 probes,
  exit 0).

## Problem

Six `clojure.core` vars are `out-of-scope` with no slice and no
code: `sorted-map`, `sorted-set`, `sorted-map-by`,
`sorted-set-by`, `subseq`, `rsubseq`. Any program touching
ordered collections fails at resolve time, and the `sorted?` /
`reversible?` pins explicitly defer sorted-coll extension to
FT6 (this slice).

## Solution

Port sorted collections as `Value::Extension` values (the
`Eduction` precedent — no new `Value` variants, no exhaustive
match churn): `SortedMapValue` / `SortedSetValue` holding
`Arc<Vec>` entries in comparator order plus an optional custom
comparator (`None` = natural `jvm_compare`). Ordered-vec +
binary search keeps every op obviously correct; persistence is
`Arc` sharing like the existing sidecar `Vec`s. Hooks everywhere
`Eduction` has them (seq, print, `dyn_eq`/`dyn_hash`, count,
empty) plus the map/set ops (`assoc`/`dissoc`/`conj`/`disj`/
`get`/`contains`/`keys`/`vals`/`comparator`), the `sorted?` /
`reversible?` extensions, and `subseq`/`rsubseq` bound walks in
`clj-eval` following `core.clj` exactly (`mk-bound-fn` over
*entry keys*, `seqFrom` ascending/descending, head-drop rule).

## Part 1 — User/functional specification

### 1.1 Goals

1. The six rows become `implemented`, with notes naming the
   expansion points and pinning tests.
2. `sorted?` answers true on sorted values; `reversible?`
   answers true (JVM: sorted colls are `Reversible`).
3. Sortedness survives `assoc`/`dissoc`/`conj`/`disj`/`empty`
   (with comparator); content equality with plain maps/sets
   holds (`(= (sorted-map :a 1) {:a 1})`, matching hashes).

### 1.2 Non-goals (explicit)

- No transient sorted variants (JVM has none either).
- No metadata semantics beyond the `Extension` precedent
  (whatever `Eduction` does, sorted values do).
- No `sorted-map-by` key-order validation beyond what the
  comparator calls produce (errors surface from the calls).

### 1.3 Acceptance criteria

- [ ] Six rows flipped; `sorted?`/`reversible?` notes extended.
- [ ] All 19 probes pinned by tests, verbatim outputs.
- [ ] `coverage_matrix` green; 100% line+patch; full gates green.

### 1.4 BDD scenarios (user-observable)

- Given `(sorted-map :b 2 :a 1)`, when seqed/printed, then
  `([:a 1] [:b 2])` / `{:a 1, :b 2}` (probe `sorted-map`).
- Given `(sorted-set-by > 3 1 2)`, when seqed, then `(3 2 1)`
  (probe `sorted-set-by`).
- Given `(subseq m >= :b)` / `(rsubseq m <= :b)`, when evaluated,
  then the ascending/descending bound walks (probes `subseq-ge`,
  `rsubseq-le`).
- Given `(subseq m >= :z)` past the end, when evaluated, then
  `nil`; given `(subseq [] >= 1)`, then the `Sorted` cast error
  (probes `subseq-past-end`, `subseq-unsorted`).
- Given `(sorted? (sorted-map :a 1))`, when evaluated, then
  `true`; given `(sorted? {:a 1})`, then `false`.

## Part 2 — Technical specification

- Source of truth: `core.clj` sorted section at the alpha10 tag
  (`sorted-map`/`sorted-set`/`sorted-map-by`/`sorted-set-by`,
  `mk-bound-fn`, `subseq`, `rsubseq`, `comparator`), confirmed by
  the 19 probes.
- Port shape: `SortedMapValue`/`SortedSetValue` in
  `clj-value/src/extension.rs` (or a new `sorted.rs` if the
  trait impls exceed ~300 lines); natives in `coll.rs`
  (`SORTED_NATIVES` or alongside `COLL_NATIVES`); bound walks in
  `coll.rs` reusing `jvm_compare` for natural order and
  `call_checked` + `afunction_order` (the `sort-by` precedent)
  for custom comparators.
- Contracts to pin: duplicate (compare-0) keys collapse like
  repeated `assoc`/`conj`; default comparator is natural order
  with `nil`-first; custom comparator errors surface from the
  calls; `subseq` 3-arg (`>`/`>=` head-drop vs take-while) and
  5-arg (start-drop + end-take) exactly per `core.clj`;
  `rsubseq` mirrors descending; entry *keys* drive map bounds;
  `empty` keeps the comparator; `comparator` returns the fn or
  `nil` for natural order.
- Error shapes (probe-pinned): non-`Sorted` source → CCE to
  `clojure.lang.Sorted`; bad comparator application → the
  call's own error (probe `sorted-map-by-bad`).
- TDD plan: constructor/ordering tests first (red: unresolvable
  vars), then assoc/dissoc/conj/empty preservation, then
  equality/hash/predicates, then `subseq`/`rsubseq` bound tables
  (3-arg × 4 tests × map/set, 5-arg start+end), then error
  shapes. Failing tests first, then the implementation.
- CI impact: no workflow changes; `rust.yml` gates apply
  (fmt, clippy `-D warnings`, 100% line+patch floors,
  complexity ratchet, loom/TLA only if shared dispatch state
  appears — none expected: all state is per-value `Arc` data).

### Constraints

EPL-1.0 headers on new Rust files; MSRV 1.85 (no let-chains);
signed commits (m-hu) with `Ref #31`; green-before-push;
macOS work runs from `rust/`; oracle command + output committed
for every new behavior test.

### Oracle evidence

`docs/i31-sorted/probes/sorted.out` (19 probes, exit 0):
key-ordered construction, `>`/`>=`/`<`/`<=` walks both
directions, past-end `nil`, unsorted-source `Sorted` CCE, bad
comparator CCE, `assoc`/`dissoc`/`conj` preservation,
`sorted?` t/f, `empty`, `count`, `comparator` fn object.
