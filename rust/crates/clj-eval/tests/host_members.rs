// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Host-object members: string search/replace, `charValue`,
//! `Pattern.split`, matcher find/group/append/replace, builder
//! append/length/reverse, and the `StringBuilder`/`StringBuffer`/
//! `IllegalArgumentException` ctors. Every pin cites its JVM
//! oracle label; the probes ran under:
//! `java -cp clojure-1.12.0.jar:spec.alpha-0.5.238.jar:
//! core.specs.alpha-0.4.74.jar clojure.main probeN.clj`.
//! Instance int params follow the port's `boxArg` convention
//! (`str_int_arg` — wrapping, `boxarg` nils); multi-overload
//! selection misses read `No matching method … for class …`.

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

/// The compile-error chain (message + cause) for phase pins.
fn chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => e.chain(),
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(_) => panic!("expected a compile error, got success"),
    }
}

#[test]
fn index_of_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"b\")"), "1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"b\" 2)"), "-1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" 98)"), "1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"z\")"), "-1");
    assert_eq!(run(&mut eval, "(.indexOf \"ab\" \"abcd\")"), "-1");
    // UTF-16 units, not chars or bytes.
    assert_eq!(run(&mut eval, "(.indexOf \"\u{1D11E}b\" \"b\")"), "2");
    assert_eq!(
        run(&mut eval, "(.lastIndexOf \"\u{1D11E}b\" \"\u{1D11E}\")"),
        "0"
    );
    // from-index clamping (oracles `idx-fromneg`, `idx-frombig`).
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"b\" -5)"), "1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"b\" 99)"), "-1");
    // The empty needle answers the (clamped) end.
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"\")"), "0");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"\" 5)"), "3");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" \"\" -2)"), "0");
    // Code points: negatives through the low 16 bits (oracle
    // `idx-neg`), astral pairs, out-of-range never matches.
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" -1)"), "-1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" 1114112)"), "-1");
    assert_eq!(run(&mut eval, "(.indexOf \"abc\" 55296)"), "-1");
    // Nil needles NPE in the overload bodies (oracles `idx-nil`,
    // `idx-nil2`).
    assert_eq!(
        uncaught(&mut eval, "(.indexOf \"abc\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.coder()\" because \"str\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.indexOf \"abc\" nil 1)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"tgtStr\" is null"
    );
    // Anything else misses (the multi-overload rule — oracles
    // `idx-char`, `idx-dbl`, `idx-nilfrom`, `idx-strfrom`).
    for src in [
        "(.indexOf \"abc\" \\b)",
        "(.indexOf \"abc\" 98.0)",
        "(.indexOf \"abc\" true)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method indexOf found taking 1 args for class java.lang.String",
            "{src}"
        );
    }
    for src in [
        "(.indexOf \"abc\" \"b\" nil)",
        "(.indexOf \"abc\" 98 nil)",
        "(.indexOf \"abc\" \"b\" \"c\")",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method indexOf found taking 2 args for class java.lang.String",
            "{src}"
        );
    }
}

#[test]
fn last_index_of_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abcb\" \"b\")"), "3");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abcb\" \"b\" 2)"), "1");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abc\" 98)"), "1");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abc\" \"z\")"), "-1");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"ab\" \"abcd\")"), "-1");
    // from-index clamping: negatives fail, past-the-end clamps.
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abcb\" \"b\" -1)"), "-1");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abcb\" \"b\" 99)"), "3");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abc\" \"\" 5)"), "3");
    assert_eq!(run(&mut eval, "(.lastIndexOf \"abc\" \"\" -2)"), "-1");
    // Nil needles (oracles `lidx-nil`, `lidx-nilfrom` — the
    // 2-arity nil misses outright, unlike `indexOf`).
    assert_eq!(
        uncaught(&mut eval, "(.lastIndexOf \"abc\" nil)"),
        "java.lang.NullPointerException | Cannot read field \"value\" because \"tgtStr\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.lastIndexOf \"abc\" nil 1)"),
        "java.lang.IllegalArgumentException | No matching method lastIndexOf found taking 2 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.lastIndexOf \"abc\" \"b\" nil)"),
        "java.lang.IllegalArgumentException | No matching method lastIndexOf found taking 2 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.lastIndexOf \"abc\" 98.0)"),
        "java.lang.IllegalArgumentException | No matching method lastIndexOf found taking 1 args for class java.lang.String"
    );
}

