// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S15 Java arrays: one test per var replaying the
//! `docs/zero-oos-s15-arrays/probes/*.out` and
//! `docs/zero-oos-s15-arrays/mi-3d/*.out` labels verbatim
//! (oracle label → port assertion, quoting the `.out` line),
//! plus port-only pins (D1–D6, keyword designators, the OOM
//! guard).
//!
//! Intentionally unreplayed labels (20): `g-vec-*` (5 — port
//! vector comparison, not S15 behavior), `i-*` + `a-long-maxdbl`
//! (14 — core-fn comparison rows owned by their slices),
//! `h-comp-1d` (needs `.getComponentType` interop — S16).

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let last = vs.last().unwrap();
    pr(&realized(eval, last))
}

fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw for {src}, got {other}"),
        Ok(_) => panic!("expected an uncaught throw for {src}"),
    }
}

/// The probe `normhex`: `0x<hex>` → `0xH`, `@<hex>` → `@L`.
fn normhex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut it = s.chars().peekable();
    while let Some(c) = it.next() {
        if c == '0' && it.peek() == Some(&'x') {
            it.next();
            let mut any = false;
            while it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                it.next();
                any = true;
            }
            if any {
                out.push_str("0xH");
                continue;
            }
            out.push_str("0x");
            continue;
        }
        if c == '@' {
            let mut any = false;
            while it.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                it.next();
                any = true;
            }
            if any {
                out.push_str("@L");
                continue;
            }
            out.push('@');
            continue;
        }
        out.push(c);
    }
    out
}

fn run_norm(eval: &mut Evaluator, src: &str) -> String {
    normhex(&run_last(eval, src))
}

#[test]
fn smoke_ctors() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(alength (int-array 3))"), "3");
    assert_eq!(run_last(&mut eval, "(aget (int-array [1 2]) 1)"), "2");
    assert_eq!(
        run_last(&mut eval, "(do (aset (int-array [1 2]) 0 9) 7)"),
        "7"
    );
    assert_eq!(
        uncaught(&mut eval, "(alength nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: alength"
    );
}

#[test]
fn smoke_integration() {
    let mut eval = boot();
    // seq / count / nth / get over arrays.
    assert_eq!(run_last(&mut eval, "(seq (int-array [1 2]))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(seq (int-array 0))"), "nil");
    assert_eq!(run_last(&mut eval, "(count (int-array [1 2]))"), "2");
    assert_eq!(run_last(&mut eval, "(nth (int-array [1 2]) 1)"), "2");
    assert_eq!(run_last(&mut eval, "(get (int-array [1 2]) 0)"), "1");
    assert_eq!(run_last(&mut eval, "(get (int-array [1 2]) 9 :nf)"), ":nf");
    // vector-of basics.
    assert_eq!(run_last(&mut eval, "(vector-of :int 1 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(conj (vector-of :int 1) 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "((vector-of :int 7 8) 1)"), "8");
    // amap / areduce.
    assert_eq!(
        run_last(
            &mut eval,
            "(seq (amap (int-array [1 2]) i ret (* 2 (aget ret i))))"
        ),
        "(2 4)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2 3])] (areduce a i ret 0 (+ ret (aget a i))))"
        ),
        "6"
    );
    // make-array / into-array / to-array.
    assert_eq!(run_last(&mut eval, "(alength (make-array :long 2 3))"), "2");
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int [1 2]))"),
        "(1 2)"
    );
    assert_eq!(run_last(&mut eval, "(seq (to-array [1 2]))"), "(1 2)");
}

// ---- ctors: sizes (`ctors.out` §A) ----

#[test]
fn ctors_zero() {
    let mut eval = boot();
    // a-bool-size (false false)
    assert_eq!(
        run_last(&mut eval, "(seq (boolean-array 2))"),
        "(false false)"
    );
    // a-byte-size (0 0)
    assert_eq!(run_last(&mut eval, "(seq (byte-array 2))"), "(0 0)");
    // a-char-size (0 0) — the probe maps `int` over the NULs.
    assert_eq!(run_last(&mut eval, "(map int (char-array 2))"), "(0 0)");
    // a-short-size (0 0)
    assert_eq!(run_last(&mut eval, "(seq (short-array 2))"), "(0 0)");
    // a-int-size (0 0)
    assert_eq!(run_last(&mut eval, "(seq (int-array 2))"), "(0 0)");
    // a-long-size (0 0)
    assert_eq!(run_last(&mut eval, "(seq (long-array 2))"), "(0 0)");
    // a-float-size (0.0 0.0)
    assert_eq!(run_last(&mut eval, "(seq (float-array 2))"), "(0.0 0.0)");
    // a-double-size (0.0 0.0)
    assert_eq!(run_last(&mut eval, "(seq (double-array 2))"), "(0.0 0.0)");
    // a-object-size (nil nil)
    assert_eq!(run_last(&mut eval, "(seq (object-array 2))"), "(nil nil)");
    // a-zero-size 0
    assert_eq!(run_last(&mut eval, "(alength (int-array 0))"), "0");
}

// ---- ctors: seq fills (`ctors.out` §B) ----

