// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Dispatch macros (`reader-contract.md` §8–§9, §14–§18, §20).
//!
//! - `;`/`#!`: to end of line, sentinel. `#_`: discard one form.
//! - `#tag`: the ctor path (unread, read tag — non-symbol rejects —
//!   read form, then preserve/suppress ⇒ `TaggedLiteral`, dotted
//!   name ⇒ record deferral, `inst`/`uuid` ⇒ data-reader deferral,
//!   else unknown tag).
//! - `#=`: immediate deferral without consuming the form. `#<`:
//!   unreadable.
//! - `#:`: namespaced maps (auto/current-ns/alias resolution, key
//!   rewrite, evenness + duplicate rules).
//! - `##`: symbolic values (one recursive form, must be an
//!   `Inf`/`-Inf`/`NaN` symbol).
//! - `#?`/`#?@`: reader conditionals — allow-list checked before the
//!   body is touched; the `readCondDelimited` loop shape replicated
//!   exactly (match arm, then the ALWAYS-run suppressed discard;
//!   splices prepend list/vector items to the shared pending queue;
//!   top-level splicing rejected unless nothing matched).

use clj_value::{Keyword, List, Symbol, Value, java_debug};

use crate::Reader;
use crate::error::ReaderError;
use crate::opts::ReadCond;
use crate::symbol::is_whitespace;

impl Reader {
    /// `DispatchReader`: `#` plus one char (EOF ⇒ `EofChar`); unknown
    /// chars (including non-ASCII) unread into the ctor path.
    pub(crate) fn dispatch_reader(&mut self, top: bool) -> Result<Option<Value>, ReaderError> {
        let Some(ch) = self.stream.read() else {
            return Err(ReaderError::EofChar { span: self.span() });
        };
        match ch {
            '^' => self.meta_reader(),
            '#' => self.symbolic_reader(),
            '\'' => self.var_reader(),
            '"' => self.regex_reader(),
            '(' => self.fn_reader(),
            '{' => self.set_reader(),
            '=' => Err(ReaderError::ReadEval { span: self.span() }),
            '!' => self.comment_reader(),
            '<' => Err(ReaderError::Unreadable { span: self.span() }),
            '_' => self.discard_reader(),
            '?' => self.conditional_reader(top),
            ':' => self.nsmap_reader(),
            _ => {
                self.stream.unread(ch);
                self.ctor_reader()
            }
        }
    }

