// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S11 extended arith (issue #56): probe replay for
//! `+'`/`-'`/`*'`/`dec'`/`inc'`, the 21 `unchecked-*`, and
//! the 12 bitwise fns (see
//! `docs/zero-oos-s11-arith/probes/*.{clj,out}`). Every oracle
//! label replays below; the documented port mappings (runtime
//! arity errors for the `:inline`d fns, no
//! `Integer`/`Short`/`Byte`/`Float` kinds, exact-double
//! `unchecked-float`, surrogate `unchecked-char`) are pinned
//! with the upstream value in the comment.

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
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(r) => {
                        let _ = pr(&r);
                    }
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

fn check_rows(eval: &mut Evaluator, rows: &[(&str, &str)]) {
    for (src, want) in rows {
        assert_eq!(run_last(eval, src), (*want).to_owned(), "for {src}");
    }
}

fn check_throws(eval: &mut Evaluator, rows: &[(&str, &str)]) {
    for (src, want) in rows {
        assert_eq!(uncaught(eval, src), (*want).to_owned(), "for {src}");
    }
}

/// The same-module (`java.*` to `java.*`) cast text.
fn cce(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} and {to} are in module java.base of loader 'bootstrap')"
    )
}

fn arity(name: &str, n: usize) -> String {
    format!(
        "clojure.lang.ArityException | Wrong number of args ({n}) passed to: clojure.core/{name}"
    )
}

const NPE_GETCLASS: &str =
    "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null";
const NPE_CHAR_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null";
const NPE_LONG_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.longValue()\" because \"x\" is null";
const NPE_DOUBLE_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null";
const NPE_FLOAT_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.floatValue()\" because \"x\" is null";
const NPE_SHORT_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.shortValue()\" because \"x\" is null";
const NPE_BYTE_VALUE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.byteValue()\" because \"x\" is null";

/// `inc'` (promotion fix) and `dec'` (probes `ip-*`, `dp-*`).
#[test]
fn s11_prime_steps_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(inc' 5)", "6"),                                        // ip-plain
            ("(inc' 9223372036854775807)", "9223372036854775808N"),   // ip-max
            ("(inc' -9223372036854775808)", "-9223372036854775807"),  // ip-min
            ("(inc' -1)", "0"),                                       // ip-neg
            ("(inc' 1.5)", "2.5"),                                    // ip-dbl
            ("(inc' 5N)", "6N"),                                      // ip-big
            ("(inc' 1/2)", "3/2"),                                    // ip-rat
            ("(inc' 5M)", "6M"),                                      // ip-dec
            ("(inc' 0.1M)", "1.1M"),                                  // ip-bigdec
            ("(dec' 5)", "4"),                                        // dp-plain
            ("(dec' -9223372036854775808)", "-9223372036854775809N"), // dp-min
            ("(dec' 9223372036854775807)", "9223372036854775806"),    // dp-max
            ("(dec' -1)", "-2"),                                      // dp-neg
            ("(dec' 1.5)", "0.5"),                                    // dp-dbl
            ("(dec' 5N)", "4N"),                                      // dp-big
            ("(dec' 1/2)", "-1/2"),                                   // dp-rat
            ("(dec' 5M)", "4M"),                                      // dp-dec
            ("(dec' 0.1M)", "-0.9M"),                                 // dp-bigdec
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    let char_num = cce("java.lang.Character", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            ("(inc' nil)", NPE_GETCLASS), // ip-nil
            ("(inc' \"a\")", &str_num),   // ip-str
            ("(inc' \\a)", &char_num),    // ip-char
            ("(dec' nil)", NPE_GETCLASS), // dp-nil
            ("(dec' \"a\")", &str_num),   // dp-str
        ],
    );
    // Upstream arity failures are CompilerException (inlined);
    // the port reports the runtime shape (documented mapping).
    check_throws(
        &mut eval,
        &[
            ("(inc')", &arity("inc'", 0)),     // ip-ar0
            ("(inc' 1 2)", &arity("inc'", 2)), // ip-ar2
            ("(dec')", &arity("dec'", 0)),     // dp-ar0
            ("(dec' 1 2)", &arity("dec'", 2)), // dp-ar2
        ],
    );
}

/// `+'` (probes `ap-*`, `a2-add*`, `g-mulnil`).
#[test]
fn s11_add_prime_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(+' )", "0"),                                            // ap-ar0
            ("(+' 5)", "5"),                                           // ap-ar1i
            ("(+' 1.5)", "1.5"),                                       // ap-ar1d
            ("(+' 5N)", "5N"),                                         // ap-ar1n
            ("(+' nil)", "nil"), // ap-ar1nil (cast passes nil)
            ("(+' 1/2)", "1/2"), // a2-addrat
            ("(+' 1 2)", "3"),   // ap-plain
            ("(+' 9223372036854775807 1)", "9223372036854775808N"), // ap-max1
            ("(+' -9223372036854775808 -1)", "-9223372036854775809N"), // ap-min-1
            (
                "(+' 9223372036854775807 9223372036854775807)",
                "18446744073709551614N",
            ), // ap-maxmax
            ("(+' 1 1/2)", "3/2"), // ap-mixed-ir
            ("(+' 1/2 5N)", "11/2"), // ap-mixed-rn
            ("(+' 5N 5M)", "10M"), // ap-mixed-nm
            ("(+' 5M 0.5)", "5.5"), // ap-mixed-md
            ("(+' 1 0.5)", "1.5"), // ap-mixed-id
            ("(+' 9223372036854775807 1 1)", "9223372036854775809N"), // ap-fold
            ("(+' 1 2 3 4)", "10"), // ap-fold3
            (
                "(+' 1.7976931348623157E308 1.7976931348623157E308)",
                "##Inf",
            ), // ap-dblinf
            ("(+' ##NaN 1)", "##NaN"), // ap-nan
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            (
                "(+' \"a\")",
                "java.lang.ClassCastException | Cannot cast java.lang.String to java.lang.Number",
            ), // ap-ar1s (cast)
            (
                "(+' \\a)",
                "java.lang.ClassCastException | Cannot cast java.lang.Character to java.lang.Number",
            ), // a2-addchar
            (
                "(+' true)",
                "java.lang.ClassCastException | Cannot cast java.lang.Boolean to java.lang.Number",
            ), // a2-addtrue
            ("(+' nil 1)", NPE_GETCLASS), // ap-nil1
            ("(+' 1 nil)", NPE_GETCLASS), // ap-nil2
            ("(+' \"a\" 1)", &str_num),   // ap-str1
            ("(+' 1 \"a\")", &str_num),   // ap-str2
        ],
    );
}

