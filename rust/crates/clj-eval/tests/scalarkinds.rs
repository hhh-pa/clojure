// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S9 scalar kinds: probe replay for `parse-uuid` /
//! `random-uuid` / `uuid?` / `uri?` / `Inst` / `inst-ms` /
//! `inst-ms*` / `inst?` (see
//! `docs/zero-oos-s9-scalarkinds/probes/*.{clj,out}`). Labels name
//! the replayed pins (`u-*` in `uuid.out`, `i-*` in `inst.out`,
//! `r-*` in `uri.out`, `e-*` in `edges.out`).
//!
//! The S6 boundary: `#inst` / `#uuid` reads still answer
//! `TaggedLiteral` on this slice (S6 owns tag routing), so no core
//! var constructs `Instant` (or `Uri` — upstream has no URI
//! constructor either). `inst-ms` over real instants and the `Uri`
//! kind arms are covered by Rust tests (`scalar.rs`,
//! `scalars.rs`); this file pins everything Clojure-reachable plus
//! the current boundary behavior (loudly marked — the S6-flip
//! integration flips those lines).

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
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
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

/// Valid shapes canonicalize; lenient lengths mask
/// (`u-parse-canon`, `u-parse-upper`, `u-parse-short1`,
/// `u-parse-shortmid`, `u-parse-longshort`, `u-parse-zeros`,
/// `e-uuid-plus`).
#[test]
fn parse_uuid_shapes() {
    let mut eval = boot();
    for (input, want) in [
        (
            "550e8400-e29b-41d4-a716-446655440000",
            "\"550e8400-e29b-41d4-a716-446655440000\"",
        ),
        (
            "550E8400-E29B-41D4-A716-446655440000",
            "\"550e8400-e29b-41d4-a716-446655440000\"",
        ),
        (
            "550e840-e29b-41d4-a716-446655440000",
            "\"0550e840-e29b-41d4-a716-446655440000\"",
        ),
        (
            "550e8400-e29b-41d-a716-446655440000",
            "\"550e8400-e29b-041d-a716-446655440000\"",
        ),
        (
            "123456789-123-1234-1234-123456789012",
            "\"23456789-0123-1234-1234-123456789012\"",
        ),
        ("0-0-0-0-0", "\"00000000-0000-0000-0000-000000000000\""),
        (
            "+550e840-e29b-41d4-a716-44665544000",
            "\"0550e840-e29b-41d4-a716-044665544000\"",
        ),
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(str (parse-uuid \"{input}\"))")),
            want,
            "{input}"
        );
    }
}

/// Invalid strings answer nil (`u-parse-bogus`, `u-parse-empty`,
/// `u-parse-braces`, `u-parse-urn`, `u-parse-nonhex`,
/// `u-parse-nodash`, `u-parse-short`, `u-parse-emptypart`,
/// `u-parse-traildash`, `u-parse-dashes`, `u-parse-badidx`,
/// `u-parse-long1`).
#[test]
fn parse_uuid_failures_answer_nil() {
    let mut eval = boot();
    for input in [
        "bogus",
        "",
        "{550e8400-e29b-41d4-a716-446655440000}",
        "urn:uuid:550e8400-e29b-41d4-a716-446655440000",
        "550e8400-e29b-41d4-a716-44665544000g",
        "550e8400e29b41d4a716446655440000",
        "550e8400-e29b-41d4-a716",
        "550e8400--41d4-a716-446655440000",
        "550e8400-e29b-41d4-a716-446655440000-",
        "----",
        "g50e8400-e29b-41d4-a716-446655440000",
        "123456789abcdef0-e29b-41d4-a716-446655440000",
        "550e8400-e29b-41d4-a7160-446655440000",
        "550e8400-e29b-41d4-a716-1234567890123",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(parse-uuid \"{input}\")")),
            "nil",
            "{input}"
        );
    }
}

/// Slow-path edges also answer nil: a five-dash shape (never a
/// UUID), a group overflowing the `u64` accumulator, and a
/// non-Latin-1 nibble (the fast path only reads Latin-1).
#[test]
fn parse_uuid_slow_path_edges_answer_nil() {
    let mut eval = boot();
    for input in [
        "1-2-3-4-5-6",
        "10000000000000000-1-1-1-1",
        "550e8400-e29b-41d4-a716-44665544Ā000",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(parse-uuid \"{input}\")")),
            "nil",
            "{input}"
        );
    }
}

/// Nil is the verbatim NPE; non-strings fail the `String` cast
/// (`u-parse-nil`, `u-parse-num`, `u-parse-bool`, `u-parse-vec`,
/// `u-parse-kw`).
#[test]
fn parse_uuid_input_classes() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(parse-uuid nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"name\" is null"
    );
    for (input, class) in [
        ("42", "java.lang.Long"),
        ("true", "java.lang.Boolean"),
        ("[1]", "clojure.lang.PersistentVector"),
        (":x", "clojure.lang.Keyword"),
    ] {
        let err = uncaught(&mut eval, &format!("(parse-uuid {input})"));
        assert!(
            err.starts_with(&format!(
                "java.lang.ClassCastException | class {class} cannot be cast to class java.lang.String"
            )),
            "{input}: {err}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(parse-uuid \"x\" \"y\")")
            .split(" | ")
            .next()
            .unwrap(),
        "clojure.lang.ArityException"
    );
}

