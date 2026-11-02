# Recipes drop-in: all `recipes/clojure` tests green under `clojure-rs` (spec)

Parent: [#201](https://github.com/hhh-pa/clojure/issues/201). Goal: `clojure-rs run
<script.clj>` replaces `java -cp clojure.jar clojure.main <script.clj>` for all
394 recipes scripts at snippet-HEAD (stdin in, stdout out, byte-identical
after the harness's per-line trim; 1754 shared cases). Upstream is a text
oracle only; no JDK/JVM anywhere.

§1–§5 are the original S1–S12 spec (stale-base era, kept verbatim); §6
records the HEAD-pivot follow-ups (each gap found by running the full
1754-case suite on `workstation.hhh.pa`, each fixed in `clojure-rs` — the
snippet repo stays pristine).

## 1. Problem

Probes against the built binary (all executed, §4) show a solid core — doubles,
`format`, transients, sorting, regex, atoms, ex-info all match the JVM — with
eleven gaps. Corpus shape: zero interop, zero deftype/macros/multimethods in
scripts; only stdlib needs are `clojure.string` (33 files) and `clojure.set`
(4 files); every stdin read is `(slurp *in*)` (189 calls).

Upstream `clojure/set.clj` already loads VERBATIM. Upstream
`clojure/string.clj` does not, for three load-time reasons plus missing
members on its called paths.

## 2. Solution (one slice, dependency order)

S1. `ns` `:import` clause. `parse_ns` (`expr.rs`) rejects `:import` (N1-era
    leftover); standalone `import` already works. Accept both shapes —
    package-list `(pkg Class*)` and bare symbols — into a new
    `NsClause::Import`, evaluated through the existing import machinery.
    `:gen-class`/`:load` stay spec errors.
S2. `clojure.lang.LazilyPersistentVector` class. Hierarchy row + `import`
    resolution + `createOwning` static. Our `Pattern/.split` returns a
    `Value::Vector` directly, so `createOwning` is pass-through on vectors
    (documented emulation note: no arrays exist to own).
S3. Driver `with-bindings` frame. Upstream evidence chain (all read as
    text): `RT.load` binds only CURRENT_NS/WARN/UNCHECKED_MATH around the
    class-load attempt; `Compiler.load` binds LOADER/SOURCE/.../UNCHECKED_MATH
    but NOT warn; `core.clj load` binds only `*pending-paths*`. The binding
    that makes upstream `string.clj:46` `(set! *warn-on-reflection* true)`
    legal comes from `clojure.main/with-bindings` ("vars that often need to
    be set!"), which wraps every script/eval/repl. So `cmd_run`/`cmd_eval`
    must push the same frame (`*ns* *warn-on-reflection* *math-context*
    *print-meta* *print-length* *print-level* *print-namespace-maps*
    *data-readers* *default-data-reader-fn* *compile-path* (root — no
    `System/getProperty` yet) `*command-line-args*` `*unchecked-math*`
    `*assert*` `*1 *2 *3 *e*`) around the script. All vars verified
    present. (Not the loader — `run_loaded` stays as is.) Follow-up:
    `eval`/`repl` drivers adopt the frame via a shared helper (needs
    public bindings API or Value surgery in the binary crate).
S4. String instance members (called paths only): `.length` (UTF-16 units),
    `.charAt`, `.subSequence`, `.startsWith`; `.toString` already works.
    `.toUpperCase/.toLowerCase/.split/.replace*` deferred (uncalled).
S5. `StringBuilder`: new value kind (growable buffer) + `(StringBuilder.
    String|int)` ctors + `.append` (CharSequence/char/Object via `str`) +
    `str`/`toString` renders content.
S6. `Character/isWhitespace` static (Java semantics: `Character.isWhitespace`
    code-point rules, not `char::is_whitespace` alone — verify the
    difference set).
S7. `Pattern/.split` (s) + (s, limit) with Java limit semantics (limit=0
    drops trailing empties; negative keeps all) over the existing regex
    engine.
S8. `(slurp *in*)`: `slurp` must accept a `Reader` (read fully), matching
    `clojure.java.io/reader` pass-through. THE top blocker.
S9. `conj!` variadic: `[coll x & xs]`, looping `args[1..]` (JVM parity).
S10. Distribution: embed upstream `clojure/string.clj` + `clojure/set.clj`
    (pinned vendored snapshot + sync note) via `include_str!` +
    `resolve_source` arms, following the spec/tools.deps precedent, so
    `require` works with zero install steps. Loader security posture
    unchanged (embedded first, then `*load-path*`).
