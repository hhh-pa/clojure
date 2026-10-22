# T1174 S5D core-library contract (natives + embedded composites)

Normative var, error-text, and value semantics for the `clojure.core`
port: Rust natives (`core.rs`, `coll.rs`, `seqs.rs`, `lazy.rs`,
`fmt.rs`, `io.rs`) plus embedded composites (`core.clj`) and the
analyzer builtins they target (`assert`, `time`, `with-out-str`,
`lazy-seq*`, `case*`).
Pinned against `s5d-pins.md` (7g9–7i6 probes, decompiles, FmtProbe),
`s5d-coll-pins.md` (coll probes c–c8), and the differential corpus
`s5d-corpus/01–19` (oracle: en_US, UTC).

Companion contracts: `namespaces-contract.md` (S5A),
`vars-contract.md` (S5B), `macros-contract.md` (S5C),
`evaluator-contract.md` (S4 wrappers, `IFn` table, destructure rules —
unchanged here). Machine-read var/status rows live in
`rust/core-coverage.md`; this doc pins the semantics behind them.

## D1. Scope

| Area | Vars | Impl | Tests | Status |
|---|---|---|---|---|
| Predicates + coercions | `nil?`…`record?`, `counted?`, `int`…`char` | `core.rs` (`CORE_NATIVES`) | corpus 01; `counted_pred_rows`; `arith.rs::coerce_rows` | implemented |
| Collections | `count`…`distinct?` (D3) | `coll.rs` (`COLL_NATIVES`) | `tests/coll.rs` (26 green); corpus 07/08/09 | implemented |
| Transients | `transient`, `persistent!`, `conj!`, `assoc!`, `dissoc!`, `pop!` | `coll.rs` | `coll.rs::transient_rows`; corpus 07 | implemented |
| Seqs + lazy | `seq`…`min-key` (D4) | `seqs.rs` (`SEQS_NATIVES`) + `lazy.rs` (`seq`/`first`/`rest`/`next`/`cons`) | corpus 02–06/10/16–19 | implemented |
| HOF | `comp`…`not-every?`, `every-pred`, `some-fn`, `max-key`, `min-key` | `seqs.rs` | corpus 02/07/15 spot rows | implemented (seq/fn arities only) |
| str/name/subs | `str`, `pr-str`, `print-str`, `name`, `namespace`, `subs` | `core.rs` | `tests/str.rs` (8 green); corpus 11/19 | implemented |
| `format` | `format` | `fmt.rs` (`native_format`) | `tests/format.rs` (15 green); corpus 12/13/14 | implemented |
| ex-info | `ex-info`, `ex-data`, `ex-message`, `ex-cause` | `core.rs` | S4 probe22 roundtrip path | implemented |
| Print + `*out*`/`*err*` | `print`, `println`, `pr`, `prn`, `newline`, `flush` | `io.rs` (`IO_NATIVES`) | corpus 11 | implemented |
| Slurp/spit/read | `slurp`, `spit`, `read-string` | `io.rs` | `tests/io.rs::io_smoke_rows` | implemented |
| `with-out-str` | `with-out-str` | analyzer builtin (`eval.rs`) | `control.rs::with_out_str_rows`; corpus 11 | implemented |
| `eval` | `eval` | `eval.rs`/`ns.rs` | `control.rs::eval_chains_and_catch` | implemented |
| `declare`, `var-set` | (S5A/S5B, not S5D) | `macros.rs`, `vars.rs` | sibling contracts | implemented (pointers only) |
| Control composites | `defn`, `and`/`or`, `when`, `cond`, `condp`, `case`, threaders, `dotimes`, `doseq`, `for`, `lazy-seq`, `comment`, `assert`, `time`, `doto`, + `defn-`/`defonce`/`if-not`/`when-not`/`if-let`/`when-let`/`if-some`/`when-some`/`lazy-cat` | `core.clj` + analyzer builtins | `tests/control.rs` (23 green); corpus 15 | implemented |

## D2. Predicates + coercions (`core.rs`)

