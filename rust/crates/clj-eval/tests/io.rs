// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D io smoke: `println`/`pr`/`newline`/`flush` captured via
//! `with-out-str`, a `spit`/`slurp` roundtrip (dirs owned by the
//! Rust side), and `read-string` basics.

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

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(_) => panic!("expected an uncaught throw, got values"),
    }
}

#[test]
fn io_smoke_rows() {
    let mut eval = boot();
    // Print fns captured through `with-out-str` (exact strings).
    assert_eq!(
        run_last(&mut eval, "(with-out-str (println 1))"),
        "\"1\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pr 1 \"a\"))"),
        "\"1 \\\"a\\\"\""
    );
    assert_eq!(run_last(&mut eval, "(with-out-str (newline))"), "\"\\n\"");
    assert_eq!(run_last(&mut eval, "(with-out-str (flush))"), "\"\"");
    // `spit`/`slurp` roundtrip: Rust owns the dirs, the path goes
    // into the form as a string literal.
    let dir = "./target/tmp-s5d-io";
    let file = "./target/tmp-s5d-io/round.txt";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(dir).unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(do (spit {file:?} \"hello\") (slurp {file:?}))")
        ),
        "\"hello\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(do (spit {file:?} \"!!\" :append true) (slurp {file:?}))")
        ),
        "\"hello!!\""
    );
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        // Only remove what we created; ignore a non-empty dir.
        let _ = std::fs::remove_dir("./target");
    }
    // `read-string` reads one form.
    assert_eq!(run_last(&mut eval, "(read-string \"1 2\")"), "1");
    assert_eq!(run_last(&mut eval, "(read-string \"[1 :a]\")"), "[1 :a]");
}

