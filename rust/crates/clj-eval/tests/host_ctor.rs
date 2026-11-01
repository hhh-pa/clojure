// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #120 slice 2: emulated construction — `(new Class …)`,
//! `(Class. …)`, `(Class/new …)` for `String`/`Object` over the
//! emulated lattice, with JVM-text misses elsewhere. Every pin
//! cites its oracle label (`docs/gap-120-slice2/probes/ctor*.out`;
//! `java -Duser.language=en -Duser.country=US -Duser.timezone=UTC
//! -Dfile.encoding=UTF-8 -cp clojure-1.13.0-alpha9.jar:…
//! clojure.main`). Narrowings N1–N9: see
//! `docs/gap-120-slice2/spec.md`.

use clj_eval::{Evaluator, SourceError, boot};
use clj_reader::{NoHooks, ReaderOpts, read_str};
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

/// Evaluate one form in a fresh `eval_source` call (reads see the
/// imports/defs the previous calls committed).
fn run_step(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src)
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
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// A compile error's `Class | message` (strict: uncaught throws
/// fail — phase pins must not silently pass).
fn compile_err(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(vs) => panic!("expected a compile error, got values: {vs:?}"),
    }
}

/// The full compile chain (wrapper + cause) for an analyzed form.
fn chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let form = read_str(src, &ReaderOpts::default(), &mut NoHooks).unwrap();
    match eval.analyze(&form) {
        Ok(_) => panic!("expected an error for {src}"),
        Err(e) => e.chain(),
    }
}

fn no_ctor(class: &str) -> String {
    format!("java.lang.IllegalArgumentException | No matching ctor found for class {class}")
}

fn no_ctor_iface(iface: &str) -> String {
    format!("java.lang.IllegalArgumentException | No matching ctor found for interface {iface}")
}

const AMBIGUITY: &str =
    "java.lang.IllegalArgumentException | More than one matching method found: java.lang.String";
const BYTES_NPE: &str = "java.lang.NullPointerException | Cannot invoke \"java.lang.String.<init>(byte[])\" because \"bytes\" is null";

// ---- String success shapes ----

// ctor-str-0/1/class/eq/fq, new-str-0/1/fq, new-str-chararr:
// every spelling of the trivial overloads.
#[test]
fn string_basics() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(String.)"), "\"\"");
    assert_eq!(run(&mut eval, "(String. \"x\")"), "\"x\"");
    assert_eq!(
        run(&mut eval, "(class (String. \"x\"))"),
        "java.lang.String"
    );
    assert_eq!(run(&mut eval, "(= String (class (String. \"x\")))"), "true");
    assert_eq!(run(&mut eval, "(java.lang.String. \"x\")"), "\"x\"");
    assert_eq!(run(&mut eval, "(new String \"x\")"), "\"x\"");
    assert_eq!(run(&mut eval, "(new String)"), "\"\"");
    assert_eq!(run(&mut eval, "(new java.lang.String \"x\")"), "\"x\"");
    assert_eq!(run(&mut eval, "(new String (char-array [\\a]))"), "\"a\"");
    assert_eq!(run(&mut eval, "(String/new \"x\")"), "\"x\"");
    assert_eq!(run(&mut eval, "(String/new)"), "\"\"");
    assert_eq!(run(&mut eval, "(java.lang.String/new \"x\")"), "\"x\"");
}

// ctor-str-chararr(-off/-empty), ctor-str-bytearr(-off),
// ctor-str-hibyte, ctor-str-intarr, ctor-str-coerce-long: the
// array overloads over port arrays.
#[test]
fn string_arrays() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(String. (char-array [\\a \\b]))"), "\"ab\"");
    assert_eq!(
        run(&mut eval, "(String. (char-array [\\a \\b \\c]) 1 2)"),
        "\"bc\""
    );
    assert_eq!(run(&mut eval, "(String. (char-array 0))"), "\"\"");
    assert_eq!(run(&mut eval, "(String. (byte-array [65 66]))"), "\"AB\"");
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65 66 67]) 1 2)"),
        "\"BC\""
    );
    assert_eq!(run(&mut eval, "(String. (byte-array [65 66]) 0)"), "\"AB\"");
    assert_eq!(
        run(&mut eval, "(String. (int-array [65 66]) 0 2)"),
        "\"AB\""
    );
    assert_eq!(
        run(&mut eval, "(String. (char-array [\\a \\b \\c]) 1 (long 2))"),
        "\"bc\""
    );
    // Hibyte bit semantics (Java int wrap, then narrow).
    assert_eq!(run(&mut eval, "(String. (byte-array [65 66]) 2)"), "\"Ɂɂ\"");
    assert_eq!(run(&mut eval, "(String. (byte-array [65]) -1)"), "\"ａ\"");
    assert_eq!(run(&mut eval, "(String. (byte-array [-1]) 0)"), "\"ÿ\"");
}