#[test]
fn subsequence_contains_replace_rows() {
    let mut eval = boot();
    // `subSequence` is UTF-16 (like `charAt`, unlike `substring`).
    assert_eq!(run(&mut eval, "(.subSequence \"abc\" 1 3)"), "\"bc\"");
    assert_eq!(run(&mut eval, "(.subSequence \"aéb\" 1 3)"), "\"éb\"");
    assert_eq!(
        uncaught(&mut eval, "(.subSequence \"a\" 0 5)"),
        "java.lang.StringIndexOutOfBoundsException | Range [0, 5) out of bounds for length 1"
    );
    assert_eq!(
        uncaught(&mut eval, "(.subSequence \"abc\" 2 1)"),
        "java.lang.StringIndexOutOfBoundsException | Range [2, 1) out of bounds for length 3"
    );
    // `contains` (oracle `con-nil`).
    assert_eq!(run(&mut eval, "(.contains \"abc\" \"b\")"), "true");
    assert_eq!(run(&mut eval, "(.contains \"abc\" \"z\")"), "false");
    assert_eq!(
        uncaught(&mut eval, "(.contains \"abc\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.contains \"abc\" 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
    // `replace` (oracles `repl`, `repl-c`, `rep-nilt`,
    // `rep-nilr`, `rep-int`, `rep-mix`).
    assert_eq!(run(&mut eval, "(.replace \"aaa\" \"a\" \"b\")"), "\"bbb\"");
    assert_eq!(run(&mut eval, "(.replace \"aaa\" \\a \\b)"), "\"bbb\"");
    assert_eq!(
        uncaught(&mut eval, "(.replace \"aaa\" nil \"b\")"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"target\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.replace \"aaa\" \"a\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"replacement\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.replace \"aaa\" nil nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"target\" is null"
    );
    for src in ["(.replace \"aaa\" 97 98)", "(.replace \"aaa\" \"a\" 98)"] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method replace found taking 2 args for class java.lang.String",
            "{src}"
        );
    }
    // `charValue` (oracle `charval`).
    assert_eq!(run(&mut eval, "(.charValue \\q)"), "\\q");
    assert_eq!(
        uncaught(&mut eval, "(.charValue \"a\")"),
        "java.lang.IllegalArgumentException | No matching field found: charValue for class java.lang.String"
    );
}

#[test]
fn pattern_split_rows() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(vec (.split #\"\\s+\" \"a b\"))"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        run(&mut eval, "(vec (.split #\"\\s+\" \"a b \" -1))"),
        "[\"a\" \"b\" \"\"]"
    );
    assert_eq!(
        run(&mut eval, "(vec (.split #\"\" \"ab\"))"),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        uncaught(&mut eval, "(.split #\"\\s+\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
    );
}

#[test]
fn matcher_find_group_span_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"\\d+\" \"a1b22\")] [(.find m) (re-groups m) (.find m) (re-groups m) (.find m)])"
        ),
        "[true \"1\" true \"22\" false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"(\\w)(\\d)\" \"a1\")] (.find m) [(.group m) (.group m 1) (.group m 2) (.start m) (.end m)])"
        ),
        "[\"a1\" \"a\" \"1\" 0 2]"
    );
    // Unmatched groups answer nil (oracle `group-niltext`).
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"(a)|(b)\" \"a\")] (.find m) (.group m 2))"
        ),
        "nil"
    );
    // UTF-16 offsets.
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"\u{1D11E}1\")] (.find m) [(.start m) (.end m)])"
        ),
        "[2 3]"
    );
    // Reads without a match fail (oracles `group-fresh`,
    // `start-fresh`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a\")] (.group m))"
        ),
        "java.lang.IllegalStateException | No match found"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a\")] (.start m))"
        ),
        "java.lang.IllegalStateException | No match found"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [m (re-matcher #\"\\d\" \"a\")] (.end m))"),
        "java.lang.IllegalStateException | No match found"
    );
    // Bad group indexes (oracles `group-idx`, `group-neg`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"(\\w)(\\d)\" \"a1\")] (.find m) (.group m 5))"
        ),
        "java.lang.IndexOutOfBoundsException | No group 5"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"(\\d)\" \"a1\")] (.find m) (.group m -1))"
        ),
        "java.lang.IndexOutOfBoundsException | No group -1"
    );
    // `matches` (oracles `matches`, `matches-no`).
    assert_eq!(
        run(&mut eval, "(.matches (re-matcher #\"\\d+\" \"123\"))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(.matches (re-matcher #\"\\d+\" \"12a\"))"),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"(\\d)(\\d)\" \"12\")] (.matches m) (.group m 2))"
        ),
        "\"2\""
    );
    // Unknown members miss with the class shape.
    assert_eq!(
        uncaught(&mut eval, "(.find \"x\")"),
        "java.lang.IllegalArgumentException | No matching field found: find for class java.lang.String"
    );
}

