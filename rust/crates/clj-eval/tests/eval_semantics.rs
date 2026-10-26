// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Evaluator semantics: values for every special form, the §E7
//! invoke table, and the §E12 destructure engine, via injected
//! native doubles (S4 ships no prelude; full probe replay waits
//! for the S5 core subset).

use std::sync::{Arc, Mutex};

use clj_eval::{Evaluator, NativeError, SourceError};
use clj_value::{PrintOpts, Symbol, ThrowClass, Throwable, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn int(args: &[Value]) -> Result<i64, NativeError> {
    match args {
        [Value::Int(n)] => Ok(*n),
        _ => Err(NativeError::arity("test", args.len())),
    }
}

/// An evaluator with arithmetic/log natives plus a shared side-log.
fn rigged() -> (Evaluator, Arc<Mutex<Vec<String>>>) {
    let mut eval = Evaluator::new("test");
    let log: Arc<Mutex<Vec<String>>> = Arc::new(Mutex::new(Vec::new()));
    install_arith(&mut eval);
    install_int_fns(&mut eval);
    install_misc(&mut eval, &log);
    eval.eval_source("(def vfn251 (fn [x] x)) (def vu251)")
        .unwrap();
    (eval, log)
}

/// `t-plus` (with the `Number` cast) and `t-minus`. Checked
/// arithmetic: like `clojure.core/+`, overflow is
/// `ArithmeticException`, never a debug panic.
fn install_arith(eval: &mut Evaluator) {
    eval.install_native(
        "user",
        "t-plus",
        Arc::new(move |_, args| match args {
            [Value::Int(a), Value::Int(b)] => a.checked_add(*b).map(Value::Int).ok_or_else(|| {
                NativeError::new(
                    ThrowClass::ArithmeticException,
                    "integer overflow".to_owned(),
                )
            }),
            [a, b] => Err(NativeError::cast(
                &clj_eval::jvm_class_of(if matches!(a, Value::Int(_)) { b } else { a }),
                "java.lang.Number",
            )),
            _ => Err(NativeError::arity("test/t-plus", args.len())),
        }),
    );
    eval.install_native(
        "user",
        "t-minus",
        Arc::new(|_, args| match args {
            [Value::Int(a), Value::Int(b)] => a.checked_sub(*b).map(Value::Int).ok_or_else(|| {
                NativeError::new(
                    ThrowClass::ArithmeticException,
                    "integer overflow".to_owned(),
                )
            }),
            _ => Err(NativeError::arity("test/t-minus", args.len())),
        }),
    );
}

/// Single-int natives: `t-zero?`, `t-pos?`, `t-dec`.
fn install_int_fns(eval: &mut Evaluator) {
    eval.install_native(
        "user",
        "t-zero?",
        Arc::new(|_, args| int(args).map(|n| Value::Bool(n == 0))),
    );
    eval.install_native(
        "user",
        "t-pos?",
        Arc::new(|_, args| int(args).map(|n| Value::Bool(n > 0))),
    );
    eval.install_native(
        "user",
        "t-dec",
        Arc::new(|_, args| {
            int(args).and_then(|n| {
                n.checked_sub(1).map(Value::Int).ok_or_else(|| {
                    NativeError::new(
                        ThrowClass::ArithmeticException,
                        "integer overflow".to_owned(),
                    )
                })
            })
        }),
    );
}

/// `t-id` plus the side-logging `t-log`.
fn install_misc(eval: &mut Evaluator, log: &Arc<Mutex<Vec<String>>>) {
    eval.install_native(
        "user",
        "t-id",
        Arc::new(|_, args| match args {
            [v] => Ok(v.clone()),
            _ => Err(NativeError::arity("test/t-id", args.len())),
        }),
    );
    let logged = Arc::clone(log);
    eval.install_native(
        "user",
        "t-log",
        Arc::new(move |_, args| match args {
            [] => Err(NativeError::arity("test/t-log", 0)),
            [first, ..] => {
                let mut log = logged.lock().unwrap();
                log.push(pr(first));
                Ok(first.clone())
            }
        }),
    );
    eval.install_native(
        "user",
        "t-ex",
        Arc::new(|_, args| match args {
            [Value::Str(m)] => Ok(Value::Throwable(Throwable {
                id: 0,
                class: ThrowClass::ExceptionInfo,
                message: Some(Arc::clone(m)),
                data: None,
                cause: None,
            })),
            _ => Err(NativeError::arity("test/t-ex", args.len())),
        }),
    );
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap_or_else(|e| panic!("{src}: {e}"))
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

fn uncaught_with(setup: &str, src: &str) -> String {
    let (mut eval, _) = rigged();
    if !setup.is_empty() {
        eval.eval_source(setup).unwrap();
    }
    match eval.eval_source(src) {
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
            panic!("expected {src} to throw")
        }
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(e) => panic!("expected uncaught throwable for {src}: {e}"),
    }
}

fn uncaught(src: &str) -> String {
    uncaught_with("", src)
}

#[test]
fn doubles_dont_panic() {
    // 0-arg `t-log` is an arity error, not an index panic.
    assert_eq!(
        uncaught("(t-log)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: test/t-log"
    );
    // Overflowing arithmetic is `ArithmeticException`, not a debug panic.
    assert_eq!(
        uncaught("(t-plus 9223372036854775807 1)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught("(t-minus -9223372036854775808 1)"),
        "java.lang.ArithmeticException | integer overflow"
    );
}

#[test]
fn core_forms() {
    let (mut eval, _) = rigged();
    assert_eq!(run_last(&mut eval, "(if 1 2 3)"), "2");
    assert_eq!(run_last(&mut eval, "(if nil 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "(if false 2)"), "nil");
    // Real specials ignore bindings; the five macros defer to them
    // (probe31 `shadow-*`).
    assert_eq!(run_last(&mut eval, "(let [if 5] (if 1 2))"), "2");
    assert_eq!(run_last(&mut eval, "(let [loop t-plus] (loop 1 2))"), "3");
    assert_eq!(run_last(&mut eval, "(do)"), "nil");
    assert_eq!(run_last(&mut eval, "(do 1 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "()"), "()");
    assert_eq!(run_last(&mut eval, "(quote (1 2))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(let [x 1 y 2] (t-plus x y))"), "3");
    assert_eq!(run_last(&mut eval, "(let [[a b] [1 2]] (t-plus a b))"), "3");
}

#[test]
fn closures_and_scope() {
    let (mut eval, _) = rigged();
    assert_eq!(
        run_last(&mut eval, "((let [x 10] (fn [y] (t-plus x y))) 5)"),
        "15"
    );
    assert_eq!(run_last(&mut eval, "(let [x 1] ((fn [x] x) 2))"), "2");
    assert_eq!(
        run_last(
            &mut eval,
            "((fn foo [x] (if (t-zero? x) 7 (foo (t-dec x)))) 3)"
        ),
        "7"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(letfn [(f [x] (if (t-zero? x) 0 (t-plus x (f (t-dec x)))))] (f 4))"
        ),
        "10"
    );
    assert_eq!(
        run_last(&mut eval, "((fn ([x] :one) ([x y] :two)) 1)"),
        ":one"
    );
    assert_eq!(
        run_last(&mut eval, "((fn ([x] :one) ([x y] :two)) 1 2)"),
        ":two"
    );
    // `&` is a valid function name (mangled on the JVM, probe33
    // `fn-amp-*`/`letfn-amp`).
    assert_eq!(run_last(&mut eval, "((fn & [x] x) 7)"), "7");
    assert_eq!(
        run_last(
            &mut eval,
            "((fn & [a] (if (t-zero? a) :d (& (t-dec a)))) 2)"
        ),
        ":d"
    );
    assert_eq!(run_last(&mut eval, "(letfn [(& [x] x)] (& 5))"), "5");
    assert_eq!(
        uncaught("((fn & [x] x) 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: user/eval3/&--2"
    );
}

#[test]
fn loop_recur() {
    let (mut eval, _) = rigged();
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [x 3 acc 0] (if (t-zero? x) acc (recur (t-dec x) (t-plus acc x))))"
        ),
        "6"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((fn [x] (if (t-zero? x) :done (recur (t-dec x)))) 100000)"
        ),
        ":done"
    );
    // A `fn` boundary resets the try/recur fence (probe30): the
    // fence error fires at analysis, so these values pin the reset.
    assert_eq!(
        run_last(
            &mut eval,
            "(try ((fn [n] (if n 1 (recur 2))) 0) (catch Exception e 2))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try ((fn [n] {:pre [(t-pos? (t-plus n 1))]} (if (t-zero? n) :d (recur (t-dec n)))) 2) (catch Exception e :c))"
        ),
        ":d"
    );
    // A `loop` body resets the fence too (probe30 `fence-loop-try`).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (loop [n 5] (if n 7 (recur 1))) (catch Exception e 2))"
        ),
        "7"
    );
    // Nested `fn`/`loop` inside a clause re-establish a target: the
    // clauses analyze clean (probe30 `fence-fn-in-catch`,
    // `fence-loop-in-catch`; the bodies never run).
    assert_eq!(
        run_last(
            &mut eval,
            "(try 1 (catch Exception e ((fn [x] (recur x)) 1)))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try 1 (catch Exception e (loop [a 1] (recur a))))"
        ),
        "1"
    );
    // `loop` bodies are self-tail: `recur` works in non-tail loops.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (loop [a 3] (if (t-zero? a) :d (recur (t-dec a)))) 2)"
        ),
        "2"
    );
    // `recur` rebinds through patterns (with `:or`).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [[a b] [1 2]] (if (t-zero? a) b (recur [(t-dec a) (t-plus a b)])))"
        ),
        "3"
    );
    // Copy-on-recur: a closure from iteration N keeps observing
    // iteration N's frame after the rebind (oracle: 2).
    assert_eq!(
        run_last(
            &mut eval,
            "(loop [x 2] (if (t-pos? (t-minus x 1)) (do (def g2 (fn [] x)) (recur 0)) (g2)))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((fn [x] (if (t-pos? (t-minus x 1)) (do (def g3 (fn [] x)) (recur 0)) (g3))) 2)"
        ),
        "2"
    );
}

