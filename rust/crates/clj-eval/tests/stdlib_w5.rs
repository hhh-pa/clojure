// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W5 (small libs) smoke tests, portable leg: `clojure.math`, `clojure.uuid`,
//! `clojure.instant`, `clojure.gvec`, and the subprocess-free half of
//! `clojure.java.javadoc`. Each namespace is required fresh per test and
//! exercised against upstream-documented behavior (docstrings + source
//! semantics are the oracle). Approximate-math expectations come from an
//! independent oracle (CPython's math module, same IEEE doubles).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, pr_to_string};

fn pr(v: &clj_value::Value) -> String {
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

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// The last value as an f64 (pr-strings parse back losslessly).
fn as_f64(eval: &mut Evaluator, src: &str) -> f64 {
    run(eval, src).parse::<f64>().unwrap()
}

/// Assert `|actual - expected| <= tol`.
fn assert_abs(eval: &mut Evaluator, src: &str, expected: f64, tol: f64) {
    let actual = as_f64(eval, src);
    assert!(
        (actual - expected).abs() <= tol,
        "{src}: got {actual}, want {expected} +/- {tol}"
    );
}

fn require_math(eval: &mut Evaluator) {
    run(eval, "(require 'clojure.math)");
}

#[test]
fn math_ns_loads_with_exact_publics() {
    let mut eval = boot();
    require_math(&mut eval);
    assert_eq!(run(&mut eval, "clojure.math/E"), "2.718281828459045");
    assert_eq!(run(&mut eval, "clojure.math/PI"), "3.141592653589793");
    assert_eq!(
        run(&mut eval, "(sort (keys (ns-publics 'clojure.math)))"),
        "(E IEEE-remainder PI acos add-exact asin atan atan2 cbrt ceil copy-sign cos cosh \
         decrement-exact exp expm1 floor floor-div floor-mod get-exponent hypot increment-exact log \
         log10 log1p multiply-exact negate-exact next-after next-down next-up pow random rint round \
         scalb signum sin sinh sqrt subtract-exact tan tanh to-degrees to-radians ulp)"
    );
}

#[test]
// Oracle literals stay literal (independent of std consts by design).
#[allow(clippy::approx_constant)]
fn math_trig() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/sin 0.0)", "0.0"),
        ("(clojure.math/cos 0.0)", "1.0"),
        ("(clojure.math/tan 0.0)", "0.0"),
        ("(clojure.math/asin 0.0)", "0.0"),
        ("(clojure.math/acos 1.0)", "0.0"),
        ("(clojure.math/atan 0.0)", "0.0"),
        ("(clojure.math/sin ##NaN)", "##NaN"),
        ("(clojure.math/cos ##Inf)", "##NaN"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_abs(
        &mut eval,
        "(clojure.math/sin 1.0)",
        0.8414709848078965,
        1e-15,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/atan 1.0)",
        0.7853981633974483,
        1e-15,
    );
}

#[test]
// Oracle literals stay literal (independent of std consts by design).
#[allow(clippy::approx_constant)]
fn math_degree_conversion() {
    let mut eval = boot();
    require_math(&mut eval);
    // Same two roundings as the JDK (deg * (PI/180)); both land exact here.
    assert_eq!(
        run(&mut eval, "(clojure.math/to-radians 180.0)"),
        "3.141592653589793"
    );
    assert_eq!(
        run(&mut eval, "(clojure.math/to-degrees clojure.math/PI)"),
        "180.0"
    );
    assert_eq!(run(&mut eval, "(clojure.math/to-radians 0.0)"), "0.0");
    assert_eq!(run(&mut eval, "(clojure.math/to-degrees 0.0)"), "0.0");
    assert_abs(
        &mut eval,
        "(clojure.math/to-radians 90.0)",
        1.5707963267948966,
        1e-15,
    );
}