#[test]
fn gap2_writer_class_rows() {
    let mut eval = boot();
    // Each writer kind names its JVM class (io.rs:47-53, 57-59).
    assert_eq!(
        uncaught(&mut eval, "(count *out*)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: OutputStreamWriter"
    );
    assert_eq!(
        uncaught(&mut eval, "(count *err*)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: PrintWriter"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-out-str (count *out*))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: StringWriter"
    );
}

#[test]
fn gap2_writer_print_captured() {
    let mut eval = boot();
    // A buffer writer prints its fixed rendering (io.rs:61-64).
    let out = run_last(&mut eval, "(with-out-str (pr *out*))");
    assert!(
        out.starts_with("\"#object[java.io.StringWriter 0x"),
        "{out}"
    );
    assert!(out.contains("java.io.StringWriter@"), "{out}");
}

#[test]
fn gap2_writer_eq_hash_rows() {
    let mut eval = boot();
    // Writers compare by id; hashing routes through dyn_hash (io.rs:66-75).
    assert_eq!(run_last(&mut eval, "(= *out* *out*)"), "true");
    assert_eq!(run_last(&mut eval, "(= *out* *err*)"), "false");
    assert_eq!(run_last(&mut eval, "(distinct? *out* *err* :x)"), "true");
}

#[test]
fn gap2_out_binding_rows() {
    let mut eval = boot();
    // Nil and non-writer *out* bindings (io.rs:86, 138-141, 144-147).
    assert_eq!(
        uncaught(&mut eval, "(binding [*out* nil] (print 1))"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*out* 1] (print 1))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.io.Writer (java.lang.Long and java.io.Writer are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn gap2_stderr_write_row() {
    let mut eval = boot();
    // Writes one char to real stderr (uncapturable); returns nil (io.rs:154-157).
    assert_eq!(
        run_last(&mut eval, "(binding [*out* *err*] (print \"x\"))"),
        "nil"
    );
}

#[test]
fn gap2_stdout_write_row() {
    let mut eval = boot();
    // Unbound `*out*` is the real stdout writer (uncapturable, like
    // the stderr row above); returns nil (io.rs:150-153).
    assert_eq!(run_last(&mut eval, "(print 1)"), "nil");
}

#[test]
fn gap2_bare_print_row() {
    let mut eval = boot();
    // Bare `print` writes nothing, not even an empty write (io.rs:187).
    assert_eq!(run_last(&mut eval, "(print)"), "nil");
}

#[test]
fn gap2_time_print_failure() {
    let mut eval = boot();
    // A failing prn surfaces through time (io.rs:215).
    assert_eq!(
        uncaught(&mut eval, "(binding [*out* nil] (time 1))"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Writer.write(String)\" because \"w\" is null"
    );
}

#[test]
fn gap2_flush_stdout_row() {
    let mut eval = boot();
    // Top-level *out* is stdout, so flush runs it (io.rs:230).
    assert_eq!(run_last(&mut eval, "(flush)"), "nil");
}

#[test]
fn gap2_slurp_spit_arg_rows() {
    let mut eval = boot();
    // Arity and target shapes, then file-option shapes (io.rs:236-244, 251-266, 297-324).
    assert_eq!(
        uncaught(&mut eval, "(slurp)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/slurp"
    );
    assert_eq!(
        uncaught(&mut eval, "(slurp 1)"),
        "java.lang.IllegalArgumentException | Cannot open <1> as an InputStream."
    );
    assert_eq!(
        uncaught(&mut eval, "(spit \"x\")"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/spit"
    );
    assert_eq!(
        uncaught(&mut eval, "(spit 1 \"x\")"),
        "java.lang.IllegalArgumentException | Cannot open <1> as an OutputStream."
    );
    let dir = "./target/tmp-s5d-io-gap";
    let file = "./target/tmp-s5d-io-gap/args.txt";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(dir).unwrap();
    eval.eval_source(&format!("(spit {file:?} \"hello\")"))
        .unwrap();
    assert_eq!(
        uncaught(&mut eval, &format!("(spit {file:?} 1)")),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(slurp {file:?} :encoding)")),
        "java.lang.IllegalArgumentException | No value supplied for key: :encoding"
    );
    assert_eq!(
        uncaught(&mut eval, &format!("(slurp {file:?} :encoding \"latin1\")")),
        "java.lang.UnsupportedOperationException | unsupported encoding: latin1"
    );
    assert_eq!(
        run_last(&mut eval, &format!("(slurp {file:?} :encoding \"UTF-8\")")),
        "\"hello\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(slurp {file:?} \"encoding\" \"utf-8\")")
        ),
        "\"hello\""
    );
    assert_eq!(
        run_last(&mut eval, &format!("(slurp {file:?} :bogus 1)")),
        "\"hello\""
    );
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        let _ = std::fs::remove_dir("./target");
    }
}

#[test]
fn gap2_file_error_rows() {
    let mut eval = boot();
    // Missing files name the path; directories fail as IOException (io.rs:273-290).
    let missing = uncaught(
        &mut eval,
        "(slurp \"./target/tmp-s5d-io-gap-missing/m.txt\")",
    );
    assert!(
        missing
            .starts_with("java.io.FileNotFoundException | ./target/tmp-s5d-io-gap-missing/m.txt ("),
        "{missing}"
    );
    let dir = "./target/tmp-s5d-io-gapdir";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(dir).unwrap();
    let is_dir = uncaught(&mut eval, &format!("(slurp {dir:?})"));
    assert!(is_dir.starts_with("java.io.IOException | "), "{is_dir}");
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        let _ = std::fs::remove_dir("./target");
    }
}

#[test]
fn gap2_read_string_shape_rows() {
    let mut eval = boot();
    // Nil and non-string inputs (io.rs:348-354).
    assert_eq!(
        uncaught(&mut eval, "(read-string nil)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string 1)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
}

// ---- S-instream (issue #36): `*in*`, `read`, `read-line`,
// `line-seq`, `with-in-str`. Every test cites the
// `docs/i36-instream/probes/instream.out` row it pins.
// Tests never touch the real stdin root (it would block CI):
// string readers everywhere.

/// Normalize identity hex for print-shape pins (the probe `norm`).
fn norm_hex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == '0' && chars.peek() == Some(&'x') {
            out.push_str("0xH");
            chars.next();
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else if c == '@' && chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push_str("@H");
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

/// The `pr-str` of the root cause (the `throwcause` oracle shape:
/// root class + message pinned through the `#error` print).
fn root_cause(eval: &mut Evaluator, src: &str) -> String {
    run_last(
        eval,
        &format!("(try {src} :no-throw (catch Throwable t (pr-str (ex-cause t))))"),
    )
}

/// The expected `root_cause` for a `RuntimeException` root (the
/// `#error` print of a causeless throwable, quoted and escaped by
/// the outer `pr`).
fn cause_pr(msg: &str) -> String {
    format!(
        "\"#error {{:cause \\\"{msg}\\\" :data nil :via [{{:type java.lang.RuntimeException :message \\\"{msg}\\\"}}] :trace []}}\""
    )
}

#[test]
fn instream_in_shape_rows() {
    let mut eval = boot();
    // Both reader kinds are `LineNumberingPushbackReader`s (probes
    // `in-class`, `wis-class`; pinned through the `count` simple
    // name — `class` is out of scope).
    assert_eq!(
        uncaught(&mut eval, "(count *in*)"),
        "java.lang.UnsupportedOperationException | count not supported on this type: LineNumberingPushbackReader"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"x\" (count *in*))"),
        "java.lang.UnsupportedOperationException | count not supported on this type: LineNumberingPushbackReader"
    );
    // The `#object` print, identity hex normalized (probe `in-pr`).
    assert_eq!(
        norm_hex(&run_last(&mut eval, "(pr-str *in*)")),
        "\"#object[clojure.lang.LineNumberingPushbackReader 0xH \\\"clojure.lang.LineNumberingPushbackReader@H\\\"]\""
    );
    // Readers compare and hash by id (fresh readers differ).
    assert_eq!(run_last(&mut eval, "(= *in* *in*)"), "true");
    assert_eq!(
        run_last(&mut eval, "(= *in* (with-in-str \"x\" *in*))"),
        "false"
    );
    assert_eq!(run_last(&mut eval, "(distinct? *in* *in*)"), "false");
    assert_eq!(
        run_last(&mut eval, "(distinct? *in* (with-in-str \"x\" *in*))"),
        "true"
    );
}

#[test]
fn instream_read_line_rows() {
    let mut eval = boot();
    // Terminator shapes (probes `rl-basic`, `rl-crlf`, `rl-cr`,
    // `rl-mixed`, `rl-empty`, `rl-trailing-nl`, `rl-empty-lines`).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\nb\" [(read-line) (read-line) (read-line)])"
        ),
        "[\"a\" \"b\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\r\nb\" [(read-line) (read-line) (read-line)])"
        ),
        "[\"a\" \"b\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\rb\" [(read-line) (read-line) (read-line)])"
        ),
        "[\"a\" \"b\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\r\n\rb\nc\rd\" [(read-line) (read-line) (read-line) (read-line) (read-line) (read-line)])"
        ),
        "[\"a\" \"\" \"b\" \"c\" \"d\" nil]"
    );
    assert_eq!(run_last(&mut eval, "(with-in-str \"\" (read-line))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"a\n\" [(read-line) (read-line)])"),
        "[\"a\" nil]"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\n\nb\" [(read-line) (read-line) (read-line) (read-line)])"
        ),
        "[\"a\" \"\" \"b\" nil]"
    );
    // Nil `*in*` is the verbatim `Var.get()` NPE (probe `rl-nil-in`).
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* nil] (read-line))"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.BufferedReader.readLine()\" because the return value of \"clojure.lang.Var.get()\" is null"
    );
    // Non-readers fail the `BufferedReader` cast (probes
    // `rl-str-in`, `rl-num-in` — the port's `cce_message` reproduces
    // the JVM module suffixes verbatim).
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* \"x\"] (read-line))"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.BufferedReader (java.lang.String and java.io.BufferedReader are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* 1] (read-line))"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.io.BufferedReader (java.lang.Long and java.io.BufferedReader are in module java.base of loader 'bootstrap')"
    );
    // `read-line` is 0-arity only (probe `rl-arity1`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"x\" (read-line 1))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/read-line"
    );
}

