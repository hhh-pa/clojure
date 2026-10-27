# Core coverage (A5, normative)

The complete inventory of `clojure.core` in this port: every var,
its conformance state, and — for exclusions — why it is excluded.
If a var is not listed here, that is a file bug, not a silent gap.

- **692 vars: 423 implemented, 2 divergent, 267 out of scope,
  0 planned.** The port reports Clojure `1.13.0-alpha9`
  (`clojure-version`).
- **Universe.** 692 rows = 685 JVM publics + 7 extra-universe rows: 4 port-visible JVM privates
  (`*loaded-libs*`, `*loading-verbosely*`, `*pending-paths*`,
  `preserving-reduced`), 1 port-only var (`*load-path*`), 2 guard
  rows (`clone-thread-bindings`, `pprint`).
- **Merged `planned` rows must resolve** — the matrix fails on any
  unresolved `planned` row, so add none unresolved (accepted
  future work lands as `out-of-scope` with a `future:` note).
- **Status meanings.** `implemented` — resolves in the booted core
  and behaves like the JVM (pinned by tests named in `notes`).
  `divergent` — resolves, but fenced behavior differs deliberately
  (each difference is pinned by the differential corpus, so drift
  fails CI). `out-of-scope` — must *not* resolve; the port has no
  slice for it. `planned` — accepted future work, resolves once it
  lands (none right now).
- **How to read a row.** `var` is the `clojure.core` name; `notes`
  names the tests or probes that pin it plus slice tags (`S5A`…,
  `T1192`, `M2`…). Section headers group vars by area; the blurbs
  under them say what each area covers.
- **Machine-read shape.** The S5 coverage-matrix test
  (`clj-core/tests/`) parses this file: `planned`/`implemented`/
  `divergent` rows must resolve in the booted core, `out-of-scope`
  rows must not. Keep every table row exactly `| var | status |
  notes |` with the var backticked — prose outside tables is free,
  rows are not.

## Namespaces (S5A)

Creating, finding, and wiring namespaces; loading code into them.

| var | status | notes |
| --- | --- | --- |
| `ns` | implemented | validated upfront, then in-ns/refer/require/use/mark-loaded |
| `in-ns` | implemented | |
| `create-ns` | implemented | |
| `remove-ns` | implemented | |
| `find-ns` | implemented | |
| `all-ns` | implemented | |
| `the-ns` | implemented | |
| `ns-name` | implemented | |
| `ns-aliases` | implemented | |
| `ns-resolve` | implemented | |
| `resolve` | implemented | |
| `alias` | implemented | |
| `refer` | implemented | |
| `use` | implemented | |
| `require` | implemented | embedded + `*load-path*` files |
| `refer-clojure` | implemented | |
| `intern` | implemented | |
| `declare` | implemented | `(def name)` per name plus `:declared` |
| `ns-interns` | implemented | |
| `ns-publics` | implemented | |
| `ns-refers` | implemented | |
| `ns-map` | implemented | |
| `ns-imports` | divergent | always `{}`; no host classes (N1) |
| `ns-unmap` | implemented | |
| `ns-unalias` | implemented | |
| `load` | implemented | file by path over `*load-path*` |
| `load-string` | implemented | |
| `load-file` | implemented | |
| `loaded-libs` | implemented | |
| `*ns*` | implemented | |
| `*file*` | implemented | bound while loading |
| `*loaded-libs*` | implemented | set, not a Ref (S5A2) |
| `*loading-verbosely*` | implemented | read by `load` |
| `*pending-paths*` | implemented | private; cycle guard |
| `*load-path*` | implemented | port-only; dirs vector |

## Vars (S5B)

Var roots, thread-local bindings, and metadata.

| var | status | notes |
| --- | --- | --- |
| `alter-var-root` | implemented | atomic root swap |
| `with-redefs` | implemented | expands to `with-redefs-fn` (builtin_expand.rs expand_with_redefs); fx4_macroexpand.rs expand_nine_heads |
| `bound?` | implemented | |
| `thread-bound?` | implemented | |
| `var-get` | implemented | |
| `var-set` | implemented | |
| `push-thread-bindings` | implemented | |
| `pop-thread-bindings` | implemented | |
| `get-thread-bindings` | implemented | |
| `with-bindings*` | implemented | |
| `with-bindings` | implemented | expands to `with-bindings*` (builtin_expand.rs expand_with_bindings); fx4_macroexpand.rs expand_nine_heads |
| `bound-fn` | implemented | expands to `bound-fn*` (builtin_expand.rs expand_bound_fn); fx4_macroexpand.rs expand_nine_heads; conveyance primitive (S6 builds on it) |
| `meta` | implemented | |
| `with-meta` | implemented | |
| `vary-meta` | implemented | |
| `alter-meta!` | implemented | |
| `reset-meta!` | implemented | |

## Macros (S5C)

Defining macros and expanding them.

| var | status | notes |
| --- | --- | --- |
| `defmacro` | implemented | def of the `&form`/`&env`-prepended fn; `:macro` + user `:arglists` |
| `macroexpand-1` | implemented | one head step; call-site spans; the nine analyzer builtins expand (FX4); real specials + fence macros are identity |
| `macroexpand` | implemented | head loop; the nine analyzer builtins expand (FX4); real specials + fence macros are identity |
| `gensym` | implemented | Rust-backed counter |

## Arithmetic (S5D, checked per C1)

Checked i64 arithmetic (overflow raises, never wraps) plus exact numeric coercions.

| var | status | notes |
| --- | --- | --- |
| `+` | implemented | `control.rs time_rows`; C1, probe39 arith-plus-star-zero |
| `-` | implemented | `time_rows` + dotimes-emitted (`composites_smoke`); C1, probe39 arith-variadic-exotic |
| `*` | implemented | `composites_smoke` (`(f 21)`); C1, probe39 arith-plus-star-zero |
| `/` | implemented | arith.rs div_zero_rows + pred_backlog_rows (exact-if-divisible else double) |
| `quot` | implemented | arith.rs quot_rem_mod_rows (C1, zero/NFE rules) |
| `rem` | implemented | arith.rs quot_rem_mod_rows (C1, zero/NFE rules) |
| `mod` | implemented | arith.rs quot_rem_mod_rows; C1 zero/non-finite rules |
| `inc` | implemented | `(map inc …)` in `coll.rs`/`str.rs`/`format.rs` + dotimes-emitted; C1, probe39 arith-inc-dec |
| `dec` | implemented | arith.rs pred_backlog_rows |
| `max` | implemented | arith.rs max_min_rows (latter-wins, NaN poison) |
| `min` | implemented | arith.rs max_min_rows (latter-wins, NaN poison) |
| `int` | implemented | arith.rs coerce_rows (stays Long, int-range checked) |
| `long` | implemented | arith.rs coerce_rows (i64 range, NaN→0) |
| `double` | implemented | arith.rs coerce_rows (chars rejected) |
| `float` | implemented | arith.rs coerce_rows (range-checked, exact double) |
| `short` | implemented | arith.rs coerce_rows (NaN→IAE, chars convert) |
| `byte` | implemented | arith.rs coerce_rows (NaN→IAE, chars convert) |
| `char` | implemented | arith.rs coerce_rows (surrogates rejected, divergent) |

## Comparison (S5D, per C2)

Same-category equality and JVM-consistent ordering.