S11. Harness (recipes repo, separate patch): `--runner` flag for
    `run_tests.py` (default stays `java ... clojure.main`).
S12. Number statics (found by the interop resurvey — 17 files):
    `Long/parseLong`, `Integer/parseInt`, `Double/parseDouble`
    (shared `parse_long_str`/`parse_double_str` bodies, NFE
    instead of nil), `Math/abs` (int wraps, else double),
    `Math/sqrt`, `Math/ceil` (double widening).

Out of scope: full `string.clj` member parity (replace/escape paths),
`Double/` statics (uncallable without interop), pprint/test/walk/zip.

## 3. BDD battery (CLI tests, `rust/tests/cli_recipes.rs` + fixtures)

Each: script (+ stdin) → exact stdout, exit 0. Mirrors the §4 probes.

- `b01_slurp_stdin`: `(println [:slurp (slurp *in*)])`, stdin `hello stdin`
  → `[:slurp "hello stdin"]`.
- `b02_ns_import_clause`: script with `(ns foo (:import
  (java.util.regex Pattern Matcher) java.lang.String))` → loads, `:ok`.
- `b03_set_warn_on_load`: `(set! *warn-on-reflection* true)` at top level
  → `:ok` (proves the loader binding).
- `b04_string_ns_loads`: `(require '[clojure.string :as str])` → `:ok`
  (embedded upstream source, S10).
- `b05_str_called_paths`: `str/blank? str/join str/split str/trim
  str/starts-with? str/split-lines` over a fixed battery incl. `\s+`,
  unicode whitespace, limit forms → exact JVM-matching strings.
- `b06_set_called_paths`: `set/union set/difference set/intersection` →
  exact sets.
- `b07_conj_bang_variadic`: `(conj! (transient [1]) 2 3)` → `[1 2 3]`.
- `b08_recipe_end_to_end`: two real recipes scripts (one `str/`-heavy sort,
  one float `%.6f` geometry) vendored as fixtures with fixed stdin →
  hand-verified outputs.

Final acceptance (user-run, needs the passphrase + JVM-free runner):
`run_tests.py --resources ... --runner 'clojure-rs run'` all green.

## 4. TDD units (repo layout, smallest per gap)

- S1: `ns_vars.rs` — both `:import` shapes; `:gen-class` still spec-error.
- S2/S7: `classemu.rs` + `re_rng.rs` — `createOwning` passthrough; split
  limit matrix (0/neg/pos, trailing empties, empty match).
- S3: driver-level (BDD b03 covers it): `cmd_run` pushes the
  `with-bindings` frame; `set!` on `*warn-on-reflection*` at script top
  level works. No `loader.rs` change.
- S4/S5/S6: `classemu.rs` — member matrix incl. UTF-16 `.length`,
  surrogate-pair `.charAt`, `isWhitespace` difference set vs Rust.
- S8: `io.rs`/`jdkio.rs` — slurp over Reader/StringReader/PushbackReader.
- S9: `coll.rs` — variadic conj!/assoc!/dissoc!/disj! incl. 0/1-arity.
- S10: `loader.rs` — `require` resolves embedded string/set with EMPTY
  `*load-path*`.

## 5. Risks

- `Character/isWhitespace` vs Rust `char::is_whitespace`: Java includes
  U+001C–U+001F, U+0085 etc.; enumerate the difference set in the test.
- Java `String/split`/`Pattern/split` edge semantics (empty regex, empty
  input, limit) — pin the matrix from upstream text, not memory.
- `str` on `StringBuilder`: upstream `(str sb)` calls `.toString` — our
  `str` must render the builder kind.
- Upstream `RT.load` binding set must be read from source text, not memory.

## 6. HEAD-pivot follow-ups (394 scripts, 1754 cases)

Each item: gap (failing cases) → fix → regression test. All verified
against the JVM oracle where semantics were in doubt.

- F1. `into` over sorted targets. `into` with a transducer threw CCE on
  sorted sets/maps (they are not `IEditableCollection`): route sorted
  targets through the plain-`conj` transduce path (`seqs.rs`,
  `into_xform_sorted_targets_201`).
- F2. `nth` over vectors/subvectors. Six `iter().nth(idx)` (O(n)) sites
  → `Vector::get` (O(log n)); 100x on the micro-bench, fixes the
  `disjoint_set` timeout (`coll.rs`, `eval.rs`, `ns.rs`, `seq.rs`).
