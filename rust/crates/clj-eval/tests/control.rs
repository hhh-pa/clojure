//! S5D control: boot dynamics, `odd?`/`even?`, `hash`, `nthrest`,
//! `key`/`val`, the `assert`/`time`/`with-out-str`/`lazy-seq*`/
//! `case*` builtins, `core.clj` `case`/`condp`, and the IAE/NPE
//! macro-error paths (corpus 11/15/17/19 spot pins).

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
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
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
            panic!("expected an uncaught throw for {src}");
        }
    }
}

fn uncaught_chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let Err(SourceError::Uncaught(t)) = eval.eval_source(src) else {
        panic!("expected an uncaught throw for {src}");
    };
    let mut out = Vec::new();
    let mut cur = Some(&t);
    while let Some(th) = cur {
        out.push(format!(
            "{} | {}",
            th.class.jvm_name(),
            th.message.as_deref().unwrap_or("")
        ));
        cur = th.cause.as_deref();
    }
    out
}

fn compile_err(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => e.chain(),
        Err(other) => panic!("expected a compile error, got {other}"),
        Ok(vs) => panic!("expected a compile error for {src}, got {vs:?}"),
    }
}

#[test]
fn core_loads() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(and true false)"), "false");
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'case))"), "false");
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'condp))"), "false");
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'assert))"), "false");
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'time))"), "false");
    assert_eq!(
        run_last(&mut eval, "(nil? (resolve 'with-out-str))"),
        "false"
    );
    // Analyzer forms have no vars (the JVM has no such vars).
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'case*))"), "true");
    assert_eq!(run_last(&mut eval, "(nil? (resolve 'lazy-seq*))"), "true");
    // Port-internal natives stay private (never referred to `user`).
    assert_eq!(
        run_last(&mut eval, "(nil? (ns-resolve 'user 'throw-illegal-arg!))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(nil? (ns-resolve 'user 'hash-code))"),
        "true"
    );
}

#[test]
fn boot_dynamics() {
    let mut eval = boot();
    // Roots (RT.java + core.clj defaults, corpus 11/15).
    assert_eq!(run_last(&mut eval, "*assert*"), "true");
    assert_eq!(run_last(&mut eval, "*unchecked-math*"), "false");
    assert_eq!(run_last(&mut eval, "*warn-on-reflection*"), "false");
    assert_eq!(run_last(&mut eval, "*allow-unresolved-vars*"), "false");
    assert_eq!(run_last(&mut eval, "*print-meta*"), "false");
    assert_eq!(run_last(&mut eval, "*print-readably*"), "true");
    assert_eq!(run_last(&mut eval, "*print-dup*"), "false");
    assert_eq!(run_last(&mut eval, "*print-length*"), "nil");
    assert_eq!(run_last(&mut eval, "*print-level*"), "nil");
    assert_eq!(run_last(&mut eval, "*print-namespace-maps*"), "true");
    assert_eq!(run_last(&mut eval, "(nil? *out*)"), "false");
    assert_eq!(run_last(&mut eval, "(nil? *err*)"), "false");
    // RT.java interns: dynamic slot, no `:dynamic` meta.
    assert_eq!(
        run_last(&mut eval, "(boolean (:dynamic (meta #'*assert*)))"),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (map (fn [v] (boolean (:dynamic (meta v)))) [#'*assert* #'*unchecked-math* #'*warn-on-reflection* #'*allow-unresolved-vars*]))"
        ),
        "[false false false false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(vec (map (fn [v] (boolean (:dynamic (meta v)))) [#'*print-meta* #'*print-readably* #'*print-dup* #'*print-length* #'*print-level* #'*print-namespace-maps*]))"
        ),
        "[false false false true true true]"
    );
    // ...but the slot binds (corpus 15 `:assert-bound-false-eval`).
    assert_eq!(
        run_last(&mut eval, "(binding [*assert* false] *assert*)"),
        "false"
    );
    // Referred into `user` like every public install.
    assert_eq!(
        run_last(&mut eval, "(nil? (ns-resolve 'user '*out*))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(nil? (ns-resolve 'user '*assert*))"),
        "false"
    );
}

#[test]
fn odd_even() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(odd? 1)"), "true");
    assert_eq!(run_last(&mut eval, "(odd? 2)"), "false");
    assert_eq!(run_last(&mut eval, "(odd? -3)"), "true");
    assert_eq!(run_last(&mut eval, "(even? 4)"), "true");
    assert_eq!(run_last(&mut eval, "(even? 4294967296)"), "true");
    assert_eq!(
        uncaught(&mut eval, "(odd? 1.5)"),
        "java.lang.IllegalArgumentException | Argument must be an integer: 1.5"
    );
    assert_eq!(
        uncaught(&mut eval, "(odd? \"a\")"),
        "java.lang.IllegalArgumentException | Argument must be an integer: a"
    );
    assert_eq!(
        uncaught(&mut eval, "(odd? nil)"),
        "java.lang.IllegalArgumentException | Argument must be an integer: "
    );
    assert_eq!(
        uncaught(&mut eval, "(even? 2.0)"),
        "java.lang.IllegalArgumentException | Argument must be an integer: 2.0"
    );
}

