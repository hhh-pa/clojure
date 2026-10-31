# Gap-122 slice C recon: fdef / generators / regex / core.specs

Recon-only (no implementation). Oracles below are upstream **source text**
read from `~/.m2` jars — no JVM was executed for this doc.

Oracle versions (all paths `~/.m2/repository/org/clojure/…`):

- `spec.alpha/0.6.249/spec.alpha-0.6.249.jar`:
  `clojure/spec/alpha.clj` (**1996 lines**), `clojure/spec/test/alpha.clj`
  (**579 lines**), `clojure/spec/gen/alpha.clj` (**227 lines**).
- `test.check/1.1.3/test.check-1.1.3.jar`: `generators.cljc` (**1787**),
  `check.cljc` (**321**), `random.clj` (**191**), `rose_tree.cljc` (**208**),
  `properties.cljc` (**95**), `clojure_test.cljc` (**203**), `impl.cljc`
  (**14**), `results.cljc` (**25**). (`random.cljs`, `longs.cljs`,
  `doubles.cljs`, `bit_count_impl.cljs`, `assertions/cljs.cljc` are
  CLJS-only; out of scope.)
- `core.specs.alpha/0.6.148-alpha12/…jar`:
  `clojure/core/specs/alpha.clj` (**261 lines**).
- Port-side cites are `rust/crates/clj-eval/…` at the base commit.

Line numbers below are against these exact files.

## 1. Ground facts

### 1a. fdef / fspec / check-asserts (spec.alpha)

