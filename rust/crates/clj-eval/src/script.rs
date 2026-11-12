// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The `run` script frame and its failure rendering, shared by
//! the `clojure-rs run` CLI and AOT-built binaries (issue #256:
//! mixed builds load their embedded source through this exact
//! frame, so the two cannot drift).

use crate::error::compile_head;
use crate::eval::SourceError;

/// The `run` wrapper around `load`, a form that loads the program
/// (`(load-file "x.clj")` for `run`): upstream
/// `clojure.main/with-bindings` thread bindings plus the up-front
/// `clojure.string` require (see `cmd_run` for the rationale).
#[must_use]
pub fn run_script(args: &[String], load: &str) -> String {
    let inits = args_expr(args);
    format!(
        "(binding [*ns* *ns* \
         *warn-on-reflection* *warn-on-reflection* \
         *math-context* *math-context* \
         *print-meta* *print-meta* \
         *print-length* *print-length* \
         *print-level* *print-level* \
         *print-namespace-maps* true \
         *data-readers* *data-readers* \
         *default-data-reader-fn* *default-data-reader-fn* \
         *compile-path* *compile-path* \
         *command-line-args* {inits} \
         *unchecked-math* *unchecked-math* \
         *assert* *assert* \
         *1 nil *2 nil *3 nil *e nil] \
         (require 'clojure.string) \
         {load})"
    )
}

/// The `*command-line-args*` init (pins P4): nil without args, else a
/// quoted list of strings (a list value on read).
pub fn args_expr(args: &[String]) -> String {
    if args.is_empty() {
        return "nil".to_owned();
    }
    let mut out = "(quote (".to_owned();
    for (i, arg) in args.iter().enumerate() {
        if i > 0 {
            out.push(' ');
        }
        out.push_str(&clj_string(arg));
    }
    out.push_str("))");
    out
}

/// A Rust string as a Clojure string literal.
pub fn clj_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for c in s.chars() {
        match c {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 || c as u32 == 0x7f => {
                out.push_str(&format!("\\u{:04x}", c as u32));
            }
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

/// The load wrapper's class: only `load-file` compile/reader failures
/// carry a parseable ` at (site).` tail.
const COMPILER_EXCEPTION: &str = "clojure.lang.Compiler$CompilerException";

/// A `run` failure as `file:line:col: message` naming the path as
/// passed. `load-file` wraps file errors as `... at (site).` with the
/// real message on the cause chain, so the site is recovered from the
/// wrapper text; anything else (notably span-less runtime throws)
/// blames 1:1.
pub fn run_diagnostic(path: &str, e: &SourceError) -> String {
    match e {
        SourceError::Reader(e) => {
            let span = e.span();
            let msg = e.message();
            format!("{path}:{}:{}: {msg}", span.line, span.col)
        }
        SourceError::Compile(e) => {
            // `compile_err` converts every bare failure to `Uncaught`
            // before `eval_source` returns, so `e.bare` is always
            // false here (the single caller passes `eval_source`
            // errors only).
            let wrapper = e.wrapper();
            let head = compile_head(&wrapper);
            let msg = &e.message;
            format!("{path}:1:1: {head}: {msg}")
        }
        SourceError::Uncaught(t) => {
            let outer = t.message.as_deref().unwrap_or("");
            let compiler = t.class.jvm_name() == COMPILER_EXCEPTION;
            match load_site(outer) {
                Some((line, col)) if compiler => {
                    let mut msg = outer;
                    let mut cur = t;
                    while let Some(next) = cur.cause.as_deref() {
                        if let Some(m) = next.message.as_deref() {
                            msg = m;
                        }
                        cur = next;
                    }
                    let head = compile_head(outer);
                    format!("{path}:{line}:{col}: {head}: {msg}")
                }
                _ => {
                    let class = t.class.jvm_name();
                    format!("{path}:1:1: {class}: {outer}")
                }
            }
        }
        SourceError::Internal(msg) => format!("{path}:1:1: internal error: {msg}"),
    }
}

/// Recover `(line, col)` from a load wrapper's trailing ` at (site).`
/// (`site` is `file:line:col`, so the last two `:` segments parse;
/// the file part itself may hold colons).
fn load_site(message: &str) -> Option<(u32, u32)> {
    let rest = message.strip_suffix(").")?;
    let i = rest.rfind(" at (")?;
    let site = &rest[i + 5..];
    let mut parts = site.rsplit(':');
    let col: u32 = parts.next()?.parse().ok()?;
    let line: u32 = parts.next()?.parse().ok()?;
    parts.next()?;
    Some((line, col))
}

#[cfg(test)]
mod tests {
    use super::{clj_string, run_diagnostic, run_script};
    use crate::eval::SourceError;
    use crate::ns::boot;

    #[test]
    fn run_diagnostic_internal_blames_script_start() {
        let err = SourceError::Internal("boom".to_owned());
        assert_eq!(
            run_diagnostic("script.clj", &err),
            "script.clj:1:1: internal error: boom"
        );
    }

    #[test]
    fn run_diagnostic_reader_blames_script_span() {
        // The `Reader` arm is defensive (`cmd_run`'s generated
        // wrapper always reads clean), so a REAL reader error from
        // the same `read_forms` exercises the formatting.
        let err = boot()
            .read_forms_live(")")
            .expect_err("stray closer must not read");
        assert!(matches!(&err, SourceError::Reader(_)));
        assert_eq!(
            run_diagnostic("script.clj", &err),
            "script.clj:1:2: Unmatched delimiter: )"
        );
    }

    #[test]
    fn run_diagnostic_compile_blames_script_start() {
        // The `Compile` arm is defensive (the wrapper always
        // compiles), so a REAL analyzer error exercises it: `nope`
        // fails resolve — never bare (bare is macro-arity/`declare`
        // only).
        let mut eval = boot();
        let err = eval.eval_source("nope").expect_err("nope must not resolve");
        assert!(matches!(&err, SourceError::Compile(_)));
        assert_eq!(
            run_diagnostic("script.clj", &err),
            "script.clj:1:1: Syntax error compiling: Unable to resolve symbol: nope in this context"
        );
    }

    #[test]
    fn clj_string_escapes_every_special() {
        assert_eq!(
            clj_string("a\"b\\c\nd\re\tf\x01g\x7fh"),
            "\"a\\\"b\\\\c\\nd\\re\\tf\\u0001g\\u007fh\""
        );
    }

    #[test]
    fn run_script_wraps_load_and_args() {
        let script = run_script(&["a".to_owned(), "b\"".to_owned()], "(load-file \"x.clj\")");
        assert!(
            script.contains("*command-line-args* (quote (\"a\" \"b\\\"\"))"),
            "{script}"
        );
        assert!(
            script.ends_with("(require 'clojure.string) (load-file \"x.clj\"))"),
            "{script}"
        );
        assert!(run_script(&[], "x").contains("*command-line-args* nil"));
    }
}