#[test]
fn variadics() {
    let (mut eval, _) = rigged();
    assert_eq!(
        run_last(&mut eval, "((fn [a & r] [a r]) 1 2 3)"),
        "[1 (2 3)]"
    );
    assert_eq!(run_last(&mut eval, "((fn [a & r] [a r]) 1)"), "[1 nil]");
    assert_eq!(run_last(&mut eval, "((fn [& [a b]] [a b]) 1 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "((fn [{:keys [k]}] k) {:k 9})"), "9");
    assert_eq!(
        run_last(&mut eval, "((fn [[a b]] (t-plus a b)) [1 2])"),
        "3"
    );
}

#[test]
fn prepost() {
    let (mut eval, _) = rigged();
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:pre [(t-pos? x)]} (t-plus x 1)) 1)"),
        "2"
    );
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:post [(t-pos? x)]} x) 5)"),
        "5"
    );
    assert_eq!(
        uncaught("((fn [x] {:pre [(t-pos? x)]} x) -1)"),
        "java.lang.AssertionError | Assert failed: (t-pos? x)"
    );
    assert_eq!(
        uncaught("((fn [x] {:post [(t-pos? x)]} x) -1)"),
        "java.lang.AssertionError | Assert failed: (t-pos? x)"
    );
    // A lone trailing map is the body, not conditions (probe31).
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:pre [false]}) -5)"),
        "{:pre [false]}"
    );
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:post [false]}) -5)"),
        "{:post [false]}"
    );
    assert_eq!(
        run_last(&mut eval, "((fn ([x] {:pre [false]}) ([x y] x)) -5)"),
        "{:pre [false]}"
    );
    // Map-valued pre/post seq to whole entry vectors, never
    // flattened to k/v conds (probe33 `pre-map-*`).
    assert_eq!(run_last(&mut eval, "((fn [x] {:pre {nil 1}} x) 1)"), "1");
    assert_eq!(run_last(&mut eval, "((fn [x] {:post {nil 1}} x) 5)"), "5");
    assert_eq!(run_last(&mut eval, "((fn [x] {:pre [{nil 1}]} x) 1)"), "1");
    // Entries evaluate in map order, key then value (probe33
    // `pre-map-side`, `pre-map-order`, `post-map-side`).
    let (mut eval, log) = rigged();
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:pre {(t-log :k) (t-log :v)}} x) 1)"),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((fn [x] {:pre {(t-log :a) 1 (t-log :b) 2}} x) 1)"
        ),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:post {(t-log :p) 1}} x) 5)"),
        "5"
    );
    assert_eq!(*log.lock().unwrap(), vec![":k", ":v", ":a", ":b", ":p"]);
}

