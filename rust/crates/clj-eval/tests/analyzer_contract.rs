// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Analyzer contract tests: every §E2–E6/E11/E12 shape error, with
//! oracle-verbatim classes, messages, and wrappers (probes 20–29).
//! Single-line sources blame 1:1; one multi-line case pins real
//! spans.

use clj_eval::{Evaluator, boot};
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

fn analyze_ok(src: &str) {
    let mut eval = Evaluator::new("test");
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
    if let Err(e) = eval.analyze(&form) {
        panic!("expected {src} to analyze: {}", e.cause());
    }
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
    check(
        "(def a 1 2 3)",
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
    // `loop` inits keep the outer fence (probe30 `fence-loop-init`).
    check(
        "(loop [x 1] (loop [n (try (recur 5) (catch Exception e 2))] n))",
        "Syntax error compiling recur at (test:1:27).",
        "java.lang.UnsupportedOperationException | Cannot recur across try",
    );
    // Catch/finally bodies reset the fence and hide the target: any
    // `recur` there is the tail-position error (probe30).
    for (src, col) in [
        ("(try (throw 42) (catch Exception e (recur 1)))", 36),
        ("(try 1 (finally (recur 1)))", 17),
        (
            "(try (loop [a 1] (try 1 (catch Exception e (recur 1)))) (catch Exception f 2))",
            44,
        ),
    ] {
        check(
            src,
            &format!("Syntax error compiling recur at (test:1:{col})."),
            "java.lang.UnsupportedOperationException | Can only recur from tail position",
        );
    }
    // A nested `try` body inside a clause re-arms the fence.
    check(
        "(loop [a 1] (try 1 (catch Exception e (try (recur 1) (catch Exception f 2)))))",
        "Syntax error compiling recur at (test:1:44).",
        "java.lang.UnsupportedOperationException | Cannot recur across try",
    );
    // Arg analysis precedes the arity check (failures keep their own
    // wrappers); targetless never consults tail (probe30 `recur-*`).
    check(
        "(loop [a 1] (recur nope-xyz q))",
        "Syntax error compiling at (test:1:13).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope-xyz in this context",
    );
    check(
        "(do (recur nope-xyz) 2)",
        "Syntax error compiling at (test:1:5).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope-xyz in this context",
    );
    check(
        "(do (recur 1) 2)",
        "Syntax error compiling recur at (test:1:5).",
        "java.lang.IllegalArgumentException | Mismatched argument count to recur, expected: 0 args, got: 1",
    );
    check(
        "(loop [a 1] (do (recur nope-xyz q) 3))",
        "Syntax error compiling recur at (test:1:17).",
        "java.lang.UnsupportedOperationException | Can only recur from tail position",
    );
    // Targetless `recur` in a `try` body is across-`try`, not the
    // 0-vs-N mismatch (probe30 `recur-targetless-try`).
    check(
        "(try (recur 1) (catch Exception e 2))",
        "Syntax error compiling recur at (test:1:6).",
        "java.lang.UnsupportedOperationException | Cannot recur across try",
    );
}

#[test]
fn try_shapes() {
    check(
        "(try 1 (catch))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: null",
    );
    check(
        "(try 1 (catch String))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: null",
    );
    check(
        "(try 1 (catch String \"s\" 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: s",
    );
    check(
        "(try 1 (catch Exception q/e 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.RuntimeException | Can't bind qualified name:q/e",
    );
    check(
        "(try 1 (catch NopeZZZ e 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: NopeZZZ",
    );
    check(
        "(try 1 (catch q/NopeZZZ e 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: q/NopeZZZ",
    );
    check(
        "(try 1 (catch 5 e 1))",
        "Syntax error compiling try at (test:1:1).",
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
        "(try 1 (finally 1) 2)",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.RuntimeException | finally clause must be last in try expression",
    );
    // Unknown dotted names are `ClassNotFoundException` (probe32).
    for (class, fqn) in [
        ("foo.bar.Baz", "foo.bar.Baz"),
        ("java.io.Nope", "java.io.Nope"),
        ("java.lang.Nope", "java.lang.Nope"),
        ("clojure.lang.Nope", "clojure.lang.Nope"),
    ] {
        check(
            &format!("(try 1 (catch {class} e 1))"),
            "Syntax error compiling try at (test:1:1).",
            &format!("java.lang.ClassNotFoundException | {fqn}"),
        );
    }
    // The `ArityException` short does not resolve (probe32).
    check(
        "(try 1 (catch ArityException e 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: ArityException",
    );
}

#[test]
fn catch_class_table() {
    // Every `java.lang` throwable short, the `ExceptionInfo` short,
    // and the curated FQNs resolve (probe32 `catch-*`).
    let mut names = vec![
        "InterruptedException",
        "NumberFormatException",
        "IllegalMonitorStateException",
        "IllegalThreadStateException",
        "ArrayIndexOutOfBoundsException",
        "StringIndexOutOfBoundsException",
        "NegativeArraySizeException",
        "ArrayStoreException",
        "NoSuchMethodError",
        "NoSuchFieldError",
        "AbstractMethodError",
        "UnsatisfiedLinkError",
        "ExceptionInInitializerError",
        "StackOverflowError",
        "OutOfMemoryError",
        "ClassFormatError",
        "NoClassDefFoundError",
        "VerifyError",
        "LinkageError",
        "ClassCircularityError",
        "IncompatibleClassChangeError",
        "InstantiationError",
        "InternalError",
        "UnknownError",
        "ThreadDeath",
        "ReflectiveOperationException",
        "ClassNotFoundException",
        "IllegalAccessException",
        "InstantiationException",
        "NoSuchMethodException",
        "NoSuchFieldException",
        "CloneNotSupportedException",
        "SecurityException",
        "TypeNotPresentException",
        "EnumConstantNotPresentException",
        "ExceptionInfo",
        "java.io.IOException",
        "java.io.FileNotFoundException",
        "java.io.EOFException",
        "java.io.UncheckedIOException",
        "java.util.NoSuchElementException",
        "java.util.concurrent.ExecutionException",
        "java.lang.reflect.InvocationTargetException",
        "clojure.lang.Compiler$CompilerException",
    ];
    assert_eq!(names.len(), 44);
    names.sort_unstable();
    names.dedup();
    assert_eq!(names.len(), 44);
    for class in names {
        let src = format!("(try 1 (catch {class} e 1))");
        let form = read_str(&src, &ReaderOpts::default()).unwrap();
        Evaluator::new("test").analyze(&form).unwrap();
    }
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
fn macro_shadowing() {
    // The five macros defer to local bindings (probe31 `shadow-*`):
    // the form analyzes as an invocation, and the error blames the
    // nearest enclosing list (vectors are span-transparent).
    for (src, col, name) in [
        ("(let [loop 5] (loop [x 1] x))", 15, "x"),
        ("(let [fn 5] (fn [x] x))", 13, "x"),
        ("(let [let 5] (let [x 1] x))", 14, "x"),
        ("(let [binding 5] (binding [x 1] x))", 18, "x"),
        ("(let [letfn 5] (letfn [(f [x] x)] (f 1)))", 24, "f"),
    ] {
        check(
            src,
            &format!("Syntax error compiling at (test:1:{col})."),
            &format!(
                "java.lang.RuntimeException | Unable to resolve symbol: {name} in this context"
            ),
        );
    }
    // A user-ns global shadows too (probe31 `shadow-global`).
    let mut eval = Evaluator::new("test");
    let setup = read_str("(def loop 5)", &ReaderOpts::default()).unwrap();
    let _ = eval.analyze(&setup);
    let form = read_str("(loop [x 1] x)", &ReaderOpts::default()).unwrap();
    assert_eq!(
        eval.analyze(&form).unwrap_err().chain(),
        vec![
            "Syntax error compiling at (test:1:1).".to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: x in this context".to_owned(),
        ]
    );
}

#[test]
fn span_lists_only() {
    // Vectors and maps are span-transparent; only lists rebind the
    // span (probe31 `span-*`).
    check(
        "(do [nope] 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope in this context",
    );
    check(
        "(do {:a nope} 1)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope in this context",
    );
    check(
        "(do (t-plus nope 1) 2)",
        "Syntax error compiling at (test:1:5).",
        "java.lang.RuntimeException | Unable to resolve symbol: t-plus in this context",
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
        "(let [{:foo/strs [a]} {}] a)",
        "(let [{:foo/strs! [a]} {}] a)",
        "(let [{:foo/keys [b/c]} {}] a)",
        "(let [{:strs [b/c]} {}] a)",
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
        "(fn u/f [x] x)",
        "(fn [{:as 5}] 1)",
        "(fn [{:keys [5]}] 1)",
        "(fn [{:foo/strs [a]}] a)",
        "(fn [{:foo/keys [b/c]}] a)",
        // fx2-mapentry19.fn-empty: the JVM rejects the empty
        // clause too (CompilerException, spec `:params`).
        "(fn ())",
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
    // `:as` values validate immediately, before emission (probe31):
    // scalars fail the walk, shapes pass and die at the inner spec.
    check(
        "(loop [[a :as 5] [1]] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form: 5",
    );
    check(
        "(loop [[a :as] [1]] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form: ",
    );
    check(
        "(loop [[a :as \"s\"] [1]] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported binding form: s",
    );
    check(
        "(loop [[a :as [b]] [1 2]] a)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
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
fn keys_element_cast_matrix() {
    // Only idents pass the key-list walk (probe32): anything else
    // takes the `(namespace x)` cast — CCE to Named, the
    // helpful-NPE text for nil — except under `:strs`, which
    // never casts and reports `Unsupported binding form`.
    let unexp = "Unexpected error macroexpanding loop at (test:1:1).";
    for (src, cause) in [
        (
            "(loop [{:keys [\"a\"]} {}] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Named",
        ),
        (
            "(loop [{:syms [\"a\"]} {}] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Named",
        ),
        (
            "(loop [{:keys [true]} {}] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.Boolean to clojure.lang.Named",
        ),
        (
            "(loop [{:keys [nil]} {}] a)",
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null",
        ),
        (
            "(loop [{:syms [nil]} {}] a)",
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null",
        ),
        // Shapes take the same Named cast (probe33 `keys-vec`,
        // `keys-map`, `syms-set`).
        (
            "(loop [{:keys [[a]]} {}] a)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.Named",
        ),
        (
            "(loop [{:keys [{a 1}]} {}] a)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentArrayMap to clojure.lang.Named",
        ),
        (
            "(loop [{:syms [#{a}]} {}] a)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentHashSet to clojure.lang.Named",
        ),
        // Qualified directives short-circuit `(namespace %)`: only
        // `(name %)` runs, so vectors/numbers keep the same CCE
        // text but nil takes the `getName` NPE (probe33
        // `qual-cast-*`).
        (
            "(loop [{:foo/keys [[a]]} {}] a)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.Named",
        ),
        (
            "(loop [{:foo/keys [5]} {}] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Named",
        ),
        (
            "(loop [{:foo/keys [nil]} {}] a)",
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null",
        ),
    ] {
        check(src, unexp, cause);
    }
    let syn = "Syntax error macroexpanding loop at (test:1:1).";
    for (src, cause) in [
        (
            "(loop [{:strs [\"a\"]} {}] a)",
            "java.lang.Exception | Unsupported binding form: a",
        ),
        (
            "(loop [{:strs [5]} {}] a)",
            "java.lang.Exception | Unsupported binding form: 5",
        ),
        (
            "(loop [{:strs [nil]} {}] a)",
            "java.lang.Exception | Unsupported binding form: ",
        ),
        // Qualified strings skip the cast into `push1`/`pb`
        // (probe33 `qual-cast-str`, `qual-cast-syms-str`).
        (
            "(loop [{:foo/keys [\"a\"]} {}] a)",
            "java.lang.Exception | Unsupported binding form: a",
        ),
        (
            "(loop [{:foo/syms [\"a\"]} {}] a)",
            "java.lang.Exception | Unsupported binding form: a",
        ),
        // Shapes under `:strs` walk as nested patterns: malformed
        // content still throws (probe33 `strs-shape-nested`,
        // `strs-shape-set`).
        (
            "(loop [{:strs [[5]]} {}] a)",
            "java.lang.Exception | Unsupported binding form: 5",
        ),
        (
            "(loop [{:strs [#{a}]} {}] a)",
            "java.lang.Exception | Unsupported binding form: #{a}",
        ),
    ] {
        check(src, syn, cause);
    }
    // Keywords ride the fast path untouched: `:strs`/`:syms`
    // walk fine and die at the inner spec (probe32), as do
    // `:strs` shapes (probe33 `strs-shape-*`).
    for src in [
        "(loop [{:strs [:a]} {}] a)",
        "(loop [{:syms [:a]} {}] a)",
        "(loop [{:strs [[a]]} {}] a)",
        "(loop [{:strs [{a 1}]} {}] a)",
        "(loop [{:foo/strs [[a]]} {}] a)",
    ] {
        check(
            src,
            "Syntax error macroexpanding clojure.core/let at (test:1:1).",
            "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
        );
    }
}

#[test]
fn star_form_errors() {
    // Direct `let*`/`loop*` take raw symbol bindings (probe33):
    // missing or non-vector bindings, odd counts, and
    // non-symbol bindings fail with `Bad binding form`.
    let star_let = "Syntax error compiling let* at (test:1:1).";
    for (src, cause) in [
        (
            "(let* [a] a)",
            "java.lang.IllegalArgumentException | Bad binding form, expected matched symbol expression pairs",
        ),
        (
            "(let* a a)",
            "java.lang.IllegalArgumentException | Bad binding form, expected vector",
        ),
        (
            "(let* [[a] [1]] a)",
            "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: [a]",
        ),
        (
            "(let* [a 1 & [2]] a)",
            "java.lang.RuntimeException | Can't use & as a local binding",
        ),
        (
            "(let*)",
            "java.lang.IllegalArgumentException | Bad binding form, expected vector",
        ),
    ] {
        check(src, star_let, cause);
    }
    let star_loop = "Syntax error compiling loop* at (test:1:1).";
    for (src, cause) in [
        (
            "(loop* [a] a)",
            "java.lang.IllegalArgumentException | Bad binding form, expected matched symbol expression pairs",
        ),
        (
            "(loop* [{a :x} {:x 1}] a)",
            "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: {a :x}",
        ),
        (
            "(loop* [u/a 1] a)",
            "java.lang.RuntimeException | Can't let qualified name: u/a",
        ),
        (
            "(loop*)",
            "java.lang.IllegalArgumentException | Bad binding form, expected vector",
        ),
    ] {
        check(src, star_loop, cause);
    }
    // Direct `fn*` is raw `FnExpr`: a leading symbol is always
    // the name, arity items are seqs, params are plain symbols
    // (probe33).
    let star_fn = "Syntax error compiling fn* at (test:1:1).";
    for (src, cause) in [
        (
            "(fn* a a)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.Symbol to clojure.lang.ISeq",
        ),
        (
            "(fn* 5 [a] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.ISeq",
        ),
        (
            "(fn* ([a] a) [b])",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.ISeq",
        ),
        (
            "(fn* {:a 1})",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentArrayMap to clojure.lang.ISeq",
        ),
        (
            "(fn* \"doc\" [a] a)",
            "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.ISeq",
        ),
        (
            "(fn* [1] 1)",
            "java.lang.IllegalArgumentException | fn params must be Symbols",
        ),
        (
            "(fn* [[a]] a)",
            "java.lang.IllegalArgumentException | fn params must be Symbols",
        ),
        (
            "(fn* [u/a] a)",
            "java.lang.RuntimeException | Can't use qualified name as parameter: u/a",
        ),
        (
            "(fn* [a & 5] a)",
            "java.lang.IllegalArgumentException | fn params must be Symbols",
        ),
        (
            "(fn* [a & u/b] a)",
            "java.lang.RuntimeException | Can't use qualified name as parameter: u/b",
        ),
        (
            "(fn* [& &] 1)",
            "java.lang.RuntimeException | Invalid parameter list",
        ),
        (
            "(fn* [a & &] 1)",
            "java.lang.RuntimeException | Invalid parameter list",
        ),
        (
            "(fn* [a & b & c] 1)",
            "java.lang.RuntimeException | Invalid parameter list",
        ),
        (
            "(fn* [a & b c] a)",
            "java.lang.RuntimeException | Unexpected parameter",
        ),
        (
            "(fn* (a b))",
            "java.lang.ClassCastException | Cannot cast clojure.lang.Symbol to clojure.lang.IPersistentVector",
        ),
        (
            "(fn* ())",
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IPersistentVector.count()\" because \"arglist\" is null",
        ),
        (
            "(fn* nil)",
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IPersistentVector.count()\" because \"arglist\" is null",
        ),
        (
            "(fn* ([a] a) ([b] b))",
            "java.lang.RuntimeException | Can't have 2 overloads with same arity",
        ),
        (
            "(fn* ([a & r] a) ([b & s] b))",
            "java.lang.RuntimeException | Can't have more than 1 variadic overload",
        ),
        (
            "(fn* ([a b] a) ([x & r] x))",
            "java.lang.RuntimeException | Can't have fixed arity function with more params than variadic function",
        ),
    ] {
        check(src, star_fn, cause);
    }
    let params21 = (0..21)
        .map(|n| format!("p{n}"))
        .collect::<Vec<_>>()
        .join(" ");
    check(
        &format!("(fn* [{params21}] 1)"),
        star_fn,
        "java.lang.RuntimeException | Can't specify more than 20 params",
    );
    // Star bodies share the resolve and recur machinery; the
    // wrapper names the failing subform (probe33). A qualified
    // `fn*` name binds only its simple name, so a qualified
    // self-call resolves globally.
    check(
        "(let* [a 1] nope321)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope321 in this context",
    );
    check(
        "(loop* [a 1] (recur 1 2))",
        "Syntax error compiling recur at (test:1:14).",
        "java.lang.IllegalArgumentException | Mismatched argument count to recur, expected: 1 args, got: 2",
    );
    check(
        "((fn* u/foo327 [a] (if a :d (u/foo327 a))) 2)",
        "Syntax error compiling at (test:1:29).",
        "java.lang.RuntimeException | No such namespace: u",
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
    check(
        "(letfn [(u/f [x] x)] 1)",
        "Syntax error compiling letfn* at (test:1:1).",
        "java.lang.RuntimeException | Can't let qualified name: u/f",
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
    // Per-clause order: 20-count, walk, body, dup checks (probe32).
    check(
        "(fn ([x] x) ([y] y) ([{a :x :or {a 1 :x 2}}] a))",
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't have 2 overloads with same arity",
    );
    check(
        "(fn ([x] nope) ([y] y) ([z] z))",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope in this context",
    );
    check(
        "(fn ([x] x) ([y] nope))",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope in this context",
    );
    let or21 = format!(
        "(fn ([{{a :x :or {{a 1 :x 2}}}} {}] x))",
        (0..20)
            .map(|n| format!("p{n}"))
            .collect::<Vec<_>>()
            .join(" ")
    );
    check(
        &or21,
        "Syntax error compiling fn* at (test:1:1).",
        "java.lang.RuntimeException | Can't specify more than 20 params",
    );
    check(
        "(fn ([x & r] x) ([a b] a) ([{a :x :or {a 1 :x 2}}] z))",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "java.lang.Exception | Multiple :or defaults for same key: :x 'a'",
    );
    check(
        "(fn [{a :x :or {a 1 :x 2}}] nope)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "java.lang.Exception | Multiple :or defaults for same key: :x 'a'",
    );
    // Clause lists are span-transparent, but body lists rebind.
    check(
        "(fn [x] (do nope 2))",
        "Syntax error compiling at (test:1:9).",
        "java.lang.RuntimeException | Unable to resolve symbol: nope in this context",
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
    // The key renders via `str`, so a `:strs` key prints bare
    // (probe33 `or-multi-strs`).
    check(
        "(loop [{:strs [s] :or {s 1 \"s\" 2}} {}] s)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Multiple :or defaults for same key: s 's'",
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
        "(let [{:foo/keys [a]} {}] a)",
        "(let [{:foo/syms [a]} {}] a)",
        "(let [{:foo/keys! [a]} {}] a)",
        "(let [{:foo/syms! [a]} {}] a)",
        "(let [{:keys [b/c]} {}] c)",
        "(fn [{:foo/keys [a]}] a)",
        "(loop [{:foo/keys [a]} {}] a)",
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

#[test]
fn gap2_catch_explicit_nil() {
    // An explicit nil classname renders via value_of (expr.rs:465).
    check(
        "(try 1 (catch nil e 1))",
        "Syntax error compiling try at (test:1:1).",
        "java.lang.IllegalArgumentException | Unable to resolve classname: null",
    );
}

#[test]
fn gap2_unresolvable_core_builtin() {
    // No clojure.core ns here, so the qualified builtin misses (expr.rs:873).
    check(
        "(clojure.core/let [x 1] x)",
        "Syntax error compiling at (test:1:1).",
        "java.lang.RuntimeException | No such namespace: clojure.core",
    );
}

#[test]
fn gap2_loop_walk_rows() {
    // First-fault walk errors plus key-list seq shapes (expr.rs:2243, 2320, 2354-2406, 3270-3276).
    check(
        "(loop [{:foo bar} {:foo 1}] bar)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.Exception | Unsupported map directive: :foo",
    );
    check(
        "(loop [{:or []} {}] 1)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
    );
    check(
        "(loop [{:keys [a & &]} {}] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | & can only appear once in :keys",
    );
    check(
        "(loop [{:keys [a & b]} {}] a)",
        "Syntax error macroexpanding loop at (test:1:1).",
        "java.lang.IllegalArgumentException | 'b' - binding symbols can only appear before '&', use keys after",
    );
    check(
        "(loop [{:keys {:a 1}} {}] 1)",
        "Unexpected error macroexpanding loop at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.Named",
    );
    check(
        "(loop [{:keys #{a}} {}] a)",
        "Syntax error macroexpanding clojure.core/let at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.",
    );
    check(
        "(loop [{:keys \"ab\"} {}] 1)",
        "Unexpected error macroexpanding loop at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast java.lang.Character to clojure.lang.Named",
    );
}

#[test]
fn gap2_case_star_spec_rows() {
    // Accident-shaped slots keep the spec error (T1192 FX1): the
    // JVM answers these with NPE/IOOBE accidents or
    // compiler-internal casts (frozen wedge), or they are
    // macro-unreachable (out-of-range ints route to hash mode).
    let wrapper = "Syntax error macroexpanding clojure.core/case* at (test:1:1).";
    let cause = "clojure.lang.ExceptionInfo | Call to clojure.core/case* did not conform to spec.";
    for src in [
        "(case* 1 0 0 1 {} :bad :int)",
        "(case* 1 0 0 1 {} :sparse :bad)",
        "(case* 1 0 0 1 {} :sparse :int #{:a})",
        "(case* 1 0 0 1 {1 2} :sparse :int)",
        "(case* 1 0 0 1 {1 [1]} :sparse :int)",
        "(case* 1 4294967296 0 1 {} :sparse :int)",
        "(case* 1 0 0 1 {4294967296 [1 2]} :sparse :int)",
    ] {
        check(src, wrapper, cause);
    }
}

#[test]
fn t1192_case_star_cast_rows() {
    // Wrong-typed slots are the JVM's deterministic casts (T1192
    // FX1 probes): `Syntax error compiling case*` + E8 cast text.
    let wrapper = "Syntax error compiling case* at (test:1:1).";
    for (src, cause) in [
        (
            "(case* 1 :bad 0 1 {} :sparse :int)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to java.lang.Number",
        ),
        (
            "(case* 1 0 \"m\" 1 {} :sparse :int)",
            "java.lang.ClassCastException | Cannot cast java.lang.String to java.lang.Number",
        ),
        (
            "(case* 1 0 0 1 :bad :sparse :int)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to java.util.Map",
        ),
        (
            "(case* 1 0 0 1 {} 5 :int)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Keyword",
        ),
        (
            "(case* 1 0 0 1 {} :sparse 5)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Keyword",
        ),
        (
            "(case* 1 0 0 1 {} :sparse :int 5)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to java.util.Set",
        ),
        (
            "(case* 1 0 0 1 {:a [1 2]} :sparse :int)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to java.lang.Number",
        ),
    ] {
        check(src, wrapper, cause);
    }
}

#[test]
fn gap2_case_star_skip_ok() {
    // A valid skip set collects (expr.rs:2818).
    analyze_ok("(case* 1 0 0 1 {} :sparse :int #{1})");
}

#[test]
fn gap2_with_redefs_binding_rows() {
    // Vector/nil/set/empty-map bindings pass; maps fail the Symbol cast (expr.rs:1770-1784).
    analyze_ok("(with-redefs [va251 2] va251)");
    analyze_ok("(with-redefs nil 1)");
    analyze_ok("(with-redefs #{} 1)");
    analyze_ok("(with-redefs {} 1)");
    check(
        "(with-redefs {:a 1} 1)",
        "Syntax error compiling var at (test:1:1).",
        "java.lang.ClassCastException | Cannot cast clojure.lang.MapEntry to clojure.lang.Symbol",
    );
}

#[test]
fn gap2_slash_class_ok() {
    // Slash-qualified dotted classes run the FQN rule (expr.rs:1615, 1628).
    analyze_ok("(try 1 (catch java.lang/Exception e 1))");
}

#[test]
fn gap2_let_spec_tail_rows() {
    // Mid-vector :as, bad rests, bad keys, non-vector key lists (expr.rs:2084-2165).
    let wrapper = "Syntax error macroexpanding clojure.core/let at (test:1:1).";
    let cause = "clojure.lang.ExceptionInfo | Call to clojure.core/let did not conform to spec.";
    for src in [
        "(let [[a :as] [1 2]] a)",
        "(let [[x & 5] [1 2]] x)",
        "(let [{5 :a} {}] 5)",
        "(let [{:keys foo} {}] foo)",
    ] {
        check(src, wrapper, cause);
    }
}

#[test]
fn gap2_fn_param_spec_rows() {
    // Keywords in params and non-vector arities (expr.rs:3286, 3295, 3801).
    let wrapper = "Syntax error macroexpanding clojure.core/fn at (test:1:1).";
    let cause = "clojure.lang.ExceptionInfo | Call to clojure.core/fn did not conform to spec.";
    for src in ["(fn [a :k] a)", "(fn [a :k & r] a)", "(fn (5 1))"] {
        check(src, wrapper, cause);
    }
}

#[test]
fn gap2_letfn_binding_rows() {
    // Nil bindings, non-seqable bindings, nil fnspecs (expr.rs:3267, 3694-3705).
    analyze_ok("(letfn nil 1)");
    check(
        "(letfn 5 1)",
        "Syntax error macroexpanding letfn at (test:1:1).",
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long",
    );
    check(
        "(letfn [nil] 1)",
        "Syntax error compiling letfn* at (test:1:1).",
        "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: null",
    );
}

#[test]
fn gap2_prepost_seq_rows() {
    // List/set/string preconditions seq through (expr.rs:3868, 3871, 3872).
    analyze_ok("(fn [x] {:pre (x)} x)");
    analyze_ok("(fn [x] {:pre #{x}} x)");
    analyze_ok("(fn [x] {:pre \"ab\"} x)");
}

#[test]
fn gap2_ns_clause_head_row() {
    // A non-keyword clause head is the ns spec error (expr.rs:2005).
    check(
        "(ns foo (require x))",
        "Syntax error macroexpanding clojure.core/ns at (test:1:1).",
        "clojure.lang.ExceptionInfo | Call to clojure.core/ns did not conform to spec.",
    );
}

#[test]
fn gap2_private_macro_direct_analyze() {
    // Direct analysis (no eval_top_form pre-expansion) takes the bare arm (expr.rs:936-942).
    let mut eval = boot();
    eval.eval_source("(create-ns 'fx936)").unwrap();
    eval.eval_source("(ns fx936)").unwrap();
    eval.eval_source("(defmacro ^:private privm [x] (list 'quote x))")
        .unwrap();
    eval.eval_source("(ns user)").unwrap();
    let form = read_str("(fx936/privm 1)", &ReaderOpts::default()).unwrap();
    match eval.analyze(&form) {
        Err(e) => assert_eq!(
            e.chain(),
            vec!["java.lang.IllegalStateException | var: #'fx936/privm is not public".to_owned()]
        ),
        Ok(_) => panic!("expected a bare private-macro error"),
    }
}

#[test]
fn gap2_shadowed_builtin_macro_invoke() {
    // A local shadows the `let` builtin, so the inner form analyzes
    // as an invocation of the local (expr.rs:906).
    let mut eval = boot();
    let form = read_str("(let [let 5] (let 1))", &ReaderOpts::default()).unwrap();
    assert!(eval.analyze(&form).is_ok());
}

#[test]
fn gap2_core_alias_shadow() {
    // An alias named clojure.core redirects the builtin check (expr.rs:876).
    let mut eval = boot();
    eval.eval_source("(create-ns 'aa876)").unwrap();
    eval.eval_source("(ns aa876)").unwrap();
    eval.eval_source("(def let 42)").unwrap();
    eval.eval_source("(in-ns 'user)").unwrap();
    eval.eval_source("(alias 'clojure.core 'aa876)").unwrap();
    let form = read_str("(clojure.core/let [1 2] 3)", &ReaderOpts::default()).unwrap();
    assert!(eval.analyze(&form).is_ok());
}

#[test]
fn gap2_def_outside_current_ns() {
    // A qualified def onto an existing foreign var (expr.rs:1051).
    let mut eval = boot();
    let form = read_str("(def clojure.core/map 1)", &ReaderOpts::default()).unwrap();
    match eval.analyze(&form) {
        Err(e) => assert_eq!(
            e.chain(),
            vec![
                "Syntax error compiling def at (user:1:1).".to_owned(),
                "java.lang.RuntimeException | Can't create defs outside of current ns".to_owned(),
            ]
        ),
        Ok(_) => panic!("expected a cross-ns def error"),
    }
}

#[test]
fn gap2_seq_form_force_throw() {
    // Analyzing a seq value forces it; the throw escapes bare (expr.rs:623-629).
    let mut eval = boot();
    let mut vs = eval
        .eval_source("(lazy-seq (throw (ex-info \"boom623\" {})))")
        .unwrap();
    let seq = vs.pop().unwrap();
    match eval.analyze(&seq) {
        Err(e) => assert_eq!(
            e.chain(),
            vec!["clojure.lang.ExceptionInfo | boom623".to_owned()]
        ),
        Ok(_) => panic!("expected the forced seq to throw"),
    }
}
