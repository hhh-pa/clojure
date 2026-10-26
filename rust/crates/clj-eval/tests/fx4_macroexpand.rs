// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! FX4: `macroexpand-1` of the nine analyzer builtins matches the
//! alpha9 shapes (gensym ids are the per-boot counter). Eval keeps
//! the macro body's own errors — there is no spec instrumentation.

use clj_eval::{Evaluator, SourceError, boot};
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

fn compile_err(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src).unwrap_err() {
        SourceError::Compile(e) => e.chain(),
        other => panic!("expected a compile error, got {other}"),
    }
}

fn fresh(src: &str) -> String {
    run_last(&mut boot(), src)
}

const CONVERT1: &str = "(if (clojure.core/instance? clojure.lang.ISeq map__1) \
(if (clojure.core/next map__1) \
(clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array map__1)) \
(if (clojure.core/seq map__1) (clojure.core/first map__1) \
clojure.lang.PersistentArrayMap/EMPTY)) map__1)";

#[test]
fn expand_nine_heads() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(let [a 1] a))"),
        "(let* [a 1] a)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(macroexpand-1 '(loop [a 1] (if (pos? a) (recur (dec a)) a)))"
        ),
        "(loop* [a 1] (if (pos? a) (recur (dec a)) a))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fn [a] a))"),
        "(fn* ([a] a))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fn ([a] a) ([a b] b)))"),
        "(fn* ([a] a) ([a b] b))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fn f ([a] a)))"),
        "(fn* f ([a] a))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fn f [a] a))"),
        "(fn* f ([a] a))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(binding [*out* *out*] 1))"),
        "(clojure.core/let [] (clojure.core/push-thread-bindings (clojure.core/hash-map (var *out*) *out*)) (try 1 (finally (clojure.core/pop-thread-bindings))))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(letfn [(f [a] a)] (f 1)))"),
        "(letfn* [f (clojure.core/fn f [a] a)] (f 1))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(with-redefs [*out* *out*] 1))"),
        "(clojure.core/with-redefs-fn {(var *out*) *out*} (clojure.core/fn [] 1))"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(macroexpand-1 '(with-bindings {(var *out*) *out*} 1))"
        ),
        "(clojure.core/with-bindings* {(var *out*) *out*} (clojure.core/fn [] 1))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(bound-fn [x] x))"),
        "(clojure.core/bound-fn* (clojure.core/fn [x] x))"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(clojure.core/let [a 1] a))"),
        "(let* [a 1] a)"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(clojure.core/fn [a] a))"),
        "(fn* ([a] a))"
    );
    // One step does not expand the inner `let`.
    let danced = fresh("(macroexpand-1 '(loop [[a b] v] a))");
    assert_eq!(
        danced,
        "(clojure.core/let [G__4 v [a b] G__4] (loop* [G__4 G__4] (clojure.core/let [[a b] G__4] a)))"
    );
    // Full expansion rewrites the outer `let` only.
    assert_eq!(
        fresh("(macroexpand '(loop [[a b] v] a))"),
        "(let* [G__4 v vec__5 G__4 a (clojure.core/nth vec__5 0 nil) b (clojure.core/nth vec__5 1 nil)] (loop* [G__4 G__4] (clojure.core/let [[a b] G__4] a)))"
    );
}

#[test]
fn expand_locking_and_destructure_gensyms() {
    assert_eq!(
        fresh("(macroexpand-1 '(locking *out* 1))"),
        "(clojure.core/let [lockee__1__auto__ *out*] (try (clojure.core/let [locklocal__2__auto__ lockee__1__auto__] (monitor-enter locklocal__2__auto__) (try 1 (finally (monitor-exit locklocal__2__auto__))))))"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(let [[a b] v] a))"),
        "(let* [vec__1 v a (clojure.core/nth vec__1 0 nil) b (clojure.core/nth vec__1 1 nil)] a)"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(let [[a & r] v] a))"),
        "(let* [vec__1 v seq__2 (clojure.core/seq vec__1) first__3 (clojure.core/first seq__2) seq__2 (clojure.core/next seq__2) a first__3 r seq__2] a)"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(let [[a :as m] v] m))"),
        "(let* [vec__1 v a (clojure.core/nth vec__1 0 nil) m vec__1] m)"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(let [{:keys [a] :or {a 1} :as m} v] a))"),
        format!(
            "(let* [default__3 1 map__1 v map__1 {CONVERT1} m map__1 a (clojure.core/get map__1 :a default__3)] a)"
        )
    );
    let keys_bang = fresh("(macroexpand-1 '(let [{:keys! [a] :missing m} v] a))");
    assert!(
        keys_bang.contains("notfound__3 (new Object)"),
        "{keys_bang}"
    );
    assert!(
        keys_bang.contains("(clojure.core/identical? temp__2 notfound__3)"),
        "{keys_bang}"
    );
    assert!(keys_bang.ends_with("m (clojure.core/merge m (clojure.core/some-vals nil))] a)"));
    assert_eq!(
        fresh("(macroexpand-1 '(fn [[a b]] a))"),
        "(fn* ([p__1] (clojure.core/let [[a b] p__1] a)))"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(fn [x] {:pre [(pos? x)] :post [(number? %)]} x))"),
        "(fn* ([x] (clojure.core/assert (pos? x)) (clojure.core/let [% x] (clojure.core/assert (number? %)) %)))"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(fn [x] {:post []} x))"),
        "(fn* ([x] (clojure.core/let [% x] %)))"
    );
}

