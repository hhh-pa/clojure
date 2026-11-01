// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Gap-120 slice 3: the `clojure.java.io` facsimile — `file`,
//! `reader`, `writer`, `input-stream`, `output-stream`, and
//! `delete-file` over `std::fs`.
//! Oracle: upstream `clojure/java/io.clj` source text (read, never
//! executed). Error texts match upstream; D-divergences are pinned
//! per test (see `jio.rs` docs).

use clj_eval::{Evaluator, SourceError, boot};
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

/// Intern a Rust-built value as `user/<name>` (the `jdkio.rs`
/// handle-test pattern): the only way `Uri` values enter
/// evaluated code (the port has no `uri` fn or `#uri` tag).
fn intern(eval: &mut Evaluator, name: &str, v: Value) {
    let sym = clj_value::Symbol::new(Some("user"), name);
    let id = eval.registry.intern(&sym);
    eval.registry.with_state_mut(id, |st| {
        st.root = Some(v);
    });
}

/// Intern `user/<name>` as the parsed URI `text`.
fn intern_uri(eval: &mut Evaluator, name: &str, text: &str) {
    let uri = clj_value::Uri::parse(text).unwrap();
    intern(eval, name, Value::Uri(Box::new(uri)));
}

/// A Rust string as a Clojure string literal.
fn clj_str(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        if c == '"' || c == '\\' {
            out.push('\\');
        }
        out.push(c);
    }
    out.push('"');
    out
}

/// A `file:` URL spelling for an absolute fs path — well-formed
/// on every platform (Windows needs the third slash before `C:`;
/// the bare `file://{p}` form parks the drive in the authority
/// slot there and resolves drive-relative).
fn file_url(p: &str) -> String {
    format!("file:///{}", p.replace('\\', "/").trim_start_matches('/'))
}