| var | status | notes |
| --- | --- | --- |
| `=` | implemented | `hash_values`, `assert_rows`, `condp_rows`, `str_lazy_content_hash`; same-category, `s5d-pins.md:492`, probe39 cmp-eq-* |
| `==` | implemented | `case_hashes` prep; C2; 1-arg passes unchecked (arith.rs comparison_single_rows) |
| `not=` | implemented | arith.rs pred_backlog_rows (negates =) |
| `<` | implemented | `condp_rows` + dotimes-emitted; 1-arg passes unchecked (arith.rs) |
| `>` | implemented | case-body (`case_*`); 1-arg passes unchecked (arith.rs) |
| `<=` | implemented | case mode-cond (`case_ints`); 1-arg passes unchecked (arith.rs) |
| `>=` | implemented | arith.rs pred_backlog_rows |
| `zero?` | implemented | for/doseq assert-args (`composites_smoke`); probe39 cmp-zero-pos-neg |
| `pos?` | implemented | arith.rs pred_backlog_rows |
| `neg?` | implemented | arith.rs pred_backlog_rows |
| `compare` | implemented | arith.rs pred_backlog_rows (C2, NaN ties) |

## Predicates (S5D)

Type and shape predicates.

| var | status | notes |
| --- | --- | --- |
| `nil?` | implemented | `core_loads`, `boot_dynamics` (true+false paths) |
| `not` | implemented | arith.rs not_rows (truthiness, was is-nil) |
| `boolean` | implemented | `boot_dynamics` |
| `some?` | implemented | arith.rs pred_backlog_rows |
| `true?` | implemented | arith.rs pred_backlog_rows |
| `false?` | implemented | arith.rs pred_backlog_rows |
| `boolean?` | implemented | arith.rs pred_backlog_rows |
| `integer?` | implemented | case mode-cond (`case_ints`) |
| `int?` | implemented | arith.rs pred_backlog_rows |
| `float?` | implemented | arith.rs float_rows (identical to double?) |
| `double?` | implemented | arith.rs pred_backlog_rows |
| `number?` | implemented | arith.rs pred_backlog_rows |
| `string?` | implemented | defn sig-parse (`composites_smoke`) |
| `char?` | implemented | arith.rs pred_backlog_rows |
| `keyword?` | implemented | for/doseq groups (`composites_smoke`) |
| `symbol?` | implemented | defn sig-parse (`composites_smoke`) |
| `ident?` | implemented | arith.rs pred_backlog_rows |
| `qualified-ident?` | implemented | arith.rs pred_backlog_rows |
| `simple-ident?` | implemented | arith.rs pred_backlog_rows |
| `qualified-keyword?` | implemented | arith.rs pred_backlog_rows |
| `simple-keyword?` | implemented | arith.rs pred_backlog_rows |
| `qualified-symbol?` | implemented | arith.rs pred_backlog_rows |
| `simple-symbol?` | implemented | arith.rs pred_backlog_rows |
| `coll?` | implemented | `coll.rs counted_pred_rows` |
| `sequential?` | implemented | `counted_pred_rows` (lists covered via `is_seq_value`) |
| `associative?` | implemented | `counted_pred_rows` |
| `counted?` | implemented | `counted_pred_rows` (11 rows) |
| `list?` | implemented | arith.rs pred_backlog_rows |
| `vector?` | implemented | `counted_pred_rows` + `nthrest_rows` + defn assert-args |
| `map?` | implemented | `counted_pred_rows` + defn sig-parse |
| `set?` | implemented | arith.rs pred_backlog_rows |
| `sorted?` | implemented | true on sorted maps/sets (FT6 sort sidecar); coll.rs sorted_sticky_rows |
| `reversible?` | implemented | vectors, subvecs, entries, and sorted colls (FT6); coll.rs sorted_sticky_rows |
| `fn?` | implemented | arith.rs pred_backlog_rows (incl. natives) |
| `ifn?` | implemented | `counted_pred_rows` |
| `var?` | implemented | arith.rs pred_backlog_rows |
| `seq?` | implemented | `counted_pred_rows` + case assoc-test incl. seq-const (`case_hashes`) |
| `seqable?` | implemented | `counted_pred_rows` |
| `record?` | implemented | arith.rs pred_backlog_rows (always false) |
| `odd?` | implemented | control.rs odd_even (parity) |
| `even?` | implemented | control.rs odd_even (parity) |

## Strings (S5D)

String building, conversion, and formatting.

| var | status | notes |
| --- | --- | --- |
| `str` | implemented | variadic; LazySeq prints @content-hash, seqs/colls readably |
| `keyword` | implemented | arith.rs str_fn_rows (nil else-arm, fixprobe8) |
| `symbol` | implemented | arith.rs str_fn_rows (no-conversion IAE) |
| `name` | implemented | arith.rs str_fn_rows (getName NPE, Named CCE) |
| `namespace` | implemented | arith.rs str_fn_rows (nil/simple, strings mistype) |
| `subs` | implemented | nth-family index coercion; JDK OOB text |
| `pr-str` | implemented | arith.rs str_fn_rows (exact spacing) |
| `print-str` | implemented | 1 assert: str.rs str_seq_contents_corpus_19 (contents-printing) |
| `format` | implemented | Formatter subset, oracle-pinned (`fmt.rs`, 15/15 `format.rs`) |

## Collections (S5D)

Persistent collections: lookup, update, transients, sets, maps.