    /// `CommentReader` (shared by `;` and `#!`): skip to `\n` or EOF
    /// (the stream never yields `\r`), return the sentinel.
    pub(crate) fn comment_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        while let Some(c) = self.stream.read() {
            if c == '\n' {
                break;
            }
        }
        Ok(None)
    }

    /// `DiscardReader`: read one form, drop it, return the sentinel.
    pub(crate) fn discard_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        self.read_nested_value()?;
        Ok(None)
    }

    /// `VarReader`: `#'X` ⇒ `(var X)`.
    fn var_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let form = self.read_nested_value()?;
        Ok(Some(Value::List(List::from_vec(vec![
            Value::Symbol(Symbol::new(None, "var")),
            form,
        ]))))
    }

    /// `CtorReader`: `#tag form` (see module docs). The tag read is
    /// `eofIsError`; a non-symbol tag rejects before the form is read;
    /// the form reads before any tag lookup (EOF there wins).
    fn ctor_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let tag = self.read_nested_value()?;
        let Value::Symbol(sym) = tag else {
            return Err(ReaderError::TagNotSymbol { span: self.span() });
        };
        let form = self.read_nested_value()?;
        // Suppressed reads (the cond discard loop) and preserved
        // conds produce `TaggedLiteral` without invoking anything.
        if self.suppress || (self.in_cond && self.opts.read_cond == ReadCond::Preserve) {
            return Ok(Some(Value::TaggedLiteral {
                tag: sym,
                form: Box::new(form),
            }));
        }
        // The dotted check is on the tag NAME, not the namespace.
        if sym.name().contains('.') {
            return Err(ReaderError::Record {
                span: self.span(),
                tag: sym.to_string(),
            });
        }
        if sym.namespace().is_none() && (sym.name() == "inst" || sym.name() == "uuid") {
            return Err(ReaderError::DataReader {
                span: self.span(),
                tag: sym.to_string(),
            });
        }
        Err(ReaderError::UnknownTag {
            span: self.span(),
            tag: sym.to_string(),
        })
    }

    /// `NamespaceMapReader` (see module docs).
    fn nsmap_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let auto = match self.stream.read() {
            Some(':') => true,
            Some(c) => {
                self.stream.unread(c);
                false
            }
            // EOF: `unread(-1)` is skipped on the JVM; fall through
            // so the symbol read below reports `Eof`.
            None => false,
        };
        let (sym, next) = self.nsmap_head(auto)?;
        // EOF here (including right after the symbol) means "must
        // specify a map", not `Eof`.
        if next != Some('{') {
            return Err(ReaderError::NsMapMap { span: self.span() });
        }
        let ns = self.resolve_nsmap_ns(auto, sym)?;
        self.nsmap_assemble(&ns)
    }

    /// Skip whitespace chars, returning the first non-ws char (or EOF).
    fn skip_ws(&mut self, mut next: Option<char>) -> Option<char> {
        while next.is_some_and(is_whitespace) {
            next = self.stream.read();
        }
        next
    }

    /// Scan the ns-map head: optional alias symbol plus the char that
    /// follows it (whitespace skipped). A bad namespace errors here.
    fn nsmap_head(&mut self, auto: bool) -> Result<(Option<Value>, Option<char>), ReaderError> {
        let mut sym: Option<Value> = None;
        let mut next = self.stream.read();
        if next.is_some_and(is_whitespace) {
            if auto {
                // `#:: { }`: skip to `{`, else no namespace.
                next = self.skip_ws(next);
                if next != Some('{') {
                    if let Some(c) = next {
                        self.stream.unread(c);
                    }
                    return Err(ReaderError::NsMapNs { span: self.span() });
                }
            } else {
                if let Some(c) = next {
                    self.stream.unread(c);
                }
                return Err(ReaderError::NsMapNs { span: self.span() });
            }
        } else if next != Some('{') {
            if let Some(c) = next {
                self.stream.unread(c);
            }
            sym = Some(self.read_nested_value()?);
            let head = self.stream.read();
            next = self.skip_ws(head);
        }
        Ok((sym, next))
    }

    /// Assemble the ns-map body: read `{...}`, check evenness, rewrite
    /// keys, build the checked map.
    fn nsmap_assemble(&mut self, ns: &str) -> Result<Option<Value>, ReaderError> {
        let items = self.read_delimited('}')?;
        if items.len() % 2 == 1 {
            return Err(ReaderError::NsMapOdd { span: self.span() });
        }
        let pairs = crate::coll::pair_up(items)
            .into_iter()
            .map(|(k, v)| (nsmap_key(k, ns), v))
            .collect();
        Self::checked_map(pairs, self.span()).map(|m| Some(Value::Map(m)))
    }

    /// Resolve the ns-map namespace (auto/current-ns/alias rules).
    fn resolve_nsmap_ns(&mut self, auto: bool, sym: Option<Value>) -> Result<String, ReaderError> {
        if auto {
            match sym {
                None => Ok(self.opts.current_ns.clone()),
                Some(Value::Symbol(sym)) if sym.namespace().is_none() => {
                    let name = sym.name().to_owned();
                    match self.opts.aliases.get(&name) {
                        Some(target) => Ok(target.clone()),
                        None => Err(ReaderError::NsMapAlias {
                            span: self.span(),
                            sym: sym.to_string(),
                        }),
                    }
                }
                Some(other) => Err(ReaderError::NsMapValidNs {
                    span: self.span(),
                    dbg: java_debug(&other),
                }),
            }
        } else {
            match sym {
                // `#:{}`: null sym ⇒ invalid namespace, rendering `null`.
                Some(Value::Symbol(sym)) if sym.namespace().is_none() => Ok(sym.name().to_owned()),
                Some(other) => Err(ReaderError::NsMapValidNs {
                    span: self.span(),
                    dbg: java_debug(&other),
                }),
                None => Err(ReaderError::NsMapValidNs {
                    span: self.span(),
                    dbg: "null".to_owned(),
                }),
            }
        }
    }

    /// `SymbolicValueReader`: one recursive form; non-symbols and
    /// unknown names reject with `##`-prefixed messages.
    fn symbolic_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let form = self.read_nested_value()?;
        if let Value::Symbol(sym) = &form {
            return match (sym.namespace(), sym.name()) {
                (None, "Inf") => Ok(Some(Value::Double(f64::INFINITY))),
                (None, "-Inf") => Ok(Some(Value::Double(f64::NEG_INFINITY))),
                (None, "NaN") => Ok(Some(Value::Double(f64::NAN))),
                _ => Err(ReaderError::UnknownSymbolic {
                    span: self.span(),
                    sym: sym.to_string(),
                }),
            };
        }
        Err(ReaderError::BadSymbolic {
            span: self.span(),
            dbg: java_debug(&form),
        })
    }

    /// `ConditionalReader`: allow-list first (before touching the
    /// body), optional `@`, whitespace, `(`, then preserve (read the
    /// list raw) or the delimited loop — all under `in_cond`.
    fn conditional_reader(&mut self, top: bool) -> Result<Option<Value>, ReaderError> {
        if !matches!(self.opts.read_cond, ReadCond::Allow | ReadCond::Preserve) {
            return Err(ReaderError::CondNotAllowed { span: self.span() });
        }
        // No EOF check between `@` and the whitespace skip (an EOF
        // there falls through to the check below).
        let mut ch = self.stream.read();
        let mut splicing = false;
        if ch == Some('@') {
            splicing = true;
            ch = self.stream.read();
        }
        ch = self.skip_ws(ch);
        let Some(ch) = ch else {
            return Err(ReaderError::EofChar { span: self.span() });
        };
        if ch != '(' {
            return Err(ReaderError::CondBody { span: self.span() });
        }
        let prev = self.in_cond;
        self.in_cond = true;
        // No `?` before the restore (like `cond_discard`): a failed
        // preserve read must not stick `in_cond` for the next read.
        let result = if self.opts.read_cond == ReadCond::Preserve {
            // The raw list (with line/col metas as usual). The
            // list reader returns a form on every success path,
            // so `expect` (whose line always executes) spells the
            // invariant coverably (100% line gate).
            self.list_reader().map(|form| {
                Some(Value::ReaderConditional {
                    form: Box::new(form.expect("the list reader always returns a form")),
                    splicing,
                })
            })
        } else {
            self.read_cond_delimited(splicing, top)
        };
        self.in_cond = prev;
        result
    }

    /// `readCondDelimited`: the loop shape, not "collect pairs" (see
    /// module docs). `top` is the calling frame's top-level flag.
    fn read_cond_delimited(
        &mut self,
        splicing: bool,
        top: bool,
    ) -> Result<Option<Value>, ReaderError> {
        let firstline = self.firstline();
        let mut result: Option<Value> = None;
        loop {
            if result.is_none() && self.cond_match_arm(&mut result, firstline)? {
                break;
            }
            // The discard runs ALWAYS — even right after a match.
            if self.cond_discard(firstline)? {
                break;
            }
        }
        self.cond_finish(result, splicing, top)
    }

    /// One feature-match attempt of the cond loop: read a feature,
    /// validate it, and read its value on match. True finishes the
    /// loop (`)` where a feature was expected).
    fn cond_match_arm(
        &mut self,
        result: &mut Option<Value>,
        firstline: Option<u32>,
    ) -> Result<bool, ReaderError> {
        match self.read_inner(false, Some(')'), false)? {
            crate::Form::Eof => Err(self.eof_delimited(firstline)),
            crate::Form::Finished => Ok(true),
            crate::Form::Value(f) => {
                if is_reserved_feature(&f) {
                    return Err(ReaderError::ReservedFeature {
                        span: self.span(),
                        kw: java_debug(&f),
                    });
                }
                let Value::Keyword(kw) = &f else {
                    return Err(ReaderError::BadFeature {
                        span: self.span(),
                        dbg: java_debug(&f),
                    });
                };
                if is_default(kw) || self.opts.features.contains(kw) {
                    match self.read_inner(false, Some(')'), false)? {
                        crate::Form::Eof => Err(self.eof_delimited(firstline)),
                        crate::Form::Finished => Err(ReaderError::CondEven {
                            span: self.span(),
                            line: firstline,
                        }),
                        crate::Form::Value(v) => {
                            *result = Some(v);
                            Ok(false)
                        }
                    }
                } else {
                    Ok(false)
                }
            }
        }
    }

    /// The ALWAYS-run suppressed discard of the cond loop. True
    /// finishes the loop.
    fn cond_discard(&mut self, firstline: Option<u32>) -> Result<bool, ReaderError> {
        let prev = self.suppress;
        self.suppress = true;
        let discarded = self.read_inner(false, Some(')'), false);
        self.suppress = prev;
        match discarded? {
            crate::Form::Eof => Err(self.eof_delimited(firstline)),
            crate::Form::Finished => Ok(true),
            crate::Form::Value(_) => Ok(false),
        }
    }

    /// Finish the cond loop: no match ⇒ sentinel; splicing matches
    /// prepend to the pending queue (top-level splices reject).
    fn cond_finish(
        &mut self,
        result: Option<Value>,
        splicing: bool,
        top: bool,
    ) -> Result<Option<Value>, ReaderError> {
        let Some(result) = result else {
            // No match: the reader sentinel (recur).
            return Ok(None);
        };
        if !splicing {
            return Ok(Some(result));
        }
        let items = match &result {
            Value::List(l) => l.iter().cloned().collect::<Vec<_>>(),
            Value::Vector(v) => v.iter().cloned().collect::<Vec<_>>(),
            _ => {
                return Err(ReaderError::SpliceType { span: self.span() });
            }
        };
        if top {
            return Err(ReaderError::SpliceTopLevel { span: self.span() });
        }
        for v in items.into_iter().rev() {
            self.pending.push_front(v);
        }
        Ok(None)
    }
}