#[test]
fn ctors_seq() {
    let mut eval = boot();
    // b-int-seq (1 2 3)
    assert_eq!(run_last(&mut eval, "(seq (int-array [1 2 3]))"), "(1 2 3)");
    // b-int-list (4 5)
    assert_eq!(run_last(&mut eval, "(seq (int-array '(4 5)))"), "(4 5)");
    // b-int-str: chars are not `Number`s.
    assert_eq!(
        uncaught(&mut eval, "(int-array \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // b-long-mixed (1 2 3)
    assert_eq!(
        run_last(&mut eval, "(seq (long-array [1 2.0 3N]))"),
        "(1 2 3)"
    );
    // b-byte-trunc (1 44)
    assert_eq!(run_last(&mut eval, "(seq (byte-array [1 300]))"), "(1 44)");
    // b-char-str (\a \b)
    assert_eq!(
        run_last(&mut eval, "(seq (char-array \"ab\"))"),
        "(\\a \\b)"
    );
    // b-char-seq (\a \b)
    assert_eq!(
        run_last(&mut eval, "(seq (char-array [\\a \\b]))"),
        "(\\a \\b)"
    );
    // b-bool-seq: `1` is not a `Boolean`.
    assert_eq!(
        uncaught(&mut eval, "(boolean-array [true 1 nil \"x\"])"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Boolean (java.lang.Long and java.lang.Boolean are in module java.base of loader 'bootstrap')"
    );
    // b-double-seq (1.0 2.0)
    assert_eq!(
        run_last(&mut eval, "(seq (double-array [1 2.0]))"),
        "(1.0 2.0)"
    );
    // b-float-seq (1.0 2.5)
    assert_eq!(
        run_last(&mut eval, "(seq (float-array [1 2.5]))"),
        "(1.0 2.5)"
    );
    // b-object-seq (1 "a" nil :k)
    assert_eq!(
        run_last(&mut eval, "(seq (object-array [1 \"a\" nil :k]))"),
        "(1 \"a\" nil :k)"
    );
}

// ---- print shapes (`interop.out` §D; rep quoted like the tag) ----

#[test]
fn print_shapes() {
    let mut eval = boot();
    // d-pr-int #object["[I" 0xH "[I@L"]
    assert_eq!(
        run_norm(&mut eval, "(pr-str (int-array [1 2]))"),
        "\"#object[\\\"[I\\\" 0xH \\\"[I@L\\\"]\""
    );
    // d-pr-obj (tag and rep quoted).
    assert_eq!(
        run_norm(&mut eval, "(pr-str (to-array [1]))"),
        "\"#object[\\\"[Ljava.lang.Object;\\\" 0xH \\\"[Ljava.lang.Object;@L\\\"]\""
    );
    // d-pr-2d.
    assert_eq!(
        run_norm(&mut eval, "(pr-str (make-array :int 2 2))"),
        "\"#object[\\\"[[I\\\" 0xH \\\"[[I@L\\\"]\""
    );
    // d-str-int "[I@L" — `str` is the `toString` (quoted: a string).
    assert_eq!(run_norm(&mut eval, "(str (int-array [1 2]))"), "\"[I@L\"");
    // d-print-str-obj — `print` leaves both fields bare.
    assert_eq!(
        run_norm(&mut eval, "(print-str (to-array [1]))"),
        "\"#object[[Ljava.lang.Object; 0xH [Ljava.lang.Object;@L]\""
    );
    // `str` of a primitive vector prints like a vector.
    assert_eq!(
        run_last(&mut eval, "(str (vector-of :int 1 2))"),
        "\"[1 2]\""
    );
}

// ---- ctors: size+init (`ctors.out` §C, `edge.out` §B, `scalar.out` §D) ----

#[test]
fn ctors_init() {
    let mut eval = boot();
    // c-int-init (7 7 7)
    assert_eq!(run_last(&mut eval, "(seq (int-array 3 7))"), "(7 7 7)");
    // c-int-initseq (1 2 0 0 0)
    assert_eq!(
        run_last(&mut eval, "(seq (int-array 5 [1 2]))"),
        "(1 2 0 0 0)"
    );
    // c-byte-init: `Int` inits take the seq path and fail it.
    assert_eq!(
        uncaught(&mut eval, "(byte-array 2 9)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // c-char-init (\x \x)
    assert_eq!(run_last(&mut eval, "(seq (char-array 2 \\x))"), "(\\x \\x)");
    // c-bool-init (true true)
    assert_eq!(
        run_last(&mut eval, "(seq (boolean-array 2 true))"),
        "(true true)"
    );
    // c-double-init (1.5 1.5)
    assert_eq!(
        run_last(&mut eval, "(seq (double-array 2 1.5))"),
        "(1.5 1.5)"
    );
    // c-object-init: `object-array` is 1-arity only.
    assert_eq!(
        uncaught(&mut eval, "(object-array 2 \"s\")"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/object-array"
    );
    // b-byte-seq (7 8 0)
    assert_eq!(run_last(&mut eval, "(seq (byte-array 3 [7 8]))"), "(7 8 0)");
    // b-byte-longseq (7)
    assert_eq!(run_last(&mut eval, "(seq (byte-array 1 [7 8 9]))"), "(7)");
    // b-short-scalar: `Int` inits fail the seq path.
    assert_eq!(
        uncaught(&mut eval, "(short-array 2 9)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // b-short-seq (7 0 0)
    assert_eq!(run_last(&mut eval, "(seq (short-array 3 [7]))"), "(7 0 0)");
    // b-long-scalar (9 9)
    assert_eq!(run_last(&mut eval, "(seq (long-array 2 9))"), "(9 9)");
    // b-long-seq (7 8 0)
    assert_eq!(run_last(&mut eval, "(seq (long-array 3 [7 8]))"), "(7 8 0)");
    // b-float-scalar (9.0 9.0)
    assert_eq!(run_last(&mut eval, "(seq (float-array 2 9))"), "(9.0 9.0)");
    // b-float-seq (7.0 0.0 0.0)
    assert_eq!(
        run_last(&mut eval, "(seq (float-array 3 [7]))"),
        "(7.0 0.0 0.0)"
    );
    // b-double-scalar (9.0 9.0)
    assert_eq!(run_last(&mut eval, "(seq (double-array 2 9))"), "(9.0 9.0)");
    // b-double-seq (7.0)
    assert_eq!(run_last(&mut eval, "(seq (double-array 1 [7 8]))"), "(7.0)");
    // b-int-longseq (1 2)
    assert_eq!(run_last(&mut eval, "(seq (int-array 2 [1 2 3]))"), "(1 2)");
    // b-bool-scalar1: `Int` inits fail the seq path.
    assert_eq!(
        uncaught(&mut eval, "(boolean-array 2 1)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // b-bool-seq (true false false)
    assert_eq!(
        run_last(&mut eval, "(seq (boolean-array 3 [true]))"),
        "(true false false)"
    );
    // b-char-str2 (\x \y \NUL) — padded with NUL (the raw byte
    // that makes `edge.out` binary; the port prints `\` + raw NUL
    // like the JVM).
    assert_eq!(
        run_last(&mut eval, "(seq (char-array 3 \"xy\"))"),
        "(\\x \\y \\\0)"
    );
    // b-char-num: `Int` inits fail the seq path.
    assert_eq!(
        uncaught(&mut eval, "(char-array 2 9)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    // b-byte-str: seq-fill unboxes `Number`s.
    assert_eq!(
        uncaught(&mut eval, "(byte-array 2 \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // b-obj-size (nil nil nil)
    assert_eq!(
        run_last(&mut eval, "(seq (object-array 3))"),
        "(nil nil nil)"
    );
    // b-obj-nil nil
    assert_eq!(run_last(&mut eval, "(seq (object-array nil))"), "nil");
    // b-int-nil nil
    assert_eq!(run_last(&mut eval, "(seq (int-array nil))"), "nil");
    // b-int-true: not seqable.
    assert_eq!(
        uncaught(&mut eval, "(int-array true)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Boolean"
    );
    // b-int-dsize: `Double` sizes narrow (`2.5` → 2; the probe
    // prints the array, the port pins the length).
    assert_eq!(run_last(&mut eval, "(alength (int-array 2.5))"), "2");
    // b-long-dsize (0 0)
    assert_eq!(run_last(&mut eval, "(seq (long-array 2.0))"), "(0 0)");
    // d-int-dblinit (2 2)
    assert_eq!(run_last(&mut eval, "(seq (int-array 2 2.5))"), "(2 2)");
    // d-int-strinit: scalar inits that are not `Number`s seq-fill.
    assert_eq!(
        uncaught(&mut eval, "(int-array 2 \"ab\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-int-nilinit (0 0)
    assert_eq!(run_last(&mut eval, "(seq (int-array 2 nil))"), "(0 0)");
    // d-int-boolinit: not seqable.
    assert_eq!(
        uncaught(&mut eval, "(int-array 2 true)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Boolean"
    );
    // d-long-dblinit (1 1)
    assert_eq!(run_last(&mut eval, "(seq (long-array 2 1.9))"), "(1 1)");
    // d-bool-nilinit (false false)
    assert_eq!(
        run_last(&mut eval, "(seq (boolean-array 2 nil))"),
        "(false false)"
    );
}

// ---- ctors: sizes, arities, fill errors (`ctors.out` §D, `match.out` §F) ----

#[test]
fn ctors_sizes() {
    let mut eval = boot();
    // d-neg-size (message is the size).
    assert_eq!(
        uncaught(&mut eval, "(int-array -1)"),
        "java.lang.NegativeArraySizeException | -1"
    );
    // d-int-of-str.
    assert_eq!(
        uncaught(&mut eval, "(int-array [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-int-of-nil (JVM null message ≡ port empty message).
    assert_eq!(
        uncaught(&mut eval, "(int-array [nil])"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.intValue()\" because the return value of \"clojure.lang.ISeq.first()\" is null"
    );
    // d-int-of-char.
    assert_eq!(
        uncaught(&mut eval, "(int-array [\\a])"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-char-of-int.
    assert_eq!(
        uncaught(&mut eval, "(char-array [65])"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Character (java.lang.Long and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // d-char-of-str.
    assert_eq!(
        uncaught(&mut eval, "(char-array [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // d-byte-of-str.
    assert_eq!(
        uncaught(&mut eval, "(byte-array [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-bool-arity.
    assert_eq!(
        uncaught(&mut eval, "(boolean-array)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/boolean-array"
    );
    // d-int-arity3.
    assert_eq!(
        uncaught(&mut eval, "(int-array 1 2 3)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/int-array"
    );
    // d-size-type: non-`Number` sizes seq-fill (`\a` is not a `Number`).
    assert_eq!(
        uncaught(&mut eval, "(int-array \"a\")"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-double-of-str.
    assert_eq!(
        uncaught(&mut eval, "(double-array [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // d-long-of-bool.
    assert_eq!(
        uncaught(&mut eval, "(long-array [true])"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // f-int-dsize-len 2 (`Double` sizes narrow).
    assert_eq!(run_last(&mut eval, "(alength (int-array 2.0))"), "2");
    // f-int-dsize2 (1 2) — 2-arity `Double` sizes take the `long` stop.
    assert_eq!(run_last(&mut eval, "(seq (int-array 2.0 [1 2]))"), "(1 2)");
    // f-int-ssize2: 2-arity sizes unbox (`checkCast` first).
    assert_eq!(
        uncaught(&mut eval, "(int-array \"a\" [1])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // f-long-dsize2 (9 9).
    assert_eq!(run_last(&mut eval, "(seq (long-array 2.0 9))"), "(9 9)");
}

// ---- `make-array` (keyword designators — the §8 extension) ----

#[test]
fn make_array() {
    let mut eval = boot();
    // All nine designators build (the oracle takes classes; the
    // port takes keywords — S16 adds class values).
    for (kw, desc) in [
        (":boolean", "[Z"),
        (":byte", "[B"),
        (":char", "[C"),
        (":short", "[S"),
        (":int", "[I"),
        (":long", "[J"),
        (":float", "[F"),
        (":double", "[D"),
        (":object", "[Ljava.lang.Object;"),
    ] {
        // The descriptor surfaces through `find`'s error text.
        assert_eq!(
            uncaught(&mut eval, &format!("(find (make-array {kw} 1) 0)")),
            format!("java.lang.IllegalArgumentException | find not supported on type: {desc}")
        );
    }
    // e-bad-dim: dims coerce through core-`int`.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // e-extra-bad-dim: every dim coerces.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int 2 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // e-neg-dim.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int -2)"),
        "java.lang.NegativeArraySizeException | -2"
    );
    // e-nil-class: nil passes the hint, `newInstance` NPEs (JVM
    // null message ≡ port empty message).
    assert_eq!(
        uncaught(&mut eval, "(make-array nil 3)"),
        "java.lang.NullPointerException | "
    );
    // e-kw-class DIVERGES by design: the oracle CCEs keywords
    // (`Keyword` is not `Class`); the port accepts the nine
    // designators (§8 extension).
    assert_eq!(run_last(&mut eval, "(alength (make-array :int 3))"), "3");
    // e-num-class: anything else fails the `^Class` hint.
    assert_eq!(
        uncaught(&mut eval, "(make-array 42 3)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.Class (java.lang.Long and java.lang.Class are in module java.base of loader 'bootstrap')"
    );
    // e-int-2d-cell (0 0 0).
    assert_eq!(
        run_last(&mut eval, "(seq (aget (make-array :int 2 3) 1))"),
        "(0 0 0)"
    );
    // e-int-3d 3.
    assert_eq!(
        run_last(&mut eval, "(alength (aget (make-array :int 2 3 4) 0))"),
        "3"
    );
    // e-obj-2d-nil: rectangular — the inner exists (an empty
    // `Object[]`, printed).
    assert_eq!(
        run_norm(&mut eval, "(pr-str (aget (make-array :object 2 2) 0))"),
        "\"#object[\\\"[Ljava.lang.Object;\\\" 0xH \\\"[Ljava.lang.Object;@L\\\"]\""
    );
    // g-str-2d-inner (nil nil) — the inner's slots are nil.
    assert_eq!(
        run_last(&mut eval, "(seq (aget (make-array :object 2 2) 0))"),
        "(nil nil)"
    );
    // f-make-dlen 2 (`Double` dims truncate).
    assert_eq!(run_last(&mut eval, "(alength (make-array :int 2.0))"), "2");
    // f-make-nillen: core-`int` of nil.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // f-make-chardim 97 (chars are codes).
    assert_eq!(run_last(&mut eval, "(alength (make-array :int \\a))"), "97");
    // a-make-huge: core-`int` of a huge `Long`.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int 9999999999)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // Zero dims build empties (no error).
    assert_eq!(run_last(&mut eval, "(alength (make-array :int 0 5))"), "0");
    // Arity.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/make-array"
    );
}

// ---- `into-array` (`seqToTypedArray`) ----

#[test]
fn into_array() {
    let mut eval = boot();
    // f-into-plain: `Long[]` inferred, `"a"` misses.
    assert_eq!(
        uncaught(&mut eval, "(into-array [1 \"a\"])"),
        "java.lang.IllegalArgumentException | array element type mismatch"
    );
    // f-into-mixed: same shape.
    assert_eq!(
        uncaught(&mut eval, "(into-array [1 \"a\" 2])"),
        "java.lang.IllegalArgumentException | array element type mismatch"
    );
    // f-into-empty 0.
    assert_eq!(run_last(&mut eval, "(alength (into-array []))"), "0");
    // f-into-typed (1 2).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int [1 2]))"),
        "(1 2)"
    );
    // f-into-typemismatch: `intCast` fails `String`s to `Character`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :int [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // f-into-kwtype DIVERGES by design (see `make_array`): the
    // port accepts `:int` (§8 extension).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int [1 2]))"),
        "(1 2)"
    );
    // f-into-niltype: nil fails at `newInstance`.
    assert_eq!(
        uncaught(&mut eval, "(into-array nil [1 2])"),
        "java.lang.NullPointerException | "
    );
    // f-into-strs ("a" "b") — the port infers `String[]`.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array [\"a\" \"b\"]))"),
        "(\"a\" \"b\")"
    );
    // a-int-float (1 2) — `intCast` truncates.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int [1.5 2.5]))"),
        "(1 2)"
    );
    // a-int-char (97 98) — `intCast` takes chars.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int [\\a \\b]))"),
        "(97 98)"
    );
    // a-int-nil: `intCast` of nil.
    assert_eq!(
        uncaught(&mut eval, "(into-array :int [nil])"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // a-int-bool: `intCast` fails non-`Number`s to `Character`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :int [true])"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Character (java.lang.Boolean and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // a-long-str: raw `set` misses.
    assert_eq!(
        uncaught(&mut eval, "(into-array :long [\"a\"])"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // a-long-char (97) — `char` widens to `long`.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :long [\\a]))"),
        "(97)"
    );
    // a-byte-trunc: `byteCast` range-checks (unlike ctor fills).
    assert_eq!(
        uncaught(&mut eval, "(into-array :byte [300])"),
        "java.lang.IllegalArgumentException | Value out of range for byte: 300"
    );
    // a-byte-str: `longCast` fails `String`s to `Number`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :byte [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // a-char-char.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :char [\\a]))"),
        "(\\a)"
    );
    // a-char-str: `charCast` routes through `longCast`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :char [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // a-char-int: codes in range store (the probe prints the array).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :char [65]))"),
        "(\\A)"
    );
    // a-bool-num / a-bool-nil: raw `set` rules.
    assert_eq!(
        uncaught(&mut eval, "(into-array :boolean [1])"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    assert_eq!(
        uncaught(&mut eval, "(into-array :boolean [nil])"),
        "java.lang.IllegalArgumentException | "
    );
    // a-bool-ok.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :boolean [true false]))"),
        "(true false)"
    );
    // a-double-ok (1.0 2.0).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :double [1 2.0]))"),
        "(1.0 2.0)"
    );
    // a-double-str: raw `set` misses.
    assert_eq!(
        uncaught(&mut eval, "(into-array :double [\"a\"])"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // a-float-ok.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :float [1 2.5]))"),
        "(1.0 2.5)"
    );
    // a-short-ok (300).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :short [300]))"),
        "(300)"
    );
    // a-into-nil-elem (1 nil) — nil fits the inference.
    assert_eq!(run_last(&mut eval, "(seq (into-array [1 nil]))"), "(1 nil)");
    // a-into-obj (1 "a" nil).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :object [1 \"a\" nil]))"),
        "(1 \"a\" nil)"
    );
    // a-into-strempty 0 (the probe uses `String`; empty is empty).
    assert_eq!(
        run_last(&mut eval, "(alength (into-array :object []))"),
        "0"
    );
    // a-into-empty-cls: empty infers `Object[]` (the probe uses
    // `class`; the port pins the descriptor through `find`).
    assert_eq!(
        uncaught(&mut eval, "(find (into-array []) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [Ljava.lang.Object;"
    );
    // a-into-str-nil ("a" nil) — the port infers `String[]`.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array [\"a\" nil]))"),
        "(\"a\" nil)"
    );
    // a-into-huge: `intCast` of a huge `Long`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :int [9999999999])"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // c-long-nil / c-double-nil: nil into a primitive.
    assert_eq!(
        uncaught(&mut eval, "(into-array :long [nil])"),
        "java.lang.IllegalArgumentException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(into-array :double [nil])"),
        "java.lang.IllegalArgumentException | "
    );
    // c-long-double: no `Double`→`long` narrowing.
    assert_eq!(
        uncaught(&mut eval, "(into-array :long [1.5])"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // c-short-char (97) — `char` routes through `longCast`.
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :short [\\a]))"),
        "(97)"
    );
    // c-double-char (97.0).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :double [\\a]))"),
        "(97.0)"
    );
    // c-lazy-plain: inference over a lazy seq is `Long[]` (the
    // probe uses `class`; the port pins the narrowing
    // behaviorally — a `String` store fails).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (into-array (range 3))] (aset a 0 \"s\"))"
        ),
        "java.lang.ArrayStoreException | java.lang.String"
    );
    // c-lazy-typed (2 3).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int (map inc [1 2])))"),
        "(2 3)"
    );
    // c-obj-lazy (0 1).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :object (range 2)))"),
        "(0 1)"
    );
    // c-int-lazy (0 1 2).
    assert_eq!(
        run_last(&mut eval, "(seq (into-array :int (range 3)))"),
        "(0 1 2)"
    );
    // c-int-empty-cls "class [I": typed empties keep their
    // descriptor (the probe uses `class`; the port pins through
    // `find`).
    assert_eq!(
        uncaught(&mut eval, "(find (into-array :int []) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [I"
    );
    // Ad-hoc `ia-*`: the `*Cast` take the `long` stop.
    assert_eq!(
        uncaught(&mut eval, "(into-array :int [1e300])"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    assert_eq!(
        uncaught(&mut eval, "(into-array :short [1e300])"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    assert_eq!(
        uncaught(&mut eval, "(into-array :char [70000])"),
        "java.lang.IllegalArgumentException | Value out of range for char: 70000"
    );
    assert_eq!(run_last(&mut eval, "(seq (into-array :byte [1.5]))"), "(1)");
}

// ---- `to-array` / `to-array-2d` (`RT.toArray`) ----

#[test]
fn to_arrays() {
    let mut eval = boot();
    // f-to-vec (1 "a" nil).
    assert_eq!(
        run_last(&mut eval, "(seq (to-array [1 \"a\" nil]))"),
        "(1 \"a\" nil)"
    );
    // f-to-list (1 2).
    assert_eq!(run_last(&mut eval, "(seq (to-array '(1 2)))"), "(1 2)");
    // f-to-nil: the shared empty (printed).
    assert_eq!(
        run_norm(&mut eval, "(pr-str (to-array nil))"),
        "\"#object[\\\"[Ljava.lang.Object;\\\" 0xH \\\"[Ljava.lang.Object;@L\\\"]\""
    );
    // f-to-map ([:a 1]) — entries.
    assert_eq!(run_last(&mut eval, "(seq (to-array {:a 1}))"), "([:a 1])");
    // f-to-str (\a \b) — chars.
    assert_eq!(run_last(&mut eval, "(seq (to-array \"ab\"))"), "(\\a \\b)");
    // f-to-arr (1 2) — a boxed copy.
    assert_eq!(
        run_last(&mut eval, "(seq (to-array (int-array [1 2])))"),
        "(1 2)"
    );
    // f-to2d ((1 2) (3)).
    assert_eq!(
        run_last(&mut eval, "(map seq (to-array-2d [[1 2] [3]]))"),
        "((1 2) (3))"
    );
    // f-to2d-empty 0.
    assert_eq!(run_last(&mut eval, "(alength (to-array-2d []))"), "0");
    // f-to2d-ragged-nil: a nil row becomes the empty row.
    assert_eq!(
        run_norm(&mut eval, "(pr-str (to-array-2d [nil]))"),
        "\"#object[\\\"[[Ljava.lang.Object;\\\" 0xH \\\"[[Ljava.lang.Object;@L\\\"]\""
    );
    // d-range (0 1 2).
    assert_eq!(run_last(&mut eval, "(seq (to-array (range 3)))"), "(0 1 2)");
    // d-seq (1 2).
    assert_eq!(run_last(&mut eval, "(seq (to-array (seq [1 2])))"), "(1 2)");
    // d-lazy (2 3).
    assert_eq!(
        run_last(&mut eval, "(seq (to-array (map inc [1 2])))"),
        "(2 3)"
    );
    // d-set 2.
    assert_eq!(run_last(&mut eval, "(count (to-array #{1 2}))"), "2");
    // d-map2 2.
    assert_eq!(run_last(&mut eval, "(count (to-array {:a 1 :b 2}))"), "2");
    // d-ident: `Object[]` (any dims) keeps its handle (the probe
    // uses `identical?`, out-of-scope here — `=` is identity for
    // arrays, so it pins the same fact).
    assert_eq!(
        run_last(&mut eval, "(let [a (to-array [1])] (= (to-array a) a))"),
        "true"
    );
    // d-prim-cls / d-str-cls: other arrays box into fresh
    // `Object[]`s (the probes use `class`; the port pins the
    // copy behaviorally — anything stores).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (to-array (int-array [1]))] (aset a 0 \"s\") (seq a))"
        ),
        "(\"s\")"
    );
    // d-num / d-kw: junk fails to convert.
    assert_eq!(
        uncaught(&mut eval, "(to-array 1)"),
        "java.lang.RuntimeException | Unable to convert: class java.lang.Long to Object[]"
    );
    assert_eq!(
        uncaught(&mut eval, "(to-array :k)"),
        "java.lang.RuntimeException | Unable to convert: class clojure.lang.Keyword to Object[]"
    );
    // e-to-nil-ident / g-to-nil-len: THE shared empty (`=`
    // stands in for the out-of-scope `identical?`).
    assert_eq!(
        run_last(&mut eval, "(= (to-array nil) (to-array nil))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(alength (to-array nil))"), "0");
    // g-to2d-nil: the `^Collection` hint passes nil to `.size`.
    assert_eq!(
        uncaught(&mut eval, "(to-array-2d nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.util.Collection.size()\" because \"coll\" is null"
    );
    // g-to2d-num: rows convert per element.
    assert_eq!(
        uncaught(&mut eval, "(to-array-2d [1])"),
        "java.lang.RuntimeException | Unable to convert: class java.lang.Long to Object[]"
    );
    // g-to2d-str-elem ((\a \b)).
    assert_eq!(
        run_last(&mut eval, "(map seq (to-array-2d [\"ab\"]))"),
        "((\\a \\b))"
    );
    // g-to2d-lens (2 1).
    assert_eq!(
        run_last(&mut eval, "(map alength (to-array-2d [[1 2] [3]]))"),
        "(2 1)"
    );
    // Ad-hoc `t2d-*`: the `^Collection` hint CCEs (the spec's
    // `t2d-map`/`t2d-str` labels do not exist in the probes;
    // these texts are ad-hoc-oracle-verbatim).
    assert_eq!(
        uncaught(&mut eval, "(to-array-2d {:a 1})"),
        "java.lang.ClassCastException | class clojure.lang.PersistentArrayMap cannot be cast to class java.util.Collection (clojure.lang.PersistentArrayMap is in unnamed module of loader 'app'; java.util.Collection is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(to-array-2d \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.Collection (java.lang.String and java.util.Collection are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(to-array-2d (int-array [1]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class java.util.Collection ([I and java.util.Collection are in module java.base of loader 'bootstrap')"
    );
}

// ---- `aget` (`access.out` §A, `match.out` §C, `edge.out` §C, `scalar.out` §A) ----

#[test]
fn aget_access() {
    let mut eval = boot();
    // a-idx-long-obj 2.
    assert_eq!(run_last(&mut eval, "(aget (to-array [1 2]) (long 1))"), "2");
    // a-idx-type 2 (`Double` idx truncates).
    assert_eq!(run_last(&mut eval, "(aget (int-array [1 2]) 1.0)"), "2");
    // a-idx-nil: core-`int` of nil.
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // a-idx-str: core-`int` of a `String`.
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]) \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // a-aget-huge / a-aget-hugedbl: core-`int` stops.
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]) 9999999999)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]) 1e300)"),
        "java.lang.IllegalArgumentException | Value out of range for int: 1.0E300"
    );
    // a-char \b.
    assert_eq!(
        run_last(&mut eval, "(aget (char-array [\\a \\backspace]) 1)"),
        "\\backspace"
    );
    // a-nonarray / a-nil: the overload match errors.
    assert_eq!(
        uncaught(&mut eval, "(aget [1 2] 0)"),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget nil 0)"),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    // a-2d-row (0 0 0).
    assert_eq!(
        run_last(&mut eval, "(seq (aget (make-array :int 2 3) 0))"),
        "(0 0 0)"
    );
    // a-oob: the VM `Index` text (direct overloads only).
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]) 5)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    // a-nil-inner: the 2-arity body's `(class array)` NPE (the
    // probe builds `int[][2]` via `int[].class` — S16; the port
    // replays the shape with `:object`, whose slots are nil).
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :object 2) 0 0)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.getComponentType()\" because the return value of \"clojure.core$class.invokeStatic(Object)\" is null"
    );
    // a-deep-nonarray.
    assert_eq!(
        uncaught(&mut eval, "(aget (to-array [1 2]) 0 0)"),
        "java.lang.IllegalArgumentException | Argument is not an array"
    );
    // a-3d 0 (middle steps are direct, the final step raw).
    assert_eq!(
        run_last(&mut eval, "(aget (make-array :int 2 2 2) 1 1 1)"),
        "0"
    );
    // A nil MIDDLE step takes the direct rule (`More than one`,
    // not the body's NPE — `core.clj` inlines every step but the
    // final `apply`; port pin).
    assert_eq!(
        uncaught(
            &mut eval,
            "(aget (into-array [(into-array [(into-array [1])]) nil]) 1 0 0)"
        ),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    // c-aget-tail-idx 0.
    assert_eq!(run_last(&mut eval, "(aget (make-array :int 2 2) 0 0)"), "0");
    // c-aget-str-tail: tails `checkCast` first.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // b-aget-nidx: nil trips `intCast`'s `Character` branch.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // b-aget-cidx: chars are not tail indices.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 \\a)"),
        "java.lang.ClassCastException | class java.lang.Character cannot be cast to class java.lang.Number (java.lang.Character and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // a-tail-hugedbl / a-tail-nan: the `long` stop, `NaN` → 0.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 1e300)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
    assert_eq!(
        run_last(&mut eval, "(aget (make-array :int 2 2) 0 ##NaN)"),
        "0"
    );
    // Ad-hoc `tail-oob-get`: `Array.get` bounds-checks explicitly
    // (empty message — only the direct overloads use `Index` text).
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 5)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // Idx coerces before matching (inline arg order — port pin):
    // bad idx + bad array reports the idx.
    assert_eq!(
        uncaught(&mut eval, "(aget nil \"bad\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
}

// ---- `aset` direct (`access.out` §B, `edge.out` §E, `scalar.out` §E) ----

#[test]
fn aset_direct() {
    let mut eval = boot();
    // b-obj [8 (8 2)] — answers the val, mutates in place.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (to-array [1 2])] [(aset a 0 8) (seq a)])"
        ),
        "[8 (8 2)]"
    );
    // b-basic [9 (9 2)].
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2])] [(aset a 0 9) (seq a)])"
        ),
        "[9 (9 2)]"
    );
    // b-idx-type: `Double` idx truncates, then OOB is `Index` text.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 1.0 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 1 out of bounds for length 1"
    );
    // b-store-obj: refs store-check (names the val class).
    assert_eq!(
        uncaught(&mut eval, "(let [a (into-array [\"x\"])] (aset a 0 1))"),
        "java.lang.ArrayStoreException | java.lang.Long"
    );
    // b-store-2d: nesting checks the inner descriptor.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (make-array :int 1 1)] (aset a 0 (long-array [1])))"
        ),
        "java.lang.ArrayStoreException | [J"
    );
    // b-oob: match first, then bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 5 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    // b-nil / b-nonarray: the overload match errors.
    assert_eq!(
        uncaught(&mut eval, "(aset nil 0 5)"),
        "java.lang.IllegalArgumentException | More than one matching method found: aset"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset [1 2] 0 5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // b-narrow DIVERGES (D2): the JVM only matches the erased
    // `Short`/`Byte` wrappers; the port stores in-range `Int`s —
    // and 300 is out of `byte` range, so the width error fires.
    assert_eq!(
        uncaught(&mut eval, "(aset (byte-array [1]) 0 300)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: 300"
    );
    // b-narrow-int: `Double`s never match primitives.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 0 2.5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // b-char [\z (\z \b)].
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (char-array [\\a \\backspace])] [(aset a 0 \\z) (seq a)])"
        ),
        "[\\z (\\z \\backspace)]"
    );
    // b-bool [false (false)].
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (boolean-array [true])] [(aset a 0 false) (seq a)])"
        ),
        "[false (false)]"
    );
    // e-long-long (5).
    assert_eq!(
        run_last(&mut eval, "(let [a (long-array [1])] (aset a 0 5) (seq a))"),
        "(5)"
    );
    // e-int-double / e-double-long: no-match both ways.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 0 2.5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset (double-array [1]) 0 5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // e-double-double (2.5).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] (aset a 0 2.5) (seq a))"
        ),
        "(2.5)"
    );
    // e-float-float (2.5) / e-float-double 2.5 (read back).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (float-array [1])] (aset a 0 2.5) (seq a))"
        ),
        "(2.5)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (float-array [1])] (aset a 0 2.5) (aget a 0))"
        ),
        "2.5"
    );
    // e-byte-byte / e-short-short (D2 stores what the JVM matches
    // through the erased wrappers — same outcome).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (byte-array [1])] (aset a 0 (byte 7)) (seq a))"
        ),
        "(7)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (short-array [1])] (aset a 0 (short 7)) (seq a))"
        ),
        "(7)"
    );
    // e-char-long / e-bool-long: no-match.
    assert_eq!(
        uncaught(&mut eval, "(aset (char-array [\\a]) 0 5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset (boolean-array [true]) 0 5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // e-obj-nil (nil) / e-obj-any (2.5).
    assert_eq!(
        run_last(&mut eval, "(let [a (to-array [1])] (aset a 0 nil) (seq a))"),
        "(nil)"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (to-array [1])] (aset a 0 2.5) (seq a))"),
        "(2.5)"
    );
    // e-long-double / e-int-intobj.
    assert_eq!(
        uncaught(&mut eval, "(aset (long-array [1]) 0 2.5)"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] (aset a 0 (int 7)) (seq a))"
        ),
        "(7)"
    );
    // e-direct-int-huge: `toIntExact` past `i32`.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 0 9999999999)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // e-direct-float-huge DIVERGES (D3): the JVM literal rule
    // narrows to `Inf`; the port stores the exact `f64`.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (float-array [1.0])] [(aset a 0 1e300) (seq a)])"
        ),
        "[1.0E300 (1.0E300)]"
    );
    // The `No-match` value list (`edge.out` §E: `a-int-bigint`,
    // `a-long-bigint`, `a-double-bigdec`, `a-double-intcall`,
    // `a-int-char`, `a-float-long`, `a-int-ratio`).
    for src in [
        "(aset (int-array [1]) 0 1N)",
        "(aset (long-array [1]) 0 1N)",
        "(aset (float-array [1]) 0 5)",
        "(aset (double-array [1]) 0 1.5M)",
        "(aset (int-array [1]) 0 1N)",
        "(aset (int-array [1]) 0 \\a)",
        "(aset (int-array [1]) 0 1/2)",
        "(aset (double-array [1]) 0 5)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method aset found taking 3 args",
            "{src}"
        );
    }
    // Ad-hoc `i-ch`/`l-ch`/`d-ch`/`f-ch`/`s-ch`: `Reflector` never
    // widens `Character` for direct stores.
    for src in [
        "(aset (int-array [1]) 0 \\a)",
        "(aset (long-array [1]) 0 \\a)",
        "(aset (double-array [1]) 0 \\a)",
        "(aset (float-array [1]) 0 \\a)",
        "(aset (short-array [1]) 0 \\a)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method aset found taking 3 args",
            "{src}"
        );
    }
}