| var | status | notes |
| --- | --- | --- |
| `count` | implemented | coll.rs count_rows (UTF-16, transients, subvec, walk, UOE) |
| `empty?` | implemented | coll.rs empty_rows (nil/str/transient, IAE on 5) |
| `not-empty` | implemented | coll.rs empty_rows |
| `seq` | implemented | eager (S5D makes it lazy); nil-punning |
| `first` | implemented | control.rs key_val + lazy_seq_star_rows + case_star_shape |
| `second` | implemented | coll.rs pos_rows |
| `last` | implemented | coll.rs pos_rows (string→char, nil, IAE) |
| `rest` | implemented | tests/seqs.rs seq_direct_rows (() on nil) |
| `next` | implemented | tests/seqs.rs seq_direct_rows (nil on empty) |
| `nnext` | implemented | coll.rs pos_rows |
| `fnext` | implemented | coll.rs pos_rows |
| `nfirst` | implemented | coll.rs pos_rows (+IAE) |
| `cons` | implemented | str.rs str_seq_contents + coll.rs counted_pred_rows |
| `conj` | implemented | coll.rs conj_rows (pair-arg, map CCEs, subvec class) |
| `assoc` | implemented | coll.rs assoc_rows (Key-must-be-int, odd-arity IAE, subvec class) |
| `dissoc` | implemented | coll.rs dissoc_rows (nil/1-arity passthrough, transient CCE) |
| `get` | implemented | coll.rs get_rows (nil-pun, strings, transients, subvec) |
| `req!` | implemented | M2 9e51d6dc (`:added 1.13`): arity-2 get that throws `Missing required key` when absent; coll.rs req_rows |
| `contains?` | implemented | coll.rs contains_rows (incl. string non-Number IAE) |
| `find` | implemented | coll.rs find_rows ([k v] vectors, 2-arity only) |
| `keys` | implemented | str.rs str_seq_contents + str_lazy_content_hash + coll.rs counted_pred_rows; entry-CCE per 7i pins (lazy.rs entry_of) |
| `vals` | implemented | tests/seqs.rs seq_direct_rows |
| `merge` | implemented | coll.rs merge_rows (nil→nil, into-shaped error) |
| `merge-with` | implemented | coll.rs merge_with_rows (kvreduce incl. vectors, Map$Entry CCE) |
| `select-keys` | implemented | coll.rs select_keys_rows (get-able, meta restore) |
| `vec` | implemented | coll.rs vec_rows (subvec stays SubVector, meta stripped) |
| `vector` | implemented | early for syntax-quote (S5C) |
| `list` | implemented | early for syntax-quote (S5C) |
| `list*` | implemented | coll.rs list_star_rows |
| `nth` | implemented | coll.rs nth_rows (f64 truncate, coerce-first, UOE/SIOOBE/IOOBE); neg+:nf→:nf matches RT.java nthFrom |
| `update` | implemented | coll.rs update_rows |
| `update-in` | implemented | coll.rs in_rows |
| `assoc-in` | implemented | coll.rs in_rows |
| `get-in` | implemented | coll.rs in_rows |
| `zipmap` | implemented | coll.rs zipmap_rows |
| `array-map` | implemented | coll.rs array_map_rows (forced array class, dup-last-wins) |
| `hash-map` | implemented | early for syntax-quote (S5C) |
| `hash-set` | implemented | early for syntax-quote (S5C) |
| `transient` | implemented | coll.rs transient_rows (class, nil-NPE, CCE) |
| `persistent!` | implemented | coll.rs transient_rows (meta round-trip, use-after-persistent!) |
| `conj!` | implemented | coll.rs transient_rows (0-arity, map pairs, self-conj CCE) |
| `assoc!` | implemented | coll.rs transient_rows (Key-must-be-int, wrong-kind CCE) |
| `dissoc!` | implemented | coll.rs transient_rows (wrong-kind CCE) |
| `pop!` | implemented | coll.rs transient_rows (empty ISE, map-kind CCE) |
| `peek` | implemented | coll.rs peek_pop_rows (nil-pun, IPersistentStack CCE) |
| `pop` | implemented | coll.rs peek_pop_rows; subvec 2→1 stays SubVector per APersistentVector.java pop() |
| `subvec` | implemented | coll.rs subvec_rows (s==e→EMPTY, nesting, invoke arity) |
| `replace` | implemented | coll.rs replace_rows (vector/subvec stay, else LazySeq) |
| `into` | implemented | coll.rs into_rows + 3-arg xform (T1200 transduce_rows; probes xform.out) |
| `distinct?` | implemented | coll.rs distinct_rows |
| `empty` | implemented | coll.rs empty_fn_rows (class-sticky, meta kept, seqs→(), else nil) |
| `key` | implemented | T1192 FX2 entry-only (control.rs key_val + map_entry.rs key_and_val; non-entries fail the Map$Entry CCE) |
| `val` | implemented | T1192 FX2 entry-only (control.rs key_val + map_entry.rs key_and_val; non-entries fail the Map$Entry CCE) |
| `disj` | implemented | coll.rs disj_set_rows (passthrough, nil-pun, CCE→Set) |
| `set` | implemented | coll.rs disj_set_rows (rebuild via seq; :set-5 seq IAE) |

## Seqs (S5D, chunked-lazy per C4)

Lazy sequence operations (chunked like the JVM); transducer arities implemented (T1200).

| var | status | notes |
| --- | --- | --- |
| `map` | implemented | seq arities (corpus 02) + 1-arg transducer (T1200 xform_rows; probes xform.out) |
| `filter` | implemented | seq arities (corpus 02) + 1-arg transducer (T1200 xform_rows; probes xform.out) |
| `remove` | implemented | seq arities (corpus 02) + 1-arg transducer (T1200 xform_rows; probes xform.out) |
| `reduce` | implemented | corpus 02/08; reduced short-circuit, f/0 empty rule + eductions (T1200; 2-arg CCE) |
| `take` | implemented | rust str::str_lazy_content_hash + corpus 02/17 (ceil rule) + 1-arg transducer (T1200) |
| `drop` | implemented | rust control::hash_values + corpus 17; IDrop eager per pins 7i + 1-arg transducer (T1200) |
| `take-while` | implemented | tests/seqs.rs while_rows (prefix shape) + 1-arg transducer (T1200) |
| `drop-while` | implemented | tests/seqs.rs while_rows (suffix shape) + 1-arg transducer (T1200) |
| `take-last` | implemented | corpus 02/05/19; kind-preserving, eager n (pins 7i) |
| `drop-last` | implemented | rust str::str_lazy_content_hash + corpus 02/05/19; map-thunk shape |
| `split-at` | implemented | corpus 02; [take_thunk, drop_value] pair |
| `split-with` | implemented | tests/seqs.rs while_rows ([take drop] pair) |
| `concat` | implemented | eager (S5D makes it lazy); variadic |
| `cycle` | implemented | rust control::hash_values + corpus 02/04; Cycle class |
| `interleave` | implemented | corpus 02/04/18; () on 0-arg (pins 7i) |
| `interpose` | implemented | corpus 02/04 + 1-arg transducer (T1200) |
| `repeat` | implemented | rust coll::count_rows + corpus 02/03; strict n, Repeat IDrop (pins) |
| `repeatedly` | implemented | corpus 02/03; (take n (repeatedly f)), lazy n ceil (pins) |
| `iterate` | implemented | rust control::hash_values + corpus 02/04/18; lazy more (pins 7i) |
| `range` | implemented | rust coll::count_rows + corpus 02/03/16; LongRange/Range dispatch (pins) |
| `reverse` | implemented | corpus 02/05/10; eager reversed list |
| `rseq` | implemented | T1192 FX2 map_entry.rs rseq_rows (RSeq kind, Reversible cast, rev NPE) |
| `sort` | implemented | rust str::str_seq_contents + corpus 02/10; ArraySeq, AFunction.compare (pins) |
| `sort-by` | implemented | corpus 02/10; direct (comp (kf x) (kf y)) (pins) |
| `mapcat` | implemented | corpus 02/08; concat-of-map composite + 1-arg transducer (T1200) |
| `keep` | implemented | seq arities (corpus 02) + 1-arg transducer (T1200 xform_rows; probes xform.out) |
| `keep-indexed` | implemented | seq arities (corpus 02) + 1-arg transducer (T1200 xform_rows; probes xform.out) |
| `map-indexed` | implemented | corpus 02 + 1-arg transducer (T1200) |
| `distinct` | implemented | seq arities (corpus 02) + 0-arg transducer (T1200 xform_rows; probes xform.out) |
| `dedupe` | implemented | seq arities (corpus 02) + 0-arg transducer (T1200 xform_rows; probes xform.out) |
| `partition` | implemented | corpus 02/06/18; eager step validate per window (pins 7i-partition) |
| `partition-all` | implemented | corpus 06/18; trailing partial kept |
| `partition-by` | implemented | corpus 02/06; cons+take-while-tail runs |
| `take-nth` | implemented | corpus 02/06; seq+first+(drop n s) per force, 0 replays (pins) + 1-arg transducer (T1200) |
| `butlast` | implemented | corpus 02/05; eager ChunkedSeq-or-nil (pins 7i) |
| `dorun` | implemented | corpus 02; force_n walk, nil |
| `doall` | implemented | rust str::str_lazy_content_hash + corpus 02; returns input |
| `mapv` | implemented | corpus 02 :mapv-*; eager via collect_all |
| `filterv` | implemented | corpus 02 :filterv-*; eager via collect_all |
| `reductions` | implemented | corpus 02; reduced-init → [inner] edge |
| `every-pred` | implemented | corpus 15; 0-arg→true, >3-pred order branch |
| `some-fn` | implemented | corpus 15; 0-arg→nil, >3-pred order branch |
| `group-by` | implemented | corpus 02; conj-onto-vector groups |
| `frequencies` | implemented | corpus 02 |
| `max-key` | implemented | corpus 10; >= keeps later ties |
| `min-key` | implemented | corpus 10; <= keeps later ties |
| `nthrest` | implemented | control.rs nthrest_rows (0/neg→coll itself, ceil rule) |
| `nthnext` | implemented | T1192 FX2 map_entry.rs seq_fn_rows (`(seq (nthrest coll n))`) |
| `reduced` | implemented | tests/seqs.rs reduced_rows (early-exit wrap) |
| `reduced?` | implemented | tests/seqs.rs reduced_rows |
| `sequence` | implemented | tests/seqs.rs sequence_eduction_rows (T1200; probes xform.out) |
| `transduce` | implemented | tests/seqs.rs transduce_rows (T1200; probes xform.out) |
| `eduction` | implemented | tests/seqs.rs sequence_eduction_rows (T1200; Eduction value; probes xform.out) |
| `completing` | implemented | tests/seqs.rs completing_rows (T1200; probes xform.out) |
| `ensure-reduced` | implemented | tests/seqs.rs reduced_helpers_rows (T1200) |
| `unreduced` | implemented | tests/seqs.rs reduced_helpers_rows (T1200) |
| `preserving-reduced` | implemented | private (not referred); tests/seqs.rs reduced_helpers_rows (T1200; probes xform.out) |
| `halt-when` | implemented | tests/seqs.rs halt_when_rows (T1200; `::halt` contract; probes xform.out) |
| `cat` | implemented | tests/seqs.rs xform_builders_composite_rows (T1200; probes xform.out) |
| `random-sample` | implemented | `core.clj` defn over `filter` + `rand` (zero Rust); re_rng.rs random_sample_rows |