/// `-'` (probes `sp-*`, `a2-subchar`).
#[test]
fn s11_sub_prime_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(-' 5)", "-5"),                                           // sp-neg5
            ("(-' -9223372036854775808)", "9223372036854775808N"),      // sp-negmin
            ("(-' 9223372036854775807)", "-9223372036854775807"),       // sp-negmax
            ("(-' 1.5)", "-1.5"),                                       // sp-ar1d
            ("(-' 5N)", "-5N"),                                         // sp-ar1n
            ("(-' 1/2)", "-1/2"),                                       // sp-ar1r
            ("(-' 5M)", "-5M"),                                         // sp-ar1m
            ("(-' 5 3)", "2"),                                          // sp-plain
            ("(-' -9223372036854775808 1)", "-9223372036854775809N"),   // sp-min1
            ("(-' 9223372036854775807 -1)", "9223372036854775808N"),    // sp-max-1
            ("(-' 1 1/2)", "1/2"),                                      // sp-mixed-ir
            ("(-' 1/2 5N)", "-9/2"),                                    // sp-mixed-rn
            ("(-' 5N 5M)", "0M"),                                       // sp-mixed-nm
            ("(-' 5M 0.5)", "4.5"),                                     // sp-mixed-md
            ("(-' -9223372036854775808 1 1)", "-9223372036854775810N"), // sp-fold
            ("(-' 10 1 2 3)", "4"),                                     // sp-fold3
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    let char_num = cce("java.lang.Character", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            ("(-')", &arity("-'", 0)),    // sp-ar0
            ("(-' \"a\")", &str_num),     // sp-ar1s
            ("(-' nil)", NPE_GETCLASS),   // sp-ar1nil
            ("(-' \\a)", &char_num),      // a2-subchar
            ("(-' nil 1)", NPE_GETCLASS), // sp-nil1
            ("(-' 1 \"a\")", &str_num),   // sp-str2
        ],
    );
}

/// `*'` (probes `mp-*`, `a2-mul*`, `g-mulnil`).
#[test]
fn s11_mul_prime_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(*' )", "1"),                                            // mp-ar0
            ("(*' 5)", "5"),                                           // mp-ar1i
            ("(*' nil)", "nil"),                                       // g-mulnil (cast passes nil)
            ("(*' 5M)", "5M"),                                         // a2-muldec
            ("(*' 6 7)", "42"),                                        // mp-plain
            ("(*' 9223372036854775807 2)", "18446744073709551614N"),   // mp-max2
            ("(*' -9223372036854775808 -1)", "9223372036854775808N"),  // mp-min-1
            ("(*' -9223372036854775808 2)", "-18446744073709551616N"), // mp-min2
            ("(*' 2 1/2)", "1N"),                                      // mp-mixed-ir
            ("(*' 1/2 5N)", "5/2"),                                    // mp-mixed-rn
            ("(*' 5N 5M)", "25M"),                                     // mp-mixed-nm
            ("(*' 5M 0.5)", "2.5"),                                    // mp-mixed-md
            ("(*' 9223372036854775807 2 2)", "36893488147419103228N"), // mp-fold
            ("(*' 5 0)", "0"),                                         // mp-zero
            ("(*' 1.7976931348623157E308 2)", "##Inf"),                // mp-dblinf
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            (
                "(*' \"a\")",
                "java.lang.ClassCastException | Cannot cast java.lang.String to java.lang.Number",
            ), // mp-ar1s (cast)
            (
                "(*' \\a)",
                "java.lang.ClassCastException | Cannot cast java.lang.Character to java.lang.Number",
            ), // a2-mulchar
            (
                "(*' true)",
                "java.lang.ClassCastException | Cannot cast java.lang.Boolean to java.lang.Number",
            ), // a2-multrue
            ("(*' nil 1)", NPE_GETCLASS), // mp-nil1
            ("(*' 1 \"a\")", &str_num),   // mp-str2
        ],
    );
}

/// `*math-context*` consultation by the promoting and
/// unchecked long ops (probes `mc-*`, `xm-*`).
#[test]
fn s11_prime_context_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(with-precision 2 (+' 1/3 0M))", "0.33M"), // mc-addp
            ("(with-precision 2 (+ 1/3 0M))", "0.33M"),  // mc-add
            ("(with-precision 2 (-' 1/3 0M))", "0.33M"), // mc-subp
            ("(with-precision 2 (*' 1/3 2M))", "0.66M"), // mc-mulp
            ("(with-precision 2 (inc' 1/3))", "4/3"),    // mc-incp (ratios never round)
            ("(with-precision 2 (dec' 2M))", "1M"),      // mc-decp
            ("(with-precision 2 (unchecked-add 1/3 0M))", "0.33M"), // xm-addu
            ("(with-precision 2 (unchecked-inc 1/3))", "4/3"), // xm-incu
            ("(with-precision 2 (unchecked-multiply 1/3 2M))", "0.66M"), // xm-mulu
            ("(with-precision 2 (-' 0.125M))", "-0.13M"), // n-negmc
            ("(with-precision 2 (unchecked-negate 0.125M))", "-0.13M"), // n-unegmc
            ("(with-precision 10 (-' 5M))", "-5M"),      // n-negmc10
            ("(with-precision 10 (unchecked-negate 5M))", "-5M"), // n-unegmc10
        ],
    );
}