#[test]
fn instream_read_value_rows() {
    let mut eval = boot();
    // Plain forms across the arities (probes `r-basic`,
    // `r-shapes`, `r-comment`, `r-discard`, `r-meta`,
    // `r-kw-colons`, `r-line-meta`, `r-recursive`,
    // `r-recursive-discard`).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1 2\" [(read) (read)])"),
        "[1 2]"
    );
    assert_eq!(run_last(&mut eval, "(with-in-str \"1\" (read *in*))"), "1");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"1 1.5 \\\"s\\\" :k sym \\\\c\" [(read) (read) (read) (read) (read) (read)])"
        ),
        "[1 1.5 \"s\" :k sym \\c]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"; c\n42\" (read))"),
        "42"
    );
    assert_eq!(run_last(&mut eval, "(with-in-str \"#_ 1 2\" (read))"), "2");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"^:k [1]\" (let [v (read)] [(pr-str v) (meta v)]))"
        ),
        "[\"[1]\" {:k true}]"
    );
    assert_eq!(run_last(&mut eval, "(with-in-str \":a:b\" (read))"), ":a:b");
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"(a)\n(b)\" (do (read) (meta (read))))"
        ),
        "{:line 2, :column 1}"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1\" (read *in* true nil true))"),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#_ 1 2\" (read *in* true nil true))"
        ),
        "2"
    );
    // Reader conditionals through the opts map (probes
    // `r-readcond-allow`, `r-readcond-preserve`, `r-features`,
    // `r-splice-nested`).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:clj 1 :default 2)\" (read {:read-cond :allow} *in*))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:clj 1 :default 2)\" (read {:read-cond :preserve} *in*))"
        ),
        "#?(:clj 1 :default 2)"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:foo 1 :default 2)\" (read {:read-cond :allow :features #{:foo}} *in*))"
        ),
        "1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"(1 #?@(:clj [2 3]))\" (read {:read-cond :allow} *in*))"
        ),
        "(1 2 3)"
    );
}

