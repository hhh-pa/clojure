// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #32 tower tests: every probe in
//! `docs/i32-rng-tower/probes/tower.out` pinned verbatim (except
//! `promo-types`, which needs the out-of-scope `type` var — the
//! result kinds are visible in the printed values instead).

use clj_eval::{Evaluator, SourceError, boot, jvm_class_of};
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
            // can still throw at realization time.
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

/// Issue #32: `bigint` coercions (probes `bigint-*`).
#[test]
fn tower_bigint_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(bigint 5)"), "5N");
    assert_eq!(run_last(&mut eval, "(bigint 5.5)"), "5N");
    assert_eq!(run_last(&mut eval, "(bigint -5.9)"), "-5N");
    assert_eq!(run_last(&mut eval, "(bigint \"42\")"), "42N");
    assert_eq!(run_last(&mut eval, "(bigint 1/2)"), "0N");
    assert_eq!(run_last(&mut eval, "(bigint 5.9M)"), "5N");
    assert_eq!(run_last(&mut eval, "(bigint 5N)"), "5N");
    // All-zero strings are zero; a bare sign (or nothing) is the
    // JDK's zero-length failure (oracle `tower-gap`).
    assert_eq!(run_last(&mut eval, "(bigint \"0\")"), "0N");
    assert_eq!(run_last(&mut eval, "(bigint \"000\")"), "0N");
    for form in ["(bigint \"\")", "(bigint \"+\")"] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.NumberFormatException | Zero length BigInteger",
            "{form}"
        );
    }
    // Only one leading sign strips; a second fails embedded
    // (oracle `tower-r2`).
    assert_eq!(
        uncaught(&mut eval, "(bigint \"--5\")"),
        "java.lang.NumberFormatException | Illegal embedded sign character"
    );
    assert_eq!(
        run_last(&mut eval, "(bigint \"123456789012345678901234567890\")"),
        "123456789012345678901234567890N"
    );
    assert_eq!(
        run_last(&mut eval, "(bigint 1e100)"),
        "10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000N"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigint nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"val\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigint true)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigint \"x\")"),
        "java.lang.NumberFormatException | For input string: \"x\""
    );
    assert_eq!(
        uncaught(&mut eval, "(bigint \"0x10\")"),
        "java.lang.NumberFormatException | For input string: \"x10\""
    );
    assert_eq!(
        uncaught(&mut eval, "(bigint ##NaN)"),
        "java.lang.NumberFormatException | Infinite or NaN"
    );
}

/// Issue #32: `bigdec` coercions (probes `bigdec-*`).
#[test]
fn tower_bigdec_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(bigdec 5)"), "5.0M");
    assert_eq!(run_last(&mut eval, "(bigdec 5.1)"), "5.1M");
    assert_eq!(run_last(&mut eval, "(bigdec -5)"), "-5.0M");
    assert_eq!(run_last(&mut eval, "(bigdec \"5\")"), "5M");
    assert_eq!(run_last(&mut eval, "(bigdec 1/2)"), "0.5M");
    assert_eq!(run_last(&mut eval, "(bigdec 5.0M)"), "5.0M");
    assert_eq!(run_last(&mut eval, "(bigdec \"1e3\")"), "1E+3M");
    assert_eq!(run_last(&mut eval, "(bigdec \"1.5e-3\")"), "0.0015M");
    // Explicit exponent signs, and the exponent message shapes
    // (oracles `bigdec-exp`, `bigdec-expe`).
    assert_eq!(run_last(&mut eval, "(bigdec \"1e+3\")"), "1E+3M");
    assert_eq!(
        uncaught(&mut eval, "(bigdec \"1e99999999999999999999999\")"),
        "java.lang.NumberFormatException | Too many nonzero exponent digits."
    );
    for form in ["(bigdec \"1e\")", "(bigdec \"1e-\")"] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.NumberFormatException | ",
            "{form}"
        );
    }
    assert_eq!(
        run_last(
            &mut eval,
            "[(bigdec \".5\") (bigdec \"5.\") (bigdec \"+5\")]"
        ),
        "[0.5M 5M 5M]"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.toCharArray()\" because \"val\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec true)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigDecimal"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec \"x\")"),
        "java.lang.NumberFormatException | Character x is neither a decimal digit number, decimal point, nor \"e\" notation exponential mark."
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec 1/3)"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec ##NaN)"),
        "java.lang.NumberFormatException | Infinite or NaN"
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec ##Inf)"),
        "java.lang.NumberFormatException | Infinite or NaN"
    );
}

