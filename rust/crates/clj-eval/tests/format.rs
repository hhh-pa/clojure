// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D `format`: corpus 12/13/14 transcription (in-reach rows) plus
//! the `s5d-fmt*` probe batches.
//!
//! Skipped as out of reach (documented divergences, not tested):
//! ratio/bigint/decimal args, `BigInteger`/`Date` interop,
//! `(int …)` `%c` rows (the port has no `Integer`), astral `%s`
//! precision (UTF-16 units vs scalars), and the lone-surrogate
//! `%c` (unrepresentable).

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

/// The uncaught `Class | message` (corpus `THROW` lines print the
/// message without quotes; `pr` here adds them — strip).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{}: {}",
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
                            "{}: {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values");
        }
    }
}

fn unquote(s: &str) -> String {
    // `pr` of a string: strip the surrounding quotes.
    let inner = &s[1..s.len() - 1];
    inner.replace("\\n", "\n").replace("\\\"", "\"")
}

fn fmt(eval: &mut Evaluator, src: &str) -> String {
    unquote(&run_last(eval, src))
}

#[test]
fn format_s_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%s\" \"a\")", "a"),
        ("(format \"%s\" nil)", "null"),
        ("(format \"%s\" :kw)", ":kw"),
        ("(format \"%s\" 5)", "5"),
        ("(format \"%.2s\" \"abcdef\")", "ab"),
        ("(format \"%s\" [1 2])", "[1 2]"),
        ("(format \"%.2s\" 12345)", "12"),
        ("(format \"%.2s\" nil)", "nu"),
        ("(format \"%5.2s\" \"abcdef\")", "   ab"),
        ("(format \"[%.0s]\" \"abc\")", "[]"),
        ("(format \"[%5.2s]\" \"abcdef\")", "[   ab]"),
        ("(format \"%d-%s\" 1 \"a\")", "1-a"),
        (
            "(format \"%s\" (map inc [1 2]))",
            "clojure.lang.LazySeq@402",
        ),
        ("(format \"%s\" [1 \"a\" :b])", "[1 \"a\" :b]"),
        ("(format \"%s\" {:a 1})", "{:a 1}"),
        ("(format \"%s\" \\a)", "a"),
        ("(format \"%s\" -0.0)", "-0.0"),
        ("(format \"%S\" \"ab\")", "AB"),
        ("(format \"%5s\" \"ab\")", "   ab"),
        ("(format \"%-5s|\" \"ab\")", "ab   |"),
        ("(format \"%s\" (format \"%d\" 5))", "5"),
        ("(format \"\")", ""),
        ("(format \"hello\")", "hello"),
        ("(format \"%d-%s-%x\" 10 \"a\" 255)", "10-a-ff"),
        ("(format \"%d\" 1 2 3)", "1"),
        ("(format \"%s\" 1 2)", "1"),
        ("(format \"[%s]\" \"a\" \"b\")", "[a]"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_d_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%d\" 5)", "5"),
        ("(format \"%d\" nil)", "null"),
        ("(format \"%5d\" 42)", "   42"),
        ("(format \"%05d\" 42)", "00042"),
        ("(format \"%+d\" 5)", "+5"),
        ("(format \"%,d\" 1234567)", "1,234,567"),
        ("(format \"%(d\" -5)", "(5)"),
        ("(format \"%d\" -5)", "-5"),
        ("(format \"%5d\" -42)", "  -42"),
        ("(format \"%05d\" -42)", "-0042"),
        ("(format \"%+d\" -5)", "-5"),
        ("(format \"% d\" 5)", " 5"),
        ("(format \"% d\" -5)", "-5"),
        ("(format \"%(d\" 5)", "5"),
        ("(format \"%,d\" -1234567)", "-1,234,567"),
        ("(format \"%d\" 0)", "0"),
        ("(format \"[%05d]\" -42)", "[-0042]"),
        ("(format \"[%(05d]\" -42)", "[(042)]"),
        ("(format \"[%+05d]\" 42)", "[+0042]"),
        ("(format \"%-5d|\" 42)", "42   |"),
        ("(format \"%(d\" -42)", "(42)"),
        ("(format \"%6d\" -42)", "   -42"),
        ("(format \"%d\" 9223372036854775807)", "9223372036854775807"),
        ("(format \"%,d\" 5)", "5"),
        ("(format \"% ,d\" 5000)", " 5,000"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
    // `%100d` (99 spaces + `5`).
    let wide = fmt(&mut eval, "(format \"%100d\" 5)");
    assert_eq!(wide.len(), 100);
    assert!(wide.ends_with('5'));
}