/// The unchecked long ops: wrapping `Int` arms over the
/// checked tower lattice (probes `ua-*`, `us-*`, `um-*`,
/// `ui-*`, `ud-*`, `un-*`, `xc-addbi`).
#[test]
fn s11_unchecked_long_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(unchecked-add 1 2)", "3"), // ua-plain
            (
                "(unchecked-add 9223372036854775807 1)",
                "-9223372036854775808",
            ), // ua-wrap
            (
                "(unchecked-add -9223372036854775808 -1)",
                "9223372036854775807",
            ), // ua-wrapneg
            ("(unchecked-add 1.5 2)", "3.5"), // ua-dbl
            ("(unchecked-add 1 2.5)", "3.5"), // ua-dbl2
            ("(unchecked-add 1N 2)", "3N"), // ua-big
            ("(unchecked-add 1/2 1)", "3/2"), // ua-rat
            ("(unchecked-add 1M 1)", "2M"), // ua-dec
            ("(unchecked-add (biginteger 1) 2)", "3N"), // xc-addbi (absorbs)
            ("(unchecked-subtract 5 3)", "2"), // us-plain
            (
                "(unchecked-subtract -9223372036854775808 1)",
                "9223372036854775807",
            ), // us-wrap
            ("(unchecked-subtract 1.5 2)", "-0.5"), // us-dbl
            ("(unchecked-subtract 1N 2)", "-1N"), // us-big
            ("(unchecked-subtract 1/2 1)", "-1/2"), // us-rat
            ("(unchecked-subtract 1M 1)", "0M"), // us-dec
            ("(unchecked-multiply 6 7)", "42"), // um-plain
            ("(unchecked-multiply 9223372036854775807 2)", "-2"), // um-wrap
            (
                "(unchecked-multiply -9223372036854775808 -1)",
                "-9223372036854775808",
            ), // um-minneg
            ("(unchecked-multiply 1.5 2)", "3.0"), // um-dbl
            ("(unchecked-multiply 1N 2)", "2N"), // um-big
            ("(unchecked-multiply 1/2 2)", "1N"), // um-rat
            ("(unchecked-multiply 1M 2)", "2M"), // um-dec
            ("(unchecked-inc 5)", "6"),   // ui-plain
            (
                "(unchecked-inc 9223372036854775807)",
                "-9223372036854775808",
            ), // ui-wrap
            ("(unchecked-inc 1.5)", "2.5"), // ui-dbl
            ("(unchecked-inc 5N)", "6N"), // ui-big
            ("(unchecked-inc 1/2)", "3/2"), // ui-rat
            ("(unchecked-inc 5M)", "6M"), // ui-dec
            ("(unchecked-dec 5)", "4"),   // ud-plain
            (
                "(unchecked-dec -9223372036854775808)",
                "9223372036854775807",
            ), // ud-wrap
            ("(unchecked-dec 1.5)", "0.5"), // ud-dbl
            ("(unchecked-dec 5N)", "4N"), // ud-big
            ("(unchecked-dec 1/2)", "-1/2"), // ud-rat
            ("(unchecked-dec 5M)", "4M"), // ud-dec
            ("(unchecked-negate 5)", "-5"), // un-plain
            (
                "(unchecked-negate -9223372036854775808)",
                "-9223372036854775808",
            ), // un-min
            ("(unchecked-negate 1.5)", "-1.5"), // un-dbl
            ("(unchecked-negate 5N)", "-5N"), // un-big
            ("(unchecked-negate 1/2)", "-1/2"), // un-rat
            ("(unchecked-negate 5M)", "-5M"), // un-dec
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            ("(unchecked-add nil 1)", NPE_GETCLASS),      // ua-nil
            ("(unchecked-add \"a\" 1)", &str_num),        // ua-str
            ("(unchecked-subtract nil 1)", NPE_GETCLASS), // us-nil
            ("(unchecked-subtract 1 \"a\")", &str_num),   // us-str
            ("(unchecked-multiply nil 1)", NPE_GETCLASS), // um-nil
            ("(unchecked-multiply 1 \"a\")", &str_num),   // um-str
            ("(unchecked-inc nil)", NPE_GETCLASS),        // ui-nil
            ("(unchecked-inc \"a\")", &str_num),          // ui-str
            ("(unchecked-dec nil)", NPE_GETCLASS),        // ud-nil
            ("(unchecked-negate nil)", NPE_GETCLASS),     // un-nil
            ("(unchecked-negate \"a\")", &str_num),       // un-str
        ],
    );
    // Upstream arity failures are CompilerException (inlined);
    // the port reports the runtime shape (documented mapping).
    check_throws(
        &mut eval,
        &[
            ("(unchecked-add 1)", &arity("unchecked-add", 1)), // ua-ar1
            ("(unchecked-add 1 2 3)", &arity("unchecked-add", 3)), // ua-ar3
            ("(unchecked-inc 1 2)", &arity("unchecked-inc", 2)), // ui-ar2
            ("(unchecked-add-int 1)", &arity("unchecked-add-int", 1)), // xi-ar1
            ("(unchecked-int 1 2)", &arity("unchecked-int", 2)), // uci-ar2
        ],
    );
}

