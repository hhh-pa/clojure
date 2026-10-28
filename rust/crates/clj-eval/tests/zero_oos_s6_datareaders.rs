// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S6 data readers: probe replay for `*data-readers*` /
//! `*default-data-reader-fn*` / `default-data-readers` /
//! `reader-conditional` / `reader-conditional?` / `tagged-literal` /
//! `tagged-literal?` plus the tag-table read path (see
//! `docs/zero-oos-s6-datareaders/probes/*.{clj,out}`). Labels name
//! the replayed pins (`d-*` in `datareaders.out`, the rest in
//! `tagreads.out`).

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
            panic!("expected an uncaught throw, got values")
        }
    }
}

#[test]
fn data_reader_vars() {
    let mut eval = boot();
    // `a-roots`: the table starts empty, the default fn nil.
    assert_eq!(run_last(&mut eval, "(pr-str @#'*data-readers*)"), "\"{}\"");
    assert_eq!(
        run_last(&mut eval, "(pr-str @#'*default-data-reader-fn*)"),
        "\"nil\""
    );
    // `a-meta`: both `^:dynamic`.
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*data-readers*))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*default-data-reader-fn*))"),
        "true"
    );
    // `a-ns`: both live in `clojure.core` (the port prints
    // namespaces as `#object`, so read the name).
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (ns-name (:ns (meta #'*data-readers*))))"
        ),
        "\"clojure.core\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str (ns-name (:ns (meta #'*default-data-reader-fn*))))"
        ),
        "\"clojure.core\""
    );
}

#[test]
fn default_data_readers_table() {
    let mut eval = boot();
    // `a-defaults*`: the map shape (keys, all fns, count).
    assert_eq!(
        run_last(&mut eval, "(sort (map str (keys default-data-readers)))"),
        "(\"inst\" \"uuid\")"
    );
    assert_eq!(
        run_last(&mut eval, "(every? ifn? (vals default-data-readers))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(count default-data-readers)"), "2");
    assert_eq!(
        run_last(&mut eval, "(every? symbol? (keys default-data-readers))"),
        "true"
    );
    // `a-defaults-arity`: a map, so lookup position applies.
    assert_eq!(run_last(&mut eval, "(default-data-readers 1)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(ifn? (get default-data-readers 'inst))"),
        "true"
    );
    // `a-ddr-meta`: plain (neither dynamic nor private).
    assert_eq!(
        run_last(
            &mut eval,
            "(pr-str [(:dynamic (meta #'default-data-readers)) (:private (meta #'default-data-readers))])"
        ),
        "\"[nil nil]\""
    );
}

#[test]
fn tagged_literal_ctors() {
    let mut eval = boot();
    // `b-ctor*`: readable construction (qualified tags print too).
    assert_eq!(run_last(&mut eval, "(tagged-literal 'foo 1)"), "#foo 1");
    assert_eq!(
        run_last(&mut eval, "(tagged-literal 'a [1 {:b 2}])"),
        "#a [1 {:b 2}]"
    );
    assert_eq!(
        run_last(&mut eval, "(tagged-literal 'foo/bar 1)"),
        "#foo/bar 1"
    );
    // `b-ctor-niltag`: nil coerces to `nil` (probe `b-niltag-eq`).
    assert_eq!(run_last(&mut eval, "(tagged-literal nil 1)"), "#nil 1");
    assert_eq!(
        run_last(
            &mut eval,
            "(= (tagged-literal nil 1) (tagged-literal 'nil 1))"
        ),
        "true"
    );
    // `b-ctor-strtag` / `b-ctor-kwtag`: the `Symbol` cast.
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal \"x\" 1)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Symbol (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Symbol is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal :k 1)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.Symbol (clojure.lang.Keyword and clojure.lang.Symbol are in unnamed module of loader 'app')"
    );
    // `b-arity*`: the house arity text.
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal 'x)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/tagged-literal"
    );
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/tagged-literal"
    );
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal 'x 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/tagged-literal"
    );
    // `b-eq` / `b-ne-*`: value equality over tag and form.
    assert_eq!(
        run_last(&mut eval, "(= (tagged-literal 'a 1) (tagged-literal 'a 1))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (tagged-literal 'a 1) (tagged-literal 'b 1))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(= (tagged-literal 'a 1) (tagged-literal 'a 2))"),
        "false"
    );
    // `b-str`: the port prints readably (the oracle records the JVM
    // `@hex` `toString`; print dispatch is not S6).
    assert_eq!(
        run_last(&mut eval, "(str (tagged-literal 'a 1))"),
        "\"#a 1\""
    );
}

