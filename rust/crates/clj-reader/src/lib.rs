// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The Clojure reader: text to [`Value`](clj_value::Value), replicating
//! JVM `LispReader` behavior per `docs/t1174/reader-contract.md`
//! (positions, tokens, numbers, escapes, wrappers, meta, fn,
//! syntax-quote, conditionals, tags, ns-maps, collections, `##`).
//!
//! [`read_str`] reads one form; [`read_all`] reads every form.
//! Failures are [`ReaderError`]s carrying their span, message, and
//! JVM class. Gensym ids come from a process-global counter starting
//! at 1 (`RT.nextID`).

pub mod coll;
pub mod dispatch;
pub mod error;
pub mod metaform;
pub mod number;
pub mod opts;
pub mod stream;
pub mod symbol;
pub mod text;

pub use error::{ReaderError, Span};
pub use opts::{ReadCond, ReaderOpts};

use std::collections::{BTreeMap, HashMap, VecDeque};
use std::sync::atomic::{AtomicU64, Ordering};

use clj_value::{List, Symbol, Value};

use error::ReaderError as E;
use number::{NumErr, NumLit};
use stream::CharStream;
use symbol::{interpret_token, is_digit_start, is_macro, is_whitespace, read_token};

/// Process-global gensym id counter (`RT.nextID` starts at 1).
static NEXT_ID: AtomicU64 = AtomicU64::new(1);

/// Next process-global gensym id (first call returns 1).
pub(crate) fn next_id() -> u64 {
    NEXT_ID.fetch_add(1, Ordering::Relaxed)
}

/// One form read: a value, EOF, or the awaited delimiter.
#[derive(Debug)]
pub(crate) enum Form {
    Value(Value),
    Eof,
    Finished,
}

/// A reader over one input: the §1 tracker, the shared pending queue
/// (reader-conditional splices), normalized options, and the fn-arg,
/// gensym, suppression, and read-cond environments.
///
/// `Clone` snapshots the read position (issue #36: stdin `read`
/// retries a truncated form over the restored head plus the next
/// line).
#[derive(Clone)]
pub struct Reader {
    pub(crate) stream: CharStream,
    pub(crate) pending: VecDeque<Value>,
    pub(crate) opts: ReaderOpts,
    pub(crate) arg_env: Option<BTreeMap<i32, Symbol>>,
    pub(crate) gensym_env: Option<HashMap<Symbol, Symbol>>,
    pub(crate) suppress: bool,
    pub(crate) in_cond: bool,
}

// `Reader` lives behind a `Mutex` in the `clj-eval` streams.
const _: fn() = || {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Reader>();
};

impl Reader {
    /// Start reading `input` (fresh line 1, column 1; options
    /// normalized with `:clj` installed).
    #[must_use]
    pub fn new(input: &str, opts: &ReaderOpts) -> Self {
        Self {
            stream: CharStream::new(input),
            pending: VecDeque::new(),
            opts: opts.clone().normalized(),
            arg_env: None,
            gensym_env: None,
            suppress: false,
            in_cond: false,
        }
    }

    /// Replace the options (cloned and normalized like [`new`](Self::new)).
    pub fn set_opts(&mut self, opts: &ReaderOpts) {
        self.opts = opts.clone().normalized();
    }

    /// Append `chunk` to the input (stdin growth; same
    /// normalization as construction, so a split `\r\n`
    /// coalesces).
    pub fn extend(&mut self, chunk: &str) {
        self.stream.extend(chunk);
    }

    /// Read one `\n`-split line (`\n` consumed and stripped);
    /// `None` at EOF, the tail when EOF lands mid-line.
    pub fn read_line(&mut self) -> Option<String> {
        self.stream.read_line()
    }

    /// Current 1-based line (live tracker position).
    #[must_use]
    pub fn line(&self) -> u32 {
        self.stream.line()
    }

    /// Current column (live tracker position).
    #[must_use]
    pub fn col(&self) -> u32 {
        self.stream.col()
    }

    /// Read one top-level form: `eof_is_error` decides whether EOF
    /// errors (single `read`) or ends the stream (`None`, for `load`
    /// loops).
    pub fn read_form(&mut self, eof_is_error: bool) -> Result<Option<Value>, ReaderError> {
        // `Finished` needs an awaited delimiter (`return_on` is
        // `None` here), so it shares the `Eof` arm (100% line
        // gate: a lone arm would be uncoverable).
        match self.read_inner(eof_is_error, None, true)? {
            Form::Value(v) => Ok(Some(v)),
            Form::Eof | Form::Finished => Ok(None),
        }
    }