/// The `-int` ops: checked intcast, wrapping `i32` math
/// (probes `uai-*`, `usi-*`, `umi-*`, `uii-*`, `udi-*`,
/// `uni-*`, `udv-*`, `urm-*`, `xi-*`, `xd-*`, `g-int*`).
#[test]
fn s11_unchecked_int_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(unchecked-add-int 1 2)", "3"),                    // uai-plain
            ("(unchecked-add-int 2147483647 1)", "-2147483648"), // uai-wrap
            ("(unchecked-add-int 1.5 2)", "3"),                  // uai-dbl
            ("(unchecked-add-int 1N 2)", "3"),                   // uai-big
            ("(unchecked-subtract-int 5 3)", "2"),               // usi-plain
            ("(unchecked-subtract-int -2147483648 1)", "2147483647"), // usi-wrap
            ("(unchecked-subtract-int 1.5 2)", "-1"),            // usi-dbl
            ("(unchecked-subtract-int 1N 2)", "-1"),             // usi-big
            ("(unchecked-multiply-int 6 7)", "42"),              // umi-plain
            ("(unchecked-multiply-int 2147483647 2)", "-2"),     // umi-wrap
            ("(unchecked-multiply-int 1.5 2)", "2"),             // umi-dbl
            ("(unchecked-multiply-int 1N 2)", "2"),              // umi-big
            ("(unchecked-inc-int 5)", "6"),                      // uii-plain
            ("(unchecked-inc-int 2147483647)", "-2147483648"),   // uii-wrap
            ("(unchecked-inc-int 1.5)", "2"),                    // uii-dbl
            ("(unchecked-inc-int 5N)", "6"),                     // uii-big
            ("(unchecked-dec-int 5)", "4"),                      // udi-plain
            ("(unchecked-dec-int -2147483648)", "2147483647"),   // udi-wrap
            ("(unchecked-dec-int 1.5)", "0"),                    // udi-dbl
            ("(unchecked-dec-int 5N)", "4"),                     // udi-big
            ("(unchecked-negate-int 5)", "-5"),                  // uni-plain
            ("(unchecked-negate-int -2147483648)", "-2147483648"), // uni-min
            ("(unchecked-negate-int 1.5)", "-1"),                // uni-dbl
            ("(unchecked-negate-int 5N)", "-5"),                 // uni-big
            ("(unchecked-divide-int 7 2)", "3"),                 // udv-plain
            ("(unchecked-divide-int -7 2)", "-3"),               // udv-neg
            ("(unchecked-divide-int -2147483648 -1)", "-2147483648"), // udv-minneg
            ("(unchecked-divide-int 7.5 2)", "3"),               // udv-dbl
            ("(unchecked-divide-int 7N 2)", "3"),                // udv-big
            ("(unchecked-remainder-int 7 2)", "1"),              // urm-plain
            ("(unchecked-remainder-int -7 2)", "-1"),            // urm-neg
            ("(unchecked-remainder-int 7.5 2)", "1"),            // urm-dbl
            ("(unchecked-remainder-int 7N 2)", "1"),             // urm-big
            ("(unchecked-add-int ##NaN 1)", "1"),                // xi-nan
            ("(unchecked-add-int 7/2 1)", "4"),                  // xi-rat
            ("(unchecked-add-int 7.5M 1)", "8"),                 // xi-dec
            ("(unchecked-divide-int ##NaN 2)", "0"),             // xd-nan
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    let char_num = cce("java.lang.Character", "java.lang.Number");
    check_throws(
        &mut eval,
        &[
            (
                "(unchecked-add-int 4294967296 1)",
                "java.lang.ArithmeticException | integer overflow",
            ), // uai-long
            ("(unchecked-add-int nil 1)", NPE_CHAR_VALUE), // uai-nil
            ("(unchecked-add-int \"a\" 1)", &str_num),     // uai-str
            ("(unchecked-add-int \\a 1)", &char_num),      // xi-char (no char conversion)
            (
                "(unchecked-add-int ##Inf 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: Infinity",
            ), // xi-inf
            (
                "(unchecked-add-int ##-Inf 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: -Infinity",
            ), // xi-neginf
            (
                "(unchecked-add-int 1.0E20 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: 1.0E20",
            ), // xi-hugedbl
            (
                "(unchecked-add-int 4294967296N 1)",
                "java.lang.ArithmeticException | integer overflow",
            ), // xi-bigwrap
            (
                "(unchecked-add-int 8589934595/2 1)",
                "java.lang.ArithmeticException | integer overflow",
            ), // xi-hugerat
            (
                "(unchecked-add-int 1.0E20M 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: 1.0E20",
            ), // xi-hugedec (double rendering)
            (
                "(unchecked-add-int 18446744073709551616N 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: 18446744073709551616",
            ), // g-intbighuge (exact digits)
            (
                "(unchecked-add-int -18446744073709551617N 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: -18446744073709551617",
            ), // g-intbighugneg
            (
                "(unchecked-add-int 55340232221128654849/3 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: 18446744073709551616",
            ), // g-intrathuge (exact trunc digits)
            (
                "(unchecked-add-int 1.0E19M 1)",
                "java.lang.IllegalArgumentException | Value out of range for long: 1.0E19",
            ), // g-intdechuge
            ("(unchecked-divide-int \\a 2)", &char_num),   // xd-char
            ("(unchecked-divide-int nil 1)", NPE_CHAR_VALUE), // udv-nil
            ("(unchecked-divide-int 1 \"a\")", &str_num),  // udv-str
            (
                "(unchecked-divide-int 7 0)",
                "java.lang.ArithmeticException | / by zero",
            ), // udv-zero
            (
                "(unchecked-divide-int 0 0)",
                "java.lang.ArithmeticException | / by zero",
            ), // udv-zero2
            (
                "(unchecked-remainder-int 7 0)",
                "java.lang.ArithmeticException | / by zero",
            ), // urm-zero
            ("(unchecked-remainder-int nil 1)", NPE_CHAR_VALUE), // urm-nil
        ],
    );
}