#[test]
fn tagged_literal_predicate() {
    let mut eval = boot();
    // Battery `b-p-*`: only tagged literals answer true.
    for (src, want) in [
        ("(tagged-literal? (tagged-literal 'a 1))", "true"),
        ("(tagged-literal? nil)", "false"),
        ("(tagged-literal? true)", "false"),
        ("(tagged-literal? 1)", "false"),
        ("(tagged-literal? \"x\")", "false"),
        ("(tagged-literal? 's)", "false"),
        ("(tagged-literal? :k)", "false"),
        ("(tagged-literal? [1])", "false"),
        ("(tagged-literal? {:a 1})", "false"),
        ("(tagged-literal? (list 1))", "false"),
        ("(tagged-literal? (fn [x] x))", "false"),
        ("(tagged-literal? (atom 1))", "false"),
        ("(tagged-literal? (reader-conditional '(1) false))", "false"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/tagged-literal?"
    );
    assert_eq!(
        uncaught(&mut eval, "(tagged-literal? 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/tagged-literal?"
    );
}

#[test]
fn reader_conditional_ctors() {
    let mut eval = boot();
    // `c-ctor*`: readable construction, splicing prints `#?@`.
    assert_eq!(
        run_last(&mut eval, "(reader-conditional '(:clj 1 :cljs 2) false)"),
        "#?(:clj 1 :cljs 2)"
    );
    assert_eq!(
        run_last(&mut eval, "(reader-conditional '(1 2) true)"),
        "#?@(1 2)"
    );
    // `c-arity*`: 2-arity exactly.
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional '(:clj 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/reader-conditional"
    );
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/reader-conditional"
    );
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional '(:clj 1) false :x)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/reader-conditional"
    );
    // `c-badform*`: the form is unguarded (even nil).
    assert_eq!(run_last(&mut eval, "(reader-conditional 1 false)"), "#?1");
    assert_eq!(
        run_last(&mut eval, "(reader-conditional [1] false)"),
        "#?[1]"
    );
    assert_eq!(
        run_last(&mut eval, "(reader-conditional nil false)"),
        "#?nil"
    );
    // `c-badsplice-*`: splicing takes `Boolean` only.
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional '(1) :nope)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Boolean (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Boolean is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional '(1) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Boolean.booleanValue()\" because \"splicing_QMARK_\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional '(1) 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Boolean (java.lang.Long and java.lang.Boolean are in module java.base of loader 'bootstrap')"
    );
    // `c-eq` / `c-ne-splice`: value equality over form and splicing.
    assert_eq!(
        run_last(
            &mut eval,
            "(= (reader-conditional '(1) false) (reader-conditional '(1) false))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (reader-conditional '(1) false) (reader-conditional '(1) true))"
        ),
        "false"
    );
    // `c-str*`: the port prints readably (the oracle records the JVM
    // `@hex` `toString`; print dispatch is not S6).
    assert_eq!(
        run_last(&mut eval, "(str (reader-conditional '(1) false))"),
        "\"#?(1)\""
    );
    assert_eq!(
        run_last(&mut eval, "(str (reader-conditional '(1) true))"),
        "\"#?@(1)\""
    );
}