// ---- `aset` tails + `Array.setX` (`access.out` §C, `match.out` §B/C, `edge.out` §C/D) ----

#[test]
fn aset_tails_setx() {
    let mut eval = boot();
    // b-2d: tails never narrow `Long` to `int`.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 2 2) 1 1 8)"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // c-int-2d [8 (0 8)].
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :int 2 2)] [(aset-int a 1 1 8) (seq (aget a 1))])"
        ),
        "[8 (0 8)]"
    );
    // e-tail-int-long: `int[]`←`Int` misses…
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 1 1) 0 0 5)"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // …while `long[]`←`Int` stores (e-tail-long-long (5)).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :long 1 1)] (aset a 0 0 5) (seq (aget a 0)))"
        ),
        "(5)"
    );
    // e-tail-int-int DIVERGES (D5): `(int 5)` inlines to a
    // primitive `int`, which `apply` boxes to `Integer` (exact
    // unbox — stores); the port's `Int` is always `Long`
    // (mismatch, like the literal).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (make-array :int 1 1)] (aset a 0 0 (int 5)) (seq (aget a 0)))"
        ),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // b-aset-nilinner: the empty NPE.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :object 1) 0 0 9)"),
        "java.lang.NullPointerException | "
    );
    // b-deep-nonarray.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 0 0 9)"),
        "java.lang.IllegalArgumentException | Argument is not an array"
    );
    // Ad-hoc `tail-oob-set`: `Array.set` bounds-checks explicitly.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 2 2) 0 5 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // Ad-hoc `store-order`: bounds beat the store check.
    assert_eq!(
        uncaught(&mut eval, "(aset (into-array [\"a\"]) 5 1)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    // c-aset-obj2d ["x" (nil "x")].
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :object 2 2)] [(aset a 0 1 \"x\") (seq (aget a 0))])"
        ),
        "[\"x\" (nil \"x\")]"
    );
    // c-aset-int2d-f: tails never narrow.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 2 2) 0 1 1.5)"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // c-aset-tail-idx "x" (tails answer the val; `Double` tails truncate).
    assert_eq!(
        run_last(&mut eval, "(aset (make-array :object 2 2) 0 1.0 \"x\")"),
        "\"x\""
    );
    // c-aset-long-tail ["x" (nil "x")] (`Long` idxs).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :object 2 2)] [(aset a (long 0) (long 1) \"x\") (seq (aget a 0))])"
        ),
        "[\"x\" (nil \"x\")]"
    );
    // c-int [5 (5)] — `setX` answers the ORIGINAL val.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] [(aset-int a 0 5) (seq a)])"
        ),
        "[5 (5)]"
    );
    // c-long / c-short / c-byte / c-char / c-bool / c-float / c-double.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] [(aset-long a 0 5) (seq a)])"
        ),
        "[5 (5)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (short-array [1])] [(aset-short a 0 300) (seq a)])"
        ),
        "[300 (300)]"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-byte (byte-array [1]) 0 300)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: 300"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (char-array [\\a \\backspace])] [(aset-char a 0 122) (seq a)])"
        ),
        "[122 (\\z \\backspace)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (boolean-array [false])] [(aset-boolean a 0 1) (seq a)])"
        ),
        "[1 (true)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (float-array [1])] [(aset-float a 0 2.5) (seq a)])"
        ),
        "[2.5 (2.5)]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] [(aset-double a 0 2) (seq a)])"
        ),
        "[2 (2.0)]"
    );
    // c-int-wrongarr 5 (`int` widens to `long`).
    assert_eq!(run_last(&mut eval, "(aset-int (long-array [1]) 0 5)"), "5");
    // c-long-wrongarr / c-bool-intarr: narrowing misses.
    assert_eq!(
        uncaught(&mut eval, "(aset-long (int-array [1]) 0 5)"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-boolean (int-array [1]) 0 true)"),
        "java.lang.IllegalArgumentException | argument type mismatch"
    );
    // c-int-objarr: refs fail the primitive check.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (to-array [1]) 0 5)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    // c-int-oob: bounds are the empty `AIOOBE`.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) 5 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // c-double-oob (same).
    assert_eq!(
        uncaught(&mut eval, "(aset-double (double-array [1]) 5 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // c-char-str / c-short-nil / c-float-str / c-byte-str: vals
    // coerce through the core fns.
    assert_eq!(
        uncaught(&mut eval, "(aset-char (char-array [\\a]) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-short (short-array [1]) 0 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-float (float-array [1]) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-byte (byte-array [1]) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // b-setint-vec / b-setint-2d / b-setint-sidx / b-setint-nidx.
    assert_eq!(
        uncaught(&mut eval, "(aset-int [1 2] 0 9)"),
        "java.lang.IllegalArgumentException | Argument is not an array"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-int (make-array :int 2 2) 0 9)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) \"a\" 9)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) nil 9)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // e-setint-vec / e-setint-nilarr: raw checks, empty NPE.
    assert_eq!(
        uncaught(&mut eval, "(aset-int [1 2] 0 9)"),
        "java.lang.IllegalArgumentException | Argument is not an array"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-int nil 0 9)"),
        "java.lang.NullPointerException | "
    );
    // e-setint-oob: bounds beat the widen check (ad-hoc P2).
    assert_eq!(
        uncaught(&mut eval, "(aset-int (byte-array [1]) 5 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // Ad-hoc: the primitive check beats bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (to-array [1]) 5 9)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    // The `d-set*` widen matrix (`d-setint-long-stores`,
    // `d-setint-double`, `d-setint-float`, `d-setint-byte`,
    // `d-setint-short`, `d-setint-char`, `d-setint-bool`,
    // `d-setlong-int`, `d-setlong-double`, `d-setfloat-long`,
    // `d-setdouble-float`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] (aset-int a 0 5) (seq a))"
        ),
        "(5)"
    );
    for src in [
        "(aset-int (byte-array [1]) 0 5)",
        "(aset-int (short-array [1]) 0 5)",
        "(aset-int (char-array [\\a]) 0 5)",
        "(aset-int (boolean-array [true]) 0 5)",
        "(aset-long (int-array [1]) 0 5)",
        "(aset-float (long-array [1]) 0 5)",
        "(aset-double (float-array [1]) 0 5)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | argument type mismatch",
            "{src}"
        );
    }
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] (aset-int a 0 5) (seq a))"
        ),
        "(5.0)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (float-array [1])] (aset-int a 0 5) (seq a))"
        ),
        "(5.0)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] (aset-long a 0 5) (seq a))"
        ),
        "(5.0)"
    );
    // d-setshort-int 5 / d-setbyte-int 5 (read back).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] (aset-short a 0 5) (aget a 0))"
        ),
        "5"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] (aset-byte a 0 5) (aget a 0))"
        ),
        "5"
    );
    // d-setshort-long / d-setbyte-long (5).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] (aset-short a 0 5) (seq a))"
        ),
        "(5)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] (aset-byte a 0 5) (seq a))"
        ),
        "(5)"
    );
    // d-sethcar-int \a (the ORIGINAL val).
    assert_eq!(
        run_last(&mut eval, "(aset-char (int-array [1]) 0 \\a)"),
        "\\a"
    );
    // d-setfloat-double (5.0) / d-setbool-bool (false).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] (aset-float a 0 5) (seq a))"
        ),
        "(5.0)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (boolean-array [true])] (aset-boolean a 0 nil) (seq a))"
        ),
        "(false)"
    );
    // d-setbool-obj: the primitive check.
    assert_eq!(
        uncaught(&mut eval, "(aset-boolean (to-array [true]) 0 true)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    // a-setint-huge / a-setlong-huge: vals take their stops.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) 0 9999999999)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-long (long-array [1]) 0 1e300)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300"
    );
}