/// A scratch dir unique to this test (parallel-safe), wiped first.
fn fixture(name: &str) -> std::path::PathBuf {
    let dir = std::env::temp_dir().join(format!("clj-jio-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&dir);
    std::fs::create_dir_all(&dir).unwrap();
    dir
}

/// A booted evaluator with the facsimile required as `io`.
fn jio() -> Evaluator {
    let mut eval = boot();
    eval.eval_source("(require '[clojure.java.io :as io])")
        .unwrap();
    eval
}

fn sep() -> String {
    std::path::MAIN_SEPARATOR.to_string()
}

// --- `file` ---

#[test]
fn file_str_is_literal() {
    let mut eval = jio();
    assert_eq!(run(&mut eval, r#"(str (io/file "a/b"))"#), r#""a/b""#);
    // No URL resolution in `as-file` (unlike the stream fns).
    assert_eq!(
        run(&mut eval, r#"(str (io/file "file:///tmp/x"))"#),
        r#""file:///tmp/x""#
    );
}

#[test]
fn file_passthrough_equal_by_path() {
    let mut eval = jio();
    assert_eq!(run(&mut eval, r#"(= (io/file "a") (io/file "a"))"#), "true");
    assert_eq!(
        run(&mut eval, r#"(let [f (io/file "a")] (= f (io/file f)))"#),
        "true"
    );
}

#[test]
fn file_nil_is_nil() {
    let mut eval = jio();
    assert_eq!(run(&mut eval, "(io/file nil)"), "nil");
}

#[test]
fn file_miss_is_protocol_text() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/file 42)"),
        "java.lang.IllegalArgumentException | No implementation of method: :as-file of protocol: #'clojure.java.io/Coercions found for class: java.lang.Long"
    );
}

#[test]
fn file_join() {
    let mut eval = jio();
    let want = clj_str(&format!("a{0}b{0}c", sep()));
    assert_eq!(run(&mut eval, r#"(str (io/file "a" "b" "c"))"#), want);
    let want2 = clj_str(&format!("a{0}b", sep()));
    assert_eq!(run(&mut eval, r#"(str (io/file "a" "b"))"#), want2);
}

#[test]
fn file_join_trailing_separator_never_doubles() {
    let mut eval = jio();
    assert_eq!(run(&mut eval, r#"(str (io/file "a/" "b"))"#), r#""a/b""#);
    assert_eq!(run(&mut eval, r#"(str (io/file "a\\" "b"))"#), r#""a\\b""#);
}

#[test]
fn file_nil_parent_answers_child_d3() {
    // D3: the JVM resolves against the fs root (a platform
    // quirk); the facsimile answers the child alone.
    let mut eval = jio();
    assert_eq!(run(&mut eval, r#"(str (io/file nil "c"))"#), r#""c""#);
}

#[test]
fn file_absolute_child_is_iae() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, r#"(io/file "a" "/abs")"#),
        "java.lang.IllegalArgumentException | /abs is not a relative path"
    );
}

#[test]
fn file_nil_child_is_npe() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, r#"(io/file "a" nil)"#),
        "java.lang.NullPointerException | Cannot invoke \"java.io.File.isAbsolute()\" because \"f\" is null"
    );
}

#[test]
fn file_bad_child_miss_propagates() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, r#"(io/file "a" 42)"#),
        "java.lang.IllegalArgumentException | No implementation of method: :as-file of protocol: #'clojure.java.io/Coercions found for class: java.lang.Long"
    );
}

#[test]
fn file_arity() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/file)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/file"
    );
}

#[test]
fn file_uri_shapes() {
    let mut eval = jio();
    intern_uri(&mut eval, "u1", "file:///tmp/a%20b");
    // Platform paths, pr-quoted (Windows emits `\`, escaped by `pr`,
    // like `java.io.File.getPath`).
    assert_eq!(
        run(&mut eval, "(str (io/file u1))"),
        clj_str(&format!("{0}tmp{0}a b", sep()))
    );
    // The authority drops (even `localhost`).
    intern_uri(&mut eval, "u2", "file://host/x");
    assert_eq!(
        run(&mut eval, "(str (io/file u2))"),
        clj_str(&format!("{0}x", sep()))
    );
    intern_uri(&mut eval, "u3", "file://host");
    assert_eq!(run(&mut eval, "(str (io/file u3))"), r#""""#);
    // Schemes match case-insensitively; `+` stays literal.
    intern_uri(&mut eval, "u4", "FILE:/q");
    assert_eq!(
        run(&mut eval, "(str (io/file u4))"),
        clj_str(&format!("{0}q", sep()))
    );
    intern_uri(&mut eval, "u5", "file:/a+b");
    assert_eq!(
        run(&mut eval, "(str (io/file u5))"),
        clj_str(&format!("{0}a+b", sep()))
    );
    intern_uri(&mut eval, "u6", "file:rel");
    assert_eq!(run(&mut eval, "(str (io/file u6))"), r#""rel""#);
}

#[test]
fn file_uri_drive_letter() {
    // `file:///C:/x` drops the redundant slash ahead of the drive on
    // Windows (the JDK `File(URI)` route); anywhere else the leading
    // slash is a legitimate absolute path and survives.
    let mut eval = jio();
    intern_uri(&mut eval, "ud", "file:///C:/x");
    #[cfg(windows)]
    assert_eq!(run(&mut eval, "(str (io/file ud))"), clj_str("C:\\x"));
    #[cfg(not(windows))]
    assert_eq!(run(&mut eval, "(str (io/file ud))"), r#""/C:/x""#);
}

#[test]
fn file_uri_not_a_file() {
    let mut eval = jio();
    intern_uri(&mut eval, "uhttp", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/file uhttp)"),
        "java.lang.IllegalArgumentException | Not a file: http://h/x"
    );
    // D5: relative URIs are `Not a file` too (upstream throws
    // `MalformedURLException` — no network, no such throw here).
    intern_uri(&mut eval, "urel", "ab");
    assert_eq!(
        uncaught(&mut eval, "(io/file urel)"),
        "java.lang.IllegalArgumentException | Not a file: ab"
    );
}

#[test]
fn file_uri_percent_edges() {
    // Valid escapes decode (via `file:` strings at real files —
    // plus the `%20` URI row in `file_uri_shapes`).
    let dir = fixture("percent-edges");
    write_bytes(&dir, "a~", b"tilde\n");
    write_bytes(&dir, "aAb", b"mix\n");
    let mut eval = jio();
    for (name, want) in [
        ("a%7e", "[\"tilde\"]"),
        ("a%7E", "[\"tilde\"]"),
        ("a%41b", "[\"mix\"]"),
    ] {
        let url = file_url(&dir.join(name).to_string_lossy());
        assert_eq!(
            run(
                &mut eval,
                &format!("(vec (line-seq (io/reader {})))", clj_str(&url))
            ),
            want,
            "{name}"
        );
    }
    // D5: malformed `%` stays literal (the JDK throws there).
    // These ride `file:` strings — the port's URI parser itself
    // rejects malformed escapes, so no `Uri` value can spell them.
    for (text, want) in [
        ("file:/a%zz", "/a%zz"),
        ("file:/a%2z", "/a%2z"),
        ("file:/a%", "/a%"),
        ("file:/a%2", "/a%2"),
    ] {
        let got = uncaught(&mut eval, &format!("(io/reader {})", clj_str(text)));
        assert!(
            got.replace('\\', "/")
                .starts_with(&format!("java.io.FileNotFoundException | {want} (")),
            "{text}: {got}"
        );
    }
}

// --- `reader` ---

/// Write `name` under `dir` with `bytes`, answer its path string.
fn write_bytes(dir: &std::path::Path, name: &str, bytes: &[u8]) -> String {
    let path = dir.join(name);
    std::fs::write(&path, bytes).unwrap();
    path.to_string_lossy().into_owned()
}

#[test]
fn reader_str_and_file_roundtrip() {
    let dir = fixture("reader-roundtrip");
    let p = write_bytes(&dir, "r.txt", b"l1\nl2\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(&mut eval, &format!("(vec (line-seq (io/reader {lit})))")),
        r#"["l1" "l2"]"#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader (io/file {lit}))))")
        ),
        r#"["l1" "l2"]"#
    );
    // `read` through `*in*` works too (D1 interop).
    assert_eq!(
        run(
            &mut eval,
            &format!("(binding [*in* (io/reader {lit})] (read))")
        ),
        "l1"
    );
}

#[test]
fn reader_missing_names_path() {
    let dir = fixture("reader-missing");
    let missing = dir.join("nope.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let got = uncaught(&mut eval, &format!("(io/reader {})", clj_str(&missing)));
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {missing} (")),
        "{got}"
    );
}

#[test]
fn reader_nil_and_other_texts() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/reader nil)"),
        "java.lang.IllegalArgumentException | Cannot open <nil> as a Reader."
    );
    assert_eq!(
        uncaught(&mut eval, "(io/reader 42)"),
        "java.lang.IllegalArgumentException | Cannot open <42> as an InputStream."
    );
}

#[test]
fn reader_passthrough_ignores_opts() {
    let dir = fixture("reader-passthrough");
    let p = write_bytes(&dir, "r.txt", b"x\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (identical? r (io/reader r)))")
        ),
        "true"
    );
    // Passthrough skips even charset validation (upstream `Reader` arm).
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (identical? r (io/reader r :encoding \"BAD\")))")
        ),
        "true"
    );
}

#[test]
fn reader_odd_opts() {
    let dir = fixture("reader-odd");
    let p = write_bytes(&dir, "r.txt", b"x\n");
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, &format!("(io/reader {} :encoding)", clj_str(&p))),
        "java.lang.IllegalArgumentException | No value supplied for key: :encoding"
    );
}