/// The unchecked coercions (probes `uci-*`, `ucl-*`, `ucf-*`,
/// `ucd-*`, `ucs-*`, `ucb-*`, `ucc-*`, `xr-*`, `xc-*`,
/// `g-shortrat*`, `g-byterat`).
#[test]
fn s11_unchecked_coercion_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(unchecked-int 5)", "5"),                       // uci-plain
            ("(unchecked-int 4294967296)", "0"),              // uci-wrap
            ("(unchecked-int -4294967297)", "-1"),            // uci-negwrap
            ("(unchecked-int 1.5)", "1"),                     // uci-dbl
            ("(unchecked-int -1.5)", "-1"),                   // uci-negdbl
            ("(unchecked-int ##NaN)", "0"),                   // uci-nan
            ("(unchecked-int ##Inf)", "2147483647"),          // uci-inf
            ("(unchecked-int ##-Inf)", "-2147483648"),        // uci-neginf
            ("(unchecked-int 1.0E20)", "2147483647"),         // uci-hugedbl
            ("(unchecked-int 4294967296N)", "0"),             // uci-big
            ("(unchecked-int -4294967297N)", "-1"),           // uci-negbig
            ("(unchecked-int 7/2)", "3"),                     // uci-rat
            ("(unchecked-int -7/2)", "-3"),                   // uci-negrat
            ("(unchecked-int 7.5M)", "7"),                    // uci-dec
            ("(unchecked-int 1.0E20M)", "1661992960"),        // uci-hugedec
            ("(unchecked-int \\a)", "97"),                    // uci-char
            ("(unchecked-int 8589934595/2)", "2147483647"),   // xr-intwrap (saturates)
            ("(unchecked-int -8589934595/2)", "-2147483648"), // xr-intneg
            // xr-inthuge: `N/1` reads as BigInt (low 32 bits).
            ("(unchecked-int 100000000000000000000/1)", "1661992960"),
            ("(unchecked-int (biginteger 4294967296N))", "0"), // xc-bi (absorbs)
            ("(unchecked-long 5)", "5"),                       // ucl-plain
            ("(unchecked-long 1.5)", "1"),                     // ucl-dbl
            ("(unchecked-long ##NaN)", "0"),                   // ucl-nan
            ("(unchecked-long ##Inf)", "9223372036854775807"), // ucl-inf
            ("(unchecked-long ##-Inf)", "-9223372036854775808"), // ucl-neginf
            ("(unchecked-long 1.0E20)", "9223372036854775807"), // ucl-hugedbl
            ("(unchecked-long 18446744073709551616N)", "0"),   // ucl-big
            ("(unchecked-long -18446744073709551617N)", "-1"), // ucl-negbig
            ("(unchecked-long 7/2)", "3"),                     // ucl-rat
            ("(unchecked-long 7.5M)", "7"),                    // ucl-dec
            ("(unchecked-long 1.0E30M)", "5076944270305263616"), // ucl-hugedec
            ("(unchecked-long 36893488147419103233/2)", "0"),  // xr-longwrap
            ("(unchecked-long -36893488147419103233/2)", "0"), // xr-longneg
            ("(unchecked-float 5)", "5.0"),                    // ucf-plain
            ("(unchecked-float 0.1)", "0.1"),                  // ucf-dbl
            // PORT MAPPING: upstream `##Inf` (Float); the port
            // has no Float and keeps the exact double.
            ("(unchecked-float 1.0E300)", "1.0E300"), // ucf-huge
            ("(unchecked-float ##NaN)", "##NaN"),     // ucf-nan
            ("(unchecked-float ##Inf)", "##Inf"),     // ucf-inf
            ("(unchecked-float 5N)", "5.0"),          // ucf-big
            ("(unchecked-float 1/2)", "0.5"),         // ucf-rat
            ("(unchecked-float 0.1M)", "0.1"),        // ucf-dec
            ("(unchecked-double 5)", "5.0"),          // ucd-plain
            ("(unchecked-double 0.1)", "0.1"),        // ucd-dbl
            ("(unchecked-double 5N)", "5.0"),         // ucd-big
            ("(unchecked-double 1/2)", "0.5"),        // ucd-rat
            ("(unchecked-double 0.1M)", "0.1"),       // ucd-dec
            ("(unchecked-short 5)", "5"),             // ucs-plain
            ("(unchecked-short 32768)", "-32768"),    // ucs-wrap
            ("(unchecked-short -32769)", "32767"),    // ucs-negwrap
            ("(unchecked-short 1.5)", "1"),           // ucs-dbl
            ("(unchecked-short ##NaN)", "0"),         // ucs-nan
            ("(unchecked-short ##Inf)", "-1"),        // xc-sinf
            ("(unchecked-short ##-Inf)", "0"),        // xc-sneginf
            ("(unchecked-short 65536N)", "0"),        // ucs-big
            ("(unchecked-short 7/2)", "3"),           // ucs-rat
            ("(unchecked-short 7.5M)", "7"),          // ucs-dec
            // `N/1` reads as BigInt (low 16 bits).
            ("(unchecked-short 100000000000000000000/1)", "0"), // xr-short
            ("(unchecked-short 100000000000000000000/3)", "-1"), // g-shortrat
            ("(unchecked-short -100000000000000000000/3)", "0"), // g-shortratneg
            ("(unchecked-byte 5)", "5"),                        // ucb-plain
            ("(unchecked-byte 128)", "-128"),                   // ucb-wrap
            ("(unchecked-byte -129)", "127"),                   // ucb-negwrap
            ("(unchecked-byte 1.5)", "1"),                      // ucb-dbl
            ("(unchecked-byte ##NaN)", "0"),                    // ucb-nan
            ("(unchecked-byte ##Inf)", "-1"),                   // xc-binf
            ("(unchecked-byte 256N)", "0"),                     // ucb-big
            ("(unchecked-byte 7/2)", "3"),                      // ucb-rat
            ("(unchecked-byte 7.5M)", "7"),                     // ucb-dec
            // `N/1` reads as BigInt (low 8 bits).
            ("(unchecked-byte 100000000000000000000/1)", "0"), // xr-byte
            ("(unchecked-byte 100000000000000000000/3)", "-1"), // g-byterat
            ("(unchecked-char 65)", "\\A"),                    // ucc-plain
            ("(unchecked-char 65536)", "\\\0"),                // ucc-wrap (NUL)
            ("(unchecked-char -1)", "\\\u{ffff}"),             // ucc-neg (U+FFFF)
            ("(unchecked-char 65.5)", "\\A"),                  // ucc-dbl
            ("(unchecked-char ##NaN)", "\\\0"),                // ucc-nan
            ("(unchecked-char 65536N)", "\\\0"),               // ucc-big
            ("(unchecked-char 131/2)", "\\A"),                 // ucc-rat
            ("(unchecked-char 65.5M)", "\\A"),                 // ucc-dec
            ("(unchecked-char \\a)", "\\a"),                   // ucc-char
            ("(unchecked-char 1.0E20)", "\\\u{ffff}"),         // xc-charhuge
            ("(unchecked-char -5)", "\\\u{fffb}"),             // xc-charneg (U+FFFB)
            ("(unchecked-char 4294967297/2)", "\\\0"),         // xr-char
        ],
    );
    let str_num = cce("java.lang.String", "java.lang.Number");
    let str_char = cce("java.lang.String", "java.lang.Character");
    let char_num = cce("java.lang.Character", "java.lang.Number");
    let bool_char = cce("java.lang.Boolean", "java.lang.Character");
    check_throws(
        &mut eval,
        &[
            ("(unchecked-int nil)", NPE_CHAR_VALUE),      // uci-nil
            ("(unchecked-int \"a\")", &str_char),         // uci-str
            ("(unchecked-int true)", &bool_char),         // xc-intbool
            ("(unchecked-long \\a)", &char_num),          // ucl-char
            ("(unchecked-long nil)", NPE_LONG_VALUE),     // ucl-nil
            ("(unchecked-long \"a\")", &str_num),         // ucl-str
            ("(unchecked-float nil)", NPE_FLOAT_VALUE),   // ucf-nil
            ("(unchecked-float \"5\")", &str_num),        // ucf-str
            ("(unchecked-float \\a)", &char_num),         // xc-fchar
            ("(unchecked-double nil)", NPE_DOUBLE_VALUE), // ucd-nil
            ("(unchecked-double \"5\")", &str_num),       // ucd-str
            ("(unchecked-double \\a)", &char_num),        // xc-dchar
            ("(unchecked-short \\a)", &char_num),         // ucs-char
            ("(unchecked-short nil)", NPE_SHORT_VALUE),   // ucs-nil
            ("(unchecked-short \"a\")", &str_num),        // ucs-str
            ("(unchecked-byte \\a)", &char_num),          // ucb-char
            ("(unchecked-byte nil)", NPE_BYTE_VALUE),     // ucb-nil
            ("(unchecked-byte \"a\")", &str_num),         // ucb-str
            ("(unchecked-char nil)", NPE_LONG_VALUE),     // ucc-nil
            ("(unchecked-char \"a\")", &str_num),         // ucc-str
            // PORT MAPPING: lone surrogates are unrepresentable
            // in Rust `char` (upstream answers the surrogate);
            // shares the checked-`char` divergence.
            (
                "(unchecked-char 55296)",
                "java.lang.IllegalArgumentException | Value out of range for char: 55296",
            ),
        ],
    );
}