#[test]
fn def_var_set() {
    let (mut eval, _) = rigged();
    assert_eq!(run_last(&mut eval, "(def dv913 42)"), "#'user/dv913");
    assert_eq!(run_last(&mut eval, "dv913"), "42");
    assert_eq!(
        run_last(
            &mut eval,
            "(def ^:dynamic dyn913 1) (binding [dyn913 1] (set! dyn913 2) dyn913)"
        ),
        "2"
    );
    assert_eq!(
        uncaught_with("(def dv913 42)", "(set! dv913 9)"),
        "java.lang.IllegalStateException | Can't change/establish root binding of: dv913 with set"
    );
    assert_eq!(
        uncaught("(do (def ^:dynamic dd913 1) (set! dd913 2))"),
        "java.lang.IllegalStateException | Can't change/establish root binding of: dd913 with set"
    );
    assert_eq!(
        uncaught_with("(def dv913 42)", "(binding [dv913 2] dv913)"),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: user/dv913"
    );
    // `def` meta: auto keys plus user meta.
    run(&mut eval, "(def ^:private pm913 \"d\" 1)");
    let id = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("user"), "pm913"))
        .unwrap();
    let meta = eval.registry.state(id).unwrap().meta.clone().unwrap();
    assert_eq!(
        pr(&Value::Map(meta)),
        "{:private true, :doc \"d\", :line 1, :column 1, :file \"test\", :name pm913, :ns #object[clojure.lang.Namespace 0x1 \"user\"]}"
    );
    assert!(!eval.registry.state(id).unwrap().dynamic);
    // Unbound vars: the sentinel value, `str` shape, numeric CCE.
    assert_eq!(run_last(&mut eval, "(def ub913)"), "#'user/ub913");
    let ub_id = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("user"), "ub913"))
        .unwrap();
    assert_eq!(
        run_last(&mut eval, "ub913"),
        format!("#object[clojure.lang.Var$Unbound 0x{ub_id:x} \"Unbound: #'user/ub913\"]")
    );
    assert_eq!(
        uncaught("(do (def ub914) (t-plus ub914 1))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Var$Unbound to java.lang.Number"
    );
}