// ---- the eight casts + `bytes?` (`access.out` §D, `edge.out` §H) ----

#[test]
fn casts_bytesq() {
    let mut eval = boot();
    // d-ints-pass (1 2), d-longs-pass (1), d-bytes-pass (1),
    // d-bools-pass (true), d-chars-pass (\a \b), d-shorts-pass (1).
    assert_eq!(
        run_last(&mut eval, "(seq (ints (int-array [1 2])))"),
        "(1 2)"
    );
    assert_eq!(run_last(&mut eval, "(seq (longs (long-array [1])))"), "(1)");
    assert_eq!(run_last(&mut eval, "(seq (bytes (byte-array [1])))"), "(1)");
    assert_eq!(
        run_last(&mut eval, "(seq (booleans (boolean-array [true])))"),
        "(true)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (chars (char-array \"ab\")))"),
        "(\\a \\b)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (shorts (short-array [1])))"),
        "(1)"
    );
    // d-doubles-pass (1.0), d-floats-pass (1.0).
    assert_eq!(
        run_last(&mut eval, "(seq (doubles (double-array [1])))"),
        "(1.0)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (floats (float-array [1])))"),
        "(1.0)"
    );
    // d-ints-vec / d-ints-wrong / d-ints-nil.
    assert_eq!(
        uncaught(&mut eval, "(ints [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [I (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [I is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(ints (long-array [1]))"),
        "java.lang.ClassCastException | class [J cannot be cast to class [I ([J and [I are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(run_last(&mut eval, "(ints nil)"), "nil");
    // d-bytes-vec / d-bytes-str / d-chars-str / d-longs-vec /
    // d-doubles-vec / d-floats-vec / d-shorts-vec / d-bools-vec.
    assert_eq!(
        uncaught(&mut eval, "(bytes [1 2])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [B (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [B is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(bytes \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class [B (java.lang.String and [B are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(chars \"ab\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class [C (java.lang.String and [C are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(longs [1])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [J (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [J is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(doubles [1.0])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [D (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [D is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(floats [1.0])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [F (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [F is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(shorts [1])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [S (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [S is in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(booleans [true])"),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class [Z (clojure.lang.PersistentVector is in unnamed module of loader 'app'; [Z is in module java.base of loader 'bootstrap')"
    );
    // h-ints-2d: nesting misses the flat checkcast.
    assert_eq!(
        uncaught(&mut eval, "(ints (make-array :int 2 2))"),
        "java.lang.ClassCastException | class [[I cannot be cast to class [I ([[I and [I are in module java.base of loader 'bootstrap')"
    );
    // h-ints-clone-same / h-bytes-clone-same: casts keep the
    // handle (`=` stands in for the out-of-scope `identical?`).
    assert_eq!(
        run_last(&mut eval, "(let [a (int-array [1])] (= (ints a) a))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (byte-array [1])] (= (bytes a) a))"),
        "true"
    );
    // e-bytes-t / e-bytes-f-int / e-bytes-f-nil / e-bytes-f-vec /
    // e-bytes-f-str / e-bytes-f-obj: true only for flat `byte[]`
    // (never throws).
    assert_eq!(run_last(&mut eval, "(bytes? (byte-array [1]))"), "true");
    assert_eq!(run_last(&mut eval, "(bytes? (int-array [1]))"), "false");
    assert_eq!(run_last(&mut eval, "(bytes? nil)"), "false");
    assert_eq!(run_last(&mut eval, "(bytes? [1])"), "false");
    assert_eq!(run_last(&mut eval, "(bytes? \"ab\")"), "false");
    // e-bytes-arity: 1-arity only.
    assert_eq!(
        uncaught(&mut eval, "(bytes?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/bytes?"
    );
}

// ---- `alength` / `aclone` (`ctors.out` §G) ----

#[test]
fn alen_clone() {
    let mut eval = boot();
    // g-clone-eq [false (1 2)] — fresh identity (`=` stands in
    // for the out-of-scope `identical?`).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2])] [(= (aclone a) a) (seq (aclone a))])"
        ),
        "[false (1 2)]"
    );
    // g-clone-mut [(9 2) (1 2)] — independent cells…
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2]) b (aclone a)] (aset b 0 9) [(seq b) (seq a)])"
        ),
        "[(9 2) (1 2)]"
    );
    // …but shallow inners (g-clone-2d-shallow (9 0)).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :int 2 2) b (aclone a)] (aset (aget b 0) 0 9) (seq (aget a 0)))"
        ),
        "(9 0)"
    );
    // g-clone-obj (1 "a").
    assert_eq!(
        run_last(&mut eval, "(seq (aclone (to-array [1 \"a\"])))"),
        "(1 \"a\")"
    );
    // g-clone-nonarray / g-clone-nil.
    assert_eq!(
        uncaught(&mut eval, "(aclone [1])"),
        "java.lang.IllegalArgumentException | No matching method aclone found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(aclone nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: aclone"
    );
    // g-len 5 / g-len-2d 2.
    assert_eq!(run_last(&mut eval, "(alength (long-array 5))"), "5");
    assert_eq!(run_last(&mut eval, "(alength (make-array :int 2 3))"), "2");
    // g-len-nonarray / g-len-nil / g-len-str.
    assert_eq!(
        uncaught(&mut eval, "(alength [1 2])"),
        "java.lang.IllegalArgumentException | No matching method alength found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(alength nil)"),
        "java.lang.IllegalArgumentException | More than one matching method found: alength"
    );
    assert_eq!(
        uncaught(&mut eval, "(alength \"ab\")"),
        "java.lang.IllegalArgumentException | No matching method alength found taking 1 args"
    );
    // h-clone: clones keep the descriptor (the probe uses `class`;
    // the port pins behaviorally — still an `int[]`).
    assert_eq!(
        uncaught(&mut eval, "(aset (aclone (int-array [1])) 0 \"s\")"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
}

// ---- `vector-of` (`interop.out` §F, `edge.out` §J, `match.out` §G, `scalar.out` §F/G) ----

#[test]
fn vector_of() {
    let mut eval = boot();
    // f-ctor [1 2 3].
    assert_eq!(run_last(&mut eval, "(vector-of :int 1 2 3)"), "[1 2 3]");
    // f-cls: a `Vec` (the probe uses `class`; the port pins the
    // name through the invoke arity error).
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 1) 0 :nf)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core.Vec"
    );
    // f-conj-bad / f-assoc-bad: elements coerce through core-`int`.
    assert_eq!(
        uncaught(&mut eval, "(conj (vector-of :int 1) \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (vector-of :int 1) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // f-eq-vec / f-eq-vo / f-hash-eq: sequential `=`/`hash`.
    assert_eq!(
        run_last(&mut eval, "(= (vector-of :int 1 2) [1 2])"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (vector-of :int 1 2) (vector-of :long 1 2))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (hash (vector-of :int 1 2)) (hash [1 2]))"),
        "true"
    );
    // f-big [100 33 99] (conj 100 deep).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [v (apply conj (vector-of :int) (range 100))] [(count v) (nth v 33) (nth v 99)])"
        ),
        "[100 33 99]"
    );
    // f-badtype / f-badtype-str: only the eight keywords.
    assert_eq!(
        uncaught(&mut eval, "(vector-of :bad 1)"),
        "java.lang.IllegalArgumentException | Unrecognized type :bad"
    );
    assert_eq!(
        uncaught(&mut eval, "(vector-of \"int\" 1)"),
        "java.lang.IllegalArgumentException | Unrecognized type int"
    );
    // f-char [\a \b] / f-bool (true false) / f-float (1.0 2.5).
    assert_eq!(
        run_last(&mut eval, "(vector-of :char \\a \\b)"),
        "[\\a \\b]"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (vector-of :boolean true false))"),
        "(true false)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (vector-of :float 1 2.5))"),
        "(1.0 2.5)"
    );
    // f-meta {:a 1}.
    assert_eq!(
        run_last(&mut eval, "(meta (with-meta (vector-of :int 1) {:a 1}))"),
        "{:a 1}"
    );
    // f-invoke 20 / f-invoke-2 / f-invoke-bad / f-invoke-oob.
    assert_eq!(run_last(&mut eval, "((vector-of :int 10 20) 1)"), "20");
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) 0 :nf)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core.Vec"
    );
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) \"k\")"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) 9)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // f-peek / f-pop / f-pop-empty / f-empty-coll / f-empty?-t.
    assert_eq!(run_last(&mut eval, "(peek (vector-of :int 1 2))"), "2");
    assert_eq!(run_last(&mut eval, "(pop (vector-of :int 1 2))"), "[1]");
    assert_eq!(
        uncaught(&mut eval, "(pop (vector-of :int))"),
        "java.lang.IllegalStateException | Can't pop empty vector"
    );
    assert_eq!(run_last(&mut eval, "(empty (vector-of :int 1 2))"), "[]");
    assert_eq!(run_last(&mut eval, "(empty? (vector-of :int))"), "true");
    // f-count / f-nth / f-seq / f-counted?.
    assert_eq!(run_last(&mut eval, "(count (vector-of :int 1 2 3))"), "3");
    assert_eq!(run_last(&mut eval, "(nth (vector-of :int 5 6) 1)"), "6");
    assert_eq!(
        run_last(&mut eval, "(seq (vector-of :double 1.5 2.5))"),
        "(1.5 2.5)"
    );
    assert_eq!(run_last(&mut eval, "(counted? (vector-of :int))"), "true");
    // f-get 6 / f-nth-nf :nf / f-nth-oob.
    assert_eq!(run_last(&mut eval, "(get (vector-of :int 5 6) 1 :nf)"), "6");
    assert_eq!(run_last(&mut eval, "(nth (vector-of :int 5) 9 :nf)"), ":nf");
    assert_eq!(
        uncaught(&mut eval, "(nth (vector-of :int 5) 9)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // f-rseq (3 2 1).
    assert_eq!(
        run_last(&mut eval, "(rseq (vector-of :int 1 2 3))"),
        "(3 2 1)"
    );
    // f-subvec [2 3].
    assert_eq!(
        run_last(&mut eval, "(subvec (vector-of :int 1 2 3) 1)"),
        "[2 3]"
    );
    // f-object: `:object` is not a `vector-of` type.
    assert_eq!(
        uncaught(&mut eval, "(vector-of :object 1 \"a\")"),
        "java.lang.IllegalArgumentException | Unrecognized type :object"
    );
    // j-empty-cls / j-empty-conj: `empty` stays typed (the probe
    // uses `class`; the port pins behaviorally — coerce still
    // fires).
    assert_eq!(
        uncaught(&mut eval, "(conj (empty (vector-of :int)) \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // j-subvec-cls: `subvec` answers a PLAIN subvector (the probe
    // uses `class`; the port pins behaviorally — no coerce).
    assert_eq!(
        run_last(
            &mut eval,
            "(assoc (subvec (vector-of :int 1 2) 0 1) 0 \"a\")"
        ),
        "[\"a\"]"
    );
    // j-conj-nil / j-assoc-nil: core-`int` of nil.
    assert_eq!(
        uncaught(&mut eval, "(conj (vector-of :int) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (vector-of :int 1) 0 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // j-byte-vec: `conj` coerces through core-`byte`.
    assert_eq!(
        uncaught(&mut eval, "(conj (vector-of :byte 1) 300)"),
        "java.lang.IllegalArgumentException | Value out of range for byte: 300"
    );
    // j-long-char [97].
    assert_eq!(run_last(&mut eval, "(conj (vector-of :long) \\a)"), "[97]");
    // j-bool-num [true true] (truthiness).
    assert_eq!(
        run_last(&mut eval, "(conj (vector-of :boolean) 1 2)"),
        "[true true]"
    );
    // j-peek-empty nil / j-rseq-empty nil / j-count-empty 0.
    assert_eq!(run_last(&mut eval, "(peek (vector-of :int))"), "nil");
    assert_eq!(run_last(&mut eval, "(rseq (vector-of :int))"), "nil");
    assert_eq!(run_last(&mut eval, "(count (vector-of :int))"), "0");
    // j-nth-neg :nf.
    assert_eq!(
        run_last(&mut eval, "(nth (vector-of :int 5) -1 :nf)"),
        ":nf"
    );
    // j-arity0.
    assert_eq!(
        uncaught(&mut eval, "(vector-of)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/vector-of"
    );
    // g-ctor-trunc [1] (truncation, not wrap).
    assert_eq!(run_last(&mut eval, "(vector-of :int 1.5)"), "[1]");
    // g-bool-nil [false] (truthiness).
    assert_eq!(
        run_last(&mut eval, "(conj (vector-of :boolean) nil)"),
        "[false]"
    );
    // g-count-big 40.
    assert_eq!(
        run_last(
            &mut eval,
            "(count (apply conj (vector-of :int) (range 40)))"
        ),
        "40"
    );
    // g-nth-float 20 (`nth` coerces `Double`s).
    assert_eq!(
        run_last(&mut eval, "(nth (vector-of :int 10 20) 1.0)"),
        "20"
    );
    // g-get-float nil (like vectors: `Double`s are not keys).
    assert_eq!(
        run_last(&mut eval, "(get (vector-of :int 10 20) 1.0)"),
        "nil"
    );
    // g-assoc-end [1 9] (at-count appends) / g-assoc-oob.
    assert_eq!(
        run_last(&mut eval, "(assoc (vector-of :int 1) 1 9)"),
        "[1 9]"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (vector-of :int 1) 5 9)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // g-dissoc: `Vec` is not a map.
    assert_eq!(
        uncaught(&mut eval, "(dissoc (vector-of :int 1) 0)"),
        "java.lang.ClassCastException | class clojure.core.Vec cannot be cast to class clojure.lang.IPersistentMap (clojure.core.Vec and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
    // g-conj-multi [1 2 3].
    assert_eq!(
        run_last(&mut eval, "(conj (vector-of :int) 1 2 3)"),
        "[1 2 3]"
    );
    // g-equiv-str false.
    assert_eq!(run_last(&mut eval, "(= (vector-of :int 1) \"a\")"), "false");
    // f-vo-ifn / f-vo-invoke / f-vo-assoc / f-vo-coll / f-vo-seq.
    assert_eq!(run_last(&mut eval, "(ifn? (vector-of :int 1))"), "true");
    assert_eq!(run_last(&mut eval, "((vector-of :int 10 20) 1)"), "20");
    assert_eq!(
        run_last(&mut eval, "(associative? (vector-of :int 1))"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(coll? (vector-of :int 1))"), "true");
    assert_eq!(run_last(&mut eval, "(seq? (vector-of :int 1))"), "false");
    // g-vo-fkey / g-vo-skey: the `isInteger` gate.
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) 1.0)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) \"k\")"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    // g-vo-bigkey 20 (unlike the port's vectors, per the oracle).
    assert_eq!(run_last(&mut eval, "((vector-of :int 10 20) 1N)"), "20");
    // g-vo-hugekey: the key coerces through core-`int`.
    assert_eq!(
        uncaught(
            &mut eval,
            "((vector-of :int 10 20) 9999999999999999999999N)"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 9999999999999999999999"
    );
    // g-vo-negkey: negatives are OOB.
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) -1)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // g-vo-arity2 DIVERGES (pinned): the JVM throws
    // `AbstractMethodError` for the missing overload; the port
    // pins `ArityException`, promise precedent.
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) 0 :nf)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core.Vec"
    );
    // a-vo-huge: init elements coerce through core-`int`.
    assert_eq!(
        uncaught(&mut eval, "(vector-of :int 9999999999)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // Port pins: `get`/`assoc`/`contains?` mirror the port's
    // vectors (`int` keys only) even though the JVM `Vec`
    // accepts `BigInt` keys via `isInteger` (ad-hoc `get-vo-big`
    // → 6 on the JVM) — the `invoke` gate is the spec's only
    // `BigInt` exception.
    assert_eq!(run_last(&mut eval, "(get (vector-of :int 5 6) 1N)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(assoc (vector-of :int 1) 1N 2)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (vector-of :int 5) 0N)"),
        "false"
    );
    // Port pin (PR #72 H3): `merge-with` over a PV
    // right-hand side folds index/value pairs.
    assert_eq!(
        run_last(&mut eval, "(merge-with + {0 100} (vector-of :int 1 2))"),
        "{0 101, 1 2}"
    );
    // Port pins (PR #72 H4): `conj` of a PV onto a map
    // assocs pairs; non-pairs are the pair error.
    assert_eq!(
        run_last(&mut eval, "(conj {} (vector-of :int 1 2))"),
        "{1 2}"
    );
    assert_eq!(
        uncaught(&mut eval, "(conj {} (vector-of :int 1 2 3))"),
        "java.lang.IllegalArgumentException | Vector arg to map conj must be a pair"
    );
    // Port pin (PR #72 H6): negative `assoc` on a PV is the
    // empty `IOOBE`.
    assert_eq!(
        uncaught(&mut eval, "(assoc (vector-of :int 1) -1 9)"),
        "java.lang.IndexOutOfBoundsException | "
    );
}

// ---- `seq`/`nth`/`get`/`count` over arrays (`interop.out` §A/B, `match.out` §E, `scalar.out` §B/C) ----

#[test]
fn seq_coll_arrays() {
    let mut eval = boot();
    // a-seq (1 2) / a-seq-empty nil / a-seq-obj ("a" "b").
    assert_eq!(run_last(&mut eval, "(seq (int-array [1 2]))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(seq (int-array []))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(seq (to-array [\"a\" \"b\"]))"),
        "(\"a\" \"b\")"
    );
    // a-seq-2d: the inners (the probe maps `class` — S16; the
    // port pins the same shape through `alength`).
    assert_eq!(
        run_last(&mut eval, "(map alength (seq (make-array :int 2 2)))"),
        "(2 2)"
    );
    // D1: `seq` snapshots (the JVM's `ArraySeq` is lazy; the port
    // walks a snapshot — later mutation is invisible).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2]) s (seq a)] (aset a 0 9) s)"
        ),
        "(1 2)"
    );
    // a-last 3 / a-nthrest (3) / a-map (2 3) / a-reduce 6.
    assert_eq!(run_last(&mut eval, "(last (int-array [1 2 3]))"), "3");
    assert_eq!(
        run_last(&mut eval, "(nthrest (int-array [1 2 3]) 2)"),
        "(3)"
    );
    assert_eq!(run_last(&mut eval, "(map inc (int-array [1 2]))"), "(2 3)");
    assert_eq!(run_last(&mut eval, "(reduce + (int-array [1 2 3]))"), "6");
    // a-nth 2 / a-nth-nf :nf / a-nth-oob (empty `AIOOBE`).
    assert_eq!(run_last(&mut eval, "(nth (int-array [1 2]) 1)"), "2");
    assert_eq!(run_last(&mut eval, "(nth (int-array [1 2]) 9 :nf)"), ":nf");
    assert_eq!(
        uncaught(&mut eval, "(nth (int-array [1 2]) 9)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // e-nth-float 20 / n-arr-str / n-arr-nil.
    assert_eq!(run_last(&mut eval, "(nth (int-array [10 20]) 1.0)"), "20");
    assert_eq!(
        uncaught(&mut eval, "(nth (int-array [10 20]) \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(nth (int-array [10 20]) nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // b-get / b-contains.
    assert_eq!(run_last(&mut eval, "(get (int-array [10 20]) 1)"), "20");
    assert_eq!(
        run_last(&mut eval, "(contains? (int-array [10 20]) 1)"),
        "true"
    );
    // e-get-float 20 / e-get-bigint 20 (`intValue` narrows all `Number`s).
    assert_eq!(run_last(&mut eval, "(get (int-array [10 20]) 1.0)"), "20");
    assert_eq!(run_last(&mut eval, "(get (int-array [10 20]) 1N)"), "20");
    // f-get-str nil / f-get-nil nil / f-get-neg :nf / f-get-obj nil.
    assert_eq!(run_last(&mut eval, "(get (int-array [1]) \"k\")"), "nil");
    assert_eq!(run_last(&mut eval, "(get (int-array [1]) nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(get (int-array [1]) -1 :nf)"), ":nf");
    assert_eq!(run_last(&mut eval, "(get (to-array [1]) \"k\")"), "nil");
    // f-contains-oob false / f-contains-neg false / f-contains-str.
    assert_eq!(
        run_last(&mut eval, "(contains? (int-array [1]) 9)"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(contains? (int-array [1]) -1)"),
        "false"
    );
    assert_eq!(
        uncaught(&mut eval, "(contains? (int-array [1]) \"k\")"),
        "java.lang.IllegalArgumentException | contains? not supported on type: [I"
    );
    // e-contains-nil: even nil keys are unsupported.
    assert_eq!(
        uncaught(&mut eval, "(contains? (int-array [1]) nil)"),
        "java.lang.IllegalArgumentException | contains? not supported on type: [I"
    );
    // b-empty? true / b-empty?-full false.
    assert_eq!(run_last(&mut eval, "(empty? (int-array []))"), "true");
    assert_eq!(run_last(&mut eval, "(empty? (int-array [1]))"), "false");
    // b-empty nil.
    assert_eq!(run_last(&mut eval, "(empty (int-array [1]))"), "nil");
    // b-conj / b-assoc / b-rseq / b-with-meta / b-compare.
    assert_eq!(
        uncaught(&mut eval, "(conj (int-array [1]) 2)"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IPersistentCollection ([I is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentCollection is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(assoc (int-array [1]) 0 9)"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.Associative ([I is in module java.base of loader 'bootstrap'; clojure.lang.Associative is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(rseq (int-array [1 2]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.Reversible ([I is in module java.base of loader 'bootstrap'; clojure.lang.Reversible is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-meta (int-array [1]) {:a 1})"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IObj ([I is in module java.base of loader 'bootstrap'; clojure.lang.IObj is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(compare (int-array [1]) (int-array [1]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class java.lang.Comparable ([I and java.lang.Comparable are in module java.base of loader 'bootstrap')"
    );
    // b-compare-vec (the probe catches `Throwable` and prints the
    // class name — `class` is S16; the port catches the name).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (compare (int-array [1]) [1]) (catch ClassCastException t :cce))"
        ),
        ":cce"
    );
    // Ad-hoc coll corners (all `CCE`s, oracle-verbatim).
    assert_eq!(
        uncaught(&mut eval, "(peek (int-array [1 2]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IPersistentStack ([I is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentStack is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(pop (int-array [1 2]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IPersistentStack ([I is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentStack is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(dissoc (int-array [1 2]) 0)"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IPersistentMap ([I is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(transient (int-array [1 2]))"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IEditableCollection ([I is in module java.base of loader 'bootstrap'; clojure.lang.IEditableCollection is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(subvec (int-array [1 2]) 0 1)"),
        "java.lang.ClassCastException | class [I cannot be cast to class clojure.lang.IPersistentVector ([I is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(transient (vector-of :int 1))"),
        "java.lang.ClassCastException | class clojure.core.Vec cannot be cast to class clojure.lang.IEditableCollection (clojure.core.Vec and clojure.lang.IEditableCollection are in unnamed module of loader 'app')"
    );
    // Ad-hoc `replace-arr`: non-vectors take the lazy path.
    assert_eq!(
        run_last(&mut eval, "(replace {1 9} (int-array [1 2]))"),
        "(9 2)"
    );
    // `replace` over a primitive vector stays typed (`vector?`).
    assert_eq!(
        run_last(&mut eval, "(replace {1 9} (vector-of :int 1 2))"),
        "[9 2]"
    );
    // c-hash-int? / c-hash-same / c-hash-mut / c-hash-distinct?
    // (`hash` is the stable id hash).
    assert_eq!(
        run_last(&mut eval, "(integer? (hash (int-array [1 2])))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(let [a (int-array [1])] (= (hash a) (hash a)))"),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1]) h1 (hash a)] (aset a 0 2) (= h1 (hash a)))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1]) b (aclone a)] (= (hash a) (hash b)))"
        ),
        "false"
    );
    // D4: nil arrays always take the stable overload errors (the
    // JVM's inner/runtime-nil NPEs vary run to run —
    // `e-aset-nil-multi`, `g-nil-aget`, `g-nil-alen`, `g-nil-aset`
    // all pin the stable rule here).
    assert_eq!(
        uncaught(&mut eval, "(aset nil 0 0 9)"),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [x (first [])] (aget x 0))"),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [x (first [])] (alength x))"),
        "java.lang.IllegalArgumentException | More than one matching method found: alength"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [x (first [])] (aset x 0 5))"),
        "java.lang.IllegalArgumentException | More than one matching method found: aset"
    );
    // `meta` of an array is nil.
    assert_eq!(run_last(&mut eval, "(meta (int-array [1]))"), "nil");
    // `find` on arrays names the descriptor.
    assert_eq!(
        uncaught(&mut eval, "(find (long-array [1]) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [J"
    );
}

// ---- `amap` / `areduce` (`interop.out` §E) ----

#[test]
fn amap_areduce() {
    let mut eval = boot();
    // e-amap (2 4 6) / e-amap-long (11 12).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2 3])] (seq (amap a i ret (* 2 (aget a i)))))"
        ),
        "(2 4 6)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1 2])] (seq (amap a i ret (+ 10 (aget a i)))))"
        ),
        "(11 12)"
    );
    // e-amap-cls: `amap` keeps the element type (the probe uses
    // `class`; the port pins behaviorally — still an `int[]`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (int-array [1])] (aset (amap a i ret i) 0 \"s\"))"
        ),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // e-amap-orig (1 2) — the input is untouched (clone loop).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2])] (amap a i ret (* 2 (aget a i))) (seq a))"
        ),
        "(1 2)"
    );
    // e-amap-nonarray: `alength` fails first.
    assert_eq!(
        uncaught(&mut eval, "(amap [1 2] i ret i)"),
        "java.lang.IllegalArgumentException | No matching method alength found taking 1 args"
    );
    // e-areduce 10 / e-areduce2 4.0.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2 3 4])] (areduce a i ret 0 (+ ret (aget a i))))"
        ),
        "10"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1.5 2.5])] (areduce a i ret 0.0 (+ ret (aget a i))))"
        ),
        "4.0"
    );
    // e-expand-amap / e-expand-areduce: the expansion matches the
    // JVM shape modulo gensyms (`G__n` here, `__auto__` there)
    // and `inc` for `unchecked-inc`.
    let amap = run_last(
        &mut eval,
        "(pr-str (macroexpand-1 '(amap arr idx ret (* 2 (aget ret idx)))))",
    );
    for token in [
        "(clojure.core/let [",
        " arr ",
        "(clojure.core/alength ",
        "(clojure.core/aclone ",
        "(clojure.core/loop [idx 0]",
        "(if (clojure.core/< idx ",
        "(clojure.core/aset ret idx (* 2 (aget ret idx)))",
        "(recur (clojure.core/inc idx))",
    ] {
        assert!(amap.contains(token), "missing {token} in {amap}");
    }
    let areduce = run_last(
        &mut eval,
        "(pr-str (macroexpand-1 '(areduce arr idx ret 0 (+ ret (aget arr idx)))))",
    );
    for token in [
        "(clojure.core/let [",
        " arr ",
        "(clojure.core/alength ",
        "(clojure.core/loop [idx 0 ret 0]",
        "(if (clojure.core/< idx ",
        "(recur (clojure.core/inc idx) (+ ret (aget arr idx)))",
    ] {
        assert!(areduce.contains(token), "missing {token} in {areduce}");
    }
}

