# S17 JDK/IO adapters (zero-oos slice 17)

Nine JDK/IO adapter vars over emulated opaque handles, no JVM:
`enumeration-seq` `iterator-seq` `stream-into!`
`stream-reduce!` `stream-seq!` `stream-transduce!`
`resultset-seq` `xml-seq` `file-seq`.

This slice adds its 9 rows to the single `### Previously
unlisted — accepted future work` alpha table in
`rust/core-coverage.md`, flipped straight to `implemented`
with test notes; the header counts are recounted to actuals.

Upstream sources below are alpha10 `clojure/core.clj`
(`file-seq` L5189, `xml-seq` L5199, `resultset-seq` L5934,
`iterator-seq` L5953, `enumeration-seq` L5963,
`stream-reduce!` L7010, `stream-seq!` L7019,
`stream-transduce!` L7026, `stream-into!` L7036) and
`clojure/core/protocols.clj` (`iterator-reduce!` L34);
docstrings and `:added` metadata are ported verbatim.

## Handles (constructor surface)

There is no JVM to hand the port an `Enumeration`, so each
handle is an opaque `ExtensionValue` (the `io.rs` `Writer`
pattern: `class_name` / `print` / `dyn_eq` / `dyn_hash` /
`as_any`, identity by per-evaluator sequence id) built only
by embedding Rust code through `pub` constructors in the new
`clj-eval` module `jdkio.rs`. No new vars construct handles
(the matrix forbids unlisted resolving vars); Clojure code
consumes handles it is given. Integration tests build
handles via these constructors and intern them into `user/`
vars (the `concurrency.rs` promise-test pattern).

- `JdkEnum` (`java.util.Enumeration`): `enumeration(items)`
  over `Vec<Value>` with a shared `pos` cursor.
- `JdkIter` (`java.util.Iterator`): `iterator(items)`, same
  shape.
- `JdkStream` (`java.util.stream.Stream`): `stream(items)`,
  same shape plus a `consumed` latch — every terminal op
  (`stream-seq!` at construction, the three consumers)
  throws the JDK `IllegalStateException` text when set, and
  sets it (probe `b-reuse`, `b-reuse2`).
- `JdkResultSet` (`java.sql.ResultSet`): `result_set(labels,
  rows)` with `Vec<String>` labels and `Vec<Vec<Value>>`
  rows plus a shared row cursor.
- `JdkFile` (`java.io.File`): `file(path)` over a `String`
  path; immutable, no cursor. `print_str` returns the path,
  so `(str f)` is the path like the JVM (probe `d-file-str`)
  while `pr` keeps the `#object[java.io.File 0xID "path"]`
  shape (probe `d-file-print`).

`class_name` reports the stable interface names above (plus
`java.io.File`, which is concrete); the JVM reports
concrete impl classes instead (probes `f-*` pin
`Collections$3`, `PersistentVector$2`,
`ReferencePipeline$Head`, the proxy class). That divergence
is inherent to having no JVM and is documented, not fixed.
`type`/`class` are out of scope (S16 owns class emulation);
handle classes are pinned through `pr-str` today, and S16
will consult `class_name` later. Handles are not seqable:
`(seq h)` fails the house "Don't know how to create ISeq"
shape. Equality/hash are handle identity.

The CCE target names the JVM interface each native
casts to: `java.util.Enumeration`, `java.util.Iterator`,
`java.util.stream.BaseStream`, `java.sql.ResultSet`,
`java.io.File`. A new `java.sql.*` arm in `cce_message`'s
`module_of` reports `module java.sql of loader 'platform'`
(probe `g-rs-argerr`); every other arm is unchanged.

## Semantics

- `enumeration-seq` — strict spine (the JVM's
  `EnumerationSeq` is not `IPending`: probe `a-enum-lazy`
  throws CCE). The native peeks `hasMoreElements`: empty
  answers nil (probe `a-enum-empty-nil`); otherwise the
  remaining items answer as one strict realized seq
  (`RealizedSeq`, uncounted — `seq?` true, `list?` false,
  `realized?` CCE, exactly like `EnumerationSeq`). Nil
  throws the oracle NPE text eagerly (probe `n-enum-nil`);
  non-nil non-handles throw CCE to `java.util.Enumeration`
  (probe `g-enum-argerr`).