- `(counted? nil)` and `(counted? "ab")` are FALSE. Counted:
  collections, subvectors, transients, long ranges, `seq` of
  vector/string/small-map, sort results, take-last-vector/butlast
  results. Not counted: `nil`, strings, double ranges, repeats
  (even finite), cycles, iterates, `keys`/`vals`, conses (`Cons`
  is never `Counted` — fixprobe7 reverses the old read)
  (pins §Print / str, §Predicates / classes, §Fixbatch;
  coll-pins §`counted?`).
- `fn?` includes natives. `record?` is always false (no records
  exist). `simple_class_of` strips the package only
  (`LazySeq`, `core$map`, `PersistentList$EmptyList`)
  (pins §Predicates / classes).
- **Divergence**: `(int 65)` is `Long` here (no 32-bit ints); `%c`
  therefore rejects every integer (D7).
- Coercions (`tests/arith.rs::coerce_rows`, fixprobe10–12):
  `int` stays `Long` but range-checks (longs past i32 →
  `AE | integer overflow`, doubles past it →
  `IAE | Value out of range for int`, `NaN` → 0); `long`
  likewise with the i64 range; `short`/`byte`/`char` IAE on
  `NaN` (unlike `int`/`long`); `float` range-checks and returns
  the exact double (no `f32` rounding); `int`/`long`/`short`/
  `byte` take Characters, `double`/`float` reject them; nil
  trips the `charValue`/`doubleValue`/`longValue` NPE per fn.
  `char` rejects surrogates (the JVM accepts them unprintably).
- `odd?`/`even?` parity (`tests/control.rs::odd_even`).

## D3. Collections + transients (`coll.rs`)

Map classes (pins §Map classes; coll-pins §§Vector assoc, `vec`/`empty`):

- `canBePAM`: ≤16 slots → array-map; ≤128 slots with keys past the
  first 8 pairs ALL keywords → array-map; else hash-map.
  `(hash-map …)` is ALWAYS hash-map; `(array-map …)` ALWAYS
  array-map (never promotes; dup keys: last wins at first position).
- Persistent `assoc` on a new key promotes at ≥64 pairs (keyword
  key) or ≥8 pairs (non-keyword); replace never promotes;
  promotion is one-way. Transient `assoc!` is capacity-gated:
  append while `len < cap`, else promote iff `len ≥ 128` or the
  key is non-keyword, else grow to `min(ceil(n*1.65),128)` made
  even (`kw31-then-int9`→A, `int9-then-kw31`→H, `kw64`→A,
  `kw65`→H). `(transient bigmap)` → TransientHashMap.
- `dissoc` is order-preserving and class-sticky; `dissoc!` is
  SWAP-LAST. `(empty m)` is kind-preserving; `(empty "ab")`,
  `(empty nil)`, `(empty (transient …))` → nil. `empty` keeps
  metadata; `vec` strips it (subvectors stay subvectors).
  `(merge)` → nil; `(merge {} {})` → ArrayMap; `select-keys`
  from big → fresh ArrayMap (restores input meta).

`assoc`/`get`/`contains?`/`find`/`nth` (coll-pins §§Vector
assoc, `get`/`contains?`/`find`, `nth`; pins §`nth`):

- Vector `assoc` keys must satisfy `Util.isInteger` (`Long` only
  here — `Double` never, even `1.0`), else
  `IAE | Key must be integer`. Index is `intValue` (wraps);
  at-count appends; else IOOBE with EMPTY message. `assoc` on
  nil → map. Odd varargs:
  `IAE | assoc expects even number of arguments after
  map/vector, found odd number` (the `No value supplied …` text
  is `array-map`/`hash-map` only); `assoc!` with a trailing key
  silently assocs nil.
- `get` nil-puns bad keys and OOB (vectors, strings, transients);
  `contains?` on vectors is false for bad keys; `contains?` on a
  string with a non-`Number` key is
  `IAE | contains? not supported on type: java.lang.String`;
  other bad types name their full class. `find` is 2-arity only;
  misses are nil; bad types are
  `IAE | find not supported on type: <full class>`.
  `(find [1 2] 0)` → `[0 1]`.