## Higher-order (S5D)

Function combinators and application.

| var | status | notes |
| --- | --- | --- |
| `apply` | implemented | early for syntax-quote (S5C) |
| `comp` | implemented | corpus 15; 0→identity, 1→self, right-nested apply |
| `partial` | implemented | corpus 15; 1-arg→self |
| `complement` | implemented | corpus 15 |
| `juxt` | implemented | corpus 15; vector of calls |
| `constantly` | implemented | corpus 15 |
| `identity` | implemented | corpus 02/15 (some/every?/keep rows) |
| `fnil` | implemented | rust coll::in_rows + corpus 07/15; min-arity fill |
| `every?` | implemented | corpus 02; short-circuit, true on empty |
| `some` | implemented | corpus 02 :some-*; returns truthy val |
| `not-any?` | implemented | corpus 02 |
| `not-every?` | implemented | corpus 02 |

## Control (S5D, embedded `.clj` composites)

Branching, binding, and iteration macros, mostly real `core.clj` ports.

| var | status | notes |
| --- | --- | --- |
| `and` | implemented | core.clj real; control.rs core_loads |
| `or` | implemented | control.rs control_backlog_rows |
| `when` | implemented | core.clj real; control.rs composites_smoke (nested, "13") |
| `when-not` | implemented | control.rs control_backlog_rows |
| `if-not` | implemented | control.rs control_backlog_rows (branch swap) |
| `if-let` | implemented | control.rs control_backlog_rows |
| `when-let` | implemented | control.rs control_backlog_rows |
| `when-some` | implemented | control.rs control_backlog_rows (0 passes) |
| `if-some` | implemented | control.rs control_backlog_rows (false passes) |
| `cond` | implemented | control.rs control_backlog_rows (branching, nil default) |
| `condp` | implemented | core.clj real; control.rs condp_rows (:>>, IAE, pred order) |
| `case` | implemented | core.clj + case* analyzer; control.rs case_ints/hashes/identity/dup/star_shape |
| `->` | implemented | control.rs control_backlog_rows |
| `->>` | implemented | control.rs control_backlog_rows |
| `as->` | implemented | control.rs control_backlog_rows |
| `cond->` | implemented | control.rs control_backlog_rows |
| `cond->>` | implemented | control.rs control_backlog_rows |
| `doto` | implemented | core.clj real; control.rs composites_smoke |
| `comment` | implemented | control.rs control_backlog_rows (nil) |
| `defn` | implemented | core.clj real; control.rs composites_smoke + macro_error_classes |
| `defn-` | implemented | control.rs control_backlog_rows (:private meta) |
| `defonce` | implemented | control.rs control_backlog_rows (nil re-def, keeps root) |
| `dotimes` | implemented | core.clj real; control.rs composites_smoke (incl CCE path) |
| `doseq` | implemented | core.clj real; control.rs composites_smoke + macro_error_classes (IAE+NPE) |
| `for` | implemented | core.clj real; control.rs composites_smoke + macro_error_classes |
| `lazy-seq` | implemented | core.clj → lazy-seq* analyzer; control.rs composites_smoke + lazy_seq_star_rows |
| `lazy-cat` | implemented | core.clj real; control.rs composites_smoke |
| `assert` | implemented | analyzer builtin (*assert* gate); control.rs assert_rows/disabled/arity |
| `time` | implemented | analyzer builtin (prn shape); control.rs time_rows |
| `with-out-str` | implemented | analyzer builtin (binding discipline); control.rs with_out_str_rows |

## Exceptions (S5D)

Exception info carriers.

| var | status | notes |
| --- | --- | --- |
| `ex-info` | implemented | native real; str.rs str_lazy_content_hash (ExceptionInfo/boom) + control.rs with_out_str_rows |
| `ex-data` | implemented | control.rs ex_data_trampoline_rows |
| `ex-message` | implemented | native real; control.rs eval_chains_and_catch |
| `ex-cause` | implemented | native real; control.rs eval_chains_and_catch |

## IO (S5D)

Printing, reading, files, and print dynamic vars.

| var | status | notes |
| --- | --- | --- |
| `print` | implemented | native real; control.rs with_out_str_rows + composites_smoke + str.rs |
| `println` | implemented | tests/io.rs io_smoke_rows (with-out-str capture) |
| `pr` | implemented | tests/io.rs io_smoke_rows |
| `prn` | implemented | native real; control.rs with_out_str_rows (prn 1) |
| `newline` | implemented | tests/io.rs io_smoke_rows |
| `flush` | implemented | tests/io.rs io_smoke_rows |
| `*out*` | implemented | dynamic root; control.rs boot_dynamics + with_out_str_rows (rebind) |
| `*err*` | implemented | dynamic root; control.rs boot_dynamics (root + referred) |
| `slurp` | implemented | tests/io.rs io_smoke_rows (roundtrip) |
| `spit` | implemented | tests/io.rs io_smoke_rows (:append) |
| `read-string` | implemented | tests/io.rs io_smoke_rows (S2, 1 form); denies reader conditionals (S-instream, instream_read_string_deny_row); 2-arity opts (F-read-opts, read_string_opts_rows; probes rs-opts-*) |
| `*print-length*` | implemented | dyn var, root+meta; control.rs boot_dynamics |
| `*print-level*` | implemented | dyn var, root+meta; control.rs boot_dynamics |
| `*print-dup*` | divergent | declared; ignored by the printer |
| `*print-meta*` | implemented | ^meta prefix honored (str.rs print_opts_rows) |
| `*print-readably*` | implemented | honored via pr/print natives (str.rs print_opts_rows) |
| `*print-namespace-maps*` | implemented | root true; gates #:ns lift (str.rs print_opts_rows) |

