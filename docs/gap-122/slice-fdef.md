# Gap-122 slice FDEF (finale): `fspec`/`fdef`/`assert` + `instrument` + `check` (closes #122)

Part of #122 (clojure.spec port). Recon: `docs/gap-122/recon-fdef-generators.md`
§1a+§1b + §2 options G (fspec/fdef/assert), H (instrument) and I
(check + reporting). Slices 1+E+A+D2+B+F1+F2 landed the data
specs, regex, and generation; F2's `exercise-fn` awaits real
fspecs ("slice G") and F2 divergence 5 says so. This slice lands
G+H+I in one finale: `fspec`/`fdef` (+ `assert` family and the
`int-in`/`double-in`/`inst-in` range specs that complete
`clojure.spec.alpha`), a new embedded `clojure.spec.test.alpha`
(`instrument`/`unstrument`, `check`/`check-fn`, reporting), and
`exercise-fn` over real fspecs. No further slices: recon J
(core.specs.alpha) and K (full test.check fidelity) stay
explicitly out (see "Out of scope" below).

## Oracles (text only — NEVER execute a JVM)

- `~/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar`:
  `clojure/spec/alpha.clj` (1996 lines). Line refs below are against
  it: `*fspec-iterations*` 32-34, `fspec` 674-692,
  `macroexpand-check` 700-712, `fdef` 714-747, `call-valid?`/
  `validate-fn` 1726-1744, `fspec-impl` 1745-1793,
  `inst-in-range?`/`inst-in` 1890-1902, `int-in-range?`/`int-in`
  1904-1915, `double-in` 1917-1935, `*compile-asserts*`/
  `check-asserts?`/`check-asserts`/`assert*`/`assert` 1937-1996.
- Same jar: `clojure/spec/test/alpha.clj` (579 lines), ported whole
  except the `pprint`/`string` requires (see below): `->sym`/
  `->var`/`collectionize`/`enumerate-namespace` 25-57,
  `*instrument-enabled*` 59-75, stack-trace fns 76-122,
  `spec-checking-fn` 124-151, `instrumented-vars` + kwargs
  codegen 153-262, `instrument-1`/`unstrument-1` 264-310,
  `instrument`/`unstrument` 312-387, `explain-check`/`check-call`/
  `quick-check` 390-431, `check-1` 433-466, `check-fn`/
  `checkable-syms`/`check` 468-549, reporting 551-579.
- Port cites are `rust/crates/clj-eval/…` at the base commit
  `759045a1`, plus pre-spec probes
  (`rust/crates/clj-eval/tests/zz_probe_fdef.rs`, scratched before
  the PR): P1 record field named `fn` + `(:fn rec)` works; P2
  `assoc` of extra keys onto records works; P3
  `Thread/currentThread` throws catchable
  `IllegalArgumentException` at eval time (guard design
  confirmed); P4/P4b qualified refs to private vars are a
  compile error; P16/P17 `resolve`/`find-var` + `deref` reach
  them (the `@#'` substitute); P5 `alter-var-root` returns the
  new root; P6 `binding` accepts qualified vars; P7 `eval` of
  `fn` forms works; P8 regex literals + `re-find` work; P9b
  `ns-interns` takes a symbol; P10 `locking` works; P15 raw
  `&` map-destructuring already accepts a trailing map
  (divergence 8).

Erratum on the recon: §1a says `*fspec-iterations*` defaults to
100; the oracle text (`alpha.clj:32-34`) says **21**. This slice
pins 21.

## Problem

`s/fdef`, `s/fspec`, `s/assert`, `s/int-in`, `s/double-in`,
`s/inst-in` do not exist; `exercise-fn` only works over
`{:args …}` maps (F2 divergence 5); there is no
`clojure.spec.test.alpha` at all, so port code cannot instrument
fns or run generative `check`s — the "instrumentation" half of
the #122 goal ("macros, instrumentation, generators usable from
port code").

## Solution