- `nth` coerces the index FIRST (`RT.nth` takes `(Object,int)`):
  Number → long-range check
  (`IAE | Value out of range for long: X`, BigInt prints without
  `N`) → int-range check (`AE | integer overflow`) → truncate
  (`1.9`→1, `3/2`→1, `NaN`→0); nil → NPE `charValue`; anything
  else → CCE →`java.lang.Number` (coercion beats bad/nil coll:
  `(nth 5 "a")` → CCE, `(nth nil 1e100)` → IAE). Then:
  nil coll → nil/`:nf`; non-`Indexed` seqables walk O(n)
  (negative → IOOBE empty); strings → SIOOBE
  `Index N out of bounds for length L` (UTF-16); unsupported
  types → `UOE | nth not supported on this type: X`
  (`getSimpleName` = strip package: `Long`,
  `PersistentArrayMap`, `LazySeq`, `core$map`) and `:nf` does
  NOT save them. Transient maps/sets →
  `UOE | nth not supported on this type: TransientArrayMap`
  (member classes strip the owner — unlike `simple_class_of`).
  Transients support `nth`/`get` but are NOT seqable
  (`IAE | Don't know how to create ISeq from: …`).

`conj`/`merge`/`into`, `peek`/`pop`, subvectors, misc
(coll-pins §§`conj`, `merge`, `peek`/`pop`, Subvectors,
`vec`/`zipmap`, Transients, Arity):

- `(conj nil 1 2 3)` → `(3 2 1)` (plain reduce). Map + vector
  requires exactly 2
  (`IAE | Vector arg to map conj must be a pair`); map + seqable
  loops `assoc` casting each element to `Map$Entry`; map + map
  assocs every pair; 1-arity passes anything through.
  `(conj (transient [1]) 2)` → CCE →IPersistentCollection
  (pins §`conj`/`assoc`/`dissoc` error pins).
- `merge` is into-shaped (`([x y] (if (and x y) (into x y)
  (or x y)))`, left-folded; 1-arity → x). `merge-with` 1-arity
  → nil; maps AND vectors (incl. subvectors) take the `kvreduce`
  path; anything else seqs and `key`/`val`s (CCE →Map$Entry).
  `into` is `reduce conj` (plain only — xform arity out of scope).
- `peek`/`pop` nil-pun. Stacks are lists/vectors/subvectors only;
  every seq value, strings, maps, sets, transients → CCE
  →IPersistentStack. `(pop '())` →
  `ISE | Can't pop empty list`, `(pop [])` →
  `ISE | Can't pop empty vector` (also `pop!`); list pops keep
  the tail's meta (empty restores the popped list's).
- Subvectors: `start == end` → plain `EMPTY` (never a subvector);
  metadata dropped; nested windows flatten; bad int coercions
  match `nth` exactly. `conj`/`assoc` stay subvectors (`assoc`
  at `end` appends); `pop` to ≤1 element and `empty` fall back
  to plain vectors. `(seq sv)` is counted but NOT `IDrop` (drop
  is lazy); subvector invoke names `APersistentVector/SubVector`
  (slash). `replace`/`vec` of a subvector stay subvectors.
- `zipmap` seqs keys before vals, stops at the shorter side,
  interleaves (never over-realizes an infinite side).
- Transients: `(transient x)` on lists/strings/numbers/transients
  → CCE →IEditableCollection; on nil → NPE asTransient
  `"coll"`. `conj!` 0-arity → empty transient vector;
  `dissoc!`/`pop!` hint `ITransientMap`/`ITransientVector`
  (CCE on the wrong kind). Nil bangs name the hinted
  method + local. Use after `persistent!` (incl. `count` and a
  second `persistent!`) is always
  `IllegalAccessError | Transient used after persistent! call`.
  Metadata round-trips (`meta` of a transient is nil).
- `count` of strings is the UTF-16 length (`(count "😀")` → 2);
  transients count via `Counted`; anything else uncountable is
  `UOE | count not supported on this type: <simple>`.
- **`key`/`val` (divergence)**: no `MapEntry` type exists — map
  seqs/`find` yield `[k v]` vectors, and `key`/`val` read slots
  0/1 through `nth`, failing exactly like that `nth`. On the JVM
  non-entries fail the `Map$Entry` cast instead (coll-pins §Known
  divergences; `coll.rs::native_key`).