#[test]
fn destructure_values() {
    let (mut eval, _) = rigged();
    for (src, want) in [
        ("(let [[a b] [1]] [a b])", "[1 nil]"),
        ("(let [[a & r] [1 2 3]] [a r])", "[1 (2 3)]"),
        ("(let [[& r] [1 2]] r)", "(1 2)"),
        ("(let [[& r] \"xy\"] r)", "(\\x \\y)"),
        ("(let [[& r] nil] r)", "nil"),
        ("(let [[a] 5] a)", "UnsupportedOperationException"),
        ("(let [{x :x :or {x 5}} {:x nil}] x)", "nil"),
        ("(let [{x :x :or {x 5}} {}] x)", "5"),
        ("(let [{x :x :or {:x 7}} {}] x)", "7"),
        ("(let [{:keys [a] :or {a 1}} {}] a)", "1"),
        (
            "(let [{:keys [a b] :as m} {:a 1 :b 2}] [a b m])",
            "[1 2 {:a 1, :b 2}]",
        ),
        ("(let [{:strs [s]} {\"s\" 1}] s)", "1"),
        ("(let [{:foo/keys [a]} {:foo/a 1}] a)", "1"),
        ("(let [{:foo/syms [a]} {'foo/a 1}] a)", "1"),
        ("(let [{:foo/keys! [a]} {:foo/a 1}] a)", "1"),
        ("(let [{:foo/syms! [a]} {'foo/a 1}] a)", "1"),
        ("((fn [{:foo/keys [a]}] a) {:foo/a 1})", "1"),
        ("(loop [{:foo/keys [a]} {:foo/a 1}] a)", "1"),
        ("(let [{:syms [s]} {'s 1}] s)", "1"),
        ("(let [{:keys [:b]} {:b 1}] b)", "1"),
        // Keywords ride the key-list fast path on every form
        // (probe32 `keys-kw` evaluates).
        ("(loop [{:keys [:a]} {:a 1}] a)", "1"),
        ("(let [[a & [b c]] [1 2 3]] [a b c])", "[1 2 3]"),
        ("(let [[a b] \"xy\"] [a b])", "[\\x \\y]"),
        ("(let [{:keys! [a]} {:a 1}] a)", "1"),
        (
            "(let [{:keys [a b] :select s} {:a 1 :b 2 :c 3}] s)",
            "{:a 1, :b 2}",
        ),
        (
            "(let [{:keys [a] :all m :or {a 9}} {:b 2}] m)",
            "{:a 9, :b 2}",
        ),
        ("(let [{:keys [a] :excess e} {:a 1 :b 2}] e)", "{:b 2}"),
        (
            "(let [{:keys! [a b] :missing m} {:a 1}] [a b m])",
            "[1 nil {:b nil}]",
        ),
        (
            "(let [{:keys [a & :c] :select s} {:a 1 :c 3 :d 4}] s)",
            "{:a 1, :c 3}",
        ),
        // M2 `:req!` (probe `req-*`, upstream 9e51d6dc).
        ("(let [{:req! {a :long-a}} {:long-a 1, :b 2}] a)", "1"),
        (
            "(let [{:keys! [b], :keys [d], :req! {a :long-a}} {:long-a 1, :b 2}] [a b d])",
            "[1 2 nil]",
        ),
        (
            "(let [{:keys! [b], :keys [d], :req! {a :long-a}, :or {d 42}} {:long-a 1, :b 2}] [a b d])",
            "[1 2 42]",
        ),
        (
            "(let [{:req! {{:keys! [c]} :nested}} {:nested {:c 3}}] c)",
            "3",
        ),
        (
            "(let [{:req! {a :long-a}, :missing miss} {}] miss)",
            "{:long-a nil}",
        ),
        ("(loop [{:req! {a :long-a}} {:long-a 1}] a)", "1"),
        // M2 `:req!` through the analyzer engine (`fn` params,
        // probes `fn-req`, `fn-req-nested`, `fn-req-missingsel`).
        ("((fn [{:req! {a :long-a}}] a) {:long-a 1})", "1"),
        (
            "((fn [{:req! {{:keys [c]} :nested}}] c) {:nested {:c 3}})",
            "3",
        ),
        (
            "((fn [{:req! {a :long-a}, :missing miss}] miss) {})",
            "{:long-a nil}",
        ),
        // M2 post-`&` qualification through `fn` (analyzer engine).
        (
            "((fn [{:foo/keys [aa & :bb], :select sel}] sel) {:foo/aa 1, :bb 2, :foo/bb 3})",
            "#:foo{:aa 1, :bb 3}",
        ),
        // M2 post-`&` qualification (probe `ns-postamp*`, 4610bb46).
        (
            "(let [{:keys! [aa & :bb], :select sel} {:aa 1, :bb 2}] sel)",
            "{:aa 1, :bb 2}",
        ),
        (
            "(let [{:foo/keys! [aa & :bb], :select sel} {:foo/aa 1, :bb 2, :foo/bb 3}] sel)",
            "#:foo{:aa 1, :bb 3}",
        ),
        // Qualified post-`&` keys keep their own namespace
        // (probes `post-qual`, `post-qual-req`; selection order is
        // source order here, hash order on the JVM).
        (
            "(let [{:foo/keys [a & :other/b], :select s} {:foo/a 1, :other/b 2}] s)",
            "{:foo/a 1, :other/b 2}",
        ),
        (
            "((fn [{:foo/keys [a & :other/b], :select s}] s) {:foo/a 1, :other/b 2})",
            "{:foo/a 1, :other/b 2}",
        ),
        ("(let [{a :a} '(:a 1)] a)", "1"),
        ("(let [{a :a} '(:a 1 {:a 9 :b 8})] [a])", "[9]"),
        (
            "(let [{:keys [a b] :as m} '(:a 1 {:b 2})] m)",
            "{:a 1, :b 2}",
        ),
        ("(let [[a & r] [1]] [a r])", "[1 nil]"),
    ] {
        if want == "UnsupportedOperationException" {
            assert_eq!(
                uncaught(src),
                "java.lang.UnsupportedOperationException | nth not supported on this type: Long",
                "{src}"
            );
        } else {
            assert_eq!(run_last(&mut eval, src), want, "{src}");
        }
    }
    // `:or` defaults evaluate eagerly, even when unused.
    let (mut eval, log) = rigged();
    assert_eq!(
        run_last(&mut eval, "(let [{a :x :or {a (t-log 7)}} {:x 5}] a)"),
        "5"
    );
    assert_eq!(*log.lock().unwrap(), vec!["7".to_owned()]);
    // Nested `:or` defaults fire exactly once under outlet forcing
    // (probe28): the nested pattern binds once, projecting `subs`.
    for (src, want) in [
        (
            "(let [{{:keys [b] :or {b (t-log 7)}} :m :select s} {:m {}}] [s])",
            "[{:m {:b 7}}]",
        ),
        (
            "(let [{{:keys [b] :or {b (t-log 7)}} :m :all s} {:m {}}] [s])",
            "[{:m {:b 7}}]",
        ),
        (
            "(let [{{:keys [b] :or {b (t-log 7)}} :m :excess s} {:m {} :z 9}] [s])",
            "[{:z 9}]",
        ),
        (
            "(let [{{:keys [b] :select ns :or {b (t-log 7)}} :m :select s} {:m {}}] [ns s])",
            "[{:b 7} {:m {:b 7}}]",
        ),
        (
            "(let [{{:keys [b] :or {b (t-log 7)}} :m :select s} {:m {:b 1}}] [s])",
            "[{:m {:b 1}}]",
        ),
    ] {
        let (mut eval, log) = rigged();
        assert_eq!(run_last(&mut eval, src), want, "{src}");
        assert_eq!(*log.lock().unwrap(), vec!["7".to_owned()], "{src}");
    }
    // Required keys throw when missing.
    assert_eq!(
        uncaught("(let [{:keys! [a]} {}] a)"),
        "java.lang.IllegalArgumentException | Missing required key: :a"
    );
    // M2 `:req!` missing keys throw at runtime (probe `req-missing`).
    // Expansion-time `:req!` errors live in `fx4_macroexpand`.
    assert_eq!(
        uncaught("(let [{:req! {a :long-a}} {}] a)"),
        "java.lang.IllegalArgumentException | Missing required key: :long-a"
    );
    assert_eq!(
        uncaught("((fn [{:req! {a :long-a}}] a) {})"),
        "java.lang.IllegalArgumentException | Missing required key: :long-a"
    );
    assert_eq!(
        uncaught("(let [{a :a} '(:a 1 :b)] a)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    // `&` seqs its target first (`destvec*` `gseq`), so
    // non-seqables raise the `ISeq` error, not `nth`'s.
    assert_eq!(
        uncaught("(let [[& r] 5] r)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn try_catch_finally() {
    let (mut eval, log) = rigged();
    assert_eq!(run_last(&mut eval, "(try 1 2 3)"), "3");
    assert_eq!(run_last(&mut eval, "(try 1 (catch Exception e :c))"), "1");
    assert_eq!(run_last(&mut eval, "(try 1 (finally :f))"), "1");
    assert_eq!(
        run_last(&mut eval, "(try 1 (catch Exception e) (finally))"),
        "1"
    );
    // Thrown values propagate with their message; catches match by hierarchy.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (throw 42) (catch clojure.lang.ArityException e :arity) (catch Exception e :ex))"
        ),
        ":ex"
    );
    assert_eq!(
        uncaught("(try (throw 42) (catch clojure.lang.ArityException e :arity))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.Throwable"
    );
    // `finally` runs on every path; its value is discarded.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (do (t-log :body) 1) (catch Exception e :c) (finally (t-log :fin)))"
        ),
        "1"
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec![":body".to_owned(), ":fin".to_owned()]
    );
    log.lock().unwrap().clear();
    assert_eq!(
        run_last(
            &mut eval,
            "(try (try (do (t-log :body) (throw 42)) (finally (t-log :fin))) (catch Exception e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        *log.lock().unwrap(),
        vec![":body".to_owned(), ":fin".to_owned()]
    );
    // A throwing `finally` masks the in-flight exception (probe29),
    // on the body path and the handler path alike.
    assert_eq!(
        uncaught("(try (throw (t-ex \"A\")) (finally (throw (t-ex \"F\"))))"),
        "clojure.lang.ExceptionInfo | F"
    );
    assert_eq!(
        uncaught(
            "(try (throw (t-ex \"A\")) (catch Exception e (throw (t-ex \"H\"))) (finally (throw (t-ex \"F\"))))"
        ),
        "clojure.lang.ExceptionInfo | F"
    );
    // `AssertionError` is not an `Exception`, but `Throwable` catches all.
    assert_eq!(
        uncaught("(try ((fn [x] {:pre [(t-pos? x)]} x) -1) (catch Exception e :ex))"),
        "java.lang.AssertionError | Assert failed: (t-pos? x)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try ((fn [x] {:pre [(t-pos? x)]} x) -1) (catch Throwable t :t))"
        ),
        ":t"
    );
    // Unthrowable catch classes resolve and never match (probe25
    // `catch-string`; the JVM VerifyErrors instead — E6 divergence).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (throw (t-ex \"e\")) (catch String s :s) (catch Exception e :e))"
        ),
        ":e"
    );
    // Widened `catch` classes resolve (probe32 `catch-*`).
    assert_eq!(
        run_last(&mut eval, "(try 1 (catch java.io.IOException e 1))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(try 1 (catch InterruptedException e 1))"),
        "1"
    );
    // `binding` unwinds through throws.
    assert_eq!(
        run_last(
            &mut eval,
            "(def ^:dynamic bw913 1) (try (binding [bw913 2] (throw 42)) (catch Exception e bw913))"
        ),
        "1"
    );
    // Every init evaluates before any var validates (probe29):
    // an init throw beats the dynamic-ness error either way round.
    assert_eq!(
        uncaught_with(
            "(def ^:dynamic bd913 1) (def bn913 1)",
            "(binding [bd913 1 bn913 (throw (t-ex \"init\"))] :body)"
        ),
        "clojure.lang.ExceptionInfo | init"
    );
    assert_eq!(
        uncaught_with(
            "(def ^:dynamic bd913 1) (def bn913 1)",
            "(binding [bn913 1 bd913 (throw (t-ex \"init3\"))] :body)"
        ),
        "clojure.lang.ExceptionInfo | init3"
    );
}

