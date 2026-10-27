// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S4b: probe replay for `definline` / `selector` /
//! `destructure` / `seq-to-map-for-destructuring` / `unquote` /
//! `unquote-splicing` / `munge` / `namespace-munge` /
//! `with-local-vars` (see
//! `docs/zero-oos-s4b-macros/probes/macros.{clj,out}`).
//! Expansion cases boot a fresh evaluator each (deterministic
//! `next_gensym` counters, the `fx4_macroexpand` `fresh`
//! pattern); only reader-issued `__auto__` ids and object hexes
//! normalize.

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

fn fresh(src: &str) -> String {
    run_last(&mut boot(), src)
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
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

/// The root cause's `Class | message` of an uncaught throw
/// (macroexpansion wrappers blame the site; the cause pins
/// the oracle text).
fn root_cause(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            let mut c = &t;
            while let Some(next) = c.cause.as_deref() {
                c = next;
            }
            format!(
                "{} | {}",
                c.class.jvm_name(),
                c.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

/// Normalize `__<digits>__auto__` (reader-issued, boot-order
/// dependent) to `__N__auto__`.
fn norm_auto(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("__auto__") {
        let (head, tail) = rest.split_at(i);
        let cut = head.rfind("__").map_or(head.len(), |j| {
            if j + 2 < head.len() && head[j + 2..].bytes().all(|b| b.is_ascii_digit()) {
                j
            } else {
                head.len()
            }
        });
        out.push_str(&head[..cut]);
        if cut < head.len() {
            out.push_str("__N__auto__");
        } else {
            out.push_str("__auto__");
        }
        rest = &tail["__auto__".len()..];
    }
    out.push_str(rest);
    out
}

/// Normalize `0x<hex>` object identities to `0xH`.
fn norm_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("0x") {
        let (head, tail) = rest.split_at(i);
        out.push_str(head);
        let hexlen = tail[2..]
            .bytes()
            .take_while(|b| b.is_ascii_hexdigit())
            .count();
        if hexlen == 0 {
            out.push_str("0x");
            rest = &tail[2..];
        } else {
            out.push_str("0xH");
            rest = &tail[2 + hexlen..];
        }
    }
    out.push_str(rest);
    out
}

#[test]
fn s4b_munge_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(munge \"a-b\")"), "\"a_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a/b\")"), "\"a_SLASH_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a.b\")"), "\"a.b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a:b\")"), "\"a_COLON_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a+b\")"), "\"a_PLUS_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a>b\")"), "\"a_GT_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a<b\")"), "\"a_LT_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a=b\")"), "\"a_EQ_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a~b\")"), "\"a_TILDE_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a!b\")"), "\"a_BANG_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a@b\")"), "\"a_CIRCA_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a#b\")"), "\"a_SHARP_b\"");
    assert_eq!(
        run_last(&mut eval, "(munge \"a'b\")"),
        "\"a_SINGLEQUOTE_b\""
    );
    assert_eq!(
        run_last(&mut eval, "(munge \"a\\\"b\")"),
        "\"a_DOUBLEQUOTE_b\""
    );
    assert_eq!(run_last(&mut eval, "(munge \"a%b\")"), "\"a_PERCENT_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a^b\")"), "\"a_CARET_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a&b\")"), "\"a_AMPERSAND_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a*b\")"), "\"a_STAR_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a|b\")"), "\"a_BAR_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a{b\")"), "\"a_LBRACE_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a}b\")"), "\"a_RBRACE_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a[b\")"), "\"a_LBRACK_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a]b\")"), "\"a_RBRACK_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a?b\")"), "\"a_QMARK_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a\\\\b\")"), "\"a_BSLASH_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a,b\")"), "\"a,b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a;b\")"), "\"a;b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a(b\")"), "\"a(b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a)b\")"), "\"a)b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a$b\")"), "\"a$b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a_b\")"), "\"a_b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a b\")"), "\"a b\"");
    assert_eq!(run_last(&mut eval, "(munge \"a\nb\")"), "\"a\\nb\"");
    assert_eq!(run_last(&mut eval, "(munge \"a`b\")"), "\"a`b\"");
    assert_eq!(run_last(&mut eval, "(munge \"\")"), "\"\"");
    assert_eq!(
        run_last(&mut eval, "(munge \"abcXYZ019\")"),
        "\"abcXYZ019\""
    );
    assert_eq!(
        run_last(&mut eval, "(munge \"--//..\")"),
        "\"___SLASH__SLASH_..\""
    );
    assert_eq!(run_last(&mut eval, "(munge \"aéb中😀\")"), "\"aéb中😀\"");
    assert_eq!(run_last(&mut eval, "(munge nil)"), "\"\"");
    assert_eq!(run_last(&mut eval, "(munge 123)"), "\"123\"");
    assert_eq!(run_last(&mut eval, "(munge :a/b)"), "\"_COLON_a_SLASH_b\"");
    assert_eq!(run_last(&mut eval, "(munge (quote a/b))"), "a_SLASH_b");
    assert_eq!(run_last(&mut eval, "(symbol? (munge (quote foo)))"), "true");
    assert_eq!(run_last(&mut eval, "(string? (munge \"foo\"))"), "true");
    assert_eq!(
        run_last(&mut eval, "(munge [1 2])"),
        "\"_LBRACK_1 2_RBRACK_\""
    );
    assert_eq!(run_last(&mut eval, "(munge \\c)"), "\"c\"");
}