- `disj` 1-arg passes through (even nil/non-sets); 2+ args
  nil-pun, else CCE→`IPersistentSet`. `set` rebuilds through
  `seq` (nil→`#{}`, strings→char sets, non-seqables fail the
  seq). `empty` is class-sticky for maps, plain for subvectors,
  `()` for seqs, nil otherwise, and keeps metadata on the
  strict kinds (`tests/coll.rs::disj_set_rows`, `empty_fn_rows`;
  fixprobe10 `:disj-vec`, `:set-5`).
- `hash` is `hasheq` after a full realize; unbounded/cyclic
  seqs throw bare `UOE`, infinite lazies hang, like the JVM
  (`tests/control.rs::hash_values`; pins §7i str/tostring +
  hash).

## D4. Seqs + lazy (`seqs.rs`, `lazy.rs`)

Ranges/repeat/iterate/cons (pins §§Ranges, Repeat / cycle /
iterate / cons, 7i range ctors, 7i iterate/cons/cycle):

- `(range)` → `(iterate inc' 0)` (class `Iterate`). All-`int?`
  args → `LongRange/create`, else `Range/create`. Zero step →
  `Repeat(start)` (`(range 5 5 0)` → `()`, `(range 0 5 -0.0)` →
  Repeat). `rangeCount` overflow → double path (`(range 0 MAX 1)`
  → class `Range`). `(range 0 ##NaN)` → infinite `Range`;
  `(range ##NaN ##NaN)` → infinite NaNs; `(range 1 1.0)` → `()`
  (cross-type equiv). 1-arg: `(range nil)` → NPE getClass,
  `(range "a")` → CCE →Number. 3-arg: ANY non-Number in ANY
  slot → CCE/NPE (zero-step saves nothing). `LongRange` is
  Counted + IDrop + chunked; double `Range` is neither; `seq`
  returns self; `next`/`drop` shift.
- `(repeat n x)` is strict, truncates (`2.5`→2 items), `n≤0` →
  `EmptyList`. `(repeatedly n f)` = `(take n (repeatedly f))` —
  LAZY `n` with the take ceil rule (`2.5`→3 calls). `Repeat` is
  seq + IDrop, NOT counted/chunked; `seq`/`next`/`drop`/
  `take-last` preserve it.
- `iterate`'s `more` is LAZY: `rest`/`next` force the CURRENT
  val only (fresh: 0 calls; `rest`-of-`rest`: 1;
  `first`-of-`rest`: 1; `(nth it 2)`: 2). `(iterate 5 0)` → CCE
  →IFn; `(first (iterate nil 0))` → 0;
  `(first (rest (iterate nil 0)))` → NPE `this.f` null.
  `(cons 1 nil)` → `PersistentList`; `(cons 1 [2])` → `Cons`
  (never forces the tail). `(cycle nil)`/`(cycle [])` →
  `EmptyList`; `(cycle [1])` → `Cycle`. `conj`: range/lazy →
  `Cons`; list → `PersistentList`; nil → `List`.

take/drop/partition/sort/`=` (pins §§take-last / butlast / drop,
sort, 7i drop/nthrest, 7i take-last, 7i partition, 7i `=`/equiv):

- `take-last`: 0/neg/empty → nil; result is kind-preserving
  (vector→ChunkedSeq, list→PersistentList, LongRange→LongRange,
  double-Range→Range, Repeat→Repeat, string→StringSeq, small-map
  seq→counted, set/big-map→KeySeq, lazy→ChunkedCons); `n`
  validated eagerly (`nil` n → NPE even on lazy). `butlast`:
  always ChunkedSeq or nil, eager. `drop-last` → LazySeq.
- `drop`: `IDrop` inputs (vector, LongRange, Repeat, StringSeq,
  small-map seq/map, ChunkedSeq, take-last/butlast results)
  validate + drop EAGERLY (past-end → `EmptyList`); everything
  else (incl. subvectors, lists, strings, hash maps,
  double-Range) is lazy — including at `n ≤ 0`.
- Coercion families: `drop`/`take`/`nthrest` CEIL (`2.5`→3);
  `nth`/`repeat`/`subvec`/`subs` TRUNCATE (`2.5`→2).
  Non-positive/NaN drop-`n` returns as-is with NO coercion.
  Positive huge-`n`: IAE long-range, then AE `integer overflow`.
  Huge-`n` on seqs walks to end silently (no throw).
  `nthrest` 0/neg → the coll ITSELF (never seqed).