/// Version/variant bits, width, round-trip, uniqueness
/// (`u-rand-*`, `e-uuid-roundtrip`, `e-uuid-ver4`).
#[test]
fn random_uuid_bits() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(count (str (random-uuid)))"), "36");
    assert_eq!(run_last(&mut eval, "(nth (str (random-uuid)) 14)"), "\\4");
    assert_eq!(
        run_last(
            &mut eval,
            "(contains? #{\\8 \\9 \\a \\b} (nth (str (random-uuid)) 19))"
        ),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(uuid? (random-uuid))"), "true");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [u (random-uuid)] (= u (parse-uuid (str u))))"
        ),
        "true"
    );
    assert_eq!(
        run_last(&mut eval, "(= (random-uuid) (random-uuid))"),
        "false"
    );
}

/// The `uuid?` battery (`u-p-*`).
#[test]
fn uuid_predicate_battery() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(uuid? (parse-uuid \"550e8400-e29b-41d4-a716-446655440000\"))"
        ),
        "true"
    );
    for input in [
        "nil",
        "\"550e8400-e29b-41d4-a716-446655440000\"",
        "42",
        "[]",
        "{}",
        ":x",
        "'x",
        "#inst \"2024-01-01\"",
        "(tagged-literal 'uuid \"550e8400-e29b-41d4-a716-446655440000\")",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(uuid? {input})")),
            "false",
            "{input}"
        );
    }
}

/// Equality, hashing, map lookup (`u-eq-*`, `u-hash-*`).
#[test]
fn uuid_equality_hashing() {
    let mut eval = boot();
    let a = "(parse-uuid \"550e8400-e29b-41d4-a716-446655440000\")";
    let b = "(parse-uuid \"550E8400-E29B-41D4-A716-446655440000\")";
    assert_eq!(run_last(&mut eval, &format!("(= {a} {b})")), "true");
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(= {a} \"550e8400-e29b-41d4-a716-446655440000\")")
        ),
        "false"
    );
    assert_eq!(run_last(&mut eval, &format!("(= {a} nil)")), "false");
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(= {a} (parse-uuid \"550e8400-e29b-41d4-a716-446655440001\"))")
        ),
        "false"
    );
    assert_eq!(run_last(&mut eval, &format!("(hash {a})")), "1170702770");
    assert_eq!(
        run_last(
            &mut eval,
            "(hash (parse-uuid \"00000000-0000-0000-0000-000000000000\"))"
        ),
        "0"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(hash (parse-uuid \"ffffffff-ffff-ffff-ffff-ffffffffffff\"))"
        ),
        "0"
    );
    assert_eq!(run_last(&mut eval, &format!("(hash {b})")), "1170702770");
    assert_eq!(
        run_last(&mut eval, &format!("(get {{{a} :hit}} {b})")),
        ":hit"
    );
}

/// Signed `(hi, lo)` ordering; cross-kind pairs fail the
/// checkcast (`u-cmp-*` — the port normalizes same-kind results
/// to the sign).
#[test]
fn uuid_ordering() {
    let mut eval = boot();
    for (a, b, want) in [
        (
            "550e8400-e29b-41d4-a716-446655440000",
            "550E8400-E29B-41D4-A716-446655440000",
            "0",
        ),
        (
            "00000000-0000-0000-0000-000000000001",
            "00000000-0000-0000-0000-000000000002",
            "-1",
        ),
        (
            "00000001-0000-0000-0000-000000000000",
            "00000000-ffff-ffff-ffff-ffffffffffff",
            "1",
        ),
        (
            "ffffffff-ffff-ffff-ffff-ffffffffffff",
            "00000000-0000-0000-0000-000000000000",
            "-1",
        ),
        (
            "00000000-0000-0000-ffff-ffffffffffff",
            "00000000-0000-0000-0000-000000000000",
            "-1",
        ),
        (
            "00000000-0000-0000-0000-000000000002",
            "00000000-0000-0000-0000-000000000001",
            "1",
        ),
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("(compare (parse-uuid \"{a}\") (parse-uuid \"{b}\"))")
            ),
            want,
            "{a} {b}"
        );
    }
    assert_eq!(
        uncaught(
            &mut eval,
            "(compare (parse-uuid \"550e8400-e29b-41d4-a716-446655440000\") \"s\")"
        ),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.util.UUID (java.lang.String and java.util.UUID are in module java.base of loader 'bootstrap')"
    );
    // `sort` rides the same order.
    assert_eq!(
        run_last(
            &mut eval,
            "(map str (sort [(parse-uuid \"00000000-0000-0000-0000-000000000002\") (parse-uuid \"00000000-0000-0000-0000-000000000001\")]))"
        ),
        "(\"00000000-0000-0000-0000-000000000001\" \"00000000-0000-0000-0000-000000000002\")"
    );
}

