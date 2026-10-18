// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Analyzer contract tests: every §E2–E6/E11/E12 shape error, with
//! oracle-verbatim classes, messages, and wrappers (probes 20–25).
//! Single-line sources blame 1:1; one multi-line case pins real
//! spans.

use clj_eval::Evaluator;
use clj_reader::{ReaderOpts, read_str};

fn chain_of(src: &str) -> Vec<String> {
    let mut eval = Evaluator::new("test");
    // Some cases need interned vars (`var`, `set!` targets).
    for setup in [
        "(def va251 7)",
        "(def qnd251 1)",
        "(def user/q251 1)",
        "(def vfn251 (fn [x] x))",
        "(def vu251)",
    ] {
        let form = read_str(setup, &ReaderOpts::default()).unwrap();
        let _ = eval.analyze(&form);
    }
    let form = read_str(src, &ReaderOpts::default()).unwrap();
    match eval.analyze(&form) {
        Ok(_) => panic!("expected an error for {src}"),
        Err(e) => e.chain(),
    }
}

fn check(src: &str, wrapper: &str, cause: &str) {
    assert_eq!(
        chain_of(src),
        vec![wrapper.to_owned(), cause.to_owned()],
        "{src}"
    );
}

fn check_bare(src: &str, cause: &str) {
    assert_eq!(chain_of(src), vec![cause.to_owned()], "{src}");
}

#[test]
fn if_arity() {
    check(
        "(if)",
        "Syntax error compiling if at (test:1:1).",
        "java.lang.RuntimeException | Too few arguments to if",
    );
    check(
        "(if 1)",
        "Syntax error compiling if at (test:1:1).",
        "java.lang.RuntimeException | Too few arguments to if",
    );
    check(
        "(if 1 2 3 4)",
        "Syntax error compiling if at (test:1:1).",
        "java.lang.RuntimeException | Too many arguments to if",
    );
}

#[test]
fn quote_arity() {
    check(
        "(quote)",
        "Syntax error compiling quote at (test:1:1).",
        "clojure.lang.ExceptionInfo | Wrong number of args (0) passed to quote",
    );
    check(
        "(quote a b)",
        "Syntax error compiling quote at (test:1:1).",
        "clojure.lang.ExceptionInfo | Wrong number of args (2) passed to quote",
    );
}

#[test]
fn def_shapes() {
    check(
        "(def)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | Too few arguments to def",
    );
    check(
        "(def a 1 2)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | Too many arguments to def",
    );
    check(
        "(def :k 1)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | First argument to def must be a Symbol",
    );
    check(
        "(def :k 1 2)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | Too many arguments to def",
    );
    check(
        "(def other/x 1)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | Can't refer to qualified var that doesn't exist",
    );
    check(
        "(def m {:a 1} 2)",
        "Syntax error compiling def at (test:1:1).",
        "java.lang.RuntimeException | Too many arguments to def",
    );
}

#[test]
fn throw_shapes() {
    check(
        "(throw)",
        "Syntax error compiling throw at (test:1:1).",
        "java.lang.RuntimeException | Too few arguments to throw, throw expects a single Throwable instance",
    );
    check(
        "(throw a b)",
        "Syntax error compiling throw at (test:1:1).",
        "java.lang.RuntimeException | Too many arguments to throw, throw expects a single Throwable instance",
    );
}

#[test]
fn set_shapes() {
    check(
        "(set! x)",
        "Syntax error compiling set! at (test:1:1).",
        "java.lang.IllegalArgumentException | Malformed assignment, expecting (set! target val)",
    );
    check(
        "(set! 1 2)",
        "Syntax error compiling set! at (test:1:1).",
        "java.lang.IllegalArgumentException | Invalid assignment target",
    );
    check(
        "(set! undeclared-xyz-1 2)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: undeclared-xyz-1 in this context",
    );
    check(
        "(let [x 1] (set! x 2))",
        "Syntax error compiling fn* at (test:1:12).",
        "java.lang.IllegalArgumentException | Cannot assign to non-mutable: x",
    );
}

