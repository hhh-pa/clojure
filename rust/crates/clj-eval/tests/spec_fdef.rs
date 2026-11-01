// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-122 slice-FDEF tests (finale — closes #122): `s/fspec`/`s/fdef`
//! (+ `s/assert` family and the `s/int-in`/`s/double-in`/`s/inst-in`
//! range specs completing `clojure.spec.alpha`) and the embedded
//! `clojure.spec.test.alpha` (`instrument`/`unstrument`,
//! `check`/`check-fn`, reporting).
//!
//! Oracles (text only, never executed): upstream
//! `spec.alpha-0.6.249` `clojure/spec/alpha.clj` (1996 lines) for
//! `fspec`/`fdef`/`macroexpand-check`/`fspec-impl`/`call-valid?`/
//! `validate-fn`, the range specs, and the assert family; same jar
//! `clojure/spec/test/alpha.clj` (579 lines) for instrumentation,
//! check, and reporting. All generative tests are seed-robust:
//! `gen/sample` draws from a random-seeded RNG (slice-F1), so every
//! fdef pins input-independent outcomes (a good fn passes on every
//! input, a bad fn fails on every input) rather than exact draws.
//! See `docs/gap-122/slice-fdef.md` for the divergences pinned here
//! (no shrinking, `prn` summaries, `:num-tests`-only check driver,
//! `*check-asserts*` dynamic, plain-fn `macroexpand-check`, core
//! `NaN?`/`infinite?`, behavior-preserving kwargs flattener, absent
//! `::stc/caller`).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// Require spec (as `s`), gen (as `gen`), and spec.test (as `stest`).
fn require_spec_test(eval: &mut Evaluator) {
    eval.eval_source(
        "(require '[clojure.spec.alpha :as s] '[clojure.spec.gen.alpha :as gen] '[clojure.spec.test.alpha :as stest])",
    )
    .unwrap();
}

/// Assert an uncaught throw whose class and message contain the parts
/// (pins the oracle text without overfitting the port's wrappers).
fn assert_throws(eval: &mut Evaluator, src: &str, class_part: &str, msg_part: &str) {
    let got = uncaught(eval, src);
    assert!(
        got.contains(class_part),
        "class mismatch for {src}: got {got}"
    );
    assert!(
        got.contains(msg_part),
        "message mismatch for {src}: got {got}"
    );
}

// ---- fspec / fdef ----

#[test]
fn fdef_registers_retrievable_fspec() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    // `fdef` returns the qualified fn sym.
    assert_eq!(
        run(
            &mut eval,
            "(defn fplus [a b] (+ a b))
             (s/fdef fplus :args (s/cat :a int? :b int?) :ret int?)"
        ),
        "user/fplus"
    );
    // Bare symbols miss (registry keys are qualified), as upstream;
    // qualified symbols and vars hit the same fspec.
    assert_eq!(
        run(
            &mut eval,
            "(let [bare (s/get-spec 'fplus)
                   b (s/get-spec #'fplus)
                   c (s/get-spec 'user/fplus)]
               [(nil? bare) (= b c) (some? (s/spec? b))
                (some? (:args b)) (some? (:ret b)) (nil? (:fn b))])"
        ),
        "[true true true true true true]"
    );
    // `describe` abbreviates (strips namespaces), as upstream; `form`
    // keeps the qualified `res` output.
    assert_eq!(
        run(&mut eval, "(s/describe (s/get-spec 'user/fplus))"),
        "(fspec :args (cat :a int? :b int?) :ret int? :fn nil)"
    );
    assert_eq!(
        run(&mut eval, "(s/form (s/get-spec 'user/fplus))"),
        "(clojure.spec.alpha/fspec :args (clojure.spec.alpha/cat :a clojure.core/int? :b clojure.core/int?) :ret clojure.core/int? :fn nil)"
    );
}