    /// The `read()` core loop (§3): shared pending queue first, then
    /// whitespace, EOF, the awaited delimiter, the number path,
    /// macros (sentinel ⇒ recur), `+`/`-`-then-digit, else a token.
    /// `top` marks the outermost frame (conditional splicing rejects
    /// only there); every nested read passes `false`.
    pub(crate) fn read_inner(
        &mut self,
        eof_is_error: bool,
        return_on: Option<char>,
        top: bool,
    ) -> Result<Form, ReaderError> {
        // A fresh top-level `read()` starts with a null (= empty)
        // pending list on the JVM; nested frames share the parent's.
        // Without this, splice leftovers (e.g. `'#?@(:clj (1 2))`,
        // where the quote consumes only the first item) leak into the
        // NEXT top-level form instead of being dropped (probe16).
        if top {
            self.pending.clear();
        }
        loop {
            if let Some(v) = self.pending.pop_front() {
                return Ok(Form::Value(v));
            }
            let mut ch = self.stream.read();
            while ch.is_some_and(is_whitespace) {
                ch = self.stream.read();
            }
            let Some(ch) = ch else {
                if eof_is_error {
                    return Err(E::Eof { span: self.span() });
                }
                return Ok(Form::Eof);
            };
            if return_on == Some(ch) {
                return Ok(Form::Finished);
            }
            if let Some(form) = self.dispatch_char(ch, top)? {
                return Ok(form);
            }
        }
    }

    /// Dispatch one non-whitespace char: the number path, macros
    /// (sentinel ⇒ `None`, the loop recurs), `+`/`-`-then-digit,
    /// else a token.
    fn dispatch_char(&mut self, ch: char, top: bool) -> Result<Option<Form>, ReaderError> {
        if is_digit_start(ch) {
            return Ok(Some(Form::Value(self.read_number_value(ch)?)));
        }
        if is_macro(ch) {
            return match self.invoke_macro(ch, top)? {
                Some(v) => Ok(Some(Form::Value(v))),
                None => Ok(None),
            };
        }
        if ch == '+' || ch == '-' {
            let ch2 = self.stream.read();
            if ch2.is_some_and(is_digit_start) {
                self.stream.unread(ch2.unwrap());
                return Ok(Some(Form::Value(self.read_number_value(ch)?)));
            }
            if let Some(c2) = ch2 {
                self.stream.unread(c2);
            }
        }
        let token = read_token(&mut self.stream, ch);
        match interpret_token(&token, &self.opts) {
            Some(v) => Ok(Some(Form::Value(v))),
            None => Err(E::InvalidToken {
                span: self.span(),
                token,
            }),
        }
    }

    /// One nested form, EOF errors (what every wrapping reader uses).
    pub(crate) fn read_nested_value(&mut self) -> Result<Value, ReaderError> {
        unwrap_single(self.read_inner(true, None, false)?)
    }

    /// Dispatch one macro char to its reader.
    fn invoke_macro(&mut self, ch: char, top: bool) -> Result<Option<Value>, ReaderError> {
        match ch {
            '"' => self.string_reader(),
            ';' => self.comment_reader(),
            '\'' => self.wrapping_reader(Symbol::new(None, "quote")),
            '@' => self.wrapping_reader(Symbol::new(Some("clojure.core"), "deref")),
            '^' => self.meta_reader(),
            '`' => self.syntax_quote_reader(),
            '~' => self.unquote_reader(),
            '(' => self.list_reader(),
            ')' | ']' | '}' => Err(E::UnmatchedDelim {
                span: self.span(),
                c: ch,
            }),
            '[' => self.vector_reader(),
            '{' => self.map_reader(),
            '\\' => self.char_reader(),
            '%' => self.arg_reader(),
            '#' => self.dispatch_reader(top),
            _ => unreachable!("macro table covers every macro char"),
        }
    }

    /// `WrappingReader`: one nested form wrapped in `(sym X)`.
    fn wrapping_reader(&mut self, sym: Symbol) -> Result<Option<Value>, ReaderError> {
        let form = self.read_nested_value()?;
        Ok(Some(Value::List(List::from_vec(vec![
            Value::Symbol(sym),
            form,
        ]))))
    }