- F3. Lazy-chain drop avalanche. Walking a 500K+ lazy chain with the
  head held (e.g. `nth`'s arg) overflowed the stack on cleanup: each
  forced cell's tail dropped recursively. `PendingSeq::drop` now
  unwinds sole-owned successor cells iteratively (new `as_any_mut`
  hook on `ExtensionValue`; `take_chain_drop_500k_201`).
- F4. `clojure.string` preload. JVM `clojure.main <script>` loads
  `clojure.main` → `clojure.spec.alpha` → `clojure.string`, so 27
  scripts use `clojure.string/...` fully-qualified without requiring
  it (verified: the compiler does NOT auto-require — `Compiler.java`
  `resolveIn` throws). `cmd_run` requires it up front, inside the
  S3 frame (`run_preloads_clojure_string_201`).
- F5. `odd?`/`even?` over bigints. Only `Int` answered; the
  miller_rabin recipe calls `(even? (bigint n))`. All integer kinds
  answer via the low bit (`bit(0)`, two's complement — `control.rs`
  `odd_even` big-int rows).
- F6. Class gaps (each oracle-pinned): `Character/digit` 2-arg,
  `Matcher/replaceAll` 1-arg (String; nil reports the String-vs-
  Function ambiguity), `Long/remainderUnsigned` 2-arg (`/ by zero`
  on zero), `String.indexOf` 1-arg (UTF-16 offsets) +
  `List.indexOf` 1-arg (first `.equals`), `Math/log|exp|pow`,
  `Character/isLetterOrDigit` 1-arg (`*_201` rows in
  `tests/classemu.rs`).
- F7. `Locale/US` + `String/format` 3-arg (44 scripts pin the locale
  explicitly). New `LocaleHandle` extension (`en_US`), lattice row,
  `US` field; `format` reuses the `clojure.core/format` engine over
  the args array (`locale_string_format_201`).
- F8. `into-array` with a class designator (pre-existing breakage:
  only keywords worked). Classes map by name; reference element
  types share the `Object` store (store checks narrow away —
  documented; `arrays.rs` `into_array` rows).
- F9. `PersistentQueue/EMPTY` (4 BFS scripts). Rides on a
  metadata-marked vector (FIFO via queue `peek`/`pop` arms;
  empty pops answer the queue itself — oracle; prints as a
  vector — documented narrowing). Lattice row transcribed from
  the JVM (`Obj` + `Counted`/`IHashEq`/`IPersistentList`/
  `Collection` — no `List`). `persistent_queue_fifo_201`.
- F10. String scan cache. `(nth s i)` rescanned chars per call
  (O(n²) — the 37MB dijkstra parse never finished). A Weak-keyed
  8-slot MRU cache holds one scan per `Arc` (ASCII: byte index;
  else capped char→byte offsets + UTF-16 length); `count` rides
  it too (`coll.rs` `nth_rows` big-index rows).
- F11. Math trig/hyperbolic family + `Character` case. The optics
  scripts call `Math/sin|cos`, `finance/var_es` too,
  `search/backprop` calls `Math/tanh`, `bitonic_tsp` computes
  `Math/hypot` over ints, `fol_unify` tests `Character/isLowerCase`
  (31 cases). All widen through `math_double_arg` (a `macro_rules!`
  one-liner each: `sin cos tan asin acos atan sinh cosh tanh cbrt
  floor log10`, plus 2-arg `atan2 hypot`, plus `round` with
  Java-style narrowing); a static-usage sweep of all 394 scripts
  confirmed these are the only missing members
  (`math_trig_hypot_201`, `character_case_201`).
- F12. `concat` over tens of thousands of inputs. `Thunk::Concat`
  held owned `inputs: Vec` and `force_pending` clones the whole
  thunk per force (to restore it when a step fails): stepping
  `(apply concat buckets)` was O(n²) (68s at n=32K — the
  `suffix_array` timeouts). Inputs are now `Arc<Vec>` plus an
  index, with the current input's tail in `cur`: clone and step
  are O(1) (8s at n=32K, outputs identical;
  `concat_many_inputs_order_201`).

Resources: the 1754-case set is the superproject's
`resource/shared` at the pinned local HEAD (remote-HEAD has only
641; the workstation checkout was stale and showed 278
missing-resource problems). Suite command (workstation):
`snip test --resources ~/w-resources/resource/shared --runner
~/w-clojure-rs/rust/target/release/clojure-rs --jobs 4 --timeout
120` (a `--runner`-carrying `snip` build; the flag is local-only,
never shipped to the snippet repo).