#[test]
fn instream_read_eof_rows() {
    let mut eval = boot();
    // Clean EOF honors `eof-error?` (probes `r-eof-val`,
    // `r-eof-nil`, `r-eof-false`, `r-eof-ws`, `r-eoferr-nil`).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read *in* false :eof))"),
        ":eof"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read *in* false nil))"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read *in* false false))"),
        "false"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"  \" (read *in* false :eof))"),
        ":eof"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read *in* nil :eof))"),
        ":eof"
    );
    // Clean EOF throws when asked (probes `r-eof-throw`, `r-eof-3`,
    // `r-eoferr-truthy` — roots; `r-eof-outer` — the wrapper).
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\" (read))"),
        cause_pr("EOF while reading")
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\" (read *in* true nil))"),
        cause_pr("EOF while reading")
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\" (read *in* 0 :eof))"),
        cause_pr("EOF while reading")
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading"
    );
    // The `:eof` presence rule (probes `r-opts-eof`,
    // `r-opts-eofnil`, `r-opts-noeof`, `r-opts-unknown`,
    // `r-opts-nil`).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read {:eof :x} *in*))"),
        ":x"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (read {:eof nil} *in*))"),
        "nil"
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\" (read {} *in*))"),
        cause_pr("EOF while reading")
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1\" (read {:bogus 1} *in*))"),
        "1"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1\" (read nil *in*))"),
        "1"
    );
    // Clean EOF ex-data sits at the live position (probe
    // `r-exdata-eof`); `recursive?` rethrows the raw root (probe
    // `r-recursive-outer` — no wrapper, no data, no cause).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"\" (read)) :no-throw (catch Throwable t (ex-data t)))"
        ),
        "#:clojure.error{:line 1, :column 1}"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"\" (read *in* true nil true))"),
        "java.lang.RuntimeException | EOF while reading"
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\" (read *in* true nil true))"),
        "\"nil\""
    );
}