#[test]
fn hash_values() {
    let mut eval = boot();
    // Corpus 19 oracle values.
    assert_eq!(
        run_last(&mut eval, "(hash (take 1 (repeat :x)))"),
        "1469441678"
    );
    assert_eq!(run_last(&mut eval, "(hash (map inc [1 2]))"), "-1504369821");
    assert_eq!(run_last(&mut eval, "(hash (range 10))"), "804834377");
    assert_eq!(
        run_last(&mut eval, "(hash (filter odd? [1 2 3]))"),
        "-1179299920"
    );
    assert_eq!(
        run_last(&mut eval, "(hash (drop 1 [1 2 3]))"),
        "-1504369821"
    );
    assert_eq!(
        run_last(&mut eval, "(hash (take 2 (cycle [1])))"),
        "670845861"
    );
    assert_eq!(run_last(&mut eval, "(hash '())"), "-2017569654");
    assert_eq!(run_last(&mut eval, "(hash (repeat 0 :x))"), "-2017569654");
    assert_eq!(run_last(&mut eval, "(hash nil)"), "0");
    assert_eq!(run_last(&mut eval, "(= (hash [1]) (hash '(1)))"), "true");
    // Unbounded seqs: bare UOE, top-level, nested, and after forcing.
    for src in [
        "(hash (cycle [1]))",
        "(hash (iterate inc 0))",
        "(hash (repeat :x))",
        "(hash (range))",
        "(hash (range 0 5 0))",
        "(hash [(cycle [1])])",
        "(hash {:a (repeat 1)})",
        // NOTE: `(hash (lazy-seq* (cycle [1])))` hangs (forcing an
        // infinite lazy thunk), like the JVM — untestable.
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.UnsupportedOperationException | ",
            "{src}"
        );
    }
}

#[test]
fn hash_code_spots() {
    let mut eval = boot();
    // Private: call from `clojure.core` (`case-expand-kw` probe
    // values — Util/hash, not murmur).
    run(&mut eval, "(in-ns 'clojure.core)");
    assert_eq!(run_last(&mut eval, "(hash-code :a)"), "1013910569");
    assert_eq!(run_last(&mut eval, "(hash-code :b)"), "1013910508");
    assert_eq!(run_last(&mut eval, "(hash-code \"a\")"), "97");
    assert_eq!(run_last(&mut eval, "(hash-code nil)"), "0");
    assert_eq!(run_last(&mut eval, "(hash-code 5)"), "5");
    assert_eq!(
        uncaught(&mut eval, "(hash-code (repeat :x))"),
        "java.lang.UnsupportedOperationException | "
    );
}