#[test]
fn star_form_values() {
    // Direct `let*`/`loop*`/`fn*` evaluate with raw semantics:
    // sequential symbol bindings, no destructuring, no spec,
    // no pre/post (probe33).
    let (mut eval, _) = rigged();
    for (src, want) in [
        ("(let* [a 1] a)", "1"),
        ("(loop* [a 1] a)", "1"),
        ("(let* [a 1 b a] b)", "1"),
        ("(let* [a 1 a 2] a)", "2"),
        ("(let* [] 1)", "1"),
        ("(loop* [] 1)", "1"),
        (
            "(loop* [a 3] (if (t-zero? a) :done (recur (t-dec a))))",
            ":done",
        ),
        ("((fn* [a] a) 5)", "5"),
        ("((fn* [a & r] r) 1 2 3)", "(2 3)"),
        ("((fn* [& r] r) 1 2)", "(1 2)"),
        ("((fn* [a &] a) 1)", "1"),
        ("((fn* [x] {:pre [(t-pos? x)]} x) -1)", "-1"),
        (
            "((fn* foo322 [a] (if (t-zero? a) :d (foo322 (t-dec a)))) 3)",
            ":d",
        ),
        (
            "((fn* u/foo328 [a] (if (t-zero? a) :d (foo328 (t-dec a)))) 2)",
            ":d",
        ),
        ("(let [let* 5] (let* [a 1] a))", "1"),
        ("((fn* ([a] a) ([a b] b)) 1 2)", "2"),
        ("((fn* [a]) 1)", "nil"),
        (
            "((fn* [a] (if (t-zero? a) :done (recur (t-dec a)))) 3)",
            ":done",
        ),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // Arity failures name the display (`user/eval3/fn--2`: fresh
    // evaluator, setup fn first, probe33 `fn*-invoke-amp` and
    // `fn*-invoke0`).
    for (src, want) in [
        (
            "((fn* [a &] a) 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: user/eval3/fn--2",
        ),
        (
            "((fn*) 1)",
            "clojure.lang.ArityException | Wrong number of args (1) passed to: user/eval3/fn--2",
        ),
        (
            "((fn* [a] a))",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: user/eval3/fn--2",
        ),
    ] {
        assert_eq!(uncaught(src), want, "{src}");
    }
}

#[test]
fn invoke_table() {
    let (mut eval, _) = rigged();
    for (src, want) in [
        ("(:a {:a 1})", "1"),
        ("(:b {:a 1})", "nil"),
        ("(:b {:a 1} :nf)", ":nf"),
        ("(:a 5)", "nil"),
        ("(:a nil)", "nil"),
        ("('s {'s 1})", "1"),
        ("('s {} :nf)", ":nf"),
        ("([1 2] 1)", "2"),
        ("(#{1} 1)", "1"),
        ("(#{1} 5)", "nil"),
        ("(#{1} 5 :nf)", ":nf"),
        ("({:a 1} :b)", "nil"),
        ("((var vfn251) 5)", "5"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    for (src, want) in [
        (
            "(:k)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: :k",
        ),
        (
            "(:k :a :b :c)",
            "clojure.lang.ArityException | Wrong number of args (3) passed to: :k",
        ),
        (
            "((fn [x] x) 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: user/eval3/fn--2",
        ),
        (
            "((fn foo [x] x) 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: user/eval3/foo--2",
        ),
        (
            "((fn [x & r] x))",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: user/eval3/fn--2",
        ),
        ("([1] 5)", "java.lang.IndexOutOfBoundsException | "),
        (
            "([1 2] \"a\")",
            "java.lang.IllegalArgumentException | Key must be integer",
        ),
        (
            "(([1]))",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.lang.PersistentVector",
        ),
        (
            "([1] 1 2 3)",
            "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.lang.PersistentVector",
        ),
        (
            "({:a 1} :a :b :c)",
            "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.lang.PersistentArrayMap",
        ),
        (
            "((var vu251) 5)",
            "java.lang.IllegalStateException | Attempting to call unbound fn: #'user/vu251",
        ),
        (
            "(vu251 5)",
            "java.lang.IllegalStateException | Attempting to call unbound fn: #'user/vu251",
        ),
        (
            "(1 2)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn",
        ),
        (
            "((quote (1 2)) 3)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentList to clojure.lang.IFn",
        ),
        (
            "((quote ()) 1)",
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentList$EmptyList to clojure.lang.IFn",
        ),
        (
            "(let [f nil] (f 1))",
            "java.lang.NullPointerException | Cannot invoke nil as a function",
        ),
        (
            "(throw 42)",
            "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.Throwable",
        ),
        (
            "(throw (fn [x] x))",
            "java.lang.ClassCastException | Cannot cast user$eval3$fn__2 to java.lang.Throwable",
        ),
        (
            "(throw t-plus)",
            "java.lang.ClassCastException | Cannot cast user$t_plus to java.lang.Throwable",
        ),
        (
            "(throw nil)",
            "java.lang.NullPointerException | Cannot throw exception because \"null\" is null",
        ),
    ] {
        assert_eq!(uncaught(src), want, "{src}");
    }
    // Past 20, counts render as `> 20`.
    let args21 = (0..21).map(|n| n.to_string()).collect::<Vec<_>>().join(" ");
    assert_eq!(
        uncaught(&format!("((fn [x] x) {args21})")),
        "clojure.lang.ArityException | Wrong number of args (> 20) passed to: user/eval3/fn--2"
    );
}

#[test]
fn object_prints() {
    // Fresh-evaluator ids are deterministic.
    let mut eval = Evaluator::new("test");
    assert_eq!(
        run_last(&mut eval, "(fn [x] x)"),
        "#object[user$eval1$fn__1 0x1 \"user$eval1$fn__1@1\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(fn foo [x] x)"),
        "#object[user$eval2$foo__2 0x2 \"user$eval2$foo__2@2\"]"
    );
}

#[test]
fn native_prints() {
    let (mut eval, _) = rigged();
    // Natives print `#object[…]` with the munged display and id, like
    // closures.
    let pr = run_last(&mut eval, "t-plus");
    assert!(pr.starts_with("#object[user$t_plus 0x"), "{pr}");
    assert!(pr.contains("user$t_plus@"), "{pr}");
    assert!(pr.ends_with("\"]"), "{pr}");
}

#[test]
fn top_do_splits_each_element() {
    let mut eval = Evaluator::new("split");
    eval.install_native(
        "user",
        "t-mkvar",
        Arc::new(|e, _| {
            let sym = Symbol::new(Some("user"), "t-made");
            let id = e.registry.intern(&sym);
            e.registry.with_state_mut(id, |state| {
                state.root = Some(Value::Int(5));
            });
            Ok(Value::Nil)
        }),
    );
    // Without the split, `t-made` would analyze before `t-mkvar`
    // evaluates (`Unable to resolve symbol`).
    let out = eval.eval_source("(do (t-mkvar) t-made)").unwrap();
    assert_eq!(out, vec![Value::Int(5)]);
    // A nested `do` stays one analysis unit: inside a `fn` body the
    // reference analyzes before anything evaluates.
    let mut nested = Evaluator::new("split-nested");
    nested.install_native(
        "user",
        "t-mkvar",
        Arc::new(|e, _| {
            let sym = Symbol::new(Some("user"), "t-made");
            let id = e.registry.intern(&sym);
            e.registry.with_state_mut(id, |state| {
                state.root = Some(Value::Int(5));
            });
            Ok(Value::Nil)
        }),
    );
    let err = nested
        .eval_source("(fn [] (do (t-mkvar) t-made))")
        .unwrap_err();
    match err {
        SourceError::Compile(e) => assert!(
            e.message.contains("Unable to resolve symbol: t-made"),
            "unexpected message: {}",
            e.message
        ),
        other => panic!("expected a compile error, got {other}"),
    }
}

#[test]
fn map_destructure_odd_trailing() {
    let (mut eval, _) = rigged();
    // 2-vector trailers assoc.
    assert_eq!(
        run_last(&mut eval, "(let [{a :a, b :b} '(:a 1 [:b 2])] [a b])"),
        "[1 2]"
    );
    // Non-pair vector trailers are the pair IAE.
    assert_eq!(
        uncaught("(let [{a :a} '(:a 1 [:b])] a)"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    // `nil` trailers vanish.
    assert_eq!(run_last(&mut eval, "(let [{a :a} '(:a 1 nil)] a)"), "1");
    // Empty seqable trailers vanish.
    assert_eq!(run_last(&mut eval, "(let [{a :a} '(:a 1 ())] a)"), "1");
    // Non-empty seqable trailers cast the first element to an
    // entry (short cast format like the other eval CCE pins).
    assert_eq!(
        uncaught("(let [{a :a} '(:a 1 (2))] a)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.util.Map$Entry"
    );
    assert_eq!(
        uncaught("(let [{a :a} '(:a 1 #{2})] a)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.util.Map$Entry"
    );
    assert_eq!(
        uncaught("(let [{a :a} '(:a 1 \"xy\")] a)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Character to java.util.Map$Entry"
    );
}

#[test]
fn map_destructure_single_seq_is_element() {
    let (mut eval, _) = rigged();
    // `(5)` converts to `5`; the key lookup misses.
    assert_eq!(run_last(&mut eval, "(let [{:keys [a]} '(5)] a)"), "nil");
}

#[test]
fn map_destructure_select_non_map() {
    let (mut eval, _) = rigged();
    // Non-map merge results select by lookup, absent keys skipped.
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :select s} [1 2]] s)"),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (quote 5)] (let [{:keys [a] :select s} n] s))"
        ),
        "{}"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a], :select s} #{:a}] s)"),
        "{:a :a}"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{a 0, :select s} [10 20]] [a s])"),
        "[10 {0 10}]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{c 0, :select s} \"xy\"] [c s])"),
        "[\\x {0 \\x}]"
    );
}

