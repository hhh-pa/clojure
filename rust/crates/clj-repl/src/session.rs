// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S7B PTY-less REPL core: [`Session`] runs prompt → read → eval →
//! print → loop over generic IO (`BufRead` + `Write`, no tty
//! dependency). Prompt/banner policy lives in the S7C tty adapter
//! and the `bin`; the core prints a prompt only when
//! [`set_prompt`](Session::set_prompt) enables one (streaming purity
//! per P6: prompt-off transcripts carry one result per form).
//!
//! `*1`/`*2`/`*3`/`*e` rules follow P1/P2 (every success shifts,
//! including nil; errors never shift and bind `*e`), updates run
//! through the evaluator's thread-binding machinery (pushed session
//! frame + `var-set` writes, so user `set!`/`binding` participate).
//! Shift-before-print order and EOF-mid-form reporting are
//! JVM-probed (`(+ 1`-at-EOF prints its diagnostic, then exits 0).
//!
//! One buffer iterates per-form like the JVM: each form evals,
//! prints, and shifts `*1` in turn, stopping at the first error
//! (so `1 nope` prints `1`, then the `Unable to resolve`
//! diagnostic — earlier values are never dropped).

use std::io::{BufRead, Write};
use std::sync::Arc;

use clj_eval::{Evaluator, NativeError, Signal, SourceError};
use clj_value::{Symbol, ThrowClass, Throwable, Value, Var, pr_to_string};

/// Resolve an interned `clojure.core` var to a driver handle (`None`
/// when the evaluator never booted it — e.g. a bare `Evaluator`).
/// S7A's `run` path reuses this for `*command-line-args*`.
#[must_use]
pub fn core_var(eval: &Evaluator, name: &str) -> Option<Var> {
    let sym = Symbol::new(Some("clojure.core"), name);
    let id = eval.registry.get("clojure.core")?.lookup(&sym)?;
    Some(Var { id, sym })
}

/// Whether a reader message means "incomplete form, read more". The
/// four incomplete shapes (`EofDelimited`/`EofString`/`EofChar`/
/// `EofRegex`) all extend `EOF while reading` with a suffix; the
/// bare top-level `EOF while reading` means done, never continue.
/// Pure over the oracle-pinned message strings (`reader-contract.md`
/// §22), so it needs no `clj-reader` dependency.
#[must_use]
pub fn is_incomplete_message(message: &str) -> bool {
    message.starts_with("EOF while reading") && message != "EOF while reading"
}

/// Whether a source error is an incomplete form (read more lines)
/// rather than a hard failure (report and continue).
#[must_use]
pub fn is_incomplete_error(err: &SourceError) -> bool {
    match err {
        SourceError::Reader(e) => is_incomplete_message(&e.message()),
        SourceError::Compile(_) | SourceError::Uncaught(_) | SourceError::Internal(_) => false,
    }
}

/// Whether `buf` reads as a complete unit (side-effect-free probe
/// for the tty accumulator + validator: balance is structural, so
/// default reader opts agree with the session's own read).
#[must_use]
pub fn read_is_complete(buf: &str) -> bool {
    match clj_reader::read_all(buf, &clj_reader::opts::ReaderOpts::default()) {
        Ok(_) => true,
        Err(e) => !is_incomplete_message(&e.message()),
    }
}

/// How one accumulated buffer resolves.
enum Step {
    /// The buffer is done; the payload is the `run` status (nonzero
    /// only on IO failure — otherwise the loop continues).
    Done(i32),
    /// Incomplete form; the payload reports it when EOF strands the
    /// buffer (the JVM prints the pending diagnostic, then exits).
    More(SourceError),
}

/// One REPL session over generic IO: `R` feeds input lines, `W`
/// carries prompts, results, and diagnostics. Owns its evaluator;
/// `run` returns the process exit status (0 clean EOF, 1 IO
/// failure — user errors never kill the session).
pub struct Session<R, W> {
    eval: Evaluator,
    reader: R,
    writer: W,
    prompt: Option<String>,
    star1: Option<Var>,
    star2: Option<Var>,
    star3: Option<Var>,
    star_e: Option<Var>,
}

