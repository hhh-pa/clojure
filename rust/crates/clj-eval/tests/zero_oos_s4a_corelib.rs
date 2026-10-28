// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S4a corelib small vars: probe replay for `some->` /
//! `some->>` / `when-first` / `while`, the `parse-*` natives,
//! `printf` / `println-str` / `prn-str`, `rand-nth`, and `test`
//! (see `docs/zero-oos-s4a-corelib/probes/corelib.{clj,out}`).
//! RNG pins are shape/range only — exact draws differ across
//! runtimes by design.

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

/// Stabilize gensyms in macroexpansion pins (`G__12` → `G__N`,
/// `xs__12__auto__` → `xs__N__auto__`).
fn norm_gensym(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let bytes = s.as_bytes();
    let mut i = 0;
    while i < bytes.len() {
        let rest = &s[i..];
        if let Some(tail) = rest
            .strip_prefix("xs__")
            .filter(|t| t.bytes().next().is_some_and(|b| b.is_ascii_digit()))
        {
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            if tail[digits..].starts_with("__auto__") {
                out.push_str("xs__N__auto__");
                i += "xs__".len() + digits + "__auto__".len();
                continue;
            }
        }
        if let Some(tail) = rest
            .strip_prefix("G__")
            .filter(|t| t.bytes().next().is_some_and(|b| b.is_ascii_digit()))
        {
            let digits = tail.bytes().take_while(u8::is_ascii_digit).count();
            out.push_str("G__N");
            i += "G__".len() + digits;
            continue;
        }
        out.push(bytes[i] as char);
        i += 1;
    }
    out
}

#[test]
fn some_thread_first_rows() {
    let mut eval = boot();
    // st-basic / st-nil-first / st-nil-mid / st-none / st-bare.
    assert_eq!(run_last(&mut eval, "(some-> {:a {:b 42}} :a :b)"), "42");
    assert_eq!(run_last(&mut eval, "(some-> nil :a :b)"), "nil");
    assert_eq!(run_last(&mut eval, "(some-> {:a nil} :a :b)"), "nil");
    assert_eq!(run_last(&mut eval, "(some-> 7)"), "7");
    assert_eq!(run_last(&mut eval, "(some-> 1 inc inc)"), "3");
    // st-thread: multi-step first-position threading.
    assert_eq!(
        run_last(&mut eval, "(some-> [3 1 2] (conj 9) (sort) (first))"),
        "1"
    );
    // st-short / st-skips: later steps never run on nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [ran (atom [])] [(some-> 5 ((fn [x] (swap! ran conj :a) x))) @ran])"
        ),
        "[5 [:a]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [ran (atom [])] [(some-> nil ((fn [x] (swap! ran conj :a) x))) @ran])"
        ),
        "[nil []]"
    );
    // st-exp: one let, nil-guarded steps, last step in the body.
    assert_eq!(
        norm_gensym(&run_last(&mut eval, "(macroexpand-1 '(some-> x (f 1) g))")),
        "(clojure.core/let [G__N x G__N (if (clojure.core/nil? G__N) nil (clojure.core/-> G__N (f 1)))] (if (clojure.core/nil? G__N) nil (clojure.core/-> G__N g)))"
    );
}

#[test]
fn some_thread_last_rows() {
    let mut eval = boot();
    // stt-basic / stt-nil-first / stt-none / stt-bare.
    assert_eq!(
        run_last(
            &mut eval,
            "(some->> [1 2 3] (map inc) (filter odd?) (into []))"
        ),
        "[3]"
    );
    assert_eq!(run_last(&mut eval, "(some->> nil (map inc))"), "nil");
    assert_eq!(run_last(&mut eval, "(some->> 7)"), "7");
    assert_eq!(run_last(&mut eval, "(some->> 1 (inc) (inc))"), "3");
    // stt-nil-mid: `(inc nil)` would throw — the nil guard skips it.
    assert_eq!(
        run_last(&mut eval, "(some->> [1] (map (fn [_] nil)) (first) (inc))"),
        "nil"
    );
    // stt-exp: same shape over `->>`.
    assert_eq!(
        norm_gensym(&run_last(
            &mut eval,
            "(macroexpand-1 '(some->> x (f 1) (g)))"
        )),
        "(clojure.core/let [G__N x G__N (if (clojure.core/nil? G__N) nil (clojure.core/->> G__N (f 1)))] (if (clojure.core/nil? G__N) nil (clojure.core/->> G__N (g))))"
    );
}

