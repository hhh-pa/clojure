# Issue #271: AOT Performance Catch-Up with OCaml — Specification

Status: Draft (Spec-first commit)
Issue: https://github.com/hhh-pa/clojure/issues/271
Baseline: `ca27a18e` (`master`, post-#259/#260)

## 1. Goal

**Requirement: AOT performance must outperform OCaml** (issue #271, updated 2026-11-17). On the Linux `h2h-cpu-mem` workflow, native binaries produced by `clojure-rs build` must beat native OCaml (`ocamlopt -O2`) across the shared snippet suite. On every compared case, AOT's median Pin instruction count and median peak RSS must both be strictly below OCaml's: every `aot-ocaml` case is a win in `h2h-compare`'s terms, with no losses and no ties (`--metrics mem,ins`). Staying within the 1.15x margin (parity) is not enough. Enforcing this needs an opt-in `h2h-compare` mode that fails on ties as well as losses; the other gates keep their loss-only rule.

To establish the authoritative Linux baseline and enforce every subsequent PR, first extend `.github/workflows/h2h-cpu-mem.yml` and `scripts/ci/go/cmd/h2h-compare` to measure `AOT from`, `AOT to`, and `OCAML`, and add the `AOT from` vs `AOT to` (`aot-rsrs`) and `AOT` vs `OCAML` (`aot-ocaml`) quality gates.

## 2. Invariants and Non-Goals

1. **Interpreter invariance:** `rs-from` and `rs-to` inside `h2h-cpu-mem.yml` MUST remain built with `cargo build --release --no-default-features --locked` and run via `clojure-rs run`, preserving exact parity on existing `recipes` (`rs-from` vs `python`) and `recipes-rsrs` (`rs-from` vs `rs-to`) gates.
2. **Semantic equivalence:** `clojure-rs build` optimizations MUST preserve observable output, exit codes, and Clojure error semantics of `clojure-rs run`.
3. **No benchmark gaming:** Snippet sources in `hhh-pa/clojure-code-snippet` and `hhh-pa/ocaml-code-snippet` MUST NOT be altered to work around compiler/runtime bottlenecks inside `clojure-rs`.
4. **Workspace quality gates unchanged:** All existing Rust and Go gates apply without relaxation (100.00% canonical line and patch coverage, `cargo fmt --all --check`, `cargo clippy --workspace --all-targets -- -D warnings`, MSRV `1.85` on non-`aot` crates, `--no-default-features`, complexity ratchet, `geiger`, `udeps`, `deny`, `audit`, `go test ./...`).
5. **Non-goals for Slice 1:** Slice 1 delivers measurement, gating, tests, and the authoritative Linux Pin baseline report; compiler/runtime optimization changes belong to Slice 2+ so the baseline instrumentation can be reviewed cleanly and verified first.

## 3. Suite Parity Context

Direct repository inventory at spec time:

- `hhh-pa/clojure-code-snippet`: 394 scripts, 1,754 test cases.
- `hhh-pa/ocaml-code-snippet`: 393 scripts, 1,743 test cases.
- Every script in `ocaml-code-snippet` has a matching `(label, stem)` and case set in `clojure-code-snippet`; `string_related/suffix_tree/string_function_calculation` (11 cases) is Clojure-only.
- `ocaml-code-snippet` uses `ocamlopt -O2` and only OCaml standard library modules (`open Complex` appears in `optics/jones_mueller.ml` and `optics/thin_film.ml`). Its `discover.go` differs slightly from `clojure-code-snippet/scripts/ci/go/internal/snipcore/discover.go` on nested paths, so `h2h-cpu-mem.yml` MUST run `clojure-code-snippet`'s `snip measure --script-extension .ml` (mirroring `python`'s `--script-extension .py`) so shards and `(label, stem, case)` keys match deterministically.

## 4. Slice 1 — `h2h-compare` Contract

Extend `scripts/ci/go/cmd/h2h-compare`:

1. **New flag:** `--allow-extra-challenger` (boolean, default `false`).
2. **Default behavior (`false`):** Preserved byte-for-byte. Any non-empty `onlyBase` or `onlyChal` fails with `leg mismatch (only-<base> N, only-<chal> M)`. Existing gates (`recipes`, `recipes-rsrs`, `aot-rsrs`) MUST leave this flag off.
3. **Opt-in behavior (`--allow-extra-challenger`):**
   - `onlyChal` cases (challenger keys missing from base, e.g. the 11 Clojure-only cases of `string_function_calculation`) are still printed in the informational summary (`only-<chal>: N` and listed keys) and are **excluded from failing `problems` as a leg mismatch**.
   - `onlyBase` cases (any base/OCaml key missing from challenger/AOT) **MUST STILL FAIL** with `leg mismatch (only-<base> N)`.
   - If the intersection of base and challenger keys is empty (`len(common) == 0`), comparison **MUST FAIL** (`no common cases between <base> and <chal>`).
   - Metric comparisons (`MEM`, `INS`), `not-ok`, `test_counts` drift, `failures`, and `missing` checks run strictly over the compared cases and leg summaries using the exact same rules and `--margin` threshold.
4. **Tests:** Unit tests and golden/fixture tests in `scripts/ci/go/cmd/h2h-compare/main_test.go` MUST cover:
   - flag parsing (`--allow-extra-challenger`),
   - passing run when only `onlyChal` is non-empty and all common cases pass,
   - failing run when `onlyBase` is non-empty even with `--allow-extra-challenger`,
   - failing run when `len(common) == 0` with `--allow-extra-challenger`,
   - default strict mode unaffected.

## 5. Slice 1 — `.github/workflows/h2h-cpu-mem.yml` Contract

### 5.1 Inputs & `resolve` job

- Resolve `ocaml_snippets_sha` from `hhh-pa/ocaml-code-snippet` (default `master`, overridable via `workflow_dispatch` input alongside `snippets_sha` and `python_snippets_sha`).
- Compute `waive_ocaml` boolean alongside `waive_recipes` and `waive_h2h`:
  - Triggered by `[waive-ocaml]` in PR head commit subject/body (or `waive_ocaml` `workflow_dispatch` input).
  - Independent of `[waive-recipes]` so `recipes` stays controlled solely by `[waive-recipes]`/`[waive-h2h]` and `aot-rsrs` is controlled only by `[waive-h2h]`.

### 5.2 `measure` job (32 shards)

1. **Toolchains & dependencies:**
   - Provision LLVM 23 via `./.github/actions/setup-llvm`.
   - Install `ocaml` and `ocaml-native-compilers` via `awalsh128/cache-apt-pkgs-action` (same package pair as `ocaml-code-snippet`).
   - Check out `hhh-pa/ocaml-code-snippet` at `ocaml_snippets_sha` alongside `snippets` (`clojure-code-snippet`) and `py` (`python-code-snippet`).
   - Decrypt and extract shared test resources into `ocaml/tests/resources` using `RESOURCES_PASSPHRASE` (same archive already extracted for `snippets` and `py`).
2. **Isolated binary builds:**
   - Build `--no-default-features` interpreter binaries (`rs-from`, `rs-to`) and copy/stage them **before or in separate target dirs from** default-feature `aot` builds so feature toggling never contaminates `run` measurements.
   - Build `aot-from` (from `work/`) and `aot-to` (from `base/` when distinct from `work/`) with default `aot` feature:
     - `cargo build --release --locked`
     - `cargo rustc --release --locked -p clj-rt --crate-type staticlib`
     - Stage `clojure-rs`, `libcljrt.a`, and `hot-text.ld` together into dedicated directories (`$WORK/aot-from-bin/`, `$WORK/aot-to-bin/`) so `NativeBuildCache` / `clojure-rs build` automatically discovers matching `libcljrt.a` and Linux `hot-text.ld` next to the compiler binary.
3. **Isolated OCaml wrapper:**
   - Provide an executable wrapper in `$RUNNER_TEMP/bin/ocaml-build`:
     ```bash
     #!/usr/bin/env bash
     set -euo pipefail
     script=$1
     out=$2
     work=$(mktemp -d)
     trap 'rm -rf "$work"' EXIT
     cp "$script" "$work/$(basename "$script")"
     ocamlopt -O2 -o "$work/a.out" "$work/$(basename "$script")"
     mv "$work/a.out" "$out"
     ```
   - This prevents `ocamlopt` from writing `.cmi`/`.cmx`/`.o` artifacts into `ocaml/src/...` and isolates concurrent `--jobs` workers cleanly.
4. **Measurement runs:**
   - Measure `aot-from`: `./scripts/ci/go/snip measure --runtime aot-from --resources tests/resources --build-template "$AOT_FROM_DIR/clojure-rs build {script} -o {out}" --pin ...` writing `aot-from-results.json`.
   - Measure `aot-to` (using `aot-from-results.json` directly if `base == work` and non-PR dispatch reuses the single commit, OR building and measuring `base/`): writing `aot-to-results.json`.
   - Measure `ocaml`: inside `ocaml/`, `../snippets/scripts/ci/go/snip measure --runtime ocaml --script-extension .ml --resources tests/resources --build-template "$RUNNER_TEMP/bin/ocaml-build {script} {out}" --pin ...` writing `ocaml-results.json`.
   - Upload shard artifacts `aot-from-results-<shard>`, `aot-to-results-<shard>`, `ocaml-results-<shard>` (or bundled alongside shard outputs) for `h2h-compare`.

### 5.3 `h2h-compare` job

Run five comparisons with `--metrics mem,ins --margin 0.15` (and pass `--report-only` only per the corresponding waive flag):

1. `recipes`: `python` (base) vs `rs-from` (challenger) — existing gate, strict key match.
2. `recipes-rsrs`: `rs-to` (base) vs `rs-from` (challenger) — existing gate, strict key match.
3. `aot-rsrs`: `AOT to` (`aot-to`, base) vs `AOT from` (`aot-from`, challenger) — regression gate for `AOT from` vs `AOT to`, strict key match, fails on any MEM/INS loss unless `waive-h2h`.
4. `aot-ocaml`: `OCAML` (`ocaml`, base) vs `AOT` (`aot-from`, challenger) — `--allow-extra-challenger`, compares all 1,743 OCaml-counterpart cases cleanly, honors `waive-ocaml` / `waive-h2h` during Slice 1 baseline capture and optimization bring-up, then becomes strictly enforced (unwaived) before closing #271.
5. `aot-run`: `run` (`rs-from`, base) vs `AOT` (`aot-from`, challenger) — the same revision interpreted vs compiled, strict key match (both legs run all 394 scripts), fails on any MEM/INS loss unless `waive-aot-run` / `waive-h2h` (#259: compiled binaries must outperform `clojure-rs run`).

Upload merged JSONs (`merged-aot-from.json`, `merged-aot-to.json`, `merged-ocaml.json` alongside existing merged artifacts) so local analysis scripts can inspect exact per-case Pin instruction and RSS ratios.

## 6. Slice 2+ — Optimization Methodology

Once Slice 1 yields authoritative Linux Pin (`pin_instructions`) and RSS (`maxrss_kb`) distributions for `AOT` vs `OCAML`:

1. Categorize losses by:
   - small-case startup/runtime RSS floor vs OCaml (~8–10 MB OCaml floor),
   - steady-state allocation / peak heap working set,
   - instruction-count hot families across `clj-aot`, `clj-bc`, `clj-rt`, `clj-value`.
2. Ship focused, well-tested slices, each verified against:
   - `rust.yml` full gate suite (100.00% line & patch coverage, MSRV, `--no-default-features`, complexity, geiger),
   - `h2h-cpu-mem.yml` showing measurable progress on `aot-ocaml` and zero regressions (`0 MEM`, `0 INS`) on `aot-rsrs`, `recipes`, and `recipes-rsrs`.
3. Remove `[waive-ocaml]` usage once unwaived `aot-ocaml` shows AOT outperforming OCaml on every compared case (wins only on MEM and INS: no losses, no ties).

### 6.1 `BootMode::Lite` semantic parity contract

`BootMode::Lite` binaries keep no `clojure.core` / `clojure.string` source resident, so every native that stands in for an interpreted definition under Lite MUST reproduce `clojure-rs run` for **every** input — values, exception classes, and messages — not only the inputs its fast path takes (Invariant 2):

1. **Natives replacing `core.clj` definitions** (`reduce-kv`, `run!`) transcribe the definition literally: a non-fn is invoked exactly as the interpreted `(f …)` invokes it (the interpreter's own invoke error, never an `IFn` pre-check), and `reduce-kv` takes `core.clj`'s index loop for every `vector?` (subvecs, `vector-of`), not only persistent vectors.
2. **`clojure.string` slow paths** (inputs outside a native's fast path): `run` keeps calling the interpreted `string.clj` definition; Lite runs that definition's literal transcription through the same interop entry points (`__meth*` for `.length` / `.charAt` / `.toString` / `.startsWith` …, the `Character/isWhitespace` emulation, the `zero?` / `dec` / `inc` / `split` natives), so `java.io.File`, `StringBuilder`, nil, and non-`CharSequence` receivers fail or succeed exactly as under `run`.
3. **Enforcement:** `crates/clj-eval/tests/lite_parity.rs` evaluates every case in a full-boot evaluator (the oracle) and a Lite evaluator and requires identical printed outcomes; a case either side cannot read, analyze, or realize fails the suite (no vacuous matches).
4. **Deferred (not Lite-eligible in Slice 2):** `clojure.set` and `clojure.string/replace`. Programs referencing them keep the full boot. A native `replace` / `clojure.set` cannot be exact yet: `run`'s own `replace-by` throws `No matching ctor found for class java.lang.StringBuffer` on any fn replacement that matches, and the `Matcher.replaceAll` emulation diverges from the JVM on `$10` / `\$` / empty matches; fixing `run` itself is a separate change, after which these natives land with `lite_parity` coverage. (`clojure.set` became Lite-eligible in Slice 3k without natives, by compiling its source into the program: §6.5.)

5. **Stream vars of `with-out-str` / `with-in-str`.** These analyzer builtins name an unqualified `*out*` / `*in*`, which the runtime resolves when the body runs. Under `run`, `user` refers `clojure.core`, so the name finds `clojure.core/*out*`. A Lite program refers nothing: the lookup missed and interned a fresh, non-dynamic `user/*out*`, and `binding` it threw `Can't dynamically bind non-dynamic var: *out*`. The stream var now resolves as the current namespace maps the name, so a user shadow still wins as under `run`, else through `clojure.core`. The compiler declares `clojure.core/*out*` / `*in*`, so the Lite boot installs them as dynamic vars. Corpus `docs/spec/i259-native-compiler/corpus/with_out_str.clj`.

### 6.2 Slice 2 contents

- Lite startup: dynamic vars installed only when referenced, var metadata synthesized only when a reader (`meta`, `doc`, `resolve`, …) is referenced, lazily allocated binding-count / lazy-index / tap-queue tables.
- Codegen: inline static fast paths (`Long/parseLong`, `Integer/parseInt`, `Double/parseDouble`, `Long/remainderUnsigned`, `Math/*`) that fall back to the generic `__static*` invoke for any input they do not take, variadic `+` / `-` / `*` folding (up to 8 args), exact integer `/`, numeric comparison chains, and `min` / `max`.
- New Lite natives: `reduce-kv`, `run!` (contract 6.1).

### 6.3 Slice 3a: static Linux AOT links

Linux callgrind probes (issue #271 comment) put the dynamic loader at ~43% of a small AOT program's instructions (eager GOT symbol lookups) and the shared C libraries at ~1.65 MB of its RSS. `clojure-rs build` on Linux therefore links static non-PIE first and keeps the dynamic link as the fallback (static C libraries missing):

1. **Flags:** `-static -Wl,--eh-frame-hdr -Wl,--wrap=__register_frame_info -Wl,--wrap=__deregister_frame_info`, on top of the existing `-no-pie`, hot-text script, and `--gc-sections`.
2. **No eager FDE registration:** crtbeginT's `.eh_frame` registration (GCC 13+ classifies every FDE at registration — ~415K instructions plus the whole `.eh_frame` faulted in) is wrapped to no-ops the compiler emits into every module (unreferenced and dead-stripped elsewhere); `--eh-frame-hdr` keeps unwinding (`catch_panic`) on the `dl_iterate_phdr` path dynamic executables use.
3. **Observable behavior is unchanged:** output, exit codes, and error text are the same as the dynamic binary's; only the startup floor and RSS move (probe: `(println 1)` 356K → 170K instructions, 3.2 → 1.5 MB RSS).

### 6.4 Slice 3d: per-program startup work

Linux callgrind profiles of real snippets run on empty stdin (issue #271 comment) put a typical program's startup at ~960K instructions, about 800K of it above the `(println 1)` floor and ~25% of it in `malloc`/`free`. The dominant costs were per-var and per-`defn`, so this slice removes them:

1. **Native lookup hints.** The compiler records, per declared var, where its Lite native sits in the capability tables (`aot_boot::lite_native_hint`: group, table, entry — first-match order, like the runtime scan). The binary declares all vars in one `rt_n_declare_vars` table (`ns`, `name`, hint; each namespace name shared), and boot installs a hinted native only after checking that the entry still carries the var's name, falling back to the scan otherwise (a runtime archive from another build stays correct).
2. **Var metadata only where it can be seen.** A Lite program that references no var-metadata reader (`aot_boot::needs_var_meta`: `meta`, `doc`, `alter-meta!`, `reset-meta!`, `vary-meta`, `macro?`, `resolve`, `ns-resolve`, `ns-publics`, `ns-interns`, `ns-refers`) cannot observe var metadata, so:
   - `defn`'s discarded `(alter-meta! (var f) assoc :arglists '…)` statements (and `merge`s of constant maps) are not lowered when they only write `aot_boot::LITE_INERT_DEF_META` keys (`:arglists`, `:doc`, `:line`, `:column`, `:file`) with constant values; a reader-free program then references no `alter-meta!` at all;
   - `def` names and their evaluated metadata pairs drop the same inert keys, so names pool as plain symbols instead of read-back forms; every other key (`:private`, `:dynamic`, `:tag`, `:const`, …) stays, since runtime paths (`refer`, `binding`) consult some of them;
   - interned vars carry no `{:ns :name}` stamp and the Lite dynamics no `:dynamic` stamp (binding semantics use the var's own `dynamic` flag).
   - a `def` whose name carries metadata (`defn-`'s `{:private true}`) keeps exactly that map as the var's metadata, without the `:line` / `:column` / `:file` / `:name` / `:ns` auto keys a reader would see (Slice 3n; `refer` consults `:private`, nothing reads the rest).

   A program that references any reader keeps every metadata key exactly as `run` builds it.
3. **Cheaper installs:** native symbols share their namespace's name and are qualified in one allocation; compiled var slots resolve through symbols built from their shared names.
4. **Smaller fixes:** `split-lines` takes the `\r?\n` byte cut (its source spelled the pattern with literal CR/LF, which compiled a full regex — ~275K instructions — on first use), and the static class lattice index is Fx-hashed and pre-sized.
5. **Batched installs:** boot installs each namespace's declared natives in one registry pass (`install_native_rows` → `intern_roots`: one lock, one namespace resolution, one mutation epoch per batch) with no per-var "already installed" probe (the compiler's list is distinct; only the internal helpers the program did not declare are probed). Hints carry an "owned-args entry exists" bit, so most installs skip the owned-table searches; a stale bit can only drop the owned fast path, never change results.
6. **`zipmap` and literal strings:** `zipmap` collects its pairs and builds a small result once (`Map::from_kvs_last_wins`, the `assoc` chain's exact map) instead of interning a key shape per pair — `(zipmap "0123456789" (range))` opens 238 of the 394 snippets — and continues with in-place inserts past 64 pairs; its forcing order (both heads, then both tails) is unchanged. Compiler-emitted strings decode through `str::from_utf8`'s fast path, with the lossy decoder as the fallback. Later (Slice 3n), a string's chars zipped against the core `(range)` iterate or a counted long range pair up directly. No user code runs and nothing anyone else sees is memoized, so the pairs feed the same build; overflow takes the walk. An iterate stepping the core `inc'` / `inc` natives over a long below the max computes `n + 1` without the call (corpus `zipmap_ranges.clj`).
7. **Names with metadata pool without the reader.** A pooled constant the compiler cannot build directly is printed to text and read back with the full reader at startup (about 12K instructions per constant on macOS). Every `defn-` name is such a constant: it is a symbol carrying `{:private true}`, which Lite keeps because `refer` consults it. `^String` and `^:dynamic` names are too. A symbol whose metadata maps keywords to scalars, keywords, plain symbols, or strings is now a `Const::MetaSymbol`. The constant init writes its pairs into slots by name (`rt_n_kw` / `rt_n_sym` / `rt_n_str`, scalars inline; it cannot read other pool constants before the pool freezes). `rt_n_meta_sym` then inserts them pair by pair into an empty map, exactly as the reader merges `^{…}` (`native_abi::meta_symbol`, checked against the reader). Other metadata, such as an `:arglists` list when the program reads var metadata, still reads back. Locally, no_prefix_set (ten `defn-`s) saves 110K–210K instructions per run (corpus `meta_names.clj`). Metadata-free collections of such forms build the same way: quoted data, catch class lists, and `:arglists` lists become `Const::Coll`, built by the reader's own constructors (`native_abi::const_coll` via `rt_n_coll`). Each candidate is kept only if building it at compile time prints exactly as the original does, and it is capped at 256 forms. That cuts startup reads across the snippets from 212 to 9 (only the queue constant, which carries metadata, still reads). It also fixes a divergence: the reader stamped `:line`/`:column` on macro-built lists such as `:arglists`, which carry none under `run` (corpus `const_colls.clj`).

### 6.5 Slice 3k: spliced libraries

The full boot costs ~14M instructions per run (Linux Pin), against OCaml's ~0.6M for a whole small case, so every program forced onto it loses all its cases. Four snippets took the full boot only because their leading `ns` form requires `clojure.set` (three call `union` / `intersection` / `difference`; one never calls it).

1. **Splice:** when a program's first unit is an `ns` form whose `:require` names a library in `aot_front::SPLICED_LIBS` (`clojure.set`), the front end reads, expands, and analyzes that library's vendored source (the same embedded text `run`'s `require` loads) and prepends its units, recorded as `FrontEnd::lib_units` / `FrontEnd::libs`. A `require` anywhere else (a later unit, a `(require …)` call) does not splice.
2. **Exact by construction:** the library's definitions compile like user code, so they behave exactly as `run`'s interpreted library: values, thrown classes and messages, non-set arguments, the variadic arities, and the one-value-per-form `#{}` of `union`'s zero arity. No native stands in for them, so contract 6.1's transcription rule does not apply. The i259 corpus program `clojure_set.clj` covers every `clojure.set` fn (`jit` ≡ `run`, the same module `build` links).
3. **Boot selection:** the Lite check accepts a `:require` spec naming a spliced library (the namespace's vars resolve to the spliced definitions); at runtime a Lite `ns` form only switches namespaces, as for `clojure.string`. A program that needs the full boot for another reason drops the spliced units and loads the interpreted library through its own `require`, exactly as before (corpus `clojure_set_full.clj`). The `aot: native N/N defns` report counts the program's own definitions only.
4. **Effect (macOS instructions, unchanged output):** ssa_dominance, dfa_minimization, hindley_milner, and ll1_parser each drop ~14M per case (e.g. ssa_dominance 37.1M → 22.7M, including macOS's ~20M process floor).
5. **Selective libraries (`clojure.string`):** a library whose vars are mostly Lite natives splices only what the program reaches that no native covers. The front end collects the library vars the program names without a native (`aot_boot::is_lite_string_var`), then keeps the top-level forms defining them and, transitively, the forms those reach. It also keeps the library's `ns` form with its clauses dropped: the build evaluator applied them, and a Lite `ns` form only switches namespaces. `TopUnit::form` groups each `defn`'s split units, so `def`, metadata write, and `var` stay together. If a needed var has no definition there, nothing is spliced. A program that looks vars up at runtime (`resolve`, `ns-resolve`, `ns-publics`, `ns-interns`, `ns-map`, `find-var`) splices nothing either, since a subset would show there. A `def` into `clojure.string` blocks Lite only when it redefines a native-served var, so spliced definitions keep it. knuth_div and cantor_expansion (`str/replace`) leave the full boot: macOS instructions −9M to −12M per case and RSS about −2 MB, with unchanged output (corpus `clojure_string_replace.clj`). The spliced `replace` reproduces `run` exactly, including #273's divergences from the JVM.

### 6.6 Slice 4a: stable var roots and borrowed operands

Per-operation costs, not startup, separate AOT from OCaml on most remaining cases. Linux callgrind on loop microbenchmarks (`probe/mb`) priced a global var read at about 100 instructions: `rt_bc_deref` (epoch, slot, and binding checks plus an atomic `Arc` increment) and the matching drop. Every pooled constant used as an argument also went through an `rt_n_const` clone and a drop. `(:k m)` paid the general `rt_n_invoke` (about 450 instructions on a three-key map).

1. **Stable vars.** In a Lite program in which no unit can re-root an arbitrary var (`mentions_rebinding`: `alter-var-root`, `with-redefs`, the var-resolving fns, …), a var is stable when exactly one `def` with an init names it anywhere in the program. That `def` must be a whole top-level unit, the var must not be `^:dynamic`, and the program must never `set!` it or take it as a var object (`#'v`) outside the inert units `defn` emits. Its root cannot change once that `def` has run.
2. **Root caches.** Each stable var gets an internal module global, holding `nil` until its `def` copies the root in (`rt_n_deref`), and a set flag stored with release ordering. A call that only reads its arguments (bitcode helpers, `rt_n_op*`, the borrowed core call, direct `defn` calls) is passed the cache itself; other reads clone from it. Code in a later unit than the `def` can only run after the `def` has run, so it skips the flag. Code in the `def`'s unit or earlier checks the flag (acquire) and falls back to the var while the cache is unset.
3. **Borrowed constants.** `cljtop_0` stores the frozen constant table's base (`rt_n_consts_base`; the table never moves after `freeze`), and read-only call operands borrow `&consts[k]` in place.
4. **Keyword invokes.** `(:k coll)` and `(:k coll not-found)` with a literal keyword go through the bitcode `get` with the keyword borrowed from the pool. Any shape the helper leaves to the runtime takes the keyword's own invoke. Both use the interpreter's `lookup`, so results are identical.
5. **Keyword equality is pointer equality.** Every keyword is interned (`Keyword::new` is the only constructor, and the table keeps each entry for the life of the process), so `Keyword::eq` no longer falls back to comparing strings when the pointers differ. `Hash` is unchanged, because hash-trie iteration order depends on it.
6. **Effect (Linux callgrind, per iteration):** a vector `nth` through a global drops from 271 to about 170 instructions. On macOS, `(:b rec)` falls from 661 to 139, `contains?` on a global set from 485 to 361, and `get` on a global map from 560 to 440. Snippet output is unchanged (corpus `stable_vars.clj`: forward references, dynamic, re-`def`ed, nested, and `#'`-taken vars, and every keyword-invoke shape).

### 6.7 Slice 4b: cheaper direct calls

A direct call of a compiled `defn` cost about 290 instructions on macOS beyond its body. `rt_n_enter_fn` checked that the var still held that fn (thread-local caches, the registry epoch) and the call depth. The callee also cloned every parameter whose last use the move analysis recorded, even when that use only read it.

1. **No root check for stable `defn`s.** A direct-call target's `def` init is its `fn` literal, so a call site in that `def`'s own unit (the body, including recursion) or a later unit can only run once the root is the compiled fn. If the var is stable (§6.6), such a site calls `rt_n_enter`, which only checks call depth (the `MAX_CALL_DEPTH` / stack red-zone fallback is unchanged). Forward calls through `declare` keep `rt_n_enter_fn`.
2. **Borrowed parameters.** A parameter whose every use is an operand of a core call outside the owned-move ops, or of an invoke, keeps reading the caller's argument array. An owned copy at entry can buy no in-place update for it. Parameters reaching an owned-move op, a `let`, `recur`, or the result still take or clone their value as before, including `reduce`'s handed-over accumulator.
3. **Effect (macOS, per iteration):** `(defn pick [w i] (nth w i))` called in a loop falls from 422 to 231 instructions (call overhead about 290 → 96). Corpus `direct_calls.clj` covers recursion at depth 20,000, forward calls, and every parameter shape.

### 6.8 Slice 4c: inline traversal loops

`doseq` expands to `(dorun (map (fn [x] …) xs))` and `for` to a lazy `map` of a `fn`, so every iteration of these and of `mapv` / `reduce` with a literal `fn` was a lazy-seq step plus a callback into a compiled closure. That cost 400–550 instructions per item beyond the body (Linux callgrind: `doseq` over a range 531, `for` 548).

1. **Shapes.** `(dorun (map F xs))`, `(mapv F xs)`, `(vec (map F xs))`, `(into [] (map F xs))`, and `(reduce F init xs)`, all `clojure.core` in a program that cannot rebind core vars, where `F` is a literal `fn` with one arity of exactly the needed symbol params (one, or two for `reduce`) that never `recur`s to itself. A named `fn` arrives as a `letfn`, so `F` never refers to itself.
2. **Direct loop.** The collection (and `reduce`'s init) evaluate in argument order. `rt_n_iter_plan` (`native_abi::iter_plan`) then classifies the collection. A persistent vector iterates by index (`rt_n_vec_item`) and a long range by arithmetic (wrapping, so the unused value after the last cannot trap). The `fn`'s body runs in place with its params bound like a `let` at the env indices the analyzer gave them. It is lowered with its own last-use moves: a param or a body local may move, while an enclosing local is read afresh every item. `mapv` and `vec`/`into` accumulate into a uniquely held vector (`rt_n_vec_push`). `reduce` stops on a `reduced` value (`rt_n_unreduce`).
3. **General path.** Any other collection (lazy seqs, lists, maps, sets, strings, nil) makes the `fn` a closure and calls the original core functions over the already-evaluated collection, with the arguments moved over as the original call took its temporaries. `run`'s semantics therefore hold exactly, lazy forcing order and `walk_sole_map` fusion included.
4. **Moves.** The analysis saw the closure capture its free locals before the collection evaluated, so it may have let the collection move a local the body still reads. While the collection and init lower, those locals' moves are suppressed.
5. **Effect (macOS, per item):** `doseq` over a range 423 → 22 instructions, `reduce` with a literal `fn` 460 → 138, `mapv` over a vector 449 → 307 (the accumulator push dominates). Corpus `hof_loops.clj` covers every shape and collection kind, early `reduced`, exceptions in the body, nested loops, captured locals, and lazy forcing order.

### 6.9 Slice 4d: fewer runtime boundaries for small programs

70% of the instruction non-wins (458 of 655 at `b4a57992`) are cases where OCaml's whole run is under 0.7M instructions. Linux callgrind of every losing script with empty input puts AOT's per-program startup at a median 293K (trivial floor 96K). Exact per-case call counts (an instrumented runtime archive, macOS) show the small cases crossing these runtime boundaries most: `rt_n_nth_nil` (146 per case: vector destructuring), `rt_n_vec_lit` (66), and closure creation (48).

1. **Destructuring reads inline.** `(let [[a b] v] …)` and destructured params read each position with the inlined bitcode `(nth v i nil)` (vectors, strings, nil), falling back to `rt_n_nth_nil` for every shape the helper leaves to the runtime. macOS: a two-element destructure in a loop drops from 481 to 352 instructions per iteration.
2. **Vector literals take their items.** `rt_n_vec_lit` moves the evaluated items out of their slots (each left nil) instead of cloning them (`native_abi::vec_lit_owned`). All-long items go straight to packed storage, the storage `Vector::from_vec` would choose, without an intermediate boxed vector.
3. **Loop accumulators without copy-on-write.** A compiled `mapv` / `vec` / `into []` loop (§6.8) gathers its items in a `VecBuilder`. Only the loop holds it, so a push is a uniqueness check and a buffer push: no `Arc::make_mut`, and no nil drops. Items stay packed while every one is a long, and the builder freezes into one persistent vector at the end (`rt_n_vec_done`). `mapv` over a vector: 307 → 233 macOS instructions per item.
4. **Var metadata only where it can be read.** The by-name readers (`doc`, `macro?`, the var-resolving fns) find vars themselves, so they still count. The value readers (`meta`, `vary-meta`, `alter-meta!`, `reset-meta!`) now count only when the program can hold a var object, meaning a `#'v` outside `defn`'s own forms (`needs_var_meta(vars, var_objects)`). A discarded constant metadata write whose keys the runtime never acts on (anything but `:dynamic`, `:private`, `:macro`, `:redef`) is inert: an attr map's `:added`, a `:tag` hint. The compiler passes its decision to the boot (`rt_n_cap_var_meta`), so both agree. `clojure.set`'s `select` and `rename` read their relations' metadata and its attr maps write `:added` / `:tag`, so every program that spliced it kept full def metadata. hindley_milner, ssa_dominance, ll1_parser and dfa_minimization each save 0.6–0.9M macOS instructions per run (Linux startup about 1.2M before). Corpus `var_meta_reads.clj` and `set_meta_data.clj`.

### 6.10 Slice 4e: printing reads its settings vars through a cache

Exact Linux costs of single operations come from callgrind on synthetic programs on the fork, by slope between two iteration counts. `println` cost about 14.2K instructions per call, even for a constant string. About 12K of that went to resolving about 13 `clojure.core` settings vars by name per call: `*out*` per argument and again for the newline and the flush, `*flush-on-newline*`, the six print options, and `*print-readably*` for the `print` binding. Each `deref_core` string-hashed the namespace and the name, allocated a fresh `Symbol`, then looked the var's state up again through its home namespace.

`Evaluator::core_read` keeps each probed `clojure.core` name's id, or its absence, and its root while the lineage epoch holds. Thread bindings are still checked first on every read. Every root write and namespace mutation bumps the epoch. So now do `remove-ns` and any intern that creates a var: a Lite program installs a print var on first reference, after printing may already have cached its absence. Names are literals, so entries match by address before by text. Linux, exact: `println` 14.2K → 8.5K per call with found names cached; caching misses saves about 2.6K more per `println` on macOS. Most cases print 1-4 lines (median 2). Corpus `print_settings.clj` pins bindings, root writes and nested `with-out-str` against `run`; `print_late_vars.clj` pins print vars first referenced after a print.

### 6.11 Slice 4f: less fixed work per program

Linux callgrind of every losing script's empty-input run, profiled function by function, prices each program's fixed work:

1. **`slurp` of stdin.** It read into an 8 KiB zero-filled first chunk. Pin and callgrind count the bulk zero-fill per byte, so that cost about 8.4K instructions per program, although inputs are tiny (median 26 bytes; 94% at most 8 KiB). Closing the reader then built a fresh parser (about 3.4K) over one `slurp` had just drained. Stdin now reads through `read_to_end` into one buffer that is never zero-filled. std probes small inputs on the stack and grows large buffers by `realloc`. The text is validated once, with `\r\n` and a lone `\r` normalized in one pass, and `slurp` only latches the reader closed.
2. **Declared var names.** `rt_n_declare_vars` received an `ns`/`name` string pair per var: two UTF-8 validations and an `Arc<str>` allocation per var, about 22K for a typical program's 42 vars. The compiler now emits one names text, each namespace once, then every var name, plus a `(ns span, name span, hint)` row per var. The runtime validates and copies the text once, and slots hold spans. Only the vars a program actually interns build a `Symbol`.
3. **Docstrings.** `def` built its docstring even when the program keeps no var metadata, which never reads it (§6.9 item 4).

Exact Linux startup (callgrind, empty input, 365 scripts) for the three together: median 256K → 225K instructions. Every script saves between 22K and 41K (median 29K).

### 6.12 Slice 4g: unread pure defs drop

Most snippets carry the same input boilerplate. A `digit-values` table `(zipmap "0123456789" (range))` appears in 220 of 393 scripts and is never read in 177. Likewise `separators` / `whitespace-chars` character sets (unread in 165) and `max-long` `(bit-not min-long)` (unread in 140; it holds the only read of `min-long`). Each unread def still built its value at startup (a `zipmap` costs about 8.8K instructions on Linux, a 6-char set literal about 4.1K) and ran the def machinery (about 2.8K per def).

A Lite program drops a top-level `def` (`dead_defs`) when:

1. its var is stable (§6.6);
2. nothing reads, binds, sets, declares or takes the var as an object (`visit_var_reads`);
3. the program cannot reach vars by name (no `var-get`, `refer`, `load-string`, …; the Lite check already excludes `resolve`, `eval`, `intern`, `ns-publics`, …);
4. its init is pure and total (`is_pure_total`). That means a constant, a collection literal of constants, a long built from long literals, known-long vars and the bit operations, or `(zipmap "text" (range n?))`.

A dropped def's own reads no longer count, so the set grows to a fixpoint: `max-long` frees `min-long`. An unread def whose init has an effect still runs it. macOS empty-input startup: −87K or more for a quarter of the scripts (median −12K). Emit test `native_emit_ir_drops_unread_pure_defs_271`; corpus `dead_defs.clj`.

### 6.13 Slice 4h: generic core calls without the argument buffer

A core fn the compiler neither inlines nor calls directly goes through `rt_n_core_call` (and the `rt_n_op*` fallbacks). That path cloned the arguments into a `SmallVec<[Value; 4]>`, moved the 72-byte buffer through three call layers, and returned a `Step` (`Result<Value, Signal>`, about 60 bytes). On Linux the call cost about 470 instructions; for `identity`, the native's own share was 10. The non-win scripts make about 1300 such calls per case.

A native that takes its arguments borrowed, which is most of them, now gets 1-3 clones in a stack array and writes the result straight to the output slot. A `Signal` is built only on failure, exactly as the general path throws. Owned-args natives (`reduce`, `into`, …) and anything else keep the general path. macOS, per call: `identity` 401 → 304 instructions, `subs` 837 → 737, `name` 608 → 519.

### 6.14 Slice 4i: common callbacks without the native call

Higher-order natives (`mapv`, `filterv`, `reduce`, the lazy `map` / `filter` steps) resolve their fn once (`Prepared`) and call it per item. Two common callbacks paid for a native call that their results did not need:

1. **`seq` as a filter predicate.** `(filterv seq tokens)` drops empty strings after a split, and each call allocated a string seq only to test its truthiness. The lazy `filter` already tests emptiness instead (`filter_pred`: nil, lists, vectors, maps, sets, strings, realized seqs; anything else still calls `seq`), and `filterv` now shares that predicate preparation (`filter_prep`). macOS: 639 → 201 instructions per item.
2. **`inc` / `dec`.** `Prepared::call1` now steps a long inline when the native carries the `inc`/`dec` arithmetic tag, as `call2` already did for the binary tagged ops. Overflow and every other operand still call the native, so its errors and numeric tower stay exact. macOS: `(mapv inc v)` 404 → 261 instructions per element, the lazy `map` and `reduce` over it alike.

### 6.15 Slice 4j: a fresh hot-text order, from generic workloads only

Linux AOT links apply `rust/hot-text-aot.ld` (embedded in clj-aot): a linker script listing the runtime functions that typical programs execute, which the linker packs into one `.text.hot` section. The kernel faults file-backed text in 64 KiB windows, so a run touches a few windows of the hot section instead of most of the runtime's 5-6 MB of code. Code pages dominate a short program's peak RSS.

The order names functions, so it goes stale as runtime code moves: renamed or new functions fall back to the cold `.text`. The committed order (generated from the 30 workloads in `scripts/bench/hot-text` in Slice 3c, and last regenerated for Slices 3k-3n) had drifted, and MEM wins fell from 1372 (b4a57992) to 1280 (90509c29).

**Training set.** The order is trained only on `scripts/bench/hot-text/workloads` and `aot-workloads`: 30 small generic programs (map-assoc, regex, sort, strings, stdin readers, …) committed in #246, before this goal. It is never trained on the benchmark snippets or their test inputs. AOT will run other code bases, and an order fitted to the measured programs would not carry over.

**Measured** with the m-hu fork probe (151 programs, empty stdin, RSS at exit, the same runtime with four orders):

| Order | Median RSS | vs committed | Projected aot-ocaml MEM (win/tie/loss) |
|---|---|---|---|
| committed (stale) | 2936 KB | — | 1280/408/55 (= h2h 90509c29) |
| none | 3464 KB | +512 KB (worse for 151/151) | 297/869/577 |
| regenerated from the workloads | 2436 KB | −448 KB (better for 151/151) | 1701/32/10 |
| trained on the snippets (rejected) | 1940 KB | −960 KB | 1729/6/8 |

The projection adds each probed program's RSS change to its h2h 90509c29 cases, and the median change to the rest.

**Tooling.** `scripts/bench/hot-text/regen-aot.sh RUST_WORKSPACE [OUT.ld]` works as follows:

1. Builds the revision's runtime with a blank order.
2. Compiles each workload with `clojure-rs build` and profiles it under callgrind (stdin from `NAME.in`, else empty).
3. Lists the functions they run with `hot_text.py`, the generator of the interpreter's `rust/hot-text.ld` and of the committed AOT order: the functions most workloads run come first, ties in the order the profiles first ran them. (Until f955bd39 a separate `aot_order.py` packed 62 KiB windows of co-used functions; see the second correction below.)

Ties break in a fixed order, so every job that links the order gets the same one. `run.sh --aot` delegates to the script.

**Correction after the first CI runs.** The probe above ran on empty stdin, so it measured mostly startup. With real inputs, the h2h for f9a16ff5, the first run with a CI-regenerated order, regressed instead: MEM 1280/408/55 → 1046/597/100, and AOT median peak RSS rose 116 KB.

The runtime changes in between were not the cause: on macOS, which has no layout order, peak RSS on the same inputs fell (median −80 KB). A same-runtime probe on empty stdin still ranked the CI order 192 KB better than the committed one.

The first suspect was the generator's name filter. `aot_order.py` kept only Rust-mangled names, so it dropped the runtime's `rt_*` C-ABI entry points (`rt_n_core_call`, `rt_n_drop`, `rt_n_deref`, `rt_n_core_own`, …). Every compiled program calls these while it processes input, but empty-stdin runs barely touch them. The committed order (from `hot_text.py`) listed them. f955bd39 kept `rt_*` names alongside Rust ones.

**Second correction.** That was not the whole cause. The h2h for f955bd39 (run 44411733078) still gave MEM 1024/616/103 and an AOT median of 2992 KB, against 2876 KB with the committed order at 90509c29. Per case, the shift from the committed order is broad, not a constant: median +116 KB, 5th to 95th percentile −160 to +368 KB. 300 cases that won MEM at 90509c29 no longer win.

Both orders come from the same 30 workload profiles. The one remaining difference is the layout: the committed order lists functions by how many workloads run them (`hot_text.py`), while `aot_order.py` packed 62 KiB windows of co-used functions. `regen-aot.sh` therefore lays out with `hot_text.py` itself, and `aot_order.py` is gone. The next h2h checks the result.

**Third correction: coverage, not layout.** With `hot_text.py` itself, the h2h for fdc5a900 gave MEM 1037/612/94 (AOT median 2976 KB). After the next runtime slices, dd228091 dropped to 627/857/259 (3176 KB), while the master leg without an order stayed at 7196 KB. On macOS, which links no order, the worst-hit scripts used the same or less RSS at dd228091. The jump was text pages, not heap.

Two fork probes on Linux (generic programs, no snippet inputs) settled the cause:

- **Revision by order.** On the i259 corpus programs, dd228091 with a fresh order (2968 KB median) matched 90509c29 with its own orders (2996 fresh, 2968 committed). Code growth alone cost about 120 KB (3680 → 3800 KB with no order).
- **Held-out training.** Orders trained on the workloads alone, and on the workloads plus half of the corpus, were measured on the other half of the corpus, which neither saw. The wider training cut median peak RSS by 508 KB there (p10 −576, p90 −256). It cost 62 KB on the workloads themselves.

The snippets run runtime paths the 30 workloads never reach. Each such function sits in the cold text and faults in its own 64 KiB window. `regen-aot.sh` now also profiles the compiler's test corpus (`docs/spec/i259-native-compiler/corpus`, empty stdin): generic programs, still never the benchmark snippets or their inputs.

**CI keeps both orders fresh.**

- The h2h workflow's `hot-text` job regenerates each side's interpreter order (`rust/hot-text.ld` via `regen-run.sh`, generic 26 workloads) and AOT order (`rust/hot-text-aot.ld` via `regen-aot.sh`) once, using this run's tooling and workloads, and the 32 measure shards link with them. (At ae942dbd, the stale committed `rust/hot-text.ld` had let `rs-from` drift +526 KB above `rs-to`, tripping 6 `recipes` MEM losses against CPython.)
- `rust-release.yml` regenerates both orders before the Linux release build, so shipped binaries never carry a stale order.
- A failed regeneration keeps the committed order.
- The committed copies only serve local builds and are refreshed from the same pipeline.

### 6.16 Slice 4k: map destructuring by direct lookups

The AOT lowered every map pattern (`{:keys [a b] :or {…} :as m}`) to `rt_n_destructure`. That call decoded the pattern constant back into a `Pattern` and ran the interpreter's whole destructure in a fresh `Env`, on every bind: about 7.8K macOS instructions for `(fn [s {:keys [a b]}] …)`, against about 700 for the vector pattern `[a b]`.

A plain map pattern now binds by direct lookups, as the interpreter binds it. Plain means: no required (`!`) keys, no `&` key list, and no 1.13 outlets.

1. `rt_n_destr_map` converts the source: a list becomes its map, and anything else is itself.
2. Every `:or` default is evaluated once in the enclosing frame.
3. Each entry and each `:keys`/`:strs`/`:syms` local is read by `rt_n_destr_get`, which uses the interpreter's own `lookup` with the key passed in place from the constant table. A present key wins (a present `nil` stays `nil`). Otherwise the local takes the default keyed by its symbol, then the default keyed by its key form, then nil.
4. `:as` binds the converted source.

Locals follow `pattern_names` order, and nested patterns lower recursively. Patterns that are not plain keep the generic path.

Measured on macOS, instructions per element: `{:keys [a b]}` 7777 → 1069; `{:keys [a b c] :or {c 1} :as m}` 12598 → 1371; `{:strs [a b]}` in a `let` 7580 → 761.

The corpus file `map_destructure.clj` replays the JIT against `run` across every source shape: lists, records, vectors, strings, sets, numbers and nil. It also covers present `nil`, both kinds of `:or` key, eager defaults, nested patterns, and required keys.

### 6.17 Slice 4l: lazy steps walk chunks without copying them

`step_head` serves every lazy op that consumes its input one item at a time: multi-input `map`, `take-while`, `drop-while`, `drop`, `take-nth`, the `partition` family, `distinct`, `reductions`, `interleave`, `sequence` and `cycle`. It used to re-chain the rest of each stepped chunk as a fresh copy (`chain_tail`), so walking a 32-item chunk item by item copied about 16 values per item and allocated a vector for each.

The rest of a chunk is now a forced cursor cell sharing the chunk: `SeqState::Chunk { items, pos, tail }`, the cell `rest` already uses (the JVM's `ChunkedCons`). `step_head` peels a cursor cell in place without forcing it. Values, laziness, `realized?` and the seq classes are unchanged (the remainder keeps the `LazySeq` class the re-chained copy had), and `run`, AOT and the previous binary agree on every op above.

Measured on macOS, instructions per element over a 100-item vector:

| Op | Before | After |
|---|---|---|
| `(partition 2 v)` | 6509 | 4285 |
| `(partition-all 3 v)` | 5676 | 3335 |
| `drop-while` | 4249 | 1508 |
| `take-while` | 6311 | 3551 |
| `distinct` | 7474 | 4694 |
| `(map + v v)` | 10784 | 5199 |
| `interleave` | 12662 | 7071 |
| `reductions` | 6634 | 3878 |

### 6.18 Slice 4m: keyword map literals share a shaped template

A map literal lowered every key into a value slot. It then went through `map_pairs` (a pairs vector, `realize_key` per key) and `from_kvs_last_wins` (a dedupe scan plus separate key and value vectors), then `from_parts`, which probes the shape registry. That is about 1.5K macOS instructions more than a two-element vector literal for `{:x x :y y}`, the usual record-like map.

A literal without metadata whose keys are 1 to 8 distinct constant keywords now lowers to `rt_n_kw_map`. The call takes a `{k nil …}` template constant (rebuilt at startup like any constant map, so it holds the interned shape) and the evaluated values, in order. `Map::with_vals` shares the template's shape `Arc` and stores only the values. Keys, iteration order, equality, class (`PersistentArrayMap`), and the order in which values evaluate are unchanged. Any other literal keeps the pairs path.

Measured on macOS, instructions per iteration: `(count {:x i :y i})` 2098 → 652 (a two-element vector literal is 609); four keys 3205 → 785; `(:x {:x i :y i})` 2148 → 697.

### 6.19 Slice 4n: `aget` and `aset` as inline ops

`(aget a i)` and `(aset a i v)` went through the generic core call: the arguments were cloned into a buffer, and the native's `Result` plumbing came back. That cost about 740 macOS instructions per element of an `aset` loop. Both are now inline ops (`APPLY2`/`BINARY` "aget", `APPLY3`/`TERNARY` "aset"), so the AOT lowers them to `rt_n_op2`/`rt_n_op3` and the VM to its op steps.

- `aget` reads an in-bounds long index under one lock, with `array_read`'s slot mapping.
- `aset` stores only the unambiguous cases: an in-bounds long index, with a `long`, `int` (in range), `double`, `boolean` or `char` slot taking its own value type.
- Every other shape returns `None`, so the native gives its exact answer or error: negative or out-of-bounds indices, coerced indices, `int` overflow, mismatched value types, object and nested arrays, nil.

The corpus file `arrays_inline.clj` pins every case against `run` and against the previous binary.

Measured on macOS, instructions per element:

| Loop | Before | After |
|---|---|---|
| `long-array` `aset` loop | 738 | 291 |
| `double-array` `aset` loop | 746 | 298 |
| `int-array` sum by `aget` | 644 | 267 |
| a DP step `(aset a i (+ (aget a (dec i)) i))` | 1331 | 509 |

### 6.20 Slice 4o: identical collections compare without a walk

Value equality on maps, sets, vectors and lists walked every entry, even when both sides were the same collection. `swap!` publishes only when the atom still holds the value it read (`state.value != old`), so every swap on an atom holding a collection walked the whole collection. The common memo and accumulator pattern `(swap! cache assoc k v)` was quadratic: 2000 swaps into a growing map cost 721M macOS instructions, about 360K per swap at the end.

Equality now answers true first for the same collection (`Arc::ptr_eq`), as the JVM's `Util.equiv` checks `k1 == k2` before anything else. The same 2000 swaps cost 14.5M (500 cost 5.2M: linear). Clojure's `=` goes through its own `equiv` and is unchanged; the shortcut serves internal equality: atom publishes, map keys and set members.

### 6.21 Slice 4p: eager walks fuse `mapcat` (`for` over several bindings)

`for` with several bindings expands to a `mapcat` over inner `map`s, and so does a `mapcat` call. Every element went through lazy cells even when an eager consumer was about to walk the whole seq. `for` is used by 47% of the scripts that lose by more than 2×, against 13% of all-win scripts. A sole unforced `map` was already walked without its cells (`walk_sole_map`).

The fused walk now also takes a sole unforced `mapcat` whose outer `map` is sole and unforced. It walks the outer `map` as before, then each of its results in turn:

- nil, lists and vectors item by item (as `splice_realized` splices them);
- a sole `map`/`mapcat` fused again, recursively;
- anything else unit by unit through its seq.

These are the steps forcing the cells runs, in the same order. A visitor's stop ends the whole walk.

The fusion only applies where the walk owns the seq, so nothing else can see the cells. Its consumers are the `walk_seq` callers: `reduce`, `into`, `mapv`, `vec`, `some`, `every?`, `not-any?`, `frequencies`, `group-by`, `doall`, `dorun` and `run!`. The corpus file `for_fused.clj` pins items, side-effect order and counts (including chunked over-evaluation before an early stop), errors, and reuse of a held seq against `run` and the previous binary.

Measured on macOS, instructions per element:

| Consumer | Before | After |
|---|---|---|
| `reduce` over a 2-D `for` | 2135 | 1147 |
| `into []` over a 2-D `for` | 2204 | 1221 |
| `vec` over a 2-D `for` | 2637 | 1586 |
| `some` over a 2-D `for` | 2340 | 1354 |
| `(reduce + (mapcat …))` | 979 | 862 |

`count` keeps the cells for now: its AOT slow path passes the seq borrowed, and a borrowed seq may be used again.

### 6.22 Slice 4q: calls through a value

A call whose head is not a known var, such as a local closure or a fn argument (`(f x)` inside a higher-order fn), went through `rt_n_invoke` and the general `Evaluator::call`. That path ran a VM-closure hook, a struct check and two downcasts, then `call_code`'s depth guard and evaluator scope, and finally the compiled code. It cost about 4× a direct `defn` call.

1. `native_abi::invoke` calls a native callee's `func` straight away. Compiled closures are natives, and for them the general call reached the same `func` anyway.
2. A compiled closure carries its single fixed arity's entry (`NativeFn::compiled`: code address, captured values, parameter count). At a value call site, the AOT asks `rt_n_enter_closure` for it. That function checks the callee and its arity and performs the same call-depth check as `rt_n_enter`. On a hit, the site calls the code directly with the argument slots and closes the call with `rt_n_leave`, exactly as a `defn`'s direct call does. Every other callee and arity, and the depth error, keep `rt_n_invoke`.

The corpus file `closure_calls.clj` pins captured values, nested and returned closures, multi-arity, variadic and wrong-arity calls, non-closure callees (keywords, maps, sets, vectors, natives, nil), exceptions crossing the call, and the `StackOverflowError` of deep recursion through a local fn, against `run` and the previous binary.

3. As a `defn`'s direct call does, plain locals ride in place. A local head is called where it lives, and local arguments are bitwise aliases in the argument array (the callee only reads its args), cleared after the call. A local moved by its last use drops after the call. Moves never target captured values, so nothing the enclosing closure owns is released.

Measured on macOS, instructions per call:

| Call | Before | Direct entry | + in-place locals |
|---|---|---|---|
| local closure in a `conj` loop | 710 | 587 | 478 |
| a fn parameter called per item | 477 | 354 | 245 |
| a captured-adder closure | 475 | 353 | 245 |

### 6.23 Slice 4r: scalar loops keep exits over outer locals

The scalar loop compiler (`vm_scalar`, shared by the VM and the AOT) gave up on the whole loop when an exit value had no scalar form. The common case is a loop that fills a transient and returns it persistent:

```clojure
(let [pi (transient (vec (repeat m 0)))]
  (loop [i 1 j 0]
    (if (>= i m)
      (persistent! pi)
      (do (assoc! pi i j) (recur (inc i) …)))))
```

That exit misses on every run, and the AOT does not lower a loop with transient effects that may deopt (a resumed iteration would replay its effects). So the loop stayed boxed, and every `nth`, comparison and `assoc!` in it went through boxed values.

An exit now runs boxed when the loop ends (`ExitVal::Boxed`) if all of these hold:

- its scalar form always misses;
- it is the outer loop's own exit, not an inner loop's value;
- it reads only locals bound before the loop: literals, vars, calls, collection literals, and `if` over such forms. An exit that reads a loop variable or a body binding keeps its scalar form.

The AOT lowers such an exit as ordinary code after the scalar loop. The effects it can observe already happened in place, in order. The VM's scalar loops deopt at that exit as before, and the boxed loop evaluates it.

The corpus file `scalar_boxed_exits.clj` pins `persistent!`, `count`, map and vector exits, an exit that reads the loop variable, and a throwing exit, against `run`.

Measured on macOS: KMP substring search (`string_related/kmp/substring_searching`) fell from 163M to 67M instructions on one input; its prefix-table loop is now scalar.

### 6.24 Slice 4s: lookups without the general call

Three lookup paths still went through generic machinery that the lookup itself never needed:

1. `(:k coll)` and `(:k coll nf)` with a literal keyword try the bitcode `get`, which answers nil, vectors, strings and shaped maps with scalar keys. Every other shape (a hash map past 16 entries, a record, a set) fell back to the keyword's own invoke through `rt_n_invoke` and `Evaluator::call`. That invoke is the interpreter's `lookup` with the default (`invoke_keyed_name`), so the fallback now calls `rt_n_destr_get`, the same `lookup` with the default, directly.
2. A call through a value (`(m k)`, `(s x)`, `(k m)` with the callee in a local) that misses the compiled-closure entry now tries `rt_n_lookup_call` before `rt_n_invoke`. A map, set, keyword or symbol callee with one argument, or two with a default, answers there with the lookup `Evaluator::invoke` ends in (`lookup`, `member`). Any other callee or arity takes `rt_n_invoke`, whose `native_abi::invoke` makes the same shortcut for its other callers.
3. The inline `get` and `contains?` ops (VM and `rt_n_op2`/`rt_n_op3`) answered unsorted maps and sets only for scalar keys, so a tuple key such as `[r c]` or a double paid the generic core call. They now accept any key except an extension value. The natives realize only a pending seq key (an extension) before the same `Map::get`, `Map::contains_key` and `Set::contains`, so the answers are the natives' own.
4. A shaped map finds a keyword key by address: keywords are interned and equal only to themselves, so this is `Value::eq`'s answer without its dispatch per key.

Measured on macOS, instructions per element: `(:k m)` on a 40-key hash map 713 → 489; a 4-key map called as a function 690 → 520.

The corpus file `lookup_calls.clj` pins keyword, map and set calls with and without defaults on shaped and hash maps, records, sets, sorted maps, nil, vectors and strings, tuple, double and lazy-seq keys, and the arity errors, against `run`.

### 6.25 Slice 4t: core-var reads by name address

Printing reads about ten `clojure.core` vars per call: `*out*` three times, six `*print-*` settings, `*flush-on-newline*` and `*print-dup*`. `Evaluator::core_read` cached each name's var id and root per lineage epoch, but found a name by scanning the cache, which by then held some 50 names. The scan cost every `println` and every `format` about 2.2K Linux instructions (`deref_core` in the probe below). `print` also resolved `*print-readably*` by name through the registry for its binding on every call.

The cache now keeps a 64-way direct-mapped table from the name's address to its position. Call sites pass their own literal, so a probe hits without a scan; a collision falls back to the scan. `push_core_binding` takes its var id from the same cache. Ids and roots still live only as long as the lineage epoch, as before.

### 6.26 Slice 4u: longs and doubles compare without the numeric tower

`compare`, `sort-by`, sorted collections and vector comparison order numbers through `jvm_compare`. Two longs took the exact path, which converted both to `Ratio<BigInt>` (heap digits) and cross-multiplied them on every compare. Over the 348 tiny-input non-win cases, the instrumented runtime counted 583K such compares and 1.17M rational conversions.

`jvm_compare` now orders long and double pairs first, exactly as `compare_numbers` does: longs exactly, and a double on either side as `f64` with `<`/`>` (`NaN` ties). Every other pair keeps the tower. A unit test checks the fast path against `compare_numbers` over longs, doubles, signed zeros, infinities, `NaN` and the long extremes.

Measured on macOS, instructions per element: `(compare i 50)` 761 → 418; `sort-by` on a key 1584 → 1183; sorted-set inserts 12297 → 10482.

### 6.27 Slice 4v: scalar prints in one write

`print`, `println`, `pr` and `prn` validated the print options, pushed a `*print-readably*` binding frame (a fresh `Var`, a frame vector and a count-map entry, all undone after), and then dispatched every argument through `pr-on`, the static `print-method` and `scalar_default`. That path re-read the options and the writer, and wrote each argument, separator and newline separately.

When every argument is nil, a boolean, long, double, string, char or keyword, the booted print methods are in place (`print_defaults_only`) and `*print-dup*` is off, `scalar_default` renders each argument as `pr_to_string` under the ambient options: none of these carries meta. Those calls now build the line once from the validated options (`readably` already carries the `print`/`println` override) and write it in one emission, under the same `*out*` check, emission lock and `*flush-on-newline*` flush. Any other argument, a live `print-method`, `*print-dup*`, or a call with no arguments takes the general road.

The corpus file `print_scalars.clj` pins every scalar shape (unicode, `NaN` and infinities, readable strings and chars), `with-out-str`, the `*print-readably*`, `*print-dup*` and `*print-length*` bindings, a nil `*out*`, empty calls, and a user `print-method`, against `run` and the previous binary.

Measured on macOS (kernel `write` included): `println` of a long 22.4K → 18.2K instructions; a two-argument `prn` 25.2K → 19.2K; `println` into `with-out-str` 13.4K → 9.4K.

The line is one buffer presized for the string arguments, and each argument renders into it through `pr_into`. A first version rendered each argument to a temporary and copied it in, so `println` of a 1.27 MB string cost 1.57M macOS instructions instead of 0.19M; it now costs 0.25M. Readable strings also copy the runs between escapes whole: every `CHAR_ESCAPES` entry is ASCII, so a byte scan finds the same cuts the per-char table lookup did. `prn` of that string went from 49.9M to 41.7M (it escapes a newline every 6 bytes).

### 6.28 Slice 4w: unchecked arithmetic, `deref`, `char` and `compare` inline

A core call with no inline op goes through `core_fallback`: the evaluator, the var's entry, cloned arguments and the native. That is about 240 Linux instructions even for `identity` (probe `identity_call`). A trace of these generic calls over the 348 tiny-input non-win cases counted 210K. Some of the most frequent were trivial: `unchecked-multiply` and `unchecked-add` (38.6K each, a pseudo-random generator), `deref` (14.3K, `@atom`), `char` (2.8K) and `compare` (1.7K).

These are now VM inline ops, which the AOT's slow path reaches through `rt_n_op1`/`rt_n_op2` before any generic call:

- `unchecked-add`, `-subtract`, `-multiply`, `-inc`, `-dec` and `-negate` over longs wrap. Doubles and the tower stay the natives'.
- `deref` of an atom or a volatile reads the cell under its lock, as the native does. Refs, delays, futures, promises, vars and the timeout arity stay the native's.
- `char` of a long code point narrows through `char::from_u32`. Out-of-range values (errors) and other shapes stay the native's.
- `compare` of longs and doubles answers -1, 0 or 1 from `jvm_compare`'s numeric order. Other operands stay the native's.

The corpus file `core_inline_ops.clj` pins the wrapping edges, double operands, every derefable, the `char` range errors and mixed compares against `run` and the previous binary.

### 6.29 Slice 4x: sorts call their functions directly

`sort` with a comparator and `sort-by` made a checked general call (`check_ifn`, then `Evaluator::invoke`) for every key function and comparator call: three per comparison for `sort-by`. On Linux, `(sort-by first ts)` over 50 tuples cost 351K instructions.

- The key function and any comparator go through their prepared entry (`vm::Prepared`: a native, or a closure of that fixed arity). Keywords, maps, vars and other callees keep the checked general call.
- The booted `compare` (the native itself; a rebound var holds something else) orders through `jvm_compare` directly.
- The key function still runs twice per comparison and the comparator once, as on the JVM. The corpus pins the call counts.
- `first` of a vector and `compare_vectors` read items by index instead of building a trie iterator per call.

The corpus file `sort_calls.clj` pins key functions (natives, closures, keywords, `juxt`), boolean and three-way comparators, var comparators, the error cases and the call counts against `run` and the previous binary.

Measured on macOS, instructions per element: `sort >` 5665 → 2464; `sort-by` on a keyword key 1183 → 800; `sort-by first` on tuples 8442 → 4763; `sort-by` with a closure key 1502 → 1078.

### 6.30 Slice 4y: collection printers render scalar children directly

A printed vector, list, set or map dispatched every child through `pr-on`: `*print-dup*` read, method lookup, then `scalar_default`, which re-read all six print settings, cloned the child and wrote it. Each child and each separator was a separate write. On Linux, `prn` of a three-item vector cost 15.2K instructions, 4K of them in core-var reads.

Under the booted print methods with `*print-dup*` off, a scalar child (nil, a boolean, long, double, string, char or keyword) now renders from the parent's options through `pr_into`. That is `scalar_default`'s own answer: no meta to prefix, and levels and lengths never apply to a scalar. The collection's text gathers in one buffer, flushed before any child that prints itself (nested collections, records, seqs), so the output order and every other path are unchanged.

The corpus file `print_collections.clj` pins nested shapes, records, sorted and namespace maps, `*print-length*`, `*print-level*`, `*print-meta*` with meta, `*print-dup*`, `*print-readably*`, `with-out-str`, and a user `print-method` that turns the direct path off, against `run` and the previous binary.

Measured on macOS, instructions per call: `prn` of a 5-item vector into `with-out-str` 31.1K → 17.9K; `prn` of a map 39.5K → 29.1K (kernel `write` included).

### 6.31 Slice 4z: print settings read once per binding state

Even on the scalar path, a `println` still read nine core vars: the six behind `print_opts`, plus `*print-dup*`, `*out*` and `*flush-on-newline*`. On Linux that was about 950 of its 2690 instructions (`deref_core` and the cache slot lookup). Each collection level read the six again.

The evaluator keeps a `PrintSnap`: the validated print options, `*print-dup*`, `*out*` and `*flush-on-newline*`, stamped with the lineage epoch and the binding version it was read under. Every binding push, pop and `set!` bumps the version, and every root write bumps the epoch, so a matching stamp means the vars still hold those values. `print_opts` and the scalar print path take the snapshot. A stale stamp reads afresh and keeps the result. An invalid `*print-length*` or `*print-level*` is never kept, so it errors on every call as before. `pr-on`'s own `*print-dup*` read is unchanged, so a user `print-method` that ignores the print options sees no new error.

Measured on macOS, instructions per call: `println` of a long 18.0K → 16.6K; `prn` of a 3-item vector 25.6K → 23.5K (kernel `write` included). A print inside `with-out-str` rebinds `*out*` each time, so it misses: 9.2K → 9.5K. No snippet uses `with-out-str`.

### 6.32 Slice 5a: generic core calls resolve their native once

`core_fallback`, behind `rt_n_core_call` and the `rt_n_op*` misses, resolved var `k`'s native entry and offered it the arguments as a borrowed stack array. A native with an owned-args entry (`range`, `repeat`, `concat`, `map` …) refused that array. Its clones were dropped, the arguments were cloned again into a buffer, and a second `with_active` resolved the entry a second time before the call.

`CoreEntry::call_refs` now takes the resolved entry once. An owned-args native gets its clones by value directly; a borrowing one gets them in a stack array (one to three args) or a buffer.

The re-resolving fallback is gone. Direct core calls are emitted only for programs that cannot rebind core vars; a rebinding program calls each var's current root (`core_rebind.clj`). So var `k` always holds the native the analyzer saw, and `core_fallback` states that invariant (`expect`). Undetected rebinding already broke the inline ops' answers.

Measured on macOS, instructions per call: `(range 3)` 1680 → 1510, `(repeat 2 :x)` 1750 → 1600, `(concat a b)` 9360 → 9190 (owned natives); `subvec` (borrowing) 1170 → 1140.

### 6.33 Slice 5b: natives call compiled closures over their own argument array

`Prepared::call1`/`call2` (the step calls of `map`, `filter`, `mapv`, `reduce`, `sort-by` …) reached a compiled closure through `call_native`. That built a 144-byte `SmallVec` argument buffer and moved it by value through the owned `dyn Fn` entry into `guarded`'s closure. On Linux those moves were out-of-line `memcpy` calls: 126 instructions per element of `(count (for [x (range n)] x))`.

`Prepared` now calls a compiled closure whose single fixed arity matches through `InPlaceCall`: its code (`CompiledEntry::code`, now a typed `CodePtr`) over an argument array the caller owns. `Prepared` passes one or two args as a stack array (a slice). The code may take its params from the slots, exactly as on the owned path (`TAKE_ARGS`), and the caller drops what is left. The native runtime registers the one `InPlaceCall` (it publishes the evaluator to compiled code), so no closure is allocated per compiled fn. At startup each allocation costs about 190 Linux instructions in glibc's slow path. Other natives, other arities, VM closures and owned calls (`update`, `reduce-kv`, transducers) keep their paths. The corpus file `native_closure_calls.clj` pins both paths, their exceptions and arity errors against `run` and the previous binary.

Measured on macOS: `sort-by` with a closure key 1077 → 877 instructions per element. On macOS the buffer moves are inline copies, so elsewhere the gain shows only on Linux.

### 6.34 Slice 5c: zero-closure native dispatch, bulk fn tables, and lazy Lite helpers

A Linux callgrind startup profile (`probe-initline`, 34 near-miss scripts vs trivial) put per-script startup at ~300 `malloc` calls (~48K glibc instructions) plus ~19K self instructions in `rt_n_main_lite`: `install_native_rows` (58.9 mallocs), `rt_n_declare_fn` (33.9), `Symbol::new` (30.4), `rt_n_make_fn` (27.4), and 19 repeated linear scans of declared vars in `boot_lite_impl`.

- **`NativeTarget` / `OwnedTarget` / `DisplayName` (`clj-eval::ext`):** table-driven natives carry bare function pointers (`NativeTarget::Ptr` / `OwnedTarget::Ptr`) instead of allocating `Arc<dyn Fn>` wrappers; compiled AOT closures carry `(desc, captured)` dispatched through one process-wide `CompiledDispatch` table and slice their display name (`DisplayName::Span`) straight from `Tables::fn_names`. `rt_n_make_fn` drops from 3 allocations to 1 (no per-fn closures or display string), and direct core-call resolution (`lite_entry`) allocates nothing.
- **Bulk `rt_n_declare_fns` (`clj-aot`, `clj-rt`):** mirrors `rt_n_declare_vars`, declaring every compiled fn's display name span and arities slice in one table call and pre-sizing `b.consts`; `rt_n_set_const` moves constants out of the builder slot instead of cloning and dropping them.
- **Single-pass Lite boot + lazy `LITE_INTERNAL_HELPERS` (`clj-eval::aot_boot`, `clj-rt`):** `boot_lite_impl` scans declared vars once (`scan_lite_vars`), reserves tight `clojure.core` capacity when reflection vars are absent, and defers the 13 `LITE_INTERNAL_HELPERS` until `Evaluator::core_read` actually misses on one. `run_main_lite` passes `&[Code]` and moves `BUILDING` vectors without cloning.

Measured on macOS (mimalloc, `libinsn.dylib` shim): median empty-stdin startup across 12 near-miss scripts −82K instructions (larger on Linux glibc); closure creation + call −391 instructions per capturing closure (17.07M → 13.16M for 10K) and −388 per capture-free closure (13.48M → 9.60M).

### 6.35 Slice 5d: hoisted flat int-vector and locked transient pointers in scalar loops

Scalar `loop`/`recur` functions over integer vectors and transient DP tables already lowered their arithmetic unboxed (`clj-aot/src/scalar.rs`), but every `(nth v i)` and `(assoc! t i x)` still called `rt_bc_scalar_nth` / `rt_bc_scalar_assoc`, re-acquiring `TransientLock` and re-dispatching `Value` tags per iteration (e.g. `dp/knapsack/solution` spent 295.7M Pin instructions vs OCaml's 50.0M).

- **Zero-`unsafe` packed-int slice & raw transient lock (`clj-value`, `clj-bc`):** `Vector::as_flat_ints` exposes `Packed::Flat<i64>` directly (`rt_bc_scalar_ivec`); `TransientLock::lock_ints_ptr` acquires the existing spinlock for a valid non-empty packed-`Int` `TransientVector`, retains it via `std::mem::forget(guard)` (zero new `unsafe`), and pairs with `TransientLock::unlock_raw` (`rt_bc_scalar_tvec_lock` / `rt_bc_scalar_tvec_unlock`), alongside `rt_bc_scalar_same_transient` for preheader aliasing checks.
- **Preheader hoisting & single-compare bounds checks (`clj-aot/src/scalar.rs`):** `AstScan` separates growing mutations (`conj!`/`pop!`) from in-bounds updates (`assoc!`) and skips probes a loop does not read. Read-only flat `Int` vectors hoist `(ptr, len)` once; unaliased non-growing `TransientVector`s lock once in the preheader (also caching `count`). Setting inactive `str_len`/`ivec_len` to `0` collapses each fast `nth` and `assoc!` check to one unsigned `idx < len` compare plus a direct `i64` load or store. Every miss, slow call (before any potential throw), deopt, and outer exit unlocks all locked transients and zeroes their cached lengths cleanly.
- **Verification:** `docs/spec/i259-native-compiler/corpus/scalar_hoisted_vecs.clj` pins flat int vectors, nested knapsack DP, aliased transients, append/type-widening fallback, read-only transients, OOB throws, and overflow deopt against `run`; `native_emit_ir_hoists_int_vecs_and_locks_transients_271` pins the IR.

Measured on macOS (`libinsn.dylib` shim): `dp/knapsack/solution` drops 290.4M → 61.2M instructions (0.211x, −229.2M).

### 6.36 Slice 5e: lazy combinator fusion, `count` fusion, and `Double` fast arithmetic in `Prepared`

Eager consumers (`vec`, `into`, `reduce`, `some`, `mapv`, `count`) only fused sole unforced `Thunk::Map` and `Thunk::MapCat` seqs (`lazy.rs::walk_sole_map`). Pipelines using `filter`, `remove`, `take`, `concat`, `repeat`, or `count` still allocated `PendingSeq` cells and `Arc<[Value]>` chunks per step (e.g. `binomial_tree`'s `(vec (concat nvals (repeat k 0.0)))`), and `walk_map_vectors` / `Prepared::call2` fell back to general `eval.call` / `native_add` for 2-vector `map` and `Double` arithmetic (`backprop`, `policy_iteration`, `value_iteration`).

- **`Prepared` `Double` & mixed `Double`/`Int` fast arithmetic (`clj-eval::eval`, `clj-eval::vm`):** `Evaluator::fast_double2` handles binary `+`/`-`/`*`/`/`/`<`/`<=`/`>`/`>=` on `Double` pairs and `+`/`-`/`*`/`/` on mixed `Double`/`Int` pairs; `Prepared::call1` handles `inc`/`dec` on `Double`.
- **Extended `SoleMap` fusion (`clj-eval::lazy`, `clj-eval::coll`):** `sole_map_parts` / `walk_parts` now fuse sole unforced `Thunk::Filter`, `Thunk::Remove`, `Thunk::Take`, and iterative `Thunk::Concat`; `walk_all` walks `SubVector`, finite/infinite `RepeatValue`, and counted `LongRange` directly; `walk_map_one` and `walk_filter` walk `Vector` and `LongRange` inputs in stack-buffered 32-item chunks preserving exact chunked forcing and side-effect order; `walk_map_vectors` specializes 2-vector inputs via `Prepared::of(f, 2)`; `coll::walk_count` enters `walk_sole_map` before stepping seq cells.
- **2-input `mapv` fast path (`clj-eval::seqs`):** `native_mapv_owned` walks `(mapv f vec coll)` directly through `Prepared::of(f, 2)` without allocating an intermediate lazy `map` cell.
- **Verification:** `docs/spec/i259-native-compiler/corpus/lazy_fusion.clj` pins chunking/side-effect order under early termination, nested `concat`, infinite `repeat` under `take`, mixed `Double`/`Int` ops, `count` fusion, 2-input `mapv`, and error cases against `run`.

Measured on macOS (`libinsn.dylib` shim, vs Slice 5d): `binomial_tree` 20.64M → 7.94M (0.385x, −12.70M), `backprop` 8.45M → 7.08M (0.839x, −1.36M), `policy_iteration` 6.94M → 6.32M (0.911x, −619K), `value_iteration` 6.56M → 6.16M (0.939x, −403K).

### 6.37 Slice 5f: `PackedStrings` vector storage, direct `clojure.string/join`, and zero-copy full-string `subs`

On `30f6ab42`, `aot-ocaml` MEM jumped to `1499/7/237` strict (`551/1163/29` at ±15%, AOT median RSS `2,760 KB` vs OCaml `3,032 KB`), leaving `data_structure/disjoint_set/connected_component` (`test_case_7..9`, 23.3–27.9 MB vs OCaml 5.5–6.3 MB, up to `4.88x`) as the sole >2x MEM outlier. Its `render-chunk` collected ~250K short strings into a `TransientVector` (`Generic(Vec<Value>)`, ~16.8 MB of 32-byte `Value` slots plus ~8 MB of individual `Arc<str>` heap blocks) before passing the frozen vector to `clojure.string/join`.

- **`PackedStrings` (`clj-value`):** flat `buf: String` plus `ends: Vec<u32>` (`VectorInner::Strs(Arc<PackedStrings>)` and `TransientItems::Strs(PackedStrings)`), with 1.5x growth past 256 KiB and copy-spike-free `shed_slack`. `TransientVector` stays `Generic` below `PACK_STR_MIN = 256` (keeping small string vectors zero-alloc on `nth` / `rt_bc_2_nth`) and compacts once at length 256 when every item is a string; non-string items, in-place `assoc!`, or shared updates past `FLAT_MAX` convert cleanly to `Generic`. `ValueRef::Str(&str)` preserves exact in-bounds vs out-of-bounds dispatch in `clj-bc` without allocating in bitcode.
- **Direct `clojure.string/join` & `apply str` (`clj-eval`, `clj-value`):** `join_vector_buf` joins `Vector::as_packed_strs()` directly from the flat buffer in one pass without boxing items, and `Vector::push_to_string` copies `raw_buf()` directly.
- **Zero-copy full-string `subs` (`clj-eval::core`):** `(subs s 0 (count s))` clones the `Arc<str>` handle instead of copying multi-megabyte input windows.
- **Verification:** `docs/spec/i259-native-compiler/corpus/packed_strings.clj` plus `clj-value` and `clj-bc` unit tests pin every vector/transient/seq/equality/hash/join/subs operation against `run`.

Measured on macOS (`connected_component`, vs Slice 5e): `test_case_7` 34,624 → 23,152 KB (−11.5 MB, 0.669x), `test_case_8` 37,904 → 27,792 KB (−10.1 MB, 0.733x), `test_case_9` 44,864 → 28,880 KB (−16.0 MB, 0.644x) and INS 748.3M → 716.4M (0.957x).

### 6.38 Slice 5g: per-arg ownership transfer, 2-key `assoc-in`/`update-in` & 3-arg `update` fast paths, atom `swap!` in-place, slot recycling, and `HofLoop` `filter`/`some` fusion

Multi-step state simulations (`shared_memory_mutex`, `dining_philosophers`, `max_density_subgraph`, `q_learning`) lost ownership (`Arc::strong_count > 1`) across function boundaries and `loop`/`recur` back-edges, forcing persistent map/vector copy-on-write on every step, and allocated closures + lazy seqs for per-step `filterv`, `(count (filter ...))`, and `(some ...)` traversals.

- **Per-arg ownership transfer & tail-recur parameter moves (`clj-eval::native_abi`, `clj-aot/src/native.rs`):** `TAKE_ARG_SHIFT = 30`, `single_take_arg_bit(i)`, and `take_arg_mask(i)` encode per-argument ownership in the `n` call word when a compiled caller (`lower_direct_invoke`, `lower_value_invoke`) passes a temporary or last-use moved local; `lower_arity` and `own_consumed_params` move owned symbol parameters directly out of `argv[i]` (`take_or_clone`) and include self-recursive (`owned == true`) parameters in `taken` so last-use moves fire inside tail-recursive `defn`s; `end_fn` moves `result` into `ret` instead of cloning.
- **Destructuring, `let`-on-`recur` drop, and stack-slot recycling (`clj-aot/src/native.rs`, `clj-aot/src/scalar.rs`):** `bind_owned_slot` eliminates redundant slot allocation + `clone_v` on symbol positions in vector/map destructuring; `lower_recur` drops active `let_slots` above `target.let_depth` before jumping to the loop header (releasing extra `Arc` references held by loop-body `let` bindings before the next iteration); `FnCtx` recycles dead temporary `free_slots` and `free_arrays` across expressions (cutting `shared_memory_mutex` `one-step` stack frame from 350+ slots to 8 scalar slots + 3 arrays).
- **Zero-vector 2-key `assoc-in2`/`update-in2`, 3-arg `update`, and in-place atom `swap!` (`clj-eval::vm_ops`, `clj-eval::concurrency`, `clj-rt`):** `lower_direct_moving` unpacks 2-element vector literals in `(assoc-in m [k1 k2] v)` and `(update-in m [k1 k2] f)` into 4-arg `"assoc-in2"` / `"update-in2"` calls without allocating the path vector (`repack_own_args` repacks to 3 args on general fallback); `apply_assoc_in2`, `apply_update_in2`, and `apply_update3` (`inc`/`dec`) mutate uniquely owned outer and inner maps/vectors in place via `Map::take_value` / `Vector::take_nth`; `native_swap` updates unwatched, unvalidated atoms in place (`try_swap_in_place`) for `assoc`, 2-key `assoc-in`, and `+`.
- **`HofLoop` `FilterCollect`, `CountFilter`, `Some`, and owned 1-arg `count` slow path (`clj-aot/src/native.rs`):** `hof_loop` fuses `(filterv F xs)`, `(vec (filter F xs))`, `(into [] (filter F xs))` (`HofKind::FilterCollect`), `(count (filter|filterv F xs))` (`HofKind::CountFilter`, branchless `zext i1` counter increment), and `(some F xs)` (`HofKind::Some`, short-circuit exit on truthy result) inline over vectors and long ranges with zero closure or lazy-seq allocation; 1-arg `count` on a temporary or moved local routes its `rt_bc_1_count` miss through `rt_n_core_own` (`emit_owned_slow1`) so `native_count_owned` fuses lazy pipelines with `Arc::strong_count == 1`.
- **Verification:** `docs/spec/i259-native-compiler/corpus/ownership_in2_hof.clj` plus `clj-eval::vm_ops` and `clj-aot::emit` unit tests pin every fast path and fallback branch against `run`.

Measured on macOS (`libinsn.dylib` shim, vs Slice 5f): `shared_memory_mutex` `test_case_0` 6.19M → 3.17M (−48.8%), `test_case_1` 18.87M → 5.99M (−12.88M, −68.2%), `test_case_2` 18.49M → 6.20M (−12.29M, −66.5%), `test_case_3` 18.39M → 5.93M (−12.46M, −67.7%), `test_case_4` 6.29M → 3.19M (−49.2%); `max_density_subgraph` `test_case_0` 5.98M → 4.80M (−19.8%), `test_case_1` 4.55M → 3.89M (−14.5%), `test_case_2` 12.84M → 9.27M (−3.57M, −27.8%); `dining_philosophers` `test_case_3` 9.62M → 8.65M (−966K, −10.0%).

### 6.39 Slice 5h: zero-alloc `SplitVec` parsing, fused dot product, 2-collection/`every?`/`reduce +` `HofLoop`, and panic-free `PackedStrings` bitcode

Linux `h2h-cpu-mem` on `f6444c2b` (`44592735684`) surfaced a bitcode link error (`core::str::slice_error_fail` pulled into `clj_bc-*.bc` by `&self.buf[start..end]` in `PackedStrings::get_str`), while input-parsing and vector-math benchmarks (`stock_span`, `histogram`, `sliding_window_max`, `kd_tree`, `backprop`, `q_learning`, `approx_inference`) spent tens of millions of instructions materializing per-token `Arc<str>` slices from `clojure.string/split` and allocating closures + lazy seqs for 2-collection `map`/`mapv`, `(reduce + init (map * a b))`, `(reduce + [init] (map F ...))`, and `(every? F xs)`.

- **Panic-free `PackedStrings::get_str` (`clj-value/src/value.rs`):** replaced unchecked slice/index ops with `idx.checked_sub(1)`, `self.ends.get(prev)?`, and `self.buf.get(start..end)`, eliminating `core::str::slice_error_fail` from `clj_bc-*.bc` (pinned by a `clj-aot::emit` regression guard).
- **Zero-materialization `SplitVecValue` pipelines (`clj-value/src/seq.rs`, `clj-eval/src/{seqs,lazy}.rs`):** added `SplitVecValue::filter_non_empty` (O(1) window shift for all-non-empty or single-leading-empty splits; span-only filter otherwise) wired into `native_filterv_owned` for `(filterv seq split-vec)`, and walked `SplitVecValue` directly via `sp.iter()` in `seqs::walk_direct` and `lazy::walk_all_other`.
- **Direct string-to-number `mapv` parsing (`clj-eval::native_abi`, `clj-rt`, `clj-aot/src/native.rs`):** `parse_num_arity` + `emit_parse_num_probe` probe `rt_n_mapv_parse_num` (`try_mapv_parse_num`) for `(mapv #(Long/parseLong|Integer/parseInt|Double/parseDouble %) coll)` over `Vector` (including `PackedStrings`), `SplitVecValue`, and `SubVectorValue`, parsing straight into flat `Int` or `Double` vectors without allocating per-token `Arc<str>`s and falling back to the general loop on malformed tokens or `i32` overflow.
- **Fused dot product & 2-collection / `every?` / `reduce +` `HofLoop` (`clj-eval::native_abi`, `clj-rt`, `clj-aot/src/native.rs`):** `try_lower_dot_product` emits `rt_n_dot` (`try_dot_product`, with exact `dot_ints` and `dot_doubles` including `i64` overflow fallback) for `(reduce + init (map * c1 c2))`; `HofLoop` now supports 2-collection `mapv`/`vec`/`into []`/`dorun`/`reduce +`, short-circuit `HofKind::Every` (`every?`), and `HofKind::Sum` (`(reduce + [init] (map F c1 [c2]))` via `rt_n_add_acc`, preserving 1-element identity semantics when `init` is omitted).
- **Verification:** `docs/spec/i259-native-compiler/corpus/splitvec_dot_hof2.clj`, `clj-value::seq`, `clj-eval::native_abi`, and `clj-aot::emit` unit tests pin every fast path, fallback, and panic-free bitcode invariant against `run`.

Measured on macOS (`libinsn.dylib` shim, vs Slice 5f): `backprop` `test_case_0` 17.72M → 10.45M (−7.27M, −41.0%), `test_case_1` 7.13M → 4.89M (−31.4%), `test_case_2` 11.11M → 7.16M (−35.5%), `test_case_3` 6.04M → 4.43M (−26.8%); `stock_span` `test_case_7` 482.78M → 422.61M (−60.17M, −12.5%); `histogram` `test_case_9` 431.97M → 369.78M (−62.19M, −14.4%); `sliding_window_max` `test_case_7` 1,771.63M → 1,705.87M (−65.76M, −3.7%); `kd_tree` `test_case_6` 352.53M → 336.33M (−16.20M, −4.6%); `q_learning` `test_case_2` 20.20M → 18.84M (−6.7%); `approx_inference` `test_case_1` 11.76M → 10.96M (−6.8%).

## 7. Acceptance Checklist (Issue #271)

- [ ] `h2h-compare` `--allow-extra-challenger` implemented and covered by Go tests.
- [ ] `.github/workflows/h2h-cpu-mem.yml` measures `AOT from`, `AOT to`, and `OCAML` and runs `aot-rsrs` and `aot-ocaml`.
- [ ] Slice 1 CI run completes cleanly and captures Linux Pin `AOT` vs `OCAML` baseline artifacts.
- [ ] Unwaived `h2h-cpu-mem.yml` passes `recipes`, `recipes-rsrs`, `aot-rsrs`, and `aot-run` with 0 MEM and 0 INS losses at `--margin 0.15`, and `aot-ocaml` shows AOT outperforming OCaml on every compared case (wins only on MEM and INS).
- [ ] All CI quality gates in `rust.yml` remain green at 100.00% coverage.
