# S5D differential corpus MANIFEST

20 files, `NN-topic.clj` + `expected/NN-topic.out` (golden stdout).
Truth source: `s5d-pins.md`. Probe inputs live in `/tmp/s5d-gaps*.clj`
(not committed); corpus files are the durable, runnable record.

## Oracle

From the repo root (`/Users/hhu/workspace/clojure`):

```
CP="target/clojure-1.13.0-master-SNAPSHOT.jar:$HOME/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:$HOME/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar:$HOME/.m2/repository/org/clojure/test.check/1.1.3/test.check-1.1.3.jar"
java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
  -cp "$CP" clojure.main docs/t1174/s5d-corpus/NN-topic.clj \
  > docs/t1174/s5d-corpus/expected/NN-topic.out 2>&1
```

Flags are load-bearing: the bare machine oracle drifts (locale en_FR,
zone Europe/Paris). Regenerating one file must diff to zero unless its
`.clj` changed (staleness check: `.clj -nt expected/*.out`, plus the
tag-set check documented below).

## Entry format (all files)

- `(p ":tag" #(form))` prints `:tag  => <prn of value>`; a throw prints
  `:tag  => "THROW <class>: <message>"` (outermost only).
- `(cls ":c-tag" #(x))` prints `[class seq? counted? list?
  sequential? coll?]` (files 05, 06, 08, 10, 16–19).
- `(pe ":tag" #(x))` prints the FULL cause chain with file paths
  normalized to `(<file>)` (file 15 only).
- Tag prefixes: `:a-*` = 0-arity via `(eval (list 'f))`; `:c-*` =
  class row; otherwise kebab-case topic tags.
- Every file ends with `(println "S5D-NN DONE")`; the runner diffs
  normalized stdout against `expected/` (see Determinism).

## Determinism rules

From `s5d-pins.md` (Corpus rules) plus established practice:

- `LazySeq@<hex>` lines need NO normalization: the hex is the Java
  List CONTENT hash (deterministic across runs), not an identity
  hash. Verified byte-identical on re-run (19).
- Set/big-map order normalized inside entries (`[(set? s) (sort s)]`,
  `sort-by pr-str`); small array-maps keep insertion order.
- Wall clock shape-checked only (`:time-shape`); `%t` is UTC-pinned;
  grouping (`%,d`) is en_US.
- `expected/14-*.out` is NOT valid UTF-8 by design (`:c-surrogate`
  prints raw lone-surrogate bytes; diff binary-safe).
- 01/02 print multi-line predicate tables, so their `.out` line count
  exceeds entries+1; every other file is one line per entry + DONE.
- Staleness audit 2026-10-19: 13 was missing its last entry
  (`:co-1st-dash-s`, added without regen) — regenerated, diff was
  exactly the one line. 15 was missing its `.out` — generated.
  Tag-set check: entry tags in each `.clj` match `^:tag` lines in its
  `.out` (01/02 excepted: table rows).

## Files

- `01-preds.clj` (49): predicates, scalar edges, class names.
  Sources: pins Predicates/classes, 7b, 7d (distinct?), 7g4, probe39.
  Prints predicate matrices (multi-line tables).
- `02-seqs-lazy.clj` (162): seqs, lazy semantics, seq lib
  (map/filter/reduce/...). Sources: pins, 7e, 7g/7g2/7g3,
  7g9/7g10, 7g4, gaps–gaps4, probe39. Tables; transducer fns print
  as `[(fn? t) (class t)]`, never raw.
- `03-ranges-repeat.clj` (111): ranges, repeat, repeatedly.
  Sources: pins, 7g9/7g10/7g11, 7g3, 7g4, 7g5/7g6, 7g14c, 7e,
  gaps–gaps6, probe39. Infinite/step-0/NaN always take-bounded.
- `04-cycle-iterate-cons.clj` (49): cycle, iterate, cons, concat,
  interleave, interpose. Sources: pins, 7g9/7g10/7g11, 7e, 7g4,
  7g5, gaps–gaps3, probe39. Infinites take-bounded.
- `05-take-last-butlast-drop.clj` (94): take-last, butlast,
  drop-last, drop (IDrop/eager matrix). Sources: pins,
  7g9/7g10, 7g5/7g6, 7e, 7g4, gaps/gaps3/gaps4, probe39.
- `06-take-nth-partitions.clj` (67): take-nth, partition,
  partition-all, partition-by. Sources: pins, 7g9/7g5/7g14c/7g4,
  7e/7g4/7g5/7g6, gaps–gaps6, probe39. Degenerate n/step
  take-bounded. Step-strictness/MAX-step cells live in 18.
- `07-conj-assoc-dissoc.clj` (246): conj/into/assoc/dissoc/merge/
  update/select-keys/zipmap/replace/ctors. Sources: pins,
  7c, 7d/7d2, 7g/7g2/7g3, 7g4, 7g5/7g6, 7g11, 7g14c,
  gaps–gaps4, probe39. Sets order-normalized.
- `08-map-classes.clj` (64): array-map vs hash-map rules.
  Sources: pins Map classes, 7g12/7g13/7g14(fixed)/7g14b/7g14c.