- `take-nth`: every force = `seq` + `first` + `(drop n s)`
  (IDrop inputs throw on first force with bad `n`);
  `(take-nth 0 coll)` replays the head forever.
  `(interleave)` → `()`; `(concat)` → LazySeq.
- `partition` construction is fully lazy; FIRST force coerces
  the step via STRICT `nthrest` (`"a"` → CCE, nil → NPE
  getClass); MAX step: vector → AE, list → single window;
  n/step 0 or `partition-all`-neg → infinite empties;
  `(partition -1 …)` → `()`.
- `sort`: `ArraySeq`, counted, NOT `IDrop`, no metadata;
  empty/nil → `EmptyList`; stable. Comparator protocol:
  `true`→less, `false`→reversed call, numbers→sign of
  intValue/trunc, else CCE →Number. `sort-by` calls
  `(comp (keyfn x) (keyfn y))` directly.
- `keys`/`vals` of `{}`/nil → nil; construction and tails NEVER
  validate (`(keys [10 20])` → KeySeq; only entry-force throws
  CCE →`Map$Entry`). `meta(next …)` is dropped except
  StringSeq. KeySeq is NOT `IDrop`.
- `(= 1 1.0)` → false; `(= 1N 1)` → true;
  `(= ##NaN ##NaN)` → false. Sequential equiv crosses types.
  Pairwise + early exit (counter-verified: infinite-vs-shorter
  terminates; mismatch at pair 2 forces exactly 2+2).
- Sequential destructuring delegates non-nil/vector/list/string
  colls to `nth` (lazy seqs force and walk; `condp`'s
  `split-at` take-thunks need this) (pins §Control / builtins).
- `reduced`/`reduced?` wrap/test the early-exit value
  (`tests/seqs.rs::reduced_rows`); `unreduced` stays out with
  the transducers.

## D5. Higher-order fns (`seqs.rs`)

`comp` (fn composition, NOT xform comp), `partial`,
`complement`, `juxt`, `constantly`, `identity`, `fnil`,
`every?`, `some`, `not-any?`, `not-every?`, plus `every-pred`,
`some-fn`, `max-key`, `min-key`, `mapv`/`filterv` (eager),
`group-by`, `frequencies`, `dorun`/`doall`, `reduce` (no xform
path). The one-arity (transducer) arities of `map`, `filter`,
`remove`, `keep`, `keep-indexed`, `distinct`, `dedupe`, `into`
are OUT of scope with the rest of transducers (D13); only the
seq arities resolve.

## D6. `str` / `pr-str` / `subs` / `name` (`core.rs`)

(pins §§Format `%s`/LazySeq, Print / str, 7i str/tostring + hash;
coll-pins astral note.)

- `str` is per-arg `.toString` — NOT in the `pr` family.
  `LazySeq` (realized or not) → `clojure.lang.LazySeq@hash`
  where the hash is the Java List CONTENT hash (deterministic:
  `()`→`@1`, `(2 3)`→`@402`, `(\b \c)`→`@1002`,
  `((1 2) (1 2))`→`@8001`, `(1 1)`→`@3e1`, `(1 2)`→`@3e2`,
  `(1)`→`@20`; same via `%s`). The hash walk REALIZES, so
  infinite lazies hang, like the JVM (`tests/str.rs`, corpus
  11/19).
- `str` of strict colls/seqs prints readably (`[1 2]`, `(1 2)`,
  `{:a 1}`, `#{1}`, `["a" :b]` with quotes, `(\a \b)` for
  StringSeq); `print-str`/`pr` DO print lazy contents
  (`(map inc [1])` → `"(2)"`).
- `subs`: the `Double` index truncates (reflector int-unbox);
  nil → `NPE | getClass`; OOB is the JDK text
  (`Range [b, e) out of bounds for length N`).