#[test]
// Oracle literals stay literal (independent of std consts by design).
#[allow(clippy::approx_constant)]
fn math_exp_log_powers() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/exp 0.0)", "1.0"),
        ("(clojure.math/exp ##-Inf)", "0.0"),
        ("(clojure.math/log 1.0)", "0.0"),
        ("(clojure.math/log 0.0)", "##-Inf"),
        ("(clojure.math/log10 1.0)", "0.0"),
        ("(clojure.math/sqrt 4.0)", "2.0"),
        ("(clojure.math/sqrt -1.0)", "##NaN"),
        ("(clojure.math/cbrt 0.0)", "0.0"),
        ("(clojure.math/atan2 0.0 1.0)", "0.0"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_abs(
        &mut eval,
        "(clojure.math/exp 1.0)",
        2.718281828459045,
        1e-15,
    );
    assert_abs(&mut eval, "(clojure.math/log clojure.math/E)", 1.0, 1e-15);
    assert_abs(&mut eval, "(clojure.math/log10 100.0)", 2.0, 1e-15);
    assert_abs(&mut eval, "(clojure.math/cbrt 27.0)", 3.0, 1e-15);
    assert_abs(&mut eval, "(clojure.math/pow 2.0 10.0)", 1024.0, 1e-12);
    assert_abs(&mut eval, "(clojure.math/hypot 3.0 4.0)", 5.0, 1e-15);
}

#[test]
fn math_ieee_remainder() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/IEEE-remainder 5.0 2.0)", "1.0"),
        ("(clojure.math/IEEE-remainder 5.0 3.0)", "-1.0"),
        (
            "(clojure.math/IEEE-remainder 5.3 2.0)",
            "-0.7000000000000002",
        ),
        (
            "(clojure.math/IEEE-remainder 0.1 0.03)",
            "0.010000000000000009",
        ),
        ("(clojure.math/IEEE-remainder 0.0 1.0)", "0.0"),
        ("(clojure.math/IEEE-remainder -0.0 5.0)", "-0.0"),
        ("(clojure.math/IEEE-remainder 5.0 0.0)", "##NaN"),
        ("(clojure.math/IEEE-remainder ##NaN 1.0)", "##NaN"),
        ("(clojure.math/IEEE-remainder ##Inf 1.0)", "##NaN"),
        ("(clojure.math/IEEE-remainder 5.0 ##Inf)", "5.0"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

#[test]
fn math_ceil_floor_rint() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/ceil 2.1)", "3.0"),
        ("(clojure.math/ceil -2.1)", "-2.0"),
        ("(clojure.math/floor 2.9)", "2.0"),
        ("(clojure.math/floor -2.1)", "-3.0"),
        ("(clojure.math/rint 2.5)", "2.0"),
        ("(clojure.math/rint 3.5)", "4.0"),
        ("(clojure.math/rint -2.5)", "-2.0"),
        ("(clojure.math/rint -3.5)", "-4.0"),
        ("(clojure.math/rint 0.5)", "0.0"),
        ("(clojure.math/rint -0.5)", "-0.0"),
        ("(clojure.math/rint 2.1)", "2.0"),
        ("(clojure.math/rint -0.1)", "-0.0"),
        ("(clojure.math/rint 0.0)", "0.0"),
        ("(clojure.math/rint -0.0)", "-0.0"),
        ("(clojure.math/rint ##NaN)", "##NaN"),
        ("(clojure.math/rint ##Inf)", "##Inf"),
        ("(clojure.math/rint ##-Inf)", "##-Inf"),
        ("(clojure.math/rint 1e16)", "1.0E16"),
        (
            "(clojure.math/rint 4503599627370496.0)",
            "4.503599627370496E15",
        ),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

#[test]
fn math_round_random() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/round 2.5)", "3"),
        ("(clojure.math/round -2.5)", "-2"),
        ("(clojure.math/round 2.4)", "2"),
        ("(clojure.math/round ##NaN)", "0"),
        ("(clojure.math/round ##Inf)", "9223372036854775807"),
        ("(clojure.math/round ##-Inf)", "-9223372036854775808"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_eq!(run(&mut eval, "(double? (clojure.math/random))"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(and (>= (clojure.math/random) 0.0) (< (clojure.math/random) 1.0))"
        ),
        "true"
    );
}

