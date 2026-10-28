# Zero-oos S18: compiler surface — Specification

- Status: spec + oracle probes committed; no source edits.
  Implementer follows §1.10 then §2.7.
- Parent: out-of-scope reduction. Issue: #62.
  Branch: `zero-oos-s18-compiler`.
- Probes: `docs/zero-oos-s18-compiler/probes/`
  `{knobs,classpath,compile}.{clj,out}`
  (108 labeled pins + 5 bare WARNING lines: 48/19/41 per file;
  every file exit 0 and rerun-stable, verified by rerun + `cmp`).
- ENV LIMIT (critical): the spec host has no system LLVM
  (`llvm-sys`/`clj-aot` do not build here). Every AOT-path
  behavior in this spec is marked CI-verified; §1.10 splits
  each row's tests into LOCAL (fully verifiable without LLVM)
  and CI (needs the LLVM legs). Non-AOT logic — knobs,
  resolution, `add-classpath`, the failure-taxonomy mapping —
  is fully verifiable locally by construction (§1.6).

## Problem

Ten compiler-surface vars are `out-of-scope` with no slice:
`compile` plus the knobs `*compile-files*`, `*compile-path*`,
`*compiler-options*`, `*source-path*`, `*fn-loader*`,
`*use-context-classloader*`, `*verbose-defrecords*`,
`primitives-classnames`, and the deprecated-but-functional
`add-classpath`. The port owns both ends of the seam — a working
LLVM AOT backend (`compile_file` plans, lowers, emits, and links
in `rust/crates/clj-aot/src/{lib,link,lower,target}.rs`) and a
file loader (`require`/`load`/`load-file`/`load-string` over
`*load-path*` in `rust/crates/clj-eval/src/ns.rs`) — but nothing
connects `(compile 'some.ns)` to either.

## Solution

Oracle-pin the JVM behavior first (this spec), then implement:
`(compile lib)` resolves `lib` to source through the existing
`*load-path*` machinery, binds `*compile-files*` true, and drives
the AOT backend per root (plan → lower → emit object, never link);
emission is a load side effect gated on `*compile-files*`
truthiness, so `false` skips exactly the plan+emit phases while
evaluation runs unchanged. The knobs land as dynamic vars with
oracle-pinned roots (JVM `*compile-path*` root is `nil` with the
launcher binding `"classes"` — the port folds the session binding
into the root, §2.2); `add-classpath` appends to `*load-path*`
(append order first-wins, duplicates no-op, WARNING preserved);
`primitives-classnames` transfers as data with class-name-string
keys; `gen-class` types survive a compile round-trip untouched.

## Part 1 — Normative contract

### 1.1 Conventions

- A probe id is `<prefix>-<name>` where the prefix names the file:
  `k` knobs, `a` add-classpath, `c` compile.
- `THROW C|"m"` quotes the `.out` throw line verbatim (root cause).
- `(derived)` marks a port-test assertion derived from an oracle
  pin rather than quoted by it.
- `(oracle-only)` marks a pin with no port transfer
  (unrepresentable without N1 interop), recorded as context.
- `LOCAL` marks behavior verifiable without LLVM (fake driver,
  pure logic); `CI` marks behavior needing the LLVM legs
  (real `clj-aot` driver).
- JVM sources cited as `core.clj:<line>` refer to
  `clojure/core.clj` extracted from the alpha10 oracle jar.

### 1.2 `compile` — end-to-end contract

`(compile lib)` is `(binding [*compile-files* true] (load-one
lib true true))` answering `lib` (`core.clj:6387`), with the
plan+emit phases of §1.4–§1.6 running inside each root load:

1. Arity is exactly 1 (§1.7 T1).
2. `name` of the arg (`Named` cast for non-named, NPE for nil —
   §1.7 T2/T3). The arg itself is kept for step 8 and the answer.
3. Push a dynamic binding of `*compile-files*` to `true`
   (unconditionally — an outer `false` does NOT skip; the BDD's
   "`false` skips emission" refers to the load path holding
   `false`, §1.2 table rows `c-false-*`).
4. Resolve the name to source via the `*load-path*` machinery
   (§1.3). Resolution failure throws before anything else (§1.7 T4).
5. Per root, before evaluation: validate `*compile-path*` (§1.7
   T7/T8), strip the leading `ns` form (§1.5), plan the stripped
   text (`plan_source`). Plan failure throws with no evaluation,
   no artifact, no mark (§1.7 T6 — atomic where the JVM is
   partial, witnessed by `c-broken-files`).
6. Evaluate the full (unstripped) text through the existing load
   path. Eval failure throws the load's exact shape (§1.7 T12)
   with no artifact and no mark.
7. Emit the object through the driver (§1.6) to the §1.4 path.
   Emit failure throws `build:`-prefixed `IOException` (§1.7
   T9–T11) with no mark. The file's own `ns` form may already
   have self-marked its name (§1.2 `c-mismatch-other`).
8. Run the `need-ns` check through `find-ns` over the ORIGINAL
   arg (not the name string), then mark `*loaded-libs*` — the
   existing `load_one` order, unchanged. Check failure throws
   after emission: the artifact exists, the lib is unmarked
   (`c-mismatch-files`, §1.7 T5).
9. Pop the binding; answer the original arg value.

