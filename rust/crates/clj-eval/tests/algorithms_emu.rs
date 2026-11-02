// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #205 emulation cover: every new/changed product line the
//! algorithms green run needs, pinned through the public surface.
//! Sections mirror the implementation batches (boxing/Math/System
//! statics first, then instance methods, then the big emulations).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

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

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    pr(&realized(eval, vs.last().unwrap()))
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

/// A compile error's `Class | message` (known-member arity
/// misses fail at compile time through surface syntax).
fn compile_err(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(vs) => panic!("expected a compile error, got values: {vs:?}"),
    }
}

/// The runtime miss of a direct `__static*` call (covers the
/// per-member arity arms that surface syntax never reaches).
fn runtime_miss(eval: &mut Evaluator, class: &str, member: &str, args: &str) -> String {
    uncaught(
        eval,
        &format!(r#"(clojure.core/__static* "{class}" "{member}" {args})"#),
    )
}

#[test]
fn math_rint_radians_degrees() {
    let mut eval = boot();
    // Ties go even (2.5→2, 3.5→4), like the JDK.
    assert_eq!(
        run(
            &mut eval,
            "[(Math/rint 2.5) (Math/rint 3.5) (Math/rint -2.5) (Math/rint 2.4)]"
        ),
        "[2.0 4.0 -2.0 2.0]"
    );
    assert_eq!(run(&mut eval, "(Math/toRadians 180)"), "3.141592653589793");
    assert_eq!(run(&mut eval, "(Math/toDegrees Math/PI)"), "180.0");
    assert_eq!(
        compile_err(&mut eval, "(Math/rint 1 2)"),
        "java.lang.IllegalArgumentException | No matching method rint found taking 2 args for class java.lang.Math"
    );
    assert_eq!(
        compile_err(&mut eval, "(Math/toRadians)"),
        "java.lang.IllegalArgumentException | No matching method toRadians found taking 0 args for class java.lang.Math"
    );
    assert_eq!(
        compile_err(&mut eval, "(Math/toDegrees 1 2)"),
        "java.lang.IllegalArgumentException | No matching method toDegrees found taking 2 args for class java.lang.Math"
    );
}

#[test]
fn math_floor_div_mod() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(Math/floorDiv 7 3) (Math/floorDiv -7 3) (Math/floorDiv 7 -3) (Math/floorDiv -7 -3)]"
        ),
        "[2 -3 -3 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(Math/floorMod 7 3) (Math/floorMod -7 3) (Math/floorMod 7 -3) (Math/floorMod -7 -3)]"
        ),
        "[1 2 -2 -1]"
    );
    // The widening long coercion truncates doubles first.
    assert_eq!(
        run(&mut eval, "[(Math/floorDiv 7.9 3) (Math/floorMod 7.9 3)]"),
        "[2 1]"
    );
    // `MIN/-1` wraps to `MIN` (no throw); the mod is zero.
    assert_eq!(
        run(
            &mut eval,
            "[(Math/floorDiv Long/MIN_VALUE -1) (Math/floorMod Long/MIN_VALUE -1)]"
        ),
        "[-9223372036854775808 0]"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/floorDiv 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/floorMod 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        compile_err(&mut eval, "(Math/floorDiv 1)"),
        "java.lang.IllegalArgumentException | No matching method floorDiv found taking 1 args for class java.lang.Math"
    );
    assert_eq!(
        compile_err(&mut eval, "(Math/floorMod 1)"),
        "java.lang.IllegalArgumentException | No matching method floorMod found taking 1 args for class java.lang.Math"
    );
    assert!(uncaught(&mut eval, "(Math/floorDiv nil 3)").contains("NullPointerException"));
}

#[test]
fn math_random_draws_unit_doubles() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(every? #(and (<= 0.0 %) (< % 1.0)) (repeatedly 50 (fn [] (Math/random))))"
        ),
        "true"
    );
    assert_eq!(
        compile_err(&mut eval, "(Math/random 1)"),
        "java.lang.IllegalArgumentException | No matching method random found taking 1 args for class java.lang.Math"
    );
}

#[test]
fn long_bit_strings_and_counts() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Long/toBinaryString 5) (Long/toBinaryString 0) (Long/toHexString 255)]"#,
        ),
        r#"["101" "0" "ff"]"#
    );
    assert_eq!(run(&mut eval, "(count (Long/toBinaryString -1))"), "64");
    assert_eq!(run(&mut eval, "(count (Long/toHexString -1))"), "16");
    assert_eq!(
        run(
            &mut eval,
            "[(Long/bitCount 7) (Long/bitCount -1) (Long/bitCount 0)]"
        ),
        "[3 64 0]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(Long/numberOfTrailingZeros 8) (Long/numberOfTrailingZeros 0) (Long/numberOfTrailingZeros -1)]"
        ),
        "[3 64 0]"
    );
    assert_eq!(
        compile_err(&mut eval, "(Long/toBinaryString)"),
        "java.lang.IllegalArgumentException | No matching method toBinaryString found taking 0 args for class java.lang.Long"
    );
    assert_eq!(
        compile_err(&mut eval, "(Long/toHexString 1 2)"),
        "java.lang.IllegalArgumentException | No matching method toHexString found taking 2 args for class java.lang.Long"
    );
    assert_eq!(
        compile_err(&mut eval, "(Long/bitCount)"),
        "java.lang.IllegalArgumentException | No matching method bitCount found taking 0 args for class java.lang.Long"
    );
    assert_eq!(
        compile_err(&mut eval, "(Long/numberOfTrailingZeros 1 2)"),
        "java.lang.IllegalArgumentException | No matching method numberOfTrailingZeros found taking 2 args for class java.lang.Long"
    );
}

#[test]
fn integer_to_binary_string_truncates_to_32_bits() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, r#"(Integer/toBinaryString 5)"#), r#""101""#);
    // Negatives print 32 digits (the `(int)` overload).
    assert_eq!(run(&mut eval, "(count (Integer/toBinaryString -1))"), "32");
    // Wide longs truncate before formatting.
    assert_eq!(
        run(&mut eval, r#"(Integer/toBinaryString 4294967296)"#),
        r#""0""#
    );
    assert_eq!(
        compile_err(&mut eval, "(Integer/toBinaryString 1 2)"),
        "java.lang.IllegalArgumentException | No matching method toBinaryString found taking 2 args for class java.lang.Integer"
    );
}

#[test]
fn double_finiteness_checks() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(Double/isNaN ##NaN) (Double/isNaN 1.0) (Double/isNaN 1)]"
        ),
        "[true false false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(Double/isInfinite ##Inf) (Double/isInfinite ##-Inf) (Double/isInfinite 1.0)]"
        ),
        "[true true false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(Double/isFinite 1.5) (Double/isFinite ##NaN) (Double/isFinite ##Inf) (Double/isFinite 3)]"
        ),
        "[true false false true]"
    );
    assert_eq!(
        compile_err(&mut eval, "(Double/isNaN)"),
        "java.lang.IllegalArgumentException | No matching method isNaN found taking 0 args for class java.lang.Double"
    );
    assert_eq!(
        compile_err(&mut eval, "(Double/isInfinite 1 2)"),
        "java.lang.IllegalArgumentException | No matching method isInfinite found taking 2 args for class java.lang.Double"
    );
    assert_eq!(
        compile_err(&mut eval, "(Double/isFinite)"),
        "java.lang.IllegalArgumentException | No matching method isFinite found taking 0 args for class java.lang.Double"
    );
}

#[test]
fn boolean_value_of() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Boolean/valueOf true) (Boolean/valueOf false) (Boolean/valueOf "true") (Boolean/valueOf "TrUe") (Boolean/valueOf "yes") (Boolean/valueOf "") (Boolean/valueOf nil)]"#
        ),
        "[true false true true false false false]"
    );
    assert_eq!(
        uncaught(&mut eval, "(Boolean/valueOf 1)"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 1 args for class java.lang.Boolean"
    );
    assert_eq!(
        compile_err(&mut eval, "(Boolean/valueOf true false)"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 2 args for class java.lang.Boolean"
    );
}

#[test]
fn float_bit_conversions() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Float/floatToIntBits 1.0)"), "1065353216");
    assert_eq!(run(&mut eval, "(Float/floatToIntBits ##NaN)"), "2143289344");
    assert_eq!(run(&mut eval, "(Float/intBitsToFloat 1065353216)"), "1.0");
    assert_eq!(
        run(
            &mut eval,
            "(Float/intBitsToFloat (Float/floatToIntBits 0.5))"
        ),
        "0.5"
    );
    assert_eq!(
        compile_err(&mut eval, "(Float/floatToIntBits)"),
        "java.lang.IllegalArgumentException | No matching method floatToIntBits found taking 0 args for class java.lang.Float"
    );
    assert_eq!(
        compile_err(&mut eval, "(Float/intBitsToFloat 1 2)"),
        "java.lang.IllegalArgumentException | No matching method intBitsToFloat found taking 2 args for class java.lang.Float"
    );
}

#[test]
fn character_classification() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Character/isDigit \5) (Character/isDigit \a) (Character/isDigit 53) (Character/isDigit -1) (Character/isDigit 1114112)]"#
        ),
        "[true false true false false]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"[(Character/isLetter \a) (Character/isLetter \5) (Character/isLetter 65) (Character/isLetter -1)]"#
        ),
        "[true false true false]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"[(Character/isAlphabetic \a) (Character/isAlphabetic \5) (Character/isAlphabetic 8364)]"#
        ),
        "[true false false]"
    );
    // Documented narrowings (pinned, not fixed): `isLetter` counts
    // letter-numbers (Roman numeral one), `isDigit` counts
    // number-others (vulgar fraction one half).
    assert_eq!(run(&mut eval, "(Character/isLetter \\u2160)"), "true");
    assert_eq!(run(&mut eval, "(Character/isDigit \\u00BD)"), "true");
    assert_eq!(
        uncaught(&mut eval, r#"(Character/isDigit "5")"#),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        compile_err(&mut eval, "(Character/isLetter 1 2)"),
        "java.lang.IllegalArgumentException | No matching method isLetter found taking 2 args for class java.lang.Character"
    );
    assert_eq!(
        compile_err(&mut eval, "(Character/isAlphabetic)"),
        "java.lang.IllegalArgumentException | No matching method isAlphabetic found taking 0 args for class java.lang.Character"
    );
}