impl<R: BufRead, W: Write> Session<R, W> {
    /// A session over `evaluator`/`reader`/`writer` with prompts OFF
    /// (streaming purity). Resolves `*1`/`*2`/`*3`/`*e` and pushes
    /// the session binding frame (nil entries); the frame lives as
    /// long as the session. Missing handles (a non-booted
    /// evaluator) skip their updates rather than failing.
    #[must_use]
    pub fn new(mut evaluator: Evaluator, reader: R, writer: W) -> Self {
        let star1 = core_var(&evaluator, "*1");
        let star2 = core_var(&evaluator, "*2");
        let star3 = core_var(&evaluator, "*3");
        let star_e = core_var(&evaluator, "*e");
        for var in [&star1, &star2, &star3, &star_e].into_iter().flatten() {
            evaluator.push_binding((*var).clone(), Value::Nil);
        }
        Self {
            eval: evaluator,
            reader,
            writer,
            prompt: None,
            star1,
            star2,
            star3,
            star_e,
        }
    }

    /// Set the prompt template (`None` disables prompts). The
    /// template renders once per form with `{ns}` replaced by the
    /// current namespace name (`"{ns}=> "` mirrors `clojure.main`).
    /// Templates without `{ns}` print literally.
    pub fn set_prompt(&mut self, prompt: Option<String>) {
        self.prompt = prompt;
    }

    /// The session evaluator (for driver assertions on `*1`/`*e`).
    #[must_use]
    pub fn evaluator(&self) -> &Evaluator {
        &self.eval
    }

    /// The session evaluator, mutably (for driver probes).
    pub fn evaluator_mut(&mut self) -> &mut Evaluator {
        &mut self.eval
    }

    /// Unwrap the session into its evaluator, reader, and writer.
    #[must_use]
    pub fn into_parts(self) -> (Evaluator, R, W) {
        (self.eval, self.reader, self.writer)
    }

    /// Render the prompt template against the current namespace
    /// (`None` when prompts are off). The tty driver re-renders
    /// this before every read, so `in-ns` re-prompts at once.
    #[must_use]
    pub fn render_prompt(&self) -> Option<String> {
        self.prompt
            .as_ref()
            .map(|template| template.replace("{ns}", &self.eval.current_ns_name()))
    }

    /// Submit one complete input unit (the tty driver's accumulation
    /// already accepted it): eval, print, update `*1`/`*e`. Returns
    /// the process status (nonzero only on IO failure). A residual
    /// incomplete verdict — unreachable when the submitter gated on
    /// [`read_is_complete`] — reports its diagnostic like EOF does.
    pub fn submit_complete(&mut self, buf: &str) -> i32 {
        match self.step(buf) {
            Step::Done(status) => status,
            Step::More(err) => self.fail(&err),
        }
    }

    /// Run the loop to EOF: 0 on clean EOF, 1 on any IO failure.
    /// Read/eval/print errors print their diagnostic, bind `*e`,
    /// and continue — the session never dies on user input.
    pub fn run(&mut self) -> i32 {
        loop {
            let rendered = self
                .prompt
                .as_ref()
                .map(|template| template.replace("{ns}", &self.eval.current_ns_name()));
            if let Some(text) = rendered {
                // (A bound status, not a let-chain: MSRV 1.85.)
                let status = self.write_str(&text);
                if status != 0 {
                    return 1;
                }
            }
            let mut buf = String::new();
            loop {
                let mut line = String::new();
                match self.reader.read_line(&mut line) {
                    Err(_) => return 1,
                    Ok(0) => {
                        if buf.trim().is_empty() {
                            return 0;
                        }
                        // EOF with a pending partial: submit it like the
                        // tty driver does (the buffer stepped `More` on
                        // the previous iteration and nothing ran since,
                        // so the re-step reports the same diagnostic).
                        return self.submit_complete(&buf);
                    }
                    Ok(_) => buf.push_str(&line),
                }
                match self.step(&buf) {
                    Step::More(_) => {}
                    Step::Done(0) => break,
                    Step::Done(status) => return status,
                }
            }
        }
    }