/// Rewrite one ns-map key: unqualified ⇒ prefix, `_`-ns ⇒ strip,
/// otherwise unchanged.
fn nsmap_key(key: Value, ns: &str) -> Value {
    match &key {
        Value::Keyword(kw) if kw.namespace().is_none() => {
            Value::Keyword(Keyword::new(Some(ns), kw.name()))
        }
        Value::Keyword(kw) if kw.namespace() == Some("_") => {
            Value::Keyword(Keyword::new(None, kw.name()))
        }
        Value::Symbol(sym) if sym.namespace().is_none() => {
            Value::Symbol(Symbol::new(Some(ns), sym.name()))
        }
        Value::Symbol(sym) if sym.namespace() == Some("_") => {
            Value::Symbol(Symbol::new(None, sym.name()))
        }
        _ => key,
    }
}

fn is_reserved_feature(f: &Value) -> bool {
    if let Value::Keyword(kw) = f {
        kw.namespace().is_none() && (kw.name() == "else" || kw.name() == "none")
    } else {
        false
    }
}

fn is_default(kw: &Keyword) -> bool {
    kw.namespace().is_none() && kw.name() == "default"
}

#[cfg(test)]
mod tests {
    use clj_value::Value;

    use crate::{ReadCond, Reader, ReaderError, ReaderOpts, read_str};