#[test]
fn expand_meta_and_fence() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(meta (macroexpand-1 (list 'fn '[a] 'a)))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(contains? (meta (macroexpand-1 '(fn [a] a))) :line)"
        ),
        "true"
    );
    let printed = run_last(
        &mut eval,
        "(binding [*print-meta* true] (pr-str (macroexpand-1 '(let [{:keys [a]} v] a))))",
    );
    assert!(printed.contains("^clojure.lang.ISeq"), "{printed}");
    for form in [
        "(declare a)",
        "(ns foo)",
        "(defmacro m [x] x)",
        "(assert true)",
        "(time 1)",
        "(with-out-str 1)",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(macroexpand-1 '{form})")),
            form,
            "{form}"
        );
    }
}

#[test]
fn expand_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(let)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/let"
    );
    assert_eq!(
        uncaught(&mut eval, "(loop)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/loop"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/binding"
    );
    assert_eq!(
        uncaught(&mut eval, "(letfn)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/letfn"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-redefs)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-redefs"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-bindings)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-bindings"
    );
    assert_eq!(
        uncaught(&mut eval, "(locking)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/locking"
    );
    assert_eq!(
        uncaught(&mut eval, "((var-get (var fn)))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/fn"
    );
    assert_eq!(
        compile_err(&mut eval, "(let 1)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | let requires a vector for its binding".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(clojure.core/let 1)"),
        vec![
            "Syntax error macroexpanding clojure.core/let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | clojure.core/let requires a vector for its binding"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [a] 1)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | let requires an even number of forms in binding vector"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [1 2] 1)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Unsupported binding form: 1".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [nil 1] 1)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Unsupported binding form: ".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn)"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Parameter declaration missing".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn f)"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Parameter declaration missing".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn 5)"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Parameter declaration 5 should be a vector"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn ())"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Parameter declaration  should be a vector"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn ([a] a) 5)"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Invalid signature 5 should be a list".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(fn (5))"),
        vec![
            "Syntax error macroexpanding fn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Parameter declaration 5 should be a vector"
                .to_owned(),
        ]
    );
    // A lazy seq is seqable but not a list: the param walk sees nothing.
    let lazy_sig = uncaught(&mut eval, "(macroexpand-1 (list 'fn (range)))");
    assert!(
        lazy_sig.contains("Parameter declaration  should be a vector"),
        "{lazy_sig}"
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn 5 1)"),
        vec![
            "Syntax error macroexpanding letfn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(with-redefs 5 1)"),
        vec![
            "Syntax error macroexpanding with-redefs at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [[a & b c] [1]] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Unsupported binding form, only :as can follow & parameter"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:foo [a]} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Unsupported map directive: :foo".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [a & b & c]} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | 'b' - binding symbols can only appear before '&', use keys after"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [a & :k & b]} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | & can only appear once in :keys".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{a a :or {a 1}} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Multiple :or defaults for same key: a 'a'".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys! [a] :or {a 1}} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Can't supply default value for required key: :a".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [a] :or {b 1} :select s} {}] s)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | symbol b in :or does not refer to a binding"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(
            &mut eval,
            "(let [{:keys [a] :or {a 1 :z 2} :select s} {}] s)"
        ),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | keys #{:z} appear only in :or".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys 5} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [1]} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Named"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys \"ab\"} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast java.lang.Character to clojure.lang.Named"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys {:a 1}} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast clojure.lang.PersistentVector to clojure.lang.Named"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [nil]} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:foo/keys [nil]} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [a] :or 5} {}] a)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(let [{:keys [a] :or [1]} {}] a)"),
        vec![
            "Unexpected error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast java.lang.Long to java.util.Map$Entry"
                .to_owned(),
        ]
    );
}