#[test]
fn fspec_conform_validates_fn_generatively() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn fplus [a b] (+ a b))
         (s/fdef fplus :args (s/cat :a int? :b int?) :ret int?)",
    );
    // Good fn conforms to itself; bad fn (input-independent failure) is invalid.
    assert_eq!(
        run(
            &mut eval,
            "(let [fs (s/get-spec 'user/fplus)
                   good (fn [a b] (+ a b))
                   bad (fn [a b] \"nope\")]
               [(identical? good (s/conform fs good))
                (s/invalid? (s/conform fs bad))
                (s/valid? fs good)
                (s/valid? fs bad)])"
        ),
        "[true true true false]"
    );
    // Non-fns are invalid and explain with `:pred ifn?`.
    assert_eq!(
        run(
            &mut eval,
            "(let [fs (s/get-spec 'user/fplus)]
               [(s/invalid? (s/conform fs 42))
                (mapv :pred (:clojure.spec.alpha/problems (s/explain-data fs 42)))])"
        ),
        "[true [ifn?]]"
    );
}

#[test]
fn fspec_fn_spec_relates_args_and_ret() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(s/fdef fid :args (s/cat :x int?) :ret int?
           :fn (fn [{:keys [args ret]}] (= (:x args) ret)))",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [fs (s/get-spec 'user/fid)]
               [(s/valid? fs (fn [x] x))
                (s/valid? fs (fn [x] (inc x)))])"
        ),
        "[true false]"
    );
}

#[test]
fn fspec_without_args_rejects_conform() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(&mut eval, "(s/fdef gret :ret int?)");
    assert_throws(
        &mut eval,
        "(s/conform (s/get-spec 'user/gret) (fn [x] x))",
        "ExceptionInfo",
        "Can't conform fspec without args spec:",
    );
}

#[test]
fn fspec_iterations_default() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    assert_eq!(run(&mut eval, "s/*fspec-iterations*"), "21");
}

#[test]
fn macroexpand_check_flags_bad_macro_calls() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn madd [a b] (+ a b))
         (s/fdef madd :args (s/cat :a int? :b int?))",
    );
    // Conforming args: nil. Nonconforming: oracle throw with `::args`.
    assert_eq!(
        run(
            &mut eval,
            "(@(find-var 'clojure.spec.alpha/macroexpand-check) #'madd '(1 2))"
        ),
        "nil"
    );
    assert_throws(
        &mut eval,
        "(@(find-var 'clojure.spec.alpha/macroexpand-check) #'madd '(1 :bad))",
        "ExceptionInfo",
        "Call to user/madd did not conform to spec.",
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (@(find-var 'clojure.spec.alpha/macroexpand-check) #'madd '(1 :bad))
                  (catch Exception e
                    (:clojure.spec.alpha/args (ex-data e))))"
        ),
        "(1 :bad)"
    );
}

// ---- exercise-fn over real fspecs (closes F2 divergence 5) ----

#[test]
fn exercise_fn_uses_registered_fspec() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn explus [a b] (+ a b))
         (s/fdef explus :args (s/cat :a int? :b int?) :ret int?)",
    );
    // Five `[args ret]` pairs with `ret` = `(apply f args)`.
    assert_eq!(
        run(
            &mut eval,
            "(let [pairs (s/exercise-fn 'user/explus 5)]
               [(count pairs)
                (every? (fn [[args ret]] (= ret (apply explus args))) pairs)
                (every? (fn [[args _]] (s/valid? (:args (s/get-spec 'user/explus)) args)) pairs)])"
        ),
        "[5 true true]"
    );
    // 1-arity defaults to 10 samples.
    assert_eq!(run(&mut eval, "(count (s/exercise-fn 'user/explus))"), "10");
    // Missing `:args` still throws the oracle text.
    run(&mut eval, "(s/fdef exret :ret int?)");
    assert_throws(
        &mut eval,
        "(s/exercise-fn 'user/exret 5)",
        "ExceptionInfo",
        "No :args spec found, can't generate",
    );
}

// ---- fspec generation ----

#[test]
fn fspec_gen_builds_callable_stub() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(s/fdef stubme :args (s/cat :a int? :b int?) :ret int?)",
    );
    // The generated stub checks args, then fabricates `:ret`-valid values.
    assert_eq!(
        run(
            &mut eval,
            "(let [stub (gen/generate (s/gen (s/get-spec 'user/stubme)))]
               [(ifn? stub)
                (s/valid? (:ret (s/get-spec 'user/stubme)) (stub 1 2))
                (try (stub 1 :bad) :no-throw
                     (catch Throwable _ :threw))])"
        ),
        "[true true :threw]"
    );
    // `with-gen` overrides the fspec generator.
    assert_eq!(
        run(
            &mut eval,
            "(let [fs (s/with-gen (s/get-spec 'user/stubme) #(gen/return :stubbed))]
               (gen/generate (s/gen fs)))"
        ),
        ":stubbed"
    );
}

// ---- assert family ----

#[test]
fn check_asserts_toggles_runtime_asserts() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    assert_eq!(run(&mut eval, "s/*compile-asserts*"), "true");
    assert_eq!(run(&mut eval, "(s/check-asserts?)"), "false");
    assert_eq!(run(&mut eval, "(s/check-asserts true)"), "true");
    assert_eq!(run(&mut eval, "(s/check-asserts?)"), "true");
    assert_eq!(run(&mut eval, "(s/check-asserts false)"), "false");
    assert_eq!(run(&mut eval, "(s/check-asserts?)"), "false");
}

#[test]
fn assert_checks_when_enabled() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(&mut eval, "(s/check-asserts true)");
    assert_eq!(run(&mut eval, "(s/assert int? 1)"), "1");
    assert_throws(
        &mut eval,
        "(s/assert int? :bad)",
        "ExceptionInfo",
        "Spec assertion failed",
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (s/assert int? :bad)
                  (catch Exception e
                    (:clojure.spec.alpha/failure (ex-data e))))"
        ),
        ":assertion-failed"
    );
}