// ---- class shapes without `class` (`ctors.out` §H — S16 owns `class`/`type`) ----

#[test]
fn descriptors() {
    let mut eval = boot();
    // h-int / h-long / h-bool / h-byte / h-char / h-short /
    // h-float / h-double / h-object: each ctor's descriptor,
    // surfaced through `find`'s error text.
    for (make, desc) in [
        ("(boolean-array 2)", "[Z"),
        ("(byte-array 2)", "[B"),
        ("(char-array 2)", "[C"),
        ("(short-array 2)", "[S"),
        ("(int-array 2)", "[I"),
        ("(long-array 2)", "[J"),
        ("(float-array 2)", "[F"),
        ("(double-array 2)", "[D"),
        ("(object-array 2)", "[Ljava.lang.Object;"),
    ] {
        assert_eq!(
            uncaught(&mut eval, &format!("(find {make} 0)")),
            format!("java.lang.IllegalArgumentException | find not supported on type: {desc}"),
            "{make}"
        );
    }
    // h-to / h-obj2d: `to-array` shapes.
    assert_eq!(
        uncaught(&mut eval, "(find (to-array [1]) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [Ljava.lang.Object;"
    );
    assert_eq!(
        uncaught(&mut eval, "(find (to-array-2d [[1]]) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [[Ljava.lang.Object;"
    );
    // h-string: inference narrows to `[Ljava.lang.String;`.
    assert_eq!(
        uncaught(&mut eval, "(find (into-array [\"a\"]) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [Ljava.lang.String;"
    );
    // h-int2d / h-comp-2d / h-name-2d: nesting descriptors.
    assert_eq!(
        uncaught(&mut eval, "(find (make-array :int 2 3) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [[I"
    );
    // h-boxed-int / h-boxed-byte / h-boxed-char / h-boxed-bool /
    // h-boxed-float / h-boxed-short: reads collapse narrow
    // wrappers to `Int`/`Double` (port-wide posture, pinned
    // through predicates).
    assert_eq!(
        run_last(&mut eval, "(integer? (aget (int-array [7]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (aget (byte-array [7]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(char? (aget (char-array [\\a]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(boolean? (aget (boolean-array [true]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(float? (aget (float-array [1.5]) 0))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(integer? (aget (short-array [7]) 0))"),
        "true"
    );
    // b-float-inf DIVERGES (D3): the JVM narrows out-of-`f32`
    // magnitudes to `Inf`; the port keeps the exact `f64`.
    assert_eq!(
        run_last(&mut eval, "(seq (float-array [1e300]))"),
        "(1.0E300)"
    );
    // b-long-huge / b-int-hugedbl: fills saturate.
    assert_eq!(
        run_last(&mut eval, "(seq (long-array [1e300]))"),
        "(9223372036854775807)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (int-array [1e300]))"),
        "(2147483647)"
    );
    // b-short-wrap / b-int-wrap / b-byte-neg: fills narrow.
    assert_eq!(
        run_last(&mut eval, "(seq (short-array [40000]))"),
        "(-25536)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (int-array [3000000000]))"),
        "(-1294967296)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (byte-array [-1 -129]))"),
        "(-1 127)"
    );
    // e-short-huge / e-byte-huge: `(short)(int)` / `(byte)(int)`.
    assert_eq!(run_last(&mut eval, "(seq (short-array [1e300]))"), "(-1)");
    assert_eq!(run_last(&mut eval, "(seq (byte-array [1e300]))"), "(-1)");
    // c-vo-meta / c-vec-meta: `^:meta` prints on vectors.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*print-meta* true] (pr-str (with-meta (vector-of :int 1 2) {:a 1})))"
        ),
        "\"^{:a 1} [1 2]\""
    );
}