#[test]
fn instream_read_error_rows() {
    let mut eval = boot();
    // Truncated forms throw even with `eof-error?` false (probes
    // `r-trunc`, `r-trunc-str`, `r-trunc-char`, `r-trunc-regex` —
    // roots; `r-trunc-outer` — the wrapper).
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"(a\" (read *in* false :eof))"),
        cause_pr("EOF while reading, starting at line 1")
    );
    assert_eq!(
        root_cause(
            &mut eval,
            "(with-in-str \"\\\"abc\" (read *in* false :eof))"
        ),
        cause_pr("EOF while reading string")
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"\\\\\" (read *in* false :eof))"),
        cause_pr("EOF while reading character")
    );
    assert_eq!(
        root_cause(
            &mut eval,
            "(with-in-str \"#\\\"ab\" (read *in* false :eof))"
        ),
        cause_pr("EOF while reading regex")
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"(a\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading, starting at line 1"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"(a\" (read *in* false :eof))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading, starting at line 1"
    );
    // Rejected forms (probes `r-baddelim`, `r-baddelim-exdata`,
    // `r-cond-plain`, `r-splice-opts`, `r-cond-opts-absent`,
    // `r-splice-top`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \")\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Unmatched delimiter: )"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \")\" (read)) :no-throw (catch Throwable t (ex-data t)))"
        ),
        "#:clojure.error{:line 1, :column 2}"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#?(:clj 1 :default 2)\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#?@(:clj [1 2])\" (read {:read-cond :allow} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Reader conditional splicing not allowed at the top level."
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#?(:clj 1 :default 2)\" (read {:eof :x} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"#?@(:clj [1 2])\" (read))"),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
    // The deny throw consumes `#?` only, so the next read sees the
    // still-spliceless `@` form (probe `r-splice-then-ok`).
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?@(:clj [1]) 2\" [(try (read) :no-throw (catch Throwable t :threw)) (read)])"
        ),
        "[:threw (clojure.core/deref (:clj [1]))]"
    );
    // A failed preserve read must not stick the conditional flag:
    // the next tag read on the same reader still throws (probe
    // `r-preserve-fail-then-tag`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#?(a b] #foo \\\"x\\\"\" [(try (read {:read-cond :preserve} *in*) :no-throw (catch Throwable t :caught)) (read {:read-cond :preserve} *in*)])"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: No reader function for tag foo"
    );
    // Ex-data and cause pins (probes `r-exdata`, `r-excause`).
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) :no-throw (catch Throwable t (ex-data t)))"
        ),
        "#:clojure.error{:line 2, :column 1}"
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"(a\" (read))"),
        cause_pr("EOF while reading, starting at line 1")
    );
    // Bad stream operands (probes `r-nil-stream`, `r-str-stream`).
    assert_eq!(
        uncaught(&mut eval, "(read nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Reader.read()\" because \"r\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(binding [*in* nil] (read))"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Reader.read()\" because \"r\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(read \"1\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.PushbackReader (java.lang.String and java.io.PushbackReader are in module java.base of loader 'bootstrap')"
    );
    // Bad opts operands (probes `r-opts-nonmap`,
    // `r-opts-mapentry`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"1\" (read :foo *in*))"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.Keyword and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"1\" (read (find {:a 1} :a) *in*))"
        ),
        "java.lang.ClassCastException | class clojure.lang.MapEntry cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.MapEntry and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
}