#[test]
fn character_case_mapping() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Character/toUpperCase \a) (Character/toUpperCase 97) (Character/toLowerCase \A) (Character/toLowerCase 65)]"#
        ),
        r#"[\A 65 \a 97]"#
    );
    // Out-of-range code points pass through, like the JDK.
    assert_eq!(
        run(
            &mut eval,
            "[(Character/toUpperCase -1) (Character/toLowerCase 1114112)]"
        ),
        "[-1 1114112]"
    );
    // Sigma lowercases simply; sharp-s takes the first scalar of
    // the full mapping (pinned narrowing: the JDK keeps `ß`).
    assert_eq!(run(&mut eval, r#"(Character/toLowerCase \u03A3)"#), "\\σ");
    assert_eq!(run(&mut eval, r#"(Character/toUpperCase \u00DF)"#), "\\S");
    assert!(uncaught(&mut eval, r#"(Character/toUpperCase "a")"#).contains("ClassCastException"));
    assert_eq!(
        compile_err(&mut eval, "(Character/toLowerCase 1 2)"),
        "java.lang.IllegalArgumentException | No matching method toLowerCase found taking 2 args for class java.lang.Character"
    );
}

#[test]
fn system_clocks_are_positive_and_nondecreasing() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [a (System/nanoTime) b (System/nanoTime)] [(pos? a) (<= a b)])"
        ),
        "[true true]"
    );
    assert_eq!(run(&mut eval, "(pos? (System/currentTimeMillis))"), "true");
    assert_eq!(
        compile_err(&mut eval, "(System/nanoTime 1)"),
        "java.lang.IllegalArgumentException | No matching method nanoTime found taking 1 args for class java.lang.System"
    );
    assert_eq!(
        compile_err(&mut eval, "(System/currentTimeMillis 1)"),
        "java.lang.IllegalArgumentException | No matching method currentTimeMillis found taking 1 args for class java.lang.System"
    );
}

#[test]
fn system_getenv_reads_the_process_environment() {
    // A unique name (parallel tests share the process environment).
    unsafe { std::env::set_var("CLJ_ALGO_EMU_PROBE_205", "probe-value") };
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, r#"(System/getenv "CLJ_ALGO_EMU_PROBE_205")"#),
        r#""probe-value""#
    );
    assert_eq!(
        run(&mut eval, r#"(System/getenv "CLJ_ALGO_EMU_ABSENT_205")"#),
        "nil"
    );
    unsafe { std::env::remove_var("CLJ_ALGO_EMU_PROBE_205") };
    assert!(uncaught(&mut eval, "(System/getenv nil)").contains("NullPointerException"));
    assert!(uncaught(&mut eval, "(System/getenv 1)").contains("ClassCastException"));
    assert_eq!(
        compile_err(&mut eval, r#"(System/getenv "A" "B")"#),
        "java.lang.IllegalArgumentException | No matching method getenv found taking 2 args for class java.lang.System"
    );
}

#[test]
fn system_arraycopy_moves_slots() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [s (byte-array [1 2 3 4]) d (byte-array 4)] (System/arraycopy s 1 d 0 2) (vec d))"
        ),
        "[2 3 0 0]"
    );
    // Overlapping same-array copies move correctly (memmove).
    assert_eq!(
        run(
            &mut eval,
            "(let [a (int-array [1 2 3 4])] (System/arraycopy a 0 a 1 3) (vec a))"
        ),
        "[1 1 2 3]"
    );
    // Reference arrays copy elementwise; the call answers nil.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [s (to-array ["a" "b"]) d (to-array [1 2])] [(System/arraycopy s 0 d 1 1) (vec d)])"#
        ),
        r#"[nil [1 "a"]]"#
    );
    // Zero-length copies are no-ops, even at the edge.
    assert_eq!(
        run(
            &mut eval,
            "(let [a (byte-array [9])] (System/arraycopy a 1 a 1 0) (vec a))"
        ),
        "[9]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [s (byte-array [1]) d (int-array 1)] (System/arraycopy s 0 d 0 1))"
        ),
        "java.lang.ArrayStoreException | arraycopy: array type mismatch"
    );
    assert!(
        uncaught(
            &mut eval,
            "(let [a (byte-array [1])] (System/arraycopy a 0 a 0 2))"
        )
        .contains("IndexOutOfBoundsException")
    );
    assert!(
        uncaught(
            &mut eval,
            "(let [a (byte-array [1])] (System/arraycopy a 0 a 1 1))"
        )
        .contains("IndexOutOfBoundsException")
    );
    assert!(
        uncaught(
            &mut eval,
            "(let [a (byte-array [1])] (System/arraycopy a -1 a 0 1))"
        )
        .contains("IndexOutOfBoundsException")
    );
    assert_eq!(
        uncaught(&mut eval, "(System/arraycopy nil 0 nil 0 0)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, r#"(System/arraycopy "s" 0 "d" 0 0)"#),
        "java.lang.ArrayStoreException | arraycopy"
    );
    assert_eq!(
        compile_err(&mut eval, "(System/arraycopy 1 2 3 4)"),
        "java.lang.IllegalArgumentException | No matching method arraycopy found taking 4 args for class java.lang.System"
    );
}

#[test]
fn parse_long_radix() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Long/parseLong "101" 2) (Long/parseLong "ff" 16) (Long/parseLong "-101" 2) (Long/parseLong "+7" 8) (Long/parseLong "z" 36) (Long/parseLong "Z" 36)]"#
        ),
        "[5 255 -5 7 35 35]"
    );
    // Radix doubles truncate through the int coercion.
    assert_eq!(run(&mut eval, r#"[(Long/parseLong "11" 2.0)]"#), "[3]");
    // Full-range extremes round-trip.
    assert_eq!(
        run(
            &mut eval,
            r#"[(Long/parseLong "-9223372036854775808" 10) (Long/parseLong "9223372036854775807" 10)]"#
        ),
        "[-9223372036854775808 9223372036854775807]"
    );
    for bad in [
        r#"(Long/parseLong "9223372036854775808" 10)"#,
        r#"(Long/parseLong "-9223372036854775809" 10)"#,
        r#"(Long/parseLong "" 10)"#,
        r#"(Long/parseLong "-" 10)"#,
        r#"(Long/parseLong "+" 10)"#,
        r#"(Long/parseLong "12a" 10)"#,
        r#"(Long/parseLong " 12" 10)"#,
        r#"(Long/parseLong "7.9" 10)"#,
    ] {
        assert!(
            uncaught(&mut eval, bad).contains("NumberFormatException"),
            "{bad}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, r#"(Long/parseLong "10" 1)"#),
        "java.lang.NumberFormatException | radix 1 less than Character.MIN_RADIX"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(Long/parseLong "10" 37)"#),
        "java.lang.NumberFormatException | radix 37 greater than Character.MAX_RADIX"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(Long/parseLong nil 10)"#),
        "java.lang.NumberFormatException | For input string: \"null\""
    );
    assert!(uncaught(&mut eval, r#"(Long/parseLong 5 10)"#).contains("ClassCastException"));
    assert!(
        uncaught(&mut eval, r#"(Long/parseLong "10" "x")"#).contains("IllegalArgumentException")
    );
    assert_eq!(
        compile_err(&mut eval, r#"(Long/parseLong "1" 10 3)"#),
        "java.lang.IllegalArgumentException | No matching method parseLong found taking 3 args for class java.lang.Long"
    );
}

#[test]
fn parse_int_radix_narrows_to_32_bits() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(Integer/parseInt "ff" 16) (Integer/parseInt "-80000000" 16) (Integer/parseInt "+42" 10)]"#
        ),
        "[255 -2147483648 42]"
    );
    assert!(
        uncaught(&mut eval, r#"(Integer/parseInt "80000000" 16)"#)
            .contains("NumberFormatException")
    );
    assert_eq!(
        uncaught(&mut eval, r#"(Integer/parseInt "10" 1)"#),
        "java.lang.NumberFormatException | radix 1 less than Character.MIN_RADIX"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(Integer/parseInt nil 10)"#),
        "java.lang.NumberFormatException | For input string: \"null\""
    );
    assert!(uncaught(&mut eval, r#"(Integer/parseInt 5 10)"#).contains("ClassCastException"));
    assert_eq!(
        compile_err(&mut eval, r#"(Integer/parseInt "1" 10 3)"#),
        "java.lang.IllegalArgumentException | No matching method parseInt found taking 3 args for class java.lang.Integer"
    );
}

/// Every new static member's runtime arity arm (surface
/// syntax rejects these at compile time, so the direct
/// `__static*` spelling covers them instead).
#[test]
fn static_runtime_arity_arms() {
    let rows: &[(&str, &str, &str, usize)] = &[
        ("java.lang.Math", "rint", "1 2", 2),
        ("java.lang.Math", "toRadians", "", 0),
        ("java.lang.Math", "toDegrees", "1 2", 2),
        ("java.lang.Math", "floorDiv", "1", 1),
        ("java.lang.Math", "floorMod", "1 2 3", 3),
        ("java.lang.Math", "random", "1", 1),
        ("java.lang.Long", "toBinaryString", "", 0),
        ("java.lang.Long", "toHexString", "1 2", 2),
        ("java.lang.Long", "bitCount", "", 0),
        ("java.lang.Long", "numberOfTrailingZeros", "1 2", 2),
        ("java.lang.Long", "parseLong", "1 2 3", 3),
        ("java.lang.Integer", "toBinaryString", "1 2", 2),
        ("java.lang.Integer", "parseInt", "", 0),
        ("java.lang.Double", "isNaN", "", 0),
        ("java.lang.Double", "isInfinite", "1 2", 2),
        ("java.lang.Double", "isFinite", "", 0),
        ("java.lang.Boolean", "valueOf", "", 0),
        ("java.lang.Float", "floatToIntBits", "1 2", 2),
        ("java.lang.Float", "intBitsToFloat", "", 0),
        ("java.lang.Character", "isDigit", "1 2", 2),
        ("java.lang.Character", "isLetter", "", 0),
        ("java.lang.Character", "isAlphabetic", "1 2", 2),
        ("java.lang.Character", "toUpperCase", "", 0),
        ("java.lang.Character", "toLowerCase", "1 2", 2),
        ("java.lang.System", "nanoTime", "1", 1),
        ("java.lang.System", "currentTimeMillis", "1", 1),
        ("java.lang.System", "getenv", "", 0),
        ("java.lang.System", "arraycopy", "1 2", 2),
    ];
    let mut eval = boot();
    for (class, member, args, argc) in rows {
        let want = format!(
            "java.lang.IllegalArgumentException | No matching method {member} found taking {argc} args for class {class}"
        );
        assert_eq!(
            runtime_miss(&mut eval, class, member, args),
            want,
            "{class}/{member}"
        );
    }
    // Unknown members on the new classes miss the same way.
    assert_eq!(
        runtime_miss(&mut eval, "java.lang.Boolean", "nope", ""),
        "java.lang.IllegalArgumentException | No matching method nope found taking 0 args for class java.lang.Boolean"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.lang.Float", "nope", "1"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 1 args for class java.lang.Float"
    );
}

/// The `->` factory survives a field named `t` (#205
/// `Matrix [t n]`): the captured descriptor binding is a
/// macroexpansion gensym, never shadowed by params.
#[test]
fn record_factory_with_field_named_t() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord TRec [t n]) [(:t (->TRec 1 2)) (:n (->TRec 1 2)) (:t (TRec. 3 4)) (:t (map->TRec {:t 5 :n 6}))]"
        ),
        "[1 2 3 5]"
    );
}

/// `instance?` resolves records in dashed namespaces (#205
/// `Dual`): the munged binary name retries demunged. An
/// unmapped dashed type still misses like `user.IQGone`.
#[test]
fn instance_resolves_dashed_namespaces() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'my-dash-ns) (clojure.core/refer-clojure) (defrecord R [x]) [(instance? R (R. 1)) (instance? R {:x 1})])"
        ),
        "[true false]"
    );
    let mut eval = boot();
    assert!(
        uncaught(
            &mut eval,
            "(do (in-ns 'gone-ns) (clojure.core/refer-clojure) (deftype G [x]) (def g G) (ns-unmap *ns* 'G) (instance? g 1))"
        )
        .contains("ClassCastException")
    );
}

/// Class/descriptor-keyed `print-method`/`print-dup` entries win
/// over the token table (#205 `Dual`/`Polynomial` records).
#[test]
fn print_method_class_keys_win_over_tokens() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord PMR [x]) (defmethod print-method PMR [o w] (print-simple (str \"PMR!\" (:x o)) w)) (with-out-str (print (PMR. 7)))"
        ),
        "\"PMR!7\""
    );
    // A plain class key overrides scalar printing exactly.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defmethod print-method String [s w] (print-simple \"STR!\" w)) [(with-out-str (print \"hi\")) (with-out-str (print 5))]"
        ),
        "[\"STR!\" \"5\"]"
    );
    // A type object dispatches on `Class`, never on an instance entry.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord PDC [x]) (defmethod print-method Class [c w] (print-simple \"CLS!\" w)) [(with-out-str (print PDC)) (with-out-str (print (PDC. 1)))]"
        ),
        "[\"CLS!\" \"#user.PDC{:x 1}\"]"
    );
    // `print-dup` honors descriptor keys the same way.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(defrecord PDR [x]) (defmethod print-dup PDR [o w] (print-simple \"DUP!\" w)) (binding [*print-dup* true] (with-out-str (pr (PDR. 1))))"
        ),
        "\"DUP!\""
    );
}