- `fdef` is a 2-line macro (`alpha.clj:714-747`, body at 747):
  `` `(def fn-sym (fspec …)) `` — no intrinsic port risk. It registers
  an fspec value under the fn's qualified symbol, retrievable via
  `get-spec` with the var or symbol.
- `fspec` macro (`674-692`) builds `(fspec-impl argspec aform retspec
  rform fnspec fform gfn)`; `fspec-impl` (`1745-1793`, ~49 lines) is a
  `reify` of `ILookup` + `Specize` + `Spec`. Conform validates a fn by
  generative testing (`validate-fn`, `1735-1744`, `*fspec-iterations*`
  default 100 per line 32); `gen*` returns `(gen/return (fn [& args]
  …))` — i.e. **fspec needs `clojure.spec.gen.alpha/return` +
  `generate` at gen time**.
- `macroexpand-check` (`700-712`) is the macro-call conform hook; the
  port has no such hook today (see §1e).
- `check-asserts` machinery (`1937-1996`, ~60 lines): `*compile-asserts*`
  (default from `clojure.spec.compile-asserts` sysprop, default true),
  `check-asserts?`/`check-asserts` over `clojure.lang.RT/checkSpecAsserts`,
  `assert*` (throws ex-info with `::failure :assertion-failed`), and the
  `assert` macro which compiles to `x` when `*compile-asserts*` is false.
  Port needs: one dynamic var replacing the RT static field, one macro
  consulting `*compile-asserts*` at expansion time (port macros can read
  dynamic-var roots — same mechanism slice 1 already uses for `assert`
  per `docs/gap-122/slice-1.md` §8-9), and sysprop reads replaced by
  defaults. No numbers, no stack traces, no var redefinition.
- Also in this family: `exercise`/`exercise-fn` (`1866-1888`, needs
  `gen` + `gen/generate`), `inst-in`/`int-in`/`double-in` (`1889-1936`;
  `inst-in` builds `(java.util.Date. ^long d)` and `inst-ms`;
  `double-in` uses `Double/isInfinite`/`isNaN` — needs host-pred
  equivalents, port has `Value::Instant`, `inst-ms`, `inst?` via `Inst`
  protocol in `core.clj:1460-1470`).

### 1b. instrument / check (spec.test.alpha, 579 lines)

Section map (`clojure/spec/test/alpha.clj`):

| lines | content |
|---|---|
| 1-23 | ns; requires `clojure.pprint`, `clojure.string` |
| 25-57 | `->sym`, `->var`, `collectionize`, `enumerate-namespace` (needs `ns-interns`) |
| 59-75 | `*instrument-enabled*`, `with-instrument-disabled` |
| 76-122 | `interpret-stack-trace-element`, `stacktrace-relevant-to-instrument` |
| 124-151 | `spec-checking-fn` (wraps fn with args-conform) |
| 153-262 | `instrumented-vars` atom, varargs/kwargs arity codegen, `maybe-wrap-kvs-emulation` (**calls `eval`**, line ~257) |
| 264-310 | `instrument-choose-fn/spec`, `instrument-1`, `unstrument-1` (**`alter-var-root`** both) |
| 312-387 | `instrumentable-syms`, `instrument`, `unstrument` (`locking` on the atom) |
| 390-431 | `explain-check`, `check-call`, `quick-check` (needs `s/gen`, `gen/for-all*`, `gen/quick-check`) |
| 433-466 | `make-check-result`, `check-1` (un-instruments, `quick-check`s, re-instruments) |
| 468-549 | `check-fn`, `checkable-syms`, `check` (uses **`pmap`**) |
| 551-579 | `abbrev-result`, `summarize-results` (needs **`pp/pprint`**) |

What breaks on the port, and what does not:

1. **Var redefinition: fine.** `alter-var-root` exists (`vars.rs:528`,
   `native_alter_var_root`); `instrument-1`/`unstrument-1` port nearly
   verbatim. `locking` exists (`builtin_expand.rs:40`, `expand_locking`).
2. **Stack traces: degraded but safe — with ONE required guard.**
   `spec-checking-fn` captures the caller via
   `(.getStackTrace (Thread/currentThread))` (line ~134). The port has
   **no** `Thread/currentThread` or `getStackTrace` host support (no
   hits in `classemu.rs`/`fx4_host.rs`/`typeinternals.rs`), so that call
   throws an interop error *inside* the failure path and would mask the
   real "did not conform" ex-info. Fix: wrap caller capture in
   `(try … (catch Throwable _ nil))` or stub it. Downstream,
   `interpret-stack-trace-element` needs `Compiler/demunge` (port: none)
   and `invoke`/`invokeStatic` method names, but the port's
   `StackTraceElement->vec` returns `[class-sym unknown nil line]`
   (`core.rs:3926-3933`, method always `unknown`), so even a real trace
   yields no `:var-scope` and `stacktrace-relevant-to-instrument`
   returns `()` → `::stc/caller` is simply absent. Document as a known
   divergence.
3. **Arity codegen: fine.** `maybe-wrap-kvs-emulation` builds arglists
   from `(->> v meta :arglists …)` and `eval`s the flattener. The port
   stores `:arglists` on defn vars (`eval.rs:1397-1398`) and has `eval`
   (`ns.rs:2087`, `native_eval`).
4. **Missing requires.** `spec.test.alpha` requires `clojure.pprint`
   (only for `summarize-results`) and `clojure.string` (`includes?` in
   `thunk-frame?`, `split`/`replace` in `interpret-stack-trace-element`).
   The port embeds neither. Corpus text oracles exist
   (`clj-reader/tests/corpus/inputs/src/clj/clojure/string.clj`, 377
   lines; `pprint/` + `pprint.clj`, 3636 lines total) but porting all of
   pprint for one summary fn is disproportionate — stub `summarize-results`
   on `prn`, and port (or add 3 natives for) the `str` subset.
5. **`check` needs `pmap`** — present (`core.clj:679`, S6D). It also
   needs `s/gen` + test.check `quick-check`/`for-all*` (see §1c).

### 1c. Generators: port status + test.check sizing

- **The port has no test.check, no `clojure.spec.gen.alpha`, no
  `clojure.spec.test.alpha`.** `grep` over `rust/crates/*/src` for
  `test.check`, `spec.gen`, `spec.test` hits only reader-corpus fixtures
  and one comment in `clj-eval/tests/re_rng.rs`. Slice 1 declares `gen*`/
  `with-gen*` on the `Spec` protocol but leaves them unextended
  (`docs/gap-122/slice-1.md`, "Out of scope").
- `clojure.spec.gen.alpha` (227 lines) is a thin **dynaload shim** over
  test.check: `dynaload`/`quick-check`/`for-all*` (16-46), `delay`
  (55-66), `gen-for-name` (68-74), `lazy-combinator(s)`/`lazy-prim(s)`
  macros + prim list (76-123), `cat` (122-129), the `gen-builtins`
  delay mapping ~45 core preds to generators (131-189), `gen-for-pred`
  (190-197). Three builtins need host construction: `uri?` →
  `java.net.URI/create`, `decimal?` → `BigDecimal/valueOf`, `inst?` →
  `(java.util.Date. …)`. The port has the *predicates* (`uri?`,
  `decimal?`, `bytes?`, `inst?`) but constructor coverage is an open
  question (see §4).
- **test.check sizing (JVM-relevant files, 1.1.3):**

| file | lines | content / tricky parts |
|---|---|---|
| `generators.cljc` | 1787 | `Generator` record + `gen-pure`/`gen-fmap`/`gen-bind` (~30-145); `sized`/`resize`/`scale`, `choose`/`one-of`/`frequency`/`elements`/`such-that` (~147-537, needs `Long/MIN_VALUE`, `Math/floor`); collections (`tuple`/`vector`/`list`/`hash-map`/`set`/`map`/distinct/shuffle, ~548-1016); `large-integer(*)` + `double(*)` bit-twiddling (~1018-1328: `Long/reverse`, `Math/scalb`, `Math/getExponent`, `Math/pow`, `Double/*` statics); `bigint`/`char`/`string`/`keyword`/`symbol`/`ratio`/`uuid` (~1329-1581); `simple-type`/`any` families (~1582-1787) |
| `check.cljc` | 321 | `quick-check` loop + shrink-loop + reporter-fn; needs a millis clock (`impl.cljc` → `get-current-time-millis`; port has no `currentTimeMillis`) |
| `random.clj` | 191 | `SplittableRandom` port: unchecked 64-bit wrap, `Long/bitCount` (**no `bit-count` in port core**), `deftype` with `^long` fields, `proxy [ThreadLocal]` + `System/currentTimeMillis` global state (JVM-only, must be rewritten on an atom) |
| `rose_tree.cljc` | 208 | pure shrink trees; `deftype` over `clojure.lang.Indexed` (port: needs seq/nth compat — likely fine) |
| `properties.cljc` | 95 | `for-all*`, catches `ThreadDeath`/`Throwable` |
| `results.cljc` / `impl.cljc` | 25 / 14 | `Result` protocol, clock indirection |
| `clojure_test.cljc` | 203 | needs `clojure.test` — port status unverified; defer regardless |

  Total full port ≈ **2641 lines** (+203 for `clojure_test`), plus new
  natives: `bit-count`, `Math` subset (`floor`/`pow`/`log`/`scalb`/
  `getExponent` — port has only `abs`), `Long` statics
  (`numberOfLeadingZeros`/`reverse`/`MIN_VALUE`/`MAX_VALUE`),
  `Double` statics (`isInfinite`/`isNaN`/`MAX_VALUE`/…), a millis clock,
  and a rewrite of `next-rng` global state. Further friction: heavy
  `#?` reader conditionals (port reader supports `:clj` features,
  `opts.rs:107-130` — fine), ratio literals (port reader **defers**
  ratios, `number.rs:15,44,83-96` — `ratio`/`big-ratio` gens affected),
  and `deftype` with primitive-hint fields (verify).
- What spec itself needs from generators: every spec `gen*` impl calls
  into `gen/` combinators; `s/gen` (`alpha.clj:292-303`) funnels through
  `gensub` + `gen-for-pred`; `exercise` needs `gen/generate`; `check`
  needs `gen/for-all*` + `gen/quick-check`; instrument `:stub` needs
  `s/gen` + `gen/generate`. Shrinking is only exercised by `check`
  failure paths and `exercise` output minimality — nothing in
  conform/valid/explain needs it.

### 1d. Regex specs (cat/alt/\*/+/&/?)