## Misc (S5D)

Evaluation, trampolines, compiler flags, hashing.

| var | status | notes |
| --- | --- | --- |
| `eval` | implemented | eval one form in the current ns |
| `trampoline` | implemented | control.rs ex_data_trampoline_rows (bounce to :done) |
| `*assert*` | implemented | root true, no meta, binds; control.rs boot_dynamics + assert_disabled |
| `*unchecked-math*` | implemented | root false; control.rs boot_dynamics |
| `*warn-on-reflection*` | implemented | root false; control.rs boot_dynamics |
| `*allow-unresolved-vars*` | implemented | root false; control.rs boot_dynamics |
| `hash` | implemented | control.rs hash_values (hasheq after realize; bare UOE) |

## Special-form macros (S4 builtins, S5C vars)

Special forms exposed as macros; `macroexpand-1` expands them (only real specials and fence macros are identity).

| var | status | notes |
| --- | --- | --- |
| `let` | implemented | expands to `let*` + destructured bindings (builtin_expand.rs expand_let); fx4_macroexpand.rs expand_nine_heads |
| `loop` | implemented | expands to `loop*` (+ loop dance for destructuring); fx4_macroexpand.rs expand_nine_heads |
| `fn` | implemented | expands to `fn*` (builtin_expand.rs expand_fn); fx4_macroexpand.rs expand_nine_heads |
| `binding` | implemented | expands to the push-thread-bindings form; fx4_macroexpand.rs expand_nine_heads |
| `letfn` | implemented | expands to `letfn*` (builtin_expand.rs expand_letfn); fx4_macroexpand.rs expand_nine_heads |

## Out of scope (significant exclusions)

Significant vars with no port slice, grouped below by area with the reason. (Three implemented groups live at the end of this section — concurrency, the REPL/CLI environment, and previously unlisted vars that resolve today — because they were inventoried with the exclusions around them.)