#[test]
fn math_exact_integer_ops() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/add-exact 2 3)", "5"),
        ("(clojure.math/add-exact 2.9 3.1)", "5"),
        ("(clojure.math/subtract-exact 2 5)", "-3"),
        ("(clojure.math/multiply-exact 2000000000 4)", "8000000000"),
        ("(clojure.math/increment-exact -1)", "0"),
        ("(clojure.math/decrement-exact 0)", "-1"),
        ("(clojure.math/negate-exact 5)", "-5"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // JDK *Exact overflow shape: ArithmeticException "long overflow".
    for src in [
        "(clojure.math/add-exact Long/MAX_VALUE 1)",
        "(clojure.math/subtract-exact Long/MIN_VALUE 1)",
        "(clojure.math/multiply-exact Long/MAX_VALUE 2)",
        "(clojure.math/increment-exact Long/MAX_VALUE)",
        "(clojure.math/decrement-exact Long/MIN_VALUE)",
        "(clojure.math/negate-exact Long/MIN_VALUE)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ArithmeticException | long overflow",
            "{src}"
        );
    }
}

#[test]
fn math_floor_div_mod() {
    let mut eval = boot();
    require_math(&mut eval);
    // Oracle: Python // and % (floor semantics, identical for ints).
    for (src, want) in [
        ("(clojure.math/floor-div 7 3)", "2"),
        ("(clojure.math/floor-div -7 3)", "-3"),
        ("(clojure.math/floor-div 7 -3)", "-3"),
        ("(clojure.math/floor-div -7 -3)", "2"),
        ("(clojure.math/floor-div -6 3)", "-2"),
        ("(clojure.math/floor-div 0 -5)", "0"),
        (
            "(clojure.math/floor-div Long/MIN_VALUE -1)",
            "-9223372036854775808",
        ),
        ("(clojure.math/floor-mod 7 3)", "1"),
        ("(clojure.math/floor-mod -7 3)", "2"),
        ("(clojure.math/floor-mod 7 -3)", "-2"),
        ("(clojure.math/floor-mod -7 -3)", "-1"),
        ("(clojure.math/floor-mod Long/MIN_VALUE -1)", "0"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(clojure.math/floor-div 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(clojure.math/floor-mod 5 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
}

#[test]
fn math_ulp_signum() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/ulp 1.0)", "2.220446049250313E-16"),
        ("(clojure.math/ulp -1.0)", "2.220446049250313E-16"),
        ("(clojure.math/ulp 0.0)", "4.9E-324"),
        ("(clojure.math/ulp -0.0)", "4.9E-324"),
        (
            "(clojure.math/ulp Double/MAX_VALUE)",
            "1.99584030953472E292",
        ),
        ("(clojure.math/ulp ##NaN)", "##NaN"),
        ("(clojure.math/ulp ##Inf)", "##Inf"),
        ("(clojure.math/ulp ##-Inf)", "##Inf"),
        ("(clojure.math/signum -5.0)", "-1.0"),
        ("(clojure.math/signum 5.0)", "1.0"),
        ("(clojure.math/signum 0.0)", "0.0"),
        ("(clojure.math/signum -0.0)", "-0.0"),
        ("(clojure.math/signum ##NaN)", "##NaN"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // Composition: ulp(MAX_VALUE) is exactly 2^971 per its own docstring.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.math/get-exponent (clojure.math/ulp Double/MAX_VALUE))"
        ),
        "971"
    );
    assert_eq!(
        run(
            &mut eval,
            "(clojure.math/get-exponent (clojure.math/ulp 1.0))"
        ),
        "-52"
    );
}

#[test]
fn math_hyperbolic() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/sinh 0.0)", "0.0"),
        ("(clojure.math/cosh 0.0)", "1.0"),
        ("(clojure.math/tanh 0.0)", "0.0"),
        ("(clojure.math/sinh ##NaN)", "##NaN"),
        ("(clojure.math/cosh ##Inf)", "##Inf"),
        ("(clojure.math/tanh ##Inf)", "1.0"),
        ("(clojure.math/hypot 0.0 0.0)", "0.0"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_abs(
        &mut eval,
        "(clojure.math/sinh 1.0)",
        1.1752011936438014,
        1e-15,
    );
}