/// Issue #32: `num` / `rationalize` coercions (probes
/// `num-*`, `rat-*`).
#[test]
fn tower_num_rationalize_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(num 5)"), "5");
    assert_eq!(run_last(&mut eval, "(num 5.5)"), "5.5");
    assert_eq!(
        run_last(&mut eval, "[(num 5N) (num 5M) (num 1/2)]"),
        "[5N 5M 1/2]"
    );
    assert_eq!(run_last(&mut eval, "(num nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(num \"5\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(num true)"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(run_last(&mut eval, "(rationalize 0.5)"), "1/2");
    assert_eq!(run_last(&mut eval, "(rationalize 5)"), "5");
    assert_eq!(
        run_last(
            &mut eval,
            "[(rationalize 5N) (rationalize 5M) (rationalize 1/2)]"
        ),
        "[5N 5N 1/2]"
    );
    assert_eq!(run_last(&mut eval, "(rationalize 5.1M)"), "51/10");
    assert_eq!(run_last(&mut eval, "(rationalize 5.0M)"), "5N");
    assert_eq!(run_last(&mut eval, "(rationalize 2.5)"), "5/2");
    assert_eq!(run_last(&mut eval, "(rationalize -0.5)"), "-1/2");
    assert_eq!(run_last(&mut eval, "(rationalize -2.5)"), "-5/2");
    // Edges (oracle `tower-gap`): zero folds to `BigInt`, huge
    // magnitudes stay exact, subnormal doubles go through
    // `BigDecimal.valueOf` (`49/10^325`, not the binary `1/2^1074`).
    assert_eq!(run_last(&mut eval, "(rationalize 0.0)"), "0N");
    assert_eq!(
        run_last(&mut eval, "(rationalize 1.0E16)"),
        "10000000000000000N"
    );
    let mut subnormal = String::from("49/1");
    subnormal.push_str(&"0".repeat(325));
    assert_eq!(run_last(&mut eval, "(rationalize 5e-324)"), subnormal);
    assert_eq!(run_last(&mut eval, "(rationalize 0.0)"), "0N");
    // Doubles go through `BigDecimal.valueOf`, not the exact
    // binary expansion (oracle `rat-01`).
    assert_eq!(run_last(&mut eval, "(rationalize 0.1)"), "1/10");
    assert_eq!(
        run_last(&mut eval, "(rationalize 1e100)"),
        "10000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000000N"
    );
    assert_eq!(run_last(&mut eval, "(rationalize nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(rationalize \"0.5\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rationalize true)"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rationalize ##NaN)"),
        "java.lang.NumberFormatException | Infinite or NaN"
    );
}

/// Issue #32: reader literals (probes `read-*`; `1/0` also in
/// `loader::load_string_ratio_reads`).
#[test]
fn tower_reader_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "[1/2 5N 5M 1E+3M]"),
        "[1/2 5N 5M 1E+3M]"
    );
    assert_eq!(run_last(&mut eval, "(read-string \"4/2\")"), "2");
    assert_eq!(run_last(&mut eval, "(read-string \"6/4\")"), "3/2");
    assert_eq!(run_last(&mut eval, "(read-string \"2/1\")"), "2");
    assert_eq!(run_last(&mut eval, "(read-string \"0/5\")"), "0");
    assert_eq!(run_last(&mut eval, "(read-string \"0N\")"), "0N");
    assert_eq!(run_last(&mut eval, "(read-string \"0xFFN\")"), "255N");
    assert_eq!(run_last(&mut eval, "(read-string \"0xFF\")"), "255");
    assert_eq!(
        run_last(&mut eval, "(read-string \"0xFFFFFFFFFFFFFFFFFF\")"),
        "4722366482869645213695N"
    );
    assert_eq!(run_last(&mut eval, "(read-string \"-1/2\")"), "-1/2");
    assert_eq!(
        run_last(
            &mut eval,
            "[(read-string \"4/2\") (read-string \"5M\") (read-string \"5N\")]"
        ),
        "[2 5M 5N]"
    );
    // A divisible huge ratio folds to `BigInt`, not `Int`.
    assert_eq!(
        run_last(&mut eval, "(read-string \"100000000000000000000/10\")"),
        "10000000000000000000N"
    );
}