/// `pr`/`print` stay tagged; `str` goes bare (`u-pr`, `u-print`,
/// `u-str` — note `read-string` stays `TaggedLiteral` here: the
/// S6 boundary, not the JVM round-trip).
#[test]
fn uuid_printing() {
    let mut eval = boot();
    let u = "(parse-uuid \"550E8400-E29B-41D4-A716-446655440000\")";
    assert_eq!(
        run_last(&mut eval, &format!("(pr-str {u})")),
        "\"#uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\""
    );
    assert_eq!(
        run_last(&mut eval, &format!("(print-str {u})")),
        "\"#uuid \\\"550e8400-e29b-41d4-a716-446655440000\\\"\""
    );
    assert_eq!(
        run_last(&mut eval, &format!("(str {u})")),
        "\"550e8400-e29b-41d4-a716-446655440000\""
    );
}

/// The `uri?` battery (`r-p-*`): nothing Clojure-reachable is a
/// URI (upstream has no URI constructor either).
#[test]
fn uri_predicate_battery() {
    let mut eval = boot();
    for input in [
        "nil",
        "\"http://example.com/x\"",
        "42",
        "(random-uuid)",
        "#inst \"2024-01-01\"",
        "[]",
        "{}",
        ":x",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(uri? {input})")),
            "false",
            "{input}"
        );
    }
}

/// `inst?` is `satisfies?` over `Inst` (`i-p-*`, `i-sat-eq`):
/// nothing Clojure-reachable satisfies it — including the
/// `#inst` literal, which still reads `TaggedLiteral` (S6
/// boundary: this line flips with the integration).
#[test]
fn inst_predicate_matches_satisfies() {
    let mut eval = boot();
    for input in [
        "nil",
        "\"2024-01-01\"",
        "42",
        "(random-uuid)",
        "#inst \"2024-01-01\"",
    ] {
        assert_eq!(
            run_last(
                &mut eval,
                &format!("[(inst? {input}) (satisfies? Inst {input})]")
            ),
            "[false false]",
            "{input}"
        );
    }
}

/// `inst-ms` failure modes (`i-ms-nil`, `i-ms-str`, `i-ms-int`,
/// `i-ms-uuid`, `i-ms-vec`).
#[test]
fn inst_ms_failures() {
    let mut eval = boot();
    // Nil names `null`, not the oracle's `nil` — the text comes
    // from shared #33 machinery (out of this slice's fence).
    assert_eq!(
        uncaught(&mut eval, "(inst-ms nil)"),
        "java.lang.IllegalArgumentException | No implementation of method: :inst-ms* of protocol: #'clojure.core/Inst found for class: null"
    );
    for (input, class) in [
        ("\"2024-01-01\"", "java.lang.String"),
        ("42", "java.lang.Long"),
        ("(random-uuid)", "java.util.UUID"),
        ("[1]", "clojure.lang.PersistentVector"),
    ] {
        assert_eq!(
            uncaught(&mut eval, &format!("(inst-ms {input})")),
            format!(
                "java.lang.IllegalArgumentException | No implementation of method: :inst-ms* of protocol: #'clojure.core/Inst found for class: {class}"
            ),
            "{input}"
        );
    }
    // S6 boundary: the `#inst` literal is still a tagged literal,
    // so it fails like one (this line flips with the integration).
    assert_eq!(
        uncaught(&mut eval, "(inst-ms #inst \"2024-01-01\")"),
        "java.lang.IllegalArgumentException | No implementation of method: :inst-ms* of protocol: #'clojure.core/Inst found for class: clojure.lang.TaggedLiteral"
    );
}

/// User types extend `Inst` through the ordinary machinery
/// (`i-ext-str`, `i-ext-deftype`).
#[test]
fn inst_user_extension() {
    let mut eval = boot();
    assert_eq!(
        run_last(
            &mut eval,
            "(do (extend-protocol Inst String (inst-ms* [s] 42)) [(inst-ms \"x\") (inst? \"x\")])"
        ),
        "[42 true]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(do (deftype S9Box [ms]) (extend-protocol Inst S9Box (inst-ms* [b] 7)) [(inst-ms (S9Box. 1)) (inst? (S9Box. 1))])"
        ),
        "[7 true]"
    );
}

/// `Inst` is a real #33 protocol map with the `Date` extension
/// installed.
#[test]
fn inst_protocol_shape() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(map? Inst)"), "true");
    assert_eq!(run_last(&mut eval, "(:on Inst)"), "\"clojure.core.Inst\"");
    assert_eq!(
        run_last(&mut eval, "(contains? (:impls Inst) \"java.util.Date\")"),
        "true"
    );
    assert_eq!(run_last(&mut eval, "(fn? inst-ms*)"), "true");
    assert_eq!(run_last(&mut eval, "(fn? inst-ms)"), "true");
}
