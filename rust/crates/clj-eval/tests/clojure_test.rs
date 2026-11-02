// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W2 test-framework smoke tests: `clojure.test` plus the `junit` and
//! `tap` reporters (stdlib-all issue #204). Each test boots a fresh
//! evaluator, requires the embedded namespaces, and exercises
//! upstream-documented behavior (pass/fail/error reports, fixtures,
//! runners, XML/TAP output). Report text is captured by rebinding
//! `clojure.test/*test-out*` inside `with-out-str`.

use std::path::PathBuf;

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = realized(eval, vs.last().unwrap());
    pr(&v)
}

fn fails(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(e) => format!("{e:?}"),
        Ok(_) => panic!("expected a failure, got values"),
    }
}

/// Wrap `body` so test reports are captured into a returned string
/// (fully qualified: works in any current namespace).
fn cap(body: &str) -> String {
    format!("(with-out-str (binding [clojure.test/*test-out* *out*] {body}))")
}

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-w2-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// The dir as a Clojure string literal.
    fn lit(&self) -> String {
        let s = self.dir_str();
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// The dir with `/` separators (what `*file*` pins during loads).
    fn dir_str(&self) -> String {
        self.dir.to_string_lossy().replace('\\', "/")
    }

    /// Wrap `src` in a `*load-path*` binding to this dir.
    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}]] {src})",
            self.lit()
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

/// All three namespaces load, and every upstream public is present.
#[test]
fn api_presence() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.junit :as j] '[clojure.test.tap :as p])",
    )
    .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "(remove #(contains? (ns-publics 'clojure.test) %)
               '[deftest deftest- with-test set-test is are testing test-var test-vars
                 test-all-vars test-ns run-tests run-all-tests run-test run-test-var
                 successful? use-fixtures compose-fixtures join-fixtures *load-tests*
                 *stack-trace-depth* *report-counters* *initial-report-counters*
                 *testing-vars* *testing-contexts* *test-out* with-test-out file-position
                 testing-vars-str testing-contexts-str inc-report-counter report do-report
                 get-possibly-unbound-var function? assert-predicate assert-any
                 assert-expr try-expr])"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(remove #(contains? (ns-publics 'clojure.test.junit) %)
               '[with-junit-output junit-report indent start-element element-content
                 finish-element test-name package-class start-case finish-case
                 suite-attrs start-suite finish-suite message-el failure-el error-el
                 *var-context* *depth*])"
        ),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(remove #(contains? (ns-publics 'clojure.test.tap) %)
               '[with-tap-output tap-report print-tap-plan print-tap-diagnostic
                 print-tap-pass print-tap-fail print-diagnostics])"
        ),
        "()"
    );
    // `report` stays a rebindable dynamic multimethod (the junit/tap hook).
    assert_eq!(
        run_last(
            &mut eval,
            "[(:dynamic (meta #'clojure.test/report))
              (binding [clojure.test/report (fn [m] :hooked)]
                (clojure.test/report {:type :custom}))]"
        ),
        "[true :hooked]"
    );
    // `test-var` keeps its upstream `:dynamic` marker.
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'clojure.test/test-var))"),
        "true"
    );
}

/// `is` returns the inner value; failures print the upstream FAIL shape.
#[test]
fn is_pass_fail_and_return() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &cap("(do (def w2-r1 (t/is (= 1 1))) (def w2-r2 (t/is (= 1 2) \"msg-here\")) nil)"),
        ),
        "\"\\nFAIL in () (NO_SOURCE_PATH:1)\\nmsg-here\\nexpected: (= 1 2)\\n  actual: (not (= 1 2))\\n\""
    );
    assert_eq!(run_last(&mut eval, "w2-r1"), "true");
    assert_eq!(run_last(&mut eval, "w2-r2"), "false");
}