- `keyword` 1-arg: nil→nil, kw→self, sym/str→kw (split on the
  first `/`), anything else nil (`(keyword 5)` → nil); 2-arg
  casts both sides to `String` (CCE). `symbol` 1-arg: str/sym
  convert, keywords convert (`(symbol :a)` → `a`), anything
  else (nil included) → `IAE | no conversion to symbol`.
  `name`: Named→name, strings pass, nil→`getName` NPE, mistypes
  CCE→`Named`. `namespace`: nil for simple names, nil→NPE,
  strings MISTYPE (CCE) — only `name` accepts them (fixprobe8;
  `tests/arith.rs::str_fn_rows`).
- `pr-str`/`print-str`/`pr`/`print`: bounded-realize with
  `*print-length*` (limit+1 then print, so `...` shows and
  infinite seqs terminate); realization happens up-front
  (interleaving with in-flight prints is a known divergence).
- `*print-length*` and `*print-level*` share the
  nil/negative/fractional/NaN→unlimited, integral-double→that-int
  rules; a non-positive level prints `#` (dec-first);
  `*print-readably*` is honored via `pr`/`print`,
  `*print-meta*` prints the `^meta` prefix, and
  `*print-namespace-maps*` (root true) gates the `#:ns` lift
  (`tests/str.rs::print_opts_rows`; fixprobe9).
  `*print-dup*` is declared but ignored (divergent).

## D7. `format` (`fmt.rs`)

(A `java.util.Formatter` subset, oracle-pinned by the `s5d-fmt*`
batches; `tests/format.rs`, corpus 12/13/14.)

- Flags normalize to the canonical order `- # + space 0 , (`.
  Per-family check orders: numerics width→pair→mismatch→prec;
  `%s` dash-width→mismatch; `%b`/`%h` dash+zero→mismatch;
  `%c`/`%t` mismatch→width; `%n` prec→width→pair→flags; `%%`
  prec→pair→flags→width.
- Floats round HALF_UP from the shortest decimal; `%g` never
  strips; `%a` is bit-exact hex; `%t` renders UTC civil time;
  `%c` takes Characters only (integers rejected — no 32-bit
  ints exist here).
- Documented divergences: astral `%s` precision counts scalars,
  not UTF-16 units; lone surrogates are unrepresentable;
  zones are UTC-only. (`%ts` negatives truncate toward zero
  on the JVM too — conformant, T1192 FX1.)

## D8. `ex-info` family (`core.rs`)

- `ex-info` builds the `ExceptionInfo` value (message + data +
  cause); `ex-data` reads the carried data (nil when absent),
  `ex-message`/`ex-cause` read any throwable's message/cause
  (nil when absent). Cause chains print and catch through the
  S4 throwable path (`tests/control.rs::eval_chains_and_catch`
  and `ex_data_trampoline_rows`, S4 probe22 roundtrip).

## D9. Print + `*out*`/`*err*` + files + `eval` (`io.rs`, `eval.rs`)

- `print`/`println`/`pr`/`prn`/`newline`/`flush` honor `*out*`
  (rebinds redirect); `*out*`/`*err*` roots are the process
  writers, rebindable (`tests/io.rs::io_smoke_rows` captures
  via `with-out-str`).
- `slurp`/`spit` are UTF-8 file roundtrips (`:append`
  appends, `:encoding` honored); `read-string` reads one form
  over the S2 reader (bare EOF errors, no meta or line
  numbers).
- `eval` evaluates one form in the current namespace
  (re-entrant top level).
- `with-out-str` is an analyzer builtin (binding discipline
  over `*out*`, never leaks).

## D10. Control composites (`core.clj` + analyzer builtins)

- `core.clj` defines `defn`/`defn-`/`defonce`, `and`/`or`,
  `when`/`when-not`/`if-not`/`if-let`/`when-let`/`if-some`/
  `when-some`, `cond`, `condp`, `case`, the threaders, `doto`,
  `comment`, `dotimes`/`doseq`/`for`, `lazy-seq`/`lazy-cat`.
  The analyzer builtins stay out of the file: `assert`,
  `time`, `with-out-str`, `lazy-seq*`, `case*`
  (`tests/control.rs`, corpus 15).
- Expansion idioms: privates emit as `(#'...)` (never
  qualified calls); `case*` consts are literal data; sequential
  destructuring delegates non-trivial colls to `nth` (lazy
  seqs force and walk); `if-not` swaps branches instead of
  calling `not`; `for`/`doseq` fuse `:when`-only chains into
  one `mapcat` stage, else filter/take-while outside with
  `map`/`mapcat`.