#[test]
fn reader_bad_encoding_is_uee() {
    let dir = fixture("reader-enc");
    let p = write_bytes(&dir, "r.txt", b"x\n");
    let mut eval = jio();
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/reader {} :encoding \"BAD\")", clj_str(&p))
        ),
        "java.io.UnsupportedEncodingException | BAD"
    );
}

#[test]
fn reader_opens_before_charset() {
    // The upstream ctor order: a missing file reports
    // `FileNotFoundException` even with a bad `:encoding`.
    let dir = fixture("reader-order");
    let missing = dir.join("nope.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let got = uncaught(
        &mut eval,
        &format!("(io/reader {} :encoding \"BAD\")", clj_str(&missing)),
    );
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {missing} (")),
        "{got}"
    );
}

#[test]
fn reader_encoding_cast_and_defaults() {
    let dir = fixture("reader-enc2");
    let p = write_bytes(&dir, "r.txt", b"x\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        uncaught(&mut eval, &format!("(io/reader {lit} :encoding 42)")),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    // nil/false fall back to UTF-8 (upstream `or`).
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader {lit} :encoding nil)))")
        ),
        r#"["x"]"#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader {lit} :encoding false)))")
        ),
        r#"["x"]"#
    );
}

#[test]
fn reader_unknown_opts_ignored() {
    let dir = fixture("reader-opts");
    let p = write_bytes(&dir, "r.txt", b"x\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    for opts in [
        ":buffer-size 5",
        "\"encoding\" \"BAD\"",
        ":foo/encoding \"BAD\"",
        ":append true",
    ] {
        assert_eq!(
            run(
                &mut eval,
                &format!("(vec (line-seq (io/reader {lit} {opts})))")
            ),
            r#"["x"]"#,
            "{opts}"
        );
    }
}

#[test]
fn reader_byte_array() {
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (byte-array [65 10 66]))))"
        ),
        r#"["A" "B"]"#
    );
    // Invalid UTF-8 becomes U+FFFD (lossy, like the JDK default).
    assert_eq!(
        run(&mut eval, "(vec (line-seq (io/reader (byte-array [-1]))))"),
        "[\"�\"]"
    );
    // Byte inputs honor `:encoding` (and its UEE).
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (byte-array [0 65]) :encoding \"UTF-16BE\")))"
        ),
        r#"["A"]"#
    );
    assert_eq!(
        uncaught(&mut eval, "(io/reader (byte-array [65]) :encoding \"BAD\")"),
        "java.io.UnsupportedEncodingException | BAD"
    );
}

#[test]
fn reader_char_array() {
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (char-array \"hi\"))))"
        ),
        r#"["hi"]"#
    );
    // Char inputs never consult the charset (no UEE possible).
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (char-array \"hi\") :encoding \"BAD\")))"
        ),
        r#"["hi"]"#
    );
}

#[test]
fn reader_input_stream() {
    let dir = fixture("reader-stream");
    let p = write_bytes(&dir, "r.txt", b"l1\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader (io/input-stream {lit}))))")
        ),
        r#"["l1"]"#
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/reader (io/input-stream {lit}) :encoding \"BAD\")")
        ),
        "java.io.UnsupportedEncodingException | BAD"
    );
    // Wrapping a closed stream reports the JDK text (D0: the
    // facsimile reads eagerly, so the wrap is the read).
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [s (io/input-stream {lit})] (.close s) (io/reader s))")
        ),
        "java.io.IOException | Stream closed"
    );
}

#[test]
fn reader_uri_and_file_strings() {
    let dir = fixture("reader-uri");
    let p = write_bytes(&dir, "r.txt", b"u\n");
    let mut eval = jio();
    intern_uri(&mut eval, "uf", &file_url(&p));
    assert_eq!(
        run(&mut eval, "(vec (line-seq (io/reader uf)))"),
        r#"["u"]"#
    );
    // `file:` strings resolve too (D2-positive half).
    let url = file_url(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader {})))", clj_str(&url))
        ),
        r#"["u"]"#
    );
    // Non-file URIs refuse (D2: no network).
    intern_uri(&mut eval, "uh", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/reader uh)"),
        "java.lang.IllegalArgumentException | Not a file: http://h/x"
    );
    // Other URL-shaped strings open literally (and miss — the miss
    // class is platform-mapped: Unix names the missing literal path,
    // Windows rejects the `:` spelling outright with os error 123).
    let got = uncaught(&mut eval, "(io/reader \"http://h/x\")");
    #[cfg(not(windows))]
    assert!(
        got.starts_with("java.io.FileNotFoundException | http://h/x ("),
        "{got}"
    );
    #[cfg(windows)]
    assert!(got.starts_with("java.io.IOException | "), "{got}");
}