| var | status | notes |
| --- | --- | --- |
| `clone-thread-bindings` | out-of-scope | does not exist on the JVM (probe37 `clone-missing`) |
| `sorted-map` | implemented | ordered sidecar on `Map` (natural `jvm_compare`); coll.rs sorted_construction_rows |
| `sorted-set` | implemented | ordered sidecar on `Set`; coll.rs sorted_construction_rows |
| `sorted-map-by` | implemented | custom comparator calls `(cmp a b)` (`sort-by` semantics); coll.rs sorted_construction_rows |
| `sorted-set-by` | implemented | custom comparator set; coll.rs sorted_construction_rows |
| `subseq` | implemented | `core.clj` bound walks over entry keys (3-arg + start/end 5-arg); coll.rs sorted_subseq_rows |
| `rsubseq` | implemented | descending-from-end `seqFrom` walk, head-dropped once by the end test and bounded by the start test (5-arg); coll.rs sorted_subseq_rows + sorted_gap_rows |
| `rand` | implemented | rng.rs natives mirroring `Math/random` formulas; re_rng.rs rand_rows |
| `rand-int` | implemented | rng.rs `randomToLong` path; re_rng.rs rand_int_rows |
| `shuffle` | implemented | Fisher-Yates over seqables (lazies realized); re_rng.rs shuffle_rows |
| `instance?` | out-of-scope | interop (N1) |
| `type` | out-of-scope | class-based; no slice |
| `class` | out-of-scope | interop (N1) |
| `bases` | out-of-scope | interop (N1) |
| `supers` | out-of-scope | interop (N1) |
| `defmulti` | implemented | multimethods.rs multi_dispatch_* + multi_var_shape + multi_custom_default + multi_options_rejected + multi_no_method + multi_bad_dispatch + multi_class_dispatch_analog + multi_value_semantics + multi_native_arity_guards + multi_multifn_native_guards; S-typechain stage 3 (#33): dispatch fn + docstring/attr head, :default option only, exact-plus-default invoke |
| `defmethod` | implemented | multimethods.rs multi_arity_methods + multi_defmethod_returns_multifn + multi_defmethod_guards + multi_defmethod_missing; S-typechain stage 3 (#33): single/multi-arity tails, answers the multifn |
| `prefer-method` | implemented | multimethods.rs multi_prefer_method; S-typechain stage 3 (#33): edges stored, never consulted (hierarchy fenced) |
| `get-method` | implemented | multimethods.rs multi_get_method + multi_value_guards; S-typechain stage 3 (#33): exact entry else default entry else nil |
| `methods` | implemented | multimethods.rs multi_methods_view + multi_value_guards; S-typechain stage 3 (#33): whole table incl the :default entry |
| `remove-method` | implemented | multimethods.rs multi_remove_method + multi_value_guards; S-typechain stage 3 (#33): missing entry no-op, answers the multifn |
| `deftype` | implemented | types.rs dtor_*; S-typechain stage 1 (#33): namespace-registry types, namespace-qualified construction, dot interop |
| `defrecord` | implemented | types.rs rec_*; S-typechain stage 1 (#33): content-map records, ->/map-> factories, record hash formula |
| `reify` | implemented | types.rs reify_*; S-typechain stage 1 (#33): anonymous instances over interfaces |
| `proxy` | implemented | types.rs proxy_*; S-typechain stage 1 (#33): Object-only superclass, this-less bodies |
| `gen-class` | implemented | types.rs genclass_*; S-typechain stage 1 (#33): runtime-constructable (documented divergence), prefix-fn forwarding |
| `gen-interface` | implemented | types.rs geniface_*; S-typechain stage 1 (#33): signature descriptors |
| `definterface` | implemented | types.rs iface_*; S-typechain stage 1 (#33): signature descriptors |
| `with-in-str` | implemented | analyzer builtin (body value); tests/io.rs instream_with_in_str_rows |
| `read` | implemented | 5 arities over StreamReader (opts map, ReaderException wrap); tests/io.rs instream_read_value_rows, instream_read_eof_rows, instream_read_error_rows, instream_read_opts_edge_rows, instream_recursive_unwrap_rows, instream_catch_rows |
| `read-line` | implemented | 0-arity over *in* (nil at EOF); tests/io.rs instream_read_line_rows |
| `line-seq` | implemented | core.clj eager-head Cons + lazy tail (accepts StreamReader by design); tests/io.rs instream_line_seq_rows |
| `*in*` | implemented | dynamic root (stdin reader); tests/io.rs instream_in_shape_rows |
| `import` | out-of-scope | interop (N1) |
| `..` | out-of-scope | interop (N1) |
| `defprotocol` | implemented | protocols.rs proto_*; S-typechain stage 2 (#33): map descriptors (:on/:sigs/:impls/:interface), variadic dispatch fns |
| `with-open` | implemented | protocols.rs with_open_*; S-typechain stage 2 (#33): Closeable contract, reverse-order finally closes |
| `extend` | implemented | protocols.rs extend_*; S-typechain stage 2 (#33): map-form extends, bare-fn per-arity merge |
| `extend-type` | implemented | protocols.rs extend_*; S-typechain stage 2 (#33): named-type extensions under the var-alter lock |
| `extend-protocol` | implemented | protocols.rs extend_*; S-typechain stage 2 (#33): host-class/null/IPersistentMap extensions |
| `satisfies?` | implemented | protocols.rs satisfies_*; S-typechain stage 2 (#33): registry + inline claims; descriptors never satisfy |
| `extends?` | implemented | protocols.rs extends_*; S-typechain stage 2 (#33): descriptor names, class-name strings, IPM fallback |

### Concurrency (implemented — atoms, futures, agents; STM excluded above)

| var | status | notes |
| --- | --- | --- |
| `atom` | implemented | concurrency.rs atom_ctor_rows (validator/meta options, odd-opts IAE) |
| `swap!` | implemented | concurrency.rs swap_rows (CAS loop, extra args, unconditional watch fire) |
| `reset!` | implemented | concurrency.rs reset_rows (validates, fires) |
| `compare-and-set!` | implemented | concurrency.rs cas_rows (bool, validates pre-compare, no fire on miss) |
| `shutdown-agents` | implemented | concurrency.rs agent_shutdown_rows (nil; joins pools + future threads; later sends drop like the JVM) |
| `delay` | implemented | concurrency.rs delay_rows (delay* form over trapped closure) |
| `force` | implemented | concurrency.rs force_rows (non-delays pass through) |
| `deref` | implemented | concurrency.rs deref_rows + deref_timeout_rows (1-arity atoms/delays/vars/futures/promises; 3-arity IBlockingDeref with Future-cast-first) |
| `future` | implemented | concurrency.rs future_rows (core.clj macro over future-call) |
| `future-call` | implemented | concurrency.rs future_rows (binding conveyance, cached failures) |
| `future?` | implemented | concurrency.rs future_pred_rows (futures only; promises false) |
| `future-cancel` | implemented | concurrency.rs future_cancel_rows (bool; no interrupt) |
| `future-done?` | implemented | concurrency.rs future_pred_rows (done incl. cancelled) |
| `promise` | implemented | concurrency.rs promise_rows (pending latch, invokable) |
| `deliver` | implemented | concurrency.rs promise_rows (invokes; first wins) |
| `realized?` | implemented | concurrency.rs realized_rows (delays + lazy seqs + futures/promises; atoms CCE) |
| `send` | implemented | concurrency.rs agent_send_shapes (fixed pool; extra args; failed throws) |
| `send-off` | implemented | concurrency.rs agent_send_shapes (unbounded pool; failed throws) |
| `send-via` | implemented | concurrency.rs agent_send_shapes (executor ignored; routes to send-off pool) |
| `await` | implemented | concurrency.rs agent_await_rows (nil; barrier rendezvous; throws when failed/in action) |
| `await-for` | implemented | concurrency.rs agent_await_rows (bool; deref-style timeout; throws when failed) |
| `agent` | implemented | concurrency.rs agent_ctor_rows (meta/validator/handler/mode options; validates initial) |
| `agent-error` | implemented | concurrency.rs agent_error_mode_rows (latched throwable or nil) |
| `restart-agent` | implemented | concurrency.rs agent_restart_rows (new state; clears; :clear-actions; validates) |
| `set-error-handler!` | implemented | concurrency.rs set_error_handler_rows (nil clears; void→nil; fires on next failure) |
| `set-error-mode!` | implemented | concurrency.rs set_error_mode_rows (void→nil; :bogus/nil behave as :fail) |
| `add-watch` | implemented | concurrency.rs watch_rows (returns ref, [k ref old new]) |
| `remove-watch` | implemented | concurrency.rs watch_rows (returns ref, missing keys no-op) |
| `locking` | implemented | expands to the monitor-enter/exit form (builtin_expand.rs expand_locking); single global reentrant monitor (S6D); fx4_macroexpand.rs expand_locking_and_destructure_gensyms |
| `pvalues` | implemented | concurrency.rs pvalues_rows (core.clj port over pcalls) |
| `pcalls` | implemented | concurrency.rs pcalls_rows (core.clj port over pmap) |
| `pmap` | implemented | concurrency.rs pmap_rows (core.clj port; fixed lookahead 6; lazy) |
| `memoize` | implemented | concurrency.rs memoize_rows (core.clj port, find-based cache) |
| `ref` | out-of-scope | STM (N4): never |
| `dosync` | out-of-scope | STM (N4): never |
| `commute` | out-of-scope | STM (N4): never |
| `ensure` | out-of-scope | STM (N4): never |
| `alter` | out-of-scope | STM (N4): never |
| `ref-set` | out-of-scope | STM (N4): never |

### REPL and CLI environment (implemented)

| var | status | notes |
| --- | --- | --- |
| `*1` | implemented | S7 REPL: dynamic nil root; session window (P1) |
| `*2` | implemented | S7 REPL: dynamic nil root; session window (P1) |
| `*3` | implemented | S7 REPL: dynamic nil root; session window (P1) |
| `*e` | implemented | S7 REPL: dynamic nil root; last throwable (P2) |
| `*command-line-args*` | implemented | S7 CLI: dynamic nil root; run binds the arg seq (P4) |
| `clojure-version` | implemented | S7 environment: port release literal "1.13.0-alpha9" (P3) |
| `*clojure-version*` | implemented | S7 environment: plain root version map (P3) |

### Still excluded (reasons inline)

| var | status | notes |
| --- | --- | --- |
| `identical?` | out-of-scope | reference identity: no slice (values clone) |
| `re-find` | implemented | re.rs over the `regex` crate (String patterns fail the `String` cast); re_rng.rs re_find_rows |
| `re-matches` | implemented | re.rs full-match; re_rng.rs re_matches_rows |
| `re-seq` | implemented | re.rs lazy match seq; re_rng.rs re_seq_rows |
| `re-pattern` | implemented | re.rs (`Pattern` passes through, else the `String` cast); re_rng.rs re_pattern_rows |
| `bigint` | implemented | tower `BigInt` coercion (`BigInteger` semantics); tower.rs tower_bigint_rows |
| `bigdec` | implemented | tower `BigDec` coercion (`BigDecimal` semantics); tower.rs tower_bigdec_rows |
| `num` | implemented | tower passthrough (nil is nil); tower.rs tower_num_rationalize_rows |
| `rationalize` | implemented | tower via `BigDecimal.valueOf` (`(rationalize 0.1)` is `1/10`); tower.rs tower_num_rationalize_rows |
| `pprint` | out-of-scope | clojure.pprint namespace, not core |

### Previously unlisted — resolves today

JVM publics that already resolve in the booted core; the missing rows were file bugs (#40 audit).

| var | status | notes |
| --- | --- | --- |
| `*agent*` | implemented | dynamic nil root, bound over actions (ns.rs def_dynamic_root; concurrency.rs agent_star_rows) |
| `bound-fn*` | implemented | fx4_host.rs native_bound_fn (conveyed bindings; fx4_macroexpand.rs host_natives_and_rewrites) |
| `inc'` | implemented | core.rs native_inc_prime (= inc on the tower; arith.rs core_r4_scalar_gaps) |
| `some-vals` | implemented | fx4_host.rs native_some_vals (drops nil vals; fx4_macroexpand.rs host_natives_and_rewrites) |
| `with-redefs-fn` | implemented | fx4_host.rs native_with_redefs_fn (root swap/restore; fx4_macroexpand.rs host_natives_and_rewrites) |

### Previously unlisted — accepted future work

Ready slices, `out-of-scope` today only because the matrix fails on unresolved `planned` rows; each note names the future work.

| var | status | notes |
| --- | --- | --- |
| `NaN?` | out-of-scope | future: double/int predicate over Value kinds |
| `abs` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `any?` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `biginteger` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `bounded-count` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `decimal?` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `delay?` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `denominator` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `disj!` | out-of-scope | future: transient/atom gap-fill |
| `error-handler` | out-of-scope | future: agent error-handler read (1-arity [a]) |
| `error-mode` | out-of-scope | future: agent error-mode read (1-arity [a]) |
| `ffirst` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `find-keyword` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `find-var` | out-of-scope | future: loader/var introspection over existing ns machinery |
| `flatten` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `future-cancelled?` | out-of-scope | future: agent/future field read |
| `get-validator` | out-of-scope | future: IRef validator read ([iref]; futures rejected) |
| `indexed?` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `infinite?` | out-of-scope | future: double/int predicate over Value kinds |
| `map-entry?` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `merge-deep` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `merge-deep-with` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `nat-int?` | out-of-scope | future: double/int predicate over Value kinds |
| `neg-int?` | out-of-scope | future: double/int predicate over Value kinds |
| `numerator` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `parse-boolean` | out-of-scope | future: pure string parse |
| `parse-double` | out-of-scope | future: pure string parse |
| `parse-long` | out-of-scope | future: pure string parse |
| `partitionv` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `partitionv-all` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `pos-int?` | out-of-scope | future: double/int predicate over Value kinds |
| `printf` | out-of-scope | future: printer/output one-liner over format/pr/with-out-str |
| `println-str` | out-of-scope | future: printer/output one-liner over format/pr/with-out-str |
| `prn-str` | out-of-scope | future: printer/output one-liner over format/pr/with-out-str |
| `rand-nth` | out-of-scope | future: RNG-backed pick over rand/shuffle |
| `ratio?` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `rational?` | out-of-scope | future: tower (BigInt/BigDec/Ratio) coercion/predicate/accessor |
| `reduce-kv` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `requiring-resolve` | out-of-scope | future: loader/var introspection over existing ns machinery |
| `reset-vals!` | out-of-scope | future: transient/atom gap-fill |
| `run!` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `set-validator!` | out-of-scope | future: IRef validator write ([iref validator-fn]) |
| `some->` | out-of-scope | future: core.clj control macro like implemented cond->/when |
| `some->>` | out-of-scope | future: core.clj control macro like implemented cond->/when |
| `special-symbol?` | out-of-scope | future: trivial const/predicate/lookup over existing kinds |
| `splitv-at` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `swap-vals!` | out-of-scope | future: transient/atom gap-fill |
| `test` | out-of-scope | future: run :test metadata fn |
| `tree-seq` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `update-keys` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `update-vals` | out-of-scope | future: pure seq/coll walk over existing machinery |
| `when-first` | out-of-scope | future: core.clj control macro like implemented cond->/when |
| `while` | out-of-scope | future: core.clj control macro like implemented cond->/when |

### Previously unlisted — no slice yet

No slice on this branch: hard-never reasons stand alone, design-blocked rows name their seam.

| var | status | notes |
| --- | --- | --- |
| `*'` | out-of-scope | pending design: auto-promote arith overflow-promotion spec; see #40 |
| `*compile-files*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*compile-path*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*compiler-options*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*data-readers*` | out-of-scope | pending design: data-reader tag table; see #40 |
| `*default-data-reader-fn*` | out-of-scope | pending design: data-reader tag table; see #40 |
| `*flush-on-newline*` | out-of-scope | pending design: printer newline-flush policy; see #40 |
| `*fn-loader*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*math-context*` | out-of-scope | pending design: BigDec precision context; see #40 |
| `*read-eval*` | out-of-scope | pending design: read/eval gate trio; see #40 |
| `*reader-resolver*` | out-of-scope | pending design: reader plug-points/flags; see #40 |
| `*repl*` | out-of-scope | pending design: *repl* root + session wiring; see #40 |
| `*source-path*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*suppress-read*` | out-of-scope | pending design: reader plug-points/flags; see #40 |
| `*use-context-classloader*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `*verbose-defrecords*` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `+'` | out-of-scope | pending design: auto-promote arith overflow-promotion spec; see #40 |
| `-'` | out-of-scope | pending design: auto-promote arith overflow-promotion spec; see #40 |
| `->ArrayChunk` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `->Eduction` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `->Vec` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `->VecNode` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `->VecSeq` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `-cache-protocol-fn` | out-of-scope | No protocol slice on this branch (protocol track owns design) |
| `-reset-methods` | out-of-scope | No protocol slice on this branch (protocol track owns design) |
| `EMPTY-NODE` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `Inst` | out-of-scope | pending design: date/time value kind + #inst tag; see #40 |
| `PrintWriter-on` | out-of-scope | java.io.PrintWriter adapter ctor; port Writer kind is separate |
| `StackTraceElement->vec` | out-of-scope | pending design: error-data shape; see #40 |
| `Throwable->map` | out-of-scope | pending design: error-data shape; see #40 |
| `accessor` | out-of-scope | pending design: legacy structmaps as pure maps; see #40 |
| `aclone` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `add-classpath` | out-of-scope | Deprecated on JVM, no replacement named; classpath URLs have no slice |
| `add-tap` | implemented | cellstaps.rs tap_returns + tap_deliver (conj; lazily starts the clojure.core/tap-loop dispatcher; single 1024 FIFO; per-tap catch) |
| `agent-errors` | out-of-scope | Deprecated on JVM; replacement implemented |
| `aget` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `alength` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `amap` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `ancestors` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `areduce` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-boolean` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-byte` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-char` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-double` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-float` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-int` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-long` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `aset-short` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `await1` | out-of-scope | Public [a] await returning the agent; implemented await is [& agents] returning nil |
| `bean` | out-of-scope | Host reflection/interop/beans/method-fns (N1) |
| `bit-and` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-and-not` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-clear` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-flip` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-not` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-or` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-set` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-shift-left` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-shift-right` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-test` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `bit-xor` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `boolean-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `booleans` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `byte-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `bytes` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `bytes?` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `cast` | out-of-scope | Host reflection/interop/beans/method-fns (N1) |
| `char-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `char-escape-string` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `char-name-string` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `chars` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `chunk` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-append` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-buffer` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-cons` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-first` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-next` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunk-rest` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `chunked-seq?` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `class?` | out-of-scope | Host reflection/interop/beans/method-fns (N1) |
| `clear-agent-errors` | out-of-scope | Deprecated on JVM; replacement implemented |
| `comparator` | implemented | cellstaps.rs comparator_calls + comparator_sort + comparator_errors (verbatim core.clj body; cond keeps truthy preds ordering; sort/sorted-map-by interop) |
| `compile` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `construct-proxy` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `create-struct` | out-of-scope | pending design: legacy structmaps as pure maps; see #40 |
| `dec'` | out-of-scope | pending design: auto-promote arith overflow-promotion spec; see #40 |
| `default-data-readers` | out-of-scope | pending design: data-reader tag table; see #40 |
| `definline` | out-of-scope | pending design: inline-expansion hook; see #40 |
| `defstruct` | out-of-scope | pending design: legacy structmaps as pure maps; see #40 |
| `derive` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `descendants` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `destructure` | out-of-scope | pending design: macro-expansion helper surface; see #40 |
| `double-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `doubles` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `enumeration-seq` | out-of-scope | JDK collection/stream adapters (N1) |
| `extenders` | out-of-scope | pending design: protocol surface (protocol track owns design); see #40 |
| `file-seq` | out-of-scope | pending design: lazy dir walk; see #40 |
| `find-protocol-impl` | out-of-scope | No protocol slice on this branch (protocol track owns design) |
| `find-protocol-method` | out-of-scope | No protocol slice on this branch (protocol track owns design) |
| `float-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `floats` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `get-proxy-class` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `hash-combine` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `hash-ordered-coll` | implemented | cellstaps.rs hash_ordered_values + hash_ordered_errors (Murmur3 ordered mix; maps iterate entries; nil NPE + non-Iterable CCE) |
| `hash-unordered-coll` | implemented | cellstaps.rs hash_unordered_values (Murmur3 unordered mix; vec-order-insensitive; entry = ordered [k v]; nil NPE) |
| `init-proxy` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `inst-ms` | out-of-scope | pending design: date/time value kind + #inst tag; see #40 |
| `inst-ms*` | out-of-scope | pending design: date/time value kind + #inst tag; see #40 |
| `inst?` | out-of-scope | pending design: date/time value kind + #inst tag; see #40 |
| `int-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `into-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `ints` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `io!` | out-of-scope | STM refs/transactions (N4): never |
| `isa?` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `iteration` | out-of-scope | pending design: step-fn seqable+reducible; see #40 |
| `iterator-seq` | out-of-scope | JDK collection/stream adapters (N1) |
| `load-reader` | out-of-scope | pending design: read/eval gate trio; see #40 |
| `long-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `longs` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `make-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `make-hierarchy` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `memfn` | out-of-scope | Host reflection/interop/beans/method-fns (N1) |
| `method-sig` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `mix-collection-hash` | out-of-scope | Chunked-seq + custom-type hashing impl details; port seqs/hash are separate |
| `munge` | out-of-scope | pending design: JVM munger escaping adoption; see #40 |
| `namespace-munge` | out-of-scope | pending design: JVM munger escaping adoption; see #40 |
| `object-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `parents` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `parse-uuid` | out-of-scope | pending design: UUID value kind; see #40 |
| `prefers` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `primitives-classnames` | out-of-scope | Bytecode compiler/classpath surface; port AOT (clj-aot) is separate |
| `print-ctor` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `print-dup` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `print-method` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `print-simple` | out-of-scope | pending design: print dispatch (defmultis + tables); see #40 |
| `proxy-call-with-super` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `proxy-mappings` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `proxy-name` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `proxy-super` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `random-uuid` | out-of-scope | pending design: UUID value kind; see #40 |
| `re-groups` | out-of-scope | pending design: stateful matcher handle over regex crate; see #40 |
| `re-matcher` | out-of-scope | pending design: stateful matcher handle over regex crate; see #40 |
| `read+string` | out-of-scope | pending design: read/eval gate trio; see #40 |
| `reader-conditional` | out-of-scope | pending design: data-reader tag table; see #40 |
| `reader-conditional?` | out-of-scope | pending design: data-reader tag table; see #40 |
| `ref-history-count` | out-of-scope | STM refs/transactions (N4): never |
| `ref-max-history` | out-of-scope | STM refs/transactions (N4): never |
| `ref-min-history` | out-of-scope | STM refs/transactions (N4): never |
| `release-pending-sends` | out-of-scope | pending design: nested-action queue; see #40 |
| `remove-all-methods` | out-of-scope | pending design: multimethod dispatch seam ([multifn]; coordinate with 33-typechain); see #40 |
| `remove-tap` | implemented | cellstaps.rs tap_returns + tap_set_semantics (disj by =; never starts the loop) |
| `replicate` | out-of-scope | Deprecated on JVM; replacement implemented |
| `resultset-seq` | out-of-scope | JDBC ResultSet adapter; never |
| `selector` | out-of-scope | pending design: 1.13 map-destructuring selector macro; see #40 |
| `seq-to-map-for-destructuring` | out-of-scope | pending design: macro-expansion helper surface; see #40 |
| `seque` | out-of-scope | pending design: queued (blocking) seq; see #40 |
| `set-agent-send-executor!` | implemented | cellstaps.rs exec_record_send (accept-and-record: any value stored + returned; dispatch stays on the fixed send pool — pinned divergences from e-routes/e-nil-send/e-non-exec) |
| `set-agent-send-off-executor!` | implemented | cellstaps.rs exec_record_send_off (accept-and-record mirror of the send slot; fixed send-off pool) |
| `short-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `shorts` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `stream-into!` | out-of-scope | JDK collection/stream adapters (N1) |
| `stream-reduce!` | out-of-scope | JDK collection/stream adapters (N1) |
| `stream-seq!` | out-of-scope | JDK collection/stream adapters (N1) |
| `stream-transduce!` | out-of-scope | JDK collection/stream adapters (N1) |
| `struct` | out-of-scope | pending design: legacy structmaps as pure maps; see #40 |
| `struct-map` | out-of-scope | pending design: legacy structmaps as pure maps; see #40 |
| `sync` | out-of-scope | STM refs/transactions (N4): never |
| `tagged-literal` | out-of-scope | pending design: data-reader tag table; see #40 |
| `tagged-literal?` | out-of-scope | pending design: data-reader tag table; see #40 |
| `tap->` | implemented | cellstaps.rs tap_thread_first (verbatim core.clj one-liner: returns x and delivers it) |
| `tap>` | implemented | cellstaps.rs tap_returns + tap_deliver + tap_nil + tap_full (non-blocking offer; true with room, false past 1024) |
| `to-array` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `to-array-2d` | out-of-scope | Java array ctor/cast/read/write; no array values (N1) |
| `unchecked-add` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-add-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-byte` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-char` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-dec` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-dec-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-divide-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-double` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-float` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-inc` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-inc-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-long` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-multiply` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-multiply-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-negate` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-negate-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-remainder-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-short` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-subtract` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `unchecked-subtract-int` | out-of-scope | pending design: wrapping vs tower-checking per fn; see #40 |
| `underive` | out-of-scope | pending design: hierarchy maps (coordinate with 33-typechain); see #40 |
| `unquote` | out-of-scope | pending design: syntax-quote support surface; see #40 |
| `unquote-splicing` | out-of-scope | pending design: syntax-quote support surface; see #40 |
| `unsigned-bit-shift-right` | out-of-scope | pending design: bitwise lattice over tower; see #40 |
| `update-proxy` | out-of-scope | No type-system slice on this branch: proxies/deftype factories/method internals |
| `uri?` | out-of-scope | pending design: URI value kind; see #40 |
| `uuid?` | out-of-scope | pending design: UUID value kind; see #40 |
| `vector-of` | out-of-scope | pending design: primitive-vector kind; see #40 |
| `volatile!` | implemented | cellstaps.rs volatile_ctor_print (fresh Volatile cell; transient #object print — pinned divergence from the JVM {:status :ready} rendering) |
| `volatile?` | implemented | cellstaps.rs volatile_pred (true on Volatile cells only) |
| `vreset!` | implemented | cellstaps.rs volatile_vreset + volatile_vswap_shape (stores + returns newval; nil NPE + non-volatile CCE; the vswap! checkcast gate) |
| `vswap!` | implemented | cellstaps.rs volatile_vswap + volatile_vswap_shape (builtin macro: (vreset! v (f (deref v) …)); deliberate double-eval of vol) |
| `with-loading-context` | out-of-scope | Load-context macro inlined by port load (*pending-paths* guard implemented) |
| `with-local-vars` | out-of-scope | pending design: var-frame macro over binding frames; see #40 |
| `with-precision` | out-of-scope | pending design: BigDec precision context; see #40 |
| `xml-seq` | out-of-scope | XML element walk; XML has no slice |