/// The bitwise IAE for an offender class.
fn bit_iae(class: &str) -> String {
    format!("java.lang.IllegalArgumentException | bit operation not supported for: class {class}")
}

/// The variadic bitwise folds: `bit-and`/`bit-or`/`bit-xor`/
/// `bit-and-not` (probes `ba-*`, `bo-*`, `bx-*`, `ban-*`,
/// `va-*`, `fx-andnot3`, `g-andnot*`, `g-or1`, `g-xor1`,
/// `g-order*`, `ao-*`).
#[test]
fn s11_bitwise_fold_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(bit-and 6 3)", "2"),         // ba-plain
            ("(bit-and -1 5)", "5"),        // ba-neg
            ("(bit-and 1 2 3)", "0"),       // ba-ar3 (folds)
            ("(bit-and 7 3 1 7)", "1"),     // va-and4
            ("(bit-or 6 3)", "7"),          // bo-plain
            ("(bit-or 1 2 4)", "7"),        // va-or3
            ("(bit-xor 6 3)", "5"),         // bx-plain
            ("(bit-xor 7 3 1)", "5"),       // va-xor3
            ("(bit-and-not 6 3)", "4"),     // ban-plain
            ("(bit-and-not 6 3 1)", "4"),   // fx-andnot3 (folds)
            ("(bit-and-not 6 3 1 2)", "4"), // g-andnot4
            // The JVM `Integer`/`Short`/`Byte` kinds do not
            // exist here; `int`/`short`/`byte` answer `Int`,
            // so these pin the value only (oracles `jt-*`).
            ("(bit-and (int 6) 3)", "2"),      // jt-andint
            ("(bit-and (short 6) 3)", "2"),    // jt-andshort
            ("(bit-and (byte 6) 3)", "2"),     // jt-andbyte
            ("(bit-or (int 6) (int 3))", "7"), // jt-orint
        ],
    );
    let iae_big = bit_iae("clojure.lang.BigInt");
    let iae_dbl = bit_iae("java.lang.Double");
    let iae_rat = bit_iae("clojure.lang.Ratio");
    let iae_dec = bit_iae("java.math.BigDecimal");
    let iae_str = bit_iae("java.lang.String");
    let iae_char = bit_iae("java.lang.Character");
    let iae_kw = bit_iae("clojure.lang.Keyword");
    let iae_vec = bit_iae("clojure.lang.PersistentVector");
    let iae_bool = bit_iae("java.lang.Boolean");
    check_throws(
        &mut eval,
        &[
            ("(bit-and 6N 3)", &iae_big),  // ba-big
            ("(bit-and 6 3N)", &iae_big),  // ba-big2
            ("(bit-and 6N 3N)", &iae_big), // ba-bigbig
            (
                "(bit-and 18446744073709551616N 18446744073709551615)",
                &iae_big,
            ), // ba-huge
            ("(bit-and 1.0 1)", &iae_dbl), // ba-dbl
            ("(bit-and 1 1.0)", &iae_dbl), // ba-dbl2
            ("(bit-and 1/2 1)", &iae_rat), // ba-rat
            ("(bit-and 1M 1)", &iae_dec),  // ba-dec
            ("(bit-and nil 1)", NPE_GETCLASS), // ba-nil
            ("(bit-and 1 nil)", NPE_GETCLASS), // ao-nil2
            ("(bit-and 1 \"a\")", &iae_str), // ba-str
            ("(bit-and 7 3 \"a\")", &iae_str), // va-andbad3 (mid-fold)
            ("(bit-and \"a\" \"b\")", &iae_str), // ao-bothbad
            ("(bit-and \"a\" 1.0)", &iae_str), // g-order (first wins)
            ("(bit-and 1.0 \"a\")", &iae_dbl), // g-order2 (first wins)
            ("(bit-and :a 1)", &iae_kw),   // ao-kw
            ("(bit-and [1] 1)", &iae_vec), // ao-vec
            ("(bit-and true 1)", &iae_bool), // ao-bool
            ("(bit-and \\a 3)", &iae_char), // jt-andchar
            ("(bit-or 6N 3)", &iae_big),   // bo-big
            ("(bit-or 6 3N)", &iae_big),   // bo-big2
            ("(bit-or 1.0 1)", &iae_dbl),  // bo-dbl
            ("(bit-or 1/2 1)", &iae_rat),  // bo-rat
            ("(bit-or 1M 1)", &iae_dec),   // bo-dec
            ("(bit-or nil 1)", NPE_GETCLASS), // bo-nil
            ("(bit-xor 6N 3N)", &iae_big), // bx-big
            ("(bit-xor 1.0 1)", &iae_dbl), // bx-dbl
            ("(bit-xor nil 1)", NPE_GETCLASS), // bx-nil
            ("(bit-and-not 6N 3N)", &iae_big), // ban-big
            ("(bit-and-not 1.0 1)", &iae_dbl), // ban-dbl
            ("(bit-and-not nil 1)", NPE_GETCLASS), // ban-nil
            // Port-only pin: `BigInteger` is rejected like
            // `BigInt` (same rule, its own class name).
            (
                "(bit-and (biginteger 6) 3)",
                &bit_iae("java.math.BigInteger"),
            ),
            ("(bit-and 1)", &arity("bit-and", 1)), // ba-ar1
            ("(bit-and)", &arity("bit-and", 0)),   // va-and0
            ("(bit-or)", &arity("bit-or", 0)),     // va-or0
            ("(bit-or 1)", &arity("bit-or", 1)),   // g-or1
            ("(bit-xor)", &arity("bit-xor", 0)),   // va-xor0
            ("(bit-xor 5)", &arity("bit-xor", 1)), // g-xor1
            ("(bit-and-not)", &arity("bit-and-not", 0)), // g-andnot0
            ("(bit-and-not 6)", &arity("bit-and-not", 1)), // g-andnot1
        ],
    );
}

