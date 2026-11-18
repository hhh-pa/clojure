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
3. Lays the profiles out with `aot_order.py`:
   - the `trivial` workload's startup set comes first, in the previous order's sequence;
   - then come 62 KiB windows of functions that the same workloads use together, so that each program touches few windows.

Ties break in a fixed order, so every job that links the order gets the same one. `run.sh --aot` delegates to the script.

**Correction after the first CI runs.** The probe above ran on empty stdin, so it measured mostly startup. With real inputs, the h2h for f9a16ff5, the first run with a CI-regenerated order, regressed instead: MEM 1280/408/55 → 1046/597/100, and AOT median peak RSS rose 116 KB.

The runtime changes in between were not the cause: on macOS, which has no layout order, peak RSS on the same inputs fell (median −80 KB). A same-runtime probe on empty stdin still ranked the CI order 192 KB better than the committed one.

The cause was the generator. `aot_order.py` kept only Rust-mangled names, so it dropped the runtime's `rt_*` C-ABI entry points (`rt_n_core_call`, `rt_n_drop`, `rt_n_deref`, `rt_n_core_own`, …). Every compiled program calls these while it processes input, but empty-stdin runs barely touch them. The committed order (from `hot_text.py`) listed them. The generator now keeps `rt_*` names alongside Rust ones.

**CI keeps it fresh.**

- The h2h workflow's `hot-text` job regenerates each side's order (aot-from, aot-to) once, using this run's tooling and workloads, and the 32 measure shards link with it.
- `rust-release.yml` regenerates the order before the Linux release build, so shipped binaries never carry a stale order.
- A failed regeneration keeps the committed order.
- The committed copy only serves local builds and is refreshed from the same pipeline.

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

## 7. Acceptance Checklist (Issue #271)

- [ ] `h2h-compare` `--allow-extra-challenger` implemented and covered by Go tests.
- [ ] `.github/workflows/h2h-cpu-mem.yml` measures `AOT from`, `AOT to`, and `OCAML` and runs `aot-rsrs` and `aot-ocaml`.
- [ ] Slice 1 CI run completes cleanly and captures Linux Pin `AOT` vs `OCAML` baseline artifacts.
- [ ] Unwaived `h2h-cpu-mem.yml` passes `recipes`, `recipes-rsrs`, `aot-rsrs`, and `aot-run` with 0 MEM and 0 INS losses at `--margin 0.15`, and `aot-ocaml` shows AOT outperforming OCaml on every compared case (wins only on MEM and INS).
- [ ] All CI quality gates in `rust.yml` remain green at 100.00% coverage.