#[test]
fn reader_conditional_predicate() {
    let mut eval = boot();
    // Battery `c-p-*`: only conditionals answer true (either
    // splicing).
    for (src, want) in [
        (
            "(reader-conditional? (reader-conditional '(1) false))",
            "true",
        ),
        (
            "(reader-conditional? (reader-conditional '(1) true))",
            "true",
        ),
        ("(reader-conditional? nil)", "false"),
        ("(reader-conditional? 1)", "false"),
        ("(reader-conditional? \"x\")", "false"),
        ("(reader-conditional? (list 1))", "false"),
        ("(reader-conditional? (tagged-literal 'a 1))", "false"),
        ("(reader-conditional? (fn [x] x))", "false"),
    ] {
        assert_eq!(run_last(&mut eval, src), want, "{src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/reader-conditional?"
    );
    assert_eq!(
        uncaught(&mut eval, "(reader-conditional? 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/reader-conditional?"
    );
}

#[test]
fn inst_default_reads() {
    let mut eval = boot();
    // Valid payloads read back as tagged literals echoing the
    // original string (the S6 representation boundary: the JVM
    // answers `java.util.Date`, printing canonical — probes
    // `d-inst`, `d-inst-offset`, `d-inst-nosecs`, `d-inst-zonez`,
    // `d-inst-ym`, `d-inst-year`, `d-inst-frac7`, `d-inst-leapsec`,
    // `d-inst-yearzone`, `d-inst-monthzone`, `d-inst-dayzone`).
    for src in [
        "(read-string \"#inst \\\"2024-01-01\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T10:00:00.000+02:00\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T00:00:00\\\"\")",
        "(read-string \"#inst \\\"2024-01\\\"\")",
        "(read-string \"#inst \\\"2024\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T00:00\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T00:00:00Z\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T00:00:00.1234567\\\"\")",
        "(read-string \"#inst \\\"2024-01-01T00:59:60\\\"\")",
        "(read-string \"#inst \\\"2024Z\\\"\")",
        "(read-string \"#inst \\\"2024-01Z\\\"\")",
        "(read-string \"#inst \\\"2024-01-01+02:00\\\"\")",
        "(read-string \"#inst \\\"2024-02-29\\\"\")",
        "(read-string \"#inst \\\"2000-02-29\\\"\")",
    ] {
        let got = run_last(&mut eval, src);
        assert!(got.starts_with("#inst \""), "{src} -> {got}");
        assert!(
            run_last(&mut eval, &format!("(tagged-literal? {src})")) == "true",
            "{src}"
        );
    }
    assert_eq!(
        run_last(&mut eval, "(read-string \"#inst \\\"2024-01-01\\\"\")"),
        "#inst \"2024-01-01\""
    );
}

#[test]
fn inst_validation_errors() {
    let mut eval = boot();
    // Grammar failures (probes `d-inst-bad`, `d-inst-trail`,
    // `d-inst-empty`, `d-inst-dotnofrac`, `d-inst-zonenocolon`,
    // `d-inst-lower-t`, `d-inst-lower-z`, `d-inst-space`,
    // `d-inst-nopad`).
    for payload in [
        "bogus",
        "2024-01-01xyz",
        "",
        "2024-01-01T00:00:00.",
        "2024-01-01T00:00:00+0200",
        "2024-01-01t00:00:00z",
        "2024-01-01T00:00:00z",
        "2024-01-01 00:00:00",
        "2024-1-1",
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(read-string \"#inst \\\"{payload}\\\"\")")
            ),
            format!("java.lang.RuntimeException | Unrecognized date/time syntax: {payload}"),
            "{payload}"
        );
    }
    // Range failures (probes `d-inst-badday`, `d-inst-day45`,
    // `d-inst-feb29`, `d-inst-hour25`, `d-inst-min60`,
    // `d-inst-sec61`, `d-inst-month00`, `d-inst-day00`,
    // `d-inst-zone25`, `d-inst-zone60`).
    for (payload, message) in [
        ("2024-13-45", "failed: (<= 1 months 12)"),
        ("2024-00-01", "failed: (<= 1 months 12)"),
        (
            "2024-01-45",
            "failed: (<= 1 days (days-in-month months (leap-year? years)))",
        ),
        (
            "2024-01-00",
            "failed: (<= 1 days (days-in-month months (leap-year? years)))",
        ),
        (
            "2023-02-29",
            "failed: (<= 1 days (days-in-month months (leap-year? years)))",
        ),
        ("2024-01-01T25:00:00", "failed: (<= 0 hours 23)"),
        ("2024-01-01T00:60:00", "failed: (<= 0 minutes 59)"),
        (
            "2024-01-01T00:00:61",
            "failed: (<= 0 seconds (if (= minutes 59) 60 59))",
        ),
        (
            "2024-01-01T00:00:00+25:00",
            "failed: (<= 0 offset-hours 23)",
        ),
        (
            "2024-01-01T00:00:00+02:60",
            "failed: (<= 0 offset-minutes 59)",
        ),
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(read-string \"#inst \\\"{payload}\\\"\")")
            ),
            format!("java.lang.RuntimeException | {message}"),
            "{payload}"
        );
    }
    // Non-string payloads (probes `d-inst-nonstr`, `d-inst-nil`,
    // `d-inst-vec`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#inst 42\")"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.CharSequence (java.lang.Long and java.lang.CharSequence are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#inst nil\")"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
    assert!(uncaught(&mut eval, "(read-string \"#inst [2024]\")").starts_with(
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class java.lang.CharSequence"
    ));
}

