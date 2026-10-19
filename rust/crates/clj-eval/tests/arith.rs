// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D arithmetic/comparison fixes (coverage-audit findings, oracle
//! fixprobe batches 1–6): `not` truthiness, `max`/`min`
//! latter-wins + `NaN` poison, `float?` ≡ `double?`,
//! `quot`/`rem`/`mod` zero/non-finite rules and int-zero texts,
//! single-arg comparisons passing unchecked.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => panic!("expected an uncaught throw for {src}, got {vs:?}"),
    }
}

#[test]
fn not_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(not false)"), "true");
    assert_eq!(run_last(&mut eval, "(not nil)"), "true");
    assert_eq!(run_last(&mut eval, "(not true)"), "false");
    assert_eq!(run_last(&mut eval, "(not 0)"), "false");
    assert_eq!(run_last(&mut eval, "(not \"\")"), "false");
    assert_eq!(run_last(&mut eval, "(not '())"), "false");
}

#[test]
fn max_min_rows() {
    let mut eval = boot();
    // Ties keep the latter (type included).
    assert_eq!(run_last(&mut eval, "(max 1 1.0)"), "1.0");
    assert_eq!(run_last(&mut eval, "(max 1.0 1)"), "1");
    assert_eq!(run_last(&mut eval, "(min 1 1.0)"), "1.0");
    // `NaN` poisons from either side.
    assert_eq!(run_last(&mut eval, "(max 1 ##NaN)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(max ##NaN 1)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(min 1 ##NaN)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(min ##NaN 1)"), "##NaN");
    // Single args pass through unchecked; 0 args is arity.
    assert_eq!(run_last(&mut eval, "(max 5)"), "5");
    assert_eq!(run_last(&mut eval, "(max :a)"), ":a");
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'max))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/max"
    );
    assert_eq!(
        uncaught(&mut eval, "(eval (list 'min))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/min"
    );
    assert_eq!(run_last(&mut eval, "(max 1 5 3)"), "5");
    assert_eq!(run_last(&mut eval, "(min 2 1.0 3)"), "1.0");
}

#[test]
fn float_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(float? 1.0)"), "true");
    assert_eq!(run_last(&mut eval, "(float? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(double? 1.0)"), "true");
    assert_eq!(run_last(&mut eval, "(double? 1)"), "false");
}

#[test]
fn quot_rem_mod_rows() {
    let mut eval = boot();
    // Integer paths (truncation toward zero; `mod` takes the
    // divisor's sign).
    assert_eq!(run_last(&mut eval, "(quot -5 2)"), "-2");
    assert_eq!(run_last(&mut eval, "(rem -5 2)"), "-1");
    assert_eq!(run_last(&mut eval, "(mod -5 2)"), "1");
    // Integer zero: `quot`/`rem` say `/ by zero`, `mod` says
    // `Divide by zero`.
    assert_eq!(
        uncaught(&mut eval, "(quot 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(rem 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(mod 1 0)"),
        "java.lang.ArithmeticException | Divide by zero"
    );
    // Double normals.
    assert_eq!(run_last(&mut eval, "(quot 5.5 2.0)"), "2.0");
    assert_eq!(run_last(&mut eval, "(rem 5.5 2.0)"), "1.5");
    assert_eq!(run_last(&mut eval, "(mod 5.5 2.0)"), "1.5");
    assert_eq!(run_last(&mut eval, "(mod -5.5 2.0)"), "0.5");
    assert_eq!(run_last(&mut eval, "(rem -5.5 2.0)"), "-1.5");
    assert_eq!(run_last(&mut eval, "(quot 7.0 2)"), "3.0");
    // Double zero (either sign, either position) is `Divide by
    // zero` — even over `NaN` (zero checks first).
    for form in [
        "(quot 1.0 0.0)",
        "(rem 1.0 0.0)",
        "(mod 1.0 0.0)",
        "(mod -1.0 0.0)",
        "(quot 1 0.0)",
        "(quot ##NaN 0.0)",
        "(rem ##NaN 0.0)",
        "(mod ##NaN 0.0)",
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.ArithmeticException | Divide by zero",
            "{form}"
        );
    }
    // Non-finite operands are `Infinite or NaN`.
    for form in [
        "(quot 1.0 ##NaN)",
        "(quot ##NaN 1.0)",
        "(quot 0.0 ##NaN)",
        "(quot 1e308 1e-308)",
        "(rem 1.0 ##NaN)",
        "(rem ##NaN 1.0)",
        "(rem ##Inf 2.0)",
        "(mod 1.0 ##NaN)",
        "(mod ##NaN 1.0)",
        "(mod ##Inf 2.0)",
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.NumberFormatException | Infinite or NaN",
            "{form}"
        );
    }
    // The infinite-divisor values: `quot` truncates to zero,
    // `rem`/`mod` are `NaN`; negative fractions normalize to `+0.0`.
    assert_eq!(run_last(&mut eval, "(quot 2.0 ##Inf)"), "0.0");
    assert_eq!(run_last(&mut eval, "(quot 2.0 ##-Inf)"), "0.0");
    assert_eq!(run_last(&mut eval, "(quot -0.5 2.0)"), "0.0");
    assert_eq!(run_last(&mut eval, "(rem 1.0 ##Inf)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(rem 2.0 ##-Inf)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(rem 0.0 ##Inf)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(mod 2.0 ##Inf)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(mod 0.0 ##Inf)"), "##NaN");
}

