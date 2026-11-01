// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! #120 slice 3: `clojure.java.shell` facsimile — `sh` over the
//! spawn native, `with-sh-dir` / `with-sh-env` over `binding`.
//! Unix-only: the hermetic commands (`echo`, `sh`, `cat`) have no
//! Windows counterparts (the established `#[cfg(unix)]` pattern).

#![cfg(unix)]

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

fn require_sh(eval: &mut Evaluator) {
    run(eval, "(require '[clojure.java.shell :as sh])");
}

#[test]
fn shell_ns_loads() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(do (require 'clojure.java.shell) :ok)"),
        ":ok"
    );
}

#[test]
fn echo_returns_exit_out_err() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            r#"(= (sh/sh "echo" "hi") {:exit 0 :out "hi\n" :err ""})"#
        ),
        "true"
    );
}

#[test]
fn exit_code_and_stderr_pin() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(run(&mut eval, r#"(:exit (sh/sh "sh" "-c" "exit 3"))"#), "3");
    assert_eq!(
        run(&mut eval, r#"(:err (sh/sh "sh" "-c" "echo oops 1>&2"))"#),
        r#""oops\n""#
    );
}

#[test]
fn stdin_string_and_bytes() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(&mut eval, r#"(:out (sh/sh "cat" :in "hello\n"))"#),
        r#""hello\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "cat" :in (byte-array [104 105 10])))"#
        ),
        r#""hi\n""#
    );
}

#[test]
fn out_enc_bytes_and_names() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            r#"(vec (:out (sh/sh "echo" "hi" :out-enc :bytes)))"#
        ),
        "[104 105 10]"
    );
    assert_eq!(
        run(&mut eval, r#"(:out (sh/sh "echo" "hi" :out-enc "UTF-8"))"#),
        r#""hi\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "echo" "hi" :out-enc "ISO-8859-1"))"#
        ),
        r#""hi\n""#
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "echo" "hi" :out-enc "BOGUS")"#),
        "java.io.UnsupportedEncodingException | BOGUS"
    );
}

#[test]
fn env_map_replaces_environment() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "sh" "-c" "echo $FOO" :env {"FOO" "bar"}))"#
        ),
        r#""bar\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "sh" "-c" "echo $FOO" :env {:FOO "bar"}))"#
        ),
        r#""bar\n""#
    );
    // Replacement (not merge): with :env {}, the parent's HOME is
    // gone from the child. Control first: without :env the probe
    // sees a real HOME (fails loudly if the runner lacks one).
    assert_ne!(
        run(
            &mut eval,
            r#"(:out (sh/sh "sh" "-c" "echo ${HOME-unset}"))"#
        ),
        r#""unset\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "sh" "-c" "echo ${HOME-unset}" :env {}))"#
        ),
        r#""unset\n""#
    );
}

#[test]
fn dir_and_with_macros() {
    let mut eval = boot();
    require_sh(&mut eval);
    let dir = std::env::temp_dir().join(format!("clj-rs-sh-{}", std::process::id()));
    std::fs::create_dir_all(&dir).expect("tmp dir");
    std::fs::write(dir.join("marker"), b"x").expect("marker");
    let d = dir.to_string_lossy().replace('\\', "\\\\");
    assert_eq!(
        run(
            &mut eval,
            &format!(r#"(:out (sh/sh "sh" "-c" "test -f marker && echo yes" :dir "{d}"))"#)
        ),
        r#""yes\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            &format!(
                r#"(:out (sh/with-sh-dir "{d}" (sh/sh "sh" "-c" "test -f marker && echo yes")))"#
            )
        ),
        r#""yes\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/with-sh-env {"FOO" "wse"} (sh/sh "sh" "-c" "echo $FOO")))"#
        ),
        r#""wse\n""#
    );
    std::fs::remove_dir_all(&dir).expect("cleanup");
}

#[test]
fn in_enc_rules() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "cat" :in (byte-array [65]) :in-enc "BOGUS"))"#
        ),
        r#""A""#
    );
    assert_eq!(
        run(&mut eval, r#"(:out (sh/sh "echo" "x" :in-enc "BOGUS"))"#),
        r#""x\n""#
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "cat" :in "x" :in-enc "BOGUS")"#),
        "java.io.UnsupportedEncodingException | BOGUS"
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "cat" :in "q\n" :in-enc "UTF-8"))"#
        ),
        r#""q\n""#
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "cat" :in "x" :in-enc 5)"#),
        "java.io.UnsupportedEncodingException | 5"
    );
}

#[test]
fn keyword_out_enc_and_symbol_env_key() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        run(&mut eval, r#"(:out (sh/sh "echo" "hi" :out-enc :utf-8))"#),
        r#""hi\n""#
    );
    assert_eq!(
        run(
            &mut eval,
            r#"(:out (sh/sh "sh" "-c" "echo $FOO" :env {'FOO "bar"}))"#
        ),
        r#""bar\n""#
    );
}

#[test]
fn proc_validates_command_shape() {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.java.shell.impl :as impl])");
    assert_eq!(
        uncaught(&mut eval, r#"(impl/proc "echo" nil nil nil nil nil)"#),
        "java.lang.ClassCastException | class java.lang.String cannot be cast to class clojure.lang.IPersistentVector (java.lang.String is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentVector is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(impl/proc ["echo" 5] nil nil nil nil nil)"#),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn missing_binary_reports_io() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "nope-missing-bin-xyz")"#),
        r#"java.io.IOException | Cannot run program "nope-missing-bin-xyz": No such file or directory (os error 2)"#
    );
}

#[test]
fn arg_shapes_reject() {
    let mut eval = boot();
    require_sh(&mut eval);
    assert_eq!(
        uncaught(&mut eval, "(sh/sh)"),
        "java.lang.IllegalArgumentException | sh requires at least one command string"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "cat" :in 5)"#),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "cat" :in (char-array [\a]))"#),
        "java.lang.ClassCastException | class [C cannot be cast to class [B ([C and [B are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "echo" "x" :env 5)"#),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.IPersistentMap (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.IPersistentMap is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "echo" "x" :env {5 "x"})"#),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class clojure.lang.Named (java.lang.Long is in module java.base of loader 'bootstrap'; clojure.lang.Named is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "echo" "x" :dir 5)"#),
        "java.lang.ClassCastException | class java.lang.Long cannot be cast to class java.lang.String (java.lang.Long and java.lang.String are in module java.base of loader 'bootstrap')"
    );
    assert_eq!(
        uncaught(&mut eval, r#"(sh/sh "echo" "x" :out-enc 5)"#),
        "java.io.UnsupportedEncodingException | 5"
    );
}