/// `indexOf` from an index (negative reads 0, past-the-end
/// reads the length for empty needles, -1 otherwise).
#[test]
fn string_index_of_from() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(.indexOf "abcabc" "bc" 2) (.indexOf "abcabc" "bc" 3) (.indexOf "abcabc" "bc" -5) (.indexOf "abcabc" "" 2) (.indexOf "abcabc" "" 99) (.indexOf "abcabc" "z" 99)]"#
        ),
        "[4 4 1 2 6 -1]"
    );
    // Code-point needles and unrepresentable ones.
    assert_eq!(
        run(
            &mut eval,
            r#"[(.indexOf "abc" 98 1) (.indexOf "abc" \c 1) (.indexOf "abc" 1114112 0) (.indexOf "abc" -1 0)]"#
        ),
        "[1 2 -1 -1]"
    );
    // The 1-arg shape still delegates (empty reads 0).
    assert_eq!(
        run(&mut eval, r#"[(.indexOf "abc" "") (.indexOf "abc" "z")]"#),
        "[0 -1]"
    );
    assert!(uncaught(&mut eval, r#"(.indexOf "abc" "b" nil)"#).contains("NullPointerException"));
    assert!(
        uncaught(&mut eval, r#"(.indexOf "abc" "b" "x")"#).contains("IllegalArgumentException")
    );
    // A mistyped needle is the overload miss (before any index check).
    assert_eq!(
        uncaught(&mut eval, r#"(.indexOf "abc" :k 1)"#),
        "java.lang.IllegalArgumentException | No matching method indexOf found taking 2 args for class java.lang.String"
    );
}

/// `contains` is a literal substring test.
#[test]
fn string_contains() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(.contains "hello" "ell") (.contains "hello" "z") (.contains "hello" "")]"#
        ),
        "[true false true]"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.contains "hello" nil)"#),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.contains "hello" 5)"#),
        "java.lang.IllegalArgumentException | No matching method contains found taking 1 args for class java.lang.String"
    );
}

/// `replace` swaps chars or literal sequences (never regex).
#[test]
fn string_replace() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(.replace "hello" \l \L) (.replace "hello" "ll" "r") (.replace "aaa" "aa" "b")]"#
        ),
        r#"["heLLo" "hero" "ba"]"#
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.replace "hello" nil "x")"#),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"target\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.replace "hello" "h" nil)"#),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.CharSequence.toString()\" because \"replacement\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.replace "hello" \h "x")"#),
        "java.lang.IllegalArgumentException | No matching method replace found taking 2 args for class java.lang.String"
    );
}

/// `getBytes` encodes UTF-8 by default, or the named charset.
#[test]
fn string_get_bytes() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, r#"(vec (.getBytes "AB"))"#), "[65 66]");
    assert_eq!(run(&mut eval, r#"(vec (.getBytes "é"))"#), "[-61 -87]");
    assert_eq!(
        run(&mut eval, r#"(vec (.getBytes "AB" "UTF-8"))"#),
        "[65 66]"
    );
    // Bare UTF-16 takes the big-endian BOM, like the JDK.
    assert_eq!(
        run(&mut eval, r#"(vec (.getBytes "A" "UTF-16"))"#),
        "[-2 -1 0 65]"
    );
    // Unmappable ASCII chars become `?`.
    assert_eq!(
        run(&mut eval, r#"(vec (.getBytes "é" "US-ASCII"))"#),
        "[63]"
    );
    assert!(
        uncaught(&mut eval, r#"(.getBytes "A" "NOPE-9")"#).contains("UnsupportedEncodingException")
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.getBytes "A" nil)"#),
        "java.lang.IllegalArgumentException | More than one matching method found: getBytes"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.getBytes "A" 5)"#),
        "java.lang.IllegalArgumentException | No matching method getBytes found taking 1 args for class java.lang.String"
    );
}

/// `codePointCount` counts scalars over a UTF-16 window.
#[test]
fn string_code_point_count() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, r#"(.codePointCount "hello" 0 5)"#), "5");
    // An astral pair (2 units) counts once (the reader
    // rejects lone-surrogate escapes, so spell it literally).
    assert_eq!(run(&mut eval, "(.codePointCount \"a😀b\" 0 4)"), "3");
    assert_eq!(run(&mut eval, "(.codePointCount \"a😀b\" 1 3)"), "1");
    assert_eq!(
        uncaught(&mut eval, r#"(.codePointCount "hello" -1 5)"#),
        "java.lang.StringIndexOutOfBoundsException | Range [-1, 5) out of bounds for length 5"
    );
    assert!(
        uncaught(&mut eval, r#"(.codePointCount "hello" 0 9)"#)
            .contains("StringIndexOutOfBoundsException")
    );
    assert!(
        uncaught(&mut eval, r#"(.codePointCount "hello" 3 2)"#)
            .contains("StringIndexOutOfBoundsException")
    );
    // Either half mistyped is the int-coercion error.
    assert!(
        uncaught(&mut eval, r#"(.codePointCount "hello" "x" 5)"#)
            .contains("IllegalArgumentException")
    );
    assert!(
        uncaught(&mut eval, r#"(.codePointCount "hello" 0 nil)"#).contains("NullPointerException")
    );
}

/// `compareTo` orders by UTF-16 units, then length.
#[test]
fn string_compare_to() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(.compareTo "abc" "abd") (.compareTo "abd" "abc") (.compareTo "abc" "abc") (.compareTo "abc" "abcd") (.compareTo "abcd" "abc")]"#
        ),
        "[-1 1 0 -1 1]"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.compareTo "a" nil)"#),
        "java.lang.NullPointerException | Cannot invoke \"String.compareTo(String)\" because \"anotherString\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.compareTo "a" 5)"#),
        "java.lang.IllegalArgumentException | No matching method compareTo found taking 1 args for class java.lang.String"
    );
}

/// `indexOf` scans subvectors like lists.
#[test]
fn coll_index_of_subvector() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(.indexOf (subvec [10 20 30] 1) 30)"), "1");
    assert_eq!(run(&mut eval, "(.indexOf (subvec [10 20 30] 1) 10)"), "-1");
    assert_eq!(run(&mut eval, "(.indexOf (subvec [10 20 30] 1 2) 20)"), "0");
}

/// Throwable constructors build real throwables (messages,
/// causes, and catchability — not the miss error).
#[test]
fn throwable_ctors() {
    let mut eval = boot();
    // Every suite-built class, one-arg.
    assert_eq!(
        run(
            &mut eval,
            r#"[(ex-message (IllegalArgumentException. "ia")) (ex-message (ArithmeticException. "ar")) (ex-message (Exception. "ex")) (ex-message (IndexOutOfBoundsException. "io")) (ex-message (AssertionError. "as")) (ex-message (UnsupportedOperationException. "uo"))]"#
        ),
        "[\"ia\" \"ar\" \"ex\" \"io\" \"as\" \"uo\"]"
    );
    // Bare construction has a nil message; the class matches.
    assert_eq!(
        run(
            &mut eval,
            "(let [e (IllegalArgumentException.)] [(nil? (ex-message e)) (str (class e))])"
        ),
        "[true \"class java.lang.IllegalArgumentException\"]"
    );
    // Throw/catch round-trips with the message intact.
    assert_eq!(
        run(
            &mut eval,
            r#"(try (throw (IllegalArgumentException. "boom")) (catch IllegalArgumentException e (ex-message e)))"#
        ),
        "\"boom\""
    );
    // A cause-only arg chains (message is the cause's `toString`).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [c (ArithmeticException. "root") e (IllegalArgumentException. c)] [(ex-message e) (ex-message (ex-cause e))])"#
        ),
        "[\"java.lang.ArithmeticException: root\" \"root\"]"
    );
    // Message plus cause (nil cause reads absent).
    assert_eq!(
        run(
            &mut eval,
            r#"[(ex-message (IllegalArgumentException. "m" (Exception. "c"))) (ex-message (ex-cause (IllegalArgumentException. "m" (Exception. "c")))) (nil? (ex-cause (IllegalArgumentException. "m" nil)))]"#
        ),
        "[\"m\" \"c\" true]"
    );
    // A nil single arg is ambiguous; mistyped shapes miss.
    assert_eq!(
        uncaught(&mut eval, "(IllegalArgumentException. nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: java.lang.IllegalArgumentException"
    );
    assert_eq!(
        uncaught(&mut eval, "(IllegalArgumentException. 5)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(IllegalArgumentException. "a" "b")"#),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(IllegalArgumentException. "a" nil nil)"#),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.IllegalArgumentException"
    );
    // Other throwables stay misses (no silent widening).
    assert_eq!(
        uncaught(&mut eval, r#"(NullPointerException. "x")"#),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.lang.NullPointerException"
    );
}

/// A 0-arg `Class/field` call reads the field (methods win;
/// fields with args still miss).
#[test]
fn static_field_calls() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(clojure.lang.PersistentQueue/EMPTY)"), "[]");
    assert_eq!(run(&mut eval, "(Math/PI)"), "3.141592653589793");
    assert_eq!(run(&mut eval, "(Long/MAX_VALUE)"), "9223372036854775807");
    // Methods still win over the field path.
    assert_eq!(run(&mut eval, "(Math/abs -3)"), "3");
    // Fields with args, and unknown members, still miss.
    assert_eq!(
        uncaught(&mut eval, "(Math/PI 1)"),
        "java.lang.IllegalArgumentException | No matching method PI found taking 1 args for class java.lang.Math"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/NOPE)"),
        "java.lang.IllegalArgumentException | No matching method NOPE found taking 0 args for class java.lang.Math"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/NOPE)"),
        "java.lang.IllegalArgumentException | No matching method NOPE found taking 0 args for class java.lang.Long"
    );
}

/// Seeded `Random` streams are JVM-identical (vectors
/// cross-checked against an independent transcription;
/// `-1170105035` is the well-known first `Random(42)` draw).
#[test]
fn random_seeded_streams() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42)] [(.nextInt r) (.nextInt r) (.nextInt r)])"
        ),
        "[-1170105035 234785527 -1360544799]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42)] (vec (repeatedly 5 #(.nextInt r 100))))"
        ),
        "[30 63 48 84 70]"
    );
    // The power-of-two fast path agrees too.
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42)] (vec (repeatedly 5 #(.nextInt r 8))))"
        ),
        "[5 0 5 0 2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42)] [(.nextDouble r) (.nextDouble r) (.nextDouble r)])"
        ),
        "[0.7275636800328681 0.6832234717598454 0.30871945533265976]"
    );
    // Gaussians arrive in cached pairs (last-ulp platform
    // narrowing vs `StrictMath` — the first two agree here).
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42)] [(.nextGaussian r) (.nextGaussian r)])"
        ),
        "[1.141905315473055 0.919407948982788]"
    );
    // Same seed, same stream, across instances.
    assert_eq!(
        run(
            &mut eval,
            "(let [a (java.util.Random. 7) b (java.util.Random. 7)] (= (repeatedly 10 #(.nextInt a)) (repeatedly 10 #(.nextInt b))))"
        ),
        "true"
    );
    // Bytes fill little-endian, four per draw.
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42) b (byte-array 10)] (.nextBytes r b) (vec b))"
        ),
        "[53 -99 65 -70 -9 -118 -2 13 -31 -69]"
    );
    // `nextBytes` answers nil; non-multiples of four fill partially.
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42) b (byte-array 3)] [(.nextBytes r b) (vec b)])"
        ),
        "[nil [53 -99 65]]"
    );
}

