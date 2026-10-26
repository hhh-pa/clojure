// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S-rng-tower regex + RNG: probe replay for `re-pattern` /
//! `re-find` / `re-matches` / `re-seq` shapes and `rand` /
//! `rand-int` / `shuffle` / `random-sample` (see
//! `docs/i32-rng-tower/probes/re_rng.{clj,out}`). RNG pins are
//! shape/range only — exact draws differ across runtimes by
//! design.

use clj_eval::{Evaluator, SourceError, boot, jvm_class_of};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| pr(&realized(eval, v)))
        .collect()
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
    run(eval, src).pop().unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// The JVM class of the last value (`class` is out of scope, so
/// class assertions read Rust-side and never force).
fn class_of(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    jvm_class_of(vs.last().unwrap())
}

#[test]
fn re_find_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(re-find #\"\\d+\" \"a1b22\")"),
        "\"1\""
    );
    assert_eq!(
        run_last(&mut eval, "(re-find #\"a(x?)\" \"ax\")"),
        "[\"ax\" \"x\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(re-find #\"a(x)?\" \"a\")"),
        "[\"a\" nil]"
    );
    assert_eq!(run_last(&mut eval, "(re-find #\"x\" \"abc\")"), "nil");
    assert_eq!(run_last(&mut eval, "(re-find #\"a*\" \"b\")"), "\"\"");
}

#[test]
fn re_matches_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(re-matches #\"\\d+\" \"123\")"),
        "\"123\""
    );
    assert_eq!(run_last(&mut eval, "(re-matches #\"\\d+\" \"12a\")"), "nil");
    assert_eq!(
        run_last(&mut eval, "(re-matches #\"a(x)\" \"ax\")"),
        "[\"ax\" \"x\"]"
    );
    assert_eq!(run_last(&mut eval, "(re-matches #\"a*\" \"b\")"), "nil");
}

#[test]
fn re_seq_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(re-seq #\"\\d+\" \"a1b22\")"),
        "(\"1\" \"22\")"
    );
    assert_eq!(run_last(&mut eval, "(re-seq #\"a*\" \"b\")"), "(\"\" \"\")");
    assert_eq!(run_last(&mut eval, "(re-seq #\"x\" \"abc\")"), "nil");
    assert_eq!(
        run_last(&mut eval, "(re-seq #\"a(b)?\" \"ab\")"),
        "([\"ab\" \"b\"])"
    );
    assert_eq!(
        class_of(&mut eval, "(re-seq #\"\\d+\" \"a1b22\")"),
        "clojure.lang.Cons"
    );
}

#[test]
fn re_pattern_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(let [p #\"a+\"] (= p (re-pattern p)))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(re-pattern \"a+\")"), "#\"a+\"");
    assert_eq!(
        uncaught(&mut eval, "(re-pattern \"(a\")"),
        "java.util.regex.PatternSyntaxException | regex parse error:\n    (a\n    ^\nerror: unclosed group"
    );
    // A non-string pattern fails to `String` (probe `repat-long`).
    assert_eq!(
        uncaught(&mut eval, "(re-pattern 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn re_arg_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(re-find \"a\" \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.regex.Pattern (java.lang.String and java.util.regex.Pattern are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-matches \"a\" \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.regex.Pattern (java.lang.String and java.util.regex.Pattern are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-seq \"a\" \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.regex.Pattern (java.lang.String and java.util.regex.Pattern are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-find #\"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-matches #\"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-seq #\"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-find #\"a\" 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.CharSequence (java.lang.Long and java.lang.CharSequence are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(re-find #\"a\")"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/re-find"
    );
}

#[test]
fn rand_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand)] (and (number? x) (<= 0.0 x) (< x 1.0)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand 10)] (and (number? x) (<= 0.0 x) (< x 10.0)))"
        ),
        "true"
    );
    // Tower bounds narrow approximately (same shape, same range).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand 1/2)] (and (number? x) (<= 0.0 x) (< x 0.5)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand 5M)] (and (number? x) (<= 0.0 x) (< x 5.0)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand 0.5)] (and (number? x) (<= 0.0 x) (< x 0.5)))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(rand \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rand nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(rand 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/rand"
    );
}

#[test]
fn rand_int_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand-int 10)] (and (integer? x) (<= 0 x) (<= x 9)))"
        ),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(rand-int -1)"), "0");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [x (rand-int 5N)] (and (integer? x) (<= 0 x) (<= x 4)))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(rand-int \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rand-int nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
}

#[test]
fn shuffle_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(sort (shuffle [3 1 2]))"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(sort (shuffle #{3 1 2}))"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(count (shuffle [1 2 3]))"), "3");
    assert_eq!(
        uncaught(&mut eval, "(shuffle nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Collection.toArray()\" because \"c\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(shuffle \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.Collection (java.lang.String and java.util.Collection are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(shuffle {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class java.util.Collection (clojure.lang.PersistentArrayMap is in unnamed module of loader 'app'; java.util.Collection is in module java.base of loader 'bootstrap')"
    );
    // Lazies shuffle through their realized elements, and lists
    // keep their kind (probes `shuffle-lazy`, `shuffle-list`).
    assert_eq!(
        run_last(&mut eval, "(sort (shuffle (map inc [3 1 2])))"),
        "(2 3 4)"
    );
    assert_eq!(run_last(&mut eval, "(count (shuffle '(1 2 3)))"), "3");
}

#[test]
fn random_sample_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(doall (random-sample 1.0 [1 2]))"),
        "(1 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(doall (random-sample 0.0 [1 2]))"),
        "()"
    );
    assert_eq!(
        run_last(&mut eval, "(count (random-sample 1.0 [1 2 3]))"),
        "3"
    );
    assert_eq!(
        run_last(&mut eval, "(count (random-sample 0.0 [1 2 3]))"),
        "0"
    );
}
