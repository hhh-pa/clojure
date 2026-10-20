// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D `str`/`subs`: every corpus-11 row exercising `str`/`subs`,
//! the corpus-17 `subs` edges, and the LazySeq `@hash` / seq-content
//! pins (corpus 12/19 spot values). The infinite-lazy hang is
//! deliberately untested (it hangs by design, like the JVM).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

/// Eval one form, expecting a string result; return its contents.
fn eval_str(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    assert_eq!(vs.len(), 1, "expected one value for {src}");
    match &vs[0] {
        Value::Str(s) => s.to_string(),
        other => panic!("expected a string for {src}, got: {}", pr(other)),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw for {src}, got {other}"),
        Ok(_) => panic!("expected an uncaught throw for {src}"),
    }
}

const CHARVALUE_NPE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null";
const NUM_CCE: &str = "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')";

#[test]
fn str_basics_corpus_11() {
    let mut eval = boot();
    assert_eq!(eval_str(&mut eval, "(str nil)"), "");
    assert_eq!(eval_str(&mut eval, "(str nil nil)"), "");
    assert_eq!(eval_str(&mut eval, "(str :a/b \"x\" nil 5)"), ":a/bx5");
    assert_eq!(eval_str(&mut eval, "(str)"), "");
    assert_eq!(
        eval_str(&mut eval, "(str 1 :a \"b\" [1] {:k 1})"),
        "1:ab[1]{:k 1}"
    );
    assert_eq!(eval_str(&mut eval, "(str \\a)"), "a");
    assert_eq!(eval_str(&mut eval, "(eval (list 'str))"), "");
}

#[test]
fn str_nested_corpus_11() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(vector (str [\"a\"]) (str [\"a\" 1] {:k \"v\"}) (str 'sym \\c))"
        ),
        r#"["[\"a\"]" "[\"a\" 1]{:k \"v\"}" "symc"]"#,
    );
}

#[test]
fn str_lazy_content_hash() {
    let mut eval = boot();
    // Pending lazies: exact JVM `List` content hashes (pins + corpus 12/19).
    assert_eq!(
        eval_str(&mut eval, "(str (map inc [1 2]))"),
        "clojure.lang.LazySeq@402"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (map inc [1]))"),
        "clojure.lang.LazySeq@21"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (filter odd? [1]))"),
        "clojure.lang.LazySeq@20"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (map inc []))"),
        "clojure.lang.LazySeq@1"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (drop 0 '()))"),
        "clojure.lang.LazySeq@1"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (take 2 (cycle [1])))"),
        "clojure.lang.LazySeq@3e1"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (drop-last 1 [1 2]))"),
        "clojure.lang.LazySeq@20"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (drop 1 (keys {:a 1 :b 2})))"),
        "clojure.lang.LazySeq@3c6f0c0b"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (drop 2.5 \"abcde\"))"),
        "clojure.lang.LazySeq@1042"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (drop 1 \"abc\"))"),
        "clojure.lang.LazySeq@1002"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (take 2.5 [1 2 3 4]))"),
        "clojure.lang.LazySeq@7861"
    );
    assert_eq!(
        eval_str(&mut eval, "(str (take 0.5 [1 2 3 4]))"),
        "clojure.lang.LazySeq@20"
    );
    // A forced lazy is still a `LazySeq` (never contents).
    assert_eq!(
        eval_str(&mut eval, "(let [s (map inc [1])] (doall s) (str s))"),
        "clojure.lang.LazySeq@21"
    );
    // The hash walk realizes (both elements forced, like the JVM
    // counter pin; atoms don't exist here, so `print` counts).
    assert_eq!(
        eval_str(
            &mut eval,
            "(with-out-str (print (str (map (fn [x] (print \"E\") x) '(1 2)))))"
        ),
        "EEclojure.lang.LazySeq@3e2"
    );
    // Force errors surface through the hash walk.
    assert_eq!(
        uncaught(
            &mut eval,
            "(str (map (fn [x] (if (= x 2) (throw (ex-info \"boom\" {})) x)) [1 2]))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
}

#[test]
fn str_seq_contents_corpus_19() {
    let mut eval = boot();
    assert_eq!(eval_str(&mut eval, "(str (keys {:a 1}))"), "(:a)");
    assert_eq!(eval_str(&mut eval, "(str (seq [1 2]))"), "(1 2)");
    assert_eq!(eval_str(&mut eval, "(str (seq \"ab\"))"), "(\\a \\b)");
    assert_eq!(eval_str(&mut eval, "(str [1 2])"), "[1 2]");
    assert_eq!(eval_str(&mut eval, "(str '(1 2))"), "(1 2)");
    assert_eq!(eval_str(&mut eval, "(str {:a 1})"), "{:a 1}");
    assert_eq!(eval_str(&mut eval, "(str #{1})"), "#{1}");
    assert_eq!(eval_str(&mut eval, "(str (range 3))"), "(0 1 2)");
    assert_eq!(eval_str(&mut eval, "(str (repeat 2 :x))"), "(:x :x)");
    assert_eq!(eval_str(&mut eval, "(str (cons 1 [2]))"), "(1 2)");
    assert_eq!(eval_str(&mut eval, "(str (sort [2 1]))"), "(1 2)");
    assert_eq!(eval_str(&mut eval, "(str [\"a\" :b])"), "[\"a\" :b]");
    // `print-str` is in the contents-printing family; `str` is not.
    assert_eq!(eval_str(&mut eval, "(print-str (map inc [1]))"), "(2)");
}

#[test]
fn subs_basics_corpus_11() {
    let mut eval = boot();
    assert_eq!(eval_str(&mut eval, "(subs \"hello\" 1 3)"), "el");
    assert_eq!(eval_str(&mut eval, "(subs \"hello\" 1)"), "ello");
    assert_eq!(eval_str(&mut eval, "(subs \"hello\" 5)"), "");
    assert_eq!(eval_str(&mut eval, "(subs \"\" 0)"), "");
    assert_eq!(eval_str(&mut eval, "(subs \"ab\" 1.5)"), "b");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 0 1.5)"), "a");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 1)"), "bc");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 1 2)"), "b");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 3)"), "");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 0 0)"), "");
}