#[test]
// Oracle literals stay literal (independent of std consts by design).
#[allow(clippy::approx_constant)]
fn math_expm1_log1p() {
    let mut eval = boot();
    require_math(&mut eval);
    // Oracles: repr(math.expm1/log1p(x)) from CPython; Taylor paths hold ~1e-15.
    for (src, want) in [
        ("(clojure.math/expm1 0.0)", "0.0"),
        ("(clojure.math/expm1 -0.0)", "-0.0"),
        ("(clojure.math/expm1 ##NaN)", "##NaN"),
        ("(clojure.math/expm1 ##Inf)", "##Inf"),
        ("(clojure.math/expm1 -1000.0)", "-1.0"),
        ("(clojure.math/log1p 0.0)", "0.0"),
        ("(clojure.math/log1p -0.0)", "-0.0"),
        ("(clojure.math/log1p -1.0)", "##-Inf"),
        ("(clojure.math/log1p -2.0)", "##NaN"),
        ("(clojure.math/log1p ##NaN)", "##NaN"),
        ("(clojure.math/log1p ##Inf)", "##Inf"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_abs(
        &mut eval,
        "(clojure.math/expm1 1.0)",
        1.7182818284590453,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/expm1 -1.0)",
        -0.6321205588285577,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/expm1 0.5)",
        0.6487212707001282,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/expm1 1e-10)",
        1.00000000005e-10,
        1e-22,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/expm1 100.0)",
        2.6881171418161356e43,
        1e29,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/log1p 1.0)",
        0.6931471805599453,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/log1p -0.5)",
        -0.6931471805599453,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/log1p 0.5)",
        0.4054651081081644,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/log1p 10.0)",
        2.3978952727983707,
        1e-12,
    );
    assert_abs(
        &mut eval,
        "(clojure.math/log1p 1e-10)",
        9.999999999500001e-11,
        1e-22,
    );
    // Outside |x| <= 0.5 the direct formula is used bit-for-bit.
    assert_eq!(
        run(
            &mut eval,
            "(== (clojure.math/expm1 100.0) (dec (clojure.math/exp 100.0)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(== (clojure.math/log1p 10.0) (clojure.math/log 11.0))"
        ),
        "true"
    );
}

#[test]
fn math_copy_sign() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/copy-sign 3.0 -1.0)", "-3.0"),
        ("(clojure.math/copy-sign -3.0 1.0)", "3.0"),
        ("(clojure.math/copy-sign 3.0 0.0)", "3.0"),
        ("(clojure.math/copy-sign 3.0 -0.0)", "-3.0"),
        ("(clojure.math/copy-sign -3.0 0.0)", "3.0"),
        ("(clojure.math/copy-sign -0.0 1.0)", "0.0"),
        ("(clojure.math/copy-sign -0.0 -1.0)", "-0.0"),
        ("(clojure.math/copy-sign 0.0 -1.0)", "-0.0"),
        ("(clojure.math/copy-sign 0.0 1.0)", "0.0"),
        ("(clojure.math/copy-sign ##NaN -1.0)", "##NaN"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

#[test]
fn math_get_exponent() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/get-exponent 1.0)", "0"),
        ("(clojure.math/get-exponent 2.0)", "1"),
        ("(clojure.math/get-exponent 0.5)", "-1"),
        ("(clojure.math/get-exponent 3.0)", "1"),
        ("(clojure.math/get-exponent 0.1)", "-4"),
        ("(clojure.math/get-exponent 1e-300)", "-997"),
        // 2.2250738585072014E-308 is MIN_NORMAL (exponent -1022).
        (
            "(clojure.math/get-exponent 2.2250738585072014E-308)",
            "-1022",
        ),
        ("(clojure.math/get-exponent Double/MAX_VALUE)", "1023"),
        ("(clojure.math/get-exponent 0.0)", "-1075"),
        ("(clojure.math/get-exponent -0.0)", "-1075"),
        ("(clojure.math/get-exponent ##NaN)", "1024"),
        ("(clojure.math/get-exponent ##Inf)", "1024"),
        ("(clojure.math/get-exponent ##-Inf)", "1024"),
        // The true MIN_VALUE is subnormal.
        (
            "(clojure.math/get-exponent (clojure.math/ulp 0.0))",
            "-1075",
        ),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