#[test]
fn assert_passes_through_when_disabled() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    // Runtime disabled (default): returns x even when invalid.
    assert_eq!(run(&mut eval, "(s/assert int? :bad)"), ":bad");
    // Compile-time disabled: expands to x.
    assert_eq!(
        run(
            &mut eval,
            "(binding [s/*compile-asserts* false]
               (macroexpand '(s/assert int? :bad)))"
        ),
        ":bad"
    );
}

// ---- range specs ----

#[test]
fn int_in_validates_and_generates_range() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [sp (s/int-in 1 5)]
               [(s/valid? sp 1) (s/valid? sp 4)
                (s/valid? sp 5) (s/valid? sp 0) (s/valid? sp \"x\")])"
        ),
        "[true true false false false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(every? (fn [v] (and (int? v) (<= 1 v) (< v 5)))
                     (gen/sample (s/gen (s/int-in 1 5)) 100))"
        ),
        "true"
    );
}

#[test]
fn double_in_options_gate_infinities_nan_and_bounds() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "[(s/valid? (s/double-in) ##Inf)
              (s/valid? (s/double-in) ##NaN)
              (s/valid? (s/double-in :infinite? false :NaN? false) ##Inf)
              (s/valid? (s/double-in :infinite? false :NaN? false) ##NaN)
              (s/valid? (s/double-in :infinite? false :NaN? false) 1.5)
              (s/valid? (s/double-in :min 0.0 :max 1.0) 0.5)
              (s/valid? (s/double-in :min 0.0 :max 1.0) 2.0)]"
        ),
        "[true true false false true true false]"
    );
    // Draws honor the bounds.
    assert_eq!(
        run(
            &mut eval,
            "(every? (fn [v] (and (<= 0.0 v) (<= v 1.0)))
                     (gen/sample (s/gen (s/double-in :min 0.0 :max 1.0)) 50))"
        ),
        "true"
    );
    // Draws exclude infinities/NaN when disabled.
    assert_eq!(
        run(
            &mut eval,
            "(every? (fn [v] (and (not (infinite? v)) (not (NaN? v))))
                     (gen/sample (s/gen (s/double-in :infinite? false :NaN? false)) 50))"
        ),
        "true"
    );
}

#[test]
fn inst_in_validates_and_generates_inst_range() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(let [a #inst \"2020-01-01T00:00:00.000Z\"
                   b #inst \"2021-01-01T00:00:00.000Z\"
                   sp (s/inst-in a b)]
               [(s/valid? sp #inst \"2020-06-01T00:00:00.000Z\")
                (s/valid? sp a)
                (s/valid? sp b)
                (s/valid? sp \"2020-06-01\")])"
        ),
        "[true true false false]"
    );
    // Draws are insts within `[a, b]` (inclusive upper bound: exactly
    // upstream's `{:min st :max et}` construction).
    assert_eq!(
        run(
            &mut eval,
            "(let [a #inst \"2020-01-01T00:00:00.000Z\"
                   b #inst \"2021-01-01T00:00:00.000Z\"
                   lo (inst-ms a) hi (inst-ms b)]
               (every? (fn [v] (and (inst? v) (<= lo (inst-ms v)) (<= (inst-ms v) hi)))
                        (gen/sample (s/gen (s/inst-in a b)) 50)))"
        ),
        "true"
    );
}