#[test]
fn comparison_single_rows() {
    let mut eval = boot();
    // Single args pass unchecked (even non-numerics).
    assert_eq!(run_last(&mut eval, "(== :a)"), "true");
    assert_eq!(run_last(&mut eval, "(== 1)"), "true");
    assert_eq!(run_last(&mut eval, "(< \"s\")"), "true");
    assert_eq!(run_last(&mut eval, "(> nil)"), "true");
    assert_eq!(run_last(&mut eval, "(<= :a)"), "true");
    assert_eq!(run_last(&mut eval, "(>= :a)"), "true");
    assert_eq!(
        uncaught(&mut eval, "(eval (list '==))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/=="
    );
    assert_eq!(
        uncaught(&mut eval, "(eval (list '<))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/<"
    );
    // Two args still cast to `Number`.
    assert_eq!(
        uncaught(&mut eval, "(== :a :b)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class java.lang.Number (clojure.lang.Keyword is in unnamed module of loader 'app'; java.lang.Number is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn div_zero_rows() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(/ 1 0)"),
        "java.lang.ArithmeticException | Divide by zero"
    );
    assert_eq!(run_last(&mut eval, "(/ 1.0 0.0)"), "##Inf");
    assert_eq!(run_last(&mut eval, "(/ 0.0 0.0)"), "##NaN");
    assert_eq!(run_last(&mut eval, "(/ -1.0 0.0)"), "##-Inf");
}

#[test]
fn coerce_rows() {
    let mut eval = boot();
    // `int` stays `Long` (no 32-bit ints) but range-checks exactly.
    assert_eq!(run_last(&mut eval, "(int 5)"), "5");
    assert_eq!(run_last(&mut eval, "(int 2.9)"), "2");
    assert_eq!(run_last(&mut eval, "(int -2.9)"), "-2");
    assert_eq!(run_last(&mut eval, "(int ##NaN)"), "0");
    assert_eq!(run_last(&mut eval, "(int \\a)"), "97");
    assert_eq!(
        uncaught(&mut eval, "(int 3000000000)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    for (form, msg) in [
        ("(int 1e100)", "Value out of range for int: 1.0E100"),
        ("(int ##Inf)", "Value out of range for int: Infinity"),
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            format!("java.lang.IllegalArgumentException | {msg}"),
            "{form}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(int nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(int \"5\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // `long`: `NaN` is 0, out-of-range doubles are IAE.
    assert_eq!(run_last(&mut eval, "(long 2.9)"), "2");
    assert_eq!(run_last(&mut eval, "(long ##NaN)"), "0");
    assert_eq!(run_last(&mut eval, "(long \\a)"), "97");
    assert_eq!(
        uncaught(&mut eval, "(long 1e100)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E100"
    );
    assert_eq!(
        uncaught(&mut eval, "(long nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    // `double`/`float`: characters rejected (unlike `int`/`long`).
    assert_eq!(run_last(&mut eval, "(double 1)"), "1.0");
    assert_eq!(run_last(&mut eval, "(float 1)"), "1.0");
    assert_eq!(run_last(&mut eval, "(float ##NaN)"), "##NaN");
    assert_eq!(
        uncaught(&mut eval, "(float 1e100)"),
        "java.lang.IllegalArgumentException | Value out of range for float: 1.0E100"
    );
    assert_eq!(
        uncaught(&mut eval, "(double \\a)"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // `short`/`byte`: `NaN` is IAE (unlike `int`/`long`).
    assert_eq!(run_last(&mut eval, "(short 5)"), "5");
    assert_eq!(run_last(&mut eval, "(short 2.9)"), "2");
    assert_eq!(run_last(&mut eval, "(short \\a)"), "97");
    assert_eq!(
        uncaught(&mut eval, "(short 70000)"),
        "java.lang.IllegalArgumentException | Value out of range for short: 70000"
    );
    assert_eq!(
        uncaught(&mut eval, "(short ##NaN)"),
        "java.lang.IllegalArgumentException | Value out of range for short: NaN"
    );
    assert_eq!(run_last(&mut eval, "(byte 5)"), "5");
    assert_eq!(
        uncaught(&mut eval, "(byte 200)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: 200"
    );
    assert_eq!(
        uncaught(&mut eval, "(byte ##NaN)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: NaN"
    );
    // `char`: truncates, range-checks, rejects surrogates (the JVM
    // accepts them unprintably — documented divergence).
    assert_eq!(run_last(&mut eval, "(char 65)"), "\\A");
    assert_eq!(run_last(&mut eval, "(char \\A)"), "\\A");
    assert_eq!(run_last(&mut eval, "(char 65.9)"), "\\A");
    for (form, msg) in [
        ("(char 1114112)", "Value out of range for char: 1114112"),
        ("(char ##NaN)", "Value out of range for char: NaN"),
        ("(char ##Inf)", "Value out of range for char: Infinity"),
        ("(char 1e10)", "Value out of range for char: 1.0E10"),
        ("(char -1)", "Value out of range for char: -1"),
        ("(char 55296)", "Value out of range for char: 55296"),
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            format!("java.lang.IllegalArgumentException | {msg}"),
            "{form}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(char nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.longValue()\" because \"x\" is null"
    );
}

#[test]
fn pred_backlog_rows() {
    let mut eval = boot();
    // `/` normals.
    assert_eq!(run_last(&mut eval, "(/ 6 3)"), "2");
    assert_eq!(run_last(&mut eval, "(/ 7 2)"), "3.5");
    assert_eq!(run_last(&mut eval, "(/ 2)"), "0.5");
    // `dec`.
    assert_eq!(run_last(&mut eval, "(dec 1)"), "0");
    assert_eq!(run_last(&mut eval, "(dec 1.5)"), "0.5");
    // `not=`.
    assert_eq!(run_last(&mut eval, "(not= 1 1)"), "false");
    assert_eq!(run_last(&mut eval, "(not= 1 2)"), "true");
    // `>=` multi-arg.
    assert_eq!(run_last(&mut eval, "(>= 2 2 1)"), "true");
    // `pos?`/`neg?` (zero is neither, either sign).
    assert_eq!(run_last(&mut eval, "(pos? 1)"), "true");
    assert_eq!(run_last(&mut eval, "(pos? -1)"), "false");
    assert_eq!(run_last(&mut eval, "(pos? 0)"), "false");
    assert_eq!(run_last(&mut eval, "(pos? 1.5)"), "true");
    assert_eq!(run_last(&mut eval, "(neg? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(neg? -1)"), "true");
    assert_eq!(run_last(&mut eval, "(neg? 0)"), "false");
    assert_eq!(run_last(&mut eval, "(neg? 1.5)"), "false");
    // `compare` ints.
    assert_eq!(run_last(&mut eval, "(compare 1 2)"), "-1");
    assert_eq!(run_last(&mut eval, "(compare 2 1)"), "1");
    assert_eq!(run_last(&mut eval, "(compare 1 1)"), "0");
    // Nil/boolean predicates.
    assert_eq!(run_last(&mut eval, "(some? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(some? false)"), "true");
    assert_eq!(run_last(&mut eval, "(some? true)"), "true");
    assert_eq!(run_last(&mut eval, "(some? 0)"), "true");
    assert_eq!(run_last(&mut eval, "(true? true)"), "true");
    assert_eq!(run_last(&mut eval, "(true? false)"), "false");
    assert_eq!(run_last(&mut eval, "(true? 0)"), "false");
    assert_eq!(run_last(&mut eval, "(true? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(false? false)"), "true");
    assert_eq!(run_last(&mut eval, "(false? true)"), "false");
    assert_eq!(run_last(&mut eval, "(false? 0)"), "false");
    assert_eq!(run_last(&mut eval, "(false? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(boolean? true)"), "true");
    assert_eq!(run_last(&mut eval, "(boolean? false)"), "true");
    assert_eq!(run_last(&mut eval, "(boolean? 0)"), "false");
    assert_eq!(run_last(&mut eval, "(boolean? nil)"), "false");
    // Type predicates over 1/1.0/`\\a`/`:a`.
    assert_eq!(run_last(&mut eval, "(int? 1)"), "true");
    assert_eq!(run_last(&mut eval, "(int? 1.0)"), "false");
    assert_eq!(run_last(&mut eval, "(int? \\a)"), "false");
    assert_eq!(run_last(&mut eval, "(int? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(double? 1.0)"), "true");
    assert_eq!(run_last(&mut eval, "(double? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(double? \\a)"), "false");
    assert_eq!(run_last(&mut eval, "(double? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(number? 1)"), "true");
    assert_eq!(run_last(&mut eval, "(number? 1.0)"), "true");
    assert_eq!(run_last(&mut eval, "(number? \\a)"), "false");
    assert_eq!(run_last(&mut eval, "(number? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(char? \\a)"), "true");
    assert_eq!(run_last(&mut eval, "(char? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(char? 1.0)"), "false");
    assert_eq!(run_last(&mut eval, "(char? :a)"), "false");
    // `ident?` over qualified/simple keywords/symbols + non-idents.
    assert_eq!(run_last(&mut eval, "(ident? :a/a)"), "true");
    assert_eq!(run_last(&mut eval, "(ident? :a)"), "true");
    assert_eq!(run_last(&mut eval, "(ident? 'a/a)"), "true");
    assert_eq!(run_last(&mut eval, "(ident? 'a)"), "true");
    assert_eq!(run_last(&mut eval, "(ident? \"s\")"), "false");
    assert_eq!(run_last(&mut eval, "(ident? 1)"), "false");
    // Qualified/simple × ident/keyword/symbol.
    assert_eq!(run_last(&mut eval, "(qualified-ident? :a/a)"), "true");
    assert_eq!(run_last(&mut eval, "(qualified-ident? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(simple-ident? :a)"), "true");
    assert_eq!(run_last(&mut eval, "(simple-ident? :a/a)"), "false");
    assert_eq!(run_last(&mut eval, "(qualified-keyword? :a/a)"), "true");
    assert_eq!(run_last(&mut eval, "(qualified-keyword? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(qualified-keyword? 'a/a)"), "false");
    assert_eq!(run_last(&mut eval, "(qualified-symbol? 'a/a)"), "true");
    assert_eq!(run_last(&mut eval, "(qualified-symbol? 'a)"), "false");
    assert_eq!(run_last(&mut eval, "(qualified-symbol? :a/a)"), "false");
    assert_eq!(run_last(&mut eval, "(simple-keyword? :a)"), "true");
    assert_eq!(run_last(&mut eval, "(simple-keyword? :a/a)"), "false");
    assert_eq!(run_last(&mut eval, "(simple-keyword? 'a)"), "false");
    assert_eq!(run_last(&mut eval, "(simple-symbol? 'a)"), "true");
    assert_eq!(run_last(&mut eval, "(simple-symbol? 'a/a)"), "false");
    assert_eq!(run_last(&mut eval, "(simple-symbol? :a)"), "false");
    assert_eq!(run_last(&mut eval, "(keyword? :a)"), "true");
    assert_eq!(run_last(&mut eval, "(keyword? 'a)"), "false");
    assert_eq!(run_last(&mut eval, "(symbol? 'a)"), "true");
    assert_eq!(run_last(&mut eval, "(symbol? :a)"), "false");
    // Collection/callable predicates.
    assert_eq!(run_last(&mut eval, "(list? '(1))"), "true");
    assert_eq!(run_last(&mut eval, "(list? [1])"), "false");
    assert_eq!(run_last(&mut eval, "(set? #{1})"), "true");
    assert_eq!(run_last(&mut eval, "(set? [1])"), "false");
    assert_eq!(run_last(&mut eval, "(fn? inc)"), "true");
    assert_eq!(run_last(&mut eval, "(fn? 1)"), "false");
    // NOTE: `#'x` alone does not compile (`x` is unresolvable), so
    // the var exists first; the pinned behavior is `(var? #'x)`.
    assert_eq!(run_last(&mut eval, "(do (def x 1) (var? #'x))"), "true");
    assert_eq!(run_last(&mut eval, "(var? 1)"), "false");
    assert_eq!(run_last(&mut eval, "(record? {})"), "false");
}

#[test]
fn str_fn_rows() {
    let mut eval = boot();
    // `keyword` (fixprobe8-pinned): non-idents coerce, the rest is nil.
    assert_eq!(run_last(&mut eval, "(keyword :a)"), ":a");
    assert_eq!(run_last(&mut eval, "(keyword \"a\")"), ":a");
    assert_eq!(run_last(&mut eval, "(keyword \"n\" \"a\")"), ":n/a");
    assert_eq!(run_last(&mut eval, "(keyword 'a)"), ":a");
    assert_eq!(run_last(&mut eval, "(keyword nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(keyword 5)"), "nil");
    // `symbol`: nil/numbers have no conversion (IAE text pinned).
    assert_eq!(run_last(&mut eval, "(symbol \"a\")"), "a");
    assert_eq!(run_last(&mut eval, "(symbol \"n\" \"a\")"), "n/a");
    assert_eq!(run_last(&mut eval, "(symbol :a)"), "a");
    assert_eq!(
        uncaught(&mut eval, "(symbol nil)"),
        "java.lang.IllegalArgumentException | no conversion to symbol"
    );
    assert_eq!(
        uncaught(&mut eval, "(symbol 5)"),
        "java.lang.IllegalArgumentException | no conversion to symbol"
    );
    // `name`/`namespace` (+ the nil NPE texts).
    assert_eq!(run_last(&mut eval, "(name :a/a)"), "\"a\"");
    assert_eq!(run_last(&mut eval, "(name 'x)"), "\"x\"");
    assert_eq!(run_last(&mut eval, "(name \"s\")"), "\"s\"");
    assert_eq!(
        uncaught(&mut eval, "(name nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getName()\" because \"x\" is null"
    );
    assert_eq!(run_last(&mut eval, "(namespace :a/a)"), "\"a\"");
    assert_eq!(run_last(&mut eval, "(namespace :a)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(namespace nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null"
    );
    // `pr-str` joins `pr` spellings with spaces.
    assert_eq!(
        run_last(&mut eval, "(pr-str 1 \"a\" :k)"),
        "\"1 \\\"a\\\" :k\""
    );
}

#[test]
fn seq_direct_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(rest nil)"), "()");
    assert_eq!(run_last(&mut eval, "(next nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(vals {:a 1})"), "(1)");
    assert_eq!(run_last(&mut eval, "(rest [1 2])"), "(2)");
    assert_eq!(run_last(&mut eval, "(next [1])"), "nil");
}