/// `Random` shapes: class, printing, identity, and errors.
#[test]
fn random_shapes_and_errors() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str (class (java.util.Random. 1)))"),
        "\"class java.util.Random\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 1)] [(= r r) (= r (java.util.Random. 1))])"
        ),
        "[true false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(re-find #\"java\\.util\\.Random@\" (str (java.util.Random. 1)))"
        ),
        "\"java.util.Random@\""
    );
    // Unseeded construction answers a working generator.
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random.)] (and (integer? (.nextInt r)) (<= 0 (.nextInt r 10) 9)))"
        ),
        "true"
    );
    // Non-positive bounds reject, like the JDK.
    assert_eq!(
        uncaught(&mut eval, "(let [r (java.util.Random. 1)] (.nextInt r 0))"),
        "java.lang.IllegalArgumentException | bound must be positive"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [r (java.util.Random. 1)] (.nextInt r -3))"),
        "java.lang.IllegalArgumentException | bound must be positive"
    );
    // Nil bound unboxes to NPE; mistyped bounds miss.
    assert!(
        uncaught(
            &mut eval,
            "(let [r (java.util.Random. 1)] (.nextInt r nil))"
        )
        .contains("NullPointerException")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (java.util.Random. 1)] (.nextInt r \"x\"))"
        ),
        "java.lang.IllegalArgumentException | No matching method nextInt found taking 1 args for class java.util.Random"
    );
    // Nil bytes NPE; non-bytes miss.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (java.util.Random. 1)] (.nextBytes r nil))"
        ),
        "java.lang.NullPointerException | Cannot read the array length because \"bytes\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (java.util.Random. 1)] (.nextBytes r (int-array 2)))"
        ),
        "java.lang.IllegalArgumentException | No matching method nextBytes found taking 1 args for class java.util.Random"
    );
    // Unknown members miss with the class.
    assert_eq!(
        uncaught(&mut eval, "(let [r (java.util.Random. 1)] (.nextLong r))"),
        "java.lang.IllegalArgumentException | No matching method nextLong found taking 0 args for class java.util.Random"
    );
    // Ctor shapes: bad seeds miss, nil NPEs, extra args miss.
    assert_eq!(
        uncaught(&mut eval, "(java.util.Random. \"x\")"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.util.Random"
    );
    assert!(uncaught(&mut eval, "(java.util.Random. nil)").contains("NullPointerException"));
    assert_eq!(
        compile_err(&mut eval, "(java.util.Random. 1 2)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.util.Random"
    );
}

/// `SecureRandom` draws uniform values (ranges pinned, not
/// streams — OS-chaired).
#[test]
fn secure_random_draws() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str (class (java.security.SecureRandom.)))"),
        "\"class java.security.SecureRandom\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.security.SecureRandom.)] (every? #(<= 0 % 9) (repeatedly 50 #(.nextInt r 10))))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.security.SecureRandom.) b (byte-array 32)] (.nextBytes r b) (count b))"
        ),
        "32"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.security.SecureRandom.)] [(= r r) (= r (java.security.SecureRandom.))])"
        ),
        "[true false]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (java.security.SecureRandom.)] (.nextInt r 0))"
        ),
        "java.lang.IllegalArgumentException | bound must be positive"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.security.SecureRandom. 1)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.security.SecureRandom"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [r (java.security.SecureRandom.)] (.nextDouble r))"
        ),
        "java.lang.IllegalArgumentException | No matching method nextDouble found taking 0 args for class java.security.SecureRandom"
    );
}

/// `BigInteger` constants and `valueOf`.
#[test]
fn bigint_consts_and_value_of() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[BigInteger/ZERO BigInteger/ONE BigInteger/TWO BigInteger/TEN (BigInteger/valueOf 42) (BigInteger/valueOf -7)]"
        ),
        "[0 1 2 10 42 -7]"
    );
    assert_eq!(
        run(&mut eval, "(str (class BigInteger/ONE))"),
        "\"class java.math.BigInteger\""
    );
    assert_eq!(
        compile_err(&mut eval, "(BigInteger/valueOf)"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 0 args for class java.math.BigInteger"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.math.BigInteger", "valueOf", ""),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 0 args for class java.math.BigInteger"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.math.BigInteger", "nope", "1"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 1 args for class java.math.BigInteger"
    );
}

/// `BigInteger` constructors: strings, signum-magnitude,
/// random bits, and every miss shape.
#[test]
fn bigint_ctors() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(BigInteger. "123") (BigInteger. "-123") (BigInteger. "+123") (BigInteger. "ff" 16) (BigInteger. "101" 2) (BigInteger. "-ff" 16)]"#
        ),
        "[123 -123 123 255 5 -255]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [b (byte-array [1 0])] [(BigInteger. 1 b) (BigInteger. -1 b) (BigInteger. 0 (byte-array [0 0]))])"
        ),
        "[256 -256 0]"
    );
    // Random bits: range and determinism shape (seeded draws
    // stay under the bit length).
    assert_eq!(
        run(
            &mut eval,
            "(let [r (java.util.Random. 42) x (BigInteger. 64 r)] [(< x (.shiftLeft BigInteger/ONE 64)) (>= x BigInteger/ZERO) (= x (BigInteger. 64 (java.util.Random. 42)))])"
        ),
        "[true true true]"
    );
    // Zero bits is zero.
    assert_eq!(run(&mut eval, "(BigInteger. 0 (java.util.Random. 1))"), "0");
    // Exact draws (LCG + JDK nextBytes chunking, transcribed).
    assert_eq!(
        run(&mut eval, "(BigInteger. 64 (java.util.Random. 42))"),
        "3863316326626557453"
    );
    // Failures: bad strings, bad radixes, bad signums.
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "xyz")"#),
        "java.lang.NumberFormatException | For input string: \"xyz\""
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "")"#),
        "java.lang.NumberFormatException | Zero length BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "10" 1)"#),
        "java.lang.NumberFormatException | Radix out of range"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "zz" 35)"#),
        "java.lang.NumberFormatException | For input string: \"zz\""
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "-")"#),
        "java.lang.NumberFormatException | Zero length BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. 2 (byte-array [1]))"),
        "java.lang.NumberFormatException | Invalid signum value"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. 0 (byte-array [1]))"),
        "java.lang.NumberFormatException | signum-magnitude mismatch"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. -4 (java.util.Random. 1))"),
        "java.lang.NegativeArraySizeException | numBits must be non-negative"
    );
    // Nil and mistyped shapes.
    assert!(uncaught(&mut eval, "(BigInteger. nil)").contains("NullPointerException"));
    assert!(uncaught(&mut eval, r#"(BigInteger. "10" nil)"#).contains("NullPointerException"));
    assert!(uncaught(&mut eval, "(BigInteger. nil 10)").contains("NullPointerException"));
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. 5)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. "10" "x")"#),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. 1 (int-array [1]))"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(BigInteger. 1 "x")"#),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigInteger. 1 nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: java.math.BigInteger"
    );
    assert_eq!(
        compile_err(&mut eval, "(BigInteger. 1 2 3)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger"
    );
}

/// Direct `__new*` arity misses (surface syntax rejects these
/// at compile time).
#[test]
fn ctor_runtime_arity_arms() {
    let rows: &[(&str, &str, &str)] = &[
        (
            "java.util.Random",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.util.Random",
        ),
        (
            "java.security.SecureRandom",
            "1",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.security.SecureRandom",
        ),
        (
            "java.math.BigInteger",
            "",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger",
        ),
        (
            "java.math.BigInteger",
            "1 2 3",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigInteger",
        ),
        (
            "java.io.File",
            "",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.io.File",
        ),
        (
            "java.io.File",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.io.File",
        ),
        (
            "java.io.FileInputStream",
            "",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.io.FileInputStream",
        ),
        (
            "java.io.FileOutputStream",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.io.FileOutputStream",
        ),
        (
            "java.io.StringWriter",
            "1",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.io.StringWriter",
        ),
        (
            "java.util.ArrayDeque",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.util.ArrayDeque",
        ),
        (
            "java.util.ArrayList",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.util.ArrayList",
        ),
        (
            "java.util.PriorityQueue",
            "",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.util.PriorityQueue",
        ),
        (
            "java.util.PriorityQueue",
            "1 2",
            "java.lang.IllegalArgumentException | No matching ctor found for class java.util.PriorityQueue",
        ),
    ];
    let mut eval = boot();
    for (class, args, want) in rows {
        let src = format!(r#"(clojure.core/__new* '{class} {args})"#);
        assert_eq!(uncaught(&mut eval, &src), (*want).to_owned(), "{class}");
    }
}

/// `BigInteger` arithmetic and bitwise ops.
#[test]
fn bigint_arithmetic() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(.add (biginteger 7) (biginteger 5)) (.subtract (biginteger 7) (biginteger 10)) (.multiply (biginteger -3) (biginteger 4)) (.divide (biginteger -7) (biginteger 3)) (.divide (biginteger 7) (biginteger -3))]"
        ),
        "[12 -3 -12 -2 -2]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(.and (biginteger 12) (biginteger 10)) (.or (biginteger 12) (biginteger 10)) (.xor (biginteger 12) (biginteger 10)) (.not (biginteger 5)) (.not (biginteger -6))]"
        ),
        "[8 14 6 -6 5]"
    );
    assert_eq!(
        uncaught(&mut eval, "(.divide (biginteger 1) (biginteger 0))"),
        "java.lang.ArithmeticException | BigInteger divide by zero"
    );
    // Nil params NPE; mistyped params miss.
    assert_eq!(
        uncaught(&mut eval, "(.add (biginteger 1) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.math.BigInteger.add\" because \"val\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.add (biginteger 1) 5)"),
        "java.lang.IllegalArgumentException | No matching method add found taking 1 args for class java.math.BigInteger"
    );
    // `BigInt` twins take the generic miss (JVM has no such methods).
    assert_eq!(
        uncaught(&mut eval, "(.add (bigint 1) (biginteger 2))"),
        "java.lang.IllegalArgumentException | No matching method add found taking 1 args for class clojure.lang.BigInt"
    );
}

/// `BigInteger` shifts, powers, and modular arithmetic.
#[test]
fn bigint_shifts_and_modular() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(.shiftLeft (biginteger 3) 4) (.shiftRight (biginteger 48) 4) (.shiftLeft (biginteger 48) -4) (.shiftRight (biginteger 3) -4) (.shiftRight (biginteger -48) 4)]"
        ),
        "[48 3 3 48 -3]"
    );
    assert_eq!(
        run(&mut eval, "(.pow (biginteger 2) 100)"),
        "1267650600228229401496703205376"
    );
    // Diffie-Hellman-shaped: `g^x mod p`.
    assert_eq!(
        run(
            &mut eval,
            "(.modPow (biginteger 5) (biginteger 6) (biginteger 23))"
        ),
        "8"
    );
    assert_eq!(run(&mut eval, "(.mod (biginteger -7) (biginteger 3))"), "2");
    assert_eq!(
        run(&mut eval, "(.modInverse (biginteger 17) (biginteger 3120))"),
        "2753"
    );
    assert_eq!(
        uncaught(&mut eval, "(.pow (biginteger 2) -1)"),
        "java.lang.ArithmeticException | Negative exponent"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.modPow (biginteger 2) (biginteger -1) (biginteger 5))"
        ),
        "java.lang.ArithmeticException | BigInteger: negative exponent"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.modPow (biginteger 2) (biginteger 3) (biginteger 0))"
        ),
        "java.lang.ArithmeticException | BigInteger: modulus not positive"
    );
    assert_eq!(
        uncaught(&mut eval, "(.mod (biginteger 2) (biginteger -3))"),
        "java.lang.ArithmeticException | BigInteger: modulus not positive"
    );
    assert_eq!(
        uncaught(&mut eval, "(.modInverse (biginteger 2) (biginteger 4))"),
        "java.lang.ArithmeticException | BigInteger not invertible"
    );
    assert_eq!(
        uncaught(&mut eval, "(.modPow (biginteger 2) 3 (biginteger 5))"),
        "java.lang.IllegalArgumentException | No matching method modPow found taking 2 args for class java.math.BigInteger"
    );
}