#[test]
fn outlet_merge_conj_each() {
    let (mut eval, _) = rigged();
    // Long elems are the ISeq error.
    assert_eq!(
        uncaught("(let [{x :x, :or {x 1}, :select s} '((1 2))] s)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // Pair-vector elems assoc, then `:select` filters; `:all`
    // shows the full merge.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :select s} '(([:k :v]))] s)"
        ),
        "{:x 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :all a} '(([:k :v]))] a)"
        ),
        "{:x 1, :k :v}"
    );
    // `nil` elems skip.
    assert_eq!(
        run_last(&mut eval, "(let [{x :x, :or {x 1}, :select s} '((nil))] s)"),
        "{:x 1}"
    );
    // Empty-list elems contribute nothing.
    assert_eq!(
        run_last(&mut eval, "(let [{x :x, :or {x 1}, :select s} '((()))] s)"),
        "{:x 1}"
    );
    // A seq elem's `nil` first is the entry-NPE, verbatim.
    assert_eq!(
        uncaught("(let [{x :x, :or {x 1}, :select s} '(((nil)))] s)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Map$Entry.getKey()\" because \"e\" is null"
    );
    // A seq elem's non-entry first is the entry CCE (short cast
    // format).
    assert_eq!(
        uncaught("(let [{x :x, :or {x 1}, :select s} '(((2)))] s)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.util.Map$Entry"
    );
    // Map elems merge.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :select s} '(({:k :v}))] s)"
        ),
        "{:x 1}"
    );
    // Non-pair vector elems are the pair IAE.
    assert_eq!(
        uncaught("(let [{x :x, :or {x 1}, :select s} '(([:b]))] s)"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    // The `:all` merge errors identically.
    assert_eq!(
        uncaught("(let [{x :x, :or {x 1}, :all a} '((1 2))] a)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn outlet_merge_conj_entry_non_map() {
    let (mut eval, _) = rigged();
    // Entry conjed onto a vector appends the pair.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [a] :or {a 1}} :m, :all all} [1 2]] all)"
        ),
        "[1 2 [:m {:a 1}]]"
    );
    // ... onto a list prepends.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [a] :or {a 1}} :m, :all all} '((1 2))] all)"
        ),
        "([:m {:a 1}] 1 2)"
    );
    // ... onto a set inserts (port sets seq in insertion order,
    // so the pr is deterministic).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [a] :or {a 1}} :m, :all all} #{1}] all)"
        ),
        "#{1 [:m {:a 1}]}"
    );
    // ... onto a scalar is the collection CCE (short cast format).
    assert_eq!(
        uncaught("(let [{{:keys [a] :or {a 1}} :m, :all all} 5] all)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IPersistentCollection"
    );
    assert_eq!(
        uncaught("(let [{{:keys [a] :or {a 1}} :m, :all all} \"xy\"] all)"),
        "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.IPersistentCollection"
    );
}