#[test]
fn format_float_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%.2f\" 3.14159)", "3.14"),
        ("(format \"%f\" 3.14159)", "3.141590"),
        ("(format \"%e\" 12345.678)", "1.234568e+04"),
        ("(format \"%g\" 12345.678)", "12345.7"),
        ("(format \"%f\" -0.5)", "-0.500000"),
        ("(format \"%E\" 12345.678)", "1.234568E+04"),
        ("(format \"%g\" 0.0000123456)", "1.23456e-05"),
        ("(format \"%g\" 123456789.0)", "1.23457e+08"),
        ("(format \"%f\" nil)", "null"),
        ("(format \"%f\" 0.0)", "0.000000"),
        ("(format \"%e\" 0.0)", "0.000000e+00"),
        ("(format \"%g\" 0.0)", "0.00000"),
        ("(format \"%#.0f\" 5.0)", "5."),
        ("(format \"%#.0e\" 5.0)", "5.e+00"),
        ("(format \"%,f\" 1234567.891)", "1,234,567.891000"),
        ("(format \"%(f\" -1.5)", "(1.500000)"),
        ("(format \"%010f\" 3.14)", "003.140000"),
        ("(format \"%.0g\" 123.456)", "1e+02"),
        ("(format \"%.1g\" 123.456)", "1e+02"),
        ("(format \"%.10g\" 123.456)", "123.4560000"),
        ("(format \"%e\" 0.000123)", "1.230000e-04"),
        ("(format \"%e\" 1234567.0)", "1.234567e+06"),
        // Half-up on the decimal string (not binary): `2.675`
        // is `2.67499…` in binary but formats `2.68`.
        ("(format \"%.2f\" 2.675)", "2.68"),
        ("(format \"%.0f\" 2.5)", "3"),
        ("(format \"%.0f\" 3.5)", "4"),
        ("(format \"[%.0f]\" 123.456)", "[123]"),
        ("(format \"[%e]\" -123.456)", "[-1.234560e+02]"),
        ("(format \"% g\" 5.0)", " 5.00000"),
        ("(format \"% f\" 5.0)", " 5.000000"),
        ("(format \"%f\" -0.0)", "-0.000000"),
        ("(format \"%g\" -0.0)", "-0.00000"),
        ("(format \"%e\" ##NaN)", "NaN"),
        ("(format \"%+f\" ##NaN)", "NaN"),
        ("(format \"%+f\" ##Inf)", "+Infinity"),
        ("(format \"% e\" ##NaN)", "NaN"),
        ("(format \"%(f\" ##-Inf)", "(Infinity)"),
        ("(format \"%#e\" 5.0)", "5.000000e+00"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
    // `%f` of `1e300` (301 digits + `.000000`).
    let big = fmt(&mut eval, "(format \"%f\" 1e300)");
    assert_eq!(big.len(), 301 + 7);
    assert!(big.starts_with('1') && big.ends_with(".000000"));
}