/// Issue #32: `=` / maps / sets over tower values (probes
/// `eq-*`, `map-tower`).
#[test]
fn tower_eq_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "[(= 5N 5) (= 5M 5.0) (= 1/2 1/2) (= 5.0M 5.00M) (== 1/2 0.5) (= 5N 5N)]"
        ),
        "[true false true true true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(= 5M 5N) (= 5N 5M) (= 1/2 5) (= 1/2 0.5) (= 5M 1/2) (= 5N 5.0) (= 0N 0)]"
        ),
        "[false false false false false false true]"
    );
    assert_eq!(
        run_last(&mut eval, "[(== 5N 5) (== 5M 5.0) (== 5.0M 5.00M)]"),
        "[true true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (bigint \"123456789012345678901234567890\") (bigint \"123456789012345678901234567890\"))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[({5.0M :a} 5.00M) ({5N :a} 5) ({1/2 :a} 1/2) (contains? #{5N} 5) (get {5N :a} 5)]"
        ),
        "[:a :a :a true :a]"
    );
    // tower `=` through `distinct`/`=` on collections.
    assert_eq!(run_last(&mut eval, "(= [5N] [5])"), "true");
    assert_eq!(run_last(&mut eval, "(= {:a 5N} {:a 5})"), "true");
    // The reversed kind order walks the mirrored arm.
    assert_eq!(run_last(&mut eval, "(= [5] [5N])"), "true");
    // Equivalent values hash equally (the `hasheq` contract —
    // no magic numbers, which stay JVM-specific).
    assert_eq!(run_last(&mut eval, "(= (hash 5N) (hash 5))"), "true");
    assert_eq!(run_last(&mut eval, "(= (hash 5.0M) (hash 5.00M))"), "true");
    assert_eq!(run_last(&mut eval, "(= (hash 1/2) (hash 1/2))"), "true");
    // Negative-scale zeros and trailing-zero decimals hash with
    // their equiv class (oracle `tower-r2`).
    assert_eq!(run_last(&mut eval, "(= 0E+3M 0M)"), "true");
    assert_eq!(run_last(&mut eval, "(= (hash 0E+3M) (hash 0M))"), "true");
    assert_eq!(run_last(&mut eval, "(= 10E+4M 1E+5M)"), "true");
    assert_eq!(
        run_last(&mut eval, "(= (hash 10E+4M) (hash 1E+5M))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (hash (bigint \"1267650600228229401496703205376\")) (hash (bigint \"1267650600228229401496703205376\")))"
        ),
        "true"
    );
    // `hash-code` is private (unreachable from user code); its
    // tower arms are pinned by unit tests in `hasheq.rs`.
}

/// Issue #32: printing (`pr` adds `N`/`M`, `str` is plain —
/// probes `str-*`, `pr-dec`).
#[test]
fn tower_print_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "[(str 5N) (str 5M) (str 1/2)]"),
        "[\"5\" \"5\" \"1/2\"]"
    );
    assert_eq!(
        run_last(&mut eval, "[(str 5.0M) (str 5.00M) (str 1E+3M)]"),
        "[\"5.0\" \"5.00\" \"1E+3\"]"
    );
    assert_eq!(
        run_last(&mut eval, "[(pr-str 5.0M) (pr-str 5.00M)]"),
        "[\"5.0M\" \"5.00M\"]"
    );
    // Adjusted-exponent rendering with a non-empty coefficient
    // tail, and exponent/mantissa error shapes (same `bad`
    // message the oracle pins for `(bigdec "x")`).
    assert_eq!(
        run_last(&mut eval, "(str (bigdec \"1.5e5\"))"),
        "\"1.5E+5\""
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec \"1eX\")"),
        "java.lang.NumberFormatException | Character X is neither a decimal digit number, decimal point, nor \"e\" notation exponential mark."
    );
    assert_eq!(
        uncaught(&mut eval, "(bigdec \"12q\")"),
        "java.lang.NumberFormatException | Character q is neither a decimal digit number, decimal point, nor \"e\" notation exponential mark."
    );
    // Ratios with terminating decimals convert (oracles `tower-gap`,
    // `tower-gap2` — the factor-5 denominators pin the second loop).
    assert_eq!(run_last(&mut eval, "(bigdec 1/2)"), "0.5M");
    assert_eq!(run_last(&mut eval, "(bigdec 1/4)"), "0.25M");
    assert_eq!(run_last(&mut eval, "(bigdec 1/5)"), "0.2M");
    assert_eq!(run_last(&mut eval, "(bigdec 3/20)"), "0.15M");
    // The scale is an `int`: past it the parse fails, while
    // zero-padded exponents still parse (oracles `tower-r2`,
    // `tower-r2d`, `tower-r2e`).
    for form in [
        "(bigdec \"1e-3000000000\")",
        "(bigdec \"1e-2147483648\")",
        "(bigdec \"1e9999999999\")",
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.NumberFormatException | Exponent overflow.",
            "{form}"
        );
    }
    assert_eq!(
        run_last(&mut eval, "(bigdec \"1e2147483647\")"),
        "1E+2147483647M"
    );
    assert_eq!(
        run_last(&mut eval, "(bigdec \"1e2147483648\")"),
        "1E+2147483648M"
    );
    assert_eq!(run_last(&mut eval, "(bigdec \"1e00000000001\")"), "1E+1M");
    assert_eq!(run_last(&mut eval, "(bigdec \"1e000\")"), "1M");
    assert_eq!(run_last(&mut eval, "(pr-str 1/2)"), "\"1/2\"");
}