#[test]
fn instream_read_opts_edge_rows() {
    let mut eval = boot();
    // There is no `[stream eof-error?]` arity: two args are always
    // `[opts stream]` (probe `r-2arity-boolstream`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"1\" (read *in* false))"),
        "java.lang.ClassCastException | class java.lang.Boolean cannot be cast to class java.io.PushbackReader (java.lang.Boolean and java.io.PushbackReader are in module java.base of loader 'bootstrap')"
    );
    // The stream type-check runs before opts validation, but the
    // nil-stream NPE runs after it (probes `r-optsbad-nilstream`,
    // `r-eofopts-nilstream`).
    assert_eq!(
        uncaught(&mut eval, "(read :foo nil)"),
        "java.lang.ClassCastException | class clojure.lang.Keyword cannot be cast to class clojure.lang.IPersistentMap (clojure.lang.Keyword and clojure.lang.IPersistentMap are in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(read {:eof :x} nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.io.Reader.read()\" because \"r\" is null"
    );
    // `:features` must be a set (probe `r-feat-vec`); nil keeps
    // the default (probe `r-feat-nil`); set members are never
    // validated (probe `r-feat-set-nonkw`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"1\" (read {:read-cond :allow :features [:foo]} *in*))"
        ),
        "java.lang.ClassCastException | class clojure.lang.PersistentVector cannot be cast to class clojure.lang.IPersistentSet (clojure.lang.PersistentVector and clojure.lang.IPersistentSet are in unnamed module of loader 'app')"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"#?(:foo 1 :default 2)\" (read {:read-cond :allow :features nil} *in*))"
        ),
        "2"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"1\" (read {:read-cond :allow :features #{1}} *in*))"
        ),
        "1"
    );
    // Non-`:allow`/`:preserve` `:read-cond` denies (probes
    // `r-rc-bogus`, `r-rc-nil`); five args are an arity error
    // (probe `r-arity5`).
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#?(:clj 1 :default 2)\" (read {:read-cond :bogus} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(with-in-str \"#?(:clj 1 :default 2)\" (read {:read-cond nil} *in*))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: Conditional read not allowed"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"1\" (read 1 2 3 4 5))"),
        "clojure.lang.ArityException | Wrong number of args (5) passed to: clojure.core/read"
    );
}

#[test]
fn instream_mix_rows() {
    let mut eval = boot();
    // `read` and `read-line` share one position (probes `mix-1`,
    // `mix-2`, `mix-3`).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"a\n(b)\" [(read-line) (read)])"),
        "[\"a\" (b)]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1 2\" [(read) (read-line)])"),
        "[1 \" 2\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"(a) tail\" [(read) (read-line)])"),
        "[(a) \" tail\"]"
    );
}

#[test]
fn instream_line_seq_rows() {
    let mut eval = boot();
    // The port consumes its own readable by design (the JVM
    // rejects `LineNumberingPushbackReader` — probes `ls-basic`,
    // `ls-class`, `ls-lazy`, `ls-empty` document the JVM side);
    // values, laziness, and nil-termination match the
    // `BufferedReader` rows (probes `ls-basic2`, `ls-class2`,
    // `ls-lazy2`, `ls-empty2`).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"x\ny\" (doall (line-seq *in*)))"),
        "(\"x\" \"y\")"
    );
    // A `Cons` head (`seq?`, like the JVM) that is already
    // realized (a lazy head would answer `false` instead of
    // failing the `IPending` cast — probe `ls-lazy` shape).
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"x\" (seq? (line-seq *in*)))"),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"x\" (realized? (line-seq *in*)))"),
        "java.lang.ClassCastException | class clojure.lang.Cons cannot be cast to class clojure.lang.IPending (clojure.lang.Cons and clojure.lang.IPending are in unnamed module of loader 'app')"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"x\ny\" (let [s (line-seq *in*)] [(realized? (rest s)) (first (rest s)) (realized? (rest s))]))"
        ),
        "[false \"y\" true]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"\" (line-seq *in*))"),
        "nil"
    );
    // The first pull is eager: no `doall` needed to throw (probes
    // `ls-eager`, `ls-str-eager`). No `ls-nil` text pin: the JVM
    // throws its direct-`.readLine` NPE there, while the port's
    // `binding`-based `line-seq` throws the `rl-nil-in` text.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (line-seq nil) :no-throw (catch Throwable t :threw-at-call))"
        ),
        ":threw-at-call"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (line-seq \"x\") :no-throw (catch Throwable t :threw-at-call))"
        ),
        ":threw-at-call"
    );
    // A string rdr fails the `BufferedReader` cast with the probe
    // text (probe `ls-str`); the seq never closes (probe
    // `ls-noclose` — the exhausted reader still reads nil).
    assert_eq!(
        uncaught(&mut eval, "(line-seq \"x\")"),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class java.io.BufferedReader (java.lang.String and java.io.BufferedReader are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\" (do (doall (line-seq *in*)) (read-line)))"
        ),
        "nil"
    );
}