Pure `.clj` additions plus a 2-line `ns.rs` embedding row (new
`include_str!` + `EMBEDDED_SOURCES` entry, the
`tools_deps_edn.clj` precedent). Zero new natives, zero
behavior change to landed code (the append-only shape of slices
D2/B; the `spec` macro's dropped `(when form …)` guard stays
dropped — `fspec` reproduces its effect, see below).

### `spec_alpha.clj`: fspec + assert + ranges (appended)

- `*fspec-iterations*` (21, oracle `:32-34`).
- `call-valid?`/`validate-fn` (oracle `:1726-1744`): the
  `gen/for-all*` + `gen/quick-check` + `:shrunk :smallest` shape
  becomes a first-failure-wins loop over
  `(gen/sample (gen (:args specs)) iters)` (divergence 1: no
  shrinking anywhere in the port — no rose trees).
- `FspecSpec` record `[args aform ret rform fn fform gfn]`
  (P1: a field may be named `fn`, so `(:args/:ret/:fn fspec)`
  work exactly like the upstream `ILookup` reify) + `Spec`
  extension: `conform*` (generative `validate-fn`, `::invalid`
  for non-`ifn?`, ex-info "Can't conform fspec without args
  spec: …" when `args` is nil — the port's standing
  `Exception.`→`ex-info` convention), `unform*` (identity),
  `explain*` (verbatim incl. the hardcoded `100` iters,
  `(catch Throwable t t)`, `instance?`, `.getMessage`),
  `gen*` (`gfn` or `(gen/return (fn [& args] …))` stub with the
  upstream `assert` + `(gen/generate (gen retspec overrides))`),
  `with-gen*`, `describe*` (list-built `(fspec :args … :ret …
  :fn …)`).
