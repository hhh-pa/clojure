# Universe audit report (durable copy; original /tmp paths below now live in this directory)

# Universe audit for #40 (F-read-opts) — report

## 1. Counts (all corroborated)

| Source | Count | How verified |
|---|---|---|
| JVM `clojure.core` publics (alpha10 jars, `clojure-version` = 1.13.0-alpha10) | **685** | `ns-publics` print to [/tmp/i40-nspublics.txt](/tmp/i40-nspublics.txt); corroborated by `(count (ns-publics …))` = 685, 685 distinct non-empty lines, 0 dupes; `(count (ns-interns …))` = 808 |
| Rows in [rust/core-coverage.md](/home/hhu/workspace/clojure/rust/core-coverage.md) | **422** | `^\| \`name\` \|` parse to [/tmp/i40-universe.txt](/tmp/i40-universe.txt); header claims 385/2/35/0 — re-verified exact (incl. 2 divergent: `ns-imports`, `*print-dup*`) |
| In JVM but NOT in coverage (missing) | **270** | [/tmp/i40-missing.txt](/tmp/i40-missing.txt) |
| In coverage but NOT in JVM (stale) | **7** | [/tmp/i40-stale.txt](/tmp/i40-stale.txt) |
| Missing vars that already resolve in booted port core | **5** | scratch probe in /tmp (same `resolve` logic as `coverage_matrix.rs`), `CARGO_TARGET_DIR=/tmp`; controls pass (`map`/`+`/`eval` yes; `load-reader`/`*read-eval*`/`bit-and`/`derive`/`run!` no) |

Native JVM metadata (added-version, kind, docstring lead) for all 270 is in [/tmp/i40-meta.tsv](/tmp/i40-meta.tsv); resolve results in [/tmp/i40-resolves.tsv](/tmp/i40-resolves.tsv); expanded per-var triage in [/tmp/i40-triage.tsv](/tmp/i40-triage.tsv) (script-verified: exactly the 270, no dupes, no gaps).

## 2. Derivation of the 422 — story: hand-curated, no generator (unknown rationale)

- `git log --follow` shows **2 commits**: created in `63b2aa54` (squash-import, 737 files) with 422 rows (`380/2/40`), then `72992dcd` flipped 5 rows to implemented (`385/2/35`). Row count never changed; no prior history (import is the root for this path).
- **No generator exists**: `scripts/ci/go/cmd/coverage-{floor,patch,summary}` are LCOV gates (none references `core-coverage.md`); no script in `scripts/`, `docs/`, `rust/` mentions `ns-publics` or universe generation. [S5 plan](/home/hhu/workspace/clojure/docs/t1174/s5-plan.md) §S5D lists categories by hand ("Rust-backed: arithmetic, comparison, …"), consistent with a hand-picked list.
- Only in-tree mention of the gap: [i36 spec](/home/hhu/workspace/clojure/docs/i36-instream/specification.md) ll.75–76 already flags "`read+string`, `*read-eval*`, `load-reader`: absent from the 422-row coverage universe — flagged as follow-up". So #40's trio was a known symptom; this audit shows the gap is 270 wide, including 1.0-era vars (`while`, `run!`, `bit-and`, `update-vals`, `requiring-resolve`, …), not just new-in-1.13 ones.
- Note: the file header says the port reports `1.13.0-alpha9` while the audit oracle is alpha10; only a handful of missing vars are 1.12/1.13-new (`merge-deep`, `partitionv`, `splitv-at`, `some-vals`, `selector`, `tap->`, stream-`*`), so version skew explains <5% of the gap.

## 3. Stale list (7) — all deliberate, keep all rows