// ---- instrument ----

#[test]
fn instrument_checks_args_only() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn iplus [a b] [a b])
         (s/fdef iplus :args (s/cat :a int? :b int?) :ret int?)",
    );
    // Pass-through before instrumentation.
    assert_eq!(run(&mut eval, "(iplus 1 :bad)"), "[1 :bad]");
    // Instrument returns the sym; good calls work.
    assert_eq!(
        run(&mut eval, "(stest/instrument 'user/iplus)"),
        "[user/iplus]"
    );
    assert_eq!(run(&mut eval, "(iplus 1 2)"), "[1 2]");
    // Bad args throw the oracle text ...
    assert_throws(
        &mut eval,
        "(iplus 1 :bad)",
        "ExceptionInfo",
        "Call to user/iplus did not conform to spec.",
    );
    // ... with `::s/failure :instrument`, `::s/fn`, `::s/args`, and no
    // `::stc/caller` (divergence 9: no Thread stack traces).
    assert_eq!(
        run(
            &mut eval,
            "(try (iplus 1 :bad)
                  (catch Exception e
                    (let [d (ex-data e)]
                      [(:clojure.spec.alpha/failure d)
                       (:clojure.spec.alpha/fn d)
                       (:clojure.spec.alpha/args d)
                       (contains? d :clojure.spec.test.check/caller)])))"
        ),
        "[:instrument user/iplus (1 :bad) false]"
    );
    // `:ret` violations still pass: instrument checks args only.
    run(
        &mut eval,
        "(defn ibad [a] :nope) (s/fdef ibad :args (s/cat :a int?) :ret int?)",
    );
    run(&mut eval, "(stest/instrument 'user/ibad)");
    assert_eq!(run(&mut eval, "(ibad 1)"), ":nope");
}

#[test]
fn instrument_roundtrip_and_all_arities() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn rplus [a b] [a b])
         (s/fdef rplus :args (s/cat :a int? :b int?) :ret int?)",
    );
    // Double instrument preserves the raw root: one unstrument restores.
    run(
        &mut eval,
        "(stest/instrument 'user/rplus) (stest/instrument 'user/rplus)",
    );
    assert_throws(
        &mut eval,
        "(rplus 1 :bad)",
        "ExceptionInfo",
        "did not conform to spec.",
    );
    assert_eq!(
        run(&mut eval, "(stest/unstrument 'user/rplus)"),
        "[user/rplus]"
    );
    assert_eq!(run(&mut eval, "(rplus 1 :bad)"), "[1 :bad]");
    // No-arg arities cover all fdef'd vars (the "all" forms: upstream
    // has no `instrument-all` var).
    assert_eq!(run(&mut eval, "(stest/instrument)"), "[user/rplus]");
    assert_throws(
        &mut eval,
        "(rplus 1 :bad)",
        "ExceptionInfo",
        "did not conform to spec.",
    );
    assert_eq!(run(&mut eval, "(stest/unstrument)"), "[user/rplus]");
    assert_eq!(run(&mut eval, "(rplus 1 :bad)"), "[1 :bad]");
}

#[test]
fn instrument_skips_macros() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(&mut eval, "(defmacro mmac [x] x)");
    assert_eq!(run(&mut eval, "(stest/instrument 'user/mmac)"), "[]");
}

