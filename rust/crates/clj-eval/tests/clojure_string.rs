// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The embedded `clojure.string` port: every public var runs
//! through `require` and matches the JVM oracle labels (probes
//! under clojure-1.12.0 — see `host_members.rs` for the probe
//! setup).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw (strict: compile
/// errors fail — phase pins must not silently pass).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(_) => panic!("expected an uncaught throw, got success"),
    }
}

fn require_str(eval: &mut Evaluator) {
    run(eval, "(require '[clojure.string :as str])");
}

#[test]
fn string_basics_rows() {
    let mut eval = boot();
    require_str(&mut eval);
    assert_eq!(run(&mut eval, "(str/reverse \"abc\")"), "\"cba\"");
    assert_eq!(run(&mut eval, "(str/reverse \"\")"), "\"\"");
    assert_eq!(
        run(&mut eval, "(str/re-quote-replacement \"a$b\")"),
        "\"a\\\\$b\""
    );
    assert_eq!(run(&mut eval, "(str/join \", \" [1 2])"), "\"1, 2\"");
    assert_eq!(run(&mut eval, "(str/join [1 2])"), "\"12\"");
    assert_eq!(run(&mut eval, "(str/join nil)"), "\"\"");
    assert_eq!(run(&mut eval, "(str/join \", \" nil)"), "\"\"");
    assert_eq!(run(&mut eval, "(str/join \", \" [])"), "\"\"");
    assert_eq!(run(&mut eval, "(str/upper-case \"aBc\")"), "\"ABC\"");
    assert_eq!(run(&mut eval, "(str/lower-case \"aBc\")"), "\"abc\"");
    assert_eq!(run(&mut eval, "(str/capitalize \"ABC\")"), "\"Abc\"");
    assert_eq!(run(&mut eval, "(str/capitalize \"\")"), "\"\"");
    assert_eq!(
        run(
            &mut eval,
            "(str/escape \"<b>\" {\\< \"&lt;\" \\> \"&gt;\"})"
        ),
        "\"&lt;b&gt;\""
    );
}

#[test]
fn string_replace_rows() {
    let mut eval = boot();
    require_str(&mut eval);
    assert_eq!(
        run(&mut eval, "(str/replace \"aaa\" \"a\" \"b\")"),
        "\"bbb\""
    );
    assert_eq!(run(&mut eval, "(str/replace \"aaa\" \\a \\b)"), "\"bbb\"");
    assert_eq!(
        run(&mut eval, "(str/replace \"a1b2\" #\"\\d\" \"#\")"),
        "\"a#b#\""
    );
    assert_eq!(
        run(&mut eval, "(str/replace \"abc\" #\"b\" (fn [m] \"B\"))"),
        "\"aBc\""
    );
    assert_eq!(
        run(&mut eval, "(str/replace-first \"aaa\" \"a\" \"b\")"),
        "\"baa\""
    );
    assert_eq!(
        run(&mut eval, "(str/replace-first \"a1b2\" #\"\\d\" \"#\")"),
        "\"a#b2\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str/replace-first \"abc\" #\"b\" (fn [m] \"B\"))"
        ),
        "\"aBc\""
    );
    assert_eq!(
        uncaught(&mut eval, "(str/replace \"aaa\" 1 \"b\")"),
        "java.lang.IllegalArgumentException | Invalid match arg: 1"
    );
    assert_eq!(
        uncaught(&mut eval, "(str/replace-first \"aaa\" 1 \"b\")"),
        "java.lang.IllegalArgumentException | Invalid match arg: 1"
    );
}

#[test]
fn string_split_trim_rows() {
    let mut eval = boot();
    require_str(&mut eval);
    assert_eq!(
        run(&mut eval, "(str/split \"a,b\" #\",\")"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        run(&mut eval, "(str/split \"a,b,\" #\",\")"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        run(&mut eval, "(str/split \"a b \" #\"\\s+\" -1)"),
        "[\"a\" \"b\" \"\"]"
    );
    assert_eq!(run(&mut eval, "(str/split \"a,b\" #\",\" 1)"), "[\"a,b\"]");
    assert_eq!(
        run(&mut eval, "(str/split \"a,b\" #\",\" 2)"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(run(&mut eval, "(str/split \"\" #\",\")"), "[\"\"]");
    assert_eq!(
        run(&mut eval, "(str/split-lines \"a\nb\n\")"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        run(&mut eval, "(str/split-lines \"a\r\nb\")"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(run(&mut eval, "(str/trim \"  a  \")"), "\"a\"");
    assert_eq!(run(&mut eval, "(str/trim \"\")"), "\"\"");
    assert_eq!(run(&mut eval, "(str/trim \"   \")"), "\"\"");
    assert_eq!(run(&mut eval, "(str/triml \"  a  \")"), "\"a  \"");
    assert_eq!(run(&mut eval, "(str/trimr \"  a  \")"), "\"  a\"");
    assert_eq!(run(&mut eval, "(str/trim-newline \"a\n\")"), "\"a\"");
    assert_eq!(run(&mut eval, "(str/trim-newline \"a\r\n\")"), "\"a\"");
    assert_eq!(run(&mut eval, "(str/blank? nil)"), "true");
    assert_eq!(run(&mut eval, "(str/blank? \"\")"), "true");
    assert_eq!(run(&mut eval, "(str/blank? \"  \")"), "true");
    assert_eq!(run(&mut eval, "(str/blank? \" a \")"), "false");
}

#[test]
fn string_predicate_rows() {
    let mut eval = boot();
    require_str(&mut eval);
    assert_eq!(run(&mut eval, "(str/index-of \"abc\" \"b\")"), "1");
    assert_eq!(run(&mut eval, "(str/index-of \"abc\" \"b\" 2)"), "nil");
    assert_eq!(run(&mut eval, "(str/index-of \"abc\" \"z\")"), "nil");
    assert_eq!(run(&mut eval, "(str/last-index-of \"abcb\" \"b\")"), "3");
    assert_eq!(run(&mut eval, "(str/last-index-of \"abcb\" \"b\" 2)"), "1");
    assert_eq!(run(&mut eval, "(str/starts-with? \"abc\" \"ab\")"), "true");
    assert_eq!(run(&mut eval, "(str/starts-with? \"abc\" \"bc\")"), "false");
    assert_eq!(run(&mut eval, "(str/ends-with? \"abc\" \"bc\")"), "true");
    assert_eq!(run(&mut eval, "(str/includes? \"abc\" \"b\")"), "true");
    assert_eq!(run(&mut eval, "(str/includes? \"abc\" \"z\")"), "false");
}