#[test]
fn eval_expanded_forms() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(let [a 1] a)"), "1");
    assert_eq!(run_last(&mut eval, "(let [a 1])"), "nil");
    assert_eq!(run_last(&mut eval, "(let [[a b] [1 2]] [a b])"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(let [[a :as m] [1 2]] m)"), "[1 2]");
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :or {a 5}} {}] a)"),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :or {}} {}] a)"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :or []} {}] a)"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(let [{:keys [a] :or {b 1}} {}] a)"),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "(let [{:strs [a]} {\"a\" 3}] a)"), "3");
    assert_eq!(run_last(&mut eval, "(let [{:syms [a]} {'a 4}] a)"), "4");
    assert_eq!(run_last(&mut eval, "(let [{:keys [a/b]} {:a/b 5}] b)"), "5");
    assert_eq!(
        run_last(&mut eval, "(let [{:foo/keys [a]} {:foo/a 8}] a)"),
        "8"
    );
    assert!(fresh("(macroexpand-1 '(let [{:keys! [& :b] :missing m} {}] m))").starts_with("(let*"));
    assert_eq!(run_last(&mut eval, "(let [{a :a} '()] a)"), "nil");
    assert_eq!(run_last(&mut eval, "(let [{a :a} '({:a 4})] a)"), "4");
    assert_eq!(
        run_last(&mut eval, "(let [{a :a} '(:a 1 {:a 9 :b 8})] a)"),
        "9"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{:keys [a] :select s :all al :excess ex} {:a 1 :b 2}] [s al ex])"
        ),
        "[{:a 1} {:a 1, :b 2} {:b 2}]"
    );
    assert_eq!(run_last(&mut eval, "((fn [a] a) 3)"), "3");
    assert_eq!(
        run_last(
            &mut eval,
            "((eval '(fn f [n] (if (pos? n) (f (dec n)) n))) 2)"
        ),
        "0"
    );
    assert_eq!(
        uncaught(&mut eval, "((eval '(fn [x] {:pre [(pos? x)]} x)) -1)"),
        "java.lang.AssertionError | Assert failed: (pos? x)"
    );
    assert_eq!(
        uncaught(&mut eval, "((eval '(fn [x] {:post [(number? %)]} x)) :k)"),
        "java.lang.AssertionError | Assert failed: (number? %)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*assert* false] ((eval '(fn [x] {:pre [(pos? x)]} x)) -1))"
        ),
        "-1"
    );
    assert_eq!(
        run_last(&mut eval, "(loop [a 2] (if (pos? a) (recur (dec a)) a))"),
        "0"
    );
    assert_eq!(run_last(&mut eval, "(loop [[a b] [1 2]] (+ a b))"), "3");
    assert_eq!(
        run_last(
            &mut eval,
            "(letfn [(f [n] (if (pos? n) (f (dec n)) n))] (f 3))"
        ),
        "0"
    );
    assert_eq!(run_last(&mut eval, "(letfn [(f [])] 1)"), "1");
    assert_eq!(
        compile_err(&mut eval, "(letfn [(f 1)] 1)"),
        vec![
            "Syntax error macroexpanding clojure.core/fn at (user:1:1).".to_owned(),
            "clojure.lang.ExceptionInfo | Call to clojure.core/fn did not conform to spec."
                .to_owned(),
        ]
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 4] *print-length*)"),
        "4"
    );
    assert_eq!(run_last(&mut eval, "(binding [])"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-bindings {(var *print-length*) 3} *print-length*)"
        ),
        "3"
    );
    assert_eq!(run_last(&mut eval, "(locking :x 1)"), "1");
    assert_eq!(run_last(&mut eval, "(locking :x (locking :x 2))"), "2");
    assert_eq!(
        uncaught(&mut eval, "(locking nil 1)"),
        "java.lang.NullPointerException | Cannot enter synchronized block because \"lock\" is null"
    );
    run_last(&mut eval, "(def z 1)");
    assert_eq!(run_last(&mut eval, "(with-redefs [z 9] z)"), "9");
    assert_eq!(run_last(&mut eval, "z"), "1");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(with-redefs [a 1 b] a))"),
        "(clojure.core/with-redefs-fn {(var a) 1} (clojure.core/fn [] a))"
    );
    assert_eq!(run_last(&mut eval, "(with-redefs [])"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(def ^:dynamic *d* 1) (def f (binding [*d* 7] (eval '(bound-fn [] *d*)))) (f)"
        ),
        "7"
    );
    assert_eq!(run_last(&mut eval, "*d*"), "1");
}

#[test]
fn host_natives_and_rewrites() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/req! {} :a)"),
        "java.lang.IllegalArgumentException | Missing required key: :a"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/req! {:a nil} :a)"),
        "nil"
    );
    assert_eq!(run_last(&mut eval, "(some-vals nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(some-vals {:a nil})"), "nil");
    assert_eq!(
        run_last(&mut eval, "(some-vals {:a 1 :b nil :c false})"),
        "{:a 1, :c false}"
    );
    assert_eq!(
        uncaught(&mut eval, "(some-vals 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    run_last(&mut eval, "(def z 1)");
    assert_eq!(
        run_last(&mut eval, "(with-redefs-fn {(var z) 9} (fn [] z))"),
        "9"
    );
    assert_eq!(run_last(&mut eval, "z"), "1");
    assert_eq!(run_last(&mut eval, "(with-redefs-fn nil (fn [] 1))"), "1");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-redefs-fn {(var z) 9} (fn [] (throw (ex-info \"e\" {})))) (catch Exception e z))"
        ),
        "1"
    );
    assert_eq!(run_last(&mut eval, "z"), "1");
    assert_eq!(
        uncaught(&mut eval, "(with-redefs-fn 1 (fn [] 1))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-redefs-fn {'z 1} (fn [] 1))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Symbol to clojure.lang.Var"
    );
    assert_eq!(
        uncaught(&mut eval, "(bound-fn* 1)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(run_last(&mut eval, "((bound-fn* (fn [x] x)) 4)"), "4");
    let obj = run_last(&mut eval, "(pr-str (new Object))");
    assert!(obj.starts_with("\"#object[java.lang.Object "), "{obj}");
    assert_eq!(
        run_last(&mut eval, "(= (new Object) (new Object))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(count (set [(new Object) (new Object)]))"),
        "2"
    );
    assert_eq!(
        run_last(&mut eval, "clojure.lang.PersistentArrayMap/EMPTY"),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array [:a 1]))"
        ),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/instance? clojure.lang.ISeq ())"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/instance? clojure.lang.ISeq [])"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(clojure.core/identical? 1 1)"), "true");
    assert_eq!(
        run_last(&mut eval, "(clojure.core/identical? 1 2)"),
        "false"
    );
    // Classify misses fall through to an unresolved var.
    for form in [
        "(clojure.core/instance? 1)",
        "(clojure.core/instance? String 1)",
        "(clojure.core/identical? 1)",
        "(clojure.core/identical? 1 2 3)",
        "(new Object 1)",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc)",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc 1)",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc ())",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc (foo 1))",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array))",
        "(clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array 1 2))",
        "(monitor-enter)",
        "(monitor-enter a b)",
    ] {
        let err = compile_err(&mut eval, form);
        assert!(!err.is_empty(), "{form} -> {err:?}");
    }
    let called = uncaught(&mut eval, "(1)");
    assert!(called.contains("ClassCastException"), "{called}");
    assert_eq!(run_last(&mut eval, "(let [new (fn [x] x)] (new 5))"), "5");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [monitor-enter (fn [x] x)] (monitor-enter 5))"
        ),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(let [letfn* (fn [x] x)] (letfn* 5))"),
        "5"
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn*)"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | letfn* requires a vector for its binding"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* 5)"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | letfn* requires a vector for its binding"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* [a])"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | letfn* requires an even number of forms in binding vector"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* [1 (fn [x] x)])"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: 1"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* [nil (fn [x] x)])"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Bad binding form, expected symbol, got: null"
                .to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* [u/f (fn [x] x)])"),
        vec![
            "Syntax error compiling letfn* at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Can't let qualified name: u/f".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(letfn* [f 1])"),
        vec![
            "Syntax error macroexpanding clojure.core/fn at (user:1:1).".to_owned(),
            "clojure.lang.ExceptionInfo | Call to clojure.core/fn did not conform to spec."
                .to_owned(),
        ]
    );
    // Private monitor natives are reachable from `clojure.core`.
    run_last(&mut eval, "(in-ns 'clojure.core)");
    assert_eq!(
        uncaught(&mut eval, "(fx4-monitor-exit nil)"),
        "java.lang.IllegalStateException | attempt to exit a monitor not held by this thread"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (fx4-monitor-enter :x) (fx4-monitor-exit :x) 1)"
        ),
        "1"
    );
    assert_eq!(
        uncaught(&mut eval, "(fx4-monitor-enter nil)"),
        "java.lang.NullPointerException | Cannot enter synchronized block because \"lock\" is null"
    );
}