/// `bit-not` (probes `bn-*`, `jt-notint`).
#[test]
fn s11_bit_not_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(bit-not 6)", "-7"),       // bn-plain
            ("(bit-not -1)", "0"),       // bn-neg
            ("(bit-not (int 6))", "-7"), // jt-notint
        ],
    );
    check_throws(
        &mut eval,
        &[
            ("(bit-not 6N)", &bit_iae("clojure.lang.BigInt")), // bn-big
            ("(bit-not -1N)", &bit_iae("clojure.lang.BigInt")), // bn-negbig
            ("(bit-not 1.0)", &bit_iae("java.lang.Double")),   // bn-dbl
            ("(bit-not 1/2)", &bit_iae("clojure.lang.Ratio")), // bn-rat
            ("(bit-not 1M)", &bit_iae("java.math.BigDecimal")), // bn-dec
            ("(bit-not nil)", NPE_GETCLASS),                   // bn-nil
            ("(bit-not \"a\")", &bit_iae("java.lang.String")), // bn-str
            // Upstream CompilerException (inlined); the port
            // reports the runtime shape (documented mapping).
            ("(bit-not)", &arity("bit-not", 0)),     // bn-ar0
            ("(bit-not 1 2)", &arity("bit-not", 2)), // bn-ar2
        ],
    );
}