#[test]
fn s4b_namespace_munge_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(namespace-munge \"my.ns-name\")"),
        "\"my.ns_name\""
    );
    assert_eq!(
        run_last(&mut eval, "(namespace-munge \"plain\")"),
        "\"plain\""
    );
    assert_eq!(
        run_last(&mut eval, "(namespace-munge \"a-b-c\")"),
        "\"a_b_c\""
    );
    assert_eq!(
        run_last(&mut eval, "(namespace-munge \"a/b:c+d\")"),
        "\"a/b:c+d\""
    );
    assert_eq!(run_last(&mut eval, "(namespace-munge \"\")"), "\"\"");
    assert_eq!(run_last(&mut eval, "(namespace-munge nil)"), "\"\"");
    assert_eq!(
        run_last(&mut eval, "(namespace-munge (quote my-ns))"),
        "\"my_ns\""
    );
    assert_eq!(
        run_last(&mut eval, "(namespace-munge :my-kw)"),
        "\":my_kw\""
    );
    assert_eq!(run_last(&mut eval, "(namespace-munge 42)"), "\"42\"");
    assert_eq!(
        run_last(&mut eval, "(string? (namespace-munge \"a\"))"),
        "true"
    );
}

#[test]
fn s4b_destructure_rows() {
    let mut eval = boot();
    // Passthroughs (return the input unchanged).
    assert_eq!(
        run_last(&mut eval, "(destructure (quote [a 1 b 2]))"),
        "[a 1 b 2]"
    );
    assert_eq!(run_last(&mut eval, "(destructure [])"), "[]");
    assert_eq!(run_last(&mut eval, "(destructure (quote [a]))"), "[a]");
    assert_eq!(
        run_last(&mut eval, "(destructure (quote [a 1 b]))"),
        "[a 1 b]"
    );
    assert_eq!(run_last(&mut eval, "(destructure (quote (a 1)))"), "(a 1)");
    assert_eq!(run_last(&mut eval, "(destructure nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(destructure {:a 1})"), "{:a 1}");
    // Expansions (fresh boot each: exact gensym pins).
    assert_eq!(
        fresh("(destructure (quote [[a] 1]))"),
        "[vec__1 1 a (clojure.core/nth vec__1 0 nil)]"
    );
    assert_eq!(
        fresh("(destructure (quote [{a :x} m]))"),
        "[map__1 m map__1 (if (clojure.core/instance? clojure.lang.ISeq map__1) \
         (if (clojure.core/next map__1) \
         (clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array map__1)) \
         (if (clojure.core/seq map__1) (clojure.core/first map__1) \
         clojure.lang.PersistentArrayMap/EMPTY)) map__1) \
         a (clojure.core/get map__1 :x)]"
    );
    assert_eq!(
        fresh("(destructure (quote [{:keys [a]} m]))"),
        "[map__1 m map__1 (if (clojure.core/instance? clojure.lang.ISeq map__1) \
         (if (clojure.core/next map__1) \
         (clojure.lang.PersistentArrayMap/createAsIfByAssoc (clojure.core/to-array map__1)) \
         (if (clojure.core/seq map__1) (clojure.core/first map__1) \
         clojure.lang.PersistentArrayMap/EMPTY)) map__1) \
         a (clojure.core/get map__1 :a)]"
    );
    assert_eq!(
        fresh("(destructure (quote [[a & r] v]))"),
        "[vec__1 v seq__2 (clojure.core/seq vec__1) \
         first__3 (clojure.core/first seq__2) seq__2 (clojure.core/next seq__2) \
         a first__3 r seq__2]"
    );
    assert_eq!(
        fresh("(destructure (quote [[a :as all] v]))"),
        "[vec__1 v a (clojure.core/nth vec__1 0 nil) all vec__1]"
    );
    assert_eq!(
        fresh("(destructure (quote [[a [b]] 1]))"),
        "[vec__1 1 a (clojure.core/nth vec__1 0 nil) \
         vec__4 (clojure.core/nth vec__1 1 nil) b (clojure.core/nth vec__4 0 nil)]"
    );
    assert_eq!(
        fresh("(destructure [(first {(quote a) (quote b)}) 5])"),
        "[vec__1 5 a (clojure.core/nth vec__1 0 nil) b (clojure.core/nth vec__1 1 nil)]"
    );
    // Throws.
    assert_eq!(
        uncaught(&mut eval, "(destructure (quote ((a) 1)))"),
        "java.lang.Exception | Unsupported binding form: (a)"
    );
    assert_eq!(
        uncaught(&mut eval, "(destructure \"ab\")"),
        "java.lang.Exception | Unsupported binding form: a"
    );
    assert_eq!(
        uncaught(&mut eval, "(destructure :kw)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(destructure 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(destructure (quote [5 1]))"),
        "java.lang.Exception | Unsupported binding form: 5"
    );
    // Entries destructure as 2-vectors (like the JVM).
    assert_eq!(
        uncaught(&mut eval, "(destructure {:a 1 :b 2})"),
        "java.lang.Exception | Unsupported binding form: :a"
    );
    assert_eq!(
        uncaught(&mut eval, "(destructure [(first {:a 1}) 5])"),
        "java.lang.Exception | Unsupported binding form: :a"
    );
}

