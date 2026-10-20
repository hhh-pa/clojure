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
