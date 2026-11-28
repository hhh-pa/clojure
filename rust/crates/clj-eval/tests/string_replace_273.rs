// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Issue #273: `clojure.string/replace` and `replace-first`, `re-seq`,
//! `split`, and the `Matcher` / `StringBuffer` emulation against JVM
//! Clojure 1.13.0-alpha10 on JDK 26. Every row quotes a probe of
//! `docs/spec/i273-string-replace/probes/{replace,interop}.clj` and its
//! JVM answer from the matching `.out` file (`pr-str` of the value, or
//! `Class: message` of the throw). One row is left out: `e21`, whose JVM
//! NPE text comes from the `^CharSequence`-hinted `.toString` in
//! `string.clj`, a hinted-call message this runtime words generically.

use clj_eval::{Evaluator, boot};
use clj_value::Value;

/// The probes' own shape: `pr-str` of the value, or the throw's class
/// and message.
fn outcome(eval: &mut Evaluator, expr: &str) -> String {
    let src = format!(
        "(try (pr-str {expr}) (catch Throwable e (str (.getName (class e)) \": \" (.getMessage e))))"
    );
    let values = eval.eval_source(&src).expect("the probe evaluates");
    match values.last() {
        Some(Value::Str(s)) => s.to_string(),
        other => panic!("{expr}: {other:?}"),
    }
}

/// An evaluator with the probes' requires (bare `eval` needs the
/// driver's `with-bindings` frame for `string.clj`'s `set!`).
fn probe_eval() -> Evaluator {
    let mut eval = boot();
    eval.eval_source(
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require '[clojure.string :as str]))",
    )
    .expect("clojure.string loads");
    eval.eval_source("(import '[java.util.regex Matcher])")
        .expect("Matcher imports");
    eval
}