#[test]
fn uuid_default_reads() {
    let mut eval = boot();
    // Valid payloads echo (the S6 boundary: the JVM answers
    // `java.util.UUID`, lowercased — probes `d-uuid`,
    // `d-uuid-upper`, `d-uuid-shortgroup`, `d-uuid-shortmid`,
    // `d-uuid-longshort`).
    assert_eq!(
        run_last(
            &mut eval,
            "(read-string \"#uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\")"
        ),
        "#uuid \"550e8400-e29b-41d4-a716-446655440000\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(read-string \"#uuid \\\"550E8400-E29B-41D4-A716-446655440000\\\"\")"
        ),
        "#uuid \"550E8400-E29B-41D4-A716-446655440000\""
    );
    for payload in [
        "550e840-e29b-41d4-a716-446655440000",
        "550e8400-e29b-41d-a716-446655440000",
        "123456789-123-1234-1234-123456789012",
    ] {
        let got = run_last(
            &mut eval,
            &format!("(read-string \"#uuid \\\"{payload}\\\"\")"),
        );
        assert_eq!(got, format!("#uuid \"{payload}\""), "{payload}");
    }
    // Shape failures (probes `d-uuid-bad`, `d-uuid-short`,
    // `d-uuid-empty`, `d-uuid-braces`, `d-uuid-urn`,
    // `d-uuid-nodash`, `d-uuid-36nodash`, `d-uuid-emptypart`,
    // `d-uuid-traildash`, `d-uuid-dashes`).
    for payload in [
        "{550e8400-e29b-41d4-a716-446655440000}",
        "urn:uuid:550e8400-e29b-41d4-a716-446655440000",
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(read-string \"#uuid \\\"{payload}\\\"\")")
            ),
            "java.lang.IllegalArgumentException | UUID string too large",
            "{payload}"
        );
    }
    for payload in [
        "bogus",
        "550e8400-e29b-41d4-a716",
        "",
        "550e8400e29b41d4a716446655440000",
        "550e8400e29b41d4a71644665544000000",
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(read-string \"#uuid \\\"{payload}\\\"\")")
            ),
            format!("java.lang.IllegalArgumentException | Invalid UUID string: {payload}"),
            "{payload}"
        );
    }
    for payload in [
        "550e8400--41d4-a716-446655440000",
        "550e8400-e29b-41d4-a716-",
        "----",
    ] {
        assert_eq!(
            uncaught(
                &mut eval,
                &format!("(read-string \"#uuid \\\"{payload}\\\"\")")
            ),
            "java.lang.NumberFormatException | For input string: \"\" under radix 16",
            "{payload}"
        );
    }
    // Bad digits (probes `d-uuid-nonhex`, `d-uuid-badidx0`,
    // `d-uuid-badthenempty`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(read-string \"#uuid \\\"550e8400-e29b-41d4-a716-44665544000g\\\"\")"
        ),
        "java.lang.NumberFormatException | Error at index 11 in: \"44665544000g\""
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(read-string \"#uuid \\\"g50e8400-e29b-41d4-a716-446655440000\\\"\")"
        ),
        "java.lang.NumberFormatException | Error at index 0 in: \"g50e8400\""
    );
    // Non-strings (probes `d-uuid-nonstr`, `d-uuid-nil`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#uuid 42\")"),
        "java.lang.IllegalArgumentException | #uuid data reader expected string"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#uuid nil\")"),
        "java.lang.IllegalArgumentException | #uuid data reader expected string"
    );
    // Qualified names never hit the defaults (probes `d-qinst`,
    // `d-quuid`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#foo/inst \\\"2024-01-01\\\"\")"),
        "java.lang.RuntimeException | No reader function for tag foo/inst"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(read-string \"#foo/uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\")"
        ),
        "java.lang.RuntimeException | No reader function for tag foo/uuid"
    );
}

#[test]
fn default_readers_direct_calls() {
    let mut eval = boot();
    // Direct invocation validates like the read path (probes
    // `d-direct-inst`, `d-direct-uuid`, `d-direct-bad`).
    assert_eq!(
        run_last(
            &mut eval,
            "((get default-data-readers 'inst) \"2024-01-01\")"
        ),
        "#inst \"2024-01-01\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "((get default-data-readers 'uuid) \"550e8400-e29b-41d4-a716-446655440000\")"
        ),
        "#uuid \"550e8400-e29b-41d4-a716-446655440000\""
    );
    assert_eq!(
        uncaught(&mut eval, "((get default-data-readers 'inst) \"bogus\")"),
        "java.lang.RuntimeException | Unrecognized date/time syntax: bogus"
    );
    // The arity error names the JVM var (probes `d-direct-arity0`,
    // `d-direct-arity2`, `d-direct-uuid-arity0`).
    assert_eq!(
        uncaught(&mut eval, "((get default-data-readers 'inst))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.instant/read-instant-date"
    );
    assert_eq!(
        uncaught(&mut eval, "((get default-data-readers 'inst) \"a\" \"b\")"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.instant/read-instant-date"
    );
    assert_eq!(
        uncaught(&mut eval, "((get default-data-readers 'uuid))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.uuid/default-uuid-reader"
    );
}

#[test]
fn table_reads_invoke_per_occurrence() {
    let mut eval = boot();
    // `e-basic` / `e-ns`: bound tags invoke with the read form.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"#foo 1\"))"
        ),
        "[:ok 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo/bar (fn [x] [:ok x])}] (read-string \"#foo/bar 1\"))"
        ),
        "[:ok 1]"
    );
    // `e-nested` / `e-nested-tag` / `e-quote`: nested, stacked, and
    // quoted occurrences all invoke (read-time, inside-out).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"[#foo 1]\"))"
        ),
        "[[:ok 1]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:f x]) 'bar (fn [x] [:b x])}] (read-string \"#foo #bar 1\"))"
        ),
        "[:f [:b 1]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"'#foo 1\"))"
        ),
        "(quote [:ok 1])"
    );
    // `e-no-redescent`: fn results never re-resolve.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] (tagged-literal 'bar x)) 'bar (fn [x] [:B x])}] (read-string \"#foo 1\"))"
        ),
        "#bar 1"
    );
    // `e-meta-inner`: tags inside meta maps invoke.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (meta (read-string \"^{:t #foo 1} []\")))"
        ),
        "{:t [:ok 1]}"
    );
    // Map values resolve keys and values alike.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"{#foo 1 :v, :k #foo 2}\"))"
        ),
        "{[:ok 1] :v, :k [:ok 2]}"
    );
}