- `iterator-seq` — lazy chunked-32 seq (the JVM answers
  `LazySeq`, probe `a-iter-type`). Empty-at-construction
  answers nil (probe `a-iter-empty-nil`); otherwise a
  `PendingSeq` over the shared handle cursor pulling up to
  32 items per force step (new `Thunk::JdkPull` variant, one
  arm in `step_thunk_rest`). Nil → the oracle NPE
  (probe `n-iter-nil`); else CCE to `java.util.Iterator`
  (probe `g-iter-argerr`).
- `stream-seq!` — terminal `iterator-seq` over the stream's
  peeked iterator: marks the stream consumed at
  construction, then behaves exactly like `iterator-seq`
  (probes `b-seq-*`, `b-reuse2`). Nil → NPE naming
  `BaseStream.iterator()` with `stream` (probe `n-seq-nil`);
  else CCE to `java.util.stream.BaseStream`
  (probe `g-stream-argerr`).
- `stream-reduce!` — ports `iterator-reduce!` verbatim: 2
  + 3 arities; empty 2-arity calls `(f)`; single-item
  2-arity never calls `f` (probe `n-reduce-nonfn-single`);
  `reduced` unwraps. The stream check precedes any `f`
  call (probe `o-reduce-stream-first`). Nil → NPE naming
  `s` (probe `n-reduce-nil`); else CCE to
  `java.util.stream.BaseStream` (probe `g-reduce-argerr`).
- `stream-transduce!` — 3 + 4 arities; 3-arity evaluates
  `(f)` for init BEFORE applying the xform (probe
  `o-transduce-init-first`); the transformed step reduces
  over pulled items with `reduced` unwrap, then the
  1-arity completion runs (probe `b-transduce-complete`).
  Same nil/CCE shapes as `stream-reduce!`
  (probe `n-transduce-nil`).
- `stream-into!` — 2 + 3 arities mirroring `into_xform`'s
  editable dispatch (persistent vector/map/set take the
  `transient` + `conj!` + `persistent!` path through
  `deref_core`, so transient-`to` throws exactly like
  `into`; everything else reduces with `conj`): 2-arity
  over `stream-reduce!`, 3-arity over `stream-transduce!`
  (xform applied before the stream is touched — probe
  `o-into-xf-first`). Same nil/CCE shapes
  (probe `n-into-nil`).
- `resultset-seq` — eager head, lazy tail (the JVM conses
  the first row: probe `c-lazy` throws CCE on `Cons`).
  Construction pulls metadata eagerly: labels lowercased
  to keywords, duplicate labels throw `java.lang.Exception`
  "ResultSet must have unique column labels" even before
  the first row is pulled (probe `c-dupkeys`); an empty
  row set answers nil (probe `c-empty-nil`); otherwise the
  first row (a plain `Map` in column order — structmaps
  print exactly as maps, probe `c-rows`) is consed onto a
  `PendingSeq` pulling one row per step. Nil → the oracle
  NPE (probe `n-rs-nil`); else CCE to `java.sql.ResultSet`
  (probe `g-rs-argerr`).
- `file-seq` — lazy preorder walk (native pull-cursor,
  one node per step; set-equality against `listFiles`,
  preorder parent-before-child — probes `d-names`,
  `d-preorder`, `d-count`). The root answers first even
  when it is a file or missing (probes `d-file-arg`,
  `d-missing`). Errors are LAZY like `tree-seq`: a bad
  root constructs an unforced seq and throws on first
  force — CCE to `java.io.File` (probes `n-file-unforced`,
  `n-file-vec-force`, `g-file-argerr`), NPE naming `f`
  for nil (probes `n-file-nil-unforced`, `n-file-nil`).
  Unreadable directories contribute no children (mirrors
  `(seq (. d (listFiles)))` over a null return — the JVM
  never surfaces an IO error from this walk, so the
  issue's `file_err` seam has no live call site; no
  `slurp`/`spit` behavior changes). Child order follows
  `read_dir`
  (OS-defined, like `listFiles`).
- `xml-seq` — verbatim `core.clj` port over `tree-seq`:
  `(tree-seq (complement string?) (comp seq :content)
  root)`. Strings are LEAF nodes, not skipped (oracle
  correction: probes `e-walk`, `e-count` — `[root b "hi"
  c "tail"]`); `(xml-seq nil)` is `(nil)` (probe `e-nil`).
  Laziness and chunking ride `tree-seq` untouched.