// ctor-str-bytearr-bad/utf8/ascii/latin/utf16/utf16be, cs-*,
// u32-*, s-*, bom-*, a-*: named charsets — success rows,
// REPLACE semantics, BOM handling, and the probed alias set.
#[test]
fn string_charsets() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(String. (byte-array [-1]))"), "\"�\"");
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-61 -87]) \"UTF-8\")"),
        "\"é\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65]) \"US-ASCII\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-1]) \"ISO-8859-1\")"),
        "\"ÿ\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-2 -1 65 0]) \"UTF-16\")"),
        "\"䄀\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [0 65]) \"UTF-16BE\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65 66 67]) 0 2 \"UTF-8\")"),
        "\"AB\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65 0]) \"UTF-16LE\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-1 -2 65 0]) \"UTF-16\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-1]) \"US-ASCII\")"),
        "\"�\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65]) \"UTF-16BE\")"),
        "\"�\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array []) \"UTF-8\")"),
        "\"\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65]) \"utf8\")"),
        "\"A\""
    );
    // BOM-only decodes to empty, either endianness.
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-1 -2]) \"UTF-16\")"),
        "\"\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [-2 -1]) \"UTF-16\")"),
        "\"\""
    );
    // The 4-arg offset path with a fixed charset (odd tail →
    // REPLACE).
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65 66]) 1 1 \"UTF-16BE\")"),
        "\"�\""
    );
    // UTF-32 incl. both BOMs, truncation, and bad code points.
    assert_eq!(
        run(&mut eval, "(String. (byte-array [0 0 0 65]) \"UTF-32\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [0 0 0 65]) \"UTF-32BE\")"),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65 0 0 0]) \"UTF-32LE\")"),
        "\"A\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(String. (byte-array [0 0 -2 -1 0 0 0 65]) \"UTF-32\")"
        ),
        "\"A\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(String. (byte-array [-1 -2 0 0 65 0 0 0]) \"UTF-32\")"
        ),
        "\"A\""
    );
    assert_eq!(
        run(&mut eval, "(String. (byte-array [65]) \"UTF-32BE\")"),
        "\"�\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(String. (byte-array [-1 -1 -1 -1]) \"UTF-32BE\")"
        ),
        "\"�\""
    );
    // Probed aliases, incl. case-insensitivity.
    for (src, want) in [
        ("(String. (byte-array [65]) \"ASCII\")", "\"A\""),
        ("(String. (byte-array [-1]) \"Latin1\")", "\"ÿ\""),
        ("(String. (byte-array [-1]) \"ISO8859-1\")", "\"ÿ\""),
        ("(String. (byte-array [0 65]) \"UTF16\")", "\"A\""),
        ("(String. (byte-array [0 65]) \"Unicode\")", "\"A\""),
        ("(String. (byte-array [0 65]) \"UnicodeBig\")", "\"A\""),
        ("(String. (byte-array [65 0]) \"UnicodeLittle\")", "\"A\""),
        ("(String. (byte-array [0 65]) \"utf-16\")", "\"A\""),
        ("(String. (byte-array [0 65]) \"utf-16be\")", "\"A\""),
        ("(String. (byte-array [65]) \"Utf-8\")", "\"A\""),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

// ctor-str-ident, fr-e/empty/chararr: constructions are fresh
// objects, never the argument (or literal) `Arc`.
#[test]
fn string_fresh() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(identical? (String. \"x\") \"x\")"),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(identical? (String. \"é\") \"é\")"),
        "false"
    );
    assert_eq!(run(&mut eval, "(identical? (String.) \"\")"), "false");
    assert_eq!(
        run(&mut eval, "(identical? (String. (char-array [])) \"\")"),
        "false"
    );
}

// ---- String failure shapes ----

// ctor-str-arity2/4/badtype, ctor-str-coerce-double/str,
// new-str-bad/arity, s-new-str-bad, cn-badtype, h-local-new,
// d-str-nil/nil-char/3nil/nil4, n-strs/chars-int/intarr-1/longarr:
// overload misses within an existing arity fail at runtime,
// uncaught (the JVM compiles these and fails in the Reflector).
#[test]
fn string_overload_miss() {
    let mut eval = boot();
    let want = no_ctor("java.lang.String");
    for src in [
        "(String. \"a\" \"b\")",
        "(String. \"a\" \"b\" \"c\" \"d\")",
        "(String. 1)",
        "(String. (char-array [\\a \\b \\c]) 1.0 2)",
        "(String. (char-array [\\a \\b \\c]) \"1\" 2)",
        "(new String 1)",
        "(new String \"a\" \"b\")",
        "(String/new 1)",
        "(let [String 5] (new String \"x\" :bogus :bogus2 :bogus3))",
        "(String. \"a\" nil)",
        "(String. nil \\a)",
        "(String. nil nil nil)",
        "(String. nil nil nil nil)",
        "(String. \"a\" \"b\" :k)",
        "(String. (char-array [\\a]) 0)",
        "(String. (int-array [65]))",
        "(String. (long-array [65]) 0 1)",
        "(String. (char-array [\\a]) nil 1)",
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
}

// ctor-str-arity5, s-new-arity5, cn-arity5: arities no ctor takes
// fail at compile time — attributed to `new` for the `new`/dot
// spellings, to the head as written for `Class/new`.
#[test]
fn string_arity_compile() {
    let mut eval = boot();
    let cause = no_ctor("java.lang.String");
    assert_eq!(
        chain(&mut eval, "(String. 1 2 3 4 5)"),
        vec![
            "Syntax error compiling new at (user:1:1).".to_owned(),
            cause.clone(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(new String \"a\" \"b\" \"c\" \"d\" \"e\")"),
        vec![
            "Syntax error compiling new at (user:1:1).".to_owned(),
            cause.clone(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(String/new 1 2 3 4 5)"),
        vec![
            "Syntax error compiling String/new at (user:1:1).".to_owned(),
            cause.clone(),
        ]
    );
}

// ctor-str-nil1/2/3, ctor-str-bytearr-nil, d-bytes-nil,
// d-nil-0-0-nil, cs-nil-name, cn-nil1, s-new-nil: nil overload
// ambiguity reports the JVM text — at runtime (narrowing N2;
// the JVM rejects these at compile time).
#[test]
fn string_ambiguity() {
    let mut eval = boot();
    for src in [
        "(String. nil)",
        "(String. nil nil)",
        "(String. nil 0 0)",
        "(String. (byte-array [65]) nil)",
        "(String. nil 0 0 nil)",
        "(String/new nil)",
        "(new String nil)",
    ] {
        assert_eq!(uncaught(&mut eval, src), AMBIGUITY, "{src}");
    }
}

// d-nil-utf8, d-nil-0, cs-nil-bytes, e-nil-badbounds: nil bytes
// with a uniquely-selected overload NPE (port-pinned text — the
// JDK message is an internal deref and varies by JDK).
#[test]
fn string_nil_npe() {
    let mut eval = boot();
    for src in [
        "(String. nil \"UTF-8\")",
        "(String. nil 0)",
        "(String. nil 5 1 \"UTF-8\")",
    ] {
        assert_eq!(uncaught(&mut eval, src), BYTES_NPE, "{src}");
    }
}

// c-bigint/bigdec/half/char/bool: only `Long` matches an `int`
// param; ctor-str-coerce-oobig, c-hibyte-big: out-of-`i32`
// `Long`s fail `RT.intCast` (`ArithmeticException | integer
// overflow`).
#[test]
fn string_int_coerce() {
    let mut eval = boot();
    let want = no_ctor("java.lang.String");
    for src in [
        "(String. (char-array [\\a \\b \\c]) (bigint 1) (bigint 2))",
        "(String. (char-array [\\a \\b \\c]) (bigint 1) (bigint 3000000000))",
        "(String. (char-array [\\a \\b \\c]) (bigdec 1) 2)",
        "(String. (char-array [\\a \\b \\c]) 1/2 1)",
        "(String. (char-array [\\a \\b \\c]) \\a 1)",
        "(String. (char-array [\\a \\b \\c]) true 1)",
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
    for src in [
        "(String. (char-array [\\a \\b \\c]) 1 3000000000)",
        "(String. (byte-array [65 66]) 3000000000)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ArithmeticException | integer overflow",
            "{src}"
        );
    }
}

// ctor-str-off-oob/neg, ctor-str-cnt-oob, cs-off-oob: the JDK
// `checkFromIndexSize` template, expression unreduced.
#[test]
fn string_bounds() {
    let mut eval = boot();
    for (src, want) in [
        (
            "(String. (char-array [\\a]) 5 1)",
            "Range [5, 5 + 1) out of bounds for length 1",
        ),
        (
            "(String. (char-array [\\a]) -1 1)",
            "Range [-1, -1 + 1) out of bounds for length 1",
        ),
        (
            "(String. (char-array [\\a]) 0 2)",
            "Range [0, 0 + 2) out of bounds for length 1",
        ),
        (
            "(String. (byte-array [65]) 5 1 \"UTF-8\")",
            "Range [5, 5 + 1) out of bounds for length 1",
        ),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!("java.lang.StringIndexOutOfBoundsException | {want}"),
            "{src}"
        );
    }
}

// ctor-str-badcs/4, v-empty/badchars/colon, s-spcs: every charset
// lookup failure is `UnsupportedEncodingException` carrying the
// name verbatim; e-badboth, e-nil-badcs: the lookup precedes
// bounds/null checks.
#[test]
fn string_charset_errors() {
    let mut eval = boot();
    for (src, name) in [
        ("(String. (byte-array [65]) \"NOPE-CS\")", "NOPE-CS"),
        ("(String. (byte-array [65]) 0 1 \"NOPE-CS\")", "NOPE-CS"),
        ("(String. (byte-array [65]) \"\")", ""),
        ("(String. (byte-array [65]) \"bad name!\")", "bad name!"),
        ("(String. (byte-array [65]) \"x:y\")", "x:y"),
        ("(String. (byte-array [65]) \" UTF-8\")", " UTF-8"),
        ("(String. (byte-array [65]) 5 1 \"NOPE-CS\")", "NOPE-CS"),
        ("(String. nil 0 1 \"NOPE-CS\")", "NOPE-CS"),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!("java.io.UnsupportedEncodingException | {name}"),
            "{src}"
        );
    }
    // The long-tail narrowing N6: valid JDK names outside the
    // probed set throw UEE like unknown names (the JVM decodes;
    // oracle `tail-cp1252`).
    assert_eq!(
        uncaught(&mut eval, "(String. (byte-array [65]) \"windows-1252\")"),
        "java.io.UnsupportedEncodingException | windows-1252"
    );
    // UEE catches as `IOException`/`Exception`, never as
    // `RuntimeException`/`Error`.
    assert_eq!(
        run(
            &mut eval,
            "(try (String. (byte-array [65]) \"NOPE\") (catch java.io.IOException e :io))"
        ),
        ":io"
    );
    assert_eq!(
        run(
            &mut eval,
            "(try (String. (byte-array [65]) \"NOPE\") (catch Exception e :ex))"
        ),
        ":ex"
    );
}

// ctor-str-badcp, cp-toobig: out-of-range code points are `IAE |
// <int>`; cp-surrogate: unpaired surrogates become U+FFFD (the
// JVM embeds them — oracle `sg-hex` — unrepresentable in Rust;
// spec N5); cp-max/astral: the top and an astral plane pin.
#[test]
fn string_codepoints() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(String. (int-array [-1]) 0 1)"),
        "java.lang.IllegalArgumentException | -1"
    );
    assert_eq!(
        uncaught(&mut eval, "(String. (int-array [1114112]) 0 1)"),
        "java.lang.IllegalArgumentException | 1114112"
    );
    assert_eq!(run(&mut eval, "(String. (int-array [55296]) 0 1)"), "\"�\"");
    assert_eq!(
        run(&mut eval, "(= (String. (int-array [55296]) 0 1) \"?\")"),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(String. (int-array [1114111]) 0 1)"),
        "\"􏿿\""
    );
    assert_eq!(
        run(&mut eval, "(String. (int-array [128512]) 0 1)"),
        "\"😀\""
    );
}

// ---- Object ----

// ctor-obj-class/eq/ident, new-obj-fq, cn-obj: 0-arg construction
// across spellings; every object is distinct.
#[test]
fn object_ctor() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(class (Object.))"), "java.lang.Object");
    assert_eq!(run(&mut eval, "(= Object (class (Object.)))"), "true");
    assert_eq!(run(&mut eval, "(identical? (Object.) (Object.))"), "false");
    assert_eq!(run(&mut eval, "(= (Object.) (Object.))"), "false");
    assert_eq!(
        run(&mut eval, "(class (new java.lang.Object))"),
        "java.lang.Object"
    );
    assert_eq!(run(&mut eval, "(class (Object/new))"), "java.lang.Object");
}

// ctor-obj-arity/nil, new-obj-arity, m-new-obj-nil,
// m-obj-new-arity: nonzero arities fail at compile time (Object
// takes no args), attributed to the head as written.
#[test]
fn object_arity() {
    let mut eval = boot();
    let cause = no_ctor("java.lang.Object");
    for src in [
        "(Object. 1)",
        "(Object. nil)",
        "(new Object 1)",
        "(new Object nil)",
    ] {
        assert_eq!(
            chain(&mut eval, src),
            vec![
                "Syntax error compiling new at (user:1:1).".to_owned(),
                cause.clone(),
            ],
            "{src}"
        );
    }
    assert_eq!(
        chain(&mut eval, "(Object/new 1)"),
        vec![
            "Syntax error compiling Object/new at (user:1:1).".to_owned(),
            cause.clone(),
        ]
    );
}

// ---- non-allow-listed classes (narrowing N4) ----

// ctor-iface/abstract/private/private-new, cn-iface: real
// interfaces read `for interface`, abstract/private classes `for
// class` — at runtime (the JVM rejects these at compile time).
// (`Writer` stands in for the oracle's `Format` — same abstract
// `for class` shape, and present in the emulated lattice.)
// ctor-int/int-str/long-str/bool-str/hashmap/arraylist/cn-int:
// the JVM constructs these; the port takes the same uniform
// miss (the allow-list has exact values only).
#[test]
fn allowlist_miss() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(java.util.List.)"),
        no_ctor_iface("java.util.List")
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.List/new)"),
        no_ctor_iface("java.util.List")
    );
    for src in [
        "(java.io.Writer.)",
        "(System.)",
        "(new System)",
        "(java.util.Date. 0)",
        "(Integer. 5)",
        "(Integer. \"42\")",
        "(Long. \"42\")",
        "(Boolean. \"true\")",
        "(Integer/new 5)",
    ] {
        let class = if src.contains("Writer") {
            "java.io.Writer"
        } else if src.contains("System") {
            "java.lang.System"
        } else if src.contains("Date") {
            "java.util.Date"
        } else if src.contains("Integer") {
            "java.lang.Integer"
        } else if src.contains("Long") {
            "java.lang.Long"
        } else {
            "java.lang.Boolean"
        };
        assert_eq!(uncaught(&mut eval, src), no_ctor(class), "{src}");
    }
    // Names outside the emulated lattice CNFE — exactly like
    // `import` of them does (lattice expansion is the future
    // "import of real classes" slice, not this one).
    for (src, name) in [
        ("(java.util.HashMap.)", "java.util.HashMap"),
        ("(java.util.HashMap. 4 0.5)", "java.util.HashMap"),
        ("(java.util.ArrayList.)", "java.util.ArrayList"),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!("java.lang.ClassNotFoundException | {name}"),
            "{src}"
        );
    }
}