    /// `UnquoteReader`: `~@X` splicing, else `~X` (bare `~` at EOF is
    /// `EofChar`; `~@` at EOF reads a form, so `Eof`).
    fn unquote_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let Some(ch) = self.stream.read() else {
            return Err(E::EofChar { span: self.span() });
        };
        if ch == '@' {
            let form = self.read_nested_value()?;
            Ok(Some(Value::List(List::from_vec(vec![
                Value::Symbol(Symbol::new(Some("clojure.core"), "unquote-splicing")),
                form,
            ]))))
        } else {
            self.stream.unread(ch);
            let form = self.read_nested_value()?;
            Ok(Some(Value::List(List::from_vec(vec![
                Value::Symbol(Symbol::new(Some("clojure.core"), "unquote")),
                form,
            ]))))
        }
    }

    /// `readNumber` plus `matchNumber` (tower literals read
    /// since S-rng-tower; only a zero denominator still raises).
    fn read_number_value(&mut self, initch: char) -> Result<Value, ReaderError> {
        let token = number::read_number_token(&mut self.stream, initch);
        let span = self.span();
        match number::match_number(&token) {
            Ok(NumLit::Int(i)) => Ok(Value::Int(i)),
            Ok(NumLit::Double(d)) => Ok(Value::Double(d)),
            Ok(NumLit::BigInt) => Ok(number::bigint_token_value(&token)),
            Ok(NumLit::BigDecimal) => Ok(number::bigdec_token_value(&token)),
            Ok(NumLit::Ratio) => match number::ratio_token_value(&token) {
                number::RatioToken::Value(v) => Ok(v),
                number::RatioToken::ZeroDenominator => Err(E::RatioZeroDiv { span }),
            },
            Err(NumErr::InvalidNumber) => Err(E::InvalidNumber { span, token }),
            Err(NumErr::RadixRange) => Err(E::RadixRange { span }),
            Err(NumErr::RadixDigit { digits, radix }) => Err(E::RadixDigit {
                span,
                digits,
                radix,
            }),
        }
    }

    /// Current span (live tracker position).
    pub(crate) fn span(&self) -> Span {
        Span::new(self.stream.line(), self.stream.col())
    }

    /// The delimited-body first line (`None` without line numbers).
    pub(crate) fn firstline(&self) -> Option<u32> {
        self.opts.line_numbers.then(|| self.stream.line())
    }

    /// The `(line, col_after_open - 1)` capture for list/meta
    /// `:line`/`:column` (`None` without line numbers).
    pub(crate) fn meta_pos(&self) -> Option<(u32, u32)> {
        self.opts
            .line_numbers
            .then(|| (self.stream.line(), self.stream.col() - 1))
    }

    /// The delimited-EOF error for a body (bare without line numbers).
    pub(crate) fn eof_delimited(&self, firstline: Option<u32>) -> ReaderError {
        match firstline {
            Some(line) => E::EofDelimited {
                span: self.span(),
                line,
            },
            None => E::Eof { span: self.span() },
        }
    }
}

/// Read one form from `s` (fresh positions; EOF errors).
pub fn read_str(s: &str, opts: &ReaderOpts) -> Result<Value, ReaderError> {
    let mut reader = Reader::new(s, opts);
    unwrap_single(reader.read_inner(true, None, true)?)
}

/// Fold a single-read outcome: values flow. Callers pass
/// `eof_is_error=true, return_on=None`, so `Eof`/`Finished`
/// cannot occur (`read_inner` returns `Err` on EOF and
/// `Finished` only for an awaited delimiter) — the dead arms
/// live here (not at each call site) so direct unit tests pin
/// them (100% line gate).
fn unwrap_single(form: Form) -> Result<Value, ReaderError> {
    match form {
        Form::Value(v) => Ok(v),
        Form::Eof | Form::Finished => unreachable!("single reads yield values"),
    }
}

