// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W6: `clojure.xml` smoke tests — parse/emit round-trips against the
//! upstream-documented element shape (`:tag`/`:attrs`/`:content`).

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

fn boot_xml() -> Evaluator {
    // #217: no manual installer call — `require` installs the
    // natives through the `load_one` arm (the production path).
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.xml :as xml])");
    eval
}

#[test]
fn xml_ns_loads_with_api() {
    let mut eval = boot_xml();
    assert_eq!(
        run(
            &mut eval,
            "[(ifn? xml/parse) (ifn? xml/emit) (ifn? xml/emit-element) (ifn? xml/tag) (ifn? xml/attrs) (ifn? xml/content) (ifn? xml/startparse-sax) (ifn? xml/startparse-sax-safe) (ifn? xml/sax-parser) (ifn? xml/disable-external-entities) (map? xml/content-handler)]"
        ),
        "[true true true true true true true true true true true]"
    );
}

#[test]
fn parse_builds_element_tree() {
    let mut eval = boot_xml();
    assert_eq!(
        run(
            &mut eval,
            r#"(let [e (xml/parse "<a><b x='1' y=\"2\">hi</b><c/></a>")]
                 [(xml/tag e) (xml/attrs e) (count (xml/content e))
                  (xml/tag (nth (xml/content e) 0))
                  (xml/attrs (nth (xml/content e) 0))
                  (xml/content (nth (xml/content e) 0))
                  (xml/tag (nth (xml/content e) 1))
                  (xml/attrs (nth (xml/content e) 1))
                  (xml/content (nth (xml/content e) 1))])"#
        ),
        r#"[:a nil 2 :b {:x "1", :y "2"} ["hi"] :c nil nil]"#
    );
}

#[test]
fn emit_output_is_byte_exact() {
    let mut eval = boot_xml();
    assert_eq!(
        run(
            &mut eval,
            r#"(with-out-str (xml/emit {:tag :a :attrs {:x "1"} :content ["hi"]}))"#
        ),
        r#""<?xml version='1.0' encoding='UTF-8'?>\n<a x='1'>\nhi\n</a>\n""#
    );
    // Attr-less empty elements self-close.
    assert_eq!(
        run(&mut eval, r#"(with-out-str (xml/emit {:tag :a}))"#),
        r#""<?xml version='1.0' encoding='UTF-8'?>\n<a/>\n""#
    );
}

#[test]
fn emit_parse_round_trip() {
    let mut eval = boot_xml();
    // String content gains newlines (println per element — upstream
    // emit/parse are verbatim, so the loss matches exactly).
    assert_eq!(
        run(
            &mut eval,
            r#"(xml/parse (with-out-str (xml/emit {:tag :a :content ["hi"]})))"#
        ),
        r#"{:tag :a, :attrs nil, :content ["\nhi\n"]}"#
    );
    // Element-only trees round-trip exactly (whitespace-only text
    // between elements is skipped).
    assert_eq!(
        run(
            &mut eval,
            r#"(= {:tag :a :attrs nil :content [{:tag :b :attrs {:x "1"} :content nil}]}
                  (xml/parse (with-out-str (xml/emit {:tag :a :content [{:tag :b :attrs {:x "1"}}]}))))"#
        ),
        "true"
    );
}

#[test]
fn entities_cdata_comments_and_pis() {
    let mut eval = boot_xml();
    assert_eq!(
        run(
            &mut eval,
            r#"(xml/content (xml/parse "<a>&amp;&#65;&#x42;<![CDATA[<raw>&]]><!--c--><?pi d?></a>"))"#
        ),
        r#"["&AB<raw>&"]"#
    );
    // Whitespace-only text between elements is skipped.
    assert_eq!(
        run(
            &mut eval,
            r#"(xml/content (xml/parse "<a>\n  <b/>\n</a>"))"#
        ),
        r#"[{:tag :b, :attrs nil, :content nil}]"#
    );
}

#[test]
fn parse_errors_are_exceptions() {
    let mut eval = boot_xml();
    let err = uncaught(&mut eval, r#"(xml/parse "<a></b>")"#);
    assert!(
        err.starts_with("java.lang.Exception | mismatched tag: expected </a> but found </b>"),
        "{err}"
    );
    let err = uncaught(&mut eval, r#"(xml/parse "<a>&foo;</a>")"#);
    assert!(err.contains("undefined entity &foo;"), "{err}");
}

#[test]
fn startparse_variants_and_tokens() {
    let mut eval = boot_xml();
    assert_eq!(
        run(
            &mut eval,
            r#"[(xml/tag (xml/parse "<a/>" xml/startparse-sax))
               (xml/tag (xml/parse "<a/>" xml/startparse-sax-safe))
               (xml/tag (xml/parse "<a/>" (fn [s ch] (xml/startparse-sax s ch))))
               (:external-entities (xml/disable-external-entities (xml/sax-parser)))]"#
        ),
        "[:a :a :a false]"
    );
}

#[test]
fn parse_reads_files_and_streams() {
    let mut eval = boot_xml();
    let path = std::env::temp_dir().join("w6-xml-parse-reads-files.xml");
    // Forward slashes read back through the Clojure string literal
    // on every platform (Win32 accepts them).
    let path_s = path.to_string_lossy().replace('\\', "/");
    run(
        &mut eval,
        &format!(r#"(spit "{path_s}" "<root><leaf v='42'/></root>")"#),
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"[(xml/tag (xml/parse "{path_s}")) ((comp :v xml/attrs first xml/content) (xml/parse "{path_s}"))]"#
            )
        ),
        r#"[:root "42"]"#
    );
    std::fs::remove_file(&path).ok();
}

/// #217: the production path — a bare boot with no manual
/// installer call: `require` installs the natives, and `parse`
/// exercises `__xml-scan*`.
#[test]
fn require_installs_natives_without_manual_call() {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.xml :as xml])");
    assert_eq!(run(&mut eval, r#"(xml/tag (xml/parse "<a/>"))"#), ":a");
}