// ---- missing classes (narrowing N1) ----

// ctor-missing-simple/fq/fq0, new-missing-simple/fq,
// m-new-qual/qual-str: unresolvable names report the JVM text at
// runtime (the JVM rejects them at compile time — forced by
// same-form define+use, oracles `o-do-*`). val-traildot(-fq):
// value-position `String.` keeps the compile-time CNFE (exact).
#[test]
fn missing_class() {
    let mut eval = boot();
    for src in ["(NopeKlass. 1)", "(new NopeKlass)"] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | Unable to resolve classname: NopeKlass",
            "{src}"
        );
    }
    for src in [
        "(java.no.Klass. 1)",
        "(java.no.Klass.)",
        "(new java.no.Klass)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassNotFoundException | java.no.Klass",
            "{src}"
        );
    }
    for (src, sym) in [
        ("(new foo/bar 1)", "foo/bar"),
        ("(new foo/String 1)", "foo/String"),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!("java.lang.IllegalArgumentException | Unable to resolve classname: {sym}"),
            "{src}"
        );
    }
    assert_eq!(
        compile_err(&mut eval, "String."),
        "java.lang.ClassNotFoundException | String."
    );
    assert_eq!(
        compile_err(&mut eval, "java.lang.String."),
        "java.lang.ClassNotFoundException | java.lang.String."
    );
}

