# S3 seq/coll walks (zero-oos slice 3)

Twelve pure seq/coll walks over existing machinery, no new value
kinds: `bounded-count` `flatten` `merge-deep` `merge-deep-with`
`partitionv` `partitionv-all` `reduce-kv` `run!` `splitv-at`
`tree-seq` `update-keys` `update-vals`.

Target universe rows come from PR #41's inventory (unlanded);
this slice adds its 12 rows to the wave matrix at mirrored
positions, flipped straight to `implemented` (header counts =
actual at push time). The #41 land unions them later.

Upstream sources below are alpha10 `clojure/core.clj`
(`tree-seq` L5189, `reduce-kv` L7161 + `kv-reduce` default
L7139, `merge-deep` L7276, `merge-deep-with` L7290, `flatten`
L7509, `splitv-at` L7647, `partitionv` L7653,
`partitionv-all` L7676, `bounded-count` L7801, `run!` L8127,
`update-vals` L8363, `update-keys` L8378); docstrings and
`:added` metadata are ported verbatim.

## Semantics

All twelve are `core.clj` fns (zero Rust). Adaptations from
upstream, each pinned by oracle probes
(`docs/zero-oos-s3-seqs/probes/seqwalks.{clj,out}`):

- `tree-seq` / `flatten` / `merge-deep` / `merge-deep-with` /
  `splitv-at` / `partitionv` / `bounded-count` / `run!` —
  verbatim ports. Every callee exists: `mapcat` `filter`
  `complement` `sequential?` `rest` `merge-with` `apply`
  `into` (3-arity xform) `(take n)` xf `drop` `nthrest`
  `counted?` `count` `seq`/`next` `reduce`.
- `reduce-kv` — upstream dispatches on `IKVReduce` else the
  `Object` slow path (reduce over entries with
  `Map$Entry.getKey/getValue`), with a `nil → init` arm.
  Port: `(nil? coll) → init`; `(vector? coll) →` index
  loop over `(count coll)` with `nth`, unwrapping
  `reduced?` via `unreduced` (matches `kvreduce` early
  exit); else `(reduce (fn [acc e] (f acc (key e) (val e)))
  init coll)`. Records ride the slow path (they seq as
  entries, same order as `kvreduce`).
- `update-vals` — upstream transients `m` itself when it is
  `IEditableCollection`, else `(with-meta {} (meta m))`.
  `instance?` is out of scope (N1), so the port tests
  `(and (or (map? m) (vector? m) (set? m)) (not (sorted?
  m)))`: sorted maps/sets are not editable on the JVM
  either (`trans-sorted` probe), and nil/lists fall to the
  `{}` route exactly like upstream. Vectors keep the
  vector route (`(update-vals [1 2] inc)` is `[2 3]`, as
  upstream). On records the transient round-trip drops
  record-ness on both runtimes (oracle `uv-record`).
- `update-keys` — verbatim port (`(transient {})` always,
  `with-meta` at the end): on vectors the ordinals become
  map keys (`(update-keys [:a :b] str)` is `{"0" :a, ...}`).
- `partitionv-all` 1-arity — upstream answers the
  `(partition-all n)` stateful transducer, but the port's
  `partition-all` native has no 1-arity (out of fence to
  add it in `seqs.rs`). The port inlines the equivalent
  stateful xf in `core.clj` (atom vector buffer in place
  of the `ArrayList`, `unreduced` on the completing
  flush — same shape, same reduced protocol).

Inherited edges (pre-existing port contracts, not this
slice's to fix — probes record the oracle text, tests pin
the port behavior):

- `reduce-kv` slow path over non-entries: the JVM's
  `^Map$Entry` hint throws `ClassCastException`, and the
  port's `key` throws the byte-identical text (module
  clauses included — oracle `rk-list`/`rk-str`/`rk-entry`,
  `uv-entry`/`uk-list`).
- `(bounded-count n nil)` answers `0` on both; the port
  takes the seq path (`counted? nil` is false here).

Arity misses throw `ArityException` with the right count
but name the volatile `user/evalN/fn--M` display: the
house `defn` emits anonymous fns, while the JVM names
`clojure.core/NAME` (oracle `*-arity*` rows pin the JVM
side). Tests pin class + count + the `/fn--` shape only,
so other slices' `core.clj` appends cannot break them.

## BDD scenarios

- Given each battery input (empty/nil/nested/lazy/sorted/
  vector/map/set/record), each walk replays the oracle's
  printed value verbatim.
- Given an infinite input (`(range)`, cyclic `tree-seq`),
  the lazy walks (`tree-seq` `flatten` `partitionv`
  `partitionv-all`) answer `take`-bounded prefixes and
  `bounded-count` terminates (early exit, no full
  realization).
- Given `partitionv-all` 1-arity, `(into [] (pva 2)
  [1 2 3])` is `[[1 2] [3]]` and the completing flush
  emits a partial tail (or nothing when empty).
- Given `reduced` from `f`, `reduce-kv` stops early and
  unwraps; `run!` still returns nil.
- Given a wrong arg count, each var throws the house
  arity text.
- Given `update-vals`/`update-keys`, result meta matches
  the oracle (`vals` preserves via transient, `keys` via
  `with-meta`).

## Tests

`tests/zero_oos_s3_seqs.rs`: one test per var replaying its
probe labels verbatim, plus a shared arity-miss test.
Matrix `notes` name the tests.

## Non-goals

`partition-all` 1-arity transducer (another slice's gap —
`partitionv-all` carries its own), `instance?`-based
dispatch, `int?`/`counted?` redefinition, record-type
preservation through transients (both runtimes drop it).