#[test]
fn instrument_stub_replace_and_spec_override() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn uplus [a b] (+ a b))
         (s/fdef uplus :args (s/cat :a int? :b int?) :ret int?)
         (defn rawfn [a] (vector a))",
    );
    // `:stub` checks args, then fabricates `:ret`-valid values.
    run(
        &mut eval,
        "(stest/instrument 'user/uplus {:stub #{'user/uplus}})",
    );
    assert_eq!(
        run(
            &mut eval,
            "[(s/valid? (:ret (s/get-spec 'user/uplus)) (uplus 1 2))
              (try (uplus 1 :bad) :no-throw (catch Exception _ :threw))]"
        ),
        "[true :threw]"
    );
    run(&mut eval, "(stest/unstrument 'user/uplus)");
    // `:replace` runs the replacement under args checking.
    run(
        &mut eval,
        "(stest/instrument 'user/uplus {:replace {'user/uplus (fn [a b] :mocked)}})",
    );
    assert_eq!(
        run(
            &mut eval,
            "[(uplus 1 2)
              (try (uplus 1 :bad) :no-throw (catch Exception _ :threw))]"
        ),
        "[:mocked :threw]"
    );
    run(&mut eval, "(stest/unstrument 'user/uplus)");
    // `:spec` override instruments an unspec'd var.
    run(
        &mut eval,
        "(stest/instrument 'user/rawfn {:spec {'user/rawfn (s/fspec :args (s/cat :a int?))}})",
    );
    assert_eq!(
        run(
            &mut eval,
            "[(rawfn 1)
              (try (rawfn :bad) :no-throw (catch Exception _ :threw))]"
        ),
        "[[1] :threw]"
    );
    run(&mut eval, "(stest/unstrument 'user/rawfn)");
    // Opt sym with no spec and no override: the oracle no-fspec throw.
    assert_throws(
        &mut eval,
        "(stest/instrument 'user/rawfn {:stub #{'user/rawfn}})",
        "ExceptionInfo",
        "is not spec'ed.",
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (stest/instrument 'user/rawfn {:stub #{'user/rawfn}})
                  (catch Exception e
                    (:clojure.spec.alpha/failure (ex-data e))))"
        ),
        ":no-fspec"
    );
}