#[test]
fn s4b_seq_to_map_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [])"),
        "{}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring nil)"),
        "{}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [5])"),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [\"a\"])"),
        "\"a\""
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [{:a 1}])"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [:a])"),
        ":a"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [\"a\" 1])"),
        "{\"a\" 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(seq-to-map-for-destructuring [\"a\" 1 \"b\" 2])"
        ),
        "{\"a\" 1, \"b\" 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(seq-to-map-for-destructuring [\"a\" 1 \"a\" 2])"
        ),
        "{\"a\" 2}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring (quote (:a 1)))"),
        "{:a 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring \"ab\")"),
        "{\\a \\b}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring #{1})"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring {:a 1 :b 2})"),
        "{[:a 1] [:b 2]}"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring [1 2 3])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // Odd 3+: pairs plus the tail conj (complex path).
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [1 2 [:a 3]])"),
        "{1 2, :a 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [1 2 {:a 3}])"),
        "{1 2, :a 3}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(seq-to-map-for-destructuring [1 2 (first {:a 3})])"
        ),
        "{1 2, :a 3}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [1 2 nil])"),
        "{1 2}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [[:a 1] [:b 2]])"),
        "{[:a 1] [:b 2]}"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [[:a 1]])"),
        "[:a 1]"
    );
    assert_eq!(
        run_last(&mut eval, "(seq-to-map-for-destructuring [1 2 3 4 [:k 9]])"),
        "{1 2, 3 4, :k 9}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(seq-to-map-for-destructuring [1 2 (list (first {:a 3}))])"
        ),
        "{1 2, :a 3}"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring [1 2 [1 2 3]])"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring [1 2 [1]])"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring [1 2 \"abc\"])"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class \
         java.util.Map$Entry (java.lang.Character and java.util.Map$Entry are in module \
         java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring [1 2 :k])"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring :kw)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: clojure.lang.Keyword"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq-to-map-for-destructuring 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
}