#[test]
fn math_next_up_down_after() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/next-up 1.0)", "1.0000000000000002"),
        ("(clojure.math/next-up -1.0)", "-0.9999999999999999"),
        ("(clojure.math/next-up 0.0)", "4.9E-324"),
        ("(clojure.math/next-up -0.0)", "4.9E-324"),
        ("(clojure.math/next-up Double/MAX_VALUE)", "##Inf"),
        (
            "(clojure.math/next-up (- Double/MAX_VALUE))",
            "-1.7976931348623155E308",
        ),
        ("(clojure.math/next-up ##Inf)", "##Inf"),
        ("(clojure.math/next-up ##-Inf)", "-1.7976931348623157E308"),
        ("(clojure.math/next-up ##NaN)", "##NaN"),
        ("(clojure.math/next-up (- (clojure.math/ulp 0.0)))", "-0.0"),
        ("(clojure.math/next-down 1.0)", "0.9999999999999999"),
        ("(clojure.math/next-down -1.0)", "-1.0000000000000002"),
        ("(clojure.math/next-down 0.0)", "-4.9E-324"),
        ("(clojure.math/next-down -0.0)", "-4.9E-324"),
        ("(clojure.math/next-down (clojure.math/ulp 0.0))", "0.0"),
        (
            "(clojure.math/next-down Double/MAX_VALUE)",
            "1.7976931348623155E308",
        ),
        ("(clojure.math/next-down (- Double/MAX_VALUE))", "##-Inf"),
        ("(clojure.math/next-down ##Inf)", "1.7976931348623157E308"),
        ("(clojure.math/next-down ##-Inf)", "##-Inf"),
        ("(clojure.math/next-down ##NaN)", "##NaN"),
        ("(clojure.math/next-after 1.0 2.0)", "1.0000000000000002"),
        ("(clojure.math/next-after 1.0 0.0)", "0.9999999999999999"),
        ("(clojure.math/next-after 5.0 5.0)", "5.0"),
        ("(clojure.math/next-after 0.0 -0.0)", "-0.0"),
        ("(clojure.math/next-after 0.0 1.0)", "4.9E-324"),
        ("(clojure.math/next-after -0.0 -1.0)", "-4.9E-324"),
        (
            "(clojure.math/next-after ##Inf 0.0)",
            "1.7976931348623157E308",
        ),
        (
            "(clojure.math/next-after ##-Inf 0.0)",
            "-1.7976931348623157E308",
        ),
        ("(clojure.math/next-after ##NaN 1.0)", "##NaN"),
        ("(clojure.math/next-after 1.0 ##NaN)", "##NaN"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // Adjacency round-trips through the subnormal boundary.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.math/next-up (clojure.math/next-down 2.2250738585072014E-308))"
        ),
        "2.2250738585072014E-308"
    );
}