#[test]
fn outlet_merge_empty_seq_from() {
    let (mut eval, _) = rigged();
    // An empty seq `from` adds nothing.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{x :x, :or {x 1}, :select s} '(())] [x s])"
        ),
        "[1 {:x 1}]"
    );
}

#[test]
fn map_destructure_excess_edges() {
    let (mut eval, _) = rigged();
    // A `nil` target has no excess base.
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :excess e} nil] [a e])"),
        "[nil nil]"
    );
    // A non-map target is the map cast (short cast format).
    assert_eq!(
        uncaught("(let [{:keys [a] :excess e} [1 2]] [a e])"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.IPersistentMap"
    );
}

#[test]
fn map_destructure_select_absent() {
    let (mut eval, _) = rigged();
    // No defaults, `nil` target, no subs ⟹ `nil` select.
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :select s} nil] [a s])"),
        "[nil nil]"
    );
}

#[test]
fn map_destructure_outlet_forcing_edges() {
    let (mut eval, _) = rigged();
    // A symbol local under an effective outlet forces nothing.
    assert_eq!(
        run_last(&mut eval, "(let [{a :a, :select s} {:a 1}] [a s])"),
        "[1 {:a 1}]"
    );
    // A nested map requesting `:missing` under an effective
    // `:missing` outlet.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [b], :missing nm} :m, :missing m} {:m {}}] [m nm])"
        ),
        "[nil nil]"
    );
}