#[test]
fn nthrest_rows() {
    let mut eval = boot();
    // 0/neg return the coll ITSELF (a vector, never seqed).
    assert_eq!(run_last(&mut eval, "(nthrest [1 2 3] 0)"), "[1 2 3]");
    assert_eq!(run_last(&mut eval, "(vector? (nthrest [1 2 3] 0))"), "true");
    assert_eq!(run_last(&mut eval, "(nthrest [1 2 3] -1)"), "[1 2 3]");
    assert_eq!(run_last(&mut eval, "(nthrest '(1 2 3) 0)"), "(1 2 3)");
    assert_eq!(run_last(&mut eval, "(str (nthrest [1 2 3] 5))"), "\"()\"");
    assert_eq!(run_last(&mut eval, "(nthrest [1 2 3 4] 1.5)"), "(3 4)");
    assert_eq!(run_last(&mut eval, "(nthrest '(1 2 3 4) 1.5)"), "(3 4)");
    assert_eq!(
        uncaught(&mut eval, "(nthrest [1 2 3] 9223372036854775807)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        run_last(&mut eval, "(str (nthrest '(1 2 3) 9223372036854775807))"),
        "\"()\""
    );
    // Eager coercion on vectors AND lists (before coll dispatch).
    assert_eq!(
        uncaught(&mut eval, "(nthrest [1 2] \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(nthrest '(1 2) \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(nthrest [1 2] nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    // The strict walk: non-seqables fail `seq`, nil walks to `()`.
    assert_eq!(
        uncaught(&mut eval, "(nthrest 5 1)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(nthrest nil 1)"), "()");
}

#[test]
fn key_val() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(key [:a 1])"), ":a");
    assert_eq!(run_last(&mut eval, "(val [:a 1])"), "1");
    assert_eq!(run_last(&mut eval, "(key (first {:a 1}))"), ":a");
    assert_eq!(run_last(&mut eval, "(val (first {:a 1}))"), "1");
    assert_eq!(
        uncaught(&mut eval, "(key)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/key"
    );
}

#[test]
fn assert_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(assert true)"), "nil");
    assert_eq!(run_last(&mut eval, "(assert 1 \"pass\")"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(assert (= 1 2))"),
        "java.lang.AssertionError | Assert failed: (= 1 2)"
    );
    assert_eq!(
        uncaught(&mut eval, "(assert (= 1 2) \"custom msg\")"),
        "java.lang.AssertionError | Assert failed: custom msg\n(= 1 2)"
    );
    assert_eq!(
        uncaught(&mut eval, "(assert false \"boom39\")"),
        "java.lang.AssertionError | Assert failed: boom39\nfalse"
    );
    // The message evaluates only on failure.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (try (assert false (do (print \"M\") \"msg\")) (catch AssertionError e :caught)))"
        ),
        "\"M\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (assert true (do (print \"X\") \"m\")))"
        ),
        "\"\""
    );
    // A truthy test yields nil (the `when-not` expansion).
    assert_eq!(run_last(&mut eval, "(assert :x)"), "nil");
}

#[test]
fn assert_disabled() {
    let mut eval = boot();
    // Analysis inside the binding: expansion is nil (never evaluated).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*assert* false] (eval '(assert false \"never\")))"
        ),
        "nil"
    );
    // ...and never ANALYZED (an unresolvable test compiles fine).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*assert* false] (eval '(assert nope-unresolvable)))"
        ),
        "nil"
    );
    assert_eq!(
        uncaught_chain(&mut eval, "(eval '(assert nope-unresolvable))"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error compiling at (user:1:8)."
                .to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: nope-unresolvable in this context"
                .to_owned(),
        ]
    );
    // Analysis outside the binding: the runtime binding is irrelevant.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*assert* false] (assert false \"never\"))"
        ),
        "java.lang.AssertionError | Assert failed: never\nfalse"
    );
}

#[test]
fn assert_arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(assert)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/assert"
    );
    assert_eq!(
        uncaught(&mut eval, "(assert 1 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/assert"
    );
    // Bare through `eval` too (no wrapper, single level).
    assert_eq!(
        uncaught_chain(&mut eval, "(eval '(assert))"),
        vec![
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/assert"
                .to_owned()
        ]
    );
}

#[test]
fn time_rows() {
    let mut eval = boot();
    // Value preserved, nothing escapes (captured, so tests stay quiet).
    assert_eq!(
        run_last(
            &mut eval,
            "(subs (with-out-str (def tv (time (+ 1 2)))) 0 15)"
        ),
        "\"\\\"Elapsed time: \""
    );
    assert_eq!(run_last(&mut eval, "tv"), "3");
    // Shape: prn-quoted `"Elapsed time: <millis> msecs"` + newline.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [o (with-out-str (time 42))] [(subs o 0 15) (subs o (- (count o) 7))])"
        ),
        "[\"\\\"Elapsed time: \" \"msecs\\\"\\n\"]"
    );
    assert_eq!(
        uncaught(&mut eval, "(time)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/time"
    );
    assert_eq!(
        uncaught(&mut eval, "(time 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/time"
    );
}

#[test]
fn with_out_str_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(with-out-str (print \"a\") (prn 1))"),
        "\"a1\\n\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print \"a\") (with-out-str (print \"INNER\")) (print \"c\"))"
        ),
        "\"ac\""
    );
    assert_eq!(run_last(&mut eval, "(with-out-str 42)"), "\"\"");
    assert_eq!(run_last(&mut eval, "(eval '(with-out-str))"), "\"\"");
    // The inner binding pops on throw (the outer keeps capturing).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (print \"a\") (try (with-out-str (print \"X\") (throw (ex-info \"e\" {}))) (catch Exception _ :c)) (print \"b\"))"
        ),
        "\"ab\""
    );
    // User shadows of `*out*` apply (resolved like `binding`: a
    // non-dynamic shadow fails the bind check).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def *out* :shadowed) (with-out-str (print \"q\")))"
        ),
        "java.lang.IllegalStateException | Can't dynamically bind non-dynamic var: *out*"
    );
}

