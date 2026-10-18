# S3 values contract (transients, seqs, ordering)

Normative behavior for `clj-value`'s S3 additions, each backed by a
JVM oracle probe (`probes/probe17.clj`–`probe19.clj`). S3 delivers
standalone types and functions; `Value`-variant integration (new
variants plus `eq`/`hash`/printer arms) lands with the S5 core fns
that need to pass these values around.

## §1 Transients (probe19)

- `TransientVector`/`TransientMap`/`TransientSet` mirror the JVM
  edit flag: **every** op (mutating, reads, `len`, a second
  `persistent`) raises `UseAfterPersistent` once frozen —
  `IllegalAccessError | Transient used after persistent! call`.
  Methods take `&mut self`, so single-threaded use is static.
- `pop` on empty: `IllegalStateException | Can't pop empty
  vector`. `assoc` past the end: `IndexOutOfBoundsException`
  with an **empty** message; `idx == len` appends.
- `conj` takes exactly one value (`conj!` has no variadic arity —
  `ArityException` on JVM; arity validation is S5's core fn).
- Map `assoc` on an existing key keeps the original key object
  and position; `dissoc` of a missing key is a no-op. Set `conj`
  of a present member keeps the original; `disj` of a missing
  member is a no-op.
- `persistent` freezes in insertion order with **no metadata**
  (assumption: the JVM transient carries none; S5 core-fn probes
  re-verify).
- Printing (S5 integration): the JVM renders the default
  `#object[TransientVector 0x...]` shape. Our `Value` rendering
  for transients is decided in S5 (readable shape + §-style
  divergence entry, following the S2 `TaggedLiteral` precedent).

## §2 Seqs (probe18, spec C4)

- Variants: eager (lists, maps as `[k v]` vectors in seq order,
  sets in member order), chunked-32 (vectors), chunk-cons
  (realized chunk + tail), single cons, memoized lazy
  (`OnceLock`: thunks run once even under contention).
- `first`/`next`/`more`: `next` yields `None` at the end,
  `more` the empty seq. All `seq` ctors nil-pun (`None` on
  empty input).
- `chunk_next` serves the current chunk plus the after-chunk
  seq (`None` unless chunked here); mid-chunk serves the
  remainder. Realization counts are oracle-pinned: take-2 over
  mapped range/vector = 32, over a mapped list = 2, take-35
  over a mapped range = 64, take-2 over
  filter-map-range = 32, take-2 over map-filter-range = 16
  (filter preserves chunking).
- The `tests/seq.rs` map/filter/take helpers mirror core.clj's
  chunked paths; S5's core fns reuse the same primitives.
- Map seqs yield plain 2-vectors, so `(type (first {:a 1}))`
  will read `Vector`, not JVM `MapEntry`, once core fns land
  (S5 decides whether to close that gap).

## §3 Equality and ordering (probe17, spec C2)

- `=` is same-category (`Numbers.equal` gates on category):
  `(= 1 1.0)` is **false** (spec C2 corrected in S3; S2's code
  was already right). `(== 1 1.0)` is the cross-category
  comparison (S4), comparing as doubles
  (`(== 9007199254740993 9007199254740992.0)` is true).
- `compare` ports `Util.compare`/`Numbers.compare`: numeric
  across `Int`/`Double` (`Int`/`Int` exact, anything touching
  `Double` as `f64`), within-type for chars/bools/strings/
  symbols/keywords (namespace, then name; unqualified first)
  and lexicographic vectors (element errors propagate).
- The numeric rule is the JVM lt-pair, **not** total order:
  NaN compares `Equal` to everything numeric (even NaN) and
  `-0.0` equals `0.0`.
- Lists (either side), maps, sets, regexes, tagged literals,
  reader conditionals, and all other cross-type pairs fail.
  **Deviation**: our message is the stable `Cannot compare
  <A> to <B>` (Clojure type names); the JVM throws a JDK
  `ClassCastException` whose text embeds module/loader
  verbiage that varies by JDK release.
- `sorted-map` collapses compare-equal keys (`{1, 1.0}` → one
  entry); sorted collections themselves land in S5.

## §4 Non-goals and follow-ups

- No `Integer` variant: probe17 proves the JVM reader yields
  `Long` for every in-range integer (even int-sized), so no
  split exists. `Integer` values can only arrive via interop
  (S5+); their same-category `=`/dup semantics are recorded
  here for that slice (`(= (Integer. 1) 1)` true,
  `{(Integer. 1) :a 1 :b}` dups).
- No `Value::Seq` / `Value::Transient*` variants in S3 (see
  top); S5 adds them with `eq`/`hash`/printer arms and the
  complexity-baseline regen that growth requires.
- `hasheq` values are internal: only equiv-consistency is
  required, never JVM-identical integers.