#[test]
fn reader_miss_pr_shapes() {
    let mut eval = jio();
    // Non-byte arrays route with their pr-shape.
    let got = uncaught(&mut eval, "(io/reader (int-array [65]))");
    assert!(
        got.starts_with("java.lang.IllegalArgumentException | Cannot open <")
            && got.ends_with("> as an InputStream."),
        "{got}"
    );
    // Writers are openable as neither.
    let dir = fixture("reader-wmiss");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let got = uncaught(
        &mut eval,
        &format!("(let [w (io/writer {})] (io/reader w))", clj_str(&p)),
    );
    assert!(
        got.starts_with(
            "java.lang.IllegalArgumentException | Cannot open <#object[java.io.BufferedWriter"
        ) && got.ends_with("> as an InputStream."),
        "{got}"
    );
}

#[test]
fn reader_arity() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/reader)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/reader"
    );
}

#[test]
fn reader_latin1() {
    let dir = fixture("reader-latin1");
    let p = write_bytes(&dir, "r.txt", &[0xE9]);
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(vec (line-seq (io/reader {} :encoding \"ISO-8859-1\")))",
                clj_str(&p)
            )
        ),
        "[\"é\"]"
    );
}

// --- `writer` ---

/// Print `texts` (each a Clojure string literal) to a fresh
/// writer over `path` with `opts`, close it, and slurp back.
fn write_then_slurp(eval: &mut Evaluator, path: &str, opts: &str, texts: &[&str]) -> String {
    let lit = clj_str(path);
    let prints = texts
        .iter()
        .map(|t| format!("(print {t})"))
        .collect::<Vec<_>>()
        .join(" ");
    run(
        eval,
        &format!(
            "(let [w (io/writer {lit} {opts})] (binding [*out* w] {prints}) (.close w) (slurp {lit}))"
        ),
    )
}

#[test]
fn writer_roundtrip_and_class() {
    let dir = fixture("writer-roundtrip");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    assert_eq!(write_then_slurp(&mut eval, &p, "", &[r#""hi""#]), r#""hi""#);
    // Writers print as `java.io.BufferedWriter` (upstream class).
    let shown = run(&mut eval, &format!("(pr-str (io/writer {}))", clj_str(&p)));
    assert!(shown.contains("java.io.BufferedWriter"), "{shown}");
}

#[test]
fn writer_append_and_truncate() {
    let dir = fixture("writer-append");
    let mut eval = jio();
    let p = write_bytes(&dir, "w.txt", b"a");
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {lit} :append true)] (binding [*out* w] (print \"b\")) (.close w) (slurp {lit}))"
            )
        ),
        r#""ab""#
    );
    // Later pairs win.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {lit} :append false :append true)] (binding [*out* w] (print \"c\")) (.close w) (slurp {lit}))"
            )
        ),
        r#""abc""#
    );
    // The default truncates.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {lit})] (binding [*out* w] (print \"z\")) (.close w) (slurp {lit}))"
            )
        ),
        r#""z""#
    );
}

#[test]
fn writer_missing_dir() {
    let dir = fixture("writer-missing");
    let bad = dir
        .join("no-dir")
        .join("f.txt")
        .to_string_lossy()
        .into_owned();
    let mut eval = jio();
    let got = uncaught(&mut eval, &format!("(io/writer {})", clj_str(&bad)));
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {bad} (")),
        "{got}"
    );
}

#[test]
fn writer_nil_and_other_texts() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/writer nil)"),
        "java.lang.IllegalArgumentException | Cannot open <nil> as a Writer."
    );
    assert_eq!(
        uncaught(&mut eval, "(io/writer 42)"),
        "java.lang.IllegalArgumentException | Cannot open <42> as an OutputStream."
    );
}

#[test]
fn writer_passthrough_ignores_opts() {
    let dir = fixture("writer-passthrough");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [w (io/writer {lit})] (identical? w (io/writer w :encoding \"BAD\")))")
        ),
        "true"
    );
}

#[test]
fn writer_bad_encoding_truncates_first() {
    // The upstream ctor order: the file opens (truncating)
    // before the charset validates — the UEE leaves an empty
    // file behind, like the JVM.
    let dir = fixture("writer-enctrunc");
    let p = write_bytes(&dir, "w.txt", b"old");
    let mut eval = jio();
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/writer {} :encoding \"BAD\")", clj_str(&p))
        ),
        "java.io.UnsupportedEncodingException | BAD"
    );
    assert_eq!(std::fs::read_to_string(&p).unwrap(), "");
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/writer {} :encoding 42)", clj_str(&p))
        ),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(io/writer {} :append)", clj_str(&p))),
        "java.lang.IllegalArgumentException | No value supplied for key: :append"
    );
}