// ---- allocation guards (port behavior) ----

#[test]
fn oom_guards() {
    let mut eval = boot();
    // Past 256 MiB of payload → `OutOfMemoryError` (no allocation
    // attempted).
    assert_eq!(
        uncaught(&mut eval, "(make-array :byte 300000000)"),
        "java.lang.OutOfMemoryError | "
    );
    // The guard runs over the full dims product.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int 100000 100000)"),
        "java.lang.OutOfMemoryError | "
    );
    // A product overflow is out of memory too.
    assert_eq!(
        uncaught(
            &mut eval,
            "(make-array :long 1000000000 1000000000 1000000000)"
        ),
        "java.lang.OutOfMemoryError | "
    );
    // Size-form ctors guard as well.
    assert_eq!(
        uncaught(&mut eval, "(int-array 100000000)"),
        "java.lang.OutOfMemoryError | "
    );
    // Just under the guard still builds.
    assert_eq!(
        run_last(&mut eval, "(alength (make-array :byte 1000000))"),
        "1000000"
    );
    // Negative dims fail before the guard.
    assert_eq!(
        uncaught(&mut eval, "(make-array :int -1 1000000000)"),
        "java.lang.NegativeArraySizeException | -1"
    );
}

// ---- predicates over the new kinds (`scalar.out` §F) ----