#[test]
fn math_scalb() {
    let mut eval = boot();
    require_math(&mut eval);
    for (src, want) in [
        ("(clojure.math/scalb 1.5 2)", "6.0"),
        ("(clojure.math/scalb 1.5 -2)", "0.375"),
        ("(clojure.math/scalb 0.1 3)", "0.8"),
        ("(clojure.math/scalb 5.0 0)", "5.0"),
        ("(clojure.math/scalb 1.0 1024)", "##Inf"),
        ("(clojure.math/scalb 1.0 -1100)", "0.0"),
        ("(clojure.math/scalb -1.0 -1100)", "-0.0"),
        ("(clojure.math/scalb 1.0 -1074)", "4.9E-324"),
        ("(clojure.math/scalb -0.0 5)", "-0.0"),
        ("(clojure.math/scalb 0.0 5)", "0.0"),
        ("(clojure.math/scalb ##NaN 5)", "##NaN"),
        ("(clojure.math/scalb ##Inf -1000000)", "##Inf"),
        ("(clojure.math/scalb ##-Inf 1000000)", "##-Inf"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

fn require_uuid(eval: &mut Evaluator) {
    run(eval, "(require 'clojure.uuid)");
}

#[test]
fn uuid_ns_loads_with_no_publics() {
    let mut eval = boot();
    require_uuid(&mut eval);
    // Upstream exposes no publics (only the private reader + print methods).
    assert_eq!(run(&mut eval, "(ns-publics 'clojure.uuid)"), "{}");
    assert_eq!(
        run(
            &mut eval,
            "(some? (ns-resolve 'clojure.uuid 'default-uuid-reader))"
        ),
        "true"
    );
}

#[test]
fn uuid_default_reader() {
    let mut eval = boot();
    require_uuid(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(@#'clojure.uuid/default-uuid-reader \
             \"550e8400-e29b-41d4-a716-446655440000\")"
        ),
        "#uuid \"550e8400-e29b-41d4-a716-446655440000\""
    );
    // Upstream messages, ex-info class (no IAE ctor without a JVM).
    assert_eq!(
        uncaught(&mut eval, "(@#'clojure.uuid/default-uuid-reader 42)"),
        "clojure.lang.ExceptionInfo | #uuid data reader expected string"
    );
    assert_eq!(
        uncaught(&mut eval, "(@#'clojure.uuid/default-uuid-reader \"junk\")"),
        "clojure.lang.ExceptionInfo | Invalid UUID string: junk"
    );
    // The default #uuid round-trip the namespace exists to support.
    assert_eq!(
        run(
            &mut eval,
            "(pr-str (read-string \
             \"#uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\"))"
        ),
        "\"#uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\""
    );
}

fn require_instant(eval: &mut Evaluator) {
    run(eval, "(require 'clojure.instant)");
}

#[test]
fn instant_ns_loads_with_exact_publics() {
    let mut eval = boot();
    require_instant(&mut eval);
    assert_eq!(
        run(&mut eval, "(sort (keys (ns-publics 'clojure.instant)))"),
        "(parse-timestamp read-instant-calendar read-instant-date read-instant-timestamp validated)"
    );
}

#[test]
fn instant_parse_timestamp() {
    let mut eval = boot();
    require_instant(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(clojure.instant/parse-timestamp vector \"2020-06-01T12:34:56.789+05:30\")"
        ),
        "[2020 6 1 12 34 56 789000000 1 5 30]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(clojure.instant/parse-timestamp vector \"2020\")"
        ),
        "[2020 1 1 0 0 0 0 0 0 0]"
    );
    // Fraction zero-fills right to nanos.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.instant/parse-timestamp vector \"2020-01-01T00:00:00.5Z\")"
        ),
        "[2020 1 1 0 0 0 500000000 0 0 0]"
    );
    // Over-long fractions truncate to nanos.
    assert_eq!(
        run(
            &mut eval,
            "(nth (clojure.instant/parse-timestamp vector \
             \"2020-01-01T00:00:00.1234567890\") 6)"
        ),
        "123456789"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(clojure.instant/parse-timestamp vector \"junk\")"
        ),
        "clojure.lang.ExceptionInfo | Unrecognized date/time syntax: junk"
    );
}