#[test]
fn var_shapes() {
    check(
        "(var)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"sym\" is null",
    );
    check(
        "(var no-such-var-xyz)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve var: no-such-var-xyz in this context",
    );
    check(
        "(var :k)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to clojure.lang.Symbol",
    );
    check(
        "(var foo251/x)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve var: foo251/x in this context",
    );
    check(
        "(var user/nope251)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve var: user/nope251 in this context",
    );
    check(
        "(let [x 1] (var x))",
        "Syntax error compiling var at (test:1:12).",
        "java.lang.RuntimeException | Unable to resolve var: x in this context",
    );
}

#[test]
fn recur_rules() {
    check(
        "(recur)",
        "Syntax error compiling recur at (test:1:1).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
    check(
        "(recur 1)",
        "Syntax error compiling recur at (test:1:1).",
        "java.lang.IllegalArgumentException | Mismatched argument count to recur, expected: 0 args, got: 1",
    );
    check(
        "((fn [x] (x (recur x))) 1)",
        "Syntax error compiling recur at (test:1:13).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
    check(
        "(loop [a 1 b 2] (recur 1))",
        "Syntax error compiling recur at (test:1:17).",
        "java.lang.IllegalArgumentException | Mismatched argument count to recur, expected: 2 args, got: 1",
    );
    check(
        "(loop [x 2] (try (recur x) (catch Exception e :c)))",
        "Syntax error compiling recur at (test:1:18).",
        "java.lang.UnsupportedOperationException | Cannot recur across try",
    );
    // Check order: tail, then try, then arity.
    check(
        "(loop [a 1] (try (recur 1 2) (catch Exception e 1)))",
        "Syntax error compiling recur at (test:1:18).",
        "java.lang.UnsupportedOperationException | Cannot recur across try",
    );
    check(
        "((fn [x] (x (recur 1 2))) 1)",
        "Syntax error compiling recur at (test:1:13).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
    check(
        "(loop [x 1] (try x (catch Exception e (recur 2))))",
        "Syntax error compiling recur at (test:1:39).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
    check(
        "(loop [x 1] (try x (finally (recur 2))))",
        "Syntax error compiling recur at (test:1:29).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
}

#[test]
fn try_shapes() {
    check(
        "(try 1 (catch))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: null",
    );
    check(
        "(try 1 (catch String))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: null",
    );
    check(
        "(try 1 (catch String \"s\" 1))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: s",
    );
    check(
        "(try 1 (catch Exception q/e 1))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.RuntimeException | Can't bind qualified name:q/e",
    );
    check(
        "(try 1 (catch NopeZZZ e 1))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: NopeZZZ",
    );
    check(
        "(try 1 (catch q/NopeZZZ e 1))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: q/NopeZZZ",
    );
    check(
        "(try 1 (catch 5 e 1))",
        "Syntax error compiling try at (test:1:8).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: 5",
    );
    check(
        "(try 1 (finally 1) (catch Exception e 2))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.RuntimeException | finally clause must be last in try expression",
    );
    check(
        "(try 1 (finally 1) (finally 2))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.RuntimeException | finally clause must be last in try expression",
    );
    check(
        "(try 1 (catch Exception e 1) :foo)",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.RuntimeException | Only catch or finally clause can follow catch in try expression",
    );
    check(
        "(catch Exception e 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: catch in this context",
    );
}

#[test]
fn resolve_errors() {
    check(
        "(foo/bar 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | No such namespace: foo",
    );
    check(
        "(user/nope-1 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | No such var: user/nope-1",
    );
    check(
        "foo/bar",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | No such namespace: foo",
    );
    check(
        "user/nope-2",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | No such var: user/nope-2",
    );
    check(
        "(nil 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.IllegalArgumentException | Can't call nil, form: (nil 1)",
    );
}

#[test]
fn let_spec_errors() {
    let wrapper = "Syntax error macroexpanding clojure.core/let at (test:1:1).";
    let cause = "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.";
    for src in [
        "(let [x] x)",
        "(let x x)",
        "(let)",
        "(let [1 2] 1)",
        "(let [#{a} 1] a)",
        "(let [(quote a) 1] a)",
        "(let [[1] [2]] 1)",
        "(let [& 1] &)",
        "(let [a 1 & [2]] a)",
        "(let [[a &] [1]] a)",
        "(let [u/x 1] u/x)",
        "(let [{x :x :or 5} {:x 1}] x)",
        "(let [{:as 5} {}] 5)",
        "(let [{a :x :or nil} {:x 1}] a)",
        "(let [{:keys [a] :req [a]} {:a 1}] a)",
    ] {
        check(src, wrapper, cause);
    }
}

#[test]
fn fn_spec_errors() {
    let wrapper = "Syntax error macroexpanding clojure.core/fn at (test:1:1).";
    let cause = "clojure.lang.ExceptionInfo | Call to clojure.core/fn did not conform to spec.";
    for src in [
        "(fn [1] 1)",
        "(fn [#{a}] a)",
        "(fn [(quote a)] a)",
        "(fn [[1]] 1)",
        "(fn [a &] a)",
        "(fn)",
        "(fn ([x] x) [y])",
        "(fn nm [1] 1)",
        "(fn [u/x] u/x)",
        "(fn [{:as 5}] 1)",
        "(fn [{:keys [5]}] 1)",
    ] {
        check(src, wrapper, cause);
    }
}

#[test]
fn loop_assert_errors() {
    check(
        "(loop [x] x)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | loop requires an even number of forms in binding vector",
    );
    check(
        "(loop x x)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | loop requires a vector for its binding",
    );
    check(
        "(loop [1 2] 1)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form: 1",
    );
    check(
        "(loop [[a & b c] [1 2]] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form, only :as can follow & parameter",
    );
    check(
        "(loop [[a &] [1]] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form: ",
    );
    check(
        "(loop [a 1 & [2]] a)",
        "Syntax error compiling loop* at (test:1:1).",
        "java.lang.RuntimeException | Can't use & as a local binding",
    );
    check(
        "(loop [u/x 1] u/x)",
        "Syntax error compiling loop* at (test:1:1).",
        "java.lang.RuntimeException | Can't let qualified name: u/x",
    );
    check(
        "(loop [{:keys 5} {}] 1)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long",
    );
    check(
        "(loop [{a :x :or 5} {:x 1}] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long",
    );
    check(
        "(loop [{:keys [5]} {}] 1)",
        "Unexpected error macroexpanding loop at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Named",
    );
    check(
        "(loop [{a :x :or \"ab\"} {:x 1}] a)",
        "Unexpected error macroexpanding loop at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast java.lang.Character to java.util.Map$Entry",
    );
    // Emission re-validation routes through the inner `let`'s spec.
    check(
        "(loop [{:as 5} {}] 1)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
    );
    check(
        "(loop [& 1 [b] 2] b)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
    );
    check(
        "(loop [u/x 1 [b] 2] b)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
    );
}

#[test]
fn letfn_errors() {
    check(
        "(letfn [f] 1)",
        "Syntax error macroexpanding letfn at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Symbol",
    );
    check(
        "(letfn [5] 1)",
        "Syntax error macroexpanding letfn at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long",
    );
    check(
        "(letfn [(f)] 1)",
        "Syntax error macroexpanding clojure.core/fn at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/fn did not conform to spec.",
    );
    check(
        "(letfn [(1 [x] x)] 1)",
        "Syntax error compiling letfn* at (test:1:1).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: 1",
    );
    check(
        "(letfn [[]] 1)",
        "Syntax error compiling letfn* at (test:1:1).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: null",
    );
}

#[test]
fn fn_star_errors() {
    check(
        "(fn ([x] x) ([y] y))",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't have 2 overloads with same arity",
    );
    check(
        "(fn ([x y] x) ([x & r] x))",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't have fixed arity function with more params than variadic function",
    );
    check(
        "(fn ([x & r] x) ([a b] a))",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't have fixed arity function with more params than variadic function",
    );
    check(
        "(fn ([x & r] x) ([a & s] a))",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't have more than 1 variadic overload",
    );
    check(
        "(fn [x] {:pre 5} x)",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long",
    );
    let params21 = (0..21)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    check(
        &format!("(fn [{params21}] 1)"),
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't specify more than 20 params",
    );
}

#[test]
fn destructure_semantic_errors() {
    check(
        "(let [{a :x :or {a 1 :x 2}} {:x 3}] a)",
        "Syntax error macroexpanding let at (test:1:1).",
        "java.lang.Exception | Multiple :or defaults for same key: :x 'a'",
    );
    check(
        "(let [{:keys [a] :select s :or {zzz 1}} {:a 1}] s)",
        "Syntax error macroexpanding let at (test:1:1).",
        "java.lang.IllegalArgumentException | symbol zzz in :or does not refer to a binding",
    );
    check(
        "(let [{a :x :select s :or {:y 1}} {:x 5}] s)",
        "Syntax error macroexpanding let at (test:1:1).",
        "java.lang.IllegalArgumentException | keys #{:y} appear only in :or",
    );
    check(
        "(let [{:keys! [a] :or {a 1}} {:a 2}] a)",
        "Syntax error macroexpanding let at (test:1:1).",
        "java.lang.Exception | Can't supply default value for required key: :a",
    );
    // `fn` parameters destructure through the emitted `let`.
    check(
        "(fn [{a :x :or {a 1 :x 2}}] a)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "java.lang.Exception | Multiple :or defaults for same key: :x 'a'",
    );
}

#[test]
fn binding_errors() {
    check(
        "(binding [x] x)",
        "Syntax error macroexpanding binding at (test:1:1).",
        "java.lang.IllegalArgumentException | binding requires an even number of forms in binding vector",
    );
    check(
        "(binding x x)",
        "Syntax error macroexpanding binding at (test:1:1).",
        "java.lang.IllegalArgumentException | binding requires a vector for its binding",
    );
    check(
        "(binding [1 2] 1)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol",
    );
}

#[test]
fn bare_macro_arity() {
    check_bare(
        "(loop)",
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/loop",
    );
    check_bare(
        "(binding)",
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/binding",
    );
    check_bare(
        "(letfn)",
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/letfn",
    );
}

#[test]
fn nested_span() {
    check(
        "(do\n  (if 1))",
        "Syntax error compiling if at (test:2:3).",
        "java.lang.RuntimeException | Too few arguments to if",
    );
}

#[test]
fn valid_shapes_pass() {
    let mut eval = Evaluator::new("test");
    for setup in ["(def va251 7)", "(def user/q251 1)"] {
        let form = read_str(setup, &ReaderOpts::default()).unwrap();
        let _ = eval.analyze(&form);
    }
    for src in [
        "(do)",
        "(do 1 2 3)",
        "()",
        "(quote ())",
        "(try 1 2 3)",
        "(try)",
        "(try 1 (catch Exception e))",
        "(try 1 (finally))",
        "(def user/q251 1)",
        "(var va251 b)",
        "(fn [& r] r)",
        "(fn [a & r] r)",
        "(fn ([x] x) ([x & r] r))",
        "(let [[& r] [1 2]] r)",
        "(loop [[a] [1]] a)",
        "(letfn [(f [x] x)] (f 1))",
        "(if 1 2)",
        "(if 1 2 3)",
    ] {
        let form = read_str(src, &ReaderOpts::default()).unwrap();
        if let Err(e) = eval.analyze(&form) {
            panic!("expected {src} to analyze: {}", e.cause());
        }
    }
    // 20 params is legal.
    let params20 = (0..20)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    let form = read_str(&format!("(fn [{params20}] 1)"), &ReaderOpts::default()).unwrap();
    assert!(eval.analyze(&form).is_ok());
}