#[test]
fn predicates() {
    let mut eval = boot();
    // Arrays: only `seqable?` is true (`b-coll?`, `b-counted?`,
    // `b-indexed?`, `b-list?`, `b-reversible?`, `b-seqable?`,
    // `b-sequential?`, `b-vector?`).
    for (pred, want) in [
        ("seqable?", "true"),
        ("counted?", "false"),
        ("sequential?", "false"),
        ("vector?", "false"),
        ("associative?", "false"),
        ("ifn?", "false"),
        ("coll?", "false"),
        ("list?", "false"),
        ("map?", "false"),
        ("set?", "false"),
        ("indexed?", "false"),
        ("reversible?", "false"),
        ("seq?", "false"),
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("({pred} (int-array [1]))")),
            want,
            "{pred}"
        );
    }
    // f-arr-assoc / f-arr-ifn (explicit).
    assert_eq!(
        run_last(&mut eval, "(associative? (int-array [1]))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(ifn? (int-array [1]))"), "false");
    // Primitive vectors: the `Vec` interface set (`f-indexed?`,
    // `f-reversible?`, `f-seqable?`, `f-sequential?`, `f-vector?`
    // and the rest).
    for (pred, want) in [
        ("vector?", "true"),
        ("sequential?", "true"),
        ("associative?", "true"),
        ("counted?", "true"),
        ("indexed?", "true"),
        ("reversible?", "true"),
        ("seqable?", "true"),
        ("coll?", "true"),
        ("ifn?", "true"),
        ("list?", "false"),
        ("map?", "false"),
        ("set?", "false"),
        ("seq?", "false"),
        ("fn?", "false"),
        ("record?", "false"),
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("({pred} (vector-of :int 1))")),
            want,
            "{pred}"
        );
    }
}

// ---- the new throwables catch by name (and by parent) ----

#[test]
fn catch_names() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(try (nth (int-array [1]) 5) (catch ArrayIndexOutOfBoundsException e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (nth (int-array [1]) 5) (catch IndexOutOfBoundsException e :caught-parent))"
        ),
        ":caught-parent"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (aset (into-array [\"x\"]) 0 1) (catch ArrayStoreException e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (int-array -1) (catch NegativeArraySizeException e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (make-array :byte 300000000) (catch OutOfMemoryError e :caught))"
        ),
        ":caught"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (make-array :byte 300000000) (catch Error e :caught-parent))"
        ),
        ":caught-parent"
    );
}

// ---- end-to-end stories ----

#[test]
fn stories() {
    let mut eval = boot();
    // Fill, map, reduce: the `amap`/`areduce` pipeline over a
    // cloned array leaves the input untouched.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2 3 4])] (let [b (amap a i ret (* 2 (aget a i)))] [(seq a) (areduce b i r 0 (+ r (aget b i)))]))"
        ),
        "[(1 2 3 4) 20]"
    );
    // `into-array` narrows, `to-array` boxes back, `aclone`
    // isolates.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (into-array [1 2]) b (aclone a)] (aset b 0 9) [(seq a) (seq (to-array b))])"
        ),
        "[(1 2) (9 2)]"
    );
    // `vector-of` round-trips through `into-array` and `seq`.
    assert_eq!(
        run_last(
            &mut eval,
            "(seq (into-array :long (seq (vector-of :int 1 2))))"
        ),
        "(1 2)"
    );
}

// ---- remaining probe labels (one home for the stragglers) ----

#[test]
fn remaining_labels() {
    let mut eval = boot();
    // a-neg-idx / a-oob-neg: negatives keep the `Index` text.
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1 2]) -1)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index -1 out of bounds for length 2"
    );
    // a-str: strings are not arrays.
    assert_eq!(
        uncaught(&mut eval, "(aget \"ab\" 0)"),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    // a-arity1 / b-arity2.
    assert_eq!(
        uncaught(&mut eval, "(aget (int-array [1]))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/aget"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 0)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/aset"
    );
    // b-strval-prim / b-nilval-prim / b-boolval-int: no-match.
    for src in [
        "(aset (int-array [1]) 0 \"a\")",
        "(aset (int-array [1]) 0 nil)",
        "(aset (int-array [1]) 0 true)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method aset found taking 3 args",
            "{src}"
        );
    }
    // a-count 5 / a-first 7 / a-rest (8) / a-next-empty nil.
    assert_eq!(run_last(&mut eval, "(count (long-array 5))"), "5");
    assert_eq!(run_last(&mut eval, "(first (int-array [7 8]))"), "7");
    assert_eq!(run_last(&mut eval, "(rest (int-array [7 8]))"), "(8)");
    assert_eq!(run_last(&mut eval, "(next (int-array [7]))"), "nil");
    // b-get-nf :nf / b-find / b-meta (nil).
    assert_eq!(run_last(&mut eval, "(get (int-array [10]) 5 :nf)"), ":nf");
    assert_eq!(
        uncaught(&mut eval, "(find (int-array [10]) 0)"),
        "java.lang.IllegalArgumentException | find not supported on type: [I"
    );
    assert_eq!(run_last(&mut eval, "(meta (int-array [1]))"), "nil");
    // b-setint-didx (1 9) — `Double` tails truncate.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1 2])] (aset-int a 1.0 9) (seq a))"
        ),
        "(1 9)"
    );
    // b-setbool-2d: nesting fails the primitive check.
    assert_eq!(
        uncaught(&mut eval, "(aset-boolean (make-array :boolean 1 1) 0 true)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    // b-bool-nil / b-double-nil / e-char-nil: fill NPEs.
    assert_eq!(
        uncaught(&mut eval, "(boolean-array [nil])"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Boolean.booleanValue()\" because the return value of \"clojure.lang.ISeq.first()\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(double-array [nil])"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because the return value of \"clojure.lang.ISeq.first()\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(char-array [nil])"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because the return value of \"clojure.lang.ISeq.first()\" is null"
    );
    // b-int-bool / b-long-str: fill CCEs.
    assert_eq!(
        uncaught(&mut eval, "(int-array [true])"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.lang.Number (java.lang.Boolean and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(long-array [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // c-int-str / c-int-nil: `aset-int` vals coerce through core-`int`.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) 0 \"a\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset-int (int-array [1]) 0 nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    // c-short-str: `into-array` shorts route through `longCast`.
    assert_eq!(
        uncaught(&mut eval, "(into-array :short [\"a\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // c-ident-same true / c-ident-diff false / c-ident-obj false /
    // c-vs-seq false / c-vs-vec false (`=` is identity).
    assert_eq!(
        run_last(&mut eval, "(let [a (int-array [1])] (= a a))"),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (int-array [1]) (int-array [1]))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(= (to-array [1]) (to-array [1]))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(= (int-array [1 2]) (seq [1 2]))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(= (to-array [1 2]) [1 2])"), "false");
    // e-int-1d (0 0 0) / e-long-1d / e-bool-1d / e-char-1d /
    // e-double-1d / e-object-1d / e-string-1d / e-int-2d-len.
    assert_eq!(run_last(&mut eval, "(seq (make-array :int 3))"), "(0 0 0)");
    assert_eq!(run_last(&mut eval, "(seq (make-array :long 2))"), "(0 0)");
    assert_eq!(
        run_last(&mut eval, "(seq (make-array :boolean 2))"),
        "(false false)"
    );
    assert_eq!(
        run_last(&mut eval, "(map int (make-array :char 2))"),
        "(0 0)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (make-array :double 2))"),
        "(0.0 0.0)"
    );
    assert_eq!(
        run_last(&mut eval, "(seq (make-array :object 2))"),
        "(nil nil)"
    );
    // e-string-1d (nil nil) — the probe uses `String`; any ref
    // type reads nils (the port's `:object` stands in).
    assert_eq!(
        run_last(&mut eval, "(seq (make-array :object 2))"),
        "(nil nil)"
    );
    assert_eq!(run_last(&mut eval, "(alength (make-array :long 2 3))"), "2");
    // f-conj-coerce [1 2] (truncation).
    assert_eq!(
        run_last(&mut eval, "(conj (vector-of :int 1) 2.5)"),
        "[1 2]"
    );
    // f-first 7 / f-map (2 3) / f-reduce 6.
    assert_eq!(run_last(&mut eval, "(first (vector-of :int 7 8))"), "7");
    assert_eq!(
        run_last(&mut eval, "(map inc (vector-of :int 1 2))"),
        "(2 3)"
    );
    assert_eq!(
        run_last(&mut eval, "(reduce + (vector-of :long 1 2 3))"),
        "6"
    );
    // g-vo-strkey / g-vo-nilkey / g-vo-oobkey: the invoke gate.
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) \"a\")"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) nil)"),
        "java.lang.IllegalArgumentException | Key must be integer"
    );
    assert_eq!(
        uncaught(&mut eval, "((vector-of :int 10 20) 5)"),
        "java.lang.IndexOutOfBoundsException | "
    );
    // a-int-short (7) / a-int-byte (7) / a-long-int (7) /
    // a-long-short (7) / a-double-float (2.5): the port's erased
    // values store through the same arms (same outcomes).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] (aset a 0 (short 7)) (seq a))"
        ),
        "(7)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (int-array [1])] (aset a 0 (byte 7)) (seq a))"
        ),
        "(7)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] (aset a 0 (int 7)) (seq a))"
        ),
        "(7)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (long-array [1])] (aset a 0 (short 7)) (seq a))"
        ),
        "(7)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (double-array [1])] (aset a 0 (float 2.5)) (seq a))"
        ),
        "(2.5)"
    );
    // a-short-byte / a-byte-short DIVERGE (D2): in-range `Int`s
    // store (the forms answer the val).
    assert_eq!(
        run_last(&mut eval, "(aset (short-array [1]) 0 (byte 7))"),
        "7"
    );
    assert_eq!(
        run_last(&mut eval, "(aset (byte-array [1]) 0 (short 7))"),
        "7"
    );
    // a-long-bigint / a-double-bigint: no-match.
    for src in [
        "(aset (long-array [1]) 0 1N)",
        "(aset (double-array [1]) 0 5N)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | No matching method aset found taking 3 args",
            "{src}"
        );
    }
    // d-pr-bool / d-pr-byte / d-pr-char / d-pr-long / d-pr-empty /
    // d-pr-objarr (tag and rep quoted).
    for (src, desc) in [
        ("(boolean-array [true])", "[Z"),
        ("(byte-array [1])", "[B"),
        ("(char-array [\\a])", "[C"),
        ("(long-array [1])", "[J"),
        ("(int-array [])", "[I"),
        ("(object-array [1])", "[Ljava.lang.Object;"),
    ] {
        assert_eq!(
            run_norm(&mut eval, &format!("(pr-str {src})")),
            format!("\"#object[\\\"{desc}\\\" 0xH \\\"{desc}@L\\\"]\""),
            "{src}"
        );
    }
    // d-pr-in-vec (nested, quoted through `pr-str`).
    assert_eq!(
        run_norm(&mut eval, "(pr-str [(int-array [1]) (to-array [1])])"),
        "\"[#object[\\\"[I\\\" 0xH \\\"[I@L\\\"] #object[\\\"[Ljava.lang.Object;\\\" 0xH \\\"[Ljava.lang.Object;@L\\\"]]\""
    );
}