#[test]
fn matcher_append_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"a\" \"xay\") sb (StringBuffer.)] (.find m) (.appendReplacement m sb \"b\") (.appendTail m sb) (str sb))"
        ),
        "\"xby\""
    );
    // Appends return the buffer.
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"a\" \"xay\") sb (StringBuffer.)] (.find m) (identical? sb (.appendReplacement m sb \"b\")))"
        ),
        "true"
    );
    // A fresh tail appends the whole input (oracle
    // `apptail-fresh`).
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"ab\") sb (StringBuffer.)] (.appendTail m sb) (str sb))"
        ),
        "\"ab\""
    );
    // Appends without a match fail (oracle `apprep-nomatch`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"ab\") sb (StringBuffer.)] (.appendReplacement m sb \"x\"))"
        ),
        "java.lang.IllegalStateException | No match found"
    );
    // Nil buffers NPE with the call-site text (oracles
    // `apprep-nilsb`, `apptail-nilsb`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a1\")] (.find m) (.appendReplacement m nil \"x\"))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.StringBuffer.length()\" because \"sb\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a1\")] (.find m) (.appendTail m nil))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.StringBuffer.append(java.lang.CharSequence, int, int)\" because \"sb\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a1\") sb (StringBuffer.)] (.find m) (.appendReplacement m sb nil))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"replacement\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"a1\")] (.find m) (.appendTail m \"x\"))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.String to java.lang.StringBuffer"
    );
}