    /// Evaluate one accumulated buffer per-form: read every form,
    /// then eval + print + shift each in turn, stopping at the
    /// first error — or the incomplete verdict when the reader
    /// wants more lines.
    fn step(&mut self, buf: &str) -> Step {
        let forms = match self.eval.read_forms(buf) {
            Ok(forms) => forms,
            Err(err) => {
                if is_incomplete_error(&err) {
                    return Step::More(err);
                }
                return Step::Done(self.fail(&err));
            }
        };
        for form in &forms {
            match self.eval.eval_top_form(form) {
                Ok(value) => {
                    let status = self.emit(value);
                    if status != 0 {
                        return Step::Done(status);
                    }
                }
                Err(err) => return Step::Done(self.fail(&err)),
            }
        }
        Step::Done(0)
    }

    /// Emit one successful value: shift the window, then print like
    /// `prn` (JVM order — the window moves even when printing then
    /// fails; print failures bind `*e` per P2 and the shifts stand).
    /// Returns the `run` status (nonzero only on IO failure).
    fn emit(&mut self, value: Value) -> i32 {
        self.shift(value.clone());
        let text = match self.print_value(&value) {
            Ok(text) => text,
            Err(boxed) => {
                let (bound, diagnostic) = *boxed;
                self.bind_e(bound);
                return self.write_line(&diagnostic);
            }
        };
        self.write_line(&text)
    }

    /// Report one read/compile/eval failure: bind `*e`, print the
    /// diagnostic, continue. Returns the `run` status (nonzero only
    /// on IO failure).
    fn fail(&mut self, err: &SourceError) -> i32 {
        let (bound, diagnostic) = self.error_parts(err);
        self.bind_e(bound);
        self.write_line(&diagnostic)
    }

    /// The `*e` value plus the printed diagnostic for a failure:
    /// uncaught throwables bind as-is; read errors surface as
    /// `RuntimeException`s carrying the reader message (mirroring
    /// `read-string`); compile errors as `CompilerException`s over
    /// the two-line chain; internals as `Exception`s.
    fn error_parts(&mut self, err: &SourceError) -> (Value, String) {
        match err {
            SourceError::Uncaught(t) => {
                let message = t.message.as_deref().unwrap_or("");
                let diagnostic = Self::class_diagnostic(t.class, message);
                (Value::Throwable(t.clone()), diagnostic)
            }
            SourceError::Reader(e) => {
                let bound = self.throwable(ThrowClass::RuntimeException, e.message());
                (Value::Throwable(bound), e.to_string())
            }
            SourceError::Compile(e) => {
                let text = e.to_string();
                let bound = self.throwable(ThrowClass::CompilerException, text.clone());
                (Value::Throwable(bound), text)
            }
            SourceError::Internal(message) => {
                let diagnostic = format!("internal error: {message}");
                let bound = self.throwable(ThrowClass::Exception, diagnostic.clone());
                (Value::Throwable(bound), diagnostic)
            }
        }
    }

    /// Render one value exactly like `prn`: bound `*print-*` opts,
    /// deep-realized. Failures (a bad limit binding, a throwing
    /// lazy seq) come back as their `*e` value plus diagnostic.
    fn print_value(&mut self, value: &Value) -> Result<String, Box<(Value, String)>> {
        let opts = match clj_eval::core::print_opts(&self.eval, None) {
            Ok(opts) => opts,
            Err(err) => {
                let diagnostic = Self::class_diagnostic(err.class, &err.message);
                let bound = self.native_to_value(&err);
                return Err(Box::new((bound, diagnostic)));
            }
        };
        let realized = match self.eval.realize_deep(value) {
            Ok(realized) => realized,
            Err(signal) => return Err(Box::new(self.print_failure(signal))),
        };
        Ok(pr_to_string(&realized, &opts))
    }