#[test]
fn mi3d_walk_and_order() {
    let mut eval = boot();
    // m4-idx1 / m4-idx2: walk idxs fail to `Character`.
    for src in [
        "(aget (make-array :int 2 2 2) \"x\" 0 0)",
        "(aget (make-array :int 2 2 2) 0 \"x\" 0)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')",
            "{src}"
        );
    }
    // m4-idx3: the tail idx fails to `Number`.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2 2) 0 0 \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // m4-idx2dbl / m4-idx3dbl / m3-idx1dbl: the long stop.
    for src in [
        "(aget (make-array :int 2 2 2) 0 1e300 0)",
        "(aget (make-array :int 2 2 2) 0 0 1e300)",
        "(aget (make-array :int 2 2) 1e300 0)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300",
            "{src}"
        );
    }
    // m3-idx1objdbl DIVERGES (D6): the port's `int` is one rule
    // (the `int` stop), while the JVM routes `Object`-typed
    // doubles through `long` (statically unknowable here).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [x (identity 1e300)] (aget (int-array [1 2]) x))"
        ),
        "java.lang.IllegalArgumentException | Value out of range for int: 1.0E300"
    );
    // m4-mid-scalar / m3-str-first: middles match like direct.
    assert_eq!(
        uncaught(
            &mut eval,
            "(aget (object-array [(object-array [42])]) 0 0 0 0)"
        ),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget \"s\" 0 0)"),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    // m4-mid-nil / m3-nil-first DIVERGE (D4): the JVM runs the
    // nil through the overload frames (`Cannot load from double
    // array because "xs" is null` — method-order dependent);
    // the port pins the stable more-than-one text.
    for src in [
        "(let [a (object-array [(object-array [1]) nil])] (aget a 1 0 0))",
        "(aget nil 0 0)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.IllegalArgumentException | More than one matching method found: aget",
            "{src}"
        );
    }
    // m3-mid-nil: the last step is the body rule.
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (object-array [(object-array [1]) nil])] (aget a 1 0))"
        ),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.getComponentType()\" because the return value of \"clojure.core$class.invokeStatic(Object)\" is null"
    );
    // ms-idx2 / ms-idx3: `aset` walks like `aget`.
    for src in [
        "(aset (make-array :int 2 2 2) 0 \"x\" 0 0 7)",
        "(aset (make-array :int 2 2 2) 0 0 \"x\" 0 7)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')",
            "{src}"
        );
    }
    // ms-mid-scalar names `aget` (the walk); ms-mid-nil DIVERGES
    // (D4, like `m4-mid-nil`); ms-ok stores.
    assert_eq!(
        uncaught(
            &mut eval,
            "(aset (object-array [(object-array [42])]) 0 0 0 0 7)"
        ),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (object-array [(object-array [1]) nil])] (aset a 1 0 0 7))"
        ),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    // ms-ok stores (on `:long`, where the tail rule accepts
    // the port's `Int` — on `:int` this form is D5).
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :long 2 2 2)] (aset a 0 0 0 9) (aget a 0 0 0))"
        ),
        "9"
    );
    // ord-int-huge-oob: coercion beats bounds (AE, not `Index`).
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 5 3000000000)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    // ord-int-ok-oob / ord-short-ok-oob: the `Index` text.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 5 7)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    assert_eq!(
        uncaught(&mut eval, "(aset (short-array [1]) 5 (short 7))"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    // ord-short-long-oob DIVERGES (D2): the port shape-matches
    // the `Int`, so the range error beats bounds (the JVM never
    // matches and reports no-match).
    assert_eq!(
        uncaught(&mut eval, "(aset (short-array [1]) 5 100000)"),
        "java.lang.IllegalArgumentException | Value out of range for short: 100000"
    );
    // ord-byte-long-oob DIVERGES (D2): in-range, so bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset (byte-array [1]) 5 7)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 1"
    );
    // tail-oob-get / tail-oob-set: the empty `AIOOBE`.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 0 5)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :object 1 1) 0 5 :v)"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // rt-nil-aget / rt-nil-am3 / rt-nil-aset DIVERGE (D4): a nil
    // through the run-time reflector hits overload frames on the
    // JVM (`Cannot load/store ... because "xs" is null`); the
    // port pins the stable more-than-one text.
    for (src, want) in [
        (
            "(let [n (identity nil)] (aget n 0))",
            "java.lang.IllegalArgumentException | More than one matching method found: aget",
        ),
        (
            "(let [n (identity nil)] (aget n 0 0))",
            "java.lang.IllegalArgumentException | More than one matching method found: aget",
        ),
        (
            "(let [n (identity nil)] (aset n 0 7))",
            "java.lang.IllegalArgumentException | More than one matching method found: aset",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
    // apply-aget-str / apply-aget-idx / apply-aget-nil /
    // apply-aset-val: the port does not distinguish `apply`
    // (fn bodies) from direct (inlines) calls — known gap (the
    // JVM answers the body rule: `Argument is not an array`,
    // `CCE→Number`, the `getComponentType` NPE, `argument type
    // mismatch`). The port pins its direct-rule answers.
    assert_eq!(
        uncaught(&mut eval, "(apply aget [\"s\" 0])"),
        "java.lang.IllegalArgumentException | No matching method aget found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(apply aget [(int-array [1]) \"x\"])"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(apply aget [nil 0])"),
        "java.lang.IllegalArgumentException | More than one matching method found: aget"
    );
    assert_eq!(
        run_last(&mut eval, "(apply aset [(int-array [1]) 0 5])"),
        "5"
    );
    // m4-charidx / m4-nilidx / m4-bigidx / m4-negidx.
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2 2) 0 \\a 0)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 97 out of bounds for length 2"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2 2) 0 nil 0)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) 5N 0)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index 5 out of bounds for length 2"
    );
    assert_eq!(
        uncaught(&mut eval, "(aget (make-array :int 2 2) -1 0)"),
        "java.lang.ArrayIndexOutOfBoundsException | Index -1 out of bounds for length 2"
    );
    // ix-str-idx: idx failures beat array-match failures.
    assert_eq!(
        uncaught(&mut eval, "(aget \"s\" \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // ix-aset-idx: direct idx failures beat array matching.
    assert_eq!(
        uncaught(&mut eval, "(aset \"s\" \"x\" 9)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // ix-order: shape mismatches beat bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset (int-array [1]) 5 \"v\")"),
        "java.lang.IllegalArgumentException | No matching method aset found taking 3 args"
    );
    // ix-setint-order: the primitive check beats bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset-int (object-array [1]) 5 1)"),
        "java.lang.IllegalArgumentException | Argument is not an array of primitive type"
    );
    // ix-body: the tail idx unbox beats the nil check.
    assert_eq!(
        uncaught(&mut eval, "(aset (object-array [nil]) 0 \"x\" 1)"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')"
    );
    // ix-nil-literal DIVERGES: a nil array literal plus any idx
    // is a JVM compile error (`CompilerException` caused by
    // more-than-one); the port has no compiler and pins the
    // idx-first run-time answer.
    assert_eq!(
        uncaught(&mut eval, "(aget nil \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // ia-int-hugedbl / ia-short-hugedbl / ia-char-70000: the
    // `into-array` casts take the long stop.
    for (src, want) in [
        (
            "(into-array :int [1e300])",
            "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300",
        ),
        (
            "(into-array :short [1e300])",
            "java.lang.IllegalArgumentException | Value out of range for long: 1.0E300",
        ),
        (
            "(into-array :char [70000])",
            "java.lang.IllegalArgumentException | Value out of range for char: 70000",
        ),
    ] {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
    // setx-both-bad / setx-badidx-nilarr: idx beats val.
    for src in [
        "(aset-int (int-array [1]) \"x\" \"y\")",
        "(aset-int nil \"x\" 1)",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Number (java.lang.String and java.lang.Number are in module java.base of loader 'bootstrap')",
            "{src}"
        );
    }
    // setx-badval-nilarr: val coercion beats the nil check.
    assert_eq!(
        uncaught(&mut eval, "(aset-int nil 0 \"y\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.lang.Character (java.lang.String and java.lang.Character are in module java.base of loader 'bootstrap')"
    );
    // tail-oob-mismatch: bounds beat the unbox miss.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 1 1) 0 5 \"s\")"),
        "java.lang.ArrayIndexOutOfBoundsException | "
    );
    // tail-oob-nilprim: nil-into-primitive beats bounds.
    assert_eq!(
        uncaught(&mut eval, "(aset (make-array :int 1 1) 0 5 nil)"),
        "java.lang.IllegalArgumentException | "
    );
    // tail-ref-mismatch: refs miss with the `Array.set` text
    // (via a primitive-inner 2-d tail — the port's `:object`
    // inners fit everything, so the inner-descriptor arm stands
    // in for the JVM's narrowed `String` inner).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [a (make-array :int 1 1 1)] (aset a 0 0 (long-array [1])))"
        ),
        "java.lang.IllegalArgumentException | array element type mismatch"
    );
    // tail-ref-nil: nil fits ref tails.
    assert_eq!(
        run_last(
            &mut eval,
            "(let [a (make-array :object 1 1)] (aset a 0 0 nil) (seq (aget a 0)))"
        ),
        "(nil)"
    );
}