#[test]
fn table_precedence_and_errors() {
    let mut eval = boot();
    // `e-override-inst` / `e-override-novalid`: the table beats the
    // defaults without validating.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'inst (fn [x] :mine)}] (read-string \"#inst \\\"2024-01-01\\\"\"))"
        ),
        ":mine"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'inst (fn [x] :mine)}] (read-string \"#inst \\\"bogus\\\"\"))"
        ),
        ":mine"
    );
    // `e-nonmap`: a non-table fails the `ILookup` cast (before the
    // defaults — probe `e-nonmap-inst`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* 42] (read-string \"#foo 1\"))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.ILookup (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.ILookup is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* 42] (read-string \"#inst \\\"2024-01-01\\\"\"))"
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.ILookup (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.ILookup is in unnamed module of loader 'app')"
    );
    // `e-nonmap-nil` / `e-vectable` / `e-nilentry` / `e-strkey`:
    // lookups that miss fall through to the throw.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* nil] (read-string \"#foo 1\"))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* [(fn [x] :v)]] (read-string \"#foo 1\"))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo nil}] (read-string \"#foo 1\"))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {\"foo\" (fn [x] :s)}] (read-string \"#foo 1\"))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    // `e-niltable-inst`: a nil table still leaves the defaults.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* nil] (read-string \"#inst \\\"2024-01-01\\\"\"))"
        ),
        "#inst \"2024-01-01\""
    );
    // `e-nonfn`: a non-fn entry fails the `IFn` cast (the port's
    // invoke-cast shape — the oracle shows module verbiage, but the
    // invoke path is S4-frozen).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo 42}] (read-string \"#foo 1\"))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

#[test]
fn unknown_tags_throw_bare() {
    let mut eval = boot();
    // `e-unknown*`: `read-string` reports the bare
    // `RuntimeException` with nil ex-data (probes
    // `e-unknown-data`, `e-unknown-nested-data`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#foo 1\")"),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"[1 #foo 2]\")"),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#foo/bar 1\")"),
        "java.lang.RuntimeException | No reader function for tag foo/bar"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (read-string \"#foo 1\") (catch Exception e (ex-data e)))"
        ),
        "nil"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (read-string \"[1 #foo 2]\") (catch Exception e (ex-data e)))"
        ),
        "nil"
    );
}

#[test]
fn reader_fn_failures_propagate() {
    let mut eval = boot();
    // `e-fn-throws`: fn throws surface raw with data intact.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] (throw (ex-info \"boom\" {:k 1})))}] (read-string \"#foo 1\"))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (binding [*data-readers* {'foo (fn [x] (throw (ex-info \"boom\" {:k 1})))}] (read-string \"#foo 1\")) (catch Exception e (ex-data e)))"
        ),
        "{:k 1}"
    );
    // `e-fn-arity`: the house arity text (fn displays are
    // port-owned; the oracle names `user/evalN/fn--M`).
    assert!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [a b] :x)}] (read-string \"#foo 1\"))"
        )
        .starts_with("clojure.lang.ArityException | Wrong number of args (1) passed to: ")
    );
}

#[test]
fn discard_fires_in_read_order() {
    let mut eval = boot();
    // `e-discard-fires` / `e-discard-order`: `#_` invokes for
    // effect, in read order.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def drec-calls (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! drec-calls conj x) :v)}] [(read-string \"#_ #foo 1 :kept\") @drec-calls]))"
        ),
        "[:kept [1]]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def dord-calls (atom [])) (binding [*data-readers* {'a (fn [x] (swap! dord-calls conj [:a x]) :va) 'b (fn [x] (swap! dord-calls conj [:b x]) :vb)}] [(read-string \"#_ #a 1 #b 2\") @dord-calls]))"
        ),
        "[:vb [[:a 1] [:b 2]]]"
    );
    // `e-multiform-order`: drains run before each form resolves.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def mc-calls (atom [])) (binding [*data-readers* {'a (fn [x] (swap! mc-calls conj :a) :va) 'b (fn [x] (swap! mc-calls conj :b) :vb) 'c (fn [x] (swap! mc-calls conj :c) :vc)}] (with-in-str \"#a 1 #_ #b 2 #c 3\" [(read) (read) @mc-calls])))"
        ),
        "[:va :vc [:a :b :c]]"
    );
    // `e-discard-eof`: effects precede the EOF.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def eof-calls (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! eof-calls conj x) :v)}] (try [(read-string \"#_ #foo 1\")] (catch Throwable t [:caught @eof-calls]))))"
        ),
        "[:caught [1]]"
    );
    // `g-discard` / `g-discard-inst-bad`: discard throws propagate.
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#_ #bogus 1 :kept\")"),
        "java.lang.RuntimeException | No reader function for tag bogus"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#_ #inst \\\"bogus\\\" :kept\")"),
        "java.lang.RuntimeException | Unrecognized date/time syntax: bogus"
    );
}