#[test]
fn coverage_top_up_rows() {
    let mut eval = boot();
    // builtin_expand.rs:116 — a plain symbol beside a destructured
    // `loop` binding (oracle `ev-loop-mixed`).
    assert_eq!(run_last(&mut eval, "(loop [x 1 [a] [2]] [x a])"), "[1 2]");
    // builtin_expand.rs:228 — multi-body `fn` whose first form is not
    // a condition map (oracle `fn-multi-body`).
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fn [x] x x))"),
        "(fn* ([x] x x))"
    );
    // builtin_expand.rs:257 — `:post` over a multi-body `fn`
    // (oracle `ev-fn-post-multi`). The assertion below pins the
    // evaluated value (analyzer path); the expansion shape pins
    // the `do`-wrap arm.
    assert_eq!(
        run_last(&mut eval, "((fn [x] {:post [(pos? x)]} (inc x) x) 2)"),
        "2"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(fn [x] {:post [(pos? x)]} (inc x) x))"),
        "(fn* ([x] (clojure.core/let [% (do (inc x) x)] (clojure.core/assert (pos? x)) %)))"
    );
    // builtin_expand.rs:298 — a plain symbol beside a destructured
    // `fn` param (oracle `ev-fn-mixed`; value via the analyzer,
    // shape via the expander).
    assert_eq!(
        run_last(&mut eval, "((fn [x [a b]] [x a b]) 1 [2 3])"),
        "[1 2 3]"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(fn [x [a b]] [x a b]))"),
        "(fn* ([x p__1] (clojure.core/let [[a b] p__1] [x a b])))"
    );
    // destructure_expand.rs:114 — the `?` error path out of a vector
    // element (same arm as the `(let [1 2] 1)` pin; oracle `let-bad-vec`
    // shows the JVM rejects this shape too).
    assert_eq!(
        compile_err(&mut eval, "(let [[1] [1]] 1)"),
        vec![
            "Syntax error macroexpanding let at (user:1:1).".to_owned(),
            "java.lang.Exception | Unsupported binding form: 1".to_owned(),
        ]
    );
    // destructure_expand.rs:515+706 — keyword items in `:keys`
    // (oracle `ev-keys-kw`).
    assert_eq!(run_last(&mut eval, "(let [{:keys [:a]} {:a 1}] a)"), "1");
    // destructure_expand.rs:696 — `:keys!` without `:missing`
    // (oracle `ev-keys-bang` + `ev-keys-bang-missing`).
    assert_eq!(run_last(&mut eval, "(let [{:keys! [a]} {:a 1}] a)"), "1");
    assert_eq!(
        uncaught(&mut eval, "(let [{:keys! [a]} {}] a)"),
        "java.lang.IllegalArgumentException | Missing required key: :a"
    );
    // destructure_expand.rs:719 — a duplicate `:keys` entry updates
    // the binding token (oracle `ev-keys-dup`).
    assert_eq!(run_last(&mut eval, "(let [{:keys [a a]} {:a 1}] a)"), "1");
    // destructure_expand.rs:578/579/589 — a nested `:select` outlet
    // reuses the existing symbol (oracle `ev-select-nested`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [{{:keys [x] :select t} :p :select s} {:p {:x 1}}] [x t s])"
        ),
        "[1 {:x 1} {:p {:x 1}}]"
    );
    // fx4_form.rs:155/163 — nil/set `:keys` lists seq like the
    // analyzer's `seq` (the JVM rejects both at spec time, which
    // FX4 deliberately does not instrument; the expansion below is
    // the port's documented behavior).
    assert_eq!(
        fresh("(macroexpand-1 '(let [{:keys nil} {:a 1}] 42))"),
        "(let* [map__1 {:a 1} map__1 (if (clojure.core/instance? clojure.lang.ISeq map__1) (if (clojure.core/next map__1) (clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array map__1)) (if (clojure.core/seq map__1) (clojure.core/first map__1) clojure.lang.PersistentArrayMap/EMPTY)) map__1)] 42)"
    );
    assert_eq!(
        fresh("(macroexpand-1 '(let [{:keys #{a}} {:a 1}] a))"),
        "(let* [map__1 {:a 1} map__1 (if (clojure.core/instance? clojure.lang.ISeq map__1) (if (clojure.core/next map__1) (clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array map__1)) (if (clojure.core/seq map__1) (clojure.core/first map__1) clojure.lang.PersistentArrayMap/EMPTY)) map__1) a (clojure.core/get map__1 :a)] a)"
    );
    // fx4_host.rs:65-67 — `class_name` surfaces through `nth`
    // (oracle `ev-nth-object`).
    assert_eq!(
        uncaught(&mut eval, "(nth (new Object) 0)"),
        "java.lang.UnsupportedOperationException | nth not supported on this type: Object"
    );
}