    fn opts() -> ReaderOpts {
        ReaderOpts::default()
    }

    /// Assert a single-entry map holds one `Int(1)` pair keyed by a
    /// symbol (structural match: no `Value` equality needed).
    fn assert_single_sym_pair(v: Value, ns: Option<&str>, name: &str) {
        let Value::Map(m) = v else {
            panic!("expected a map, got {v:?}");
        };
        assert_eq!(m.size(), 1);
        for (k, val) in m.iter() {
            assert_eq!(val, &Value::Int(1));
            assert!(matches!(k, Value::Symbol(s) if s.namespace() == ns && s.name() == name));
        }
    }

    #[test]
    #[should_panic(expected = "expected a map")]
    fn assert_single_sym_pair_rejects_non_map() {
        // The helper's failure contract (also covers its guard line).
        assert_single_sym_pair(Value::Nil, None, "x");
    }

    #[test]
    fn automap_ws_then_nonbrace_rejects_namespace() {
        // `nsmap_head`, auto + whitespace: skip to `{`, else `NsMapNs`.
        for input in ["#:: x {}", "#:: "] {
            let err = read_str(input, &opts()).unwrap_err();
            assert!(matches!(err, ReaderError::NsMapNs { .. }), "{input}");
            assert_eq!(err.message(), "Namespaced map must specify a namespace");
        }
    }

