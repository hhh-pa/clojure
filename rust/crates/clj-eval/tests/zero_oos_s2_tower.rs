// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S2 tower math (issue #44): probe replay for `abs` /
//! `biginteger` / `decimal?` / `denominator` / `numerator` /
//! `ratio?` / `rational?` / `*math-context*` / `with-precision`
//! (see `docs/zero-oos-s2-tower/probes/towermath*.{clj,out}`,
//! nine rounds). Every `throwof` label replays verbatim except
//! the documented port mappings (bad `:rounding` modes fail at
//! the runtime ctor, not at compile time) and the untestable
//! `type` pins (`type` is S16).

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

/// Normalize `0x<hex>` identity hashes (boot-id dependent).
fn normid(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut rest = s;
    while let Some(i) = rest.find("0x") {
        out.push_str(&rest[..i + 2]);
        out.push('H');
        rest = &rest[i + 2..];
        let cut = rest
            .find(|c: char| !c.is_ascii_hexdigit())
            .unwrap_or(rest.len());
        rest = &rest[cut..];
    }
    out.push_str(rest);
    out
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

/// Issue #44: kind predicates (probes `pred-*`).
#[test]
fn s2_predicate_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(decimal? 5.0M)", "true"),
        ("(decimal? 5)", "false"),
        ("(decimal? 5.0)", "false"),
        ("(decimal? 5N)", "false"),
        ("(decimal? 1/2)", "false"),
        ("(decimal? nil)", "false"),
        ("(decimal? \"5\")", "false"),
        ("(ratio? 1/2)", "true"),
        ("(ratio? 5)", "false"),
        ("(ratio? 5.0)", "false"),
        ("(ratio? 5N)", "false"),
        ("(ratio? 5.0M)", "false"),
        ("(ratio? nil)", "false"),
        // `rational?` includes decimals (upstream `or`).
        ("(rational? 5)", "true"),
        ("(rational? 5N)", "true"),
        ("(rational? 1/2)", "true"),
        ("(rational? 5.0M)", "true"),
        ("(rational? 5.0)", "false"),
        ("(rational? nil)", "false"),
        ("(rational? \"5\")", "false"),
        ("(rational? true)", "false"),
        ("(rational? [1])", "false"),
        ("(rational? inc)", "false"),
        ("(integer? 5N)", "true"),
        ("(integer? 5.0M)", "false"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `biginteger` coercions (probes `bi-*`). Results
/// print BARE (`5`, never `5N` — the `BigInteger` kind).
#[test]
fn s2_biginteger_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(biginteger 5)", "5"),
        ("(biginteger -5)", "-5"),
        ("(biginteger 5.5)", "5"),
        ("(biginteger -5.9)", "-5"),
        ("(biginteger 5N)", "5"),
        ("(biginteger 5.9M)", "5"),
        ("(biginteger -5.9M)", "-5"),
        ("(biginteger 7/2)", "3"),
        ("(biginteger -7/2)", "-3"),
        ("(biginteger \"42\")", "42"),
        ("(biginteger \"0\")", "0"),
        ("(pr-str (biginteger 5))", "\"5\""),
        ("(str (biginteger 5))", "\"5\""),
        ("(= (bigint 5) (biginteger 5))", "true"),
        ("(pr-str (bigint 5))", "\"5N\""),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(biginteger \"\")",
            "java.lang.NumberFormatException | Zero length BigInteger",
        ),
        (
            "(biginteger \"x\")",
            "java.lang.NumberFormatException | For input string: \"x\"",
        ),
        (
            "(biginteger \"0x10\")",
            "java.lang.NumberFormatException | For input string: \"x10\"",
        ),
        (
            "(biginteger nil)",
            "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"val\" is null",
        ),
        (
            "(biginteger true)",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger",
        ),
        (
            "(biginteger \\a)",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger",
        ),
        (
            "(biginteger ##NaN)",
            "java.lang.NumberFormatException | Infinite or NaN",
        ),
        (
            "(biginteger ##Inf)",
            "java.lang.NumberFormatException | Infinite or NaN",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `numerator` / `denominator` (probes `nd-*`,
/// `r2-den-nil`, `r9-numbi`). Results are bare `BigInteger`s.
#[test]
fn s2_numerator_denominator_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(do 6/3)", "2"),
        ("(numerator 3/4)", "3"),
        ("(denominator 3/4)", "4"),
        ("(numerator -3/4)", "-3"),
        ("(denominator -3/4)", "4"),
        ("(numerator 1/2)", "1"),
        ("(denominator 1/2)", "2"),
        ("(pr-str (numerator 3/4))", "\"3\""),
        ("(pr-str (denominator 3/4))", "\"4\""),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(numerator 5)",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Ratio (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Ratio is in unnamed module of loader 'app')",
        ),
        (
            "(denominator 5N)",
            "java.lang.ClassCastException | class clojure.lang.BigInt cannot be cast to class clojure.lang.Ratio (clojure.lang.BigInt and clojure.lang.Ratio are in unnamed module of loader 'app')",
        ),
        (
            "(numerator 5.0M)",
            "java.lang.ClassCastException | class java.math.BigDecimal cannot be cast to class clojure.lang.Ratio (java.math.BigDecimal is in module java.base of loader 'bootstrap'; clojure.lang.Ratio is in unnamed module of loader 'app')",
        ),
        (
            "(numerator nil)",
            "java.lang.NullPointerException | Cannot read field \"numerator\" because \"r\" is null",
        ),
        (
            "(denominator nil)",
            "java.lang.NullPointerException | Cannot read field \"denominator\" because \"r\" is null",
        ),
        (
            "(denominator \"x\")",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.Ratio (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.Ratio is in unnamed module of loader 'app')",
        ),
        (
            "(numerator (biginteger 5))",
            "java.lang.ClassCastException | class java.math.BigInteger cannot be cast to class clojure.lang.Ratio (java.math.BigInteger is in module java.base of loader 'bootstrap'; clojure.lang.Ratio is in unnamed module of loader 'app')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `abs` (probes `abs-*`, `r9-absbi`).
#[test]
fn s2_abs_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(abs 5)", "5"),
        ("(abs -5)", "5"),
        ("(abs 0)", "0"),
        ("(abs -9223372036854775808)", "-9223372036854775808"),
        ("(abs -5.5)", "5.5"),
        ("(abs -0.0)", "0.0"),
        ("(abs 0.0)", "0.0"),
        ("(abs ##NaN)", "##NaN"),
        ("(abs ##Inf)", "##Inf"),
        ("(abs ##-Inf)", "##Inf"),
        ("(abs -5N)", "5N"),
        ("(abs -5.9M)", "5.9M"),
        ("(abs 5.9M)", "5.9M"),
        ("(with-precision 1 (abs 9.99M))", "1E+1M"),
        ("(abs -7/2)", "7/2"),
        ("(abs (biginteger -5))", "5N"),
        ("(with-precision 1 (abs -9.99M))", "1E+1M"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(abs nil)",
            "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null",
        ),
        (
            "(abs \"x\")",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(abs true)",
            "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
        (
            "(abs \\a)",
            "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `*math-context*` root and behavior (probes
/// `mc-*`, `wp-mc*`, `wp-str`, `wp-eq*`, `r2-eqrm*`).
#[test]
fn s2_math_context_rows() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "*math-context*"), "nil");
    assert_eq!(run_last(&mut eval, "(bound? #'*math-context*)"), "true");
    // Divergence, documented: upstream binds `*math-context*`
    // (to nil) in the `clojure.main` thread frame (oracle
    // `mc-thread2`), so `thread-bound?` is true there. The port
    // boots a minimal frame (`*ns*` only — pinned by
    // `vars.rs`), so here it is false.
    assert_eq!(
        run_last(&mut eval, "(thread-bound? #'*math-context*)"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(:name (meta #'*math-context*))"),
        "*math-context*"
    );
    assert_eq!(
        run_last(&mut eval, "(:dynamic (meta #'*math-context*))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(ns-name (:ns (meta #'*math-context*)))"),
        "clojure.core"
    );
    assert_eq!(
        normid(&run_last(&mut eval, "(with-precision 10 *math-context*)")),
        "#object[java.math.MathContext 0xH \"precision=10 roundingMode=HALF_UP\"]"
    );
    assert_eq!(
        normid(&run_last(
            &mut eval,
            "(with-precision 10 :rounding FLOOR *math-context*)"
        )),
        "#object[java.math.MathContext 0xH \"precision=10 roundingMode=FLOOR\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-precision 10 (str *math-context*))"),
        "\"precision=10 roundingMode=HALF_UP\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-precision 10 (= *math-context* *math-context*))"
        ),
        "true"
    );
    // Structural `=` like `MathContext.equals` (oracles
    // `wp-eq2`/`wp-eq3`/`r2-eqrm*`).
    assert_eq!(
        run_last(
            &mut eval,
            "(= (with-precision 10 *math-context*) (with-precision 10 *math-context*))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (with-precision 10 *math-context*) (with-precision 11 *math-context*))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (with-precision 10 *math-context*) (with-precision 10 :rounding FLOOR *math-context*))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(= (with-precision 10 *math-context*) (with-precision 10 :rounding HALF_UP *math-context*))"
        ),
        "true"
    );
}

/// Issue #44: which ops consult the context (probes `wp-*`,
/// `r2-*`, `r3-rem`, `r3-eqdiv`, `r3-quotscale`, `r5-*`, `r7-*`,
/// `r8-quotscale`, `r3-quotmcscale`, `r8-remz`).
#[test]
fn s2_precision_consultation_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(with-precision 3 (+ 1.234M 5.678M))", "6.91M"),
        ("(with-precision 3 (- 5.678M 1.234M))", "4.45M"),
        ("(with-precision 3 (* 1.234M 5.678M))", "7.01M"),
        ("(with-precision 3 (/ 1M 3M))", "0.333M"),
        ("(with-precision 3 (/ 1M 2M))", "0.5M"),
        ("(with-precision 3 (+ 1M 1))", "2M"),
        ("(with-precision 3 (+ 1 1M))", "2M"),
        ("(with-precision 3 (/ 1 3M))", "0.333M"),
        ("(with-precision 3 (+ 1/3 1M))", "1.33M"),
        ("(with-precision 1 (+ 100 200))", "300"),
        ("(with-precision 1 (+ 0.1 0.2))", "0.30000000000000004"),
        ("(with-precision 1 (/ 1 3))", "1/3"),
        ("(with-precision 1 (quot 5.678M 2M))", "2M"),
        ("(with-precision 1 (rem 5.678M 2M))", "1.678M"),
        ("(with-precision 1 (mod 5.678M 2M))", "1.678M"),
        ("(with-precision 2 (inc 9.99M))", "11M"),
        ("(with-precision 2 (dec 9.99M))", "9.0M"),
        ("(with-precision 2 (- 9.99M))", "-10M"),
        ("(with-precision 3 (bigdec 1/3))", "0.333M"),
        ("(with-precision 1 (bigdec 5.5))", "5.5M"),
        ("(with-precision 1 (bigdec 5))", "5.0M"),
        ("(with-precision 10)", "nil"),
        ("(with-precision 3 1 2)", "2"),
        (
            "(with-precision 3 (with-precision 5 (/ 1M 3M)))",
            "0.33333M",
        ),
        (
            "(do [(try (with-precision 3 (throw (Exception. \"x\"))) (catch Exception e :caught)) *math-context*])",
            "[:caught nil]",
        ),
        // Post-rounding for `+`/`*`, add-of-negate for `-`.
        ("(with-precision 1 (+ 2.5M 2.5M))", "5M"),
        ("(with-precision 3 (- 1.234M 5.678M))", "-4.45M"),
        ("(with-precision 1 (- 2.5M 2.4M))", "0.5M"),
        ("(with-precision 1 (* 1.5M 1.5M))", "2M"),
        ("(with-precision 3 (- 5.678M 1.234M 0.001M))", "4.45M"),
        ("(with-precision 1 (quot 55.678M 1.1M))", "5E+1M"),
        ("(with-precision 1 (+ 1/3 1/6))", "1/2"),
        ("(with-precision 3 (* 2M 1/3))", "0.666M"),
        ("(with-precision 1 (rem 55.678M 1.1M))", "0.678M"),
        ("(with-precision 1 (= 0.3M (/ 1M 3M)))", "true"),
        ("(with-precision 3 (- 5.678M 5.678M))", "-0.002M"),
        ("(with-precision 3 (- 1M 1/3))", "0.667M"),
        ("(with-precision 3 (/ 0M 3M))", "0M"),
        ("(with-precision 3 (/ 0.0M 3M))", "0.0M"),
        ("(with-precision 1 (+ 0.0M 0.0M))", "0.0M"),
        ("(binding [*math-context* 5] (+ 1 2))", "3"),
        ("(with-precision 3 (quot 1/3 1M))", "0.000M"),
        ("(with-precision 3 (rem 1/3 1M))", "0.333M"),
        ("(with-precision 5 (quot 5.678M 2.5M))", "2.00M"),
        ("(with-precision 5 (quot 1E+3M 3M))", "333M"),
        // L7: the no-context baseline (`r3-quotscale`).
        ("(quot 1E+3M 3M)", "333M"),
        ("(rem 0.333M 1M)", "0.333M"),
        // Asymmetric modes (order pins live here too).
        ("(with-precision 1 :rounding CEILING (- 2.5M 2.4M))", "0.5M"),
        ("(with-precision 1 :rounding FLOOR (- 2.5M 2.4M))", "-0.5M"),
        ("(with-precision 1 :rounding CEILING (- 2.4M))", "-2M"),
        ("(with-precision 1 :rounding FLOOR (- 2.4M))", "-3M"),
        (
            "(with-precision 1 :rounding CEILING (+ -2.5M -2.4M))",
            "-4M",
        ),
        ("(with-precision 1 :rounding CEILING (* 2.5M 1.1M))", "3M"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(+ 1/3 1M)",
            "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result.",
        ),
        (
            "(with-precision 0 (/ 1M 3M))",
            "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result.",
        ),
        (
            "(quot 1/3 1M)",
            "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result.",
        ),
        (
            "(rem 1/3 1M)",
            "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result.",
        ),
        (
            "(binding [*math-context* 5] (+ 1M 1M))",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.math.MathContext (java.lang.Long and java.math.MathContext are in module java.base of loader 'bootstrap')",
        ),
        (
            "(binding [*math-context* \"x\"] (/ 1M 3M))",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.math.MathContext (java.lang.String and java.math.MathContext are in module java.base of loader 'bootstrap')",
        ),
        (
            "(binding [*math-context* 5] (quot 1M 1M))",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.math.MathContext (java.lang.Long and java.math.MathContext are in module java.base of loader 'bootstrap')",
        ),
        (
            "(binding [*math-context* 5] (rem 1M 1M))",
            "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.math.MathContext (java.lang.Long and java.math.MathContext are in module java.base of loader 'bootstrap')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: every rounding mode (probes `wp-halfup`..`wp-unnec-*`).
#[test]
fn s2_rounding_mode_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(with-precision 2 (/ 1M 8M))", "0.13M"),
        ("(with-precision 2 :rounding HALF_DOWN (/ 1M 8M))", "0.12M"),
        ("(with-precision 2 :rounding HALF_EVEN (/ 1M 8M))", "0.12M"),
        ("(with-precision 2 :rounding DOWN (/ 1M 8M))", "0.12M"),
        ("(with-precision 2 :rounding UP (/ 1M 8M))", "0.13M"),
        ("(with-precision 2 :rounding FLOOR (/ 1M 8M))", "0.12M"),
        ("(with-precision 2 :rounding CEILING (/ 1M 8M))", "0.13M"),
        ("(with-precision 2 :rounding FLOOR (/ -1M 8M))", "-0.13M"),
        ("(with-precision 2 :rounding CEILING (/ -1M 8M))", "-0.12M"),
        ("(with-precision 2 :rounding DOWN (/ -1M 8M))", "-0.12M"),
        ("(with-precision 2 :rounding UP (/ -1M 8M))", "-0.13M"),
        ("(with-precision 2 :rounding UNNECESSARY (/ 1M 2M))", "0.5M"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-precision 2 :rounding UNNECESSARY (/ 1M 8M))"
        ),
        "java.lang.ArithmeticException | Rounding necessary"
    );
}

/// Issue #44: ctor validation (probes `wp-negprec`,
/// `wp-fracprec`, `wp-strprec`, `wp-nilprec`, `wp-bigprec`,
/// `wp-badmode`, `wp-lowmode`, `r2-str*`, `r3-sp2`..`r3-junk`,
/// `r3-lowmode`, `r4-*`, `r7-strmode`). Bad `:rounding` modes
/// fail the `valueOf` text (upstream fails at compile with a
/// path-laden `CompilerException`); the port also accepts
/// keyword/string modes by exact name (documented superset).
#[test]
fn s2_precision_ctor_rows() {
    let mut eval = boot();
    for (form, want) in [
        (
            "(with-precision \"precision=3 roundingMode=HALF_UP\" (/ 1M 3M))",
            "0.333M",
        ),
        (
            "(with-precision \"precision=3 roundingMode=FLOOR\" (str *math-context*))",
            "\"precision=3 roundingMode=FLOOR\"",
        ),
        (
            "(with-precision \"precision=+3 roundingMode=HALF_UP\" (/ 1M 3M))",
            "0.333M",
        ),
        // Keyword/string modes: port superset (exact names).
        ("(with-precision 2 :rounding :HALF_DOWN (/ 1M 8M))", "0.12M"),
        (
            "(with-precision 2 :rounding \"HALF_DOWN\" (/ 1M 8M))",
            "0.12M",
        ),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(with-precision -1 1)",
            "java.lang.IllegalArgumentException | Digits < 0",
        ),
        (
            "(with-precision 2.5 1)",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.MathContext",
        ),
        (
            "(with-precision \"3\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision nil 1)",
            "java.lang.NullPointerException | null String",
        ),
        (
            "(with-precision 10N 1)",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.MathContext",
        ),
        (
            "(with-precision true 1)",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.MathContext",
        ),
        (
            "(with-precision 3000000000 1)",
            "java.lang.ArithmeticException | integer overflow",
        ),
        (
            "(with-precision 10 :rounding FOO 1)",
            "java.lang.IllegalArgumentException | No enum constant java.math.RoundingMode.FOO",
        ),
        (
            "(with-precision 10 :rounding half_up 1)",
            "java.lang.IllegalArgumentException | No enum constant java.math.RoundingMode.half_up",
        ),
        (
            "(with-precision 10 :rounding)",
            "java.lang.IllegalArgumentException | No enum constant java.math.RoundingMode.",
        ),
        (
            "(with-precision \"precision=3  roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \" precision=3 roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingMode=HALF_UP \" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingmode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision= 3 roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingMode=HALF_UP junk\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=x roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingMode=FOO\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=-3 roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | Digits < 0",
        ),
        (
            "(with-precision \"\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=99999999999999999999 roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision= roundingMode=HALF_UP\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingMode=half_up\" 1)",
            "java.lang.IllegalArgumentException | bad string format",
        ),
        (
            "(with-precision \"precision=3 roundingMode=HALF_UP\" :rounding FLOOR 1)",
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `BigInteger` membership and absorption (probes
/// `r8-*`, `r9-*`). `int?` says false (no S1 amend); `NaN?` /
/// `infinite?` / `nat-int?` say false (S1 composition).
#[test]
fn s2_biginteger_membership_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(int? (biginteger 5))", "false"),
        ("(integer? (biginteger 5))", "true"),
        ("(number? (biginteger 5))", "true"),
        ("(rational? (biginteger 5))", "true"),
        ("(ratio? (biginteger 5))", "false"),
        ("(decimal? (biginteger 5))", "false"),
        ("(float? (biginteger 5))", "false"),
        ("(= (hash 5N) (hash (biginteger 5)))", "true"),
        ("(= (hash 5) (hash (biginteger 5)))", "true"),
        ("(bigint (biginteger 5))", "5N"),
        ("(bigdec (biginteger 5))", "5M"),
        ("(num (biginteger 5))", "5"),
        ("(rationalize (biginteger 5))", "5"),
        ("(+ (biginteger 5) 1)", "6N"),
        ("(+ (biginteger 5) 2N)", "7N"),
        ("(* (biginteger 5) 2)", "10N"),
        ("(/ (biginteger 6) 2)", "3N"),
        ("(/ (biginteger 1) 2)", "1/2"),
        ("(inc (biginteger 5))", "6N"),
        ("(quot (biginteger 7) 2)", "3N"),
        ("(< (biginteger 5) 6)", "true"),
        ("(int (biginteger 5))", "5"),
        ("(long (biginteger 5))", "5"),
        ("(double (biginteger 5))", "5.0"),
        ("(char (biginteger 65))", "\\A"),
        ("(= 5N (biginteger 5))", "true"),
        ("(= 5 (biginteger 5))", "true"),
        ("(= (biginteger 5) 5.0M)", "false"),
        ("(take (biginteger 2) [1 2 3])", "(1 2)"),
        ("(nth [1 2 3] (biginteger 1))", "2"),
        ("(range (biginteger 3))", "(0 1 2)"),
        ("(NaN? (biginteger 5))", "false"),
        ("(infinite? (biginteger 5))", "false"),
        ("(nat-int? (biginteger 5))", "false"),
        ("(neg-int? (biginteger -5))", "false"),
        ("(pos-int? (biginteger 5))", "false"),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(int (biginteger 9223372036854775808))",
            "java.lang.IllegalArgumentException | Value out of range for long: 9223372036854775808",
        ),
        (
            "(long (biginteger 18446744073709551616))",
            "java.lang.IllegalArgumentException | Value out of range for long: 18446744073709551616",
        ),
        (
            "(short (biginteger 100000))",
            "java.lang.IllegalArgumentException | Value out of range for short: 100000",
        ),
        (
            "(byte (biginteger 1000))",
            "java.lang.IllegalArgumentException | Value out of range for byte: 1000",
        ),
        (
            "(char (biginteger 1114112))",
            "java.lang.IllegalArgumentException | Value out of range for char: 1114112",
        ),
        (
            "(float (biginteger 10000000000000000000000000000000000000000))",
            "java.lang.IllegalArgumentException | Value out of range for float: 10000000000000000000000000000000000000000",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: `BigInteger` prints bare while `BigInt` prints
/// `N` (the class-distinction regression pin — no `bigint?`
/// var exists upstream, so print is the observable).
#[test]
fn s2_biginteger_print_regression() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(pr-str (biginteger 5))"), "\"5\"");
    assert_eq!(run_last(&mut eval, "(pr-str (bigint 5))"), "\"5N\"");
    assert_eq!(
        run_last(&mut eval, "(pr-str [(biginteger 5) (bigint 5)])"),
        "\"[5 5N]\""
    );
    assert_eq!(run_last(&mut eval, "(str (numerator 3/4))"), "\"3\"");
}

/// Issue #44: division/context edges (probes `r10-*`) plus the
/// branch-coverage pins (error propagation, `=`/`hash` on
/// contexts, extension garbage bindings, per-mode `toString`,
/// `print` display).
#[test]
fn s2_context_edge_rows() {
    let mut eval = boot();
    for (form, want) in [
        ("(with-precision 3 (/ 12345M 1M))", "1.23E+4M"),
        ("(with-precision 0 (bigdec 1/2))", "0.5M"),
        ("(with-precision 3 (bigdec 1/2))", "0.5M"),
        ("(with-precision 1 (/ 99M 10M))", "1E+1M"),
        ("(with-precision 3 (bigdec 1/128))", "0.00781M"),
        (
            "(with-precision 2 :rounding HALF_EVEN (/ 35001M 200000M))",
            "0.18M",
        ),
        ("(biginteger (biginteger 5))", "5"),
        (
            "(= (hash (with-precision 10 *math-context*)) (hash (with-precision 10 *math-context*)))",
            "true",
        ),
        ("(= (with-precision 10 *math-context*) *out*)", "false"),
        (
            "(with-precision 2 :rounding UP (str *math-context*))",
            "\"precision=2 roundingMode=UP\"",
        ),
        (
            "(with-precision 2 :rounding DOWN (str *math-context*))",
            "\"precision=2 roundingMode=DOWN\"",
        ),
        (
            "(with-precision 2 :rounding CEILING (str *math-context*))",
            "\"precision=2 roundingMode=CEILING\"",
        ),
        (
            "(with-precision 2 :rounding HALF_DOWN (str *math-context*))",
            "\"precision=2 roundingMode=HALF_DOWN\"",
        ),
        (
            "(with-precision 2 :rounding HALF_EVEN (str *math-context*))",
            "\"precision=2 roundingMode=HALF_EVEN\"",
        ),
        (
            "(with-precision 2 :rounding UNNECESSARY (str *math-context*))",
            "\"precision=2 roundingMode=UNNECESSARY\"",
        ),
        (
            "(print-str (with-precision 10 *math-context*))",
            "\"precision=10 roundingMode=HALF_UP\"",
        ),
    ] {
        assert_eq!(run_last(&mut eval, form), want, "{form}");
    }
    for (form, want) in [
        (
            "(with-precision 3 (/ 1M 0M))",
            "java.lang.ArithmeticException | Divide by zero",
        ),
        (
            "(with-precision 2 :rounding UNNECESSARY (- 5.678M 1.234M))",
            "java.lang.ArithmeticException | Rounding necessary",
        ),
        (
            "(with-precision 10 :rounding 5 1)",
            "java.lang.IllegalArgumentException | No enum constant java.math.RoundingMode.5",
        ),
        (
            "(binding [*math-context* *out*] (+ 1M 1M))",
            "java.lang.ClassCastException | class java.io.OutputStreamWriter cannot be cast to class java.math.MathContext (java.io.OutputStreamWriter and java.math.MathContext are in module java.base of loader 'bootstrap')",
        ),
        (
            "(with-precision 10 (numerator *math-context*))",
            "java.lang.ClassCastException | class java.math.MathContext cannot be cast to class clojure.lang.Ratio (java.math.MathContext is in module java.base of loader 'bootstrap'; clojure.lang.Ratio is in unnamed module of loader 'app')",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}

/// Issue #44: arity misses (probes `ar-*`, `r3-noprec`).
#[test]
fn s2_arity_rows() {
    let mut eval = boot();
    for (form, want) in [
        (
            "(abs)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/abs",
        ),
        (
            "(abs 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/abs",
        ),
        (
            "(biginteger)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/biginteger",
        ),
        (
            "(numerator)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/numerator",
        ),
        (
            "(denominator 1/2 1/3)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/denominator",
        ),
        (
            "(decimal?)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/decimal?",
        ),
        (
            "(ratio? 1/2 1/3)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/ratio?",
        ),
        (
            "(rational? 1 2)",
            "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/rational?",
        ),
        (
            "(with-precision)",
            "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-precision",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, form), want, "{form}");
    }
}