/// Counters accumulate under a bound `*report-counters*`.
#[test]
fn report_counters() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def w2-c (ref clojure.test/*initial-report-counters*))
                 (with-out-str
                   (binding [clojure.test/*test-out* *out*
                             clojure.test/*report-counters* w2-c]
                     (t/is (= 1 1))
                     (t/is (= 1 2))))
                 [(:test @w2-c) (:pass @w2-c) (:fail @w2-c) (:error @w2-c)])",
        ),
        "[0 1 1 0]"
    );
}

/// `inc-report-counter` is a silent no-op without bound counters.
#[test]
fn counters_nil_safe() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(&mut eval, "(do (t/inc-report-counter :pass) :ok)"),
        ":ok"
    );
}

/// `thrown?`/`thrown-with-msg?` pass, fail, and return the exception.
#[test]
fn thrown_assertions() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &cap(
                "(do (def w2-te (t/is (thrown? ArithmeticException (/ 1 0))))
                      (t/is (thrown-with-msg? ArithmeticException #\"Divide\" (/ 1 0))))"
            ),
        ),
        "\"\""
    );
    assert_eq!(
        run_last(&mut eval, "(instance? ArithmeticException w2-te)"),
        "true"
    );
    let out = run_last(
        &mut eval,
        &cap("(t/is (thrown? ArithmeticException (+ 1 1)))"),
    );
    assert!(out.contains("FAIL in () (NO_SOURCE_PATH:1)"), "{out}");
    assert!(
        out.contains("expected: (thrown? ArithmeticException (+ 1 1))"),
        "{out}"
    );
    assert!(out.contains("actual: nil"), "{out}");
    let out = run_last(
        &mut eval,
        &cap("(t/is (thrown-with-msg? ArithmeticException #\"zzz\" (/ 1 0)))"),
    );
    assert!(out.contains("FAIL in"), "{out}");
    assert!(out.contains("ArithmeticException"), "{out}");
    assert!(out.contains("Divide by zero"), "{out}");
}

/// Throwing assertions report `:error` with a cause trace.
#[test]
fn error_report_path() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &cap("(t/is (throw (ex-info \"kaboom\" {:a 1})))"),
        ),
        "\"\\nERROR in () (NO_SOURCE_PATH:1)\\nexpected: (throw (ex-info \\\"kaboom\\\" {:a 1}))\\n  actual: clojure.lang.ExceptionInfo: kaboom {:a 1}\\n at [clojure.lang.ExceptionInfo unknown nil -1]\\n\""
    );
    // Caused-by chains render innermost-first with separators.
    let out = run_last(
        &mut eval,
        &cap("(t/is (throw (ex-info \"outer\" {} (ex-info \"inner\" {:n 2}))))"),
    );
    assert!(out.contains("clojure.lang.ExceptionInfo: outer"), "{out}");
    assert!(out.contains("Caused by:"), "{out}");
    assert!(
        out.contains("clojure.lang.ExceptionInfo: inner {:n 2}"),
        "{out}"
    );
}

/// Assertion lines resolve inside deftest bodies and `are` templates.
#[test]
fn location_capture_multiline() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])\n\
         (t/deftest w2-loc-a\n\
         \u{20} (t/is (= 1 1))\n\
         \u{20} (t/is (= 1 2)))\n\
         (t/deftest w2-loc-b\n\
         \u{20} (t/are [x y] (= x y)\n\
         \u{20} \u{20} 1 1\n\
         \u{20} \u{20} 2 3))",
    )
    .unwrap();
    let out = run_last(&mut eval, &cap("(t/test-var #'w2-loc-a)"));
    assert!(
        out.contains("FAIL in (w2-loc-a) (NO_SOURCE_PATH:4)"),
        "{out}"
    );
    // `are` stamps generated assertions with the `are` form's line.
    let out = run_last(&mut eval, &cap("(t/test-var #'w2-loc-b)"));
    assert!(
        out.contains("FAIL in (w2-loc-b) (NO_SOURCE_PATH:6)"),
        "{out}"
    );
    assert!(out.contains("actual: (not (= 2 3))"), "{out}");
}

/// Loaded test files report their `*file*` path and line.
#[test]
fn location_capture_file() {
    let fx = Fixture::new("file-loc");
    fx.write(
        "w2_loc_test.clj",
        "(ns w2-loc-test (:require [clojure.test :as t]))\n\
         (t/deftest w2-file-fail\n\
         \u{20} (t/is (= :a :b)))\n",
    );
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    eval.eval_source(&fx.within("(require 'w2-loc-test)"))
        .unwrap();
    let out = run_last(&mut eval, &cap("(t/run-tests 'w2-loc-test)"));
    let expect = format!("(w2-file-fail) ({}/w2_loc_test.clj:3)", fx.dir_str());
    assert!(out.contains(&expect), "{out}");
}

/// `run-tests` prints the summary and returns the counts map.
#[test]
fn deftest_run_tests_summary() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (t/deftest w2-s-pass (t/is (= 1 1)))
         (t/deftest w2-s-fail (t/is (= 1 2)))
         (t/deftest w2-s-err (t/is (throw (ex-info \"e\" {}))))",
    )
    .unwrap();
    let out = run_last(&mut eval, &cap("(def w2-sum (t/run-tests))"));
    assert!(out.contains("Testing user"), "{out}");
    assert!(out.contains("FAIL in (w2-s-fail)"), "{out}");
    assert!(out.contains("ERROR in (w2-s-err)"), "{out}");
    assert!(
        out.contains("Ran 3 tests containing 3 assertions."),
        "{out}"
    );
    assert!(out.contains("1 failures, 1 errors."), "{out}");
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test w2-sum) (:pass w2-sum) (:fail w2-sum) (:error w2-sum) (:type w2-sum)]"
        ),
        "[3 1 1 1 :summary]"
    );
    assert_eq!(run_last(&mut eval, "(t/successful? w2-sum)"), "false");
}

/// `successful?` is true only with zero failures and errors.
#[test]
fn successful_predicate() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "[(t/successful? {:pass 2 :fail 0 :error 0}) (t/successful? {:fail 1}) (t/successful? {})]"
        ),
        "[true false true]"
    );
}

/// Nested `testing` strings join in failure output.
#[test]
fn testing_contexts() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (t/deftest w2-ctx (t/testing \"outer\" (t/testing \"inner\" (t/is (= 1 2) \"the-msg\"))))",
    )
    .unwrap();
    let out = run_last(&mut eval, &cap("(t/test-var #'w2-ctx)"));
    assert!(out.contains("outer inner"), "{out}");
    assert!(out.contains("the-msg"), "{out}");
    assert_eq!(run_last(&mut eval, "(t/testing-contexts-str)"), "\"\"");
    assert_eq!(
        run_last(&mut eval, "(t/testing-vars-str {:file \"f.clj\" :line 7})"),
        "\"() (f.clj:7)\""
    );
}

/// `:each`/`:once` fixtures wrap vars in order; `test-ns` returns counts.
#[test]
fn fixtures_each_once() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (def w2-log (atom []))
         (defn w2-each [f] (swap! w2-log conj :each-setup) (f) (swap! w2-log conj :each-teardown))
         (defn w2-once [f] (swap! w2-log conj :once-setup) (f) (swap! w2-log conj :once-teardown))
         (t/deftest w2-fx-a (swap! w2-log conj :test-a))
         (t/deftest w2-fx-b (swap! w2-log conj :test-b))
         (t/use-fixtures :each w2-each)
         (t/use-fixtures :once w2-once)",
    )
    .unwrap();
    let out = run_last(&mut eval, &cap("(def w2-ns-sum (t/test-ns 'user))"));
    assert!(out.contains("Testing user"), "{out}");
    assert_eq!(
        run_last(&mut eval, "@w2-log"),
        "[:once-setup :each-setup :test-a :each-teardown :each-setup :test-b :each-teardown :once-teardown]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test w2-ns-sum) (:pass w2-ns-sum) (:fail w2-ns-sum) (:error w2-ns-sum)]"
        ),
        "[2 0 0 0]"
    );
    // Join order: first fixture outermost; empty joins are transparent.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def w2-o (atom []))
                 ((t/join-fixtures [(fn [f] (swap! w2-o conj :a) (f) (swap! w2-o conj :a2))
                                    (fn [f] (swap! w2-o conj :b) (f) (swap! w2-o conj :b2))])
                  (fn [] (swap! w2-o conj :body)))
                 @w2-o)"
        ),
        "[:a :b :body :b2 :a2]"
    );
    assert_eq!(
        run_last(&mut eval, "((t/join-fixtures []) (fn [] :x))"),
        ":x"
    );
}

/// `test-ns-hook` replaces the whole run and skips fixtures.
#[test]
fn test_ns_hook_skips_fixtures() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (def w2-hook-log (atom []))
         (t/deftest w2-h-test (swap! w2-hook-log conj :test-ran))
         (defn test-ns-hook [] (swap! w2-hook-log conj :hook-ran))
         (t/use-fixtures :each (fn [f] (swap! w2-hook-log conj :fixture-ran) (f)))",
    )
    .unwrap();
    run_last(&mut eval, &cap("(t/test-ns 'user)"));
    assert_eq!(run_last(&mut eval, "@w2-hook-log"), "[:hook-ran]");
}

/// `with-test`/`set-test` attach `:test` fns; `*load-tests*` omits tests.
#[test]
fn with_test_set_test_load_tests() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (t/with-test (defn w2-wt [x] (* x 2)) (t/is (= 4 (w2-wt 2))))
         (defn w2-st [x] x)
         (t/set-test w2-st (t/is (= 1 (w2-st 1))))",
    )
    .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "[(fn? (:test (meta #'w2-wt))) (fn? (:test (meta #'w2-st)))]"
        ),
        "[true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            &cap("(do (t/test-var #'w2-wt) (t/test-var #'w2-st))")
        ),
        "\"\""
    );
    // Disabled loading evaluates the definition but installs no tests.
    eval.eval_source(
        "(binding [t/*load-tests* false]
           (eval '(t/deftest w2-skipped (t/is false)))
           (eval '(t/with-test (def w2-wt2 42) (t/is false))))",
    )
    .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "[(nil? (resolve 'w2-skipped)) w2-wt2 (nil? (:test (meta #'w2-wt2)))]"
        ),
        "[true 42 true]"
    );
}

/// `test-vars`, `run-test-var`, and the `run-test` macro.
#[test]
fn low_level_runners() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (def w2-tv-log (atom []))
         (t/deftest w2-rt (t/is (= 1 1)))
         (t/use-fixtures :each (fn [f] (swap! w2-tv-log conj :wrapped) (f)))",
    )
    .unwrap();
    // `test-vars` applies each-fixtures and returns nil.
    assert_eq!(run_last(&mut eval, &cap("(t/test-vars [#'w2-rt])")), "\"\"");
    assert_eq!(run_last(&mut eval, "@w2-tv-log"), "[:wrapped]");
    // `run-test-var` prints begin/end plus a summary, and returns it.
    let out = run_last(&mut eval, &cap("(def w2-rt-sum (t/run-test-var #'w2-rt))"));
    assert!(out.contains("Testing user"), "{out}");
    assert!(
        out.contains("Ran 1 tests containing 1 assertions."),
        "{out}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test w2-rt-sum) (:pass w2-rt-sum) (:type w2-rt-sum)]"
        ),
        "[1 1 :summary]"
    );
    // The `run-test` macro resolves and runs one test.
    let out = run_last(&mut eval, &cap("(t/run-test w2-rt)"));
    assert!(
        out.contains("Ran 1 tests containing 1 assertions."),
        "{out}"
    );
    // Unresolvable and non-test symbols report on `*err*` at expansion.
    let out = run_last(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (eval '(t/run-test w2-nope))))",
    );
    assert!(out.contains("Unable to resolve"), "{out}");
    eval.eval_source("(def w2-plain 1)").unwrap();
    let out = run_last(
        &mut eval,
        "(with-out-str (binding [*err* *out*] (eval '(t/run-test w2-plain))))",
    );
    assert!(out.contains("is not a test"), "{out}");
}

/// `are` checks every group, rejects arity mismatches, and expands plainly.
#[test]
fn are_templates() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(&mut eval, "(macroexpand '(t/are [x] (= x 1) 1 2))",),
        "(do (clojure.test/is (= 1 1)) (clojure.test/is (= 2 1)))"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def w2-ac (ref t/*initial-report-counters*))
                 (def w2-aout
                   (with-out-str
                     (binding [t/*test-out* *out* t/*report-counters* w2-ac]
                       (t/are [x y] (= x y) 1 1 2 2 3 4))))
                 [(:pass @w2-ac) (:fail @w2-ac)])",
        ),
        "[2 1]"
    );
    let out = run_last(&mut eval, "w2-aout");
    assert!(out.contains("actual: (not (= 3 4))"), "{out}");
    // `are` returns the last assertion's value.
    assert_eq!(run_last(&mut eval, "(t/are [x] (= x 1) 1)"), "true");
    // Empty template is meaningless but ok.
    assert_eq!(run_last(&mut eval, &cap("(t/are [] true)")), "\"\"");
    let err = fails(&mut eval, "(t/are [x y] (= x y) 1)");
    assert!(err.contains("IllegalArgumentException"), "{err}");
    assert!(
        err.contains("The number of args doesn't match are's argv."),
        "{err}"
    );
}

/// Custom `assert-expr` methods and `report` overrides inherit locations.
#[test]
fn custom_assert_and_report() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (def w2-events (atom []))
         (defmethod t/assert-expr 'w2-evens? [msg form]
           (let [[_ x] form]
             `(if (even? ~x)
                (clojure.test/do-report {:type :pass :message ~msg :expected '~form :actual ~x})
                (clojure.test/do-report {:type :fail :message ~msg :expected '~form :actual ~x}))))",
    )
    .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (binding [t/report (fn [m] (swap! w2-events conj (select-keys m [:type :expected :actual :message :file :line])))]
                      (t/is (w2-evens? 4))
                      (t/is (w2-evens? 5) \"odd one\"))
                 (mapv :type @w2-events))",
        ),
        "[:pass :fail]"
    );
    // `:pass` events carry no location (upstream parity: only :fail/:error
    // are merged); the `:fail` inherits the `is` site (line 3 below).
    assert_eq!(
        run_last(
            &mut eval,
            "[(:message (second @w2-events)) (mapv :file @w2-events) (mapv :line @w2-events)]"
        ),
        "[\"odd one\" [nil \"NO_SOURCE_PATH\"] [nil 3]]"
    );
}

/// `instance?` assertions report the actual class on failure.
#[test]
fn instance_assertions() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(&mut eval, &cap("(t/is (instance? String \"a\"))")),
        "\"\""
    );
    let out = run_last(&mut eval, &cap("(t/is (instance? String 1))"));
    assert!(out.contains("expected: (instance? String 1)"), "{out}");
    assert!(out.contains("Long"), "{out}");
}

/// `function?` recognizes fns, rejects macros and unbound vars.
#[test]
fn function_predicate() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t]) (def w2-uv)")
        .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            "[(t/function? 'map) (t/function? 'when) (t/function? 'w2-uv) (t/get-possibly-unbound-var #'w2-uv)]"
        ),
        "[true false nil nil]"
    );
}

/// `run-all-tests` filters namespaces by regex.
#[test]
fn run_all_tests_regex() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (ns w2-ra-a (:require [clojure.test :as t]))
         (t/deftest w2-ra-t1 (t/is (= 1 1)))
         (ns w2-ra-b (:require [clojure.test :as t]))
         (t/deftest w2-ra-t2 (t/is (= 1 2)))
         (ns user)",
    )
    .unwrap();
    let out = run_last(
        &mut eval,
        &cap("(def w2-ra-sum (t/run-all-tests #\"w2-ra-a\"))"),
    );
    assert!(out.contains("Testing w2-ra-a"), "{out}");
    assert!(!out.contains("w2-ra-b"), "{out}");
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test w2-ra-sum) (:pass w2-ra-sum) (:fail w2-ra-sum)]"
        ),
        "[1 1 0]"
    );
}

/// The `:refer` style (bare `deftest`/`is`) works like upstream.
#[test]
fn referred_style() {
    let mut eval = boot();
    eval.eval_source(
        "(ns w2-use (:require [clojure.test :refer [deftest is testing are run-tests]]))
         (deftest w2-bare (testing \"t\" (is (= 2 2)) (is (= 1 2))))",
    )
    .unwrap();
    let out = run_last(&mut eval, &cap("(def w2-use-sum (run-tests 'w2-use))"));
    assert!(out.contains("FAIL in (w2-bare)"), "{out}");
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test w2-use-sum) (:pass w2-use-sum) (:fail w2-use-sum)]"
        ),
        "[1 1 1]"
    );
}

/// Tests compose: calling a deftest fn runs it; names nest in reports.
#[test]
fn composed_tests() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t])
         (t/deftest w2-inner (t/is (= 1 2)))
         (t/deftest w2-outer (w2-inner))",
    )
    .unwrap();
    assert_eq!(
        run_last(&mut eval, "(fn? (:test (meta #'w2-inner)))"),
        "true"
    );
    let out = run_last(&mut eval, &cap("(w2-outer)"));
    assert!(out.contains("FAIL in (w2-outer w2-inner)"), "{out}");
    // `deftest` returns the var; `deftest-` makes it private.
    assert_eq!(
        run_last(
            &mut eval,
            "[(var? (t/deftest w2-ret (t/is true)))
              (do (t/deftest- w2-pv (t/is true)) (:private (meta #'w2-pv)))]"
        ),
        "[true true]"
    );
}

/// Deprecated `file-position` keeps its `[file line]` shape, line unknown.
#[test]
fn file_position_shape() {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.test :as t])").unwrap();
    assert_eq!(
        run_last(&mut eval, "(t/file-position 99)"),
        "[\"NO_SOURCE_PATH\" nil]"
    );
}

/// A passing var renders a bare JUnit testcase document.
#[test]
fn junit_pass_document() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.junit :as j])
         (t/deftest w2-j-pass (t/is (= 1 1)))",
    )
    .unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &cap("(j/with-junit-output (t/test-var #'w2-j-pass))")
        ),
        "\"<?xml version=\\\"1.0\\\" encoding=\\\"UTF-8\\\"?>\\n<testsuites>\\n    <testcase name=\\\"w2-j-pass\\\" classname=\\\"user\\\">\\n    </testcase>\\n</testsuites>\\n\""
    );
}

/// Failures and errors render escaped `<failure>`/`<error>` children.
#[test]
fn junit_failure_and_error() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.junit :as j])
         (t/deftest w2-j-mixed
           (t/is (= 1 1))
           (t/is (= 1 2) \"needs <fix> & \\\"quotes\\\" it's\")
           (t/is (throw (ex-info \"junit-err\" {:k 7}))))",
    )
    .unwrap();
    let out = run_last(
        &mut eval,
        &cap("(j/with-junit-output (t/test-var #'w2-j-mixed))"),
    );
    assert!(out.contains("<testsuites>"), "{out}");
    assert!(out.contains("</testsuites>"), "{out}");
    assert!(
        out.contains("<testcase name=\\\"w2-j-mixed\\\" classname=\\\"user\\\">"),
        "{out}"
    );
    assert!(
        out.contains(
            "<failure message=\\\"needs &lt;fix&gt; &amp; &quot;quotes&quot; it&apos;s\\\">"
        ),
        "{out}"
    );
    assert!(out.contains("expected: (= 1 2)"), "{out}");
    assert!(out.contains("actual: (not (= 1 2))"), "{out}");
    assert!(out.contains("at: NO_SOURCE_PATH:4"), "{out}");
    assert!(out.contains("at: NO_SOURCE_PATH:5"), "{out}");
    assert!(
        out.contains("<error>") || out.contains("<error message="),
        "{out}"
    );
    assert!(out.contains("junit-err"), "{out}");
    assert!(out.contains("clojure.lang.ExceptionInfo"), "{out}");
    // Assertion order is preserved inside the testcase.
    let fail = out.find("<failure").unwrap();
    let err = out.find("<error").unwrap();
    assert!(fail < err, "{out}");
}

/// JUnit counting matches the default reporter; the body value passes through.
#[test]
fn junit_counters_and_result() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.junit :as j])
         (t/deftest w2-j-mixed
           (t/is (= 1 1))
           (t/is (= 1 2))
           (t/is (throw (ex-info \"e\" {}))))",
    )
    .unwrap();
    run_last(
        &mut eval,
        "(do (def w2-jc (ref t/*initial-report-counters*))
             (def w2-jxml (with-out-str
                            (binding [t/*test-out* *out* t/*report-counters* w2-jc]
                              (j/with-junit-output (t/test-var #'w2-j-mixed))))))",
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(:test @w2-jc) (:pass @w2-jc) (:fail @w2-jc) (:error @w2-jc)]"
        ),
        "[1 1 1 1]"
    );
    let xml = run_last(&mut eval, "w2-jxml");
    assert!(xml.contains("<testsuites>"), "{xml}");
    let out = run_last(
        &mut eval,
        &cap("(def w2-jret (j/with-junit-output :sentinel))"),
    );
    assert_eq!(
        out,
        "\"<?xml version=\\\"1.0\\\" encoding=\\\"UTF-8\\\"?>\\n<testsuites>\\n</testsuites>\\n\""
    );
    assert_eq!(run_last(&mut eval, "w2-jret"), ":sentinel");
    // Namespace names split into package/classname suite attributes.
    let out = run_last(&mut eval, &cap("(j/with-junit-output (t/test-ns 'user))"));
    assert!(out.contains("<testsuite name=\\\"user\\\">"), "{out}");
    assert_eq!(
        run_last(
            &mut eval,
            "[(j/package-class \"my.cool.NS\") (j/package-class \"plain\")]"
        ),
        "[[\"my.cool\" \"NS\"] [nil \"plain\"]]"
    );
}

/// TAP renders ok/not-ok lines, `#` diagnostics, and a plan.
#[test]
fn tap_output() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.tap :as p])
         (t/deftest w2-t-pass (t/is (= 1 1)))
         (t/deftest w2-t-fail (t/is (= 1 2)))
         (t/deftest w2-t-err (t/is (throw (ex-info \"tap-err\" {:k 9}))))",
    )
    .unwrap();
    // `run-tests` (not `test-ns`) emits the `:summary` the plan needs.
    let out = run_last(&mut eval, &cap("(p/with-tap-output (t/run-tests 'user))"));
    // `:pass` events carry no location (only :fail/:error are merged).
    assert!(out.contains("ok (w2-t-pass) (:)"), "{out}");
    assert!(
        out.contains("not ok (w2-t-fail) (NO_SOURCE_PATH:3)"),
        "{out}"
    );
    assert!(
        out.contains("not ok (w2-t-err) (NO_SOURCE_PATH:4)"),
        "{out}"
    );
    assert!(out.contains("# expected:(= 1 2)"), "{out}");
    assert!(out.contains("#   actual:(not (= 1 2))"), "{out}");
    assert!(out.contains("tap-err"), "{out}");
    assert!(out.contains("1..3"), "{out}");
}

/// TAP error diagnostics carry the cause trace.
#[test]
fn tap_error_trace() {
    let mut eval = boot();
    eval.eval_source(
        "(require '[clojure.test :as t] '[clojure.test.tap :as p])
         (t/deftest w2-t-one (t/is (throw (ex-info \"boom\" {:n 1}))))",
    )
    .unwrap();
    let out = run_last(
        &mut eval,
        &cap("(do (def w2-tc (ref t/*initial-report-counters*))
                  (binding [t/*report-counters* w2-tc]
                    (p/with-tap-output (t/test-var #'w2-t-one))))"),
    );
    assert!(out.contains("not ok (w2-t-one)"), "{out}");
    assert!(
        out.contains("#   actual:clojure.lang.ExceptionInfo: boom {:n 1}"),
        "{out}"
    );
    assert_eq!(
        run_last(&mut eval, "[(:test @w2-tc) (:error @w2-tc)]"),
        "[1 1]"
    );
}