#[test]
fn writer_uri_shapes() {
    let dir = fixture("writer-uri");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    intern_uri(&mut eval, "uf", &file_url(&p));
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer uf)] (binding [*out* w] (print \"u\")) (.close w) (slurp {}))",
                clj_str(&p)
            )
        ),
        r#""u""#
    );
    // Non-file URIs are the verbatim upstream text (two words).
    intern_uri(&mut eval, "uh", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/writer uh)"),
        "java.lang.IllegalArgumentException | Can not write to non-file URL <http://h/x>"
    );
    // `file:` strings resolve too.
    let q = dir.join("v.txt").to_string_lossy().into_owned();
    let url = file_url(&q);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {})] (binding [*out* w] (print \"v\")) (.close w) (slurp {}))",
                clj_str(&url),
                clj_str(&q)
            )
        ),
        r#""v""#
    );
}

#[test]
fn writer_over_output_stream_d4() {
    // D4: the derived writer opens its own append handle — its
    // close never propagates back to the stream.
    let dir = fixture("writer-derived");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [s (io/output-stream {lit}) w1 (io/writer s)] \
                 (binding [*out* w1] (print \"a\")) (.close w1) \
                 (let [w2 (io/writer s)] (binding [*out* w2] (print \"b\")) (.close w2)) \
                 (.close s) (slurp {lit}))"
            )
        ),
        r#""ab""#
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [s (io/output-stream {lit})] (.close s) (io/writer s))")
        ),
        "java.io.IOException | Stream closed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/writer (io/output-stream {lit}) :encoding \"BAD\")")
        ),
        "java.io.UnsupportedEncodingException | BAD"
    );
}

#[test]
fn writer_over_deleted_stream_file() {
    // The derived append handle opens lazily: deleting the file
    // between the stream and the writer reports the path.
    let dir = fixture("writer-deleted");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    std::fs::write(&p, b"x").unwrap();
    let mut eval = jio();
    let lit = clj_str(&p);
    run(&mut eval, &format!("(def jio-s (io/output-stream {lit}))"));
    std::fs::remove_file(&p).unwrap();
    let got = uncaught(&mut eval, "(io/writer jio-s)");
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {p} (")),
        "{got}"
    );
}

#[test]
fn writer_arity() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/writer)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/writer"
    );
}

/// Assert the file holds exactly `bytes` after writing `text`
/// (a Clojure string literal) with `encoding`.
fn assert_encoded(name: &str, text: &str, encoding: &str, bytes: &[u8]) {
    let dir = fixture(name);
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    run(
        &mut eval,
        &format!(
            "(let [w (io/writer {lit} :encoding \"{encoding}\")] (binding [*out* w] (print {text})) (.close w))"
        ),
    );
    assert_eq!(std::fs::read(&p).unwrap(), bytes, "{encoding} {text}");
}