/// Read every form from `s` (fresh positions; errors abort the read).
pub fn read_all(s: &str, opts: &ReaderOpts) -> Result<Vec<Value>, ReaderError> {
    let mut reader = Reader::new(s, opts);
    let mut forms = Vec::new();
    while let Some(v) = reader.read_form(false)? {
        forms.push(v);
    }
    Ok(forms)
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::{Form, unwrap_single};
    use crate::{ReadCond, Reader, ReaderError, ReaderOpts, read_all, read_str};
    use clj_value::{Keyword, PrintOpts, Value, pr_to_string};

    fn pr(v: &clj_value::Value) -> String {
        pr_to_string(v, &PrintOpts::pr())
    }

    #[test]
    fn splice_leftovers_do_not_leak_across_top_level_forms() {
        // probe16: the quote consumes only the first spliced item; the
        // leftover is dropped, not returned as the next form.
        let opts = ReaderOpts::default();
        let forms = read_all("'#?@(:clj (1 2)) X", &opts).unwrap();
        assert_eq!(forms.len(), 2);
        assert_eq!(pr(&forms[0]), "(quote 1)");
        assert_eq!(pr(&forms[1]), "X");
        let forms = read_all("'#?@(:clj (1 2))", &opts).unwrap();
        assert_eq!(forms.len(), 1);
        assert_eq!(pr(&forms[0]), "(quote 1)");
        let forms = read_all("#(1 #?@(:clj (2 3)))", &opts).unwrap();
        assert_eq!(forms.len(), 1);
        assert!(pr(&forms[0]).starts_with("(fn* [] (1 2 3))"));
    }

    #[test]
    fn cond_eof_reports_body_line_with_line_numbers() {
        // `eof_delimited`, tracked arm.
        let err = read_str("#?(", &ReaderOpts::default()).unwrap_err();
        assert!(matches!(err, ReaderError::EofDelimited { .. }));
        assert_eq!(err.message(), "EOF while reading, starting at line 1");
    }

    #[test]
    fn cond_eof_is_bare_without_line_numbers() {
        // `eof_delimited`, untracked arm.
        let opts = ReaderOpts {
            line_numbers: false,
            ..ReaderOpts::default()
        };
        let err = read_str("#?(", &opts).unwrap_err();
        assert!(matches!(err, ReaderError::Eof { .. }), "got {err:?}");
        assert_eq!(err.message(), "EOF while reading");
    }

    #[test]
    fn unwrap_single_flows_values() {
        assert_eq!(
            unwrap_single(Form::Value(Value::Int(1))).unwrap(),
            Value::Int(1)
        );
    }

    #[test]
    #[should_panic(expected = "single reads yield values")]
    fn unwrap_single_rejects_eof() {
        let _ = unwrap_single(Form::Eof);
    }

    #[test]
    #[should_panic(expected = "single reads yield values")]
    fn unwrap_single_rejects_finished() {
        let _ = unwrap_single(Form::Finished);
    }

    #[test]
    #[should_panic(expected = "macro table covers every macro char")]
    fn invoke_macro_rejects_non_macro_chars() {
        // All callers guard with `is_macro`; direct invocation
        // pins the defensive fallback.
        let opts = ReaderOpts::default();
        let mut reader = Reader::new("x", &opts);
        let _ = reader.invoke_macro('x', false);
    }

    #[test]
    fn set_opts_takes_effect_on_next_read() {
        let mut reader = Reader::new("#?(:clj 1 :default 2)", &ReaderOpts::default());
        reader.set_opts(&ReaderOpts {
            read_cond: ReadCond::Deny,
            ..ReaderOpts::default()
        });
        let err = reader.read_form(true).unwrap_err();
        assert!(
            matches!(err, ReaderError::CondNotAllowed { .. }),
            "got {err:?}"
        );
        assert_eq!(err.message(), "Conditional read not allowed");
    }

    #[test]
    fn set_opts_normalizes_like_new() {
        let mut reader = Reader::new("1", &ReaderOpts::default());
        reader.set_opts(&ReaderOpts {
            features: HashSet::new(),
            ..ReaderOpts::default()
        });
        assert!(reader.opts.features.contains(&Keyword::new(None, "clj")));
    }

    #[test]
    fn stream_passthroughs_share_one_position() {
        // `extend`/`read_line`/`line`/`col` ride the same tracker
        // `read_form` uses (the `clj-eval` streams interleave all
        // three, issue #36).
        let mut reader = Reader::new("a\n", &ReaderOpts::default());
        assert_eq!((reader.line(), reader.col()), (1, 1));
        assert_eq!(reader.read_line(), Some("a".to_owned()));
        assert_eq!((reader.line(), reader.col()), (2, 1));
        assert_eq!(reader.read_line(), None);
        reader.extend("(b)");
        let form = reader.read_form(true).unwrap().expect("extended form");
        assert_eq!(pr(&form), "(b)");
        assert_eq!((reader.line(), reader.col()), (2, 4));
    }

    #[test]
    fn eof_delimited_span_for_truncated_list() {
        // Oracle `r-exdata`: the message names first-line 1 while the
        // span (ex-data source) sits at the bumped EOF position 2:1.
        let err = read_str("(a", &ReaderOpts::default()).unwrap_err();
        assert_eq!(err.message(), "EOF while reading, starting at line 1");
        match err {
            ReaderError::EofDelimited { span, line } => {
                assert_eq!(line, 1);
                assert_eq!((span.line, span.col), (2, 1));
            }
            other => panic!("got {other:?}"),
        }
    }
}