| Var | Coverage status | JVM truth | Verdict |
|---|---|---|---|
| `*loaded-libs*` | implemented | interned, **private** | keep — port exposes the private for its loader |
| `*loading-verbosely*` | implemented | interned, **private** | keep — read by port `load` |
| `*pending-paths*` | implemented | interned, **private** | keep — port cycle guard |
| `preserving-reduced` | implemented | interned, **private** | keep — port transducer helper |
| `*load-path*` | implemented | **absent** (removed upstream) | keep — port-only design (dirs vector) |
| `clone-thread-bindings` | out-of-scope | **absent** | keep — guard row, already documented (`clone-missing`) |
| `pprint` | out-of-scope | **absent** (it's `clojure.pprint`) | keep — guard row, already documented |

Port resolve check agrees: the 5 implemented ones resolve, the 2 guards don't. Suggest only a one-line file-intro tweak ("plus port-visible privates and guard rows") when the universe is repaired — not done here (read-only).

## 4. Complete missing list (270, every name)

```text
*'  *agent*  *compile-files*  *compile-path*  *compiler-options*
*data-readers*  *default-data-reader-fn*  *flush-on-newline*  *fn-loader*
*math-context*  *read-eval*  *reader-resolver*  *repl*  *source-path*
*suppress-read*  *use-context-classloader*  *verbose-defrecords*  +'
-'  ->ArrayChunk  ->Eduction  ->Vec  ->VecNode  ->VecSeq
-cache-protocol-fn  -reset-methods  EMPTY-NODE  Inst  NaN?  PrintWriter-on
StackTraceElement->vec  Throwable->map  abs  accessor  aclone  add-classpath
add-tap  agent-errors  aget  alength  amap  ancestors  any?  areduce  aset
aset-boolean  aset-byte  aset-char  aset-double  aset-float  aset-int
aset-long  aset-short  await1  bean  biginteger  bit-and  bit-and-not
bit-clear  bit-flip  bit-not  bit-or  bit-set  bit-shift-left  bit-shift-right
bit-test  bit-xor  boolean-array  booleans  bound-fn*  bounded-count
byte-array  bytes  bytes?  cast  char-array  char-escape-string
char-name-string  chars  chunk  chunk-append  chunk-buffer  chunk-cons
chunk-first  chunk-next  chunk-rest  chunked-seq?  class?  clear-agent-errors
comparator  compile  construct-proxy  create-struct  dec'  decimal?
default-data-readers  definline  defprotocol  defstruct  delay?  denominator
derive  descendants  destructure  disj!  double-array  doubles
enumeration-seq  error-handler  error-mode  extenders  ffirst  file-seq
find-keyword  find-protocol-impl  find-protocol-method  find-var  flatten
float-array  floats  future-cancelled?  get-proxy-class  get-validator
hash-combine  hash-ordered-coll  hash-unordered-coll  inc'  indexed?
infinite?  init-proxy  inst-ms  inst-ms*  inst?  int-array  into-array  ints
io!  isa?  iteration  iterator-seq  load-reader  long-array  longs
make-array  make-hierarchy  map-entry?  memfn  merge-deep  merge-deep-with
method-sig  mix-collection-hash  munge  namespace-munge  nat-int?  neg-int?
numerator  object-array  parents  parse-boolean  parse-double  parse-long
parse-uuid  partitionv  partitionv-all  pos-int?  prefers
primitives-classnames  print-ctor  print-dup  print-method  print-simple
printf  println-str  prn-str  proxy-call-with-super  proxy-mappings
proxy-name  proxy-super  rand-nth  random-uuid  ratio?  rational?  re-groups
re-matcher  read+string  reader-conditional  reader-conditional?  reduce-kv
ref-history-count  ref-max-history  ref-min-history  release-pending-sends
remove-all-methods  remove-tap  replicate  requiring-resolve  reset-vals!
resultset-seq  run!  selector  seq-to-map-for-destructuring  seque
set-agent-send-executor!  set-agent-send-off-executor!  set-validator!
short-array  shorts  some->  some->>  some-vals  special-symbol?  splitv-at
stream-into!  stream-reduce!  stream-seq!  stream-transduce!  struct
struct-map  swap-vals!  sync  tagged-literal  tagged-literal?  tap->  tap>
test  to-array  to-array-2d  tree-seq  unchecked-add  unchecked-add-int
unchecked-byte  unchecked-char  unchecked-dec  unchecked-dec-int
unchecked-divide-int  unchecked-double  unchecked-float  unchecked-inc
unchecked-inc-int  unchecked-int  unchecked-long  unchecked-multiply
unchecked-multiply-int  unchecked-negate  unchecked-negate-int
unchecked-remainder-int  unchecked-short  unchecked-subtract
unchecked-subtract-int  underive  unquote  unquote-splicing
unsigned-bit-shift-right  update-keys  update-proxy  update-vals  uri?  uuid?
vector-of  volatile!  volatile?  vreset!  vswap!  when-first  while
with-loading-context  with-local-vars  with-precision  with-redefs-fn  xml-seq
```

## 5. Triage (270 = 5 implemented + 53 planned + 100 out-of-scope + 112 needs-design; N/A proposed for none — the file's own contract says every var gets a row, so deprecated/edge cases go out-of-scope with a reason)

**implemented — add rows, code resolves today (5):** `*agent*` (dynamic root, bound around actions in `concurrency.rs`), `bound-fn*`, `inc'`, `some-vals` (also emitted by `destructure_expand.rs`), `with-redefs-fn` (all three in `FX4_NATIVES`/`CORE_NATIVES`; `bound-fn`/`with-redefs` rows already name their helpers). The matrix test can't see them (it only iterates listed rows) — pure file bugs.

**planned — small/clear over existing machinery (53):** predicates `NaN? infinite? nat-int? neg-int? pos-int? any? delay? indexed? map-entry? special-symbol? decimal? ratio? rational?`; tower `abs biginteger denominator numerator`; parses `parse-boolean parse-double parse-long`; lookups `find-keyword find-var requiring-resolve`; agent/atom field fns `error-handler error-mode future-cancelled? get-validator set-validator! disj! reset-vals! swap-vals!`; pure seq/coll `bounded-count ffirst flatten merge-deep merge-deep-with partitionv partitionv-all reduce-kv run! splitv-at tree-seq update-keys update-vals`; printer one-liners `printf println-str prn-str`; `rand-nth`; core.clj macros `some-> some->> when-first while` (note: AOT already lists `while` under "macros are not expanded", so AOT stays red on it); `test`.

**out-of-scope — with reasons (100):** Java arrays, all 37 (`aclone aget alength amap areduce aset` + 8 `aset-*`, `boolean-array booleans byte-array bytes bytes? char-array chars double-array doubles float-array floats int-array into-array ints long-array longs make-array object-array short-array shorts to-array to-array-2d`) — no array values (N1); interop `bean cast class? memfn`; JDK adapters `enumeration-seq iterator-seq stream-into! stream-reduce! stream-seq! stream-transduce!`; `resultset-seq` (JDBC, never); `xml-seq` (no XML slice); STM `io! ref-history-count ref-max-history ref-min-history sync` (N4, never); deprecated `add-classpath agent-errors clear-agent-errors replicate`; compiler/classpath `*compile-files* *compile-path* *compiler-options* *fn-loader* *source-path* *use-context-classloader* *verbose-defrecords* `compile primitives-classnames`; no type-system slice `->ArrayChunk ->Eduction ->Vec ->VecNode ->VecSeq EMPTY-NODE construct-proxy get-proxy-class init-proxy method-sig proxy-call-with-super proxy-mappings proxy-name proxy-super update-proxy`; no protocol slice `-cache-protocol-fn -reset-methods find-protocol-impl find-protocol-method`; chunk/hash internals `chunk chunk-append chunk-buffer chunk-cons chunk-first chunk-next chunk-rest chunked-seq? hash-combine mix-collection-hash`; `await1` (covered by `await`); `set-agent-send-executor! set-agent-send-off-executor!` (fixed pools); `PrintWriter-on` (port Writer is separate); `with-loading-context` (inlined by port `load`). Note: `to-array` is *emitted* by `destructure_expand.rs` but intercepted by `fx4_host` (`HostOp::Pam`) — the row should record that the var itself must not resolve.

**needs-design — grouped (112):** #40 core `*read-eval* load-reader read+string`; #40-adjacent tag table `*data-readers* *default-data-reader-fn* default-data-readers tagged-literal tagged-literal? reader-conditional reader-conditional?`; reader plug-points `*reader-resolver* *suppress-read*` (pin JVM role from oracle first); auto-promote arith `*' +' -' dec'` (wedge stays per S-rng-tower); wrapping arith, all 21 `unchecked-*`; bit lattice, all 12 `bit-* + unsigned-bit-shift-right`; precision `*math-context* with-precision`; hierarchy/multimethod `ancestors derive descendants isa? make-hierarchy parents prefers remove-all-methods underive`; protocols `defprotocol extenders`; print dispatch `char-escape-string char-name-string print-ctor print-dup print-method print-simple *flush-on-newline*`; structs `accessor create-struct defstruct struct struct-map`; expander helpers `destructure seq-to-map-for-destructuring selector unquote unquote-splicing munge namespace-munge definline`; hashing `hash-ordered-coll hash-unordered-coll`; errors `StackTraceElement->vec Throwable->map`; temporal `Inst inst-ms inst-ms* inst?`; identity `parse-uuid random-uuid uuid? uri?`; coll kinds `comparator vector-of`; seqs `file-seq iteration seque`; stateful regex `re-groups re-matcher`; concurrency `tap> tap-> add-tap remove-tap release-pending-sends volatile! volatile? vreset! vswap!`; misc `*repl* with-local-vars`. Multimethod/protocol-adjacent items need coordination with the `33-typechain` track (its commits are on `origin/33-typechain`, not in HEAD — typechain paths untouched per rules).

**Where the #40 trio lands:** all three `needs-design`, owned by #40 itself — `*read-eval*` (eval-reader gate; port `read` currently denies read-cond and has no `#=` eval form), `read+string` (needs pushback-reader span equivalence; port `Reader` span tracking exists), `load-reader` (streaming eval-from-reader vs port's path-based `load`/`load-file`/`load-string`).

## 6. Complete / unresolved

- **Complete:** authoritative 685 universe extracted; 422-row parse verified (header counts exact); both diffs exhaustive; 270/270 triaged with JVM added-version/kind/doc lead each; 5 silent resolvers probe-proven; 7 stale rows explained (JVM-private ×4, port-only ×1, guards ×2); derivation hunted (hand-curated at squash-import, no generator); repo untouched (`git status` clean, all builds/scratch in /tmp).
- **Unresolved / needs owner decisions:** (a) accept/reject each `planned` vs `needs-design` split — I used "trivially clear" as the bar; (b) protocol/multimethod/hierarchy/print-dispatch rows depend on the `33-typechain` track landing and flipping existing out-of-scope rows; (c) no N/A status proposed — if maintainers want fewer rows, that needs a contract change to the file's "unlisted = file bug" rule; (d) `*repl*`/`with-local-vars`/`definline`/`vector-of`/`uri?` sit on genuine product seams (REPL wiring, evaluator-vs-compiler identity, value-kind inventory).