| Label | Contract |
| --- | --- |
| c-ret | `(compile 'my.ret)` answers `my.ret` |
| c-ret-class | the answer's class is `clojure.lang.Symbol` |
| c-files | artifacts land under the bound `*compile-path*` (JVM shape: 4 `.class` files; port shape: one `.o`, §1.4) |
| c-ns-loaded | the ns exists afterwards |
| c-loaded-libs | `(loaded-libs)` contains the lib afterwards |
| c-false-files | `*compile-files*` false: a load writes zero artifacts `(derived: no driver call)` |
| c-false-eval | `*compile-files*` false: evaluation still runs (marker reads back) |
| c-true-load | `*compile-files*` true: a plain `load` emits the identical listing (emission is load-gated, not `compile`-only) |
| c-flag-truthy | the gate is truthy (`1` emits like `true`) |
| c-twice | `compile` always reloads (side-effect counter hits 2; no loaded-libs shortcut) |
| c-captured | during a compile: `*compile-files*` true, `*compile-path*` the bound value, `*source-path*` the basename, `*fn-loader*` nil, `*use-context-classloader*` true, `*verbose-defrecords*` false, `*compiler-options*` nil |
| c-use-true | compile succeeds with the flag true |
| c-use-false | compile succeeds with the flag false (flag does not gate loading) |
| c-options-bound | compile succeeds with `*compiler-options*` bound (tolerated, unconsulted) |
| c-missing | THROW FNF `"Could not locate no/such/lib__init.class, no/such/lib.clj or no/such/lib.cljc on classpath."` |
| c-arity0 | THROW ArityException `"Wrong number of args (0) passed to: clojure.core/compile"` |
| c-arity2 | THROW ArityException `"Wrong number of args (2) passed to: clojure.core/compile"` |
| c-str | string lib loads, then THROW CCE `String cannot be cast to Symbol` (check runs on the original arg) |
| c-keyword | unqualified keyword loads, then THROW CCE `Keyword cannot be cast to Symbol` |
| c-keyword-ns | `/`-qualified keyword: `name` drops the namespace → FNF `"Could not locate plain__init.class, plain.clj or plain.cljc on classpath."` |
| c-num | THROW CCE `Long cannot be cast to Named` (from `name`, before any load) |
| c-nil-lib | THROW NPE (from `name`, before any load; JVM message is JDK-shaped) |
| c-broken | THROW (root) `RuntimeException "Unable to resolve symbol: no-such-fn-xyz in this context"` |
| c-broken-unmarked | `(loaded-libs)` DOES contain the broken lib (`ns` self-marks before the later form throws; the port's `eval_ns_form` already marks — transfers as-is) |
| c-broken-files | JVM wrote partial artifacts before throwing (`:threw` + 2 `.class` files); the port writes none (atomicity divergence, §2.4) |
| c-mismatch | THROW Exception `"namespace 'my.mismatch' not found after loading '/my/mismatch'"` (leading slash) |
| c-mismatch-unmarked | the requested lib is unmarked |
| c-mismatch-other | the evaluated `ns` name IS marked (self-mark) |
| c-mismatch-files | artifacts exist despite the throw (`:threw` + 3 files); JVM paths mix source-derived `__init` with ns-derived fn classes, the port paths everything by root (§1.4, §2.4) |
| c-nil-path | THROW RuntimeException `"*compile-path* not set"` (verbatim reuse, §1.7 T7) |
| c-bad-path | THROW CCE `Long cannot be cast to String` (§1.7 T8) |
| c-no-mkdirs | missing parents: THROW IOException `"No such file or directory"` (no `mkdirs` — the port creates no directories either, §1.4) |
| c-load-captured | a flag-false load observes the identical capture except `:compile-files false` |
| c-loadfile-source | `load-file`: `:source-path` is the basename, `:file` the absolute path, `:fn-loader` nil |
| c-loadstring-source | `load-string`: `:source-path` `"NO_SOURCE_FILE"`, `:file` nil, `:fn-loader` nil |
| c-gc-files | `gen-class` lib emits `my/gc.class` plus init/fn classes (JVM shape; port shape: one `.o` — the descriptor path is in-memory, §1.4) |
| c-gc-load | the emitted `my.gc` class loads by name |
| c-gc-new | it instantiates (`my.gc@L`) |
| c-gc-fn | the ns impl fn still answers `"hi"` after the compile (round-trip survival, JVM side) |
| c-record-ok | a `defrecord` lib compiles on the JVM |
| c-record-files | 8 `.class` files incl. `R.class` (subset-gap witness: the port answers a plan diagnostic here, never an artifact) |

### 1.3 Lib→source resolution (complete reuse, no new code)

`compile` resolves exactly like `require`: `root_resource`
(dashes→underscores, dots→slashes, leading slash) +
`resolve_source` (`.clj` then `.cljc` across every `*load-path*`
dir, first hit wins) + `load_paths` (`*pending-paths*` cycle
guard, `*file*` binding, `*ns*` isolation). The implementer adds
no resolution code: `native_compile` calls the same `load_one`
path after pushing the `*compile-files*` binding. Witnesses:
`c-missing`/`c-keyword-ns` (the `could_not_locate` text),
`c-mismatch` (the `load_one` check text with the leading slash).
`*load-path*` keeps its existing contract (seqable, lenient nil,
string entries — `load_dirs`).

### 1.4 Artifact shape (decided)

- One object file per compiled root:
  `<*compile-path*>/<root>.o`, where `<root>` is the load root
  (`my/plain` → `classes/my/plain.o`). The path derives from
  the load root, never from the evaluated namespace (diverges
  from the JVM's mixed source/ns-derived paths on mismatch —
  witnessed by `c-mismatch-files`, §2.4).
- Object only: no archive (`.a`), no link step, no executable.
  Rationale: linking a library is meaningless (no `main`);
  emit-only is hermetic by construction (works without
  `cc`/`link`, honoring the `--emit-obj` convention); archives
  add nothing for single objects. The `Link`/`ArchiveNotFound`
  shapes stay CLI-only (`build`), unrepresentable in the
  driver error (§1.6).
- Never JVM bytecode: no `.class` files, ever (the JVM
  per-fn/per-ns `.class` layout in `c-files`/`c-gc-files`/
  `c-record-files` is reference-only).
- No `mkdirs` (`c-no-mkdirs`): missing parent directories fail
  the write, surfacing as `Io` → `build: ...` (§1.7 T10).
  Tests bind `*compile-path*` to existing absolute
  per-test temp directories they create first (§1.11).
- Transitive: every root loaded during the compile (nested
  `require`s) independently plans and emits its own `.o`
  (JVM-like transitive AOT). A non-subset nested dep fails the
  whole compile with its plan diagnostic (subset-transitivity
  limitation, §2.4).
- Root-loads only: `load`/`require`/`compile` paths emit;
  `load-file`/`load-string`/`load-reader` never emit even when
  the flag holds (no load root to derive the path from —
  documented narrowing, §2.4).
- `gen-class` types need no artifact: #33 descriptors are
  in-memory, so "survival across a compile round-trip" means
  the descriptor still resolves, constructs, and forwards
  after a compile runs (JVM side witnessed by
  `c-gc-load`/`c-gc-new`/`c-gc-fn`; port tests in §1.10).
- Re-emission overwrites the same path (reloads are idempotent
  in layout; bytes are never pinned — CI asserts existence and
  non-emptiness only).

### 1.5 The leading-`ns` strip (decided)

`plan_source` classifies surface forms (`defn`, `if`/`do`/`let`,
literals, prim/sibling calls, `print`) and rejects `(ns ...)`
with a subset diagnostic — so `compile` would fail every real
lib without a strip. The strip lives in the S18 compile path
(`plan_source` itself is untouched):

- Before planning, read the first form with the same reader
  opts `plan_source` uses. If it is a list headed by the
  unqualified symbol `ns`, blank its char range with spaces,
  preserving newlines; plan the blanked text.
- Offset-exact: blanking runs from the form start to the
  reader position after the form (line/col accessors), so
  every later form keeps its line/col and plan diagnostics
  blame the original lines (LOCAL-pinned, §1.10).
- Only the first form, only unqualified `ns`:
  `clojure.core/ns`-qualified heads, `in-ns`, and later `ns`
  forms are ordinary code → subset diagnostics (honest).
- Evaluation always runs the full unstripped text (the `ns`
  form creates the namespace — `c-ns-loaded`).
- A file of only `ns` plans to the empty program (valid;
  CI-pinned through the real driver).

### 1.6 The driver hook (decided)

`clj-eval` cannot depend on `clj-aot` (which depends on
`clj-eval` for `plan_source`), so emission crosses an injected
driver. All of this except the real driver is LOCAL:

- New module `rust/crates/clj-eval/src/compiler.rs` owns:
  `native_compile`, `native_add_classpath`, the knob roots,
  the strip (§1.5), the §1.4 path derivation, and the hook.
- `DriveError` (LLVM-free, in `clj-eval`): `Target(String)` |
  `Emit(String)` | `Io { path, message }` — exactly
  `CompileError` minus `Plan` (planning stays caller-side via
  `plan_source`) minus `Link` (CLI-only, §1.4). A fake driver
  in tests returns each variant on demand.
- `CompileDriver` trait (implementer names the items):
  `emit(&SubsetProgram, source_path: &str, obj_path: &Path)
  -> Result<(), DriveError>`. The driver owns the write
  (bytes + path); the caller owns nothing about directories
  (§1.4 no-`mkdirs`).
- Hook unset (the default, including every local build) →
  the CLI's exact unlinked text as `IOException`
  (`"build: LLVM backend not linked (built with
  --no-default-features; rebuild with default features to
  compile)"`, `rust/src/bin/clojure-rs/main.rs:641`).
- The real driver lives in `clj-aot` and mechanically maps its
  `CompileError::{Target,Emit,Io}` into `DriveError`
  (CI-verified). The linking binary installs it at startup
  (implementer wires this seam; the spec fixes the trait, not
  the call site).
- The registry is a guarded static (`RwLock`/`OnceLock` style)
  with loom coverage for the install/read race (follow
  `loom_var_lock.rs`, per the #33 registry precedent).
- The plan diagnostic path is the load's `*file*` candidate
  (consistent with the CLI's file-path diagnostics).

### 1.7 Failure taxonomy (surveyed reuse — nothing invented)

Every `compile` failure reuses an existing diagnostic: the
loader's errors, or the CLI's `cmd_build_compile` mapping
(`rust/src/bin/clojure-rs/main.rs:585-627` — plan failures print
verbatim/exit 1, `Target`/`Emit` print `build: ...`/exit 1,
`Io`/`Link` print `build: ...`/exit 3). A function cannot exit,
so the CLI's rendering maps to throws: verbatim → verbatim
message, `build: ...` → `IOException` carrying the exact line.

| # | Failure | Throw class | Message | Evidence |
| --- | --- | --- | --- | --- |
| T1 | arity ≠ 1 (`compile`, `add-classpath`) | ArityException | `"Wrong number of args (N) passed to: clojure.core/<fn>"` | `c-arity0/2`, `a-arity0/2` (port: existing `check_arity`) |
| T2 | non-named lib arg | ClassCastException | `"... cannot be cast to ...Named"` | `c-num` (port: existing `name_of`) |
| T3 | nil lib arg | NullPointerException | the port's stable name-NPE text (probe35-era; the JVM text is JDK-shaped — loose) | `c-nil-lib` (port: existing `name_of` nil arm) |
| T4 | lib unresolvable | FileNotFoundException | `"Could not locate ..."`, verbatim | `c-missing`, `c-keyword-ns` (port: existing `could_not_locate`) |
| T5 | ns check on the ORIGINAL arg | CCE `Symbol` (str/kw args) / Exception `"namespace ... not found after loading ..."` (symbol args) | verbatim | `c-str`, `c-keyword`, `c-mismatch` (port: existing `strict_sym` path + `load_one` message) |
| T6a | plan failure, Reader/Compile | CompilerException wrapper + cause | load-identical (`source_to_native`) | `c-broken` (reuse — no new text) |
| T6b | plan failure, Uncaught (bare) | load-identical bare shape | load-identical | reuse (no new text) |
| T6c | plan failure, Subset/Depth/Internal | CompilerException, no cause | the plan diagnostic verbatim (`path:line:col: ...`) | CLI verbatim arm (`main.rs:614-617`); `c-record-files` is the subset witness |
| T7 | nil `*compile-path*` (flag-true root) | RuntimeException | `"*compile-path* not set"`, verbatim | `c-nil-path` (checked at plan time, before any side effects — fail-fast tightening, §2.4) |
| T8 | non-string `*compile-path*` | ClassCastException | `"... cannot be cast to ...String"` | `c-bad-path` (port: existing `cast_err` site) |
| T9 | driver Target/Emit | IOException | `"build: {msg}"` | CLI arm (`main.rs:618-620`); LOCAL via fake driver |
| T10 | driver Io | IOException | `"build: {path}: {message}"` | CLI arm (`main.rs:622-624`); `c-no-mkdirs` (LOCAL via fake; real via CI) |
| T11 | driver unset | IOException | the CLI's exact unlinked line | `main.rs:641` (LOCAL) |
| T12 | eval failure | load-identical (`source_to_native`) | load-identical | `c-broken` (reuse — no new text) |

Notes: T6a/T6b/T12 add zero new strings. T6c/T9/T10/T11 reuse
the CLI's strings with the exit code mapped to the throw (the
exit-1 vs exit-3 distinction does not transfer — one `IOException`
carries every `build:` line). T7's check runs at plan time even
though the JVM fires mid-eval (same text, earlier — documented
tightening). The `Link`/`ArchiveNotFound` arms are unreachable
by construction (the driver type cannot express them, §1.6).

### 1.8 Knobs

Roots below are `getRawRoot` pins (`k-root-*`) except
`*compile-path*` (see the row). `:dynamic` meta is absent
(`nil`) for all six Java-interned knobs — the port mirrors with
slot-only dynamics (`def_dynamic_root`, like `*assert*`); only
`*verbose-defrecords*` carries `:dynamic true` (a `def ^:dynamic`
in `core_print.clj`, like the port's `core_vars` dynamics).

| Knob | Root | `:dynamic` | Role during a compile | Port behavior |
| --- | --- | --- | --- | --- |
| `*compile-path*` | `"classes"` (folded session binding; JVM root is nil, `k-root-compile-path`, with the launcher binding `"classes"`, `k-compile-path`) | nil (`k-dyn-compile-path`) | output dir, observed as bound (`c-captured`); nil/non-string → T7/T8 | slot-only dynamic; read once per flag-true root (§1.2); doc `:added "1.0"` not pinned (port natives carry no `:added` meta) |
| `*compile-files*` | false (`k-root-compile-files`) | nil (`k-dyn-compile-files`) | bound `true` by `compile`; gates plan+emit per root, truthy (`c-flag-truthy`); false skips exactly plan+emit (`c-false-files`/`c-false-eval`) | slot-only dynamic; `load_one_path` checks `is_truthy()`; flag-false path byte-identical to today |
| `*compiler-options*` | nil (`k-root-compiler-options`) | nil (`k-dyn-compiler-options`) | observed unrebound; tolerated when bound (`c-options-bound`); JVM keys (`:elide-meta`, `:disable-locals-clearing`, `:direct-linking`, `k-doc-compiler-options`) have no subset meaning | data-only: bindable, never consulted (binding replaces — the doc's merge note is JVM guidance) |
| `*source-path*` | `"NO_SOURCE_FILE"` (`k-root-source-path`) | nil (`k-dyn-source-path`) | bound per load: basename of the loaded file (`c-captured`, `c-loadfile-source`); `"NO_SOURCE_FILE"` for `load-string` (`c-loadstring-source`); plain `eval` inherits, never rebinds (`k-eval-source`) | slot-only dynamic; `run_loaded` binds in all three paths (root loads: last segment of `root.ext`; `load-file`: last segment of the path; `load-string`: `"NO_SOURCE_FILE"`); `load-reader` binds the root through `run_loaded_values` (its `*file*` is nil); `eval_top_form` untouched |
| `*fn-loader*` | nil (`k-root-fn-loader`) | nil (`k-dyn-fn-loader`) | nil in every load/compile context probed (`c-captured`, `c-load-captured`, `c-loadfile-source`, `k-eval-fnloader`) | data-only: root nil, never bound (no loader kind exists; single-loader runtime — the #33 one-loader note) |
| `*use-context-classloader*` | true (`k-root-use-context`) | nil (`k-dyn-use-context`) | unrebound; no observed effect on load/require/resource/compile in alpha10 (`c-use-true/false`, `a-use-true/false`, `a-res-true/false`, §2.2) | data-only: root true, never consulted |
| `*verbose-defrecords*` | false (`k-root-verbose`) | true (`k-dyn-verbose`) | false, unrebound (`c-captured`); role is print-dup over records: false → `#user.Rec[1]`, true → `#user.Rec{:x 1}` (`k-verbose-false/true/default`) | `core_vars` dynamic; honored in `dup_record` (port shapes derived: `#Name[v1, ...]` stays, `#Name{map}` added — §2.1 D11, S8-owner ack) |
| `primitives-classnames` | 8-entry map (`k-prims-count`) | nil, plain def (`k-dyn-prims`) | none during compile (role is print-dup over `Class`: `k-prims-dup` → `#=(identity Long/TYPE)`) | plain root map; vals verbatim (`k-prims-vals`); keys the 8 `.getName` strings (`k-prims-keys` — divergence: JVM keys are `Class` objects, unrepresentable under N1; consistent with the #33 class-name convention); the `Class` consumer does not transfer (`k-prims-dup/nodup` oracle-only) |

### 1.9 `add-classpath`

Deprecated on the JVM (`:added "1.0"`, `:deprecated "1.1"`,
`k-addcp-meta`) but specified functionally: the port implements
the historical append contract over `*load-path*`, repairing the
alpha10 breakage documented in §2.2 (deliberate
repair-divergence, BDD-driven).

Order of operations (normative): arity check (T1, no WARNING on
failure — `a-arity0/2` show no interleaved line), then print
`"WARNING: add-classpath is deprecated"` to `*out*` (verbatim,
even when the add then fails — the `a-broken-*` interleavings),
then the String check (CCE `String` for non-strings — the port
accepts plain path strings only; JVM URL objects, URL-strings,
and `File`s are unrepresentable under N1, so `a-broken-url`/
`a-urls`-side acceptance narrows to strings, §2.4), then
contains-guarded `conj` onto `*load-path*` (nil normalizes to
`[]` first, preserving the append invariant; non-vector rebound
colls get `conj` semantics with append guaranteed for the vector
shape). Answers nil (`a-ret`).

| Label | Contract |
| --- | --- |
| a-warning (bare) | the call prints `WARNING: add-classpath is deprecated` on stdout |
| a-warn-text | the line, captured: `"WARNING: add-classpath is deprecated\n"` |
| a-broken-url | under `clojure.main` the JVM throws IllegalAccessError `"Context classloader is not a DynamicClassLoader"` even for well-formed URLs (oracle-only mechanism note — the port has no loaders and always succeeds for strings) |
| a-broken-bare | JVM bare path strings throw IAE `"URI is not absolute"` (oracle-only — the port TAKES bare paths, §2.4 repair) |
| a-broken-file/num | JVM non-URL args throw CCE `... cannot be cast to ...URL` (oracle-only; port: CCE `String`) |
| a-arity0/2 | THROW ArityException `"Wrong number of args (N) passed to: clojure.core/add-classpath"` (no WARNING) |
| a-ret/add2 | each add answers nil |
| a-urls | insertion order `[d1 d2]` — APPEND (decided from the oracle) |
| a-order | first-added dir wins resolution (`"d1"`) |
| a-effect | a later-added dir resolves new libs (`"new"`) |
| a-dup-ret/urls | re-adding is a no-op returning nil (deduped — the port guards with `contains?`) |
| a-missing | nonexistent dirs are accepted silently (nil) |
| a-use-true/false | resolution works under either flag value |
| a-res-true/false | `io/resource` consults the context loader under either flag (oracle-only context for the flag's data-only port role) |

### 1.10 The ten rows + covering-test plan (no matrix edits yet)

LOCAL tests live in `rust/crates/clj-eval/tests/compiler.rs`
(fence-owned); row-1c CI tests live in `rust/tests/compile_aot.rs`
(the `clj-aot → clj-eval` cycle of §1.6 forbids the real driver
from `clj-eval/tests`). Fixture sources live under per-test temp
dirs the tests create (never the source tree, §1.11). LOCAL
tests run on any host; CI tests need the LLVM legs. The matrix
flips ride the implementation PR — mirrored from CURRENT
`origin/master` at implement time (the issue's PR #41 pointer is
STALE, §2.5 R4).

| # | Row | Covering tests (`compiler.rs`) | Oracle pins | Gate |
| --- | --- | --- | --- | --- |
| 1 | `compile` | `returns_lib_symbol`, `binds_files_true`, `always_reloads`, `uses_load_path_order`, `leaves_flag_false_path_untouched`, `arity`, `missing`, `str_arg`, `num_arg`, `nil_arg`, `keyword_arg`, `keyword_ns_arg`, `broken_plan_diagnostic`, `broken_marks_and_no_artifact`, `mismatch_check_and_unmarked`, `nil_path`, `bad_path`, `strip_keeps_spans`, `plan_first_atomic_fake`, `transitive_emits_both_fake`, `driver_unset`, `driver_target_emit_io_fake`, `genclass_survives_fake` | `c-ret`…`c-no-mkdirs`, `c-broken-files`, `c-mismatch-files` | LOCAL |
| 1c | `compile` (artifacts) | `emits_object_layout`, `overwrite_idempotent`, `no_mkdirs_real`, `mismatch_leaves_root_object`, `empty_ns_only`, `broken_leaves_nothing`, `genclass_survives_real` | `c-files`, `c-gc-files`, `c-mismatch-files`, `c-broken-files` | CI |
| 2 | `*compile-path*` | `root_classes_and_slot_only`, `bound_path_drives_emit_fake`, `relative_and_absolute` | `k-compile-path`, `k-root-compile-path`, `k-dyn-compile-path`, `k-bind-compile-path` | LOCAL |
| 3 | `*compile-files*` | `root_false_and_slot_only`, `false_skips_plan_and_emit` (fake asserts zero driver calls + eval happened), `true_load_emits_fake`, `truthy_one_emits_fake`, `compile_forces_true_over_outer_false` | `k-compile-files`, `k-root-compile-files`, `k-dyn-compile-files`, `c-false-files`, `c-false-eval`, `c-true-load`, `c-flag-truthy`, `c-captured` | LOCAL |
| 4 | `*compiler-options*` | `root_nil_slot_only_and_binds`, `bound_map_tolerated_fake` | `k-compiler-options`, `k-root-compiler-options`, `k-dyn-compiler-options`, `k-bind-options`, `c-options-bound` | LOCAL |
| 5 | `*source-path*` | `root_no_source_file_slot_only`, `load_binds_basename`, `loadfile_binds_basename`, `loadstring_binds_no_source_file`, `eval_does_not_rebind`, `compile_observes_basename` | `k-source-path`, `k-root-source-path`, `k-dyn-source-path`, `c-captured`, `c-load-captured`, `c-loadfile-source`, `c-loadstring-source`, `k-eval-source` | LOCAL |
| 6 | `*fn-loader*` | `root_nil_slot_only_and_binds`, `nil_during_load_and_compile` | `k-fn-loader`, `k-root-fn-loader`, `k-dyn-fn-loader`, `c-captured`, `k-eval-fnloader` | LOCAL |
| 7 | `*use-context-classloader*` | `root_true_slot_only_and_binds`, `loads_succeed_under_both` | `k-use-context`, `k-root-use-context`, `k-dyn-use-context`, `k-bind-use-context`, `c-use-true/false`, `a-use-true/false` | LOCAL |
| 8 | `*verbose-defrecords*` | `root_false_dynamic_and_binds`, `dup_record_both_shapes` (port shapes derived) | `k-verbose`, `k-root-verbose`, `k-dyn-verbose`, `k-verbose-false/true/default` | LOCAL |
| 9 | `primitives-classnames` | `shape_count_vals_keys`, `miss_is_nil` | `k-prims-count/vals/keys/lookup/miss`, `k-dyn-prims` | LOCAL |
| 10 | `add-classpath` | `warns_and_returns_nil`, `appends_first_wins`, `dup_is_noop`, `missing_accepted`, `nonstr_rejected`, `arity`, `nil_path_normalizes` | `a-warning`, `a-warn-text`, `a-ret`, `a-urls`, `a-order`, `a-effect`, `a-dup-*`, `a-missing`, `a-arity0/2` | LOCAL |

Suggested matrix note stems (implementer finalizes against
`origin/master` wording): ``tests/compiler.rs <fns> (S18:
<behavior>; probes <ids>)`` — e.g. ``tests/compiler.rs
returns_lib_symbol, emits_object_layout (S18: bind-true +
load-one + emit `<root>.o`; probes c-ret, c-files)``.

### 1.11 Seams and constraints

- Emitted artifacts go under per-test temp dirs in tests
  (created per test, absolute paths bound to `*compile-path*`),
  NEVER into the source tree. No test writes relative to CWD.
- Hermetic/offline: `compile` links nothing (§1.4); where
  linking is unavailable the `--emit-ir`/`--emit-obj`
  conventions apply at the driver level (CI). No network, no
  workflow changes.
- No `require`/`load` behavior changes: the flag-false path
  through `load_one_path` is byte-identical (the truthiness
  check short-circuits first); their suites stay green
  unmodified. The only load-path additions are the gated
  plan/emit steps and the additive `*source-path*` binding in
  `run_loaded` (no existing test can observe the new var).
- `plan_source`, `SubsetProgram`, and the CLI mapping are
  reused untouched; new code is additive (`compiler.rs`, the
  two NATIVES entries, knob roots, the `load_one_path` hook
  call, the `dup_record` branch, the `clj-aot` driver impl).
- Gates (implementer): `cargo test -p clj-eval -p clj-value
  -p clj-reader`, `cargo test -p clj-core --test
  coverage_matrix`, `cargo fmt --all --check`, `cargo clippy
  --all-targets` zero warnings, 100% line+patch on new code,
  complexity ratchet, loom for the hook registry; PR per
  review-loop to APPROVED; land only on explicit ask.

## Part 2 — Notes

### 2.1 Design decisions

- D1 (emit-only `.o`, §1.4): one object per root, no archive,
  no link. Alternatives weighed: per-fn objects (absurd
  granularity for the subset backend), static archive (needs
  `ar` — unhermetic), executable (a lib has no `main`).
- D2 (driver hook + `DriveError`, §1.6): forced by the crate
  cycle (`clj-aot` → `clj-eval` for `plan_source`). The error
  subset (minus `Plan`/`Link`) makes unrepresentable failures
  unrepresentable. Alternatives: a new crate depending on both
  (vars would vanish when unlinked — rejected), shelling to
  the CLI (rejected as unhermetic).
- D3 (plan-first atomic per root, §1.2): plan → eval → emit.
  The JVM interleaves per form (partial artifacts + partial
  marks on failure); the port is atomic (witnessed both sides
  by `c-broken-files`). Chosen for fail-fast cleanliness and
  LOCAL testability.
- D4 (leading-`ns` strip, §1.5): without it `compile` could
  never succeed (`ns` is outside the subset). Space-blanking
  (not re-serialization) keeps diagnostics blame-exact.
  Extending the subset planner instead would touch the CLI's
  accept/reject boundary (shared, pinned) — rejected.
- D5 (emission is load-gated, §1.2): `compile` binds the flag
  and calls `load_one`; the hook lives in `load_one_path`.
  This makes `c-true-load` (`load` + true emits) and
  `c-false-files`/`c-false-eval` (false skips emission only)
  transfer exactly, and keeps flag-false `require`/`load`
  byte-identical.
- D6 (root-loads only, §1.4): `load-file`/`load-string` never
  emit (no root for path derivation). The alternative
  (ns-derived paths) needs eval-before-pathing, contradicting
  D3 — deferred as a follow-up, not this slice.
- D7 (`primitives-classnames` string keys, §1.8): JVM keys are
  `Class` objects (N1-unrepresentable); the 8 `.getName`
  strings keep the map's documented purpose expressible and
  match the #33 class-name convention. Opaque keys were
  rejected (un.inspectable, still divergent on lookup).
- D8 (`add-classpath` append + dedupe + strings-only + WARNING,
  §1.9): append and dedupe transfer the loader semantics
  (`a-urls`, `a-dup-urls`); strings-only follows the port's
  io-path convention (slurp/spit); the WARNING is verbatim
  (including print-before-throw).
- D9 (data-only trio, §1.8): `*compiler-options*`,
  `*use-context-classloader*`, `*fn-loader*` are bindable but
  never consulted — each has zero observed compile/load role
  in the oracle (§2.2), and each JVM meaning (subset-void
  option keys, loader selection) is unrepresentable.
- D10 (`*source-path*` basename rule, §1.8): every file load
  binds the last path segment (even `load-file` absolutes — both
  separators split on every platform, a deliberate narrowing);
  `load-string` binds the root; plain `eval` never rebinds.
  Mirrors all five oracle contexts with one rule.
- D11 (verbose branch in `dup_record`, §1.8): the port today
  renders `#Name[v1, ...]` unconditionally ("always
  non-verbose"); S18 adds the truthy branch rendering
  `#Name{map}` over the content map via the existing map-body
  writer. Shapes stay port-conventional (unqualified name).
  Touches S8-owned `printsys.rs` — owner ack required (S12-R1
  precedent).
- D12 (failure reuse, §1.7): loader errors verbatim, CLI
  strings verbatim, exit codes mapped to throws. No new
  message was invented except where no JVM shape exists — and
  there is no such place: T3 reuses the port's existing NPE
  text, T6c reuses the CLI's verbatim diagnostic.

### 2.2 Spike resolutions

- `add-classpath` under `clojure.main` always throws
  (IllegalAccessError — the context loader is the app loader,
  never `Dynamic`; §A pins). `RT.load` resolves through the
  stable parent loader (L0), ignoring both the context loader
  and the per-eval compiler loader (scratch series
  accept1–accept9). Classpath §B restores the historical flow
  by pointing the context loader at L0, so `a-ret`…`a-res-*`
  drive the real fn; compile.clj's fixture setup uses a direct
  L0 `.addURL` as documented scaffolding (not spec behavior).
- `*use-context-classloader*` is observably inert in alpha10:
  `require`, `RT.load`, `RT/baseLoader`, `io/resource`, and
  `compile` behave identically under both values (`a-use-*`,
  `c-use-*`, `a-res-*`, `f-*`/`g-*` scratch). Port: data-only.
- `*fn-loader*` is nil in every load/compile/eval context
  probed; the AOT-class-load path (`RT.load` finding
  `__init.class`, which may bind it) was not probed — the port
  has no class-loading path, so no transfer question arises
  either way. Port: data-only nil.
- `*compile-path*`: JVM root nil (`k-root-compile-path`),
  launcher session binding `"classes"` (`k-compile-path`).
  The port folds the session binding into the root (§1.8):
  `getRawRoot` is N1-interop (unobservable), the port is a
  session runtime like `clojure.main`, and the issue names
  `"classes"` as the root.
- `*source-path*`: root `"NO_SOURCE_FILE"` (`k-root-source-path`)
  doubles as the `load-string` value (`c-loadstring-source`);
  every file load binds the basename (even `k-source-path`'s
  own script path shows the bare name). `run_loaded` must bind
  explicitly in all three paths, plus `load-reader` through
  `run_loaded_values` (never "leave", which would leak an outer
  binding under `load-string`/`load-reader`).
- JVM creates no directories (`c-no-mkdirs` — raw IOException);
  the port creates none either. JVM `compile` of a broken lib
  leaves partial artifacts and the mark (`c-broken-files`,
  `c-broken-unmarked` — the `ns` macro self-marks,
  `core.clj:6069`, already mirrored by `eval_ns_form`); the
  port is atomic (D3). JVM mismatch paths mix source-derived
  `__init` with ns-derived fn classes (`c-mismatch-files`);
  the port paths everything by root (D1).
- `name` keeps dots (only `/` splits): `:my.plain` loads
  `my/plain` then CCEs at `find-ns` (`c-keyword`), while
  `:my/plain` loads `plain` (`c-keyword-ns`). The port's
  `name_of` + `root_resource` + original-arg check reproduce
  the whole matrix with no special cases.
- The `*compile-files*` gate is truthy (`c-flag-truthy`,
  `1` emits) — `is_truthy()`, the port convention.
- `*compiler-options*` bound maps are tolerated
  (`c-options-bound`); keys are JVM-compiler-specific.

### 2.3 Grounding map

- `rust/crates/clj-eval/src/ns.rs` (`load_one` :1098,
  `mark_loaded` :956, `eval_ns_form` :1446 marking :1461,
  `load_one_path` :1574 hook site, `run_loaded` :1060
  `*source-path*` site, `resolve_source` :1039,
  `root_resource` :977, `could_not_locate` :998,
  `load_dirs` :1022, `strict_sym` :83, `name_of` :965,
  `NATIVES` :1685, `core_vars` :1718, `def_dynamic_root`
  :1803): §1.2–§1.3, §1.7 T1–T5/T12, §1.8 installs, §1.9.
- `rust/crates/clj-eval/src/aot_subset.rs` (`plan_source`
  :565, `PlanError::diagnostic` :215, `classify_top` :684,
  `classify_call` :891): §1.5 (untouched), §1.7 T6.
- `rust/crates/clj-aot/src/{lib,link,lower,target}.rs`
  (`CompileError` :61, `compile_str` :112, `LinkError` link:53,
  `find_archive` link:70, `link_command` link:113):
  §1.6 real driver, §1.7 T9–T11 sources.
- `rust/src/bin/clojure-rs/main.rs` (`cmd_build_compile`
  :589, verbatim arm :614, `build:` arms :618–624, unlinked
  line :641): §1.7 T6c/T9–T11 sources.
- `rust/crates/clj-eval/src/printsys.rs` (`dup_record`
  :1194 "always non-verbose", `write_record_header` :1126,
  `write_record_body` :857): §1.8 verbose branch site (D11).
- `rust/crates/clj-eval/src/types.rs` (`__genclass*`
  :427, `HostKind::GenClass` :443, `genclass_call` :765):
  §1.10 gen-class survival subjects (no changes).
- `rust/crates/clj-value/src/extension.rs`
  (`ThrowClass` :41, `CompilerException`, `IOException`):
  §1.7 throw classes (all exist — nothing to add).
- `rust/core-coverage.md` (`*compile-files*` :664 and the
  other nine rows): §1.10 flip targets (implementer).

### 2.4 Divergences (all documented at the rows)

- Artifact shape: one root-derived `.o` vs per-fn/per-ns
  `.class` files (§1.4; `c-files`, `c-gc-files`,
  `c-record-files` reference-only).
- Mismatch paths: root-derived vs the JVM's mixed
  source-`__init`/ns-fn layout (`c-mismatch-files`).
- Atomicity: plan-first with no partial artifacts/marks vs
  JVM interleaved partiality (`c-broken-files`;
  `c-broken-unmarked` still transfers via `ns` self-mark).
- Subset: only subset-conformant roots compile (plus the
  transitive closure); `def`/`defrecord`/`gen-class`/
  host-interop shapes answer plan diagnostics, never
  artifacts (`c-record-files`; T6c). Transitivity is a
  stated limitation.
- `load-file`/`load-string`/`load-reader` never emit (narrowing, §1.4 D6).
- `*compile-path*` root `"classes"` folds the launcher
  session binding (JVM root nil — N1-invisible, §2.2).
- T7 fires at plan time (before side effects) vs JVM
  mid-eval (same text — tightening).
- `T3` reuses the port's stable name-NPE (JVM text
  JDK-shaped — loose).
- `add-classpath`: functional over `*load-path*` (repair of
  the alpha10 breakage), strings-only (URL/`File` narrow to
  CCE `String`), bare paths accepted (JVM IAEs).
- `primitives-classnames` keys are class-name strings (JVM:
  `Class` objects); the `Class` consumer never transfers.
- `*file*` keeps the port's candidate path (with dir prefix)
  vs the JVM's resource-relative string (pre-existing,
  untouched — `c-captured` `:file` vs port behavior).

### 2.5 Risks and open points

- R1 (cross-slice touch): the `dup_record` verbose branch
  edits S8-owned `printsys.rs`. The S8 owner must ack the
  3-line consumer (S12-R1 precedent) or take the branch as a
  hook.
- R2 (cross-cutting touch): the `load_one_path` hook call
  and `run_loaded` binding edit loader-owned `ns.rs`. Both
  are additive and gated (flag-false byte-identical); the
  loader suite staying green unmodified is the acceptance
  proof. Owner ack still required.
- R3 (no local LLVM): the spec author verified ZERO AOT-path
  lines — every `CI` row in §1.10/§2.6 runs only on the LLVM
  legs. Mitigation: the fake driver pins every LOCAL seam
  (gating, paths, taxonomy mapping, atomicity), leaving CI
  to pin bytes-on-disk only.
- R4 (stale pointer): the issue's "mirror from PR #41" is
  STALE — the implementer mirrors the ten rows from current
  `origin/master` at implement time, header counts exact.
- R5 (empty program): a file of only `ns` plans empty; the
  real driver must accept and emit it (CI-pinned,
  `empty_ns_only`). If lowering rejects empties, the
  implementer reports back — the spec's fallback is a
  dedicated diagnostic (not silent success).
- R6 (subset-transitivity): any non-subset transitive dep
  fails the compile. This bounds `compile` to small subset
  libs until the backend grows — a documented limitation,
  not a defect. Do NOT widen the subset in this slice
  (fence + CLI-boundary risk, D4).
- R7 (hook-install seam): the binary-startup install call
  site is the implementer's to place; the spec fixes the
  trait and the unset behavior only. Forgetting the install
  degrades every compile to T11 (loud, tested).

### 2.6 CI-only list (explicit — nothing here runs locally)

- `emits_object_layout`, `overwrite_idempotent`,
  `no_mkdirs_real`, `mismatch_leaves_root_object`,
  `empty_ns_only`, `broken_leaves_nothing`,
  `genclass_survives_real` (§1.10 row 1c): real-driver
  bytes/layout/existence.
- The `clj-aot` `CompileError::{Target,Emit,Io}` →
  `DriveError` mapping arms (mechanical; covered by the
  row-1c tests above — no dedicated mapping tests needed;
  `clj-aot`'s existing `target.rs`/`link.rs` tests already
  cover the underlying failure constructors).
- Everything else in §1.10 is LOCAL, including the full T1–T12
  mapping (fake driver), gating, paths, strip spans,
  atomicity, and gen-class survival through a fake compile.

### 2.7 Implementer checklist (in order)

1. Knob roots + meta (LOCAL tests green).
2. `add-classpath` + `*load-path*` semantics (LOCAL).
3. Strip + plan integration + T6/T7/T8 (LOCAL).
4. Hook + `DriveError` + `native_compile` + `dup_record`
   branch (LOCAL with fake driver).
5. `clj-aot` real driver + binary install (CI).
6. Row-1c CI tests + full LOCAL suite green.
7. Matrix: flip the ten rows from CURRENT `origin/master`
   positions with §1.10 note stems; header counts exact.
8. Gates (§1.11) → review-loop PR to APPROVED. Land only on
   explicit ask. Board claim protocol per project README.