#[test]
fn instrument_handles_kwargs_fns() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn kfn [a & {:keys [b]}] [a b])
         (s/fdef kfn :args (s/cat :a int? :rest (s/* any?)))",
    );
    // Raw behavior for both call shapes ...
    assert_eq!(
        run(&mut eval, "[(kfn 1 :b 2) (kfn 1 {:b 2})]"),
        "[[1 2] [1 2]]"
    );
    // ... is preserved under instrumentation (divergence 8).
    run(&mut eval, "(stest/instrument 'user/kfn)");
    assert_eq!(
        run(&mut eval, "[(kfn 1 :b 2) (kfn 1 {:b 2})]"),
        "[[1 2] [1 2]]"
    );
    assert_throws(
        &mut eval,
        "(kfn :bad :b 2)",
        "ExceptionInfo",
        "did not conform to spec.",
    );
    run(&mut eval, "(stest/unstrument 'user/kfn)");
}

#[test]
fn with_instrument_disabled_bypasses_checks() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn dplus [a b] [a b])
         (s/fdef dplus :args (s/cat :a int? :b int?) :ret int?)
         (stest/instrument 'user/dplus)",
    );
    assert_eq!(
        run(&mut eval, "(stest/with-instrument-disabled (dplus 1 :bad))"),
        "[1 :bad]"
    );
    // Checks still apply outside the scope.
    assert_throws(
        &mut eval,
        "(dplus 1 :bad)",
        "ExceptionInfo",
        "did not conform to spec.",
    );
}

#[test]
fn namespace_helpers_list_fdefd_syms() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn hplus [a b] (+ a b))
         (s/fdef hplus :args (s/cat :a int? :b int?))",
    );
    assert_eq!(
        run(
            &mut eval,
            "[(contains? (stest/enumerate-namespace 'user) 'user/hplus)
              (contains? (stest/instrumentable-syms) 'user/hplus)
              (contains? (stest/checkable-syms) 'user/hplus)]"
        ),
        "[true true true]"
    );
}

// ---- check + reporting ----

#[test]
fn check_passes_correct_fn() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn cplus [a b] (+ a b))
         (s/fdef cplus :args (s/cat :a int? :b int?) :ret int?)",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/cplus {:clojure.spec.test.check/opts {:num-tests 20}}))]
               [(:sym res) (nil? (:failure res)) (some? (:spec res))])"
        ),
        "[user/cplus true true]"
    );
}

#[test]
fn check_flags_ret_violations() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn cbad [a] :nope)
         (s/fdef cbad :args (s/cat :a int?) :ret int?)",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/cbad {:clojure.spec.test.check/opts {:num-tests 20}}))]
               [(:sym res)
                (:clojure.spec.alpha/failure (ex-data (:failure res)))])"
        ),
        "[user/cbad :check-failed]"
    );
}

#[test]
fn check_flags_fn_violations_and_throws() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    // `:fn` violation (input-independent): every trial fails.
    run(
        &mut eval,
        "(defn cinc [x] (inc x))
         (s/fdef cinc :args (s/cat :x int?) :ret int?
           :fn (fn [{:keys [args ret]}] (= (:x args) ret)))",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/cinc {:clojure.spec.test.check/opts {:num-tests 20}}))]
               (:clojure.spec.alpha/failure (ex-data (:failure res))))"
        ),
        ":check-failed"
    );
    // Throwing fn (deterministic: args spec is `#{0}`).
    run(
        &mut eval,
        "(defn cthrow [x] (if (zero? x) (throw (ex-info \"boom\" {})) x))
         (s/fdef cthrow :args (s/cat :x #{0}) :ret int?)",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/cthrow {:clojure.spec.test.check/opts {:num-tests 5}}))]
               ((deref (find-var 'clojure.spec.test.alpha/result-type)) res))",
        ),
        ":check-threw"
    );
}

#[test]
fn check_reports_missing_args_spec() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(&mut eval, "(defn nag [x] x) (s/fdef nag :ret int?)");
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/nag {:clojure.spec.test.check/opts {:num-tests 5}}))]
               (:clojure.spec.alpha/failure (ex-data (:failure res))))"
        ),
        ":no-args-spec"
    );
}

#[test]
fn check_fn_honors_gen_overrides() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn gplus [a b] (if (= a 7) (+ a b) :wrong))
         (s/def ::ga int?)
         (s/fdef gplus :args (s/cat :a ::ga :b int?) :ret int?)",
    );
    // Only correct when `:a` is 7, so the override is observable in
    // both directions, deterministically.
    assert_eq!(
        run(
            &mut eval,
            "(let [res (stest/check-fn gplus (s/get-spec 'user/gplus)
                          {:gen {:user/ga (fn [] (gen/return 7))}
                           :clojure.spec.test.check/opts {:num-tests 10}})]
               (nil? (:failure res)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (stest/check-fn gplus (s/get-spec 'user/gplus)
                          {:gen {:user/ga (fn [] (gen/return 8))}
                           :clojure.spec.test.check/opts {:num-tests 10}})]
               (:clojure.spec.alpha/failure (ex-data (:failure res))))"
        ),
        ":check-failed"
    );
}

#[test]
fn check_preserves_instrumentation() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn pplus [a b] (+ a b))
         (s/fdef pplus :args (s/cat :a int? :b int?) :ret int?)
         (stest/instrument 'user/pplus)",
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/pplus {:clojure.spec.test.check/opts {:num-tests 5}}))]
               [(nil? (:failure res))
                (try (pplus 1 :bad) :no-throw (catch Exception _ :threw))])"
        ),
        "[true :threw]"
    );
}

#[test]
fn abbrev_and_summarize_report_results() {
    let mut eval = boot();
    require_spec_test(&mut eval);
    run(
        &mut eval,
        "(defn splus [a b] (+ a b))
         (s/fdef splus :args (s/cat :a int? :b int?) :ret int?)
         (defn sbad [a] :nope)
         (s/fdef sbad :args (s/cat :a int?) :ret int?)",
    );
    // Abbreviated failure: described spec, unwrapped ex-data, no test-check ret.
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/sbad {:clojure.spec.test.check/opts {:num-tests 5}}))
                   ab (stest/abbrev-result res)]
               [(first (:spec ab))
                (:clojure.spec.alpha/failure (:failure ab))
                (contains? ab :clojure.spec.test.check/ret)])"
        ),
        "[fspec :check-failed false]"
    );
    // Abbreviated pass: spec and ret trimmed.
    assert_eq!(
        run(
            &mut eval,
            "(let [res (first (stest/check 'user/splus {:clojure.spec.test.check/opts {:num-tests 5}}))]
               (stest/abbrev-result res))"
        ),
        "{:sym user/splus}"
    );
    // Summary counts (printed via `prn`: divergence 2).
    assert_eq!(
        run(
            &mut eval,
            "(stest/summarize-results
               (stest/check ['user/splus 'user/sbad] {:clojure.spec.test.check/opts {:num-tests 5}}))"
        ),
        "{:total 2, :check-passed 1, :check-failed 1}"
    );
}