- `assert` reads `*assert*` at analysis (off →
  `Literal(Nil)`); `time` prints the prn shape; `case`
  overflows to hash dispatch; `condp` applies
  `(pred test expr)` in order.
- `:pre`/`:post` in `defn` are silently ignored (D13);
  `defn-` sets `:private`, `defonce` keeps the root (a re-def
  returns nil, like the JVM).

## D11. Boot (`ns.rs::boot`)

- Installs every `*_NATIVES` table into `clojure.core`,
  builtin-macro placeholders (loud on direct invoke, `:macro`
  meta), port-internal privates (`throw-illegal-arg!`,
  `throw-npe!`, `hash-code` — never referred), the namespace
  vars (`*ns*` etc.; the `^:dynamic` trio carries slot+meta),
  and the `RT.java` dynamics (slot set, no meta).
- Refers every public into `user`, binds the `*ns*` base
  frame, loads the embedded `core.clj` with `*ns*` =
  `clojure.core`, refers the new publics out, and resets the
  `gensym` counter (user-visible numbering restarts at `G__1`,
  per the S5C pin).

## D12. Out of scope (with reasons; `core-coverage.md` rows)

- Transducers + xform arities (1-arg `map`/`filter`/`remove`/
  `keep`/`keep-indexed`, 0-arg `distinct`/`dedupe`, 3-arg
  `into`, `sequence`/`eduction`/`transduce`/`completing`/
  `ensure-reduced`/`unreduced`/`preserving-reduced`/
  `halt-when`): no slice; the port raises ArityException
  (divergent rows, `tests/seqs.rs::xform_divergence_rows`).
- Sorted collections, RNG (`rand-*`/`shuffle`), multimethods,
  `with-in-str`/`read`/`read-line`/`line-seq`/`*in*`,
  `deftype`/`defrecord`/`reify`/`proxy`/`gen-class`,
  `with-open`, protocols, `instance?`/`type`/`class`,
  `import`/`..`: no slice (N1/type-system/IOAdjacent).
- Reference identity (`identical?`): meaningless over the
  port's clone semantics. Regex matching (`re-find`/
  `re-matches`/`re-seq`/`re-pattern`): no slice (the reader
  keeps `#"..."` literals). Tower past Long/Double
  (`bigint`/`bigdec`/`num`/`rationalize`): no slice.
- S6 (concurrency: atoms, futures, promises, agents,
  delay/force/deref, `memoize`), STM (`ref`/`dosync`/…,
  never), S7 (`*1`/`*2`/`*3`/`*e`, CLI).

## D13. Open divergences and risks

- `format`: astral `%s` precision (scalars vs UTF-16 units),
  lone surrogates unrepresentable, `%c` rejects integers,
  zones UTC-only. (`%ts` negatives truncate toward zero on
  the JVM too — resolved conformant, T1192 FX1 fx1-rest.)
- `key`/`val`: no `MapEntry` (slot reads via `nth`; the CCE
  class differs). (`update` on transients and `keys` of
  non-maps: single-shot JVM throws the same full checkcast
  text the port asserts — resolved conformant, T1192 FX1
  fx1-rest/fx1-fastthrow; the "bare" record was HotSpot
  fast-throw elision.) `into` 3-arg takes the port
  ArityException (the JVM takes the xform path). Arity texts
  strip the `--NNNN` impl suffix (`clojure.core/assoc`, not
  `assoc--5596`).
- `char` rejects surrogates (JVM: unprintable but accepted);
  `float` returns exact doubles (no `f32` rounding);
  `int`/`short`/`byte` stay `Long`.
- `hash` over infinite lazies hangs (JVM-true); `nth` f64
  past 2^53 unprobed; `case*` hand-written shapes are port
  spec errors (JVM NPE/IAE out of scope).
- `:pre`/`:post` silently ignored; `sorted-map` reads as
  `{}`; huge longs untested; seq `empty` drops meta (fixed,
  T1192 FX1); `(count (eval …))`-style phase gaps assert the
  port runtime error instead of the JVM compile error.
  (`time` millis digits match the JVM `Double/toString`
  exactly — resolved conformant, T1192 FX1 fx1-time.)