#[test]
fn matcher_replace_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"\\d\" \"a1b2\") \"#\")"
        ),
        "\"a#b#\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\w)(\\d)\" \"a1\") \"$2$1\")"
        ),
        "\"1a\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.replaceFirst (re-matcher #\"\\d\" \"abc\") \"#\")"
        ),
        "\"abc\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.replaceFirst (re-matcher #\"(\\w)(\\d)\" \"a1b2\") \"$2$1\")"
        ),
        "\"1ab2\""
    );
    // Empty matches replace at every boundary (oracle
    // `repempty`).
    assert_eq!(
        run(&mut eval, "(.replaceAll (re-matcher #\"x*\" \"ab\") \"-\")"),
        "\"-a-b-\""
    );
    // `$0` is the full match, unmatched groups empty (oracles
    // `dollar0`, `unmatched-ref`).
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"[$0]\")"
        ),
        "\"a[1]\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"(a)|(b)\" \"a\") \"<$2>\")"
        ),
        "\"<>\""
    );
    // Two digits when the group exists, else one plus a literal
    // (oracle `dollar12`).
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"$12\")"
        ),
        "\"a12\""
    );
    // Quoting (oracles `repall-backslash`, `quoteReplacement`
    // covered in `host_static`).
    assert_eq!(
        run(
            &mut eval,
            "(.replaceAll (re-matcher #\"\\d\" \"a1\") \"\\\\$\")"
        ),
        "\"a$\""
    );
    // A prior `find` never leaks in (oracle `replace-reset`).
    assert_eq!(
        run(
            &mut eval,
            "(let [m (re-matcher #\"\\d\" \"1a2\")] (.find m) (.replaceAll m \"#\"))"
        ),
        "\"#a#\""
    );
    // Expansion failures (oracles `repall-dollar`,
    // `trail-dollar`, `trail-bs`, `dollar-bad`, `dollar-brace`,
    // `brace-name`, `brace-empty`, `brace-unterm`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"\\d\" \"a1\") \"$$\")"
        ),
        "java.lang.IllegalArgumentException | Illegal group reference"
    );
    assert_eq!(
        uncaught(&mut eval, "(.replaceAll (re-matcher #\"b\" \"ab\") \"x$\")"),
        "java.lang.IllegalArgumentException | Illegal group reference: group index is missing"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"b\" \"ab\") \"x\\\\\")"
        ),
        "java.lang.IllegalArgumentException | character to be escaped is missing"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"$2\")"
        ),
        "java.lang.IndexOutOfBoundsException | No group 2"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"${1}\")"
        ),
        "java.lang.IllegalArgumentException | capturing group name {1} starts with digit character"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"${foo}\")"
        ),
        "java.lang.IllegalArgumentException | No group with name {foo}"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"${}\")"
        ),
        "java.lang.IllegalArgumentException | named capturing group has 0 length name"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceAll (re-matcher #\"(\\d)\" \"a1\") \"${1\")"
        ),
        "java.lang.IllegalArgumentException | named capturing group is missing trailing '}'"
    );
    // Nil replacements (oracles `repall-nilrt`,
    // `repfirst-nilrt` — note the empty `replaceFirst` text).
    assert_eq!(
        uncaught(&mut eval, "(.replaceAll (re-matcher #\"\\d\" \"a1\") nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"replacement\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.replaceFirst (re-matcher #\"\\d\" \"a1\") nil)"
        ),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(.replaceAll (re-matcher #\"\\d\" \"a1\") 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
}

#[test]
fn builder_ctor_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(str (StringBuilder.))"), "\"\"");
    assert_eq!(run(&mut eval, "(str (StringBuilder. \"ab\"))"), "\"ab\"");
    assert_eq!(run(&mut eval, "(str (StringBuilder. 16))"), "\"\"");
    assert_eq!(run(&mut eval, "(str (StringBuffer. \"ab\"))"), "\"ab\"");
    // The `CharSequence` overload copies fellow builders
    // (oracle `sb-cs`).
    assert_eq!(
        run(&mut eval, "(str (StringBuilder. (StringBuffer. \"ab\")))"),
        "\"ab\""
    );
    assert_eq!(
        run(&mut eval, "(str (StringBuffer. (StringBuilder. \"ab\")))"),
        "\"ab\""
    );
    // Nils take the `String` overload and NPE in its body.
    assert_eq!(
        uncaught(&mut eval, "(StringBuilder. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"str\" is null"
    );
    // Anything else misses (oracle `sb-bad` — doubles and
    // tower values never unbox to the capacity).
    for src in [
        "(StringBuilder. 1.5)",
        "(StringBuilder. 16.0)",
        "(StringBuilder. 16N)",
        "(StringBuilder. true)",
        "(StringBuilder. \\a)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.StringBuilder",
            "{src}"
        );
    }
    // Capacities range-check exactly (oracles `sb-negcap`,
    // `sb-hugecap`).
    assert_eq!(
        uncaught(&mut eval, "(StringBuilder. -5)"),
        "java.lang.NegativeArraySizeException | -5"
    );
    assert_eq!(
        uncaught(&mut eval, "(StringBuilder. 1099511627776)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // Bad arities fail at compile time.
    assert_eq!(
        chain(&mut eval, "(StringBuilder. \"a\" \"b\")"),
        vec![
            "Syntax error compiling new at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.StringBuilder"
                .to_owned(),
        ]
    );
}

#[test]
fn builder_member_rows() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str (.append (StringBuilder.) \"a\"))"),
        "\"a\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (-> (StringBuilder.) (.append \"a\") (.append \\b) (.append 1)))"
        ),
        "\"ab1\""
    );
    // Nils append `"null"` (oracle `app-nilrt`).
    assert_eq!(
        run(&mut eval, "(str (.append (StringBuilder.) nil))"),
        "\"null\""
    );
    assert_eq!(
        run(&mut eval, "(str (.append (StringBuilder.) 0.1))"),
        "\"0.1\""
    );
    assert_eq!(
        run(&mut eval, "(str (.append (StringBuilder.) [1 2]))"),
        "\"[1 2]\""
    );
    // Appends return the buffer.
    assert_eq!(
        run(
            &mut eval,
            "(let [sb (StringBuilder.)] (identical? sb (.append sb \"x\")))"
        ),
        "true"
    );
    assert_eq!(run(&mut eval, "(.length (StringBuilder. \"ab\"))"), "2");
    assert_eq!(
        run(&mut eval, "(.length (StringBuilder. \"\u{1D11E}\"))"),
        "2"
    );
    assert_eq!(
        run(&mut eval, "(str (.reverse (StringBuilder. \"ab\")))"),
        "\"ba\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (.reverse (StringBuilder. \"a\u{1D11E}b\")))"
        ),
        "\"b\u{1D11E}a\""
    );
    // `toString` reads the contents.
    assert_eq!(
        run(&mut eval, "(.toString (StringBuilder. \"ab\"))"),
        "\"ab\""
    );
    // Identity equality and hash (oracles `sb-eq`, `sb-hc`).
    assert_eq!(
        run(
            &mut eval,
            "(.equals (StringBuilder. \"a\") (StringBuilder. \"a\"))"
        ),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(.equals (StringBuilder. \"a\") \"a\")"),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [sb (StringBuilder. \"a\")] (.equals sb sb))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [sb (StringBuilder. \"a\")] (= (.hashCode sb) (.hashCode sb)))"
        ),
        "true"
    );
    // Classes distinguish builders from buffers.
    assert_eq!(
        run(&mut eval, "(class (StringBuilder.))"),
        "java.lang.StringBuilder"
    );
    assert_eq!(
        run(&mut eval, "(instance? StringBuilder (StringBuilder.))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(instance? StringBuffer (StringBuilder.))"),
        "false"
    );
    // Members miss off builders with the class shape.
    assert_eq!(
        uncaught(&mut eval, "(.append \"x\" \"y\")"),
        "java.lang.IllegalArgumentException | No matching method append found taking 1 args for class java.lang.String"
    );
}

#[test]
fn iae_ctor_rows() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str (IllegalArgumentException. \"boom\"))"),
        "\"#error {:cause \\\"boom\\\" :data nil :via [{:type java.lang.IllegalArgumentException :message \\\"boom\\\"}] :trace []}\""
    );
    assert_eq!(
        run(&mut eval, "(str (IllegalArgumentException.))"),
        "\"#error {:cause nil :data nil :via [{:type java.lang.IllegalArgumentException :message nil}] :trace []}\""
    );
    assert_eq!(
        uncaught(&mut eval, "(IllegalArgumentException. nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: java.lang.IllegalArgumentException"
    );
    assert_eq!(
        uncaught(&mut eval, "(IllegalArgumentException. 42)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
    );
    // Causes attach and project.
    assert_eq!(
        run(
            &mut eval,
            "(str (ex-cause (IllegalArgumentException. (IllegalArgumentException. \"inner\"))))"
        ),
        "\"#error {:cause \\\"inner\\\" :data nil :via [{:type java.lang.IllegalArgumentException :message \\\"inner\\\"}] :trace []}\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (IllegalArgumentException. \"m\" (IllegalArgumentException. \"c\")))"
        ),
        "\"#error {:cause \\\"m\\\" :data nil :via [{:type java.lang.IllegalArgumentException :message \\\"m\\\"} {:type java.lang.IllegalArgumentException :message \\\"c\\\"}] :trace []}\""
    );
    assert_eq!(
        run(&mut eval, "(str (IllegalArgumentException. \"m\" nil))"),
        "\"#error {:cause \\\"m\\\" :data nil :via [{:type java.lang.IllegalArgumentException :message \\\"m\\\"}] :trace []}\""
    );
    assert_eq!(
        run(&mut eval, "(str (IllegalArgumentException. nil nil))"),
        "\"#error {:cause nil :data nil :via [{:type java.lang.IllegalArgumentException :message nil}] :trace []}\""
    );
    assert_eq!(
        uncaught(&mut eval, "(IllegalArgumentException. \"m\" 42)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
    );
    // Bad arities fail at compile time.
    assert_eq!(
        chain(&mut eval, "(IllegalArgumentException. 1 2 3)"),
        vec![
            "Syntax error compiling new at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
                .to_owned(),
        ]
    );
    // Thrown values catch by class.
    assert_eq!(
        run(
            &mut eval,
            "(try (throw (IllegalArgumentException. \"boom\")) (catch IllegalArgumentException e (ex-message e)))"
        ),
        "\"boom\""
    );
}