    /// A print-time forcing failure as its `*e` value plus
    /// diagnostic (a `recur` cannot escape forcing — every loop/fn
    /// frame catches its own — so the `Recur` arm is defense in
    /// depth).
    fn print_failure(&mut self, signal: Signal) -> (Value, String) {
        match signal {
            Signal::Throw(t) => {
                let message = t.message.as_deref().unwrap_or("");
                let diagnostic = Self::class_diagnostic(t.class, message);
                (Value::Throwable(t), diagnostic)
            }
            Signal::Recur(_) => {
                let diagnostic = "internal error: recur escaped its loop or fn".to_owned();
                let bound = self.throwable(ThrowClass::Exception, diagnostic.clone());
                (Value::Throwable(bound), diagnostic)
            }
        }
    }

    /// Shift the `*1`/`*2`/`*3` window for one successful value
    /// (P1): every success shifts, including nil. Reads the CURRENT
    /// bindings, so a user `set!` participates like on the JVM.
    /// Missing handles skip their slots.
    fn shift(&mut self, value: Value) {
        let one = self
            .star1
            .clone()
            .map(|var| self.eval.deref_var(var.id, &var.sym));
        let two = self
            .star2
            .clone()
            .map(|var| self.eval.deref_var(var.id, &var.sym));
        if let (Some(three), Some(previous)) = (self.star3.clone(), two) {
            self.eval.set_binding_top(three.id, previous);
        }
        if let (Some(two), Some(previous)) = (self.star2.clone(), one) {
            self.eval.set_binding_top(two.id, previous);
        }
        if let Some(one) = self.star1.clone() {
            self.eval.set_binding_top(one.id, value);
        }
    }

    /// Bind `*e` to a failure value (P2); a missing handle skips.
    fn bind_e(&mut self, value: Value) {
        if let Some(handle) = self.star_e.clone() {
            self.eval.set_binding_top(handle.id, value);
        }
    }

    /// Stamp a driver-synthesized throwable (mirrors
    /// `Evaluator::throw`'s empty-message rule).
    fn throwable(&mut self, class: ThrowClass, message: String) -> Throwable {
        Throwable {
            id: self.eval.next_id(),
            class,
            message: if message.is_empty() {
                None
            } else {
                Some(Arc::from(message.as_str()))
            },
            data: None,
            cause: None,
        }
    }

    /// Stamp a [`NativeError`] (a bad `*print-*` binding) into a
    /// throwable value, preserving class/message/data/cause.
    fn native_to_value(&mut self, err: &NativeError) -> Value {
        let message = if err.message.is_empty() {
            None
        } else {
            Some(Arc::from(err.message.as_str()))
        };
        Value::Throwable(Throwable {
            id: self.eval.next_id(),
            class: err.class,
            message,
            data: err.data.clone().map(Box::new),
            cause: err.cause.clone().map(Box::new),
        })
    }

    /// The one-line `SimpleClass: message` diagnostic (bare class
    /// when the message is empty).
    fn class_diagnostic(class: ThrowClass, message: &str) -> String {
        let simple = class.jvm_name().rsplit('.').next().unwrap_or("Throwable");
        if message.is_empty() {
            simple.to_owned()
        } else {
            format!("{simple}: {message}")
        }
    }

    /// Write `text` verbatim and flush (prompts); 0 ok, 1 on IO
    /// failure.
    fn write_str(&mut self, text: &str) -> i32 {
        if self.writer.write_all(text.as_bytes()).is_err() {
            return 1;
        }
        self.flush()
    }

    /// Write one `text` + `\n` line and flush; 0 ok, 1 on IO
    /// failure.
    fn write_line(&mut self, text: &str) -> i32 {
        if writeln!(self.writer, "{text}").is_err() {
            return 1;
        }
        self.flush()
    }