#[test]
fn lookup_collection_edges() {
    let (mut eval, _) = rigged();
    // Int keys index vectors/strings in map patterns.
    assert_eq!(run_last(&mut eval, "(let [{a 0} [10 20]] a)"), "10");
    assert_eq!(run_last(&mut eval, "(let [{c 0} \"xy\"] c)"), "\\x");
    // Non-int keys miss on vectors/strings.
    assert_eq!(run_last(&mut eval, "(:a [1 2])"), "nil");
    assert_eq!(run_last(&mut eval, "(:k \"xy\")"), "nil");
    // Keywords read sets by membership.
    assert_eq!(run_last(&mut eval, "(:a #{:a :b})"), ":a");
    assert_eq!(run_last(&mut eval, "('a '#{a b})"), "a");
}

#[test]
fn seq_destructure_rest_shapes() {
    let (mut eval, _) = rigged();
    // `&` over lists, empty and not.
    assert_eq!(
        run_last(&mut eval, "(let [[a & r] '(1 2)] [a r])"),
        "[1 (2)]"
    );
    assert_eq!(
        run_last(&mut eval, "(let [[a & r] '()] [a r])"),
        "[nil nil]"
    );
    // `&` over a set / map.
    assert_eq!(run_last(&mut eval, "(let [[a & r] #{1}] [a r])"), "[1 nil]");
    assert_eq!(
        run_last(&mut eval, "(let [[[k v] & r] {:a 1}] [k v r])"),
        "[:a 1 nil]"
    );
    // `&` over empties ⟹ `nil` rest.
    assert_eq!(run_last(&mut eval, "(let [[a & r] []] [a r])"), "[nil nil]");
    assert_eq!(
        run_last(&mut eval, "(let [[a & r] #{}] [a r])"),
        "[nil nil]"
    );
    assert_eq!(run_last(&mut eval, "(let [[a & r] {}] [a r])"), "[nil nil]");
}

#[test]
fn seq_destructure_nil_target() {
    let (mut eval, _) = rigged();
    // Positional destructuring over `nil` binds `nil`s.
    assert_eq!(run_last(&mut eval, "(let [[a b] nil] [a b])"), "[nil nil]");
}

#[test]
fn invoke_map_two_arity() {
    let (mut eval, _) = rigged();
    // Found and not-found.
    assert_eq!(run_last(&mut eval, "({:a 1} :a :nf)"), "1");
    assert_eq!(run_last(&mut eval, "({:a 1} :b :nf)"), ":nf");
}

#[test]
fn invoke_symbol_arity() {
    assert_eq!(
        uncaught("('a 1 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.lang.Symbol"
    );
}

#[test]
fn try_handler_throw_finally_ok() {
    let (mut eval, log) = rigged();
    // The handler's error propagates when `finally` is quiet;
    // `finally` still ran.
    let err = match eval.eval_source(
        "(try (throw (t-ex \"A\")) (catch Exception e (throw (t-ex \"H\"))) (finally (t-log :fin)))",
    ) {
        Err(SourceError::Uncaught(t)) => {
            format!("{} | {}", t.class.jvm_name(), t.message.as_deref().unwrap_or(""))
        }
        Err(e) => panic!("expected uncaught throwable: {e}"),
        Ok(vs) => panic!("expected a throw, got {vs:?}"),
    };
    assert_eq!(err, "clojure.lang.ExceptionInfo | H");
    assert_eq!(*log.lock().unwrap(), vec![":fin".to_owned()]);
}

#[test]
fn source_error_display_renders_all_variants() {
    let (mut eval, _) = rigged();
    // Reader errors delegate to the inner display.
    let err = eval.read_forms("(def").unwrap_err();
    let SourceError::Reader(inner) = &err else {
        panic!("expected a reader error, got {err:?}")
    };
    assert_eq!(format!("{err}"), format!("{inner}"));
    // Compile errors delegate to the inner display.
    let forms = eval.read_forms("(def a 1 2)").unwrap();
    let inner = eval.analyze(&forms[0]).unwrap_err();
    let err = SourceError::Compile(inner);
    let SourceError::Compile(inner) = &err else {
        panic!("unreachable")
    };
    assert_eq!(format!("{err}"), format!("{inner}"));
    // Uncaught throwables render `class | message`.
    let err = eval.eval_source("(throw (t-ex \"boom\"))").unwrap_err();
    assert_eq!(format!("{err}"), "clojure.lang.ExceptionInfo | boom");
    // Internal errors render with the prefix.
    assert_eq!(
        format!("{}", SourceError::Internal("x".to_owned())),
        "internal error: x"
    );
}