Upstream (`alpha.clj`):

| lines | content |
|---|---|
| 607-665 | `*` `+` `?` `alt` `cat` `&` macros (syntax-quote + `res` form capture) |
| 1397-1461 | `cat-impl`, `rep*`, `rep-impl`, `rep+impl`, `amp-impl`, `filter-alt`, `alt*`, `alt2`, `alt-impl`, `maybe-impl` (op constructors; `rep`/`alt` stamp `java.util.UUID/randomUUID` ids — port has `random-uuid`) |
| 1453-1616 | engine: `accept`/`accept?`, `pcat*`, `accept-nil?`, `preturn`, `op-unform`, `add-ret`, `deriv`, `op-describe`, `op-explain` |
| 1617-1683 | `re-gen`, `re-conform`, `re-explain` |
| 1697-1723 | `regex-spec-impl` (~27 lines: the `Spec` glide over ops) |
| 1794-1812 | `keys*` macro (regex op over kvs; `::kvs->map` conformer just above) |

Regex-exclusive upstream ≈ **390 lines**, self-contained (map-based op
DSL over `::op` dispatch; no interop except the two UUID calls). Port
friction is the known slice-1 pair: macros must avoid syntax-quote
(batch read qualifies against the requiring ns) and build expansions
with `list`, per `docs/gap-122/slice-1.md` §1. `re-gen` is the only
part needing generators — it can land with the generator slice or stub
to throw until then.