#[test]
fn s4b_definline_rows() {
    // Expansion (fresh boot; byte-identical to the oracle shape).
    assert_eq!(
        fresh("(macroexpand-1 (quote (definline foo [x] `(inc ~x))))"),
        "(do (clojure.core/defn foo [x] (clojure.core/inc x)) \
         (clojure.core/alter-meta! (var foo) clojure.core/assoc :inline \
         (clojure.core/fn foo [x] (clojure.core/seq (clojure.core/concat \
         (clojure.core/list (quote clojure.core/inc)) (clojure.core/list x))))) \
         (var foo))"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(definline di-foo [x] `(inc ~x))"),
        "#'user/di-foo"
    );
    assert_eq!(run_last(&mut eval, "(di-foo 5)"), "6");
    assert_eq!(
        run_last(&mut eval, "(fn? (:inline (meta (var di-foo))))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "((:inline (meta (var di-foo))) 5)"),
        "(clojure.core/inc 5)"
    );
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta (var di-foo)))"),
        "([x])"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (definline di-ret [x] `(inc ~x)) (var di-ret))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(definline di-doc \"doc\" {:added \"9.9\"} [x] `(inc ~x))"
        ),
        "#'user/di-doc"
    );
    assert_eq!(run_last(&mut eval, "(:doc (meta (var di-doc)))"), "\"doc\"");
    // Malformed decls and non-template bodies fail at expansion
    // (the phase matches; the texts are the port's own `fn` /
    // arithmetic machinery, S4/S5D-owned).
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 (quote (definline badb 123)))"),
        "clojure.lang.Compiler$CompilerException | Unexpected error macroexpanding definline \
         at (user:1:1).\n\
         clojure.lang.Compiler$CompilerException | Syntax error macroexpanding clojure.core/fn \
         at (user:1:1)."
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(macroexpand-1 (quote (definline badt [x] (inc x))))"
        ),
        "java.lang.ClassCastException | Unexpected error macroexpanding definline at (user:1:1).\n\
         java.lang.ClassCastException | class clojure.lang.Symbol cannot be cast to class \
         java.lang.Number (clojure.lang.Symbol is in unnamed module of loader 'app'; \
         java.lang.Number is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 (quote (definline)))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/definline"
    );
    assert_eq!(
        run_last(&mut eval, "(:macro (meta (var definline)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta (var definline)))"),
        "([name & decl])"
    );
}

#[test]
fn s4b_selector_rows() {
    // Expansions (fresh boot each; only `__auto__` normalizes).
    assert_eq!(
        norm_auto(&fresh("(macroexpand-1 (quote (selector {:select [:a]})))")),
        "(clojure.core/fn selector3 [map__N__auto__] \
         (clojure.core/let [{:select G__1} map__N__auto__] G__1))"
    );
    assert_eq!(
        norm_auto(&fresh(
            "(macroexpand-1 (quote (selector {:select s :missing m})))"
        )),
        "(clojure.core/fn selector4 [map__N__auto__] \
         (clojure.core/let [{:select G__1, :missing G__2} map__N__auto__] \
         (clojure.core/some-vals {:select G__1, :missing G__2})))"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(fn? (eval (quote (selector {:select s :keys [a]}))))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:keys [a] :select s}))) {:a 1 :b 2})"
        ),
        "{:a 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:keys [a] :all al}))) {:a 1 :b 2})"
        ),
        "{:a 1, :b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:keys [a] :excess e}))) {:a 1 :b 2})"
        ),
        "{:b 2}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:keys [a] :select s :excess e}))) {:a 1 :b 2})"
        ),
        "{:select {:a 1}, :excess {:b 2}}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:select s}))) {:a 1 :b 2})"
        ),
        "{}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((eval (quote (selector {:keys [a] :missing m}))) {:a 1})"
        ),
        "nil"
    );
    assert_eq!(
        root_cause(&mut eval, "(eval (quote (selector [1 2])))"),
        "java.lang.IllegalArgumentException | expected a map"
    );
    assert_eq!(
        root_cause(&mut eval, "(eval (quote (selector {:a 1})))"),
        "java.lang.IllegalArgumentException | form must contain at least one of :select :excess :missing :all"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 (quote (selector)))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/selector"
    );
    assert_eq!(
        run_last(&mut eval, "(:macro (meta (var selector)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta (var selector)))"),
        "([m])"
    );
}

#[test]
fn s4b_unquote_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(resolve (quote unquote))"),
        "#'clojure.core/unquote"
    );
    assert_eq!(
        run_last(&mut eval, "(resolve (quote unquote-splicing))"),
        "#'clojure.core/unquote-splicing"
    );
    assert_eq!(run_last(&mut eval, "(bound? (var unquote))"), "false");
    assert_eq!(
        run_last(&mut eval, "(bound? (var unquote-splicing))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(select-keys (meta (var unquote)) [:name])"),
        "{:name unquote}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(nil? (:declared (meta (var unquote-splicing))))"
        ),
        "true"
    );
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(var-get (var unquote))")),
        "#object[clojure.lang.Var$Unbound 0xH \"Unbound: #'clojure.core/unquote\"]"
    );
    assert_eq!(
        uncaught(&mut eval, "(eval (quote (clojure.core/unquote 1)))"),
        "java.lang.IllegalStateException | Attempting to call unbound fn: #'clojure.core/unquote"
    );
}