- `fspec-impl` (single upstream arity) + `fspec` macro: kwargs
  parsed manually (no `fn` local, no `:or` — `(get m :ret
  'clojure.core/any?)` for the upstream `:or {ret `any?}`);
  absent `:args`/`:fn` emit nil directly, reproducing the
  upstream `(spec nil)`→nil `(when form …)` effect without
  touching the `spec` macro; non-nil sides emit
  `(clojure.spec.alpha/spec …)` calls.
- `fdef` macro: list-built `(def fn-sym (fspec …))`, `fn-sym`
  passed through for `def`'s `ns-qualify` (upstream `:714-747`).
- `macroexpand-check` as a plain fn (no compiler hook —
  divergence 6).
- Assert family (oracle `:1937-1996`, appended LAST so earlier
  bare `(assert …)` core uses keep resolving to core):
  `*compile-asserts*` (default true, sysprop read replaced by
  the default), `*check-asserts*` (port-named dynamic replacing
  the `RT/checkSpecAsserts` static — divergence 5),
  `check-asserts?`/`check-asserts` (via `alter-var-root`, P5),
  `assert*` (verbatim over `explain-data*`/`explain-out`),
  `assert` macro (reads `*compile-asserts*` at expansion;
  expansion keeps `if` bare — specials are not vars — and
  qualifies `*check-asserts*`/`assert*`).
- Ranges: `int-in-range?`/`int-in`, `inst-in-range?`/`inst-in`,
  `double-in` (oracle `:1890-1935`), list-built: `int?`/
  `inst?`/`double?` heads fully qualify; `#()` literals become
  `(fn [g] …)` with `gensym`s; `and` is `clojure.spec.alpha/and`
  (as upstream's syntax-quote resolves it); `double-in`
  predicates use core `infinite?`/`NaN?` (same sets —
  divergence 7) and its `:gen` map carries only provided keys
  (so `double*`'s `:or` defaults still apply); `inst-in`'s
  `mkdate#` is `@(find-var
  'clojure.spec.gen.alpha/millis->inst)` (P4/P16 — the only
  millis→inst path; divergence 4).

### `spec_test_alpha.clj`: new embedded `clojure.spec.test.alpha`

Ns form mirrors upstream minus the `pprint`/`string` requires
(`:refer-clojure :exclude [test]` kept verbatim). `stc/` keys
are written `:clojure.spec.test.check/…` literally (no alias —
keywords need no ns).

- `->sym` (via `@(find-var 'clojure.spec.alpha/->sym)`, P16),
  `->var` (ex-info "… does not name a var", the port's
  `IllegalArgumentException.`→`ex-info` convention),
  `collectionize`, `enumerate-namespace` (P9b).
- `*instrument-enabled*`, `fn-spec?`,
  `with-instrument-disabled` (list-built `binding`, qualified
  var — P6).
- `thunk-frame?` via `re-find` (P8); `demunge`/`split`/
  `degensym` as ~30 lines of `subs`/`count` loop locals (no
  `Compiler/demunge`, no `clojure.string`);
  `stacktrace-relevant-to-instrument` verbatim over them.
- `spec-checking-fn`: `maybe-spec` via `@(find-var …)`
  (P16); caller capture wrapped in `(try … (catch Throwable _
  nil))` (P3 — the recon §1b.2 guard); `.applyTo` becomes
  `(apply f args)`; `::s/fn`/`::s/args`/`::s/failure` written
  out; `::caller` written `:clojure.spec.test.check/caller`.
- `no-fspec`, `instrumented-vars`, the kwargs codegen fns
  (`build-*` list-built with `gensym`; the eval'd flattener
  calls `clojure.spec.test.alpha/kwargs->kvs` qualified —
  privacy is not enforced on resolution, P4/P16),
  `maybe-wrap-kvs-emulation` (`eval`, P7),
  `instrument-choose-fn`/`instrument-choose-spec`,
  `instrument-1`/`unstrument-1` (`alter-var-root`, P5),
  `opt-syms`, `fn-spec-name?`, `instrumentable-syms`,
  `instrument`, `unstrument` (`locking`, P10) — all verbatim
  in shape.
- `explain-check` (3-arity `s/valid?`, as upstream),
  `check-call` (verbatim), `quick-check`: sample-based driver
  honoring `:num-tests` (default 1000 kept) and `:gen`,
  catching `Throwable` per trial; returns `{:result …,
  :num-tests n}` (+ `:fail [args]` on failure), never
  `:shrunk` (divergence 1); other test.check opts (`:seed`
  etc.) ignored (divergence 3).
- `make-check-result`, `check-1` (verbatim incl. the
  unstrument/re-instrument `finally`), `sym->check-map`,
  `validate-check-opts`, `check-fn`, `checkable-syms`,
  `check` (`pmap`), `failure-type`, `unwrap-failure`,
  `result-type`, `abbrev-result`, `summarize-results`
  (`prn`-based — divergence 2).

### `ns.rs`: embedding row

`SPEC_TEST_ALPHA_CLJ = include_str!("spec_test_alpha.clj")` +
`("clojure/spec/test/alpha", "clojure/spec/test/alpha.clj",
SPEC_TEST_ALPHA_CLJ)` in `EMBEDDED_SOURCES` (new row, never a
branch — the file's complexity-ratchet comment).

### On "instrument-all"

Upstream `spec.test.alpha` has **no** `instrument-all` var; the
no-arg arities `(instrument)`/`(unstrument)` are the
instrument/unstrument-all forms, and they port as such. No
invented alias is added (no oracle for it).

### Known divergences (pinned, not hidden)

1. No shrinking: `validate-fn` and the `check` driver report
   the first failing args drawn from `gen/sample` (no
   `:shrunk`; port-wide: no rose trees — recon K stays out).
2. `summarize-results` prints with `prn`, not `pprint`
   (recon §1b.4: porting pprint for one fn is
   disproportionate).
3. The `check` driver honors `:num-tests` (default 1000,
   kept) and `:gen` only; `:seed` and other test.check opts
   are ignored (`gen/sample` takes no seed).
4. `inst-in` builds insts via gen's millis reader (same
   instants; construction path differs — no `Date`
   interop).
5. `*check-asserts*` (port-named dynamic) replaces the
   `RT/checkSpecAsserts` static; `check-asserts?`/
   `check-asserts`/`assert`/`assert*` semantics kept.
6. `macroexpand-check` is a plain fn (the port has no
   macroexpansion-conformance hook).
7. `double-in` predicates are core `NaN?`/`infinite?`
   (same value sets as `Double/isNaN`/`isInfinite`).
8. The kwargs flattener is behavior-preserving on the port
   (raw `&` map-destructuring already accepts a trailing
   map — P15, pre-existing evaluator behavior); still
   ported verbatim in shape.
9. `::stc/caller` is always absent from instrument
   failures (no `Thread` stack traces; guarded capture —
   recon §1b.2, probe P3).

### Out of scope (final)

- `core.specs.alpha` (recon J): only consumed via the
  macroexpansion hook the port does not have; the #122 goal
  says "as needed" — not needed. Revisit if a hook lands.
- Full test.check fidelity (recon K): seed-stream
  identity, `ratio`/`double` edge distributions, rose-tree
  shrinking.
- The `spec` macro `(when form …)` guard stays dropped
  (pre-existing slice-1 narrowing; `fspec` works around it
  as described above).

## BDD (acceptance; each is a test in `rust/crates/clj-eval/tests/spec_fdef.rs`)

fdef/fspec (oracle `alpha.clj` arms above; all generative
tests seed-robust — `default-rng` is random-seeded, so every
fdef pins input-independent outcomes):

- Given `(s/fdef f :args (s/cat :a int? :b int?) :ret int?)`
  on a `defn`, the form returns `user/f`; the fspec is
  retrievable by symbol, var, and qualified symbol; a good
  fn `(fn [a b] (+ a b))` conforms (returns itself) and a
  bad one `(fn [a b] "nope")` yields `::invalid`; non-fns
  yield `::invalid` and explain with `:pred ifn?`.
- Given `:fn` relating args to ret (e.g. `= (:x args)
  ret`), a satisfying fn conforms and a violating one
  yields `::invalid`.
- Given `(s/fdef g :ret int?)` (no `:args`), conform
  throws "Can't conform fspec without args spec: …".
- Given `(s/exercise-fn 'user/f 5)` over a real fdef (F2
  divergence 5, closed here), five `[args ret]` pairs
  return with `ret` = `(apply f args)`; the 1-arity
  defaults to 10; explicit-fspec 3-arity still works.
  (The sym must be qualified: `get-spec` passes symbols
  through, as upstream.)
- Given `(s/gen fspec)`, `generate` yields a stub fn:
  valid-args calls return `:ret`-conforming values, and
  invalid-args calls throw; `s/with-gen` overrides the
  fspec generator.
- `*fspec-iterations*` reads 21.
- Given `macroexpand-check` called directly, conforming
  args return nil and nonconforming args throw "Call to …
  did not conform to spec." with `::args` in the ex-data.

assert (oracle `:1937-1996`):

- Given default state, `(s/check-asserts?)` is false and
  `s/*compile-asserts*` is true; `(s/check-asserts true)`
  returns true and flips `check-asserts?` (then back).
- Given asserts enabled, `(s/assert int? 1)` returns 1 and
  `(s/assert int? :bad)` throws "Spec assertion failed"
  with `::failure :assertion-failed`; given asserts
  disabled at runtime, `(s/assert int? :bad)` returns
  `:bad`.
- Given `*compile-asserts*` bound false at expansion,
  `(s/assert int? :bad)` macroexpands to `:bad`.

ranges (oracle `:1889-1935`):

- Given `(s/int-in 1 5)`, 1..4 validate, 5/`"x"` do not,
  and 100 draws lie in `[1, 5)`.
- Given `(s/double-in)`, infinities/`NaN` validate; given
  `:infinite? false :NaN? false`, they do not; `:min`/`:max`
  bound both validation and 100 draws.
- Given `(s/inst-in a b)`, in-range insts validate,
  out-of-range do not, and 100 draws are insts within
  `[a, b]` (inclusive upper bound, exactly as upstream's
  `{:min st :max et}` construction).

instrument (oracle `test/alpha.clj:1-387`). Sym arguments are
qualified throughout (bare syms miss the instrumentable set
and are silently dropped — identical to upstream, whose
registry keys are qualified too):

- Given an fdef'd fn, calls pass through uninstrumented;
  `(stest/instrument 'user/f)` returns `[user/f]`; bad-args
  calls throw "Call to user/f did not conform to spec."
  with `::s/failure :instrument`, `::s/fn`, `::s/args`
  and NO `:clojure.spec.test.check/caller` (divergence 9);
  good-args calls still work; `:ret` violations still
  pass (instrument checks args only, as upstream).
- Given double `instrument`, the raw root is preserved
  (one `unstrument` restores); `(stest/unstrument
  'user/f)` returns `[user/f]` and restores pass-through;
  no-arg `instrument`/`unstrument` cover all fdef'd vars;
  instrumenting a macro sym returns `[]`.
- Given `:stub`, the stub checks args then returns
  `:ret`-conforming generated values; given `:replace`,
  the replacement runs under args checking; given
  `:spec`, an unspec'd var instruments under the
  override; given an opt sym with no spec, "… is not
  spec'ed." throws with `:no-fspec`.
- Given a kwargs `(defn k [a & {:keys [b]}] …)` + fdef,
  instrumented `(k 1 :b 2)` and `(k 1 {:b 2})` both equal
  raw behavior (divergence 8).
- Given `with-instrument-disabled`, bad-args calls pass
  even when instrumented.
- `enumerate-namespace`, `instrumentable-syms` include
  fdef'd syms.

check + reporting (oracle `:390-579`; qualified syms, as above):

- Given a correct fdef'd fn, `(stest/check 'user/f
  {:clojure.spec.test.check/opts {:num-tests 20}})` yields
  one result with `:sym user/f`, no `:failure`;
- Given `:ret`/`:fn` violations, `:failure` carries
  `::s/failure :check-failed`; given a throwing fn,
  `result-type` is `:check-threw` (deterministic: args
  spec `#{0}`); given a `:ret`-only fdef, `:failure` is
  `:no-args-spec`.
- Given `:gen` overrides, `check` uses them; given an
  instrumented var, `check` passes and leaves it
  instrumented; `check-fn` works directly on fn+spec;
  `checkable-syms` includes fdef'd syms.
- Given results, `abbrev-result` abbreviates failures
  (described spec, unwrapped ex-data) and trims passes;
  `summarize-results` returns `{:total n, :check-passed
  n}` counts.

## TDD (implementation order)

1. Probes (done pre-spec; scratch file removed before the
   PR).
2. `spec_alpha.clj`: `*fspec-iterations*` + `call-valid?`/
   `validate-fn` + `FspecSpec` + `fspec-impl` +
   `fspec`/`fdef` + `macroexpand-check` + fspec tests
   (red → green).
3. `exercise-fn`-over-fspec tests (F2 divergence 5
   closure).
4. Assert family + tests.
5. Ranges + tests.
6. `spec_test_alpha.clj` + `ns.rs` row: load +
   `->sym`/`->var`/`collectionize`/`enumerate-namespace`
   + instrument core + tests.
7. kwargs flattener + stub/replace/spec-override +
   `with-instrument-disabled` + tests.
8. `check` driver + reporting + tests.
9. Gates: `cargo fmt --check`, `cargo clippy -p clj-eval
   --all-targets -- -D warnings`, new suite + full spec
   family suites green.
10. Push `gap-122-fdef-finale`, open PR (no merge — parent
    lands after peer review).

## Oracle pins (upstream texts preserved verbatim)

"Can't conform fspec without args spec: …", "Call to … did
not conform to spec.", "Fn at … is not spec'ed.", "Spec
assertion failed", "Specification-based check failed", "No
fn to spec", "No :args spec", "… does not name a var",
"instrument :gen expects ident keys", "check :gen expects
ident keys", "No :args spec found, can't generate" (now over
real fspecs). Ex-data failures kept: `:instrument`,
`:no-fspec`, `:assertion-failed`, `:check-failed`, `:no-fn`,
`:no-args-spec`. Defaults kept: `*fspec-iterations*` 21,
`*compile-asserts*` true, `check-asserts?` false,
`:num-tests` 1000, exercise 10.