#[test]
fn when_first_rows() {
    let mut eval = boot();
    // wf-basic / wf-empty / wf-nil / wf-str.
    assert_eq!(run_last(&mut eval, "(when-first [x [1 2]] x)"), "1");
    assert_eq!(run_last(&mut eval, "(when-first [x []] :body)"), "nil");
    assert_eq!(run_last(&mut eval, "(when-first [x nil] :body)"), "nil");
    assert_eq!(run_last(&mut eval, "(when-first [x \"ab\"] x)"), "\\a");
    // wf-once: `xs` evaluates exactly once.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [n (atom 0)] [(when-first [x (do (swap! n inc) [7])] x) @n])"
        ),
        "[7 1]"
    );
    // wf-badbind / wf-badcount1 / wf-badcount3: stock assert text
    // under the macroexpand wrapper (one two-line message; the
    // `:line` suffix is the single-line call site — deterministic).
    for (src, cause) in [
        (
            "(macroexpand-1 '(when-first x :body))",
            "when-first requires a vector for its binding in user:1",
        ),
        (
            "(macroexpand-1 '(when-first [x] :body))",
            "when-first requires exactly 2 forms in binding vector in user:1",
        ),
        (
            "(macroexpand-1 '(when-first [x y z] :body))",
            "when-first requires exactly 2 forms in binding vector in user:1",
        ),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!(
                "java.lang.IllegalArgumentException | Syntax error macroexpanding when-first at (user:1:1).\njava.lang.IllegalArgumentException | {cause}"
            ),
            "{src}"
        );
    }
    // wf-exp: `when-let` over one `seq` pull plus a `first` let.
    assert_eq!(
        norm_gensym(&run_last(
            &mut eval,
            "(macroexpand-1 '(when-first [x xs] body))"
        )),
        "(clojure.core/when-let [xs__N__auto__ (clojure.core/seq xs)] (clojure.core/let [x (clojure.core/first xs__N__auto__)] body))"
    );
}

#[test]
fn while_rows() {
    let mut eval = boot();
    // wh-loop: body runs till the test fails; the form is nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (atom 3) seen (atom [])] [(while (pos? @a) (swap! seen conj @a) (swap! a dec)) @seen])"
        ),
        "[nil [3 2 1]]"
    );
    // wh-false / wh-nil: a false test skips the body, still nil.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [ran (atom false)] [(while false (reset! ran true)) @ran])"
        ),
        "[nil false]"
    );
    assert_eq!(run_last(&mut eval, "(while nil :body)"), "nil");
    // wh-exp: empty `loop` over a `when` + bare `recur`.
    assert_eq!(
        norm_gensym(&run_last(&mut eval, "(macroexpand-1 '(while t b1 b2))")),
        "(clojure.core/loop [] (clojure.core/when t b1 b2 (recur)))"
    );
}