#[test]
fn writer_encodings_byte_exact() {
    assert_encoded("enc-utf8", r#""é""#, "UTF-8", &[0xC3, 0xA9]);
    assert_encoded("enc-ascii", r#""é""#, "US-ASCII", b"?");
    assert_encoded("enc-ascii-ok", r#""Az""#, "US-ASCII", b"Az");
    assert_encoded("enc-latin1", r#""é""#, "ISO-8859-1", &[0xE9]);
    assert_encoded("enc-latin1-q", r#""€""#, "ISO-8859-1", b"?");
    assert_encoded("enc-u16", r#""A""#, "UTF-16", &[0xFE, 0xFF, 0x00, 0x41]);
    assert_encoded("enc-u16be", r#""A""#, "UTF-16BE", &[0x00, 0x41]);
    assert_encoded("enc-u16le", r#""A""#, "UTF-16LE", &[0x41, 0x00]);
    assert_encoded(
        "enc-u32",
        r#""A""#,
        "UTF-32",
        &[0x00, 0x00, 0xFE, 0xFF, 0x00, 0x00, 0x00, 0x41],
    );
    assert_encoded("enc-u32be", r#""A""#, "UTF-32BE", &[0x00, 0x00, 0x00, 0x41]);
    assert_encoded("enc-u32le", r#""A""#, "UTF-32LE", &[0x41, 0x00, 0x00, 0x00]);
    // Astral chars ride surrogate pairs (and round-trip).
    assert_encoded(
        "enc-astral",
        "\"\u{1D11E}\"",
        "UTF-16BE",
        &[0xD8, 0x34, 0xDD, 0x1E],
    );
    let dir = fixture("enc-roundtrip");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {lit} :encoding \"UTF-16\")] \
                 (binding [*out* w] (print \"\u{1D11E}\")) (.close w) \
                 (vec (line-seq (io/reader {lit} :encoding \"UTF-16\"))))"
            )
        ),
        "[\"𝄞\"]"
    );
}

#[test]
fn writer_bom_once_per_stream() {
    // Two writes to one bare-form writer: a single BOM, then both
    // payloads (the per-write BOM wrote it twice — round-1 M1).
    let dir = fixture("bom-multi");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    run(
        &mut eval,
        &format!(
            "(let [w (io/writer {lit} :encoding \"UTF-16\")] \
             (binding [*out* w] (print \"A\") (print \"B\")) (.close w))"
        ),
    );
    assert_eq!(
        std::fs::read(&p).unwrap(),
        &[0xFE, 0xFF, 0x00, 0x41, 0x00, 0x42]
    );
    // An empty first write emits nothing and doesn't consume the
    // BOM (the JDK defers it to the first non-empty write).
    let dir = fixture("bom-empty-first");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    run(
        &mut eval,
        &format!(
            "(let [w (io/writer {lit} :encoding \"UTF-16\")] \
             (binding [*out* w] (print \"\") (print \"A\")) (.close w))"
        ),
    );
    assert_eq!(std::fs::read(&p).unwrap(), &[0xFE, 0xFF, 0x00, 0x41]);
    // Closing a never-written bare-form writer still emits the BOM
    // (the JDK end-of-input flush); fixed forms emit nothing.
    let dir = fixture("bom-close-empty");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let q = dir.join("v.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let (litp, litq) = (clj_str(&p), clj_str(&q));
    run(
        &mut eval,
        &format!(
            "(.close (io/writer {litp} :encoding \"UTF-16\")) \
             (.close (io/writer {litq} :encoding \"UTF-16BE\"))"
        ),
    );
    assert_eq!(std::fs::read(&p).unwrap(), &[0xFE, 0xFF]);
    assert!(std::fs::read(&q).unwrap().is_empty());
}

// --- `input-stream` ---

#[test]
fn input_stream_roundtrip() {
    let dir = fixture("input-roundtrip");
    let p = write_bytes(&dir, "i.bin", b"l1\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader (io/input-stream {lit}))))")
        ),
        r#"["l1"]"#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader (io/input-stream (io/file {lit})))))")
        ),
        r#"["l1"]"#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [s (io/input-stream {lit})] (identical? s (io/input-stream s)))")
        ),
        "true"
    );
    let shown = run(&mut eval, &format!("(pr-str (io/input-stream {lit}))"));
    assert!(shown.contains("java.io.BufferedInputStream"), "{shown}");
}

#[test]
fn input_stream_bytes_and_misses() {
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (io/input-stream (byte-array [65])))))"
        ),
        r#"["A"]"#
    );
    assert_eq!(
        uncaught(&mut eval, "(io/input-stream nil)"),
        "java.lang.IllegalArgumentException | Cannot open <nil> as an InputStream."
    );
    assert_eq!(
        uncaught(&mut eval, "(io/input-stream 42)"),
        "java.lang.IllegalArgumentException | Cannot open <42> as an InputStream."
    );
    // Char arrays are reader-only (the `Cannot open` route).
    let got = uncaught(&mut eval, "(io/input-stream (char-array \"hi\"))");
    assert!(got.ends_with("> as an InputStream."), "{got}");
    let dir = fixture("input-miss");
    let p = write_bytes(&dir, "i.txt", b"x\n");
    let got = uncaught(
        &mut eval,
        &format!("(io/input-stream (io/reader {}))", clj_str(&p)),
    );
    assert!(got.ends_with("> as an InputStream."), "{got}");
    assert_eq!(
        uncaught(&mut eval, "(io/input-stream)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/input-stream"
    );
}

#[test]
fn input_stream_opts_and_uri() {
    let dir = fixture("input-opts");
    let p = write_bytes(&dir, "i.txt", b"x\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    // Only the odd-count check runs — `:encoding` never consults.
    assert_eq!(
        uncaught(&mut eval, &format!("(io/input-stream {lit} :append)")),
        "java.lang.IllegalArgumentException | No value supplied for key: :append"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(vec (line-seq (io/reader (io/input-stream {lit} :encoding \"BAD\"))))")
        ),
        r#"["x"]"#
    );
    intern_uri(&mut eval, "uf", &file_url(&p));
    assert_eq!(
        run(
            &mut eval,
            "(vec (line-seq (io/reader (io/input-stream uf))))"
        ),
        r#"["x"]"#
    );
    intern_uri(&mut eval, "uh", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/input-stream uh)"),
        "java.lang.IllegalArgumentException | Not a file: http://h/x"
    );
    let missing = dir.join("nope.bin").to_string_lossy().into_owned();
    let got = uncaught(
        &mut eval,
        &format!("(io/input-stream {})", clj_str(&missing)),
    );
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {missing} (")),
        "{got}"
    );
}

// --- `output-stream` ---

#[test]
fn output_stream_roundtrip() {
    let dir = fixture("output-roundtrip");
    let p = dir.join("o.bin").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [s (io/output-stream {lit}) w (io/writer s)] \
                 (binding [*out* w] (print \"o\")) (.close w) (.close s) (slurp {lit}))"
            )
        ),
        r#""o""#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [s (io/output-stream {lit})] (identical? s (io/output-stream s)))")
        ),
        "true"
    );
    let shown = run(&mut eval, &format!("(pr-str (io/output-stream {lit}))"));
    assert!(shown.contains("java.io.BufferedOutputStream"), "{shown}");
}