/// Shifts (probes `bsl-*`, `bsr-*`, `ubs-*`, `jt-shlint`,
/// `ao-shnil2`, `ev-*`, `fx-sh*3`, `fx-ubs3`).
#[test]
fn s11_bit_shift_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(bit-shift-left 1 2)", "4"),                     // bsl-plain
            ("(bit-shift-left 1 70)", "64"),                   // bsl-mask
            ("(bit-shift-left 1 -1)", "-9223372036854775808"), // bsl-negsh
            ("(bit-shift-left 1 (int 2))", "4"),               // jt-shlint
            ("(bit-shift-left 5 0)", "5"),                     // ev-shl0
            (
                "(bit-shift-left 1 9223372036854775807)",
                "-9223372036854775808",
            ), // ev-hugesh
            ("(bit-shift-right -8 2)", "-2"),                  // bsr-plain
            ("(bit-shift-right -8 70)", "-1"),                 // bsr-mask
            ("(bit-shift-right 1 -1)", "0"),                   // bsr-negsh
            ("(bit-shift-right 5 0)", "5"),                    // ev-shr0
            ("(unsigned-bit-shift-right -1 1)", "9223372036854775807"), // ubs-plain
            ("(unsigned-bit-shift-right -1 70)", "288230376151711743"), // ubs-mask
            ("(unsigned-bit-shift-right -1 -1)", "1"),         // ubs-negsh
            ("(unsigned-bit-shift-right -8 2)", "4611686018427387902"), // ev-ubs
        ],
    );
    let iae_big = bit_iae("clojure.lang.BigInt");
    let iae_dbl = bit_iae("java.lang.Double");
    let iae_rat = bit_iae("clojure.lang.Ratio");
    let iae_dec = bit_iae("java.math.BigDecimal");
    let iae_str = bit_iae("java.lang.String");
    check_throws(
        &mut eval,
        &[
            ("(bit-shift-left 1 18446744073709551616N)", &iae_big), // bsl-bigsh
            ("(bit-shift-left 1N 70)", &iae_big),                   // bsl-big
            ("(bit-shift-left 1N -1)", &iae_big),                   // bsl-negbig
            ("(bit-shift-left 1N 18446744073709551616N)", &iae_big), // bsl-bigcnt
            ("(bit-shift-left 1.0 2)", &iae_dbl),                   // bsl-dbl
            ("(bit-shift-left 1 2.0)", &iae_dbl),                   // bsl-dblcnt
            ("(bit-shift-left 1/2 2)", &iae_rat),                   // bsl-rat
            ("(bit-shift-left 1M 2)", &iae_dec),                    // bsl-dec
            ("(bit-shift-left nil 1)", NPE_GETCLASS),               // bsl-nil
            ("(bit-shift-left 1 nil)", NPE_GETCLASS),               // ao-shnil2
            ("(bit-shift-left 1 \"a\")", &iae_str),                 // bsl-str
            ("(bit-shift-right -8N 2)", &iae_big),                  // bsr-big
            ("(bit-shift-right 1N -1)", &iae_big),                  // bsr-negbig
            ("(bit-shift-right 1.0 2)", &iae_dbl),                  // bsr-dbl
            ("(bit-shift-right 1 2.0)", &iae_dbl),                  // bsr-dblcnt
            ("(bit-shift-right nil 1)", NPE_GETCLASS),              // bsr-nil
            ("(unsigned-bit-shift-right -1N 1)", &iae_big),         // ubs-big
            ("(unsigned-bit-shift-right 8N 2)", &iae_big),          // ubs-posbig
            ("(unsigned-bit-shift-right 1.0 2)", &iae_dbl),         // ubs-dbl
            ("(unsigned-bit-shift-right 1 2.0)", &iae_dbl),         // ubs-dblcnt
            ("(unsigned-bit-shift-right nil 1)", NPE_GETCLASS),     // ubs-nil
            // Upstream CompilerException (inlined); the port
            // reports the runtime shape (documented mapping).
            ("(bit-shift-left 1)", &arity("bit-shift-left", 1)), // bsl-ar1
            ("(bit-shift-left 1 2 3)", &arity("bit-shift-left", 3)), // fx-shl3
            ("(bit-shift-right 8 1 1)", &arity("bit-shift-right", 3)), // fx-shr3
            (
                "(unsigned-bit-shift-right 8 1 1)",
                &arity("unsigned-bit-shift-right", 3),
            ), // fx-ubs3
        ],
    );
}

/// `bit-set`/`bit-clear`/`bit-flip`/`bit-test` (probes
/// `bst-*`, `bcl-*`, `bfl-*`, `btt-*`, `fx-set3`,
/// `fx-clear3`, `fx-flip3`, `fx-test*`, `ao-testnil2`).
#[test]
fn s11_bit_index_rows() {
    let mut eval = boot();
    check_rows(
        &mut eval,
        &[
            ("(bit-set 0 3)", "8"),                      // bst-plain
            ("(bit-set 0 -1)", "-9223372036854775808"),  // bst-neg
            ("(bit-clear 7 1)", "5"),                    // bcl-plain
            ("(bit-clear 0 -1)", "0"),                   // bcl-neg
            ("(bit-flip 7 1)", "5"),                     // bfl-plain
            ("(bit-flip 0 -1)", "-9223372036854775808"), // bfl-neg
            ("(bit-test 4 2)", "true"),                  // btt-plain
            ("(bit-test 4 1)", "false"),                 // btt-false
            ("(bit-test 4 -1)", "false"),                // btt-neg
        ],
    );
    let iae_big = bit_iae("clojure.lang.BigInt");
    let iae_dbl = bit_iae("java.lang.Double");
    let iae_rat = bit_iae("clojure.lang.Ratio");
    check_throws(
        &mut eval,
        &[
            ("(bit-set 0N 70)", &iae_big),                   // bst-big
            ("(bit-set 0N -1)", &iae_big),                   // bst-bigneg
            ("(bit-set 0 18446744073709551616N)", &iae_big), // bst-hugeidx
            ("(bit-set 1.0 1)", &iae_dbl),                   // bst-dbl
            ("(bit-set 0 1.0)", &iae_dbl),                   // bst-dblidx
            ("(bit-set 1/2 1)", &iae_rat),                   // bst-rat
            ("(bit-set nil 1)", NPE_GETCLASS),               // bst-nil
            ("(bit-clear 255N 70)", &iae_big),               // bcl-big
            ("(bit-clear 1.0 1)", &iae_dbl),                 // bcl-dbl
            ("(bit-clear nil 1)", NPE_GETCLASS),             // bcl-nil
            ("(bit-flip 255N 70)", &iae_big),                // bfl-big
            ("(bit-flip 1.0 1)", &iae_dbl),                  // bfl-dbl
            ("(bit-flip nil 1)", NPE_GETCLASS),              // bfl-nil
            ("(bit-test 255N 70)", &iae_big),                // btt-big
            ("(bit-test 4N -1)", &iae_big),                  // btt-bigneg
            ("(bit-test 1.0 1)", &iae_dbl),                  // btt-dbl
            ("(bit-test 4 1.0)", &iae_dbl),                  // btt-dblidx
            ("(bit-test nil 1)", NPE_GETCLASS),              // btt-nil
            ("(bit-test 4 nil)", NPE_GETCLASS),              // ao-testnil2
            ("(bit-set 0 1 2)", &arity("bit-set", 3)),       // fx-set3
            ("(bit-clear 7 1 0)", &arity("bit-clear", 3)),   // fx-clear3
            ("(bit-flip 7 1 0)", &arity("bit-flip", 3)),     // fx-flip3
            ("(bit-test 4 2 1)", &arity("bit-test", 3)),     // fx-test3
            ("(bit-test 4)", &arity("bit-test", 1)),         // btt-ar1
            ("(bit-test)", &arity("bit-test", 0)),           // fx-test0
        ],
    );
}