    #[test]
    fn automap_non_symbol_ns_rejected() {
        // `resolve_nsmap_ns`, auto + non-symbol head.
        let err = read_str("#::1 {}", &opts()).unwrap_err();
        assert!(matches!(err, ReaderError::NsMapValidNs { .. }));
        assert_eq!(
            err.message(),
            "Namespaced map must specify a valid namespace: 1"
        );
    }

    #[test]
    fn cond_eof_where_feature_expected() {
        // `cond_match_arm` head read hits EOF.
        let err = read_str("#?(", &opts()).unwrap_err();
        assert!(matches!(err, ReaderError::EofDelimited { .. }));
        assert_eq!(err.message(), "EOF while reading, starting at line 1");
    }

    #[test]
    fn cond_eof_where_value_expected() {
        // Feature matched, then EOF where its value goes.
        let err = read_str("#?(:clj", &opts()).unwrap_err();
        assert!(matches!(err, ReaderError::EofDelimited { line: 1, .. }));
    }

    #[test]
    fn cond_eof_in_discard_position() {
        // Unmatched feature, then EOF where its discarded value goes.
        let err = read_str("#?(:nothere", &opts()).unwrap_err();
        assert!(matches!(err, ReaderError::EofDelimited { line: 1, .. }));
    }

    #[test]
    fn preserve_cond_failure_restores_in_cond() {
        // One reused `Reader`: a failed preserve read must not
        // stick `in_cond`, else the next tag read returns
        // `TaggedLiteral` instead of the `DataReader` error.
        let opts = ReaderOpts {
            read_cond: ReadCond::Preserve,
            ..ReaderOpts::default()
        };
        let mut reader = Reader::new("#?(a b] #inst \"x\"", &opts);
        let err = reader.read_form(true).unwrap_err();
        assert!(
            matches!(err, ReaderError::UnmatchedDelim { .. }),
            "got {err:?}"
        );
        let err = reader.read_form(true).unwrap_err();
        assert!(matches!(err, ReaderError::DataReader { .. }), "got {err:?}");
    }

    #[test]
    fn nsmap_prefixes_unqualified_symbol_keys() {
        // `nsmap_key` unqualified-symbol arm.
        let v = read_str("#:foo{a 1}", &opts()).unwrap();
        assert_single_sym_pair(v, Some("foo"), "a");
    }

    #[test]
    fn nsmap_strips_underscore_symbol_ns() {
        // `nsmap_key` `_`-ns symbol arm.
        let v = read_str("#:foo{_/a 1}", &opts()).unwrap();
        assert_single_sym_pair(v, None, "a");
    }
}