#[test]
fn output_stream_append_and_misses() {
    let dir = fixture("output-append");
    let mut eval = jio();
    let p = write_bytes(&dir, "o.bin", b"a");
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [s (io/output-stream {lit} :append true) w (io/writer s)] \
                 (binding [*out* w] (print \"b\")) (.close w) (.close s) (slurp {lit}))"
            )
        ),
        r#""ab""#
    );
    assert_eq!(
        uncaught(&mut eval, "(io/output-stream nil)"),
        "java.lang.IllegalArgumentException | Cannot open <nil> as an OutputStream."
    );
    assert_eq!(
        uncaught(&mut eval, "(io/output-stream 42)"),
        "java.lang.IllegalArgumentException | Cannot open <42> as an OutputStream."
    );
    assert_eq!(
        uncaught(&mut eval, "(io/output-stream)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/output-stream"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(io/output-stream {lit} :append)")),
        "java.lang.IllegalArgumentException | No value supplied for key: :append"
    );
    let bad = dir.join("no-dir").join("f").to_string_lossy().into_owned();
    let got = uncaught(&mut eval, &format!("(io/output-stream {})", clj_str(&bad)));
    assert!(
        got.starts_with(&format!("java.io.FileNotFoundException | {bad} (")),
        "{got}"
    );
    intern_uri(&mut eval, "uh", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/output-stream uh)"),
        "java.lang.IllegalArgumentException | Can not write to non-file URL <http://h/x>"
    );
}

#[test]
fn output_stream_flush_and_double_close() {
    let dir = fixture("output-flush");
    let p = dir.join("o.bin").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [s (io/output-stream {lit})] (.flush s))")
        ),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [s (io/output-stream {lit})] (.close s) (.close s))")
        ),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [s (io/output-stream {lit})] (.close s) (.flush s))")
        ),
        "java.io.IOException | Stream closed"
    );
}

// --- `delete-file` ---

#[test]
fn delete_file_roundtrip() {
    let dir = fixture("delete-roundtrip");
    let p = write_bytes(&dir, "d.txt", b"x");
    let mut eval = jio();
    assert_eq!(
        run(&mut eval, &format!("(io/delete-file {})", clj_str(&p))),
        "true"
    );
    assert!(!std::path::Path::new(&p).exists());
    // `File` args delete the same way.
    let q = write_bytes(&dir, "e.txt", b"x");
    assert_eq!(
        run(
            &mut eval,
            &format!("(io/delete-file (io/file {}))", clj_str(&q))
        ),
        "true"
    );
    assert!(!std::path::Path::new(&q).exists());
}

#[test]
fn delete_missing_throws_naming_arg() {
    let dir = fixture("delete-missing");
    let missing = dir.join("nope.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(io/delete-file {})", clj_str(&missing))
        ),
        format!("java.io.IOException | Couldn't delete {missing}")
    );
    // The message names the ORIGINAL arg (a `File` prints its path).
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file (io/file \"nope2\"))"),
        "java.io.IOException | Couldn't delete nope2"
    );
}

#[test]
fn delete_silently_answers_value() {
    let dir = fixture("delete-silently");
    let missing = dir.join("nope.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&missing);
    assert_eq!(
        run(&mut eval, &format!("(io/delete-file {lit} :ok)")),
        ":ok"
    );
    // Falsy `silently` still throws (and success answers `true`
    // even with a falsy second arg).
    assert_eq!(
        uncaught(&mut eval, &format!("(io/delete-file {lit} false)")),
        format!("java.io.IOException | Couldn't delete {missing}")
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(io/delete-file {lit} nil)")),
        format!("java.io.IOException | Couldn't delete {missing}")
    );
    let p = write_bytes(&dir, "d.txt", b"x");
    assert_eq!(
        run(
            &mut eval,
            &format!("(io/delete-file {} false)", clj_str(&p))
        ),
        "true"
    );
}

#[test]
fn delete_nil_and_other_texts() {
    let mut eval = jio();
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.File.delete()\" because \"f\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file 42)"),
        "java.lang.IllegalArgumentException | No implementation of method: :as-file of protocol: #'clojure.java.io/Coercions found for class: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.java.io/delete-file"
    );
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file \"a\" :s :x)"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.java.io/delete-file"
    );
}

#[test]
fn delete_dirs() {
    let dir = fixture("delete-dirs");
    let mut eval = jio();
    let empty = dir.join("empty");
    std::fs::create_dir(&empty).unwrap();
    let empty_s = empty.to_string_lossy().into_owned();
    assert_eq!(
        run(
            &mut eval,
            &format!("(io/delete-file {})", clj_str(&empty_s))
        ),
        "true"
    );
    // Non-empty dirs fail (silently or throwing), like the JVM.
    let full = dir.join("full");
    std::fs::create_dir(&full).unwrap();
    std::fs::write(full.join("f.txt"), b"x").unwrap();
    let full_s = full.to_string_lossy().into_owned();
    assert_eq!(
        run(
            &mut eval,
            &format!("(io/delete-file {} :kept)", clj_str(&full_s))
        ),
        ":kept"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(io/delete-file {})", clj_str(&full_s))),
        format!("java.io.IOException | Couldn't delete {full_s}")
    );
}

#[test]
fn delete_uri() {
    let dir = fixture("delete-uri");
    let p = write_bytes(&dir, "d.txt", b"x");
    let mut eval = jio();
    intern_uri(&mut eval, "uf", &file_url(&p));
    assert_eq!(run(&mut eval, "(io/delete-file uf)"), "true");
    assert!(!std::path::Path::new(&p).exists());
    intern_uri(&mut eval, "uh", "http://h/x");
    assert_eq!(
        uncaught(&mut eval, "(io/delete-file uh)"),
        "java.lang.IllegalArgumentException | Not a file: http://h/x"
    );
}