#[test]
fn instream_with_in_str_rows() {
    let mut eval = boot();
    // The body's value escapes; the outer `*in*` is untouched
    // afterwards (probes `wis-val`, `wis-restore` — pinned through
    // `=`, since `identical?` is out of scope); nestings rebind
    // (probe `wis-nested`); reads advance (probe `wis-read`).
    assert_eq!(run_last(&mut eval, "(with-in-str \"1\" :body)"), ":body");
    assert_eq!(run_last(&mut eval, "(with-in-str \"x\")"), "nil");
    assert_eq!(
        run_last(
            &mut eval,
            "(let [out *in*] (with-in-str \"x\" :y) (= out *in*))"
        ),
        "true"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(let [out *in*] (with-in-str \"x\" (= out *in*)))"
        ),
        "false"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(with-in-str \"a\" [(read-line) (with-in-str \"b\" (read-line)) (read-line)])"
        ),
        "[\"a\" \"b\" nil]"
    );
    assert_eq!(
        run_last(&mut eval, "(with-in-str \"1 2\" [(read) (read)])"),
        "[1 2]"
    );
    // A non-string `s` fails the `String` cast (probe `wis-num`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str 1 :x)"),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    // `[s & body]`: the string is required (probe `wis-arity0`).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/with-in-str"
    );
}

#[test]
fn instream_catch_rows() {
    let mut eval = boot();
    // `ReaderException` is caught by both the short and the FQN
    // spelling and by its `RuntimeException`/`Exception` parents;
    // sibling classes do not catch it.
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) (catch ReaderException e :short))"
        ),
        ":short"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) (catch clojure.lang.LispReader$ReaderException e :fqn))"
        ),
        ":fqn"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) (catch RuntimeException e :runtime))"
        ),
        ":runtime"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) (catch Exception e :exception))"
        ),
        ":exception"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(try (with-in-str \"(a\" (read)) (catch IllegalArgumentException e :sibling))"
        ),
        "clojure.lang.LispReader$ReaderException | java.lang.RuntimeException: EOF while reading, starting at line 1"
    );
}

#[test]
fn instream_recursive_unwrap_rows() {
    let mut eval = boot();
    // `recursive?` rethrows the raw root: no wrapper, no ex-data,
    // no cause (the `r-recursive-outer` rule on a truncated form).
    assert_eq!(
        uncaught(&mut eval, "(with-in-str \"(a\" (read *in* true nil true))"),
        "java.lang.RuntimeException | EOF while reading, starting at line 1"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(try (with-in-str \"(a\" (read *in* true nil true)) :no-throw (catch RuntimeException e (ex-data e)))"
        ),
        "nil"
    );
    assert_eq!(
        root_cause(&mut eval, "(with-in-str \"(a\" (read *in* true nil true))"),
        "\"nil\""
    );
}

#[test]
fn instream_read_string_deny_row() {
    let mut eval = boot();
    // The drive-by: `read-string` rejects reader conditionals
    // (probe `rs-cond`); its EOF errors are unchanged (probes
    // `rs-empty`, `rs-trunc`).
    assert_eq!(
        uncaught(&mut eval, "(read-string \"#?(:clj 1 :default 2)\")"),
        "java.lang.RuntimeException | Conditional read not allowed"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"\")"),
        "java.lang.RuntimeException | EOF while reading"
    );
    assert_eq!(
        uncaught(&mut eval, "(read-string \"(a\")"),
        "java.lang.RuntimeException | EOF while reading"
    );
}