/// `BigInteger` inspection and conversion.
#[test]
fn bigint_inspection() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(.signum (biginteger -5)) (.signum (biginteger 0)) (.signum (biginteger 5)) (.bitLength (biginteger 0)) (.bitLength (biginteger 255)) (.bitLength (biginteger -256)) (.bitLength (biginteger -255))]"
        ),
        "[-1 0 1 0 8 8 8]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(.testBit (biginteger 5) 0) (.testBit (biginteger 5) 1) (.setBit (biginteger 5) 1) (.setBit (biginteger -8) 2)]"
        ),
        "[true false 7 -4]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"[(.toString (biginteger 255) 16) (.toString (biginteger -255) 16) (.toString (biginteger 5) 2) (.toString (biginteger 5) 99)]"#
        ),
        r#"["ff" "-ff" "101" "5"]"#
    );
    assert_eq!(run(&mut eval, "(vec (.toByteArray (biginteger 0)))"), "[0]");
    assert_eq!(
        run(&mut eval, "(vec (.toByteArray (biginteger 256)))"),
        "[1 0]"
    );
    assert_eq!(
        run(&mut eval, "(vec (.toByteArray (biginteger -129)))"),
        "[-1 127]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(.intValue (biginteger 4294967297)) (.longValue (biginteger -1)) (.longValueExact (biginteger 5)) (.compareTo (biginteger 3) (biginteger 4)) (.compareTo (biginteger 4) (biginteger 4)) (.compareTo (biginteger 5) (biginteger 4))]"
        ),
        "[1 -1 5 -1 0 1]"
    );
    assert_eq!(
        uncaught(&mut eval, "(.testBit (biginteger 1) -1)"),
        "java.lang.ArithmeticException | Negative bit address"
    );
    assert_eq!(
        uncaught(&mut eval, "(.setBit (biginteger 1) -1)"),
        "java.lang.ArithmeticException | Negative bit address"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.longValueExact (.shiftLeft BigInteger/ONE 70))"
        ),
        "java.lang.ArithmeticException | BigInteger out of long range"
    );
}

/// `StringBuilder.length` and `toString` (universal).
#[test]
fn builder_length_and_to_string() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"(let [sb (StringBuilder.)] (.append sb "ab") (.append sb \é) [(.length sb) (.toString sb) (str sb)])"#
        ),
        "[3 \"abé\" \"abé\"]"
    );
    // Astral chars count two UTF-16 units.
    assert_eq!(
        run(&mut eval, "(let [sb (StringBuilder. \"😀\")] (.length sb))"),
        "2"
    );
    assert_eq!(run(&mut eval, "(.length (StringBuilder.))"), "0");
}

/// Fully-qualified spellings route like the short ones.
#[test]
fn fq_spellings() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(java.math.BigInteger/valueOf 7) java.math.BigInteger/ONE java.math.BigInteger/TWO java.math.BigInteger/ZERO]"
        ),
        "[7 1 2 0]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (.nextInt (java.util.Random. 5) 100)] [(= a (.nextInt (java.util.Random. 5) 100)) (< -1 a 100)])"
        ),
        "[true true]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (java.security.SecureRandom.) (java.util.Random.) :ok)"
        ),
        ":ok"
    );
}

/// Bare class constants the suite reads.
#[test]
fn bare_constants() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[Math/PI Math/E Double/POSITIVE_INFINITY Double/NEGATIVE_INFINITY Long/MAX_VALUE Long/MIN_VALUE]"
        ),
        "[3.141592653589793 2.718281828459045 ##Inf ##-Inf 9223372036854775807 -9223372036854775808]"
    );
    assert!(run(&mut eval, "(Double/isNaN Double/NaN)").contains("true"));
    assert_eq!(
        run(
            &mut eval,
            "[(count clojure.lang.PersistentQueue/EMPTY) (seq clojure.lang.PersistentQueue/EMPTY)]"
        ),
        "[0 nil]"
    );
}

/// `getBytes` over US-ASCII (the `fletcher16` shape).
#[test]
fn getbytes_ascii() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, r#"(vec (.getBytes "Az09" "US-ASCII"))"#),
        "[65 122 48 57]"
    );
}

/// `Arrays/copyOfRange` over `byte[]`: slice, pad, errors.
#[test]
fn arrays_copy_of_range() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(vec (java.util.Arrays/copyOfRange (byte-array [1 2 3]) 1 3))"
        ),
        "[2 3]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(vec (java.util.Arrays/copyOfRange (byte-array [1 2 3]) 1 5))"
        ),
        "[2 3 0 0]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Arrays/copyOfRange (byte-array [1]) 2 1)"
        ),
        "java.lang.IllegalArgumentException | 2 > 1"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.Arrays/copyOfRange nil 0 1)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Arrays/copyOfRange (byte-array [1]) -1 1)"
        ),
        "java.lang.ArrayIndexOutOfBoundsException | -1"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Arrays/copyOfRange (byte-array [1]) 2 3)"
        ),
        "java.lang.ArrayIndexOutOfBoundsException | 2"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Arrays/copyOfRange (int-array [1]) 0 1)"
        ),
        "java.lang.IllegalArgumentException | No matching method copyOfRange found taking 3 args for class java.util.Arrays"
    );
    assert_eq!(
        compile_err(
            &mut eval,
            "(java.util.Arrays/copyOfRange (byte-array [1]) 0)"
        ),
        "java.lang.IllegalArgumentException | No matching method copyOfRange found taking 2 args for class java.util.Arrays"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.util.Arrays", "copyOfRange", "1 2"),
        "java.lang.IllegalArgumentException | No matching method copyOfRange found taking 2 args for class java.util.Arrays"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.util.Arrays", "nope", "1 2 3"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 3 args for class java.util.Arrays"
    );
}

/// `java.net.URI` construction and `getHost`.
#[test]
fn uri_and_get_host() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(.getHost (java.net.URI. "https://example.com:8080/a?b#c")) (.getHost (java.net.URI. "http://user@host/x")) (.getHost (java.net.URI. "http://[::1]:8080/")) (.getHost (java.net.URI. "mailto:foo@bar.com"))]"#
        ),
        "[\"example.com\" \"host\" \"::1\" nil]"
    );
    assert_eq!(
        run(&mut eval, r#"(str (java.net.URI. "https://x.io/"))"#),
        "\"https://x.io/\""
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(= (java.net.URI. "https://x.io/") (java.net.URI. "https://x.io/"))"#
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.net.URI. "http://exa mple.com")"#),
        "java.net.URISyntaxException | Illegal character in path at index 10: http://exa mple.com"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.net.URI. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.net.URI.<init>\" because \"str\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.net.URI. 5)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.net.URI"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.net.URI.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.net.URI"
    );
}

/// `URLEncoder/encode`: form encoding, charsets, nils.
#[test]
fn urlencoder_encode() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(java.net.URLEncoder/encode "a b+c" "UTF-8") (java.net.URLEncoder/encode "café" "UTF-8") (java.net.URLEncoder/encode "a-_.~" "UTF-8")]"#
        ),
        "[\"a+b%2Bc\" \"caf%C3%A9\" \"a-_.%7E\"]"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.net.URLEncoder/encode "a" "NOPE")"#),
        "java.io.UnsupportedEncodingException | NOPE"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.net.URLEncoder/encode nil "UTF-8")"#),
        "java.lang.NullPointerException | Cannot invoke \"java.net.URLEncoder.encode(String, String)\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.net.URLEncoder/encode "a" nil)"#),
        "java.lang.NullPointerException | Cannot invoke \"java.net.URLEncoder.encode(String, String)\" because \"enc\" is null"
    );
    assert_eq!(
        compile_err(&mut eval, r#"(java.net.URLEncoder/encode "a")"#),
        "java.lang.IllegalArgumentException | No matching method encode found taking 1 args for class java.net.URLEncoder"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.net.URLEncoder", "encode", "1 2 3"),
        "java.lang.IllegalArgumentException | No matching method encode found taking 3 args for class java.net.URLEncoder"
    );
}

/// Universal `equals` on bigintegers, `getSimpleName`.
#[test]
fn bigint_equals_and_simple_name() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "[(.equals (biginteger 5) (biginteger 5)) (.equals (biginteger 5) (biginteger 6)) (.equals (biginteger 5) \"5\")]"
        ),
        "[true false false]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"[(.getSimpleName (class "x")) (.getSimpleName (class (biginteger 1))) (.getSimpleName (class 1))]"#
        ),
        "[\"String\" \"BigInteger\" \"Long\"]"
    );
}