// ---- `new` head shapes ----

// new-empty, new-num/str/nil/list/kw-head, nl-new-numclass:
// head-shape errors are compile errors attributed to `new`.
// new-shadow, nl-new-shadow-str, ctor-dot-shadow,
// nl-ctor-shadow-new, cn-shadow-new, h-local-static: `new` and
// `Class/new` ignore locals.
#[test]
fn new_shapes() {
    let mut eval = boot();
    assert_eq!(
        chain(&mut eval, "(new)"),
        vec![
            "Syntax error compiling new at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | wrong number of arguments, expecting: (new Classname args...)"
                .to_owned(),
        ]
    );
    for (src, head) in [
        ("(new 1 \"x\")", "1"),
        ("(new \"String\" \"x\")", "String"),
        ("(new nil \"x\")", "null"),
        (
            "(new (Class/forName \"java.lang.String\") \"x\")",
            "(Class/forName \"java.lang.String\")",
        ),
        ("(new :String \"x\")", ":String"),
        ("(new 1 2 3)", "1"),
        // Port-derived (no oracle row): `String.valueOf` of a
        // `Character` is the char itself (JDK-spec certain).
        ("(new \\a 1)", "a"),
    ] {
        assert_eq!(
            chain(&mut eval, src),
            vec![
                "Syntax error compiling new at (user:1:1).".to_owned(),
                format!("java.lang.IllegalArgumentException | Unable to resolve classname: {head}"),
            ],
            "{src}"
        );
    }
    assert_eq!(run(&mut eval, "(let [new 5] (new String \"x\"))"), "\"x\"");
    assert_eq!(
        run(&mut eval, "(let [String 5] (new String \"x\"))"),
        "\"x\""
    );
    assert_eq!(run(&mut eval, "(let [String 5] (String. \"x\"))"), "\"x\"");
    assert_eq!(run(&mut eval, "(let [new 5] (String. \"x\"))"), "\"x\"");
    assert_eq!(run(&mut eval, "(let [new 5] (String/new \"x\"))"), "\"x\"");
    assert_eq!(
        run(&mut eval, "(let [String 5] (String/new \"x\"))"),
        "\"x\""
    );
}