#[test]
fn instant_validated() {
    let mut eval = boot();
    require_instant(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(clojure.instant/parse-timestamp (clojure.instant/validated vector) \
             \"2020-02-29\")"
        ),
        "[2020 2 29 0 0 0 0 0 0 0]"
    );
    for (src, want) in [
        (
            "(clojure.instant/parse-timestamp (clojure.instant/validated vector) \
             \"2020-13-01\")",
            "clojure.lang.ExceptionInfo | failed: (<= 1 months 12)",
        ),
        (
            "(clojure.instant/parse-timestamp (clojure.instant/validated vector) \
             \"2021-02-29\")",
            "clojure.lang.ExceptionInfo | failed: (<= 1 days (days-in-month months \
             (leap-year? years)))",
        ),
        (
            "(clojure.instant/parse-timestamp (clojure.instant/validated vector) \
             \"2020-01-01T00:00:60\")",
            "clojure.lang.ExceptionInfo | failed: (<= 0 seconds (if (= minutes 59) 60 59))",
        ),
        (
            "(clojure.instant/parse-timestamp (clojure.instant/validated vector) \
             \"2020-01-01T00:00:00+24:00\")",
            "clojure.lang.ExceptionInfo | failed: (<= 0 offset-hours 23)",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn instant_readers() {
    let mut eval = boot();
    require_instant(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(inst-ms (clojure.instant/read-instant-date \
             \"2020-01-01T00:00:00.000-00:00\"))"
        ),
        "1577836800000"
    );
    // Leap seconds roll forward, like the upstream lenient calendar.
    assert_eq!(
        run(
            &mut eval,
            "(inst-ms (clojure.instant/read-instant-date \
             \"2016-12-31T23:59:60.000-00:00\"))"
        ),
        "1483228800000"
    );
    // The offset folds into UTC; the printed shape is the point in time.
    assert_eq!(
        run(
            &mut eval,
            "(pr-str (clojure.instant/read-instant-calendar \
             \"2020-06-01T12:00:00.000+05:30\"))"
        ),
        "\"#inst \\\"2020-06-01T06:30:00.000-00:00\\\"\""
    );
    // Nanos truncate to millis, exactly like #inst literals.
    assert_eq!(
        run(
            &mut eval,
            "(inst-ms (clojure.instant/read-instant-timestamp \
             \"2020-01-01T00:00:00.123456789-00:00\"))"
        ),
        "1577836800123"
    );
    assert_eq!(
        run(
            &mut eval,
            "(inst? (clojure.instant/read-instant-timestamp \
             \"2020-01-01T00:00:00.000-00:00\"))"
        ),
        "true"
    );
    // Elided components default (midnight UTC on the given date).
    assert_eq!(
        run(
            &mut eval,
            "(inst-ms (clojure.instant/read-instant-date \"2020-01-01\"))"
        ),
        "1577836800000"
    );
}

#[test]
fn instant_data_readers_contract() {
    let mut eval = boot();
    require_instant(&mut eval);
    // The documented use: bind *data-readers* and read #inst through it.
    assert_eq!(
        run(
            &mut eval,
            "(binding [*data-readers* {'inst clojure.instant/read-instant-date}] \
             (load-string \"#inst \\\"2020-01-01T00:00:00.000-00:00\\\"\"))"
        ),
        "#inst \"2020-01-01T00:00:00.000-00:00\""
    );
}

#[test]
fn gvec_ns_loads_vector_of_already_native() {
    let mut eval = boot();
    run(&mut eval, "(require 'clojure.gvec)");
    assert_eq!(run(&mut eval, "(some? (find-ns 'clojure.gvec))"), "true");
    // The shim defines no vars: upstream gvec's only public API is
    // clojure.core/vector-of, which the runtime already implements.
    assert_eq!(run(&mut eval, "(ns-publics 'clojure.gvec)"), "{}");
    assert_eq!(run(&mut eval, "(vector-of :int 1 2 3)"), "[1 2 3]");
    assert_eq!(
        uncaught(&mut eval, "(vector-of :foo)"),
        "java.lang.IllegalArgumentException | Unrecognized type :foo"
    );
}

fn require_javadoc(eval: &mut Evaluator) {
    run(eval, "(require 'clojure.java.javadoc)");
}

#[test]
fn javadoc_ns_loads_with_exact_publics() {
    let mut eval = boot();
    require_javadoc(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(sort (keys (ns-publics 'clojure.java.javadoc)))"
        ),
        "(*core-java-api* *feeling-lucky* *feeling-lucky-url* *local-javadocs* \
         *remote-javadocs* add-local-javadoc add-remote-javadoc javadoc javadoc-url)"
    );
    assert_eq!(
        run(&mut eval, "@#'clojure.java.javadoc/*core-java-api*"),
        "\"https://docs.oracle.com/en/java/javase/21/docs/api/%s/\""
    );
}

#[test]
fn javadoc_remote_urls() {
    let mut eval = boot();
    require_javadoc(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"java.lang.String\")"
        ),
        "\"https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/lang/String.html\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"javax.swing.JFrame\")"
        ),
        "\"https://docs.oracle.com/en/java/javase/21/docs/api/java.desktop/javax/swing/JFrame.html\""
    );
    // Inner classes strip at the $.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"java.util.Map$Entry\")"
        ),
        "\"https://docs.oracle.com/en/java/javase/21/docs/api/java.base/java/util/Map.html\""
    );
    // Unknown prefixes answer nil.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"com.example.Foo\")"
        ),
        "nil"
    );
    // Guava entry has no %s slot: used verbatim.
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"com.google.common.base.Strings\")"
        ),
        "\"https://guava.dev/releases/snapshot-jre/api/docs/\
         com/google/common/base/Strings.html\""
    );
}