### 1e. core.specs.alpha

261 lines, `s/def`s + `s/fdef`s for `let`/`if-let`/`when-let`,
`defn`/`defn-`/`fn`, `ns` clauses, `deftype`/`defrecord`, binding
forms, libspecs. It consumes: `s/and`, `s/cat`, `s/alt`, `s/or`,
`s/*`, `s/+`, `s/?`, `s/merge`, `s/tuple`, `s/map-of`, `s/coll-of`,
`s/keys*`, `s/fdef` — i.e. **every data-spec slice + regex + fdef**.
It is only *used* when something calls `macroexpand-check` or
specs core macros; the port has no macroexpansion-conformance hook, so
nothing in the port needs it. **Verdict: last slice, optional** —
port it only to close the "port's own macroexpansion conformance"
loop, after D–I land.

### 1f. Port capability inventory (for slice planning)

Present: `alter-var-root`, `eval`, `resolve`, `ns-interns`, `:arglists`
var meta, `locking`, `binding`/dynamic vars, `pmap`, `future`,
`satisfies?`, `with-out-str`, `delay`(? verify — assumed, ubiquitous),
`random-uuid`, ratios + `ratio?`, bigints, doubles, `shuffle`/`rand`,
`uri?`/`uuid?`/`bytes?`/`decimal?`/`inst?`/`inst-ms`, reader
conditionals (`:clj`), `Throwable->map`, emulated
`StackTraceElement->vec`.
Missing / to-verify: `clojure.string` + `clojure.pprint` namespaces,
`Compiler/demunge`, `Thread/currentThread` + `getStackTrace`,
`System/currentTimeMillis`, `bit-count`, `Math` subset,
`Long`/`Double` statics, `java.net.URI`/`BigDecimal`/`Date`
construction, `deftype` primitive-hint fields, ratio-literal reading
(N3 deferral), `clojure.test` (for `clojure_test.cljc`).

## 2. Slice options

Assumes slice 1 + the data-shape specs (`or`/`tuple`/`multi-spec`/
`keys`/`merge`/`every`+`coll-of`+`map-of`/`conformer`/`nilable`/
`nonconforming`) land first as slice D (owned outside this recon;
upstream `alpha.clj:366-606` macros + `822-1396` impls, ≈700 lines →
port ≈600-800). Everything below builds on D.

- **E — regex ops.** Scope: `cat`/`alt`/`*`/`+`/`?`/`&`, op engine
  (`pcat*`/`deriv`/`add-ret`/explain/unform/describe), `regex-spec-impl`,
  `keys*`. Upstream ≈390 lines → port ≈450-550 (list-built expansions).
  Defer `re-gen` to F or stub it. No new natives (UUID exists).
- **F — narrow generators (RECOMMENDED over full test.check).** Scope:
  `clojure.spec.gen.alpha` surface used by spec (`return`/`bind`/`fmap`/
  `one-of`/`such-that`/`elements`/`choose`/`tuple`/`vector`/`map`/`set`/
  `list`/`shuffle`/`frequency`/`not-empty`/`sized`/`resize`/`cat`/
  `sample`/`generate` + `gen-for-pred` builtins + `gen-for-name`),
  `gen*` wiring for every spec, `s/gen`, `s/exercise`, `with-gen`.
  Back RNG by a small seeded native (or `rand`); port `rose_tree.cljc`
  (208, pure) only if shrinking is wanted in-slice, else defer to I.
  Size: ≈700-900 lines ported `.clj` + 2-3 small natives. Divergences to
  document: seed streams won't match the JVM; `ratio`/`big-ratio` and
  `double` edge distributions are approximate until K.