// ---- `Class/new` ----

// cn-deftype, m-deftype-simple-new: deftypes construct through
// `Class/new` by dotted and simple name. cn-missing/missing-fq:
// missing parts keep the slice-1 static rule at compile time.
// h-var-new: a non-descriptor var falls through to the var miss.
// cn-val/resolve: value position keeps the slice-1 narrowing
// (N7); same-form forward refs share it (N1 adjunct).
#[test]
fn class_new() {
    let mut eval = boot();
    run_step(&mut eval, "(deftype HNW [a])");
    assert_eq!(run_step(&mut eval, "(class (user.HNW/new 1))"), "user.HNW");
    run_step(&mut eval, "(deftype HNX [a])");
    assert_eq!(run_step(&mut eval, "(class (HNX/new 1))"), "user.HNX");
    assert_eq!(
        compile_err(&mut eval, "(NopeKlass/new 1)"),
        "java.lang.RuntimeException | No such namespace: NopeKlass"
    );
    assert_eq!(
        compile_err(&mut eval, "(java.no.Klass/new 1)"),
        "java.lang.ClassNotFoundException | java.no.Klass"
    );
    run_step(&mut eval, "(def CNX5 5)");
    assert_eq!(
        compile_err(&mut eval, "(CNX5/new 1)"),
        "java.lang.RuntimeException | No such namespace: CNX5"
    );
    assert_eq!(
        compile_err(&mut eval, "String/new"),
        "java.lang.RuntimeException | No such namespace: String"
    );
    assert_eq!(run(&mut eval, "(resolve 'String/new)"), "nil");
    // Same-form define+use works (top-level `do` splits like
    // `Compiler.eval`: each element analyzes after the previous
    // evaluates — oracles `o-do-*`).
    assert_eq!(
        run(&mut eval, "(do (deftype HNF [a]) (class (user.HNF/new 1)))"),
        "user.HNF"
    );
    assert_eq!(
        run(&mut eval, "(do (deftype HNG [a]) (class (HNG/new 1)))"),
        "user.HNG"
    );
}