#[test]
fn javadoc_local_and_custom_remote() {
    let mut eval = boot();
    require_javadoc(&mut eval);
    // A local doc tree: <tmp>/java/lang/String.html.
    let root = std::env::temp_dir().join(format!("w5-jdoc-{}", std::process::id()));
    let page = root.join("java").join("lang").join("String.html");
    std::fs::create_dir_all(page.parent().unwrap()).unwrap();
    std::fs::write(&page, "<html></html>").unwrap();
    let root_s = root.to_string_lossy().replace('\\', "/");
    run(
        &mut eval,
        &format!("(clojure.java.javadoc/add-local-javadoc \"{root_s}\")"),
    );
    let page_s = page.to_string_lossy().replace('\\', "/");
    if page_s.starts_with('/') {
        // Absolute POSIX paths keep the exact upstream file:/// shape.
        assert_eq!(
            run(
                &mut eval,
                "(clojure.java.javadoc/javadoc-url \"java.lang.String\")"
            ),
            format!("\"file://{page_s}\"")
        );
    } else {
        // Elsewhere (Windows drive paths) pin the file: shape loosely.
        assert_eq!(
            run(
                &mut eval,
                "(let [u (clojure.java.javadoc/javadoc-url \"java.lang.String\")] \
                 [(.startsWith u \"file:\") (.endsWith u \"String.html\")])"
            ),
            "[true true]"
        );
    }
    // A custom remote entry wins for its prefix.
    run(
        &mut eval,
        "(clojure.java.javadoc/add-remote-javadoc \
         \"com.example.\" \"https://example.com/jdoc/\")",
    );
    assert_eq!(
        run(
            &mut eval,
            "(clojure.java.javadoc/javadoc-url \"com.example.Foo\")"
        ),
        "\"https://example.com/jdoc/com/example/Foo.html\""
    );
    std::fs::remove_dir_all(&root).unwrap();
}