    /// Flush the writer; 0 ok, 1 on IO failure.
    fn flush(&mut self) -> i32 {
        if self.writer.flush().is_err() { 1 } else { 0 }
    }
}

#[cfg(test)]
mod tests {
    use super::{
        NativeError, Session, Signal, SourceError, ThrowClass, Throwable, Value,
        is_incomplete_message,
    };

    #[test]
    fn each_incomplete_variant_continues() {
        for message in [
            "EOF while reading, starting at line 1",
            "EOF while reading, starting at line 12",
            "EOF while reading string",
            "EOF while reading character",
            "EOF while reading regex",
        ] {
            assert!(is_incomplete_message(message), "{message:?} continues");
        }
    }

    #[test]
    fn top_level_eof_is_done_not_incomplete() {
        assert!(!is_incomplete_message("EOF while reading"));
    }

    #[test]
    fn hard_errors_are_not_incomplete() {
        for message in [
            "Unmatched delimiter: )",
            "Invalid token: foo",
            "Invalid number: 1a",
            "Map literal must contain an even number of forms",
            "Nested #()s are not allowed",
            "No reader function for tag foo",
            "Conditional read not allowed",
            "",
            "eof while reading string",
        ] {
            assert!(!is_incomplete_message(message), "{message:?} is hard");
        }
    }

    #[test]
    fn internal_error_parts_report_plain_exception() {
        // `Internal` is unreachable via eval, so the error is
        // synthesized.
        type T = Session<std::io::Empty, Vec<u8>>;
        let mut s = T::new(clj_eval::boot(), std::io::empty(), Vec::new());
        let (bound, diagnostic) = s.error_parts(&SourceError::Internal("boom".to_owned()));
        assert_eq!(diagnostic, "internal error: boom");
        assert!(matches!(bound, Value::Throwable(t)
                if t.class == ThrowClass::Exception
                    && t.message.as_deref() == Some("internal error: boom")));
    }

    #[test]
    fn empty_messages_stamp_message_less_throwables() {
        type T = Session<std::io::Empty, Vec<u8>>;
        let mut s = T::new(clj_eval::boot(), std::io::empty(), Vec::new());
        let t = s.throwable(ThrowClass::Exception, String::new());
        assert!(t.message.is_none());
        let bound = s.native_to_value(&NativeError::new(ThrowClass::Exception, String::new()));
        assert!(matches!(bound, Value::Throwable(t) if t.message.is_none()));
    }

    #[test]
    fn print_failure_spells_both_signals() {
        type T = Session<std::io::Empty, Vec<u8>>;
        let mut s = T::new(clj_eval::boot(), std::io::empty(), Vec::new());
        let thrown = Signal::Throw(Throwable {
            id: 1,
            class: ThrowClass::ArithmeticException,
            message: Some(std::sync::Arc::from("Divide by zero")),
            data: None,
            cause: None,
        });
        let (bound, diagnostic) = s.print_failure(thrown);
        assert_eq!(diagnostic, "ArithmeticException: Divide by zero");
        assert!(matches!(bound, Value::Throwable(_)));
        // The forcing-impossible case: pins the helper's total
        // contract directly.
        let (bound, diagnostic) = s.print_failure(Signal::Recur(vec![]));
        assert_eq!(diagnostic, "internal error: recur escaped its loop or fn");
        assert!(matches!(bound, Value::Throwable(t)
                if t.class == ThrowClass::Exception
                    && t.message.as_deref() == Some("internal error: recur escaped its loop or fn")));
    }

    #[test]
    fn class_diagnostic_is_bare_without_message() {
        type T = Session<std::io::Empty, Vec<u8>>;
        assert_eq!(
            T::class_diagnostic(ThrowClass::ArithmeticException, ""),
            "ArithmeticException"
        );
        assert_eq!(
            T::class_diagnostic(ThrowClass::ArithmeticException, "Divide by zero"),
            "ArithmeticException: Divide by zero"
        );
    }
}