- `09-nth-get-find.clj` (288): nth/get/contains?/find/keys/vals/
  count/empty/peek/pop/subvec/invocation. Persistent-only:
  transient-involved access lives in 17. Sources: pins,
  7c, 7g/7g2, 7g3/7g4, 7g5/7g6, 7g10–7g14c, gaps–gaps6, probe39.
  Sets order-normalized. `doall` does NOT realize KeySeq (see
  `:keys-vec-doall-lazy`); 7g/7c dup forms documented in-file.
- `10-sort.clj` (74): sort, sort-by, reverse, compare, max-key,
  min-key. Sources: pins, 7e, 7g4, 7g5, 7g12, gaps2/3/4,
  s5d-gap/gap2, probe39.
- `11-str-print.clj` (139): str/print, print vars, with-out-str,
  subs, name/namespace, symbol/keyword. Sources: pins, 7f, 7g4,
  7g5, 7d/7d2/7g3, gaps3/4/6, s5d-gap/gap2, probe39.
  KNOWN DIVERGENCES: print-interleave order; negative/fractional/
  NaN `*print-length*` hangs the port. NOTE: pins claimed
  `*print-namespace-maps*` root FALSE; oracle shows TRUE
  (re-verified 2026-10-18; pins fixed).
- `12-format-spec.clj` (130): format success specs. Sources: pins,
  7f/7h/7h2, FmtProbe/2, s5d-gap, probe39. `%a/%t/%c` in 14,
  errors in 13. NOTE: header says `%h` lives in 18 — STALE, see
  Open items (`:fmt-s-lazy` pins `LazySeq@402`).
- `13-format-errors.clj` (84): format errors (flags/precision/
  width/index/missing). Sources: pins, 7f/7h/7h2, FmtProbe/2,
  s5d-gap, probe39. `%a/%t/%c` errors in 14.
- `14-format-time-hexfloat.clj` (67): `%a`, `%t` (UTC), `%c`.
  Sources: pins, FmtProbe/2, 7f/7h/7h2, /tmp/cq.clj. KNOWN
  DIVERGENCES: lone-surrogate `%c` unportable; `(int 65)` is Long
  in the port; `%t` UTC-only. `.out` is binary (surrogates).
- `15-control-macros.clj` (342): case/condp/cond/control
  macros/HOFs/assert/time/flags. Sources: pins, 7a/7af,
  7g–7g5, 7e, 7f, gaps–gaps6, s5d-gap/gap2, probe39. `pe`
  cause-chain entries; `defn/def/let/loop/fn/recur` shapes are
  NOT S5D scope. `.out` generated 2026-10-19 (was missing).
- `16-range-ctor-edges.clj` (37): range ctors/coercions (3-arg
  bad-type matrix, step-0/-0.0, count-overflow to double path,
  NaN matrix, NaN step), repeat edges (0/MAX/take-1 class).
  Sources: pins 7i, 7i, 7i4, 7i6. 03 holds the basics; only
  7i-new cells here.
- `17-drop-nth-coerce.clj` (94): drop huge/fractional/bad n
  (eager-AE/IAE vs silent walk-to-end, ceil rule), drop kind
  table, take-ceil contrast, nth big-index (IAE vs AE vs
  truncate), nthrest, subvec/subs huge index, transient-involved
  access (pointed here by 09). Sources: pins 7i, 7i–7i4, 7i6.
- `18-partitions-iterate.clj` (42): partition step strictness
  (lazy ctor, strict-nthrest first force, MAX-step vec-AE vs
  list-single-window), n-0/pad degenerates, iterate ctor errors
  and f-call counters, cons/cycle/interleave edges. Sources:
  pins 7i, 7i, 7i5, 7i6. 04/06 hold the basics; only 7i-new
  cells here.
- `19-keys-tostring-eq.clj` (93): keys/vals nils/classes/
  unvalidated tails, with-meta/next (StringSeq-only propagation),
  take-last/drop-last news, str/tostring table, str-of-lazy
  (`@content-hash`, realizes-but-never-prints), seq-self,
  hash (infinite-UOE vs finite), `=`/equiv cross-type rules and
  counter-verified early exit. Sources: pins 7i, 7i2–7i4, 7i6.
  05/09/11 hold the basics; only 7i-new cells here.
- `20-print-dup.clj` (2): `*print-dup*` atom witness (S9B).
  Sources: s9-plan S9B, s5d-contract divergence. Test:
  `print_dup_flag_ignored_for_atoms` (concurrency.rs).
  KNOWN DIVERGENCE: the golden's `:dup-atom-true` THROWs
  IllegalArgumentException on the JVM; the port prints `true`
  (flag ignored — see the test).

## Open items

- `12-format-spec.clj` header promises "`%h` lives in 18", but 18
  is partitions/iterate per the 11–19 plan. `%h` (Java `.hashCode`
  hex incl. seq realization) still needs a home — suggested: a new
  `20-format-hash.clj`, plus a one-line fix to 12's header.
- `defn/def/let/loop/fn/recur` shapes are out of S5D scope (15).
- `*print-length*` negative/fractional/NaN and print interleave
  order are known port divergences (11).