#[test]
fn lazy_seq_star_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(lazy-seq*)"), "()");
    assert_eq!(run_last(&mut eval, "(lazy-seq* nil)"), "()");
    assert_eq!(run_last(&mut eval, "(vec (lazy-seq* [1 2]))"), "[1 2]");
    assert_eq!(
        uncaught(&mut eval, "(first (lazy-seq* 5))"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // Closes over locals, stays unrealized until forced.
    assert_eq!(
        run_last(&mut eval, "(let [x 41] (first (lazy-seq* [(inc x)])))"),
        "42"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def lz (lazy-seq* (print \"X\") [1])))"
        ),
        "\"\""
    );
    assert_eq!(run_last(&mut eval, "(with-out-str (first lz))"), "\"X\"");
    // A `recur` with args is the mismatch error (zero-arg boundary;
    // `s5d-priv-probe` P2: op in the outer, `IAE` cause).
    assert_eq!(
        compile_err(&mut eval, "(lazy-seq* (recur 1))"),
        vec![
            "Syntax error compiling recur at (user:1:12).".to_owned(),
            "java.lang.IllegalArgumentException | Mismatched argument count to recur, expected: 0 args, got: 1"
                .to_owned(),
        ]
    );
}

#[test]
fn case_ints() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(case 1 1 :a 2 :b :d)"), ":a");
    assert_eq!(run_last(&mut eval, "(case 2 1 :a 2 :b :d)"), ":b");
    assert_eq!(run_last(&mut eval, "(case 9 1 :a 2 :b :d)"), ":d");
    // Non-numbers miss (no error); truncations fail the post-check.
    assert_eq!(run_last(&mut eval, "(case \"a\" 1 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case nil 1 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case true 1 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case \\a 1 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case 1.5 1 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case 4294967297 1 :x :d)"), ":d");
    // Negative tests, big ranges, one/zero clauses.
    assert_eq!(run_last(&mut eval, "(case -5 -5 :a :d)"), ":a");
    assert_eq!(run_last(&mut eval, "(case 1 1)"), "1");
    assert_eq!(run_last(&mut eval, "(case 1 1 2 :bad)"), "2");
    assert_eq!(
        uncaught(&mut eval, "(case 9)"),
        "java.lang.IllegalArgumentException | No matching clause: 9"
    );
    // The expr evaluates exactly once.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (def cv (case (do (print \"E\") 5) 5 :x :d)))"
        ),
        "\"E\""
    );
    assert_eq!(run_last(&mut eval, "cv"), ":x");
}

#[test]
fn case_hashes() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(case 1.0 1.0 :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case 1 1.0 :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case 5 5.0 :a :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case \"a\" \"a\" :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case 'a 'a :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case [1] [1] :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case nil nil :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case \\a \\a :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case true true :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case false true :x :d)"), ":d");
    // 97/`\\a` share a hash: the post-check rejects.
    assert_eq!(run_last(&mut eval, "(case 97 \\a :x :d)"), ":d");
    // Grouped tests, list consts, odd shapes (corpus 15 `:c-*`).
    assert_eq!(run_last(&mut eval, "(case 2 (1 2 3) :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case 2 ((1 2)) :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case [1] ((1) :a) :b)"), ":b");
    assert_eq!(run_last(&mut eval, "(case 1 1 :a 1.5 :b :d)"), ":a");
    assert_eq!(
        uncaught(&mut eval, "(case 1.5 (1 :a) :b)"),
        "java.lang.IllegalArgumentException | No matching clause: 1.5"
    );
}

#[test]
fn case_identity() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(case :a :a :x :d)"), ":x");
    assert_eq!(run_last(&mut eval, "(case :a :b :x :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(case :a :a 1 :b 2 :d)"), "1");
}