#[test]
fn parse_boolean_rows() {
    let mut eval = boot();
    // pb-true / pb-false / pb-case / pb-upper / pb-empty / pb-pad / pb-junk.
    for (src, want) in [
        ("(parse-boolean \"true\")", "true"),
        ("(parse-boolean \"false\")", "false"),
        ("(parse-boolean \"True\")", "nil"),
        ("(parse-boolean \"TRUE\")", "nil"),
        ("(parse-boolean \"\")", "nil"),
        ("(parse-boolean \" true\")", "nil"),
        ("(parse-boolean \"yes\")", "nil"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // pb-nil / pb-int / pb-bool / pb-kw / pb-vec: parsing-err text.
    for (src, want) in [
        (
            "(parse-boolean nil)",
            "java.lang.IllegalArgumentException | Expected string, got nil",
        ),
        (
            "(parse-boolean 1)",
            "java.lang.IllegalArgumentException | Expected string, got java.lang.Long",
        ),
        (
            "(parse-boolean true)",
            "java.lang.IllegalArgumentException | Expected string, got java.lang.Boolean",
        ),
        (
            "(parse-boolean :k)",
            "java.lang.IllegalArgumentException | Expected string, got clojure.lang.Keyword",
        ),
        (
            "(parse-boolean [1])",
            "java.lang.IllegalArgumentException | Expected string, got clojure.lang.PersistentVector",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn parse_long_rows() {
    let mut eval = boot();
    // pl-dec / pl-neg / pl-plus / pl-zero / pl-negzero / pl-max /
    // pl-maxover / pl-min / pl-minover.
    for (src, want) in [
        ("(parse-long \"42\")", "42"),
        ("(parse-long \"-42\")", "-42"),
        ("(parse-long \"+42\")", "42"),
        ("(parse-long \"0\")", "0"),
        ("(parse-long \"-0\")", "0"),
        (
            "(parse-long \"9223372036854775807\")",
            "9223372036854775807",
        ),
        ("(parse-long \"9223372036854775808\")", "nil"),
        (
            "(parse-long \"-9223372036854775808\")",
            "-9223372036854775808",
        ),
        ("(parse-long \"-9223372036854775809\")", "nil"),
        // pl-empty / pl-plusonly / pl-negonly / pl-pad / pl-trail.
        ("(parse-long \"\")", "nil"),
        ("(parse-long \"+\")", "nil"),
        ("(parse-long \"-\")", "nil"),
        ("(parse-long \" 42\")", "nil"),
        ("(parse-long \"42 \")", "nil"),
        // pl-hex / pl-float / pl-under / pl-exp.
        ("(parse-long \"0x1A\")", "nil"),
        ("(parse-long \"4.5\")", "nil"),
        ("(parse-long \"1_0\")", "nil"),
        ("(parse-long \"1e3\")", "nil"),
        // pl-arabic / pl-fullwidth: every Nd script parses.
        ("(parse-long \"\u{661}\u{662}\u{663}\")", "123"),
        ("(parse-long \"\u{FF11}\u{FF12}\u{FF13}\")", "123"),
        // pl-uminus: U+2212 is not an ASCII sign.
        ("(parse-long \"\u{2212}42\")", "nil"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // pl-nil / pl-int: parsing-err text.
    assert_eq!(
        uncaught(&mut eval, "(parse-long nil)"),
        "java.lang.IllegalArgumentException | Expected string, got nil"
    );
    assert_eq!(
        uncaught(&mut eval, "(parse-long 1)"),
        "java.lang.IllegalArgumentException | Expected string, got java.lang.Long"
    );
}

#[test]
fn parse_double_rows() {
    let mut eval = boot();
    // pd-dec / pd-negzero / pd-nan / pd-pnan / pd-nnan.
    for (src, want) in [
        ("(parse-double \"1.5\")", "1.5"),
        ("(parse-double \"-0.0\")", "-0.0"),
        ("(parse-double \"NaN\")", "##NaN"),
        ("(parse-double \"+NaN\")", "##NaN"),
        ("(parse-double \"-NaN\")", "##NaN"),
        // pd-nanlow / pd-nanup: case-exact spelling.
        ("(parse-double \"nan\")", "nil"),
        ("(parse-double \"NAN\")", "nil"),
        // pd-inf / pd-pinf / pd-ninf / pd-inflow / pd-infcap.
        ("(parse-double \"Infinity\")", "##Inf"),
        ("(parse-double \"+Infinity\")", "##Inf"),
        ("(parse-double \"-Infinity\")", "##-Inf"),
        ("(parse-double \"inf\")", "nil"),
        ("(parse-double \"Inf\")", "nil"),
        // pd-exp / pd-expneg / pd-pad / pd-empty / pd-blank / pd-junk.
        ("(parse-double \"1e3\")", "1000.0"),
        ("(parse-double \"1E-3\")", "0.001"),
        ("(parse-double \"  1.5  \")", "1.5"),
        ("(parse-double \"\")", "nil"),
        ("(parse-double \"   \")", "nil"),
        ("(parse-double \"abc\")", "nil"),
        // pd-sufd / pd-sufD / pd-suff / pd-sufF.
        ("(parse-double \"1d\")", "1.0"),
        ("(parse-double \"1D\")", "1.0"),
        ("(parse-double \"1f\")", "1.0"),
        ("(parse-double \"1F\")", "1.0"),
        // pd-dot / pd-trail / pd-lead / pd-pluslead / pd-trailexp.
        ("(parse-double \".\")", "nil"),
        ("(parse-double \"5.\")", "5.0"),
        ("(parse-double \".5\")", "0.5"),
        ("(parse-double \"+.5\")", "0.5"),
        ("(parse-double \"5.e3\")", "5000.0"),
        // pd-hex / pd-hexf / pd-hexsuf / pd-hexup / pd-hexnop.
        ("(parse-double \"0x1p3\")", "8.0"),
        ("(parse-double \"0x1.8p1\")", "3.0"),
        ("(parse-double \"0x1p3f\")", "8.0"),
        ("(parse-double \"0X1P3\")", "8.0"),
        ("(parse-double \"0x10\")", "nil"),
        // pd-hexempty / pd-hexexpjunk / pd-hexbigexp /
        // pd-hexemptysig / pd-hexnegexp / pd-hexneg /
        // pd-hexplusexp: exponent and sign edges.
        ("(parse-double \"0x1p\")", "nil"),
        ("(parse-double \"0x1p3x\")", "nil"),
        ("(parse-double \"0x1p100000\")", "##Inf"),
        ("(parse-double \"0x.p3\")", "nil"),
        ("(parse-double \"0x1p-3\")", "0.125"),
        ("(parse-double \"-0x1p3\")", "-8.0"),
        ("(parse-double \"0x1p+3\")", "8.0"),
        // pd-hexjunk / pd-hexdots: junk inside the significand.
        ("(parse-double \"0x1zp3\")", "nil"),
        ("(parse-double \"0x1.2.3p4\")", "nil"),
        // pd-nansuf / pd-infsuf: no suffix on bare specials.
        ("(parse-double \"NaNF\")", "nil"),
        ("(parse-double \"InfinityF\")", "nil"),
        // pd-over / pd-overneg / pd-under: overflow is ±Inf.
        ("(parse-double \"1e999\")", "##Inf"),
        ("(parse-double \"-1e999\")", "##-Inf"),
        ("(parse-double \"1e-999\")", "0.0"),
        // pd-nbsp / pd-tab: Java-trim width (nbsp survives).
        ("(parse-double \"\u{A0}1.5\")", "nil"),
        ("(parse-double \"\t1.5\n\")", "1.5"),
        // pd-nanpad: trim plus sign plus `NaN`.
        ("(parse-double \"  +NaN  \")", "##NaN"),
        // pd-arabic / pd-fullwidth: significands are ASCII-only.
        ("(parse-double \"\u{661}\u{662}\u{663}\")", "nil"),
        ("(parse-double \"\u{FF11}.\u{FF15}\")", "nil"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    // pd-nil / pd-dbl / pd-bool: parsing-err text.
    for (src, want) in [
        (
            "(parse-double nil)",
            "java.lang.IllegalArgumentException | Expected string, got nil",
        ),
        (
            "(parse-double 1.5)",
            "java.lang.IllegalArgumentException | Expected string, got java.lang.Double",
        ),
        (
            "(parse-double true)",
            "java.lang.IllegalArgumentException | Expected string, got java.lang.Boolean",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn printf_rows() {
    let mut eval = boot();
    // pf-basic / pf-none / pf-nl / pf-ret / pf-percent.
    assert_eq!(
        run_last(&mut eval, "(with-out-str (printf \"%s=%d\" \"a\" 1))"),
        "\"a=1\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (printf \"hi\"))"),
        "\"hi\""
    );
    // `%n` is the platform separator (JVM `Formatter` parity —
    // `\r\n` on Windows), like the `format.rs` pins.
    // (`run_last` pr-prints, so the expectation carries the
    // pr-escaped separator text.)
    let nl = if cfg!(windows) { "\\r\\n" } else { "\\n" };
    assert_eq!(
        run_last(&mut eval, "(with-out-str (printf \"a%n\"))"),
        format!("\"a{nl}\"")
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (printf \"%d\" 1))"),
        "\"1\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (printf \"100%%\"))"),
        "\"100%\""
    );
    // `printf` answers nil like `print` (stdout write is captured).
    assert_eq!(run_last(&mut eval, "(printf \"q\")"), "nil");
}

#[test]
fn println_str_rows() {
    let mut eval = boot();
    // pls-basic / pls-empty / pls-str.
    assert_eq!(
        run_last(&mut eval, "(println-str 1 nil \"x\")"),
        "\"1 nil x\\n\""
    );
    assert_eq!(run_last(&mut eval, "(println-str)"), "\"\\n\"");
    assert_eq!(run_last(&mut eval, "(println-str \"a b\")"), "\"a b\\n\"");
}

#[test]
fn prn_str_rows() {
    let mut eval = boot();
    // prs-basic / prs-empty / prs-vec.
    assert_eq!(
        run_last(&mut eval, "(prn-str 1 nil \"x\")"),
        "\"1 nil \\\"x\\\"\\n\""
    );
    assert_eq!(run_last(&mut eval, "(prn-str)"), "\"\\n\"");
    assert_eq!(
        run_last(&mut eval, "(prn-str [1 \"a\" :k])"),
        "\"[1 \\\"a\\\" :k]\\n\""
    );
}

#[test]
fn rand_nth_rows() {
    let mut eval = boot();
    // rn-member: 300 draws, every one a member (shape only).
    assert_eq!(
        run_last(
            &mut eval,
            "(every? #{1 2 3} (repeatedly 300 #(rand-nth [1 2 3])))"
        ),
        "true"
    );
    // rn-class / rn-str: draws keep the coll's element kind.
    assert_eq!(class_of(&mut eval, "(rand-nth [1 2 3])"), "java.lang.Long");
    assert_eq!(
        class_of(&mut eval, "(rand-nth \"abc\")"),
        "java.lang.Character"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? #{\\a \\b \\c} (rand-nth \"abc\"))"),
        "true"
    );
    // rn-nil: `(nth nil 0)` is nil.
    assert_eq!(run_last(&mut eval, "(rand-nth nil)"), "nil");
    // rn-empty / rn-emptystr: the `nth` miss surfaces.
    assert_eq!(
        uncaught(&mut eval, "(rand-nth [])"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(rand-nth \"\")"),
        "java.lang.StringIndexOutOfBoundsException | Index 0 out of bounds for length 0"
    );
}

#[test]
fn test_var_rows() {
    let mut eval = boot();
    // tst-ok: the `:test` fn runs; its value is discarded.
    run_last(&mut eval, "(def s4a-sample (fn []))");
    run_last(
        &mut eval,
        "(alter-meta! (var s4a-sample) assoc :test (fn [] 42))",
    );
    assert_eq!(run_last(&mut eval, "(test (var s4a-sample))"), ":ok");
    // tst-missing / tst-map: no `:test` fn, no throw.
    run_last(&mut eval, "(def s4a-plain 1)");
    assert_eq!(run_last(&mut eval, "(test (var s4a-plain))"), ":no-test");
    assert_eq!(run_last(&mut eval, "(test {:a 1})"), ":no-test");
    // tst-throws: the fn's throw propagates (`ex-info`: the port
    // cannot build a bare `Exception` — `new` is out of scope).
    run_last(&mut eval, "(def s4a-boom (fn []))");
    run_last(
        &mut eval,
        "(alter-meta! (var s4a-boom) assoc :test (fn [] (throw (ex-info \"boom\" {}))))",
    );
    assert_eq!(
        uncaught(&mut eval, "(test (var s4a-boom))"),
        "clojure.lang.ExceptionInfo | boom"
    );
}

#[test]
fn arity_miss_rows() {
    let mut eval = boot();
    // The three natives throw the house arity text.
    for name in ["parse-boolean", "parse-long", "parse-double"] {
        for (src, count) in [
            (format!("({name})"), 0),
            (format!("({name} \"a\" \"b\")"), 2),
        ] {
            assert_eq!(
                uncaught(&mut eval, &src),
                format!(
                    "clojure.lang.ArityException | Wrong number of args ({count}) passed to: clojure.core/{name}"
                ),
                "{src}"
            );
        }
    }
    // `rand-nth` / `test` are 1-arity fns (the callee name carries
    // eval counters — prefix-pinned).
    for (src, count) in [
        ("(rand-nth)", 0),
        ("(rand-nth [1] [2])", 2),
        ("(test)", 0),
        ("(test 1 2)", 2),
    ] {
        let got = uncaught(&mut eval, src);
        assert!(
            got.starts_with(&format!(
                "clojure.lang.ArityException | Wrong number of args ({count}) passed to: "
            )),
            "{got}"
        );
    }
}