/// Issue #32: predicates (probes `pred-*`).
#[test]
fn tower_pred_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "[(integer? 5N) (integer? 5M) (integer? 1/2) (number? 5N) (number? 5M) (number? 1/2) (int? 5N)]"
        ),
        "[true false false true true true false]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(zero? 0N) (pos? 5M) (neg? 1/2) (zero? 0M) (neg? -1/2)]"
        ),
        "[true true false true true]"
    );
}

/// Issue #32: `compare` / `sort` / `min` / `max` (probes
/// `sort-mixed`, `cmp-tower`, `minmax-tower`).
#[test]
fn tower_compare_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(sort [5M 1/2 5N 1 0.5 2])"),
        "(1/2 0.5 1 2 5M 5N)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(compare 1/2 0.5) (compare 0.5 1/2) (compare 5M 5.0) (compare 0.1 1/10) (compare 5N 5.0) (compare 5M 5N)]"
        ),
        "[0 0 0 0 0 0]"
    );
    // `BigDec` pairs compare scale-insensitively in both orders
    // (`=` is `compareTo`-equiv, so `(= 5.00M 5.0M)` is true while
    // `(= 5.00M 5.0)` is false — `5.0` is a `Double`;
    // oracle `tower-gap2`).
    assert_eq!(
        run_last(&mut eval, "[(= 5.00M 5.0M) (= 1.5M 1.25M)]"),
        "[true false]"
    );
    // Astronomical scales never expand: signs decide first, then
    // magnitudes (oracles `tower-r2`, `tower-r2d` — the product
    // grows its scale past `i32`, no literal can).
    assert_eq!(run_last(&mut eval, "(compare 1E+2147483648M 0M)"), "1");
    assert_eq!(
        run_last(
            &mut eval,
            "(compare (* 1E+2000000000M 1E+2000000000M 1E+2000000000M) 1M)"
        ),
        "1"
    );
    // Negative pairs decide magnitudes reversed; `BigDec` against
    // `Int` converts exactly (no expansion either way).
    assert_eq!(run_last(&mut eval, "(compare -1E+2000000000M -1M)"), "-1");
    assert_eq!(run_last(&mut eval, "[(compare 5M 1) (compare 1 5M)]"), "[1 -1]");
    assert_eq!(run_last(&mut eval, "(= 0E+3M 0M)"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "[(min 1/2 1) (max 5N 3) (max 0.5 1/2) (min 0.5 1/2)]"
        ),
        "[1/2 5N 1/2 1/2]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(< 1/2 1) (< 1/3 1/2) (> 1/2 1/3) (<= 1/2 2/4)]"
        ),
        "[true true true true]"
    );
}