#[test]
fn case_dup_and_nomatch() {
    let mut eval = boot();
    for (src, dup) in [
        ("(case 1 1 :a 1 :b)", "1"),
        ("(case 1 1 :a (1 2) :b)", "1"),
        ("(case :a :a 1 :a 2)", ":a"),
        ("(case \"a\" \"a\" 1 \"a\" 2)", "a"),
        ("(case nil nil :a nil :b)", ""),
    ] {
        assert_eq!(
            compile_err(&mut eval, src),
            vec![
                "Syntax error macroexpanding case at (user:1:1).".to_owned(),
                format!("java.lang.IllegalArgumentException | Duplicate case test constant: {dup}"),
            ],
            "{src}"
        );
    }
    // The `()` test is a group of zero consts: vacuous `:ints`,
    // then `(max)` of nothing escapes bare (Q1).
    assert_eq!(
        uncaught(&mut eval, "(case 1 () :a :b)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/max"
    );
    // No-match without default (values str into the message).
    assert_eq!(
        uncaught(&mut eval, "(case 5 6 :a)"),
        "java.lang.IllegalArgumentException | No matching clause: 5"
    );
    assert_eq!(
        uncaught(&mut eval, "(case \"1\" (1 :a) :d)"),
        "java.lang.IllegalArgumentException | No matching clause: 1"
    );
    // A matched `then` still evaluates (the `:case-dup` shape).
    assert_eq!(
        uncaught(&mut eval, "(case 1 (1 :a) (1 :b))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

#[test]
fn case_star_shape() {
    let mut eval = boot();
    // `macroexpand-1`: pins what `case` emits (full `macroexpand`
    // would also expand the emitted `let` on the JVM).
    run(&mut eval, "(def cx (macroexpand-1 '(case 1 1 :a 2 :b :d)))");
    assert_eq!(run_last(&mut eval, "(first cx)"), "clojure.core/let");
    assert_eq!(run_last(&mut eval, "(first (nth cx 2))"), "case*");
    assert_eq!(run_last(&mut eval, "(nth (nth cx 2) 2)"), "0");
    assert_eq!(run_last(&mut eval, "(nth (nth cx 2) 3)"), "0");
    assert_eq!(run_last(&mut eval, "(nth (nth cx 2) 6)"), ":compact");
    assert_eq!(run_last(&mut eval, "(nth (nth cx 2) 7)"), ":int");
    run(
        &mut eval,
        "(def ck (macroexpand-1 '(case :a :a 1 :b 2 :d)))",
    );
    assert_eq!(run_last(&mut eval, "(nth (nth ck 2) 7)"), ":hash-identity");
    // Short `case*` keeps the spec error (the JVM's IOOBE is an
    // accident — frozen wedge, T1192 FX1).
    assert_eq!(
        compile_err(&mut eval, "(case* 1)"),
        vec![
            "Syntax error macroexpanding clojure.core/case* at (user:1:1).".to_owned(),
            "clojure.lang.ExceptionInfo | Call to clojure.core/case* did not conform to spec."
                .to_owned(),
        ]
    );
    // Empty cases evaluate (the JVM NPEs here — a hand-written-only
    // accident, frozen wedge).
    assert_eq!(
        run_last(&mut eval, "(case* 1 0 0 :dflt {} :sparse :int)"),
        ":dflt"
    );
}

#[test]
fn condp_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(condp = 5 5 :a :d)"), ":a");
    assert_eq!(run_last(&mut eval, "(condp = 5 6 :a :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(condp = 5 5 :>> str)"), "\"true\"");
    assert_eq!(run_last(&mut eval, "(condp = nil nil :a :d)"), ":a");
    // `(pred test expr)`: `(< 10 5)` misses, `(< 5 10)` hits (probed).
    assert_eq!(run_last(&mut eval, "(condp < 5 10 :a :d)"), ":d");
    assert_eq!(run_last(&mut eval, "(condp < 10 5 :a :d)"), ":a");
    assert_eq!(
        uncaught(&mut eval, "(condp = 5 6 :a)"),
        "java.lang.IllegalArgumentException | No matching clause: 5"
    );
    assert_eq!(
        uncaught(&mut eval, "(condp = 5)"),
        "java.lang.IllegalArgumentException | No matching clause: 5"
    );
    assert_eq!(
        uncaught(&mut eval, "(condp)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/condp"
    );
    assert_eq!(
        uncaught(&mut eval, "(condp =)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/condp"
    );
}

#[test]
fn macro_error_classes() {
    let mut eval = boot();
    // `cond` odd clauses: IAE (was ex-info).
    assert_eq!(
        compile_err(&mut eval, "(cond :a)"),
        vec![
            "Syntax error macroexpanding cond at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | cond requires an even number of forms".to_owned(),
        ]
    );
    // `assert-args` shape (via `doseq`).
    assert_eq!(
        compile_err(&mut eval, "(doseq x [1])"),
        vec![
            "Syntax error macroexpanding doseq at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | doseq requires a vector for its binding in user:1"
                .to_owned(),
        ]
    );
    // `for` validates keywords (IAE); `doseq` reproduces the JVM NPE.
    assert_eq!(
        compile_err(&mut eval, "(for [x [1] :bogus 1] x)"),
        vec![
            "Syntax error macroexpanding for at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Invalid 'for' keyword :bogus".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(doseq [x [1] :bogus 1] x)"),
        vec![
            "Unexpected error macroexpanding doseq at (user:1:1).".to_owned(),
            "java.lang.NullPointerException | Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"steppair\" is null"
                .to_owned(),
        ]
    );
    // `defn` name check.
    assert_eq!(
        compile_err(&mut eval, "(defn 1 [x] x)"),
        vec![
            "Syntax error macroexpanding defn at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | First argument to defn must be a symbol"
                .to_owned(),
        ]
    );
}

#[test]
fn composites_smoke() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(defn f [x] (* x 2))"), "#'user/f");
    assert_eq!(run_last(&mut eval, "(f 21)"), "42");
    assert_eq!(
        run_last(&mut eval, "(for [x [1 2 3] :when (odd? x)] x)"),
        "(1 3)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-out-str (doseq [x [1 2 3]] (when (odd? x) (print x))))"
        ),
        "\"13\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (dotimes [i 3] (print i)))"),
        "\"012\""
    );
    assert_eq!(run_last(&mut eval, "(doto [1 2] (conj 3))"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(lazy-seq [1 2])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(vec (lazy-cat [1] [2]))"), "[1 2]");
    // `dotimes` over a string fails the `<` cast, like the JVM.
    assert_eq!(
        uncaught(&mut eval, "(dotimes [i \"a\"] i)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn builtin_macroexpand_and_shadow() {
    let mut eval = boot();
    // Analyzer builtins expand to themselves (the `binding` precedent).
    for src in [
        "(macroexpand '(assert x))",
        "(macroexpand '(time x))",
        "(macroexpand '(with-out-str x))",
        "(macroexpand '(lazy-seq* x))",
        "(macroexpand '(case* x 0 0 :d {} :sparse :int))",
    ] {
        let want = src
            .strip_prefix("(macroexpand '")
            .and_then(|s| s.strip_suffix(')'))
            .unwrap();
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // ...but a user definition shadows them (they are macros).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defmacro assert [& _] :shadowed) (assert true))"
        ),
        ":shadowed"
    );
    assert_eq!(run_last(&mut eval, "(clojure.core/assert true)"), "nil");
}

#[test]
fn eval_chains_and_catch() {
    let mut eval = boot();
    // `eval` failures carry the structured `CompilerException` chain.
    assert_eq!(
        uncaught_chain(&mut eval, "(eval '(case 1 1 :a 1 :b))"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error macroexpanding case at (user:1:8)."
                .to_owned(),
            "java.lang.IllegalArgumentException | Duplicate case test constant: 1".to_owned(),
        ]
    );
    // ...catchable as `CompilerException`, with `ex-cause` walking.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (eval '(case 1 1 :a 1 :b)) (catch clojure.lang.Compiler$CompilerException e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (eval '(case 1 1 :a 1 :b)) (catch Throwable t (ex-message (ex-cause t))))"
        ),
        "\"Duplicate case test constant: 1\""
    );
    // `CompilerException` short stays unresolvable (JVM-true).
    assert_eq!(
        compile_err(&mut eval, "(try 1 (catch CompilerException e 2))"),
        vec![
            "Syntax error compiling try at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Unable to resolve classname: CompilerException"
                .to_owned(),
        ]
    );
}

#[test]
fn control_backlog_rows() {
    let mut eval = boot();
    // `or` (first truthy; empty is nil).
    assert_eq!(run_last(&mut eval, "(or false nil 3)"), "3");
    assert_eq!(run_last(&mut eval, "(or)"), "nil");
    // `when-not`/`if-not`.
    assert_eq!(run_last(&mut eval, "(when-not false 1)"), "1");
    assert_eq!(run_last(&mut eval, "(when-not true 1)"), "nil");
    assert_eq!(run_last(&mut eval, "(if-not false 1 2)"), "1");
    assert_eq!(run_last(&mut eval, "(if-not true 1 2)"), "2");
    // `if-let`/`when-let` (test on truthiness).
    assert_eq!(run_last(&mut eval, "(if-let [x 1] x 2)"), "1");
    assert_eq!(run_last(&mut eval, "(if-let [x nil] x 2)"), "2");
    assert_eq!(run_last(&mut eval, "(when-let [x 1] x)"), "1");
    assert_eq!(run_last(&mut eval, "(when-let [x nil] 1)"), "nil");
    // `if-some`/`when-some` (test on non-nil: `false`/`0` pass).
    assert_eq!(run_last(&mut eval, "(if-some [x nil] 1 2)"), "2");
    assert_eq!(run_last(&mut eval, "(if-some [x false] 1 2)"), "1");
    assert_eq!(run_last(&mut eval, "(when-some [x 0] x)"), "0");
    assert_eq!(run_last(&mut eval, "(when-some [x nil] 1)"), "nil");
    // `cond` branching (no match and no default is nil).
    assert_eq!(run_last(&mut eval, "(cond false 1 :else 2)"), "2");
    assert_eq!(run_last(&mut eval, "(cond nil 1)"), "nil");
    assert_eq!(run_last(&mut eval, "(cond false 1)"), "nil");
    // Threading macros.
    assert_eq!(run_last(&mut eval, "(-> 1 inc inc)"), "3");
    assert_eq!(run_last(&mut eval, "(->> 1 inc inc)"), "3");
    assert_eq!(run_last(&mut eval, "(as-> 1 x (inc x) (inc x))"), "3");
    assert_eq!(run_last(&mut eval, "(cond-> 1 true inc false dec)"), "2");
    assert_eq!(run_last(&mut eval, "(cond->> 1 true inc)"), "2");
    // `comment` is nil.
    assert_eq!(run_last(&mut eval, "(comment anything)"), "nil");
    // `defn-` marks `:private`.
    assert_eq!(run_last(&mut eval, "(defn- pf [] 1)"), "#'user/pf");
    assert_eq!(run_last(&mut eval, "(:private (meta #'pf))"), "true");
    // `defonce`: first wins; the re-def is a nil `when-not` miss.
    // NOTE: the task brief expected `#'user/df` from the second
    // `defonce`; the port (like the JVM) returns nil there.
    assert_eq!(run_last(&mut eval, "(defonce df 7)"), "#'user/df");
    assert_eq!(run_last(&mut eval, "df"), "7");
    assert_eq!(run_last(&mut eval, "(defonce df 9)"), "nil");
    assert_eq!(run_last(&mut eval, "df"), "7");
}

#[test]
fn ex_data_trampoline_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(ex-data (ex-info \"m\" {:a 1}))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(trampoline (fn f [n] (if (zero? n) :done #(f (dec n)))) 100)"
        ),
        ":done"
    );
}

#[test]
fn defn_pre_violation() {
    let mut eval = boot();
    run(&mut eval, "(defn f [x] {:pre [(pos? x)]} x)");
    assert_eq!(
        uncaught(&mut eval, "(f -1)"),
        "java.lang.AssertionError | Assert failed: (pos? x)"
    );
    assert_eq!(run_last(&mut eval, "(f 1)"), "1");
}

#[test]
fn defn_post_violation() {
    let mut eval = boot();
    run(&mut eval, "(defn f [x] {:post [(pos? %)]} x)");
    assert_eq!(
        uncaught(&mut eval, "(f -1)"),
        "java.lang.AssertionError | Assert failed: (pos? %)"
    );
    assert_eq!(run_last(&mut eval, "(f 1)"), "1");
}

#[test]
fn defn_multi_arity_pre_per_clause() {
    let mut eval = boot();
    run(
        &mut eval,
        "(defn f ([x] {:pre [(pos? x)]} x) ([x y] {:pre [(pos? y)]} (+ x y)))",
    );
    // Each clause enforces only its own conditions.
    assert_eq!(
        uncaught(&mut eval, "(f -1)"),
        "java.lang.AssertionError | Assert failed: (pos? x)"
    );
    assert_eq!(
        uncaught(&mut eval, "(f 1 -2)"),
        "java.lang.AssertionError | Assert failed: (pos? y)"
    );
    assert_eq!(run_last(&mut eval, "(f 1)"), "1");
    assert_eq!(run_last(&mut eval, "(f 1 2)"), "3");
}

#[test]
fn core_self_calls_survive_shadow() {
    let mut eval = boot();
    // A shadowing `and` hijacks bare 3+-arity recursion unless the
    // emitted self-call is qualified (same for if-let/if-some).
    run(&mut eval, "(defmacro and [& _] :shadowed)");
    assert_eq!(run_last(&mut eval, "(and true true true)"), ":shadowed");
    assert_eq!(
        run_last(&mut eval, "(clojure.core/and true true true)"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/and true false true)"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(clojure.core/and 1 2 3)"), "3");
    run(&mut eval, "(defmacro if-let [& _] :shadowed)");
    assert_eq!(run_last(&mut eval, "(if-let [x 1] x)"), ":shadowed");
    assert_eq!(
        run_last(&mut eval, "(clojure.core/if-let [x 1] x :else)"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/if-let [x nil] x :else)"),
        ":else"
    );
}

#[test]
fn core_r4_trampoline_callee() {
    let mut eval = boot();
    // nil callee: the stable port text (contract-pinned).
    assert_eq!(
        uncaught(&mut eval, "(trampoline nil)"),
        "java.lang.NullPointerException | Cannot invoke nil as a function"
    );
    // Non-`IFn` callee: the `IFn` cast failure (oracle-agreeing).
    assert_eq!(
        uncaught(&mut eval, "(trampoline 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IFn (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IFn is in unnamed module of loader 'app')"
    );
}

#[test]
fn core_r4_ex_info_rows() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(ex-info \"m\")"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/ex-info"
    );
    assert_eq!(
        uncaught(&mut eval, "(ex-info 5 {})"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(ex-info \"m\" 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        run_last(&mut eval, "(ex-cause (ex-info \"m\" {} nil))"),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(ex-message (ex-cause (ex-info \"m\" {} (ex-info \"c\" {}))))"
        ),
        "\"c\""
    );
    assert_eq!(
        uncaught(&mut eval, "(ex-info \"m\" {} 5)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Throwable (java.lang.Long and java.lang.Throwable are in module java.base of loader 'bootstrap')"
    );
    // Non-throwables project to nil.
    assert_eq!(run_last(&mut eval, "(ex-data nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(ex-message 5)"), "nil");
    assert_eq!(run_last(&mut eval, "(ex-cause \"s\")"), "nil");
}

#[test]
fn core_r4_trampoline_arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(trampoline)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/trampoline"
    );
}

#[test]
fn core_r4_hash_post_realize_uoe() {
    let mut eval = boot();
    // The infinite hides behind a pending `map`: the pre-realize
    // check passes, the post-realize check throws (oracle: bare UOE).
    assert_eq!(
        uncaught(&mut eval, "(hash (map identity [(repeat 1)]))"),
        "java.lang.UnsupportedOperationException | "
    );
    // (`hash-code` is private — its identical post-check is pinned
    // by the `hash_code_post_realize_uoe` unit test in core.rs.)
}

#[test]
fn core_r4_clojure_version() {
    let mut eval = boot();
    // Port release literal (deliberately NOT the JVM version).
    assert_eq!(run_last(&mut eval, "(clojure-version)"), "\"0.1.0\"");
}

#[test]
fn ext_identity_eq_hash_rows() {
    let mut eval = boot();
    // Functions compare by identity.
    assert_eq!(run_last(&mut eval, "(let [f (fn [x] x)] (= f f))"), "true");
    assert_eq!(run_last(&mut eval, "(= (fn [x] x) (fn [x] x))"), "false");
    assert_eq!(run_last(&mut eval, "(= (fn [x] x) *ns*)"), "false");
    assert_eq!(run_last(&mut eval, "(= + +)"), "true");
    assert_eq!(run_last(&mut eval, "(= + -)"), "false");
    assert_eq!(run_last(&mut eval, "(= + (fn [x] x))"), "false");
    // Namespaces compare by identity.
    assert_eq!(run_last(&mut eval, "(= *ns* *ns*)"), "true");
    assert_eq!(run_last(&mut eval, "(= *ns* (fn [x] x))"), "false");
    // Hashing is identity-consistent (`hasheq` by id; the std
    // `Hash` impl routes through `dyn_hash`, pinned by
    // `ext_transient_keys_hash_by_id`).
    assert_eq!(
        run_last(&mut eval, "(let [f (fn [x] x)] (= (hash f) (hash f)))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(= (hash +) (hash +))"), "true");
    assert_eq!(run_last(&mut eval, "(= (hash *ns*) (hash *ns*))"), "true");
    assert_eq!(
        run_last(&mut eval, "(let [f (fn [x] x)] (get {f 42} f))"),
        "42"
    );
}

#[test]
fn ext_transient_keys_hash_by_id() {
    let mut eval = boot();
    // Transient maps std-hash their keys, routing extensions
    // through `dyn_hash` (identity): round-trips hit.
    assert_eq!(
        run_last(&mut eval, "(get (assoc! (transient {}) + 1) +)"),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [f (fn [x] x)] (get (assoc! (transient {}) f 1) f))"
        ),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(get (assoc! (transient {}) *ns* 1) *ns*)"),
        "1"
    );
}

#[test]
fn case_hash_unbounded_seq_throws() {
    let mut eval = boot();
    // Hash dispatch over an open range is bare UOE (`nil`
    // message ⟺ port `String::new()`).
    assert_eq!(
        uncaught(&mut eval, "(case (range) :a 1 :b)"),
        "java.lang.UnsupportedOperationException | "
    );
    // ... nested in a realized collection too.
    assert_eq!(
        uncaught(&mut eval, "(case [(range)] :a 1 :b)"),
        "java.lang.UnsupportedOperationException | "
    );
    // ... and behind a pending `map`, where only the
    // post-realize check fires (the pre-realize check passes;
    // `realize_deep` returns early via its hash guard instead
    // of hanging).
    assert_eq!(
        uncaught(&mut eval, "(case (map identity [(repeat 1)]) :a 1 :b)"),
        "java.lang.UnsupportedOperationException | "
    );
}

#[test]
fn case_star_shift_mask_dispatch() {
    let mut eval = boot();
    // The oracle's own emission for `(case x 1 :a :b :c :d)`,
    // evaluated verbatim. Depends on `hasheq` parity with the
    // JVM (pinned by the `hash` tests): `hash(1) & 1 == 1`,
    // `hash(:b) & 1 == 0`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [G__181 1] (case* G__181 0 1 :d {0 [:b :c] 1 [1 :a]} :compact :hash-equiv nil))"
        ),
        ":a"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [G__181 :b] (case* G__181 0 1 :d {0 [:b :c] 1 [1 :a]} :compact :hash-equiv nil))"
        ),
        ":c"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [G__181 :zzz] (case* G__181 0 1 :d {0 [:b :c] 1 [1 :a]} :compact :hash-equiv nil))"
        ),
        ":d"
    );
}