/// `Matcher.find/group/start/end`, incl. the weddles `\G`
/// shape (anchored at the search offset).
#[test]
fn matcher_find_group_start_end() {
    let mut eval = boot();
    // The weddles number-token shape: find from an offset.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [m (re-matcher #"\G(?:\d+(?:\.\d*)?|\.\d+)(?:[eE][+-]?\d+)?" "x+12.5")] [(.find m 2) (.group m) (.end m) (.start m)])"#
        ),
        "[true \"12.5\" 6 2]"
    );
    // Anchored: no match away from the offset.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [m (re-matcher #"\G\d+" "ab12")] [(.find m 0) (.find m 2) (.group m)])"#
        ),
        "[false true \"12\"]"
    );
    // Unanchored `find/0` iterates.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [m (re-matcher #"\d+" "a1b22")] [(.find m) (.group m) (.find m) (.group m) (.find m)])"#
        ),
        "[true \"1\" true \"22\" false]"
    );
    // Groups: present, unmatched (nil), past-the-end (IOOBE).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [m (re-matcher #"(\d+)(x)?" "ab12")] (.find m) [(.group m 1) (.group m 2) (.start m) (.end m)])"#
        ),
        "[\"12\" nil 2 4]"
    );
    // A fresh handle has no match (ISE); bad starts/indices IOOBE.
    assert_eq!(
        uncaught(&mut eval, r#"(.group (re-matcher #"\d+" "12"))"#),
        "java.lang.IllegalStateException | No match found"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.start (re-matcher #"\d+" "12"))"#),
        "java.lang.IllegalStateException | No match found"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.end (re-matcher #"\d+" "12"))"#),
        "java.lang.IllegalStateException | No match found"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.find (re-matcher #"\d+" "12") -1)"#),
        "java.lang.IndexOutOfBoundsException | -1"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(.find (re-matcher #"\d+" "12") 3)"#),
        "java.lang.IndexOutOfBoundsException | 3"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (re-matcher #"(\d+)" "12")] (.find m) (.group m 9))"#
        ),
        "java.lang.IndexOutOfBoundsException | No group 9"
    );
    // A failed `find` clears the last match.
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (re-matcher #"\G\d+" "1a")] (.find m) (.find m 1) (.group m))"#
        ),
        "java.lang.IllegalStateException | No match found"
    );
    // Non-handles miss; nil indices unbox-NPE.
    assert_eq!(
        uncaught(&mut eval, r#"(.find "12")"#),
        "java.lang.IllegalArgumentException | No matching field found: find for class java.lang.String"
    );
    assert!(
        uncaught(&mut eval, r#"(.find (re-matcher #"\d+" "12") nil)"#)
            .contains("NullPointerException")
    );
    // `re-find` over an anchored handle obeys the anchor.
    assert_eq!(
        run(&mut eval, r#"(re-find (re-matcher #"\G\d+" "ab12"))"#),
        "nil"
    );
    assert_eq!(
        run(&mut eval, r#"(re-find (re-matcher #"\G\d+" "12ab"))"#),
        "\"12\""
    );
}

/// `Pattern/quote`: literal quoting, nils, misses.
#[test]
fn pattern_quote() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"[(java.util.regex.Pattern/quote "a.b") (java.util.regex.Pattern/quote "a\\Eb")]"#
        ),
        "[\"\\\\Qa.b\\\\E\" \"\\\\Qa\\\\E\\\\\\\\E\\\\Qb\\\\E\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(re-find (re-pattern (java.util.regex.Pattern/quote "a+b")) "xa+bz")"#,
        ),
        "\"a+b\""
    );
    // Quotation spans mid-pattern; unclosed quotes to the end;
    // an escaped `\\Q` stays a literal backslash plus `Q`.
    assert_eq!(
        run(&mut eval, r#"(re-find (re-pattern "a\\Q.+\\Eb") "xa.+bz")"#),
        "\"a.+b\""
    );
    assert_eq!(
        run(&mut eval, r#"(re-find (re-pattern "\\Qa+b") "xa+bz")"#),
        "\"a+b\""
    );
    assert_eq!(
        run(&mut eval, r#"(re-find (re-pattern "\\\\Q12") "x\\Q12")"#),
        "\"\\\\Q12\""
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.regex.Pattern/quote nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.regex.Pattern.quote(String)\" because \"s\" is null"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.util.regex.Pattern/quote)"),
        "java.lang.IllegalArgumentException | No matching method quote found taking 0 args for class java.util.regex.Pattern"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.util.regex.Pattern", "quote", "1 2"),
        "java.lang.IllegalArgumentException | No matching method quote found taking 2 args for class java.util.regex.Pattern"
    );
}

/// A unique temp path (parallel-safe; the caller cleans up).
fn io_tmp(prefix: &str) -> std::path::PathBuf {
    use std::sync::atomic::{AtomicU64, Ordering};
    static IO_SEQ: AtomicU64 = AtomicU64::new(0);
    let n = IO_SEQ.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!("algo205-{prefix}-{}-{n}", std::process::id()))
}

/// `File` construction, `exists`, `length`.
#[test]
fn file_ctor_and_members() {
    let mut eval = boot();
    let path = io_tmp("file");
    std::fs::write(&path, "AB").unwrap();
    let p = path.to_string_lossy().replace('\\', "/");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"[(.exists (java.io.File. "{p}")) (.length (java.io.File. "{p}")) (str (java.io.File. "{p}")) (.exists (java.io.File. "{p}-missing")) (.length (java.io.File. "{p}-missing"))]"#
            )
        ),
        format!("[true 2 \"{p}\" false 0]")
    );
    // `spit`/`slurp` round-trip through `File` values.
    assert_eq!(
        run(
            &mut eval,
            &format!(r#"(do (spit (java.io.File. "{p}") "xy") (slurp (java.io.File. "{p}")))"#)
        ),
        "\"xy\""
    );
    assert_eq!(
        uncaught(&mut eval, "(java.io.File. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.File.<init>\" because \"pathname\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.io.File. 5)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.io.File"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.io.File.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.io.File"
    );
    std::fs::remove_file(&path).unwrap();
}

/// `File/createTempFile`: unique files, defaults, failures.
#[test]
fn file_create_temp_file() {
    let mut eval = boot();
    let made: String = run(
        &mut eval,
        r#"(do (require 'clojure.java.io) (let [f (java.io.File/createTempFile "block_" ".dat")] [(str f) (.exists f) (.length f)]))"#,
    );
    assert!(made.contains("block_") && made.contains(".dat"));
    // Cleanup through `io/delete-file`, like `external-sort`.
    assert_eq!(
        run(
            &mut eval,
            r#"(do (require 'clojure.java.io) (let [f (str (java.io.File/createTempFile "block_" ".dat"))] [(clojure.java.io/delete-file f) (.exists (java.io.File. f))]))"#,
        ),
        "[true false]"
    );
    // Two calls mint distinct files.
    assert_eq!(
        run(
            &mut eval,
            r#"(= (java.io.File/createTempFile "block_" ".dat") (java.io.File/createTempFile "block_" ".dat"))"#,
        ),
        "false"
    );
    // Nil suffixes default to `.tmp`; short prefixes fail.
    assert_eq!(
        run(
            &mut eval,
            r#"(.endsWith (str (java.io.File/createTempFile "block_" nil)) ".tmp")"#,
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.io.File/createTempFile "ab" ".dat")"#),
        "java.lang.IllegalArgumentException | Prefix string too short"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(java.io.File/createTempFile nil ".dat")"#),
        "java.lang.NullPointerException | Cannot invoke \"java.io.File.createTempFile(String, String)\" because \"prefix\" is null"
    );
    assert!(
        uncaught(&mut eval, r#"(java.io.File/createTempFile 5 ".dat")"#)
            .contains("ClassCastException")
    );
    assert_eq!(
        compile_err(&mut eval, r#"(java.io.File/createTempFile "block_")"#),
        "java.lang.IllegalArgumentException | No matching method createTempFile found taking 1 args for class java.io.File"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.io.File", "createTempFile", "1 2 3"),
        "java.lang.IllegalArgumentException | No matching method createTempFile found taking 3 args for class java.io.File"
    );
    // An uncreatable path reports `IOException` naming it.
    let bad = uncaught(
        &mut eval,
        r#"(java.io.File/createTempFile "nope/nodir/x" ".dat")"#,
    );
    assert!(bad.contains("java.io.IOException") && bad.contains("nope"));
}

/// File streams: byte writes, `readAllBytes`, closes, ctors.
#[test]
fn file_streams_round_trip() {
    let mut eval = boot();
    let path = io_tmp("streams");
    let p = path.to_string_lossy().replace('\\', "/");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (with-open [out (java.io.FileOutputStream. "{p}")] (.write out 65) (.write out 300)) (with-open [in (java.io.FileInputStream. "{p}")] (vec (.readAllBytes in))))"#
            )
        ),
        "[65 44]"
    );
    // `File` args work; closes latch (writes/reads then fail).
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (with-open [out (java.io.FileOutputStream. (java.io.File. "{p}"))] (.write out 7)) (with-open [in (java.io.FileInputStream. (java.io.File. "{p}"))] (vec (.readAllBytes in))))"#
            )
        ),
        "[7]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                r#"(let [out (java.io.FileOutputStream. "{p}")] (.close out) (.write out 1))"#
            )
        ),
        "java.io.IOException | Stream closed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                r#"(let [in (java.io.FileInputStream. "{p}")] (.close in) (.readAllBytes in))"#
            )
        ),
        "java.io.IOException | Stream closed"
    );
    // Missing inputs name the path; nils NPE; mistypes miss.
    let missing = uncaught(
        &mut eval,
        &format!(r#"(java.io.FileInputStream. "{p}-missing")"#),
    );
    assert!(missing.contains("FileNotFoundException") && missing.contains("missing"));
    assert_eq!(
        uncaught(&mut eval, "(java.io.FileInputStream. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.FileInputStream.<init>\" because \"name\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.io.FileOutputStream. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.FileOutputStream.<init>\" because \"name\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.io.FileInputStream. 5)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.io.FileInputStream"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.io.FileOutputStream.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.io.FileOutputStream"
    );
    std::fs::remove_file(&path).unwrap();
}

/// `BufferedReader.readLine` over `io/reader` (the
/// `external-sort` shape): lines, EOF nil, closes.
#[test]
fn reader_read_line_shapes() {
    let mut eval = boot();
    let path = io_tmp("lines");
    std::fs::write(&path, "a\nb\n").unwrap();
    let p = path.to_string_lossy().replace('\\', "/");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (with-open [r (clojure.java.io/reader "{p}")] [(.readLine r) (.readLine r) (.readLine r)]))"#
            )
        ),
        "[\"a\" \"b\" nil]"
    );
    // `line-seq` over the same reader shape still works.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (with-open [r (clojure.java.io/reader "{p}")] (vec (line-seq r))))"#
            )
        ),
        "[\"a\" \"b\"]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (let [r (clojure.java.io/reader "{p}")] (.close r) (.readLine r)))"#
            )
        ),
        "java.io.IOException | Stream closed"
    );
    assert!(uncaught(&mut eval, "(.readLine \"ab\")").contains("No matching"));
    std::fs::remove_file(&path).unwrap();
}

/// `Writer.write` shapes: strings, chars, ints, and the
/// `print-method`/`*out*` suite shapes.
#[test]
fn writer_write_shapes() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            r#"(let [w (java.io.StringWriter.)] (.write w "ab") (.write w \c) (.write w 68) (.write w 0xD83D) (str w))"#,
        ),
        "\"abcD�\""
    );
    // Bound `*out*` collects prints (the LDA/weddles test shape).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [sw (java.io.StringWriter.)] (binding [*out* sw] (println "hi")) (str sw))"#,
        ),
        "\"hi\\n\""
    );
    // `print-method` bodies write through `w` (the Dual/Point shape).
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord WR [x]) (defmethod print-method WR [o w] (.write ^java.io.Writer w (str \"WR!\" (:x o)))) (with-out-str (pr (WR. 1))))",
        ),
        "\"WR!1\""
    );
    // File writers persist; nils NPE; mistypes miss.
    let path = io_tmp("wrote");
    let p = path.to_string_lossy().replace('\\', "/");
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (with-open [w (clojure.java.io/writer "{p}")] (.write w "xy")) (slurp "{p}"))"#
            )
        ),
        "\"xy\""
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [w (java.io.StringWriter.)] (.write w nil))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Writer.write(String)\" because \"str\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [w (java.io.StringWriter.)] (.write w [1]))"
        ),
        "java.lang.IllegalArgumentException | No matching method write found taking 1 args for class java.io.Writer"
    );
    assert!(uncaught(&mut eval, "(.write \"ab\" \"c\")").contains("No matching"));
    std::fs::remove_file(&path).unwrap();
}