/// Issue #32: the promotion lattice (probes `promo-*`).
#[test]
fn tower_lattice_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "[(+ 1/2 5) (+ 1/2 5N) (+ 1/2 5M) (+ 5N 5M) (+ 5N 5) (+ 5M 5) (+ 5M 0.5) (+ 1/2 0.5)]"
        ),
        "[11/2 11/2 5.5M 10M 10N 10M 5.5 1.0]"
    );
    assert_eq!(
        run_last(&mut eval, "[(* 1/2 5N) (- 5M 5) (/ 5N 2) (/ 1 2)]"),
        "[5/2 0M 5/2 1/2]"
    );
    // Single-arg negation, ratio subtraction folding to `BigInt`,
    // decimal multiplication and decrement.
    assert_eq!(
        run_last(&mut eval, "[(- 5N) (- 1/2) (- 5M)]"),
        "[-5N -1/2 -5M]"
    );
    assert_eq!(run_last(&mut eval, "(- 3/2 1/2)"), "1N");
    assert_eq!(run_last(&mut eval, "(* 2M 3)"), "6M");
    assert_eq!(run_last(&mut eval, "(dec 5.5M)"), "4.5M");
    assert_eq!(run_last(&mut eval, "(mod -5M 2)"), "1M");
    // Back-conversion and cross-kind comparison pins.
    assert_eq!(run_last(&mut eval, "(compare 2.0 1.0)"), "1");
    assert_eq!(run_last(&mut eval, "(double 5.5M)"), "5.5");
    assert_eq!(run_last(&mut eval, "(bigdec 5N)"), "5M");
    // Astronomical scales never expand into `double`: overflow is
    // `±Infinity`, underflow `±0.0`, exactly like the JVM.
    assert_eq!(
        run_last(&mut eval, "(str (double 1E+2000000000M))"),
        "\"##Inf\""
    );
    assert_eq!(
        run_last(&mut eval, "(str (double -1E+2000000000M))"),
        "\"##-Inf\""
    );
    assert_eq!(run_last(&mut eval, "(double 0M)"), "0.0");
    assert_eq!(run_last(&mut eval, "(str (double 1E-2000000M))"), "\"0.0\"");
    assert_eq!(
        run_last(&mut eval, "(str (double -1E-2000000M))"),
        "\"-0.0\""
    );
    // Tower zero divisors fail `Divide by zero` through `/` too
    // (oracle `div-tower0`).
    for form in ["(/ 5M 0M)", "(/ 1 0N)"] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.ArithmeticException | Divide by zero",
            "{form}"
        );
    }
    // `(/ 1 2)` keeps the `Int`-only wedge (`Double`, not a
    // ratio — tower division is exact).
    assert_eq!(
        run_last(
            &mut eval,
            "[(inc 5M) (inc 1/2) (dec 1/2) (dec 5N) (inc 5N) (+ 1N 2N)]"
        ),
        "[6M 3/2 -1/2 4N 6N 3N]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(long 5M) (long 1/2) (long 5.9M) (int 1/2) (double 1/2) (double 5N) (float 1/2) (long 5N) (float 5N)]"
        ),
        "[5 0 5 0 0.5 5.0 0.5 5 5.0]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(== 5N 5) (== 5M 5.0) (== 5.0M 5.00M) (< 1/2 1) (compare 5M 5N) (zero? 0M) (neg? -1/2)]"
        ),
        "[true true true true 0 true true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(quot 7N 2N) (rem 7N 2N) (mod 7N 2N) (quot 7N 2) (quot 7 2N) (quot 5M 2) (quot 1/2 1)]"
        ),
        "[3N 1N 1N 3N 3N 2M 0N]"
    );
    // Runtime ratio ops fold integral results to `BigInt`
    // (oracle `fold-add`; the reader folds to `Int` instead —
    // see `tower_reader_rows`).
    assert_eq!(
        run_last(&mut eval, "[(+ 1/2 1/2) (+ 3/2 1/2) (* 1/2 2N) (/ 4N 2N)]"),
        "[1N 2N 1N 2N]"
    );
    assert_eq!(
        run_last(&mut eval, "(take 3 (iterate (partial + 1/2) 0))"),
        "(0 1/2 1N)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(rem 5M 2) (mod 5M 2) (rem 1/2 1) (mod 7N -2N) (mod -7N 2N) (rem 7N -2N)]"
        ),
        "[1M 1M 1/2 -1N 1N 1N]"
    );
    // Tower zero divisors fail `Divide by zero` (the `Int`
    // zero divisors keep their own text, pinned in
    // `arith::div_zero_rows`).
    for form in [
        "(quot 5M 0M)",
        "(quot 5 0N)",
        "(rem 5M 0M)",
        "(mod 1/2 0N)",
        "(quot 5M 0)",
        "(rem 5 0N)",
    ] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.ArithmeticException | Divide by zero",
            "{form}"
        );
    }
    // Inexact decimal division fails like `(bigdec 1/3)`.
    assert_eq!(
        uncaught(&mut eval, "(/ 1M 3M)"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    assert_eq!(
        uncaught(&mut eval, "(+ 1/3 5M)"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
}

/// Issue #32: checked tower conversions (probes `conv-*`).
#[test]
fn tower_conv_rows() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(int (bigint \"4294967296\"))"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        run_last(&mut eval, "[(int (bigint \"-1\")) (int 7/2) (int 5.9M)]"),
        "[-1 3 5]"
    );
    assert_eq!(
        uncaught(&mut eval, "(long (bigint \"18446744073709551616\"))"),
        "java.lang.IllegalArgumentException | Value out of range for long: 18446744073709551616"
    );
    // Past-`i64` integers fail the `long` check with the exact
    // digits; decimals fail it with the `double` rendering
    // (oracles `conv-hugeint`, `conv-hugedecint`, `conv-declong`,
    // `conv-ratlong`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(int (bigint \"170141183460469231731687303715884105728\"))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 170141183460469231731687303715884105728"
    );
    assert_eq!(
        uncaught(&mut eval, "(int 1E+30M)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E30"
    );
    assert_eq!(
        uncaught(&mut eval, "(long 1E+30M)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E30"
    );
    assert_eq!(
        uncaught(&mut eval, "(long (/ (bigint \"99999999999999999999\") 3))"),
        "java.lang.IllegalArgumentException | Value out of range for long: 33333333333333333333"
    );
    // Past-`f32` ratios fail with their own rendering, decimals
    // with the decimal rendering (oracles `conv-ratfloat`,
    // `conv-hugedecfloat`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(float (/ (bigint \"12676506002282294014967032053761234567890\") 3))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for float: 12676506002282294014967032053761234567890/3"
    );
    assert_eq!(
        uncaught(&mut eval, "(float 1E+300M)"),
        "java.lang.IllegalArgumentException | Value out of range for float: 1E+300"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(long (bigint \"-1\")) (long -7/2) (long 5.9M)]"
        ),
        "[-1 -3 5]"
    );
    // In-range tower floats keep the exact `double` (past-`f32`
    // fails with the exact digits — oracle `float-huge`; the
    // `Float`-vs-`Double` printing gap for lossy values is a
    // documented pre-existing limitation, so the tests pin
    // tower/`double` consistency instead of the digits).
    assert_eq!(
        run_last(
            &mut eval,
            "(= (float (bigint \"1267650600228229401496703205376\")) (double (bigint \"1267650600228229401496703205376\")))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(== (float (bigint \"1267650600228229401496703205376\")) (bigint \"1267650600228229401496703205376\"))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(float (bigint \"12676506002282294014967032053761234567890\"))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for float: 12676506002282294014967032053761234567890"
    );
    assert_eq!(
        uncaught(&mut eval, "(float 1e300)"),
        "java.lang.IllegalArgumentException | Value out of range for float: 1.0E300"
    );
}

/// Issue #32: `short`/`byte`/`char` tower narrowings (oracles
/// `tower-r2`, `tower-r2d`, `tower-r2e`, `tower-r2f`,
/// `tower-r2g`).
#[test]
fn tower_short_byte_char_rows() {
    let mut eval = boot();
    // `short`/`byte` narrow through the `long` stop (a `BigInt`
    // past `i64` names `long`; decimals go through `double`).
    assert_eq!(
        run_last(
            &mut eval,
            "[(short 5N) (short 7/2) (byte 5M) (byte 7/2) (byte 5N)]"
        ),
        "[5 3 5 3 5]"
    );
    assert_eq!(
        uncaught(&mut eval, "(short (bigint \"9223372036854775808\"))"),
        "java.lang.IllegalArgumentException | Value out of range for long: 9223372036854775808"
    );
    assert_eq!(
        uncaught(&mut eval, "(short 1E300M)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    assert_eq!(
        uncaught(&mut eval, "(short 40000)"),
        "java.lang.IllegalArgumentException | Value out of range for short: 40000"
    );
    assert_eq!(
        uncaught(&mut eval, "(byte -129)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: -129"
    );
    assert_eq!(
        uncaught(&mut eval, "(byte 1E300M)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    // `char`: unbounded integers check whole, ratios truncate
    // first, decimals go through `BigDecimal.intValue` (so
    // `(char 1E300M)` is NUL, not a range error).
    assert_eq!(
        run_last(&mut eval, "[(char 65N) (char 65.0) (char 66M) (char 7/2)]"),
        "[\\A \\A \\B \\\u{3}]"
    );
    assert_eq!(run_last(&mut eval, "(char 1E300M)"), "\\\u{0}");
    assert_eq!(
        uncaught(&mut eval, "(char (bigint \"4294967296\"))"),
        "java.lang.IllegalArgumentException | Value out of range for char: 4294967296"
    );
    assert_eq!(
        uncaught(&mut eval, "(char (bigint \"9223372036854775808\"))"),
        "java.lang.IllegalArgumentException | Value out of range for char: 9223372036854775808"
    );
    assert_eq!(
        uncaught(&mut eval, "(char -5.9M)"),
        "java.lang.IllegalArgumentException | Value out of range for char: -5"
    );
    assert_eq!(
        uncaught(&mut eval, "(char 55296M)"),
        "java.lang.IllegalArgumentException | Value out of range for char: 55296"
    );
    assert_eq!(
        uncaught(&mut eval, "(char 2000000000M)"),
        "java.lang.IllegalArgumentException | Value out of range for char: 2000000000"
    );
}

/// Issue #32: `nth`/`subvec` truncate tower indices toward zero,
/// then range-check (oracles `nth-trunc2`, `nth-big`,
/// `nth-intrange*`, `subvec-tower`).
#[test]
fn tower_nth_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "[(nth [1 2 3] 1/2) (nth [1 2 3 4 5 6] 5.9M) (nth [1] 2N -1) (nth [1 2 3 4] 7/2)]"
        ),
        "[1 6 -1 4]"
    );
    // A truncated-out-of-bounds index fails like any other.
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2 3] 7/2)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(nth [1] (bigint \"99999999999999999999\"))"),
        "java.lang.IllegalArgumentException | Value out of range for long: 99999999999999999999"
    );
    // A true (non-folding) ratio truncates first, then checks
    // (oracle `nth-truerat`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(nth [1] (/ (bigint \"99999999999999999999\") 3))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 33333333333333333333"
    );
    for form in ["(nth [1] 4294967296N)", "(nth [1] 8589934593/2)"] {
        assert_eq!(
            uncaught(&mut eval, form),
            "java.lang.ArithmeticException | integer overflow",
            "{form}"
        );
    }
    // A `2^63` double saturates to `i64::MAX`, so the bounds check
    // reports `integer overflow` exactly like the JVM (oracle
    // `tower-r2`): failing the `long` stop first would wrongly
    // report `out of range for long`.
    assert_eq!(
        uncaught(&mut eval, "(nth [1 2 3] 9.223372036854776E18)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        run_last(&mut eval, "[(subvec [1 2 3] 1N) (subvec [1 2 3] 1/2 2N)]"),
        "[[2 3] [1 2]]"
    );
}

/// Issue #32: tower counts through the seq machinery (probes
/// `index-*`, `seq-tower`).
#[test]
fn tower_seq_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "[(drop 2N [1 2 3]) (take 2N [1 2 3]) (drop 1/2 [1 2 3]) (nth [1 2 3] 1N)]"
        ),
        "[(3) (1 2) (2 3) 2]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(doall (drop (bigint \"99999999999999999999\") [1]))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E20"
    );
    // Decimal counts round up like `double` counts; `repeat`
    // truncates tower counts like `intValue`.
    assert_eq!(run_last(&mut eval, "(drop 1.5M [1 2 3])"), "(3)");
    assert_eq!(run_last(&mut eval, "(take 1.5M [1 2 3])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(take 5.9M [1 2 3])"), "(1 2 3)");
    // `2^63` doubles saturate, never throw, on the lazy paths —
    // `repeat` keeps the huge count and `take` just takes (oracles
    // `tower-r2`); `drop` int-casts and reports `integer overflow`.
    assert_eq!(
        run_last(&mut eval, "(take 2 (repeat 9.223372036854776E18 \"x\"))"),
        "(\"x\" \"x\")"
    );
    assert_eq!(
        run_last(&mut eval, "(take 9.223372036854776E18 [1 2 3])"),
        "(1 2 3)"
    );
    assert_eq!(
        uncaught(&mut eval, "(drop 9.223372036854776E18 [1 2 3])"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(run_last(&mut eval, "(count (repeat 5.9M \"x\"))"), "5");
    assert_eq!(run_last(&mut eval, "(count (repeat 7/2 \"x\"))"), "3");
    // A plain-`Int` index past `i32` fails `integer overflow`.
    assert_eq!(
        uncaught(&mut eval, "(nth [1] 2147483648)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "[(take 2N [1 2 3]) (repeat 2N \"x\") (partition 3N [1 2 3]) (partition 3.0 [1 2 3])]"
        ),
        "[(1 2) (\"x\" \"x\") ((1 2 3)) ()]"
    );
    assert_eq!(run_last(&mut eval, "(range 0 5N)"), "(0 1 2 3 4)");
    assert_eq!(run_last(&mut eval, "(range 0 3/2 1/2)"), "(0 1/2 1N)");
    // Tower ranges step with `Numbers.add` (verbatim kinds) and
    // compare exactly (oracles `range-tower2`, `range-counted`).
    assert_eq!(run_last(&mut eval, "(range 0 2N)"), "(0 1)");
    assert_eq!(run_last(&mut eval, "(range 0N 3)"), "(0N 1N 2N)");
    assert_eq!(run_last(&mut eval, "(range 1/2 3/2 1/2)"), "(1/2 1N)");
    assert_eq!(run_last(&mut eval, "(range 0 2 1N)"), "(0 1N)");
    assert_eq!(
        run_last(
            &mut eval,
            "[(counted? (range 0 5)) (counted? (range 0 5N)) (counted? (range 0 5.0))]"
        ),
        "[true false false]"
    );
    // A `BigDec`/ratio step with no exact decimal raises on the
    // first advance (oracle `range-mixed`).
    assert_eq!(
        uncaught(&mut eval, "(doall (take 3 (range 0M 2 1/3)))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // Chunk forcing advances too: even `take 1` raises
    // (oracle `range-take1`).
    assert_eq!(
        uncaught(&mut eval, "(take 1 (range 0M 2 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    assert_eq!(
        uncaught(&mut eval, "(count (range 0M 2 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // `rest` advances once, so it raises too — while `hash`
    // realizes first and raises the same way.
    assert_eq!(
        uncaught(&mut eval, "(rest (range 0M 2 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    assert_eq!(
        uncaught(&mut eval, "(hash (range 0M 2 1/3))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // The mirrored kind order raises identically.
    assert_eq!(
        uncaught(&mut eval, "(take 1 (range 1/3 2 1M))"),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    // Exact walks over decimal and big-integer steps (oracles
    // `range-bigdec`, `range-bigstep`).
    assert_eq!(
        run_last(&mut eval, "(range 0M 2M 0.5M)"),
        "(0M 0.5M 1.0M 1.5M)"
    );
    assert_eq!(run_last(&mut eval, "(range 0N 3N 1N)"), "(0N 1N 2N)");
    // A decimal start over an `Int` end with a `BigInt` step
    // stays decimal (oracle `range-mixstep`).
    assert_eq!(run_last(&mut eval, "(range 0M 2 1N)"), "(0M 1M)");
    assert_eq!(run_last(&mut eval, "(float 5.5M)"), "5.5");
    // Double walks keep the start verbatim and narrow the rest
    // (oracle `range-dwalk`).
    assert_eq!(run_last(&mut eval, "(range 0N 2.5)"), "(0N 1N 2N)");
    assert_eq!(run_last(&mut eval, "(range 0.5 2N)"), "(0.5 1.5)");
    assert_eq!(run_last(&mut eval, "(range 0M 2.5)"), "(0M 1M 2M)");
    // Ratios narrow for `double` bound checks and walks (oracle
    // `range-ratd`).
    assert_eq!(run_last(&mut eval, "(range 1/2 2.5)"), "(1/2 3/2)");
    assert_eq!(
        run_last(&mut eval, "(range 0 2.5 1/2)"),
        "(0 1/2 1N 3/2 2N)"
    );
    // A negative tower step walks down; a zero tower step
    // repeats the start.
    assert_eq!(run_last(&mut eval, "(range 5N 0 -1N)"), "(5N 4N 3N 2N 1N)");
    assert_eq!(run_last(&mut eval, "(take 2 (range 5 0 0N))"), "(5 5)");
    // An `Int`+`Int` overflow ends the walk (oracle
    // `range-overflow`).
    assert_eq!(
        run_last(
            &mut eval,
            "(rest (range 9223372036854775800 9223372036854775807N 100))"
        ),
        "()"
    );
}

/// Issue #32: tower class names surface in casts (the out-of-scope
/// `type` var cannot name them; a call through one does).
#[test]
fn tower_class_rows() {
    let mut eval = boot();
    for (expr, class) in [
        ("(5N 1)", "clojure.lang.BigInt"),
        ("(5M 1)", "java.math.BigDecimal"),
        ("(1/2 1)", "clojure.lang.Ratio"),
    ] {
        let got = uncaught(&mut eval, expr);
        assert!(got.contains(class), "{expr} names {class}: {got}");
    }
    for (src, class) in [
        ("5N", "clojure.lang.BigInt"),
        ("5M", "java.math.BigDecimal"),
        ("1/2", "clojure.lang.Ratio"),
    ] {
        let vs = eval.eval_source(src).unwrap();
        assert_eq!(jvm_class_of(&vs[0]), class, "{src}");
    }
}