#[cfg(unix)]
#[test]
fn delete_symlink_deletes_link() {
    // Symlinks delete as links (never followed), like the JVM.
    let dir = fixture("delete-symlink");
    let target_dir = dir.join("target");
    std::fs::create_dir(&target_dir).unwrap();
    std::fs::write(target_dir.join("f.txt"), b"x").unwrap();
    let link = dir.join("link");
    std::os::unix::fs::symlink(&target_dir, &link).unwrap();
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            &format!("(io/delete-file {})", clj_str(&link.to_string_lossy()))
        ),
        "true"
    );
    assert!(!std::path::Path::new(&link).exists() && target_dir.join("f.txt").exists());
}

// --- close/flush/`with-open` ---

#[test]
fn close_reader_and_reads_past_it() {
    let dir = fixture("close-reader");
    let p = write_bytes(&dir, "r.txt", b"l1\n");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (.close r))")
        ),
        "nil"
    );
    // Second closes no-op.
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (.close r) (.close r))")
        ),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (.close r) (binding [*in* r] (read-line)))")
        ),
        "java.io.IOException | Stream closed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (.close r) (binding [*in* r] (read)))")
        ),
        "java.io.IOException | Stream closed"
    );
}

#[test]
fn with_open_closes_reader() {
    let dir = fixture("with-open");
    let p = write_bytes(&dir, "r.txt", b"l1\nl2\n");
    let mut eval = jio();
    run(
        &mut eval,
        &format!("(def jio-r (io/reader {}))", clj_str(&p)),
    );
    assert_eq!(
        run(&mut eval, "(with-open [r jio-r] (vec (line-seq r)))"),
        r#"["l1" "l2"]"#
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* jio-r] (read-line))"),
        "java.io.IOException | Stream closed"
    );
}

#[test]
fn close_flush_writer_file() {
    let dir = fixture("close-writer");
    let p = dir.join("w.txt").to_string_lossy().into_owned();
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [w (io/writer {lit})] (.flush w))")
        ),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            &format!("(let [w (io/writer {lit})] (.close w) (.close w))")
        ),
        "nil"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [w (io/writer {lit})] (.close w) (.flush w))")
        ),
        "java.io.IOException | Stream closed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [w (io/writer {lit})] (.close w) (binding [*out* w] (print \"x\")))")
        ),
        "java.io.IOException | Stream closed"
    );
    // `(flush)` reaches a bound file writer too.
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [w (io/writer {lit})] (binding [*out* w] (print \"f\") (flush)) (slurp {lit}))"
            )
        ),
        r#""f""#
    );
}

#[test]
fn close_flush_miss_shapes() {
    let mut eval = jio();
    // Files have no `close`/`flush` (the miss shapes — and the
    // `None` arms of every close/flush downcast in the chain).
    let close_miss = uncaught(&mut eval, "(let [f (io/file \"a\")] (.close f))");
    assert!(
        close_miss.starts_with("java.lang.IllegalArgumentException | "),
        "{close_miss}"
    );
    let dir = fixture("flush-miss");
    let p = write_bytes(&dir, "i.bin", b"x");
    let flush_miss = uncaught(
        &mut eval,
        &format!("(let [s (io/input-stream {})] (.flush s))", clj_str(&p)),
    );
    assert!(
        flush_miss.starts_with("java.lang.IllegalArgumentException | "),
        "{flush_miss}"
    );
}

#[test]
fn stream_identity() {
    // Streams compare by identity (like every other opaque
    // handle) and hash as map keys.
    let dir = fixture("stream-ident");
    let p = write_bytes(&dir, "i.bin", b"x");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [s (io/input-stream {lit})] [(= s s) (= s (io/input-stream {lit})) (= s 1) (integer? (hash s))])"
            )
        ),
        "[true false false true]"
    );
    let q = dir.join("o.bin").to_string_lossy().into_owned();
    assert_eq!(
        run(
            &mut eval,
            &format!(
                "(let [s (io/output-stream {})] [(= s s) (= s (io/output-stream {})) (= s 1) (integer? (hash s))])",
                clj_str(&q),
                clj_str(&q)
            )
        ),
        "[true false false true]"
    );
}

#[test]
fn load_reader_past_close_is_stream_closed() {
    // `load-reader` honors the close latch like `read`/`read-line`
    // (round-1 L1 — it took the parser unchecked).
    let dir = fixture("load-closed");
    let p = write_bytes(&dir, "f.clj", b"(+ 1 2)");
    let mut eval = jio();
    let lit = clj_str(&p);
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(let [r (io/reader {lit})] (.close r) (load-reader r))")
        ),
        "java.io.IOException | Stream closed"
    );
}

// --- namespace shape ---

#[test]
fn ns_surface_is_six_publics() {
    let mut eval = jio();
    assert_eq!(
        run(
            &mut eval,
            "(mapv str (sort (keys (ns-publics 'clojure.java.io))))"
        ),
        r#"["delete-file" "file" "input-stream" "output-stream" "reader" "writer"]"#
    );
    // Reloading reinstalls cleanly.
    assert_eq!(run(&mut eval, "(require 'clojure.java.io :reload)"), "nil");
    assert_eq!(
        run(
            &mut eval,
            "(mapv str (sort (keys (ns-publics 'clojure.java.io))))"
        ),
        r#"["delete-file" "file" "input-stream" "output-stream" "reader" "writer"]"#
    );
}