/// `io/resource`: load-path hits as `file:` URLs, misses nil.
#[test]
fn io_resource_shapes() {
    let mut eval = boot();
    let dir = io_tmp("resdir");
    std::fs::create_dir(&dir).unwrap();
    std::fs::write(dir.join("res.txt"), "RES!").unwrap();
    let load = dir.to_string_lossy().replace('\\', "/");
    // A hit reads back through `io/reader` (the anagrams shape).
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (binding [clojure.core/*load-path* ["{load}"]] (let [url (clojure.java.io/resource "res.txt")] [(some? url) (.startsWith (str url) "file:") (slurp (clojure.java.io/reader url))]))) "#
            )
        ),
        "[true true \"RES!\"]"
    );
    // Misses answer nil; nil names NPE, mistyped names CCE.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (binding [clojure.core/*load-path* ["{load}"]] (clojure.java.io/resource "nope.txt")))"#
            )
        ),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (require 'clojure.java.io) (clojure.java.io/resource nil))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.ClassLoader.getResource(String)\" because \"name\" is null"
    );
    assert!(
        uncaught(
            &mut eval,
            "(do (require 'clojure.java.io) (clojure.java.io/resource 5))"
        )
        .contains("ClassCastException")
    );
    // A relative load dir resolves against the process directory.
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.subsec_nanos())
        .unwrap_or(0);
    let rel = format!("algo205-rel-{}-{nanos}", std::process::id());
    let sub = std::env::current_dir().unwrap().join(&rel);
    std::fs::create_dir(&sub).unwrap();
    std::fs::write(sub.join("rel.txt"), "REL!").unwrap();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(do (require 'clojure.java.io) (binding [clojure.core/*load-path* ["{rel}"]] (slurp (clojure.java.io/reader (clojure.java.io/resource "rel.txt")))))"#
            )
        ),
        "\"REL!\""
    );
    std::fs::remove_dir_all(&sub).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
}

/// `ArrayDeque`: ends add/poll/peek, collection ctors, nils.
#[test]
fn deque_ends() {
    let mut eval = boot();
    // The 0-1 BFS shape: collect, both ends, empties nil.
    assert_eq!(
        run(
            &mut eval,
            "(let [dq (java.util.ArrayDeque. [1])] (.addFirst dq 0) (.addLast dq 2) (.add dq 3) [(.peekFirst dq) (.peekLast dq) (.poll dq) (.pollFirst dq) (.pollLast dq) (.pollLast dq) (.pollFirst dq) (.isEmpty dq)])"
        ),
        "[0 3 0 1 3 2 nil true]"
    );
    // Empty ends peek/poll nil.
    assert_eq!(
        run(
            &mut eval,
            "(let [dq (java.util.ArrayDeque.)] [(.peek dq) (.peekFirst dq) (.peekLast dq) (.poll dq) (.isEmpty dq)])"
        ),
        "[nil nil nil nil true]"
    );
    // Null elements NPE; unseqables take the `ISeq` error.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [dq (java.util.ArrayDeque.)] (.addLast dq nil))"
        ),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [dq (java.util.ArrayDeque.)] (.addFirst dq nil))"
        ),
        "java.lang.NullPointerException | "
    );
    assert!(uncaught(&mut eval, "(java.util.ArrayDeque. 5)").contains("ISeq"));
    assert_eq!(
        uncaught(&mut eval, "(java.util.ArrayDeque. nil)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        compile_err(&mut eval, "(java.util.ArrayDeque. 1 2)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.util.ArrayDeque"
    );
}

/// `ArrayList` collect/seq plus `Collections/shuffle`.
#[test]
fn arraylist_collect_seq_shuffle() {
    let mut eval = boot();
    // Collect and seq (the TSP `shuffled` shape).
    assert_eq!(
        run(
            &mut eval,
            "(let [a (java.util.ArrayList. [1 2 3])] [(vec a) (seq a) (first a) (count a)])"
        ),
        "[[1 2 3] (1 2 3) 1 3]"
    );
    assert_eq!(run(&mut eval, "(seq (java.util.ArrayList.))"), "nil");
    // Seeded shuffles are deterministic permutations (exact pins
    // guard the draw order; the TSP oracles verify the values).
    assert_eq!(
        run(
            &mut eval,
            "(let [a (java.util.ArrayList. [0 1 2 3 4 5 6 7])] (java.util.Collections/shuffle a (java.util.Random. 0)) (vec a))"
        ),
        "[0 7 3 4 6 1 2 5]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (java.util.ArrayList. [0 1 2 3 4 5 6 7])] (java.util.Collections/shuffle a (java.util.Random. 0)) [(vec a) (= (sort (vec a)) (range 8))])"
        ),
        "[[0 7 3 4 6 1 2 5] true]"
    );
    // Shuffling answers nil (void); nils NPE, mistypes CCE.
    assert_eq!(
        run(
            &mut eval,
            "(java.util.Collections/shuffle (java.util.ArrayList. [1]) (java.util.Random. 0))"
        ),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Collections/shuffle nil (java.util.Random. 0))"
        ),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(java.util.Collections/shuffle (java.util.ArrayList.) nil)"
        ),
        "java.lang.NullPointerException | "
    );
    assert!(
        uncaught(
            &mut eval,
            "(java.util.Collections/shuffle [1 2] (java.util.Random. 0))"
        )
        .contains("java.util.List")
    );
    assert!(
        uncaught(
            &mut eval,
            "(java.util.Collections/shuffle (java.util.ArrayList.) 5)"
        )
        .contains("java.util.Random")
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.ArrayList. nil)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        compile_err(
            &mut eval,
            "(java.util.Collections/shuffle (java.util.ArrayList.))"
        ),
        "java.lang.IllegalArgumentException | No matching method shuffle found taking 1 args for class java.util.Collections"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.util.Collections", "shuffle", "1"),
        "java.lang.IllegalArgumentException | No matching method shuffle found taking 1 args for class java.util.Collections"
    );
}

/// `PriorityQueue`: natural ordering, reified comparators, fns.
#[test]
fn pqueue_orderings() {
    let mut eval = boot();
    // The minimize-deviation shape: natural ordering over a coll.
    assert_eq!(
        run(
            &mut eval,
            "(let [heap (java.util.PriorityQueue. [-4 -2 -8])] [(.peek heap) (.poll heap) (.peek heap) (.isEmpty heap)])"
        ),
        "[-8 -8 -4 false]"
    );
    // Empties poll/peek nil; `add` answers true.
    assert_eq!(
        run(
            &mut eval,
            "(let [heap (java.util.PriorityQueue. [])] [(.poll heap) (.peek heap) (.add heap 1) (.peek heap)])"
        ),
        "[nil nil true 1]"
    );
    // The smallest-range shape: a reified `Comparator` over pairs.
    assert_eq!(
        run(
            &mut eval,
            "(do (import 'java.util.Comparator) (let [cmp (reify Comparator (compare [_ a b] (compare (first a) (first b)))) heap (java.util.PriorityQueue. cmp)] (.add heap [3 :c]) (.add heap [1 :a]) (.add heap [2 :b]) [(.poll heap) (.poll heap) (.poll heap) (.poll heap)]))"
        ),
        "[[1 :a] [2 :b] [3 :c] nil]"
    );
    // Plain fns compare too; throwing comparators propagate.
    assert_eq!(
        run(
            &mut eval,
            "(let [heap (java.util.PriorityQueue. (fn [a b] (compare b a)))] (.add heap 1) (.add heap 3) (.add heap 2) [(.poll heap) (.poll heap)])"
        ),
        "[3 2]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [heap (java.util.PriorityQueue. (fn [a b] (throw (IllegalArgumentException. "boom"))))] (.add heap 1) (.add heap 2) (.poll heap))"#
        ),
        "java.lang.IllegalArgumentException | boom"
    );
    // Doubles order by truncated sign; anything else CCEs.
    assert_eq!(
        run(
            &mut eval,
            "(let [heap (java.util.PriorityQueue. (fn [a b] (double (- a b))))] (.add heap 1) (.add heap 3) [(.poll heap) (.poll heap)])"
        ),
        "[1 3]"
    );
    assert!(uncaught(
        &mut eval,
        r#"(let [heap (java.util.PriorityQueue. (fn [a b] "x"))] (.add heap 1) (.add heap 2) (.poll heap))"#
    )
    .contains("ClassCastException"));
    // `offer` aliases `add`; peeks throw through comparators too.
    assert_eq!(
        run(
            &mut eval,
            "(let [dq (java.util.ArrayDeque.) heap (java.util.PriorityQueue. [])] [(.offer dq 1) (.offerFirst dq 0) (.offerLast dq 2) (.offer heap 5) (.pollFirst dq) (.poll heap)])"
        ),
        "[true true true true 0 5]"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [heap (java.util.PriorityQueue. (fn [a b] (throw (IllegalArgumentException. "peek-boom"))))] (.add heap 1) (.add heap 2) (.peek heap))"#
        ),
        "java.lang.IllegalArgumentException | peek-boom"
    );
    // Nils NPE on add and ambiguate the ctor; mistypes ISeq.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [heap (java.util.PriorityQueue. [])] (.add heap nil))"
        ),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.PriorityQueue. nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: java.util.PriorityQueue"
    );
    assert!(uncaught(&mut eval, "(java.util.PriorityQueue. 5)").contains("ISeq"));
    assert_eq!(
        compile_err(&mut eval, "(java.util.PriorityQueue.)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.util.PriorityQueue"
    );
}

/// `MessageDigest` vectors (RFC 1321 + FIPS 180-4, hexed the
/// suite's own `%02x` way — all nine verified against the
/// system `md5`/`shasum`, plus a multi-block input each).
#[test]
fn digest_vectors() {
    let mut eval = boot();
    let hex_of = |algo: &str, s: &str| {
        format!(
            r#"(let [md (java.security.MessageDigest/getInstance "{algo}")] (.update md (.getBytes "{s}" "UTF-8")) (apply str (map #(format "%02x" (bit-and % 0xFF)) (.digest md))))"#
        )
    };
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "[{} {} {}]",
                hex_of("MD5", ""),
                hex_of("MD5", "abc"),
                hex_of("MD5", "The quick brown fox jumps over the lazy dog")
            )
        ),
        "[\"d41d8cd98f00b204e9800998ecf8427e\" \"900150983cd24fb0d6963f7d28e17f72\" \"9e107d9d372bb6826bd81d3542a419d6\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "[{} {} {}]",
                hex_of("SHA-256", ""),
                hex_of("SHA-256", "abc"),
                hex_of("SHA-256", "The quick brown fox jumps over the lazy dog")
            )
        ),
        "[\"e3b0c44298fc1c149afbf4c8996fb92427ae41e4649b934ca495991b7852b855\" \"ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad\" \"d7a8fbb307d7809469ca9abcb0082e4f8d5651e46d3cdb762d02d0bf37c9e592\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "[{} {} {}]",
                hex_of("SHA-512", ""),
                hex_of("SHA-512", "abc"),
                hex_of("SHA-512", "The quick brown fox jumps over the lazy dog")
            )
        ),
        "[\"cf83e1357eefb8bdf1542850d66d8007d620e4050b5715dc83f4a921d36ce9ce47d0d13c5d85f2b0ff8318d2877eec2f63b931bd47417a81a538327af927da3e\" \"ddaf35a193617abacc417349ae20413112e6fa4e89a97ea20a9eeee64b55d39a2192992a274fc1a836ba3c23a3feebbd454d4423643ce80e2a9ac94fa54ca49f\" \"07e547d9586f6a73f73fbac0435ed76951218fb7d0c8d788a309d785436bbb642e93a252a954f23912547d1e8a3b5ed6e1bfd7097821233fa0538f3db854fee6\"]"
    );
    // Multi-block inputs (200 bytes) compress eagerly.
    let long = "a".repeat(200);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "[{} {} {}]",
                hex_of("MD5", &long),
                hex_of("SHA-256", &long),
                hex_of("SHA-512", &long)
            )
        ),
        "[\"887f30b43b2867f4a9accceee7d16e6c\" \"c2a908d98f5df987ade41b5fce213067efbcc21ef2240212a41e54b5e7c28ae5\" \"4b11459c33f52a22ee8236782714c150a3b2c60994e9acee17fe68947a3e6789f31e7668394592da7bef827cddca88c4e6f86e4df7ed1ae6cba71f3e98faee9f\"]"
    );
}

/// `MessageDigest` shapes: arities, resets, errors.
#[test]
fn digest_shapes() {
    let mut eval = boot();
    // `digest(bytes)` feeds first; split updates match; lengths.
    assert_eq!(
        run(
            &mut eval,
            r#"[(let [a (java.security.MessageDigest/getInstance "SHA-256") b (java.security.MessageDigest/getInstance "SHA-256")] (.update a (.getBytes "ab" "UTF-8")) (= (vec (.digest a)) (vec (.digest b (.getBytes "ab" "UTF-8"))))) (let [m (java.security.MessageDigest/getInstance "MD5")] (.update m (.getBytes "a" "UTF-8")) (.update m (.getBytes "bc" "UTF-8")) (= (vec (.digest m)) (vec (.digest (java.security.MessageDigest/getInstance "MD5") (.getBytes "abc" "UTF-8"))))) (count (.digest (java.security.MessageDigest/getInstance "MD5"))) (count (.digest (java.security.MessageDigest/getInstance "SHA-256"))) (count (.digest (java.security.MessageDigest/getInstance "SHA-512")))]"#
        ),
        "[true true 16 32 64]"
    );
    // `digest()` resets (the handle digests afresh after).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [m (java.security.MessageDigest/getInstance "SHA-256")] (.update m (.getBytes "abc" "UTF-8")) (.digest m) (= (vec (.digest m)) (vec (.digest (java.security.MessageDigest/getInstance "SHA-256")))))"#
        ),
        "true"
    );
    // Lookups are case-blind; unknown names name themselves.
    assert_eq!(
        run(
            &mut eval,
            r#"(count (.digest (java.security.MessageDigest/getInstance "sha-256") (.getBytes "a" "UTF-8")))"#
        ),
        "32"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(java.security.MessageDigest/getInstance "SHA-1")"#,
        ),
        "java.security.NoSuchAlgorithmException | SHA-1 MessageDigest not available"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.security.MessageDigest/getInstance nil)",),
        "java.lang.NullPointerException | Cannot invoke \"java.security.MessageDigest.getInstance(String)\" because \"algorithm\" is null"
    );
    assert!(
        uncaught(&mut eval, "(java.security.MessageDigest/getInstance 5)")
            .contains("ClassCastException")
    );
    assert_eq!(
        compile_err(&mut eval, "(java.security.MessageDigest/getInstance)"),
        "java.lang.IllegalArgumentException | No matching method getInstance found taking 0 args for class java.security.MessageDigest"
    );
    assert_eq!(
        runtime_miss(
            &mut eval,
            "java.security.MessageDigest",
            "getInstance",
            "1 2"
        ),
        "java.lang.IllegalArgumentException | No matching method getInstance found taking 2 args for class java.security.MessageDigest"
    );
    // Input errors name their method; non-digests miss.
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (java.security.MessageDigest/getInstance "MD5")] (.update m nil))"#
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.security.MessageDigest.update(byte[])\" because \"input\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (java.security.MessageDigest/getInstance "MD5")] (.digest m nil))"#
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.security.MessageDigest.digest(byte[])\" because \"input\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (java.security.MessageDigest/getInstance "MD5")] (.update m "abc"))"#
        ),
        "java.lang.IllegalArgumentException | No matching method update found taking 1 args for class java.security.MessageDigest"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            r#"(let [m (java.security.MessageDigest/getInstance "MD5")] (.digest m (int-array [1])))"#
        ),
        "java.lang.IllegalArgumentException | No matching method digest found taking 1 args for class java.security.MessageDigest"
    );
    assert!(uncaught(&mut eval, "(.digest \"abc\")").contains("No matching"));
}

/// Run `f` on a 256 MiB worker (the harness shape): deep
/// recursion trips the catchable guard there, never an abort.
fn on_worker<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    std::thread::Builder::new()
        .stack_size(256 * 1024 * 1024)
        .spawn(f)
        .expect("spawn worker")
        .join()
        .expect("worker panicked")
}

#[test]
fn call_depth_guard_throws_catchable_stack_overflow() {
    on_worker(|| {
        let mut eval = boot();
        eval.eval_source("(defn algo-boom [] (algo-boom))").unwrap();
        // Caught by the exact class, with a nil message.
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-boom) (catch StackOverflowError e [:caught (nil? (ex-message e))]))"
            ),
            "[:caught true]"
        );
        // `Throwable` (and `Error`/`VirtualMachineError`) catch it;
        // `Exception` does not.
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-boom) (catch Throwable e :via-throwable))"
            ),
            ":via-throwable"
        );
        assert_eq!(
            run(&mut eval, "(try (algo-boom) (catch Error e :via-error))"),
            ":via-error"
        );
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-boom) (catch java.lang.VirtualMachineError e :via-vm) (catch Throwable e :other))"
            ),
            ":via-vm"
        );
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-boom) (catch Exception e :via-exception) (catch Throwable e :via-throwable))"
            ),
            ":via-throwable"
        );
        // The caught value is a genuine `StackOverflowError`.
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-boom) (catch Throwable e [(instance? StackOverflowError e) (str (class e))]))"
            ),
            r#"[true "class java.lang.StackOverflowError"]"#
        );
        // Uncaught, it surfaces with the class and empty message.
        assert_eq!(
            uncaught(&mut eval, "(algo-boom)"),
            "java.lang.StackOverflowError | "
        );
    });
}

#[test]
fn call_depth_guard_leaves_deep_legal_code_alone() {
    on_worker(|| {
        let mut eval = boot();
        // 1000 nested calls (10x the suite's deepest) still run.
        assert_eq!(
            run(
                &mut eval,
                "(do (defn algo-count [n] (if (zero? n) 0 (inc (algo-count (dec n))))) (algo-count 1000))"
            ),
            "1000"
        );
        // Mutual recursion trips the same guard.
        eval.eval_source(
            "(declare algo-pong) (defn algo-ping [] (algo-pong)) (defn algo-pong [] (algo-ping))",
        )
        .unwrap();
        assert_eq!(
            run(
                &mut eval,
                "(try (algo-ping) (catch StackOverflowError e :caught))"
            ),
            ":caught"
        );
    });
    // `recur` never touches the counter (a million iterations).
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(loop [i 100000 acc 0] (if (zero? i) acc (recur (dec i) (inc acc))))"
        ),
        "100000"
    );
}

// Batch F (#205): `BigDecimal` ctors box every surveyed shape
// fresh, like `new` (literals/prints keep their own paths).
#[test]
fn bigdec_ctor_shapes() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal. \"2.50\"))"),
        "\"2.50\""
    );
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal. (biginteger 42)))"),
        "\"42\""
    );
    assert_eq!(run(&mut eval, "(.toPlainString (BigDecimal. 5))"), "\"5\"");
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal. 0.5))"),
        "\"0.5\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (BigDecimal. (BigDecimal. \"1.5\")))"
        ),
        "\"1.5\""
    );
    // Fresh boxes: never identical.
    assert_eq!(
        run(
            &mut eval,
            "(identical? (BigDecimal. \"5\") (BigDecimal. \"5\"))"
        ),
        "false"
    );
}

// Batch F (#205): ctor misses mirror the overload set (bad
// text fails the parse message, empty fails bare, nil NPEs on
// the `String` length, `BigInt` has no overload).
#[test]
fn bigdec_ctor_miss() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(BigDecimal. \"abc\")"),
        "java.lang.NumberFormatException | Character a is neither a decimal digit number, decimal point, nor \"e\" notation exponential mark."
    );
    assert_eq!(
        uncaught(&mut eval, "(BigDecimal. \"\")"),
        "java.lang.NumberFormatException | "
    );
    assert!(uncaught(&mut eval, "(BigDecimal. ##NaN)").contains("NumberFormatException"));
    assert_eq!(
        uncaught(&mut eval, "(BigDecimal. nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"val\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(BigDecimal. 5N)"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigDecimal"
    );
    assert_eq!(
        compile_err(&mut eval, "(BigDecimal. \"1\" \"2\")"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.BigDecimal"
    );
}

// Batch F (#205): `valueOf` — `(long)` exact through the
// canonical compact paths (small values share), `(double)`
// the short rendering.
#[test]
fn bigdec_valueof() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal/valueOf 5))"),
        "\"5\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(identical? (BigDecimal/valueOf 5) (BigDecimal/valueOf 5))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal/valueOf 0.1))"),
        "\"0.1\""
    );
    assert!(uncaught(&mut eval, "(BigDecimal/valueOf ##NaN)").contains("NumberFormatException"));
    // No `(String)`/nil overloads (both real ones take primitives).
    assert_eq!(
        uncaught(&mut eval, "(BigDecimal/valueOf nil)"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 1 args for class java.math.BigDecimal"
    );
    assert_eq!(
        runtime_miss(&mut eval, "java.math.BigDecimal", "valueOf", "1 2"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 2 args for class java.math.BigDecimal"
    );
}

// Batch F (#205): `setScale` — the entropy/sin/simpson shape
// (scale + mode), same-scale identity, `UNNECESSARY`.
#[test]
fn bigdec_setscale() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.setScale (BigDecimal. \"2.5\") 0 java.math.RoundingMode/HALF_EVEN))"
        ),
        "\"2\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.setScale (BigDecimal/valueOf 2.5) 0 java.math.RoundingMode/HALF_UP))"
        ),
        "\"3\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(identical? (let [b (BigDecimal. \"2.5\")] (.setScale b 1 java.math.RoundingMode/HALF_EVEN)) (BigDecimal. \"2.5\"))"
        ),
        "false"
    );
    let mut eval2 = boot();
    assert_eq!(
        run(
            &mut eval2,
            "(let [b (BigDecimal. \"2.5\")] (identical? (.setScale b 1 java.math.RoundingMode/HALF_EVEN) b))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.setScale (BigDecimal. \"2.5\") 0 java.math.RoundingMode/UNNECESSARY)"
        ),
        "java.lang.ArithmeticException | Rounding necessary"
    );
    assert!(
        uncaught(&mut eval, "(.setScale (BigDecimal. \"2.5\") 0 nil)")
            .contains("NullPointerException")
    );
}

// Batch F (#205): context arithmetic — the chudnovsky shapes.
#[test]
fn bigdec_context_arith() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.multiply (BigDecimal. \"426880\") (.sqrt (BigDecimal. \"10005\") (java.math.MathContext. 50 java.math.RoundingMode/HALF_EVEN)) (java.math.MathContext. 50 java.math.RoundingMode/HALF_EVEN)))"
        ),
        "\"42698670.666333395817712889160659608273320884002511\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.add (BigDecimal. \"1.5\") (BigDecimal. \"2.25\") (java.math.MathContext. 2)))"
        ),
        "\"3.8\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.add (BigDecimal. \"1.5\") (BigDecimal. \"2.25\")))"
        ),
        "\"3.75\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.divide (BigDecimal. \"1\") (BigDecimal. \"8\")))"
        ),
        "\"0.125\""
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.divide (BigDecimal. \"1\") (BigDecimal. \"3\"))"
        ),
        "java.lang.ArithmeticException | Non-terminating decimal expansion; no exact representable decimal result."
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.divide (BigDecimal. \"1\") (BigDecimal. \"0\"))"
        ),
        "java.lang.ArithmeticException | Division by zero"
    );
    assert!(
        uncaught(
            &mut eval,
            "(.divide (BigDecimal. \"1\") (BigDecimal. \"2\") nil)"
        )
        .contains("NullPointerException")
    );
}

// Batch F (#205): `sqrt(mc)` — the problem-080 shape, pinned
// against JDK digits.
#[test]
fn bigdec_sqrt() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(.toPlainString (.sqrt (BigDecimal. \"2\") (java.math.MathContext. 110 java.math.RoundingMode/HALF_UP)))"
        ),
        "\"1.4142135623730950488016887242096980785696718753769480731766797379907324784621070388503875343276415727350138462\""
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(.sqrt (BigDecimal. \"-1\") (java.math.MathContext. 10))"
        ),
        "java.lang.ArithmeticException | Attempted square root of negative BigDecimal"
    );
}

// Batch F (#205): conversions — `toPlainString`, `doubleValue`,
// `toBigInteger` (truncation toward zero), `toString`.
#[test]
fn bigdec_conv() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.toPlainString (BigDecimal. \"1E+10\"))"),
        "\"10000000000\""
    );
    assert_eq!(
        run(&mut eval, "(.doubleValue (BigDecimal. \"2.5\"))"),
        "2.5"
    );
    assert_eq!(
        run(&mut eval, "(.toBigInteger (BigDecimal. \"-1.9\"))"),
        "-1"
    );
    assert_eq!(run(&mut eval, "(.toBigInteger (BigDecimal. 512.0))"), "512");
    assert_eq!(
        run(&mut eval, "(.toString (BigDecimal. \"1E+10\"))"),
        "\"1E+10\""
    );
}

// Batch F (#205): the `(java.math.MathContext. …)` interop ctors mirror
// the port-only shapes, except the mode takes the enum object.
#[test]
fn mathcontext_ctor() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str (java.math.MathContext. 10))"),
        "\"precision=10 roundingMode=HALF_UP\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (java.math.MathContext. 110 java.math.RoundingMode/HALF_UP))"
        ),
        "\"precision=110 roundingMode=HALF_UP\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (java.math.MathContext. \"precision=7 roundingMode=FLOOR\"))"
        ),
        "\"precision=7 roundingMode=FLOOR\""
    );
    assert_eq!(
        uncaught(&mut eval, "(java.math.MathContext. -1)"),
        "java.lang.IllegalArgumentException | Digits < 0"
    );
    assert!(
        uncaught(&mut eval, "(java.math.MathContext. 9999999999)").contains("integer overflow")
    );
    assert_eq!(
        uncaught(&mut eval, "(java.math.MathContext. nil)"),
        "java.lang.NullPointerException | null String"
    );
    assert!(
        uncaught(&mut eval, "(java.math.MathContext. 10 nil)").contains("NullPointerException")
    );
    assert_eq!(
        uncaught(&mut eval, "(java.math.MathContext. 10 \"HALF_UP\")"),
        "java.lang.IllegalArgumentException | No matching ctor found for class java.math.MathContext"
    );
}

// Batch F (#205): `RoundingMode` constants read as enum values
// (equal by mode), print the JDK names; unknown names miss.
#[test]
fn rounding_fields() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(str java.math.RoundingMode/HALF_EVEN)"),
        "\"HALF_EVEN\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(= java.math.RoundingMode/HALF_UP java.math.RoundingMode/HALF_UP)"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= java.math.RoundingMode/HALF_UP java.math.RoundingMode/HALF_EVEN)"
        ),
        "false"
    );
    assert_eq!(
        compile_err(&mut eval, "java.math.RoundingMode/NOPE"),
        "java.lang.IllegalArgumentException | Error - no matches found for static method NOPE in class java.math.RoundingMode"
    );
}