#[test]
fn default_fn_fallback() {
    let mut eval = boot();
    // `f-basic` / `f-tag-shape`: the fallback runs with `[tag form]`
    // (tags arrive as symbols).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag x] [:d tag x])] (read-string \"#foo 1\"))"
        ),
        "[:d foo 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag x] [(symbol? tag) (str tag) x])] (read-string \"#foo/bar 1\"))"
        ),
        "[true \"foo/bar\" 1]"
    );
    // `f-vs-defaults`: the defaults beat the fallback.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag x] :dflt)] (read-string \"#inst \\\"2024-01-01\\\"\"))"
        ),
        "#inst \"2024-01-01\""
    );
    // `f-vs-table` / `f-table-miss`: the table beats the fallback,
    // which catches table misses.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] :t)} *default-data-reader-fn* (fn [tag x] :d)] (read-string \"#foo 1\"))"
        ),
        ":t"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'other (fn [x] :t)} *default-data-reader-fn* (fn [tag x] :d)] (read-string \"#foo 1\"))"
        ),
        ":d"
    );
    // `f-nonfn` / `f-arity1` / `f-throws`: fallback failures match
    // table-fn failures.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*default-data-reader-fn* 42] (read-string \"#foo 1\"))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert!(
        uncaught(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag] :x)] (read-string \"#foo 1\"))"
        )
        .starts_with("clojure.lang.ArityException | Wrong number of args (2) passed to: ")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag x] (throw (ex-info \"bf\" {:k 2})))] (read-string \"#foo 1\"))"
        ),
        "clojure.lang.ExceptionInfo | bf"
    );
}

#[test]
fn read_plus_string_runs_consult() {
    // B2 (review round 1): `read+string` consults like `read`
    // (defer mode was missing: table tags failed, `#inst` hit
    // the stale N5 text, default-fn tags misrouted, borked
    // tables misreported).
    let mut eval = boot();
    // Table tag resolves; the capture is the trimmed text.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'rs (fn [x] [:rs x])}] (with-in-str \"#rs 7\" (read+string *in*)))"
        ),
        "[[:rs 7] \"#rs 7\"]"
    );
    // `#inst` validates (same readable value as `read`).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#inst \\\"2020-01-02\\\"\" (read+string *in*))"
        ),
        "[#inst \"2020-01-02\" \"#inst \\\"2020-01-02\\\"\"]"
    );
    // Default-fn tags route (tags arrive as symbols).
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*default-data-reader-fn* (fn [tag x] [:d tag x])] (with-in-str \"#zzz 9\" (read+string *in*)))"
        ),
        "[[:d zzz 9] \"#zzz 9\"]"
    );
    // A borked table throws the table CCE (wrapped — `read`
    // paths ride `ReaderException`, unlike `read-string`; the
    // consult-time cast keeps the JVM module verbiage).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* 42] (with-in-str \"#foo 1\" (read+string *in*)))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.ClassCastException: class java.lang.Long cannot be cast to class clojure.lang.ILookup (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.ILookup is in unnamed module of loader 'app')"
    );
}

#[test]
fn reentrant_read_during_drain() {
    // H1 (review round 1): drain/resolve run outside the stream
    // lock — a data-reader fn that reads the same stream
    // interleaves instead of deadlocking. Bounded via a thread
    // + timeout (a regression fails here, not hangs CI).
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        let mut eval = boot();
        let out = run_last(
            &mut eval,
            "(binding [*data-readers* {'rr (fn [x] (read *in*))}] (with-in-str \"#rr 0 99\" (read *in*)))",
        );
        let _ = tx.send(out);
    });
    let out = rx
        .recv_timeout(std::time::Duration::from_secs(60))
        .expect("H1: reentrant read deadlocked");
    assert_eq!(out, "99");
}

#[test]
fn read_ignores_reader_opts() {
    let mut eval = boot();
    // `g-opts-*`: `read` honors no `:readers`/`:default` opts (only
    // the vars feed the consult).
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#foo 1\" (read {:readers {'foo (fn [x] :R)}} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: No reader function for tag foo"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#foo 1\" (read {:default (fn [tag x] :D)} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: No reader function for tag foo"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] :T)}] (with-in-str \"#foo 1\" (read {:readers {'foo (fn [x] :R)}} *in*)))"
        ),
        ":T"
    );
}

