// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S1 kind predicates: probe replay for `any?` / `delay?`
//! / `map-entry?` / `indexed?` / `NaN?` / `infinite?` / `nat-int?`
//! / `neg-int?` / `pos-int?` / `find-keyword` / `special-symbol?`
//! / `ffirst` (see
//! `docs/zero-oos-s1-predicates/probes/predicates.{clj,out}`).
//! Each test replays its probe labels verbatim, plus one shared
//! arity-miss test. The `int?`-boundary probes (`p-intp-*`) are
//! record-only: the port does not redefine `int?`.

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

#[test]
fn any_rows() {
    let mut eval = boot();
    // Oracle `p-any-*`: constantly true.
    for form in [
        "(any? nil)",
        "(any? false)",
        "(any? true)",
        "(any? 0)",
        "(any? \"\")",
        "(any? [])",
        "(any? {})",
        "(any? #{})",
        "(any? :k)",
        "(any? 's)",
        "(any? (delay 1))",
        "(any? inc)",
        "(defrecord S1Any [x]) (any? (S1Any. 1))",
        "(defmulti s1m :k) (defmethod s1m 1 [m] :one) (any? s1m)",
    ] {
        assert_eq!(run_last(&mut eval, form), "true", "{form}");
    }
}

#[test]
fn delay_rows() {
    let mut eval = boot();
    // Oracle `p-delay-*`: delays only (realized delays included).
    assert_eq!(run_last(&mut eval, "(delay? (delay 1))"), "true");
    assert_eq!(
        run_last(&mut eval, "(def s1d (delay 41)) (delay? s1d)"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(def s1d2 (delay 99)) (deref s1d2)"),
        "99"
    );
    assert_eq!(run_last(&mut eval, "(delay? s1d2)"), "true");
    for form in [
        "(delay? nil)",
        "(delay? 1)",
        "(delay? (future 1))",
        "(delay? (promise))",
        "(delay? (atom 1))",
        "(delay? inc)",
        "(delay? \"d\")",
        "(delay? [])",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn map_entry_rows() {
    let mut eval = boot();
    // Oracle `p-mapseq-*`: entries only, never 2-vectors.
    for form in [
        "(map-entry? (first {:a 1}))",
        "(map-entry? (find {:a 1} :a))",
        "(map-entry? (first {:a 1 :b 2}))",
    ] {
        assert_eq!(run_last(&mut eval, form), "true", "{form}");
    }
    for form in [
        "(map-entry? [1 2])",
        "(map-entry? [1 2 3])",
        "(map-entry? {:a 1})",
        "(map-entry? '(1 2))",
        "(map-entry? nil)",
        "(map-entry? \"ab\")",
        "(map-entry? 1)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn indexed_rows() {
    let mut eval = boot();
    // Oracle `p-indexed-*`: vectors, entries, subvectors.
    for form in [
        "(indexed? [1 2])",
        "(indexed? [])",
        "(indexed? (first {:a 1}))",
        "(indexed? (subvec [1 2 3] 0 2))",
    ] {
        assert_eq!(run_last(&mut eval, form), "true", "{form}");
    }
    // The port has no Array kind (oracle `p-indexed-objarr` /
    // `p-indexed-intarr` are false there too).
    for form in [
        "(indexed? \"ab\")",
        "(indexed? '(1 2))",
        "(indexed? (seq [1 2]))",
        "(indexed? (lazy-seq [1 2]))",
        "(indexed? {:a 1})",
        "(indexed? (sorted-map :a 1))",
        "(indexed? #{1})",
        "(indexed? (sorted-set 1))",
        "(defrecord S1Idx [x]) (indexed? (S1Idx. 1))",
        "(indexed? nil)",
        "(indexed? 1)",
        "(indexed? (delay 1))",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

/// The numbers NPE (`p-nan-nil`, `p-inf-nil`).
const DOUBLE_VALUE_NPE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null";

#[test]
fn nan_rows() {
    let mut eval = boot();
    // Oracle `p-nan-*`: NaN payloads only.
    assert_eq!(run_last(&mut eval, "(NaN? ##NaN)"), "true");
    for form in [
        "(NaN? ##Inf)",
        "(NaN? ##-Inf)",
        "(NaN? 0.0)",
        "(NaN? -0.0)",
        "(NaN? 1.5)",
        "(NaN? 1)",
        "(NaN? 5N)",
        "(NaN? 1/2)",
        "(NaN? 1.5M)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
    assert_eq!(uncaught(&mut eval, "(NaN? nil)"), DOUBLE_VALUE_NPE);
    for (form, want) in [
        (
            "(NaN? \"nan\")",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(NaN? true)",
            "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(NaN? [])",
            "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.lang.Number (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')",
        ),
        (
            "(NaN? \\a)",
            "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

#[test]
fn infinite_rows() {
    let mut eval = boot();
    // Oracle `p-inf-*`: infinite payloads only.
    assert_eq!(run_last(&mut eval, "(infinite? ##Inf)"), "true");
    assert_eq!(run_last(&mut eval, "(infinite? ##-Inf)"), "true");
    for form in [
        "(infinite? ##NaN)",
        "(infinite? 0.0)",
        "(infinite? 1.5)",
        "(infinite? 1)",
        "(infinite? 5N)",
        "(infinite? 1/2)",
        "(infinite? 1.5M)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
    assert_eq!(uncaught(&mut eval, "(infinite? nil)"), DOUBLE_VALUE_NPE);
    for (form, want) in [
        (
            "(infinite? \"inf\")",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(infinite? false)",
            "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(infinite? [])",
            "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.lang.Number (clojure.lang.PersistentVector is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')",
        ),
        (
            "(infinite? \\a)",
            "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

#[test]
fn nat_int_rows() {
    let mut eval = boot();
    // Oracle `p-nat-*`: `int?` and non-negative (BigInts out,
    // like the oracle — the port's `int?` is `Int`-only).
    for form in ["(nat-int? 0)", "(nat-int? 1)"] {
        assert_eq!(run_last(&mut eval, form), "true", "{form}");
    }
    for form in [
        "(nat-int? -1)",
        "(nat-int? 5N)",
        "(nat-int? -5N)",
        "(nat-int? 0N)",
        "(nat-int? 1.0)",
        "(nat-int? 1/2)",
        "(nat-int? 1M)",
        "(nat-int? 0M)",
        "(nat-int? nil)",
        "(nat-int? \"1\")",
        "(nat-int? true)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn neg_int_rows() {
    let mut eval = boot();
    // Oracle `p-neg-*`: `int?` and negative (0 is false).
    assert_eq!(run_last(&mut eval, "(neg-int? -1)"), "true");
    for form in [
        "(neg-int? 0)",
        "(neg-int? 1)",
        "(neg-int? 5N)",
        "(neg-int? -5N)",
        "(neg-int? -1.0)",
        "(neg-int? -1/2)",
        "(neg-int? -1M)",
        "(neg-int? nil)",
        "(neg-int? \"-1\")",
        "(neg-int? true)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn pos_int_rows() {
    let mut eval = boot();
    // Oracle `p-pos-*`: `int?` and positive (0 is false).
    assert_eq!(run_last(&mut eval, "(pos-int? 1)"), "true");
    for form in [
        "(pos-int? 0)",
        "(pos-int? -1)",
        "(pos-int? 5N)",
        "(pos-int? -5N)",
        "(pos-int? 1.0)",
        "(pos-int? 1/2)",
        "(pos-int? 1M)",
        "(pos-int? nil)",
        "(pos-int? \"1\")",
        "(pos-int? true)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn find_keyword_rows() {
    let mut eval = boot();
    // The port never interns keywords (value equality), so a
    // well-shaped lookup builds the keyword where the oracle
    // answers nil for never-interned names (`p-findkw-miss1/2`,
    // `p-findkw-2miss`, `p-findkw-symfresh`); the calls below are
    // still idempotent with no observable table growth.
    assert_eq!(
        run_last(&mut eval, "(find-keyword \"s1-never-here\")"),
        ":s1-never-here"
    );
    assert_eq!(
        run_last(&mut eval, "(find-keyword \"s1-never-here\")"),
        ":s1-never-here"
    );
    assert_eq!(
        run_last(&mut eval, "(find-keyword \"s1-ns-never\" \"s1-nm-never\")"),
        ":s1-ns-never/s1-nm-never"
    );
    assert_eq!(
        run_last(&mut eval, "(find-keyword 's1-symfresh)"),
        ":s1-symfresh"
    );
    assert_eq!(run_last(&mut eval, "(find-keyword 'a/b)"), ":a/b");
    // Oracle `p-findkw-plain` / `p-findkw-nsname` / `p-findkw-split` /
    // `p-findkw-kw` / `p-findkw-symknown` shapes replay.
    assert_eq!(run_last(&mut eval, "(find-keyword \"plain\")"), ":plain");
    assert_eq!(
        run_last(&mut eval, "(find-keyword \"ns\" \"nm\")"),
        ":ns/nm"
    );
    assert_eq!(run_last(&mut eval, "(find-keyword \"ns/nm\")"), ":ns/nm");
    assert_eq!(run_last(&mut eval, "(find-keyword :lit)"), ":lit");
    assert_eq!(
        run_last(&mut eval, "(do (keyword \"known\") (find-keyword 'known))"),
        ":known"
    );
    // Oracle `p-findkw-nil` / `p-findkw-num`: nil shapes stay nil.
    assert_eq!(run_last(&mut eval, "(find-keyword nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(find-keyword 5)"), "nil");
    // Oracle `p-findkw-2nilns`: a nil namespace means no namespace.
    assert_eq!(run_last(&mut eval, "(find-keyword nil \"a\")"), ":a");
    // Oracle `p-findkw-2badns` / `p-findkw-2badname`: `String` casts.
    for form in ["(find-keyword 5 \"a\")", "(find-keyword \"a\" 5)"] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')",
            "{form}"
        );
    }
    // Oracle `p-findkw-2nilname`: the `hashCode` NPE.
    assert_eq!(
        uncaught(&mut eval, "(find-keyword \"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.hashCode()\" because \"this.name\" is null"
    );
}

#[test]
fn special_symbol_rows() {
    let mut eval = boot();
    // Oracle `p-special-*`: the 23-member set, verbatim.
    for sym in [
        "def",
        "if",
        "do",
        "quote",
        "var",
        "fn*",
        "loop*",
        "recur",
        "throw",
        "try",
        "catch",
        "finally",
        "monitor-enter",
        "monitor-exit",
        ".",
        "new",
        "set!",
        "&",
        "let*",
        "case*",
        "deftype*",
        "reify*",
        "letfn*",
    ] {
        let form = format!("(special-symbol? '{sym})");
        assert_eq!(run_last(&mut eval, &form), "true", "{form}");
    }
    for form in [
        "(special-symbol? 'let)",
        "(special-symbol? 'fn)",
        "(special-symbol? 'loop)",
        "(special-symbol? 'case)",
        "(special-symbol? 'defn)",
        "(special-symbol? 'defmacro)",
        "(special-symbol? 'ns)",
        "(special-symbol? 'import)",
        "(special-symbol? 'import*)",
        "(special-symbol? 'deftype)",
        "(special-symbol? 'defrecord)",
        "(special-symbol? 'defrecord*)",
        "(special-symbol? 'reify)",
        "(special-symbol? 'else)",
        "(special-symbol? 'letfn)",
        "(special-symbol? 'defonce)",
        "(special-symbol? 'declare)",
        "(special-symbol? 'comment)",
        "(special-symbol? 'while)",
        "(special-symbol? '..)",
        "(special-symbol? 'user/def)",
        "(special-symbol? 'clojure.core/def)",
        "(special-symbol? \"def\")",
        "(special-symbol? :def)",
        "(special-symbol? nil)",
        "(special-symbol? 5)",
    ] {
        assert_eq!(run_last(&mut eval, form), "false", "{form}");
    }
}

#[test]
fn ffirst_rows() {
    let mut eval = boot();
    // Oracle `p-ffirst-*`: `(first (first x))`, throws included.
    for (form, want) in [
        ("(ffirst [[1 2] 3])", "1"),
        ("(ffirst [[[9]]])", "[9]"),
        ("(ffirst [])", "nil"),
        ("(ffirst nil)", "nil"),
        ("(ffirst [[]])", "nil"),
        ("(ffirst '((1 2) 3))", "1"),
        ("(ffirst {:a 1})", ":a"),
        ("(ffirst '([1] [2]))", "1"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    assert_eq!(
        uncaught(&mut eval, "(ffirst \"ab\")"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Character"
    );
    assert_eq!(
        uncaught(&mut eval, "(ffirst 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn predicate_arity_rows() {
    let mut eval = boot();
    // Oracle `p-*-ar0` / `p-*-ar2`: the house arity text.
    for name in [
        "any?",
        "delay?",
        "map-entry?",
        "indexed?",
        "NaN?",
        "infinite?",
        "nat-int?",
        "neg-int?",
        "pos-int?",
        "special-symbol?",
    ] {
        for form in [format!("({name})"), format!("({name} 1 2)")] {
            let argc = if form.ends_with(" 2)") { 2 } else { 0 };
            assert_eq!(
                uncaught(&mut eval, &form),
                format!(
                    "clojure.lang.ArityException | Wrong number of args ({argc}) passed to: clojure.core/{name}"
                ),
                "{form}"
            );
        }
    }
    for form in ["(find-keyword)", "(find-keyword \"a\" \"b\" \"c\")"] {
        let argc = if form == "(find-keyword)" { 0 } else { 3 };
        assert_eq!(
            uncaught(&mut eval, form),
            format!(
                "clojure.lang.ArityException | Wrong number of args ({argc}) passed to: clojure.core/find-keyword"
            ),
            "{form}"
        );
    }
    // `ffirst` is a `defn`: its display carries boot counters
    // (`user/evalN/fn--M`, like the oracle's `ffirst--N`), so only
    // the arity prefix pins.
    for form in ["(ffirst)", "(ffirst 1 2)"] {
        let argc = if form == "(ffirst)" { 0 } else { 2 };
        let got = uncaught(&mut eval, form);
        let want =
            format!("clojure.lang.ArityException | Wrong number of args ({argc}) passed to: ");
        assert!(got.starts_with(&want), "{form}: {got}");
    }
}