/// `(probe, label, expression, JVM answer)`.
const ORACLE: &[(&str, &str, &str, &str)] = &[
    (
        "replace",
        "r1 escape$",
        r##"(str/replace "abc" #"b" "\\$")"##,
        r##""a$c""##,
    ),
    (
        "replace",
        "r2 $10",
        r##"(str/replace "abc" #"(b)" "$10")"##,
        r##""ab0c""##,
    ),
    (
        "replace",
        "r3 a*",
        r##"(str/replace "aaa" #"a*" "-")"##,
        r##""--""##,
    ),
    (
        "replace",
        "r4 empty",
        r##"(str/replace "abc" #"" "-")"##,
        r##""-a-b-c-""##,
    ),
    (
        "replace",
        "r5 nomatch-bad",
        r##"(str/replace "abc" #"x" "$")"##,
        r##""abc""##,
    ),
    (
        "replace",
        "r6 trailing$",
        r##"(str/replace "abc" #"b" "$")"##,
        r##"java.lang.IllegalArgumentException: Illegal group reference: group index is missing"##,
    ),
    (
        "replace",
        "r7 trailing-backslash",
        r##"(str/replace "abc" #"b" "\\")"##,
        r##"java.lang.IllegalArgumentException: character to be escaped is missing"##,
    ),
    (
        "replace",
        "r8 $2",
        r##"(str/replace "abc" #"b" "$2")"##,
        r##"java.lang.IndexOutOfBoundsException: No group 2"##,
    ),
    (
        "replace",
        "r9 $a",
        r##"(str/replace "abc" #"b" "$a")"##,
        r##"java.lang.IllegalArgumentException: Illegal group reference"##,
    ),
    (
        "replace",
        "r10 named",
        r##"(str/replace "abc" #"(?<x>b)" "[${x}]")"##,
        r##""a[b]c""##,
    ),
    (
        "replace",
        "r11 named-missing",
        r##"(str/replace "abc" #"(?<x>b)" "${y}")"##,
        r##"java.lang.IllegalArgumentException: No group with name {y}"##,
    ),
    (
        "replace",
        "r12 named-empty",
        r##"(str/replace "abc" #"(?<x>b)" "${}")"##,
        r##"java.lang.IllegalArgumentException: named capturing group has 0 length name"##,
    ),
    (
        "replace",
        "r13 named-unclosed",
        r##"(str/replace "abc" #"(?<x>b)" "${x")"##,
        r##"java.lang.IllegalArgumentException: named capturing group is missing trailing '}'"##,
    ),
    (
        "replace",
        "r14 named-digit",
        r##"(str/replace "abc" #"(b)" "${1}")"##,
        r##"java.lang.IllegalArgumentException: capturing group name {1} starts with digit character"##,
    ),
    (
        "replace",
        "r15 double",
        r##"(str/replace "a1b2" #"(\d)" "<$1$1>")"##,
        r##""a<11>b<22>""##,
    ),
    (
        "replace",
        "r16 unmatched",
        r##"(str/replace "abc" #"(x)?b" "[$1]")"##,
        r##""a[]c""##,
    ),
    (
        "replace",
        "r17 order",
        r##"(str/replace "abcabc" #"(a)(b)(c)" "$3$2$1$0")"##,
        r##""cbaabccbaabc""##,
    ),
    (
        "replace",
        "r18 12groups",
        r##"(str/replace "abcdefghijkl" #"(a)(b)(c)(d)(e)(f)(g)(h)(i)(j)(k)(l)" "$12|$13|$10")"##,
        r##""l|a3|j""##,
    ),
    (
        "replace",
        "r19 escapes",
        r##"(str/replace "x" #"x" "\\a\\$\\\\")"##,
        r##""a$\\""##,
    ),
    (
        "replace",
        "r20 named-bad-char",
        r##"(str/replace "abc" #"(?<x>b)" "${x-y}")"##,
        r##"java.lang.IllegalArgumentException: named capturing group is missing trailing '}'"##,
    ),
    (
        "replace",
        "r21 $ then {",
        r##"(str/replace "abc" #"b" "${")"##,
        r##"java.lang.IllegalArgumentException: named capturing group has 0 length name"##,
    ),
    (
        "replace",
        "r22 unicode",
        r##"(str/replace "héllo wörld" #"(\w+)" "<$1>")"##,
        r##""<h>é<llo> <w>ö<rld>""##,
    ),
    (
        "replace",
        "r23 a*b",
        r##"(str/replace "baaac" #"a*" "-")"##,
        r##""-b--c-""##,
    ),
    (
        "replace",
        "f1 str",
        r##"(str/replace "a1b22" #"\d+" str)"##,
        r##""a1b22""##,
    ),
    (
        "replace",
        "f2 groups",
        r##"(str/replace "a1b22" #"(\d)(\d)?" (fn [[m a b]] (str "<" a "|" b ">")))"##,
        r##""a<1|>b<2|2>""##,
    ),
    (
        "replace",
        "f3 empty-matches",
        r##"(str/replace "aaa" #"a*" (fn [m] (str "<" m ">")))"##,
        r##""<aaa><>""##,
    ),
    (
        "replace",
        "f4 quoted",
        r##"(str/replace "ab" #"b" (fn [_] "$1\\"))"##,
        r##""a$1\\""##,
    ),
    (
        "replace",
        "f5 number",
        r##"(str/replace "a1" #"\d" (fn [_] 5))"##,
        r##"java.lang.ClassCastException: class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"##,
    ),
    (
        "replace",
        "f6 nil",
        r##"(str/replace "a1" #"\d" (fn [_] nil))"##,
        r##"java.lang.NullPointerException: Cannot invoke "String.indexOf(int)" because "s" is null"##,
    ),
    (
        "replace",
        "f7 nomatch",
        r##"(str/replace "abc" #"\d" (fn [_] 5))"##,
        r##""abc""##,
    ),
    (
        "replace",
        "f8 empty",
        r##"(str/replace "ab" #"" (fn [m] "-"))"##,
        r##""-a-b-""##,
    ),
    (
        "replace",
        "q1 replace-first fn",
        r##"(str/replace-first "a1b22" #"\d+" str)"##,
        r##""a1b22""##,
    ),
    (
        "replace",
        "q2 replace-first fn2",
        r##"(str/replace-first "a1b22" #"\d+" (fn [m] (str "<" m ">")))"##,
        r##""a<1>b22""##,
    ),
    (
        "replace",
        "q3 replace-first $10",
        r##"(str/replace-first "abc" #"(b)" "$10")"##,
        r##""ab0c""##,
    ),
    (
        "replace",
        "q4 replace-first a*",
        r##"(str/replace-first "aaa" #"a*" "-")"##,
        r##""-""##,
    ),
    (
        "replace",
        "q5 replace-first nomatch-bad",
        r##"(str/replace-first "abc" #"x" "$")"##,
        r##""abc""##,
    ),
    (
        "replace",
        "q6 replace-first empty",
        r##"(str/replace-first "abc" #"" "-")"##,
        r##""-abc""##,
    ),
    (
        "replace",
        "q7 replace-first escape",
        r##"(str/replace-first "abcb" #"b" "\\$")"##,
        r##""a$cb""##,
    ),
    (
        "replace",
        "q8 replace-first number",
        r##"(str/replace-first "a1" #"\d" (fn [_] 5))"##,
        r##"java.lang.ClassCastException: class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"##,
    ),
    (
        "replace",
        "q9 replace-first fn nomatch",
        r##"(str/replace-first "abc" #"\d" (fn [_] 5))"##,
        r##""abc""##,
    ),
    (
        "replace",
        "q10 replace-first char",
        r##"(str/replace-first "abcb" \b \x)"##,
        r##""axcb""##,
    ),
    (
        "replace",
        "q11 replace-first str",
        r##"(str/replace-first "abcb" "b" "$x")"##,
        r##""a$xcb""##,
    ),
    (
        "replace",
        "m1 replaceAll",
        r##"(.replaceAll (re-matcher #"a*" "aaa") "-")"##,
        r##""--""##,
    ),
    (
        "replace",
        "m2 replaceFirst",
        r##"(.replaceFirst (re-matcher #"a" "aaa") "-")"##,
        r##""-aa""##,
    ),
    (
        "replace",
        "m3 append loop StringBuffer",
        r##"(let [m (re-matcher #"\d" "a1b2c") sb (StringBuffer.)] (while (.find m) (.appendReplacement m sb "<$0>")) (.appendTail m sb) (str sb))"##,
        r##""a<1>b<2>c""##,
    ),
    (
        "replace",
        "m4 append loop StringBuilder",
        r##"(let [m (re-matcher #"\d" "a1b2c") sb (StringBuilder.)] (while (.find m) (.appendReplacement m sb "<$0>")) (.appendTail m sb) (str sb))"##,
        r##""a<1>b<2>c""##,
    ),
    (
        "replace",
        "m5 append no match",
        r##"(let [m (re-matcher #"\d" "abc") sb (StringBuffer.)] (.appendReplacement m sb "x"))"##,
        r##"java.lang.IllegalStateException: No match found"##,
    ),
    (
        "replace",
        "m6 quoteReplacement",
        r##"(java.util.regex.Matcher/quoteReplacement "a$b\\c")"##,
        r##""a\\$b\\\\c""##,
    ),
    (
        "replace",
        "m7 re-quote-replacement",
        r##"(str/re-quote-replacement "a$b\\c")"##,
        r##""a\\$b\\\\c""##,
    ),
    (
        "replace",
        "m8 StringBuffer int",
        r##"(let [sb (StringBuffer. 16)] (.append sb "x") (.append sb 1) (str sb (.length sb)))"##,
        r##""x12""##,
    ),
    (
        "replace",
        "m9 StringBuffer str",
        r##"(str (doto (StringBuffer. "ab") (.append \c)))"##,
        r##""abc""##,
    ),
    (
        "replace",
        "m10 appendTail only",
        r##"(let [m (re-matcher #"\d" "abc") sb (StringBuffer.)] (str (.appendTail m sb)))"##,
        r##""abc""##,
    ),
    (
        "replace",
        "m11 replaceAll after find",
        r##"(let [m (re-matcher #"a" "aXa")] (.find m) (.replaceAll m "b"))"##,
        r##""bXb""##,
    ),
    (
        "replace",
        "m12 append partial",
        r##"(let [m (re-matcher #"\d" "a1b2c") sb (StringBuffer.)] (.find m) (.appendReplacement m sb "<$0>") (.appendTail m sb) (str sb))"##,
        r##""a<1>b2c""##,
    ),
    (
        "replace",
        "m13 replaceAll bad template",
        r##"(.replaceAll (re-matcher #"b" "abc") "$9")"##,
        r##"java.lang.IndexOutOfBoundsException: No group 9"##,
    ),
    (
        "replace",
        "m14 appendReplacement returns",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer.)] (.find m) (identical? m (.appendReplacement m sb "x")))"##,
        r##"true"##,
    ),
    (
        "replace",
        "s1 re-seq a*",
        r##"(re-seq #"a*" "aaa")"##,
        r##"("aaa" "")"##,
    ),
    (
        "replace",
        "s2 re-seq a* mid",
        r##"(re-seq #"a*" "baaac")"##,
        r##"("" "aaa" "" "")"##,
    ),
    (
        "replace",
        "s3 split a*",
        r##"(str/split "baaac" #"a*")"##,
        r##"["b" "" "c"]"##,
    ),
    (
        "interop",
        "e1 quote nil",
        r##"(Matcher/quoteReplacement (identity nil))"##,
        r##"java.lang.NullPointerException: Cannot invoke "String.indexOf(int)" because "s" is null"##,
    ),
    (
        "interop",
        "e2 quote num",
        r##"(Matcher/quoteReplacement 5)"##,
        r##"java.lang.ClassCastException: class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"##,
    ),
    (
        "interop",
        "e3 append nil sb",
        r##"(let [m (re-matcher #"b" "abc")] (.find m) (.appendReplacement m (identity nil) "x"))"##,
        r##"java.lang.NullPointerException: Cannot invoke "java.lang.StringBuffer.length()" because "sb" is null"##,
    ),
    (
        "interop",
        "e4 append nil rep",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer.)] (.find m) (.appendReplacement m sb (identity nil)))"##,
        r##"java.lang.NullPointerException: Cannot invoke "String.length()" because "replacement" is null"##,
    ),
    (
        "interop",
        "e5 append num rep",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer.)] (.find m) (.appendReplacement m sb 5))"##,
        r##"java.lang.IllegalArgumentException: No matching method appendReplacement found taking 2 args for class java.util.regex.Matcher"##,
    ),
    (
        "interop",
        "e6 tail nil",
        r##"(.appendTail (re-matcher #"b" "abc") (identity nil))"##,
        r##"java.lang.NullPointerException: Cannot invoke "java.lang.StringBuffer.append(java.lang.CharSequence, int, int)" because "sb" is null"##,
    ),
    (
        "interop",
        "e7 replaceFirst nil",
        r##"(.replaceFirst (re-matcher #"b" "abc") (identity nil))"##,
        r##"java.lang.NullPointerException: "##,
    ),
    (
        "interop",
        "e8 replaceFirst num",
        r##"(.replaceFirst (re-matcher #"b" "abc") 5)"##,
        r##"java.lang.IllegalArgumentException: No matching method replaceFirst found taking 1 args for class java.util.regex.Matcher"##,
    ),
    (
        "interop",
        "e9 StringBuffer nil",
        r##"(StringBuffer. (identity nil))"##,
        r##"java.lang.NullPointerException: Cannot invoke "java.lang.CharSequence.length()" because "seq" is null"##,
    ),
    (
        "interop",
        "e10 StringBuffer neg",
        r##"(StringBuffer. -1)"##,
        r##"java.lang.NegativeArraySizeException: -1"##,
    ),
    (
        "interop",
        "e11 StringBuffer print",
        r##"(let [s (pr-str (StringBuffer. "ab"))] (subs s 0 26))"##,
        r##""#object[java.lang.StringBu""##,
    ),
    (
        "interop",
        "e12 class",
        r##"(class (StringBuffer.))"##,
        r##"java.lang.StringBuffer"##,
    ),
    (
        "interop",
        "e13 instance CharSequence",
        r##"(instance? CharSequence (StringBuffer. "x"))"##,
        r##"true"##,
    ),
    (
        "interop",
        "e14 append bad template keeps sb",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer. "Z")] (.find m) (try (.appendReplacement m sb "$9") (catch Exception _ nil)) (str sb))"##,
        r##""Za""##,
    ),
    (
        "interop",
        "e15 tail after replaceAll",
        r##"(let [m (re-matcher #"b" "abcb") sb (StringBuffer.)] (.replaceAll m "x") (str (.appendTail m sb)))"##,
        r##""""##,
    ),
    (
        "interop",
        "e16 append twice same match",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer.)] (.find m) (.appendReplacement m sb "x") (.appendReplacement m sb "y") (str sb))"##,
        r##"java.lang.IndexOutOfBoundsException: Range [2, 1) out of bounds for length 3"##,
    ),
    (
        "interop",
        "e17 find then replaceFirst then tail",
        r##"(let [m (re-matcher #"b" "abcb") sb (StringBuffer.)] (.find m) (.replaceFirst m "x") (str (.appendTail m sb)))"##,
        r##""cb""##,
    ),
    (
        "interop",
        "e18 str/replace sb subject",
        r##"(clojure.string/replace (StringBuffer. "abc") #"b" "x")"##,
        r##""axc""##,
    ),
    (
        "interop",
        "e19 fn returns sb",
        r##"(clojure.string/replace "abc" #"b" (fn [_] (StringBuilder. "q")))"##,
        r##"java.lang.ClassCastException: class java.lang.StringBuilder cannot be cast to class java.lang.String (java.lang.StringBuilder and java.lang.String are in module java.base of loader 'bootstrap')"##,
    ),
    (
        "interop",
        "e20 fn returns char",
        r##"(clojure.string/replace "abc" #"b" (fn [_] \q))"##,
        r##"java.lang.ClassCastException: class java.lang.Character cannot be cast to class java.lang.String (java.lang.Character and java.lang.String are in module java.base of loader 'bootstrap')"##,
    ),
    (
        "interop",
        "e22 tail returns same",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer.)] (identical? sb (.appendTail m sb)))"##,
        r##"true"##,
    ),
    (
        "interop",
        "e23 append StringBuilder after find",
        r##"(let [m (re-matcher #"(b)" "abc") sb (StringBuilder. "<")] (.find m) (.appendReplacement m sb "[$1]") (.appendTail m sb) (str sb))"##,
        r##""<a[b]c""##,
    ),
    (
        "interop",
        "e24 multiline anchors",
        r##"(clojure.string/replace "ab\ncd" #"(?m)^" "> ")"##,
        r##""> ab\n> cd""##,
    ),
    (
        "interop",
        "e25 append IAE rolls back",
        r##"(let [m (re-matcher #"b" "abc") sb (StringBuffer. "Z")] (.find m) (try (.appendReplacement m sb "x$") (catch Exception _ nil)) (str sb))"##,
        r##""Z""##,
    ),
    (
        "interop",
        "e26 append non-buffer",
        r##"(let [m (re-matcher #"b" "abc")] (.find m) (.appendReplacement m (identity "sb") "x"))"##,
        r##"java.lang.IllegalArgumentException: No matching method appendReplacement found taking 2 args for class java.util.regex.Matcher"##,
    ),
    (
        "interop",
        "e27 tail non-buffer",
        r##"(.appendTail (re-matcher #"b" "abc") (identity "sb"))"##,
        r##"java.lang.IllegalArgumentException: No matching method appendTail found taking 1 args for class java.util.regex.Matcher"##,
    ),
    (
        "interop",
        "e28 quote plain",
        r##"(Matcher/quoteReplacement "plain")"##,
        r##""plain""##,
    ),
    (
        "interop",
        "e29 StringBuffer double",
        r##"(StringBuffer. (identity 1.5))"##,
        r##"java.lang.IllegalArgumentException: No matching ctor found for class java.lang.StringBuffer"##,
    ),
    (
        "interop",
        "e30 StringBuffer of builder",
        r##"(str (StringBuffer. (StringBuilder. "sb")))"##,
        r##""sb""##,
    ),
    (
        "interop",
        "e31 re-seq multiline",
        r##"(re-seq #"(?m)^\w*" "ab\n\ncd")"##,
        r##"("ab" "" "cd")"##,
    ),
    (
        "interop",
        "e32 split empty-capable",
        r##"(clojure.string/split "a1b22c" #"\d*")"##,
        r##"["a" "" "b" "" "c"]"##,
    ),
    (
        "interop",
        "e33 replace-first multiline",
        r##"(clojure.string/replace-first "ab\ncd" #"(?m)$" "!")"##,
        r##""ab!\ncd""##,
    ),
    (
        "interop",
        "e34 replaceAll template on no match",
        r##"(.replaceAll (re-matcher #"z" "abc") "\\")"##,
        r##""abc""##,
    ),
    (
        "interop",
        "e35 append matcher as sb",
        r##"(let [m (re-matcher #"b" "abc")] (.find m) (.appendReplacement m m "x"))"##,
        r##"java.lang.IllegalArgumentException: No matching method appendReplacement found taking 2 args for class java.util.regex.Matcher"##,
    ),
];

#[test]
fn replace_and_matcher_answer_like_the_jvm_273() {
    let mut eval = probe_eval();
    let wrong: Vec<String> = ORACLE
        .iter()
        .filter_map(|(probe, label, expr, want)| {
            let got = outcome(&mut eval, expr);
            (got != *want).then(|| format!("{probe} {label}: got {got}, JVM {want}"))
        })
        .collect();
    assert!(wrong.is_empty(), "{}", wrong.join("\n"));
}

#[test]
fn matcher_static_miss_273() {
    // No JVM row (the JVM rejects it at compile time with the same
    // text): an unknown `Matcher` static misses uniformly.
    let mut eval = probe_eval();
    assert_eq!(
        outcome(&mut eval, "(Matcher/nope 1)"),
        "java.lang.IllegalArgumentException: No matching method nope found taking 1 args for class java.util.regex.Matcher"
    );
}