#[test]
fn s4b_with_local_vars_rows() {
    // Expansion (fresh boot; the interop-free port shape).
    assert_eq!(
        fresh("(macroexpand-1 (quote (with-local-vars [a 1] (var-get a))))"),
        "(clojure.core/let [a (clojure.core/__local-var*)] \
         (clojure.core/push-thread-bindings (clojure.core/hash-map a 1)) \
         (try (var-get a) (finally (clojure.core/pop-thread-bindings))))"
    );
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(with-local-vars [a 1 b 2] [(var-get a) (var-get b) (thread-bound? a)])"
        ),
        "[1 2 true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-local-vars [a 1] (var-set a 5) (var-get a))"
        ),
        "5"
    );
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1] :a :done)"),
        ":done"
    );
    assert_eq!(run_last(&mut eval, "(with-local-vars [] 42)"), "42");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-local-vars [a 1] [(var-get a) (with-local-vars [a 2] (var-get a)) (var-get a)])"
        ),
        "[1 2 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(with-local-vars [a 1] (var-set a 9) (var-get a)) (with-local-vars [a 1] (var-get a))]"
        ),
        "[9 1]"
    );
    run_last(&mut eval, "(def wv-g nil)");
    assert_eq!(
        run_last(
            &mut eval,
            "(do (with-local-vars [a 1] (def wv-g a) :in) (thread-bound? wv-g))"
        ),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(resolve (quote a))"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-local-vars [a 1] (def wv-g a) (throw (Exception. \"x\"))) \
             (catch Exception e :caught))"
        ),
        ":caught"
    );
    assert_eq!(run_last(&mut eval, "(thread-bound? wv-g)"), "false");
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(var-get wv-g)")),
        "#object[clojure.lang.Var$Unbound 0xH \"Unbound: #<Var: --unnamed-->\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1] (meta a))"),
        "{:ns nil, :name nil}"
    );
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1] (str a))"),
        "\"#<Var: --unnamed-->\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1] (pr-str a))"),
        "\"#<Var: --unnamed-->\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1] [(var? a) (bound? a)])"),
        "[true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-local-vars [a 1] (alter-meta! a assoc :tag :x) (:tag (meta a)))"
        ),
        ":x"
    );
    // Inits evaluate before the push (the unbound sentinel flows,
    // like the JVM — no throw on either side).
    assert_eq!(
        run_last(&mut eval, "(with-local-vars [a 1 b (var-get a)] :never)"),
        ":never"
    );
    assert_eq!(
        root_cause(
            &mut eval,
            "(eval (with-meta (list (quote with-local-vars) (quote a) 1) {:line 7}))"
        ),
        "java.lang.IllegalArgumentException | with-local-vars requires a vector for its binding in user:7"
    );
    assert_eq!(
        root_cause(
            &mut eval,
            "(eval (with-meta (list (quote with-local-vars) [(quote a)]) {:line 7}))"
        ),
        "java.lang.IllegalArgumentException | with-local-vars requires an even number of forms in binding vector in user:7"
    );
    assert_eq!(
        run_last(&mut eval, "(:macro (meta (var with-local-vars)))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(:arglists (meta (var with-local-vars)))"),
        "([name-vals-vec & body])"
    );
}

#[test]
fn s4b_native_arities() {
    let mut eval = boot();
    for (src, want, argc) in [
        ("(munge)", "clojure.core/munge", 0),
        ("(munge \"a\" \"b\")", "clojure.core/munge", 2),
        ("(namespace-munge)", "clojure.core/namespace-munge", 0),
        (
            "(namespace-munge \"a\" \"b\")",
            "clojure.core/namespace-munge",
            2,
        ),
        ("(destructure)", "clojure.core/destructure", 0),
        ("(destructure [] [])", "clojure.core/destructure", 2),
        (
            "(seq-to-map-for-destructuring)",
            "clojure.core/seq-to-map-for-destructuring",
            0,
        ),
        (
            "(seq-to-map-for-destructuring [] [])",
            "clojure.core/seq-to-map-for-destructuring",
            2,
        ),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!(
                "clojure.lang.ArityException | Wrong number of args ({argc}) passed to: {want}"
            )
        );
    }
}