// ---- instance position (narrowing N8) ----

// m-inst-new: `new` through `..` on a class value misses against
// `java.lang.Class` at runtime (the slice-1 runtime-miss
// convention — the JVM misses against the represented class at
// compile time).
#[test]
fn instance_new_miss() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(.. String (new \"x\"))"),
        "java.lang.IllegalArgumentException | No matching method new found taking 1 args for class java.lang.Class"
    );
    assert_eq!(
        uncaught(&mut eval, "((memfn new x) String \"x\")"),
        "java.lang.IllegalArgumentException | No matching method new found taking 1 args for class java.lang.Class"
    );
    // Class targets miss method-shape at every arity (unlike other
    // values, whose 0-arg misses read field-first — the S16
    // class-target convention).
    assert_eq!(
        uncaught(&mut eval, "(.. String (new))"),
        "java.lang.IllegalArgumentException | No matching method new found taking 0 args for class java.lang.Class"
    );
}

// ---- port types through `new` ----

// new-deftype, ctor-deftype, o-do-deftype-dot/new: deftypes
// construct through `new` and dot forms, including same-form
// define+use (runtime resolution).
#[test]
fn hostdef_new() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype HNP [a b]) (class (new user.HNP 1 2)))"
        ),
        "user.HNP"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype HCP [a b]) (class (user.HCP. 1 2)))"
        ),
        "user.HCP"
    );
    assert_eq!(
        run(&mut eval, "(do (deftype HQP [a b]) (class (new HQP 1 2)))"),
        "user.HQP"
    );
    run_step(&mut eval, "(deftype HSP [a b])");
    assert_eq!(run_step(&mut eval, "(class (new HSP 1 2))"), "user.HSP");
    assert_eq!(run_step(&mut eval, "(class (HSP. 1 2))"), "user.HSP");
}