All nine natives carry the house arity guard; arity misses
throw the house arity text naming `clojure.core/<name>`
(probes `g-*-arity*`, all pinned).

## BDD scenarios

- Given `(vec (enumeration-seq h))` over a 3-item handle,
  the answer is `[1 2 3]`; over an empty handle, `[]`, and
  the call itself is nil.
- Given `(stream-reduce! + (stream [1 2 3]))`, the answer
  is `6`; reusing the stream throws `IllegalStateException`
  "stream has already been operated upon or closed".
- Given `(resultset-seq rs)` over labels `["NAME" "AGE"]`,
  rows answer `({:name "a", :age 1} ...)`; duplicate
  labels throw `Exception` "ResultSet must have unique
  column labels".
- Given `(file-seq (file tmp))` over a `root/{mid/deep,
  top}` tree, names answer the 4-member set in preorder;
  `(vec (file-seq "x"))` throws CCE to `java.io.File`
  while `(file-seq "x")` alone constructs.
- Given `(xml-seq {:tag :a :content [...]})`, the walk
  answers root-first depth order including string leaves.
- Given `(iterator-seq 1)`, the call throws CCE to
  `java.util.Iterator`; given `(iterator-seq nil)`, NPE
  `Cannot invoke "java.util.Iterator.hasNext()" because
  "iter" is null`.

## Tests

`rust/crates/clj-eval/tests/jdkio.rs`: one `*_rows` test
per var replaying its probe labels (values, nil/empty
edges, `realized?`/strictness, handle print shapes,
CCE/NPE/arity texts, stream single-use, file preorder +
set-equality over a `target/` fixture tree, transducer
consumption via `(map inc)`/`(completing + str)`), plus
Rust-side cursor-advance pins (chunked-32 pull after
`first`, unchunked single pull, stream latch). Matrix
`notes` name the tests.

## Non-goals

`type`/`class`/`instance?` on handles (S16 owns class
emulation); `slurp`/`spit` behavior changes (none —
`file-seq` only reuses `file_err`); `*. onto handles (no
interop surface); JDBC drivers (handles are
Rust-constructed); `clojure.xml` (only the `:content`
tree walk ships); new resolvable vars of any kind.

## Triage (oracle pins, probe `jdkio`)

- `enumeration-seq` is strict, not lazy: `EnumerationSeq`
  is not `IPending` (`a-enum-lazy` CCE), empty answers nil
  at construction (`a-enum-empty-nil`).
- `iterator-seq`/`stream-seq!` answer `LazySeq` (`a-iter-
  type`, `b-seq-type`) and nil when empty (`a-iter-empty-
  nil`, `b-seq-empty-nil`); chunking is 32-per-step inside
  (unforced `chunked-seq?` is false — `a-iter-chunked`).
- `stream-reduce!` 2-arity never calls `f` on one item
  (`n-reduce-nonfn-single`); empty calls `(f)`
  (`b-reduce-empty`); `reduced` unwraps (`b-reduce-
  reduced`); stream errors precede `f` calls
  (`o-reduce-stream-first`).
- 3-arity `stream-transduce!` evaluates `(f)` before
  applying the xform (`o-transduce-init-first`); the xform
  applies before the stream is touched (`o-into-xf-
  first`); completion always runs (`b-transduce-complete`
  is `"9"`).
- `resultset-seq` is Cons-headed (`c-lazy` CCE on `Cons`),
  rows print exactly as maps (`c-rows`), keys run in
  column order (`c-row-keys`), the dup-label `Exception`
  fires at construction (`c-dupkeys`).
- `file-seq` errors are lazy (`n-file-unforced`,
  `n-file-str-unforced`, `n-file-nil-unforced` all
  construct); preorder holds (`d-preorder`); missing roots
  answer themselves (`d-missing`).
- `xml-seq` keeps string leaves (`e-walk` is `[:a :b nil
  :c nil]`, `e-count` is 5); nil roots answer `(nil)`
  (`e-nil`).
- Handle classes are concrete on the JVM (`f-*`) but the
  port reports the stable interfaces (documented
  divergence); `java.sql` lives in `module java.sql of
  loader 'platform'` (`g-rs-argerr`); every nil-arg NPE
  message is pinned verbatim (`n-*-nil`).