#[test]
fn conditional_tag_interplay() {
    let mut eval = boot();
    // `g-preserve*`: preserved tags stay data (never validated).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:a #inst \\\"2024-01-01\\\")\" (read {:read-cond :preserve} *in*))"
        ),
        "#?(:a #inst \"2024-01-01\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:a #inst \\\"bogus\\\")\" (read {:read-cond :preserve} *in*))"
        ),
        "#?(:a #inst \"bogus\")"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (with-in-str \"#?(:a #foo 1)\" (read {:read-cond :preserve} *in*)))"
        ),
        "#?(:a #foo 1)"
    );
    // `g-preserve-top`: a top-level default still validates.
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#inst \\\"2024-01-01\\\"\" (read {:read-cond :preserve} *in*))"
        ),
        "#inst \"2024-01-01\""
    );
    // `g-allow-match` / `g-allow-nomatch*`: matched arms invoke,
    // suppressed arms never consult.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (with-in-str \"#?(:clj #foo 1 :cljs 2)\" (read {:read-cond :allow} *in*)))"
        ),
        "[:ok 1]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:cljs #bogus 99) :kept\" (read {:read-cond :allow} *in*))"
        ),
        ":kept"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:cljs #inst \\\"bogus\\\") :kept\" (read {:read-cond :allow} *in*))"
        ),
        ":kept"
    );
}

#[test]
fn meta_on_tagged_form_errors() {
    let mut eval = boot();
    // `e-meta-vec` boundary: a `^meta` directly on a tagged form
    // errors (the reader attaches before the walk resolves; the JVM
    // attaches to the fn result). Via `read` the IAE class agrees.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (with-in-str \"^:k #foo 1\" (read)))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.IllegalArgumentException: Metadata can only be applied to IMetas"
    );
    // Via `read-string` the class is the frozen `RuntimeException`
    // mapping (S5 reports every non-consult reader error bare; the
    // JVM reports IAE here — probe `e-meta-scalar`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] 42)}] (read-string \"^:k #foo 1\"))"
        ),
        "java.lang.RuntimeException | Metadata can only be applied to IMetas"
    );
}

#[test]
fn read_wraps_tag_errors() {
    let mut eval = boot();
    // `h-lnpr-wrap` / `h-lnpr-multiline`: the port's readers are
    // line-numbering, so `read` wraps (positions verbatim).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#foo 1\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: No reader function for tag foo"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#foo 1\" (read)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 2, :column 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#foo\n1\" (read)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 3, :column 1}"
    );
    // `h-lnpr-recursive`: recursive reads rethrow raw.
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#foo 1\" (read *in* true :eofval true))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    // `h-lnpr-opts` / `h-wis`: opts don't matter; the throw site is
    // post-form (a string form leaves line 1).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#foo 1\" (read {} *in*)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 2, :column 1}"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#foo \\\"x\\\"\" (read {:read-cond :preserve} *in*)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 1, :column 9}"
    );
    // `h-lnpr-instbad`: validation errors wrap at the throw site.
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#inst \\\"bogus\\\"\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Unrecognized date/time syntax: bogus"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#inst \\\"bogus\\\"\" (read)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 1, :column 14}"
    );
    // `h-lnpr-discard-throw`: discards throw mid-read (before the
    // trailing form).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"#_ #bogus 1 :kept\" (read)) (catch Exception e (ex-data e)))"
        ),
        "#:clojure.error{:line 1, :column 12}"
    );
}

#[test]
fn read_wraps_fn_failures() {
    let mut eval = boot();
    // `h-lnpr-fnthrows`: fn throws wrap with the cause data intact
    // (the inner message lacks the data map — the port's `ex-info`
    // stores it separately, an S4 shape S6 formats faithfully).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defn hboom [x] (throw (ex-info \"hboom\" {:hk 7}))) (binding [*data-readers* {'foo hboom}] (with-in-str \"#foo 1\" (read))))"
        ),
        "clojure.lang.LispReader$ReaderException | clojure.lang.ExceptionInfo: hboom"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defn hboom [x] (throw (ex-info \"hboom\" {:hk 7}))) (binding [*data-readers* {'foo hboom}] (try (with-in-str \"#foo 1\" (read)) (catch Exception e [(ex-data e) (ex-data (ex-cause e))]))))"
        ),
        "[#:clojure.error{:line 2, :column 1} {:hk 7}]"
    );
    // `h-lnpr-fnarity`: arity failures wrap (port fn display).
    assert!(
        uncaught(
            &mut eval,
            "(do (defn harity [a b] :h) (binding [*data-readers* {'foo harity}] (with-in-str \"#foo 1\" (read))))"
        )
        .starts_with("clojure.lang.LispReader$ReaderException | clojure.lang.ArityException: Wrong number of args (1) passed to: ")
    );
    // `h-str-fnthrows`: `read-string` never wraps.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defn hboom [x] (throw (ex-info \"hboom\" {:hk 7}))) (binding [*data-readers* {'foo hboom}] (read-string \"#foo 1\")))"
        ),
        "clojure.lang.ExceptionInfo | hboom"
    );
    // A recursive read rethrows fn failures raw too.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defn hboom [x] (throw (ex-info \"hboom\" {:hk 7}))) (binding [*data-readers* {'foo hboom}] (with-in-str \"#foo 1\" (read *in* true :e true))))"
        ),
        "clojure.lang.ExceptionInfo | hboom"
    );
    // Empty messages wrap with a missing message.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defn hnil [x] (throw (ex-info nil {:k 1}))) (binding [*data-readers* {'foo hnil}] (with-in-str \"#foo 1\" (read))))"
        ),
        "clojure.lang.LispReader$ReaderException | clojure.lang.ExceptionInfo: "
    );
}