- **G — fspec/fdef/assert.** Scope: `fspec`+`fspec-impl`, `fdef`,
  `macroexpand-check` (as a plain fn; no compiler hook),
  `*compile-asserts*`/`check-asserts?`/`check-asserts`/`assert*`/
  `assert`, `int-in`/`double-in`/`inst-in`. Upstream ≈200 lines → port
  ≈200-260. Needs F only for fspec `gen*` (load-order after F, or
  resolve lazily).
- **H — instrument/unstrument.** Scope: `spec.test.alpha:1-387` minus
  pprint: `spec-checking-fn` (**with guarded caller capture**, §1b.2),
  `instrumented-vars`, arity/kwargs codegen, `instrument-1`,
  `unstrument-1`, `instrument`, `unstrument`, `instrumentable-syms`,
  `with-instrument-disabled`, `enumerate-namespace`. Needs: 3-fn `str`
  subset (`includes?`/`split`/`replace` — port or natives) and
  `demunge`-stub. Size: ≈300-350 port lines. `:stub` needs F.
- **I — check + reporting.** Scope: `check-call`, `quick-check` driver
  (port of `check.cljc`'s loop, unshrunk first, shrinking if F ported
  rose trees), `check-1`, `check-fn`, `checkable-syms`, `check`,
  `abbrev-result`, `summarize-results` (on `prn`, not pprint). Needs: a
  millis-clock native, F's `for-all*`. Size: ≈350-450 port lines.
- **J — core.specs.alpha (optional, last).** Scope: the 261-line ns
  verbatim-ish + a decision on whether the port wires
  `macroexpand-check` into macroexpansion. Size: ≈260-320 port lines +
  hook design. Skip unless macro-conformance errors are wanted.
- **K — full test.check fidelity (optional, on demand).** Scope:
  remaining `generators.cljc` (`large-integer*`/`double*`/`ratio`/
  `uuid`/string families bit-faithful), `random.clj` SplittableRandom
  bit-exact, `clojure_test.cljc`. Size: ≈1500-1800 more lines + ~8
  natives + ratio-literal N3 fix + `ThreadLocal`/`proxy` rewrite. Only
  if identical seed streams / shrinking fidelity vs the JVM is required.

## 3. Recommended order

**D → E → F → G → H → I → (J, K optional).**

Data specs (D), then regex (E: pure, no new deps), then narrow
generators (F: unlocks `gen*`/`exercise`), then fdef/assert (G: small,
needs F only for fspec gen), then instrument (H) and check (I), with
core.specs (J) and full test.check fidelity (K) last and optional.

## 4. Open questions

1. Does the port's `deftype` support `^long`-hinted fields and protocol
   impls well enough for `random.clj`/`rose_tree.cljc`, or should the
   RNG live in Rust (`rng.rs` already hosts `rand`/`shuffle`)?
2. `bit-count`, `Math/*`, `Long/*`, `Double/*` statics: confirm the full
   missing set and whether each becomes a native or a `core.clj`
   composite (affects F/K sizing by ±100 lines).
3. Ratio literals are an N3 deferral (`number.rs`): can `ratio`/
   `big-ratio` gens construct ratios programmatically anyway (yes if
   `/` on ints yields `Ratio` — verify), or is K blocked on the reader?
4. `java.net.URI`/`BigDecimal`/`Date` construction for three
   `gen-builtins` entries: what are the port equivalents?
5. Is `delay` fully general in the port (used by fspec/and-spec/gen
   builtins)? Assumed yes; confirm before F.
6. Should `summarize-results` grow a minimal `clojure.pprint` (3636
   lines — no) or is `prn`-based output an accepted divergence?
7. Does `check`'s `pmap` (1000 `:num-tests` default × threads) fit the
   port's thread model, or should the port default `:num-tests` lower?
8. Is a `macroexpand-check` compiler hook wanted at all (decides J)?