// ---- `(new Object)` reroute ----

// `(new Object)` lowers through `__new*` now (fresh object per
// call — a `fn`-captured `(new Object)` used to mint once at
// analyze time).
#[test]
fn new_object_reroute() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(class (new Object))"), "java.lang.Object");
    assert_eq!(
        run(&mut eval, "(identical? (new Object) (new Object))"),
        "false"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (def mkobj (fn [] (new Object))) (identical? (mkobj) (mkobj)))"
        ),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(let [new (fn [x] x)] (class (new Object)))"),
        "java.lang.Object"
    );
}

// ---- Runtime miss arms (coverage floor) ----

// The miss arms surface spellings cannot reach (compile-time
// arity gates) or the worker's pins skipped: 5+ String args and
// non-empty Object args arrive only through direct `__new*`;
// non-Int/Str/Nil decode selectors, non-bytes 4-arg buffers,
// and non-String 4-arg charset names miss at runtime. Same
// `No matching ctor` shape as the oracle-pinned misses.
#[test]
fn string_runtime_miss_arms() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(__new* 'String 1 2 3 4 5)"),
        no_ctor("java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(__new* 'Object 1)"),
        no_ctor("java.lang.Object")
    );
    assert_eq!(
        uncaught(&mut eval, "(String. (byte-array [65 66]) true)"),
        no_ctor("java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(String. 1 0 0 \"UTF-8\")"),
        no_ctor("java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(String. (byte-array [65]) 0 0 5)"),
        no_ctor("java.lang.String")
    );
}

// An instance-held var is not a type in ctor position: `(P/new)`
// falls through to the var miss like any non-descriptor var
// (`h-var-new` family), and `__new*` over the symbol reports
// `Not a type`.
#[test]
fn instance_var_ctor_miss() {
    let mut eval = boot();
    run_step(&mut eval, "(deftype HCPM [a])");
    run_step(&mut eval, "(def PINST (HCPM. 1))");
    assert_eq!(
        compile_err(&mut eval, "(PINST/new 1)"),
        "java.lang.RuntimeException | No such namespace: PINST"
    );
    assert_eq!(
        uncaught(&mut eval, "(__new* 'PINST 1)"),
        "java.lang.IllegalArgumentException | Not a type: PINST"
    );
}