#[test]
fn load_and_eval_source_resolve() {
    let mut eval = boot();
    // Loaded sources resolve through the shared `read_forms` hook
    // (runtime resolution: the defs don't exist at compile time).
    assert_eq!(
        run_last(
            &mut eval,
            "(do (load-string \"(def s6-loaded #inst \\\"2024-01-01\\\")\") @(resolve 's6-loaded))"
        ),
        "#inst \"2024-01-01\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (do (load-string \"(def s6l #foo 1)\") @(resolve 's6l)))"
        ),
        "[:ok 1]"
    );
}

#[test]
fn root_table_reads() {
    // The table at root (no binding): `eval_source` itself reads
    // through the consult (multi-form drains, walk failures).
    let mut eval = boot();
    run(
        &mut eval,
        "(do (def s6rec (atom [])) (alter-var-root #'*data-readers* assoc 'q (fn [x] (swap! s6rec conj x) :vq)))",
    );
    assert_eq!(
        run(&mut eval, "#_ #q 1 :r1 #_ #q 2 :r2"),
        vec![":r1".to_owned(), ":r2".to_owned()]
    );
    assert_eq!(run_last(&mut eval, "@s6rec"), "[1 2]");
    // A mid-walk table removal throws from the walk (the reader
    // consulted the pre-read snapshot).
    let mut eval = boot();
    run(
        &mut eval,
        "(alter-var-root #'*data-readers* assoc 'a (fn [x] (alter-var-root #'*data-readers* dissoc 'b) :ra) 'b (fn [x] :rb))",
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"[#a 1 #b 2]\")"),
        "java.lang.RuntimeException | No reader function for tag b"
    );
    // A mid-walk table break fails the cast from the walk.
    let mut eval = boot();
    run(
        &mut eval,
        "(alter-var-root #'*data-readers* assoc 'a (fn [x] (alter-var-root #'*data-readers* (fn [_] 42)) :ra) 'b (fn [x] :rb))",
    );
    assert!(uncaught(&mut eval, "(read-string \"[#a 1 #b 2]\")").starts_with(
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.ILookup"
    ));
    // A throwing root table fn surfaces uncaught through `read_forms`.
    let mut eval = boot();
    run(
        &mut eval,
        "(alter-var-root #'*data-readers* assoc 'q (fn [x] (throw (ex-info \"rootboom\" {}))))",
    );
    assert_eq!(
        uncaught(&mut eval, "#q 1"),
        "clojure.lang.ExceptionInfo | rootboom"
    );
}

#[test]
fn exotic_tables_and_tags() {
    let mut eval = boot();
    // Records consult their content (a miss here — no symbol keys).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defrecord S6R [f]) (binding [*data-readers* (->S6R 1)] (read-string \"#foo 1\")))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    // ...and a symbol-keyed entry hits through the content.
    assert_eq!(
        run_last(
            &mut eval,
            "(do (defrecord S6Q [f]) (binding [*data-readers* (assoc (->S6Q 1) 'foo (fn [x] :R))] (read-string \"#foo 1\")))"
        ),
        ":R"
    );
    // Map entries never symbol-match.
    assert_eq!(
        uncaught(
            &mut eval,
            "(binding [*data-readers* (first {:a 1})] (read-string \"#foo 1\"))"
        ),
        "java.lang.RuntimeException | No reader function for tag foo"
    );
    // Dotted tags keep the record deferral (both entry points).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#foo.bar [1]\")"),
        "java.lang.RuntimeException | Record construction is not supported in this build (S5): #foo.bar"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#foo.bar [1]\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.Exception: Record construction is not supported in this build (S5): #foo.bar"
    );
    // Walk coverage: collections and symbols around tags.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"(do #foo 1)\"))"
        ),
        "(do [:ok 1])"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string \"[#foo 1 a]\"))"
        ),
        "[[:ok 1] a]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*data-readers* {'foo (fn [x] 1)}] (read-string \"#{(#foo 1)}\"))"
        ),
        "#{(1)}"
    );
    // `#_` fires before a `read` error, and before the eof value.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def s6e (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! s6e conj x) (throw (ex-info \"drainboom\" {})))}] (with-in-str \"#_ #foo 1 ]\" (read))))"
        ),
        "clojure.lang.LispReader$ReaderException | clojure.lang.ExceptionInfo: drainboom"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (def s6v (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! s6v conj x) :v)}] [(with-in-str \"#_ #foo 1\" (read *in* false :eof)) @s6v]))"
        ),
        "[:eof [1]]"
    );
}