#[test]
fn subs_oob_corpus_11() {
    let mut eval = boot();
    let sioobe = "java.lang.StringIndexOutOfBoundsException | ";
    assert_eq!(
        uncaught(&mut eval, "(subs \"hello\" 6)"),
        format!("{sioobe}Range [6, 5) out of bounds for length 5")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"hello\" 3 1)"),
        format!("{sioobe}Range [3, 1) out of bounds for length 5")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"hello\" -1 2)"),
        format!("{sioobe}Range [-1, 2) out of bounds for length 5")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" -1)"),
        format!("{sioobe}Range [-1, 3) out of bounds for length 3")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" 5)"),
        format!("{sioobe}Range [5, 3) out of bounds for length 3")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" 0 5)"),
        format!("{sioobe}Range [0, 5) out of bounds for length 3")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" 1 5)"),
        format!("{sioobe}Range [1, 5) out of bounds for length 3")
    );
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" 2 1)"),
        format!("{sioobe}Range [2, 1) out of bounds for length 3")
    );
}

#[test]
fn subs_nil_and_index_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(subs nil 0)"),
        "java.lang.NullPointerException | Cannot invoke \"String.substring(int)\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(subs nil 0 1)"),
        "java.lang.NullPointerException | Cannot invoke \"String.substring(int, int)\" because \"s\" is null"
    );
    // A nil index trips the `nth`-family `charValue` NPE (corpus 11/17).
    assert_eq!(uncaught(&mut eval, "(subs \"ab\" nil)"), CHARVALUE_NPE);
    assert_eq!(uncaught(&mut eval, "(subs \"abc\" nil 1)"), CHARVALUE_NPE);
    assert_eq!(uncaught(&mut eval, "(subs \"ab\" \"a\")"), NUM_CCE);
    assert_eq!(uncaught(&mut eval, "(subs \"abc\" \"a\" 1)"), NUM_CCE);
    assert_eq!(
        uncaught(&mut eval, "(subs \"abc\" 0 9223372036854775807)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'subs))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/subs"
    );
}

#[test]
fn subs_truncates_double_index() {
    let mut eval = boot();
    // The truncate family (`nth`/`subvec`/`subs`): 1.5 and 1.9 both
    // weaken to 1 (corpus 11/17 pin 1.5; 1.9 is only pinned for `nth`).
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 1.5)"), "bc");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 1.9)"), "bc");
    assert_eq!(eval_str(&mut eval, "(subs \"abc\" 0 2.9)"), "ab");
}

#[test]
fn print_opts_rows() {
    let mut eval = boot();
    // `*print-length*`: negatives, fractionals, and NaN are
    // unlimited; integral doubles act as that int (fixprobe9).
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 2] (pr-str [1 2 3]))"),
        "\"[1 2 ...]\""
    );
    for n in ["-1", "2.5", "##NaN", "-2.0"] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(binding [*print-length* {n}] (pr-str [1 2 3]))")
            ),
            "\"[1 2 3]\"",
            "{n}"
        );
    }
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 0] (pr-str [1]))"),
        "\"[...]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-length* 2.0] (pr-str [1 2 3]))"),
        "\"[1 2 ...]\""
    );
    // `*print-level*`: dec-first, non-positive prints `#`.
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 1] (pr-str [[1]]))"),
        "\"[#]\""
    );
    assert_eq!(
        run_last(&mut eval, "(binding [*print-level* 2] (pr-str [[1]]))"),
        "\"[[1]]\""
    );
    for n in ["0", "-1"] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(binding [*print-level* {n}] (pr-str [1]))")
            ),
            "\"#\"",
            "{n}"
        );
    }
    for n in ["2.5", "##NaN"] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(binding [*print-level* {n}] (pr-str [[1]]))")
            ),
            "\"[[1]]\"",
            "{n}"
        );
    }
    // `*print-readably*` (via `pr`) and `*print-meta*` are honored.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-readably* false] (pr-str \"a\"))"
        ),
        "\"a\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta [1] {:a 1})))"
        ),
        "\"^{:a 1} [1]\""
    );
    // `*print-namespace-maps*` gates the lift (root true).
    assert_eq!(
        run_last(&mut eval, "(pr-str {:a/x 1 :a/y 2})"),
        "\"#:a{:x 1, :y 2}\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-namespace-maps* false] (pr-str {:a/x 1 :a/y 2}))"
        ),
        "\"{:a/x 1, :a/y 2}\""
    );
}

#[test]
fn core_r4_subs_nonstring() {
    let mut eval = boot();
    for src in ["(subs 5 0)", "(subs 5 0 1)"] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')",
            "{src}"
        );
    }
}

#[test]
fn core_r4_print_limit_cce() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(binding [*print-length* \"s\"] (pr-str [1]))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
}