#[test]
fn format_hex_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%x\" 255)", "ff"),
        ("(format \"%X\" 255)", "FF"),
        ("(format \"%o\" 8)", "10"),
        ("(format \"%x\" -1)", "ffffffffffffffff"),
        ("(format \"%X\" -1)", "FFFFFFFFFFFFFFFF"),
        ("(format \"%o\" -8)", "1777777777777777777770"),
        ("(format \"%#x\" 0)", "0x0"),
        ("(format \"%#x\" 5)", "0x5"),
        ("(format \"%#o\" 8)", "010"),
        ("(format \"%#o\" 0)", "00"),
        ("(format \"[%#08x]\" 255)", "[0x0000ff]"),
        ("(format \"[%x]\" -1)", "[ffffffffffffffff]"),
        ("(format \"[%o]\" -8)", "[1777777777777777777770]"),
        ("(format \"%x\" -9223372036854775808)", "8000000000000000"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_b_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%b\" nil)", "false"),
        ("(format \"%b\" false)", "false"),
        ("(format \"%b\" \"x\")", "true"),
        ("(format \"%b\" 0)", "true"),
        ("(format \"%b\" \"\")", "true"),
        ("(format \"%b\" \"false\")", "true"),
        ("(format \"%.3b\" true)", "tru"),
        ("(format \"[%.2b]\" true)", "[tr]"),
        ("(format \"[%.3B]\" true)", "[TRU]"),
        ("(format \"%B\" true)", "TRUE"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_h_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%h\" 1.5)", "3ff80000"),
        ("(format \"%h\" \"a\")", "61"),
        ("(format \"%h\" -1)", "0"),
        ("(format \"[%.3h]\" \"abcdef\")", "[ab1]"),
        ("(format \"%h\" nil)", "null"),
        ("(format \"%H\" \"abcdef\")", "AB199863"),
        ("(format \"%h\" true)", "4cf"),
        ("(format \"%h\" false)", "4d5"),
        ("(format \"%h\" :a)", "3c6f0c29"),
        ("(format \"%h\" [1 2])", "3e2"),
        ("(format \"%h\" {:a 1})", "3c6f0c28"),
        ("(format \"%h\" (map inc [1 2]))", "402"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_index_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%2$s %1$s\" \"a\" \"b\")", "b a"),
        ("(format \"%s %<s\" \"a\" \"b\")", "a a"),
        ("(format \"%2$s %s\" \"a\" \"b\" \"c\")", "b a"),
        ("(format \"%s %2$s %s\" \"a\" \"b\" \"c\")", "a b b"),
        ("(format \"%s %<s %s\" \"a\" \"b\" \"c\")", "a a b"),
        ("(format \"%1$-5s\" \"a\")", "a    "),
        ("(format \"%1$s\" \"a\")", "a"),
        ("(format \"%1$5s\" \"a\")", "    a"),
        ("(format \"%-5s\" \"a\")", "a    "),
        ("(format \"%s %% %<s\" \"a\" \"b\")", "a % a"),
        ("(format \"%%\")", "%"),
        ("(format \"[%5%]\" )", "[    %]"),
        ("(format \"[%-5%]\" )", "[%    ]"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_a_rows() {
    let mut eval = boot();
    let rows = [
        ("(format \"%a\" 1.5)", "0x1.8p0"),
        ("(format \"%a\" -0.0)", "-0x0.0p0"),
        ("(format \"%A\" ##NaN)", "NAN"),
        ("(format \"%a\" ##Inf)", "Infinity"),
        ("(format \"%a\" nil)", "null"),
        ("(format \"%a\" 0.0)", "0x0.0p0"),
        ("(format \"%a\" 1.0)", "0x1.0p0"),
        ("(format \"%a\" 0.1)", "0x1.999999999999ap-4"),
        ("(format \"%a\" 1e300)", "0x1.7e43c8800759cp996"),
        ("(format \"%a\" 5e-324)", "0x0.0000000000001p-1022"),
        ("(format \"[%.2a]\" 1.5)", "[0x1.80p0]"),
        ("(format \"[%.0a]\" 1.5)", "[0x1.8p0]"),
        ("(format \"[%.12a]\" 0.1)", "[0x1.99999999999ap-4]"),
        ("(format \"[[%12a]]\" 1.5)", "[[     0x1.8p0]]"),
        ("(format \"[[%-12a]]\" 1.5)", "[[0x1.8p0     ]]"),
        ("(format \"%+a\" 1.5)", "+0x1.8p0"),
        ("(format \"%#a\" 1.5)", "0x1.8p0"),
        ("(format \"%A\" 1.5)", "0X1.8P0"),
        ("(format \"[%a]\" ##NaN)", "[NaN]"),
        ("(format \"[%a]\" ##-Inf)", "[-Infinity]"),
        ("(format \"% a\" 1.5)", " 0x1.8p0"),
        ("(format \"[%.0a]\" 0.1)", "[0x1.ap-4]"),
        ("(format \"[%.1a]\" 0.1)", "[0x1.ap-4]"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_t_rows() {
    let mut eval = boot();
    run(&mut eval, "(def T 1234567890123)");
    let rows = [
        ("(format \"%tY\" 5)", "1970"),
        ("(format \"%tY\" 5)", "1970"),
        ("(format \"%tQ\" 5)", "5"),
        ("(format \"[%tY]\" 0)", "[1970]"),
        (
            "(format \"[%tY-%tm-%td %tH:%tM:%tS]\" -1 -1 -1 -1 -1 -1)",
            "[1969-12-31 23:59:59]",
        ),
        (
            "(format \"[%tY %ty %tm %td %te %tH %tk %tI %tl %tM %tS %tL %tN %tp %ts %tQ]\" T T T T T T T T T T T T T T T T)",
            "[2009 09 02 13 13 23 23 11 11 31 30 123 123000000 pm 1234567890 1234567890123]",
        ),
        (
            "(format \"[%tA %ta %tB %tb %th]\" T T T T T)",
            "[Friday Fri February Feb Feb]",
        ),
        (
            "(format \"[%tR %tT %tD %tF %tc]\" T T T T T)",
            "[23:31 23:31:30 02/13/09 2009-02-13 Fri Feb 13 23:31:30 UTC 2009]",
        ),
        ("(format \"[%tz %tZ]\" 0 0)", "[+0000 UTC]"),
        ("(format \"%TY\" 5)", "1970"),
        ("(format \"%TA\" T)", "FRIDAY"),
        ("(format \"%TB\" T)", "FEBRUARY"),
        ("(format \"%Tp\" T)", "PM"),
        ("(format \"%tY\" -1)", "1969"),
        ("(format \"%ts\" -1)", "0"),
        ("(format \"%ts\" -1500)", "-1"),
        ("(format \"%tj\" T)", "044"),
        ("(format \"%tC\" T)", "20"),
        ("(format \"[%te]\" 5)", "[1]"),
        ("(format \"[%tk]\" 5)", "[0]"),
        ("(format \"[%tl]\" 5)", "[12]"),
        ("(format \"%8tY\" 5)", "    1970"),
        ("(format \"%ty\" nil)", "null"),
    ];
    for (src, want) in rows {
        assert_eq!(fmt(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_c_rows() {
    let mut eval = boot();
    assert_eq!(fmt(&mut eval, "(format \"%c\" \\A)"), "A");
    assert_eq!(fmt(&mut eval, "(format \"%C\" \\a)"), "A");
    assert_eq!(fmt(&mut eval, "(format \"%c\" nil)"), "null");
    let rows = [
        (
            "(format \"%c\" 65)",
            "java.util.IllegalFormatConversionException: c != java.lang.Long",
        ),
        (
            "(format \"%c\" \"A\")",
            "java.util.IllegalFormatConversionException: c != java.lang.String",
        ),
        (
            "(format \"%c\" true)",
            "java.util.IllegalFormatConversionException: c != java.lang.Boolean",
        ),
        (
            "(format \"%c\" 1.5)",
            "java.util.IllegalFormatConversionException: c != java.lang.Double",
        ),
        (
            "(format \"%.3c\" \\A)",
            "java.util.IllegalFormatPrecisionException: 3",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

fn run(eval: &mut Evaluator, src: &str) {
    eval.eval_source(src).unwrap();
}

#[test]
fn format_conversion_errors() {
    let mut eval = boot();
    let rows = [
        (
            "(format \"%d\" 1.5)",
            "java.util.IllegalFormatConversionException: d != java.lang.Double",
        ),
        (
            "(format \"%d\" \"a\")",
            "java.util.IllegalFormatConversionException: d != java.lang.String",
        ),
        (
            "(format \"%f\" 5)",
            "java.util.IllegalFormatConversionException: f != java.lang.Long",
        ),
        (
            "(format \"%d\" \\a)",
            "java.util.IllegalFormatConversionException: d != java.lang.Character",
        ),
        (
            "(format \"%d\" true)",
            "java.util.IllegalFormatConversionException: d != java.lang.Boolean",
        ),
        (
            "(format \"%x\" \\a)",
            "java.util.IllegalFormatConversionException: x != java.lang.Character",
        ),
        (
            "(format \"%a\" 5)",
            "java.util.IllegalFormatConversionException: a != java.lang.Long",
        ),
        (
            "(format \"%tY\" 1.5)",
            "java.util.IllegalFormatConversionException: Y != java.lang.Double",
        ),
        (
            "(format \"%tY\" \"a\")",
            "java.util.IllegalFormatConversionException: Y != java.lang.String",
        ),
        (
            "(format \"%tY\" true)",
            "java.util.IllegalFormatConversionException: Y != java.lang.Boolean",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_mismatch_errors() {
    let mut eval = boot();
    run(&mut eval, "(def T 1234567890123)");
    let rows = [
        (
            "(format \"%#s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = #",
        ),
        (
            "(format \"%+x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = +",
        ),
        (
            "(format \"%,x\" 5000)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = ,",
        ),
        (
            "(format \"% x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags =  ",
        ),
        (
            "(format \"%(x\" -5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = (",
        ),
        (
            "(format \"%05s\" \"ab\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = 0",
        ),
        (
            "(format \"%#.0g\" 5.0)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = g, Flags = #",
        ),
        (
            "(format \"%#d\" 1)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = d, Flags = #",
        ),
        (
            "(format \"%0s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = 0",
        ),
        (
            "(format \"%,s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = ,",
        ),
        (
            "(format \"%(s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = (",
        ),
        (
            "(format \"%+#s\" \"ab\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = +",
        ),
        (
            "(format \"%+tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = +",
        ),
        (
            "(format \"%#tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = #",
        ),
        (
            "(format \"%0tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = 0",
        ),
        (
            "(format \"% tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags =  ",
        ),
        (
            "(format \"%,tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = ,",
        ),
        (
            "(format \"%,a\" 1.5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = a, Flags = ,",
        ),
        (
            "(format \"%(a\" -1.5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = a, Flags = (",
        ),
        (
            "(format \"%(a\" 1.5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = a, Flags = (",
        ),
        (
            "(format \"%+ty\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = y, Flags = +",
        ),
        (
            "(format \"%+TY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = +",
        ),
        (
            "(format \"%05tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = 0",
        ),
        (
            "(format \"%+ s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = + ",
        ),
        (
            "(format \"%+ b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = + ",
        ),
        (
            "(format \"%+ h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = + ",
        ),
        (
            "(format \"%+ c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = + ",
        ),
        (
            "(format \"%+ tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = + ",
        ),
        (
            "(format \"%,(s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = ,(",
        ),
        (
            "(format \"%#,s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = ,",
        ),
        (
            "(format \"%#,a\" 1.5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = a, Flags = ,",
        ),
        (
            "(format \"%#,tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = #,",
        ),
        (
            "(format \"%+#tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = #+",
        ),
        (
            "(format \"%#,b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = #",
        ),
        (
            "(format \"%#,h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = #",
        ),
        (
            "(format \"%#,c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = #,",
        ),
        (
            "(format \"% ,x\" 5000)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = ,",
        ),
        (
            "(format \"%,(b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = ,(",
        ),
        (
            "(format \"%,b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = ,",
        ),
        (
            "(format \"%,(h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = ,(",
        ),
        (
            "(format \"%,h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = ,",
        ),
        (
            "(format \"%+#c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = #+",
        ),
        (
            "(format \"%,(c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = ,(",
        ),
        (
            "(format \"%+,x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = ,",
        ),
        (
            "(format \"%,(x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = ,",
        ),
        (
            "(format \"%#(a\" 1.5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = a, Flags = (",
        ),
        (
            "(format \"%(#,s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = ,(",
        ),
        (
            "(format \"%(#,tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = #,(",
        ),
        (
            "(format \"%#, (b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = #",
        ),
        (
            "(format \"%+(x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = +(",
        ),
        (
            "(format \"% #c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = # ",
        ),
        (
            "(format \"% #tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = # ",
        ),
        (
            "(format \"%(#x\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = x, Flags = (",
        ),
        (
            "(format \"%(#o\" 8)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = o, Flags = (",
        ),
        (
            "(format \"%(#d\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = d, Flags = #",
        ),
        (
            "(format \"%# s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags =  ",
        ),
        (
            "(format \"%+#,s\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = s, Flags = +,",
        ),
        (
            "(format \"%0,x\" 5)",
            "java.util.MissingFormatWidthException: %0,x",
        ),
        (
            "(format \"%0#d\" 5)",
            "java.util.MissingFormatWidthException: %#0d",
        ),
        (
            "(format \"%+,o\" 8)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = o, Flags = ,",
        ),
        (
            "(format \"%-0tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = 0",
        ),
        (
            "(format \"%-0c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = 0",
        ),
        (
            "(format \"%-#b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = #",
        ),
        (
            "(format \"%-#tY\" 5)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = Y, Flags = #",
        ),
        (
            "(format \"%05b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = 0",
        ),
        (
            "(format \"%05h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = 0",
        ),
        (
            "(format \"%0b\" true)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = b, Flags = 0",
        ),
        (
            "(format \"%0h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = 0",
        ),
        (
            "(format \"%-#h\" \"a\")",
            "java.util.FormatFlagsConversionMismatchException: Conversion = h, Flags = #",
        ),
        (
            "(format \"%-#c\" \\a)",
            "java.util.FormatFlagsConversionMismatchException: Conversion = c, Flags = #",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_flag_errors() {
    let mut eval = boot();
    let rows = [
        (
            "(format \"%--s\" \"a\")",
            "java.util.DuplicateFormatFlagsException: Flags = '-'",
        ),
        (
            "(format \"%+ d\" 1)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ x\" 1)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%<<s\" \"a\" \"b\")",
            "java.util.DuplicateFormatFlagsException: Flags = '<'",
        ),
        (
            "(format \"%- -d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '-'",
        ),
        (
            "(format \"%+ +d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%-+ -d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '-'",
        ),
        (
            "(format \"%##d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '#'",
        ),
        (
            "(format \"%##s\" \"a\")",
            "java.util.DuplicateFormatFlagsException: Flags = '#'",
        ),
        (
            "(format \"%,,d\" 5000)",
            "java.util.DuplicateFormatFlagsException: Flags = ','",
        ),
        (
            "(format \"%00d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '0'",
        ),
        (
            "(format \"%((d\" -5)",
            "java.util.DuplicateFormatFlagsException: Flags = '('",
        ),
        (
            "(format \"%  s\" \"a\")",
            "java.util.DuplicateFormatFlagsException: Flags = ' '",
        ),
        (
            "(format \"%  d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = ' '",
        ),
        (
            "(format \"%++s\" \"a\")",
            "java.util.DuplicateFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%++x\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%+ +s\" \"a\")",
            "java.util.DuplicateFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%-,-d\" 5000)",
            "java.util.DuplicateFormatFlagsException: Flags = '-'",
        ),
        (
            "(format \"%++d\" 5)",
            "java.util.DuplicateFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%+ e\" 1.5)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ g\" 1.5)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ a\" 1.5)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ o\" 8)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ 5x\" 5)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"% +n\")",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%+ #d\" 5)",
            "java.util.IllegalFormatFlagsException: Flags = '#+ '",
        ),
        (
            "(format \"%-+ n\")",
            "java.util.IllegalFormatFlagsException: Flags = '-+ '",
        ),
        (
            "(format \"%+ 5.2d\" 5)",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"%-0s\" \"ab\")",
            "java.util.MissingFormatWidthException: %-0s",
        ),
        (
            "(format \"%1$-s\" \"a\")",
            "java.util.MissingFormatWidthException: %-1$s",
        ),
        (
            "(format \"%-s\" \"a\")",
            "java.util.MissingFormatWidthException: %-s",
        ),
        (
            "(format \"%0d\" 5)",
            "java.util.MissingFormatWidthException: %0d",
        ),
        (
            "(format \"%-tY\" 5)",
            "java.util.MissingFormatWidthException: %-tY",
        ),
        (
            "(format \"%0a\" 1.5)",
            "java.util.MissingFormatWidthException: %0a",
        ),
        (
            "(format \"%-0.2d\" 5)",
            "java.util.MissingFormatWidthException: %-0.2d",
        ),
        (
            "(format \"%-0.2s\" \"ab\")",
            "java.util.MissingFormatWidthException: %-0.2s",
        ),
        (
            "(format \"%+ -d\" 5)",
            "java.util.MissingFormatWidthException: %-+ d",
        ),
        (
            "(format \"%+-d\" 5)",
            "java.util.MissingFormatWidthException: %-+d",
        ),
        (
            "(format \"%-0b\" true)",
            "java.util.MissingFormatWidthException: %-0b",
        ),
        (
            "(format \"%-0h\" \"a\")",
            "java.util.MissingFormatWidthException: %-0h",
        ),
        (
            "(format \"%-#s\" \"a\")",
            "java.util.MissingFormatWidthException: %-#s",
        ),
        (
            "(format \"%- s\" \"a\")",
            "java.util.MissingFormatWidthException: %- s",
        ),
        (
            "(format \"%-s\")",
            "java.util.MissingFormatWidthException: %-s",
        ),
        (
            "(format \"%0d\")",
            "java.util.MissingFormatWidthException: %0d",
        ),
        (
            "(format \"%.2d\" 5)",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"%.3d\" 5)",
            "java.util.IllegalFormatPrecisionException: 3",
        ),
        (
            "(format \"%.2tY\" 5)",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"%.2n\")",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"%.0n\")",
            "java.util.IllegalFormatPrecisionException: 0",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_parse_errors() {
    let mut eval = boot();
    let rows = [
        (
            "(format \"%5.3\" 1)",
            "java.util.UnknownFormatConversionException: Conversion = '5'",
        ),
        (
            "(format \"%_s\" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = '_'",
        ),
        (
            "(format \"%q\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = 'q'",
        ),
        (
            "(format \"%*s\" \"ab\")",
            "java.util.UnknownFormatConversionException: Conversion = '*'",
        ),
        (
            "(format \"100%\")",
            "java.util.UnknownFormatConversionException: Conversion = '%'",
        ),
        (
            "(format \"%Q\" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = 'Q'",
        ),
        (
            "(format \"%5\")",
            "java.util.UnknownFormatConversionException: Conversion = '5'",
        ),
        (
            "(format \"%-\")",
            "java.util.UnknownFormatConversionException: Conversion = '-'",
        ),
        (
            "(format \"%.\")",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"%.2\")",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"%<\")",
            "java.util.UnknownFormatConversionException: Conversion = '<'",
        ),
        (
            "(format \"%2$\")",
            "java.util.UnknownFormatConversionException: Conversion = '2'",
        ),
        (
            "(format \"%2\" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = '2'",
        ),
        (
            "(format \"%\")",
            "java.util.UnknownFormatConversionException: Conversion = '%'",
        ),
        (
            "(format \"%5.\" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = '5'",
        ),
        (
            "(format \"% \" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = ' '",
        ),
        (
            "(format \"%.-2f\" 1.5)",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"%tV\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = 'tV'",
        ),
        (
            "(format \"%tq\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = 'tq'",
        ),
        (
            "(format \"x%t\")",
            "java.util.UnknownFormatConversionException: Conversion = 't'",
        ),
        (
            "(format \"%tO\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = 'tO'",
        ),
        (
            "(format \"%tE\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = 'tE'",
        ),
        (
            "(format \"[%.d]\" 5)",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"[%.s]\" \"ab\")",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"%99999999999999999999d\" 1)",
            "java.util.IllegalFormatWidthException: -2147483648",
        ),
        (
            "(format \"%2147483648d\" 1)",
            "java.util.IllegalFormatWidthException: -2147483648",
        ),
        (
            "(format \"%.99999999999999999999f\" 1.5)",
            "java.util.IllegalFormatPrecisionException: -2147483648",
        ),
        (
            "(format \"%0$s\" \"a\")",
            "java.util.IllegalFormatArgumentIndexException: Illegal format argument index = 0",
        ),
        (
            "(format \"%99999999999999999999$s\" \"a\")",
            "java.util.IllegalFormatArgumentIndexException: Format argument index: (not representable as int)",
        ),
        (
            "(format \"%<s\" \"a\")",
            "java.util.MissingFormatArgumentException: Format specifier '%<s'",
        ),
        (
            "(format \"%s %s\" 1)",
            "java.util.MissingFormatArgumentException: Format specifier '%s'",
        ),
        (
            "(format \"%d\")",
            "java.util.MissingFormatArgumentException: Format specifier '%d'",
        ),
        (
            "(format \"%2147483647$s\" \"a\" \"b\")",
            "java.util.MissingFormatArgumentException: Format specifier '%2147483647$s'",
        ),
        (
            "(format \"%8$s\" \"a\")",
            "java.util.MissingFormatArgumentException: Format specifier '%8$s'",
        ),
        (
            "(format \"%-1$s\" \"a\")",
            "java.util.UnknownFormatConversionException: Conversion = '-'",
        ),
        (
            "(format \"%.2+s\" \"ab\")",
            "java.util.UnknownFormatConversionException: Conversion = '.'",
        ),
        (
            "(format \"%% %<s\" \"a\")",
            "java.util.MissingFormatArgumentException: Format specifier '%<s'",
        ),
        (
            "(format)",
            "clojure.lang.ArityException: Wrong number of args (0) passed to: clojure.core/format",
        ),
        (
            "(format nil \"a\")",
            "java.lang.NullPointerException: Cannot invoke \"String.length()\" because \"s\" is null",
        ),
        (
            "(format nil)",
            "java.lang.NullPointerException: Cannot invoke \"String.length()\" because \"s\" is null",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

#[test]
fn format_n_and_pct_errors() {
    let mut eval = boot();
    assert_eq!(fmt(&mut eval, "(format \"%n\")"), "\n");
    let rows = [
        (
            "(format \"%5n\")",
            "java.util.IllegalFormatWidthException: 5",
        ),
        (
            "(format \"%-n\")",
            "java.util.IllegalFormatFlagsException: Flags = '-'",
        ),
        (
            "(format \"%-5n\")",
            "java.util.IllegalFormatWidthException: 5",
        ),
        (
            "(format \"%-5.2n\")",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"%5.2n\")",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"%+n\")",
            "java.util.IllegalFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"%,n\")",
            "java.util.IllegalFormatFlagsException: Flags = ','",
        ),
        (
            "(format \"%#n\")",
            "java.util.IllegalFormatFlagsException: Flags = '#'",
        ),
        (
            "(format \"%-#n\")",
            "java.util.IllegalFormatFlagsException: Flags = '-#'",
        ),
        (
            "(format \"%#,n\")",
            "java.util.IllegalFormatFlagsException: Flags = '#,'",
        ),
        (
            "(format \"%- ,n\")",
            "java.util.IllegalFormatFlagsException: Flags = '- ,'",
        ),
        (
            "(format \"%,-n\")",
            "java.util.IllegalFormatFlagsException: Flags = '-,'",
        ),
        (
            "(format \"[%05%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '0'",
        ),
        (
            "(format \"[%.2%]\" )",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"[%-%]\" )",
            "java.util.MissingFormatWidthException: %-%",
        ),
        (
            "(format \"[%+%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '+'",
        ),
        (
            "(format \"[%,%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = ','",
        ),
        (
            "(format \"[%#%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '#'",
        ),
        (
            "(format \"[% %]\" )",
            "java.util.IllegalFormatFlagsException: Flags = ' '",
        ),
        (
            "(format \"[%(%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '('",
        ),
        (
            "(format \"[%05.2%]\" )",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"[%5.2%]\" )",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"[%-#%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '-#'",
        ),
        (
            "(format \"[%,0%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '0,'",
        ),
        (
            "(format \"[%,(#%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '#,('",
        ),
        (
            "(format \"[%-.2%]\" )",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"[%+#%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '#+'",
        ),
        (
            "(format \"[%0-%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '-0'",
        ),
        (
            "(format \"[%+ %]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '+ '",
        ),
        (
            "(format \"[%+ 5.2%]\" )",
            "java.util.IllegalFormatPrecisionException: 2",
        ),
        (
            "(format \"[%+- %]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '-+ '",
        ),
        (
            "(format \"[%+# %]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '#+ '",
        ),
        (
            "(format \"[%+-%]\" )",
            "java.util.IllegalFormatFlagsException: Flags = '-+'",
        ),
    ];
    for (src, want) in rows {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}
