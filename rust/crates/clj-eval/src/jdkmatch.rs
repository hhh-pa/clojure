// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! JDK `java.util.regex.Matcher` semantics over the `regex` crate
//! (#273), shared by `clojure.string/replace`, `replace-first`,
//! `split`, `re-seq` and the `Matcher` emulation:
//!
//! - **`find` order.** After an empty match the next search starts one
//!   character later; after a non-empty one it starts right at its end,
//!   where an empty match counts (`"aaa"` over `a*` finds `"aaa"` then
//!   `""`). The crate's iterators skip that empty match, so patterns
//!   that can match the empty string step with `find_at` instead; for
//!   every other pattern both orders agree and the crate's iterators
//!   run.
//! - **Replacement templates.** `Matcher.appendReplacement`'s grammar:
//!   `\` quotes the next character, `$n` takes the longest group number
//!   that stays legal (`$10` with one group is group 1 then `0`),
//!   `${name}` a named group, with the JDK's exception classes and
//!   texts.

use std::borrow::Cow;
use std::iter::Peekable;
use std::str::Chars;
use std::sync::Arc;

use clj_value::{ThrowClass, Value};
use regex::{Captures, Match, Regex};

use crate::error::NativeError;

/// Whether a (crate-syntax) pattern can match the empty string, i.e.
/// whether its `find` order can differ from the crate's iterators. An
/// unparsable source answers `true` (the stepping order is always
/// right, only slower).
pub(crate) fn matches_empty(crate_source: &str) -> bool {
    regex_syntax::parse(crate_source).map_or(true, |hir| hir.properties().minimum_len() == Some(0))
}

/// Where the JDK's next `find` starts after a match over
/// `start..end`: one character past an empty match, else its end.
pub(crate) fn next_from(text: &str, start: usize, end: usize) -> usize {
    if start == end {
        end + text[end..].chars().next().map_or(1, char::len_utf8)
    } else {
        end
    }
}

/// The JDK stepping state over one input (see the module docs).
pub(crate) struct Step<'r, 't> {
    re: &'r Regex,
    text: &'t str,
    pos: usize,
}

impl<'t> Step<'_, 't> {
    /// One `find` through `search` (a crate `*_at` search), moving
    /// past the hit; an exhausted step stays exhausted.
    fn next_with<T>(
        &mut self,
        search: impl FnOnce(&Regex, &'t str, usize) -> Option<T>,
        span: impl FnOnce(&T) -> (usize, usize),
    ) -> Option<T> {
        if self.pos > self.text.len() {
            return None;
        }
        let Some(hit) = search(self.re, self.text, self.pos) else {
            self.pos = self.text.len() + 1;
            return None;
        };
        let (start, end) = span(&hit);
        self.pos = next_from(self.text, start, end);
        Some(hit)
    }
}

/// Successive `find` matches (spans only).
pub(crate) enum Finds<'r, 't> {
    Crate(regex::Matches<'r, 't>),
    Step(Step<'r, 't>),
}

impl<'t> Iterator for Finds<'_, 't> {
    type Item = Match<'t>;

    fn next(&mut self) -> Option<Match<'t>> {
        match self {
            Self::Crate(it) => it.next(),
            Self::Step(s) => s.next_with(Regex::find_at, |m| (m.start(), m.end())),
        }
    }
}

/// Successive `find` matches with their groups.
pub(crate) enum CaptureFinds<'r, 't> {
    Crate(regex::CaptureMatches<'r, 't>),
    Step(Step<'r, 't>),
}

impl<'t> Iterator for CaptureFinds<'_, 't> {
    type Item = Captures<'t>;

    fn next(&mut self) -> Option<Captures<'t>> {
        match self {
            Self::Crate(it) => it.next(),
            Self::Step(s) => s.next_with(Regex::captures_at, span_of),
        }
    }
}

/// The JDK `find` order over `text` (`empty`: [`matches_empty`] of
/// the pattern).
pub(crate) fn finds<'r, 't>(re: &'r Regex, empty: bool, text: &'t str) -> Finds<'r, 't> {
    if empty {
        Finds::Step(Step { re, text, pos: 0 })
    } else {
        Finds::Crate(re.find_iter(text))
    }
}

/// [`finds`] with groups.
pub(crate) fn capture_finds<'r, 't>(
    re: &'r Regex,
    empty: bool,
    text: &'t str,
) -> CaptureFinds<'r, 't> {
    if empty {
        CaptureFinds::Step(Step { re, text, pos: 0 })
    } else {
        CaptureFinds::Crate(re.captures_iter(text))
    }
}

/// A match's whole span.
fn span_of(caps: &Captures<'_>) -> (usize, usize) {
    let m = caps.get(0).expect("a match holds group 0");
    (m.start(), m.end())
}

/// A match the splice loop can cut around.
pub(crate) trait Hit {
    /// The whole match's byte span.
    fn span(&self) -> (usize, usize);
}

impl Hit for Match<'_> {
    fn span(&self) -> (usize, usize) {
        (self.start(), self.end())
    }
}

impl Hit for Captures<'_> {
    fn span(&self) -> (usize, usize) {
        span_of(self)
    }
}

/// The `replaceAll` / `replaceFirst` loop: the text between matches
/// verbatim, each match through `push` (the first `limit` only), the
/// tail; the input itself when nothing matches.
pub(crate) fn splice<'t, H: Hit>(
    text: &'t str,
    hits: impl Iterator<Item = H>,
    limit: usize,
    mut push: impl FnMut(&mut String, &H) -> Result<(), NativeError>,
) -> Result<Cow<'t, str>, NativeError> {
    let mut out = String::new();
    let mut last = 0;
    let mut matched = false;
    for hit in hits.take(limit) {
        let (start, end) = hit.span();
        if !matched {
            out.reserve(text.len());
            matched = true;
        }
        out.push_str(&text[last..start]);
        push(&mut out, &hit)?;
        last = end;
    }
    if !matched {
        return Ok(Cow::Borrowed(text));
    }
    out.push_str(&text[last..]);
    Ok(Cow::Owned(out))
}

/// One piece of a parsed replacement template.
#[derive(Debug)]
enum Piece {
    Lit(String),
    Group(usize),
}

/// A replacement template parsed against a pattern's groups: the
/// pieces up to the first error, and that error (the JDK appends as it
/// scans, so a failing template has still emitted its prefix).
#[derive(Debug)]
pub(crate) struct Template {
    pieces: Vec<Piece>,
    error: Option<NativeError>,
}

/// An `IllegalArgumentException` with the JDK's text.
fn iae(message: impl Into<String>) -> NativeError {
    NativeError::new(ThrowClass::IllegalArgumentException, message.into())
}

impl Template {
    /// Parse `rep` against `re`'s groups (`Matcher.appendReplacement`).
    pub(crate) fn parse(re: &Regex, rep: &str) -> Self {
        let mut pieces = Vec::new();
        let mut lit = String::new();
        let mut chars = rep.chars().peekable();
        let mut error = None;
        while let Some(c) = chars.next() {
            let group = match c {
                '\\' => match chars.next() {
                    Some(q) => {
                        lit.push(q);
                        continue;
                    }
                    None => Err(iae("character to be escaped is missing")),
                },
                '$' => group_ref(re, &mut chars),
                c => {
                    lit.push(c);
                    continue;
                }
            };
            if !lit.is_empty() {
                pieces.push(Piece::Lit(std::mem::take(&mut lit)));
            }
            match group {
                Ok(g) => pieces.push(Piece::Group(g)),
                Err(e) => {
                    error = Some(e);
                    break;
                }
            }
        }
        if !lit.is_empty() {
            pieces.push(Piece::Lit(lit));
        }
        Self { pieces, error }
    }

    /// A template that fails before emitting anything (a nil
    /// replacement's NPE).
    pub(crate) fn failing(error: NativeError) -> Self {
        Self {
            pieces: Vec::new(),
            error: Some(error),
        }
    }

    /// The parse failure, if any.
    pub(crate) fn error(&self) -> Option<&NativeError> {
        self.error.as_ref()
    }

    /// Whether expanding needs a numbered group past group 0.
    pub(crate) fn needs_groups(&self) -> bool {
        self.pieces
            .iter()
            .any(|p| matches!(p, Piece::Group(g) if *g > 0))
    }

    /// Append the expansion (the error-free prefix) to `out`, reading
    /// group texts through `group` (unmatched groups append nothing).
    pub(crate) fn expand_into<'a>(
        &self,
        out: &mut String,
        group: impl Fn(usize) -> Option<&'a str>,
    ) {
        for piece in &self.pieces {
            match piece {
                Piece::Lit(s) => out.push_str(s),
                Piece::Group(g) => out.push_str(group(*g).unwrap_or("")),
            }
        }
    }
}

/// The group a `$` names (the `$` itself consumed): a number, or a
/// `{name}`.
fn group_ref(re: &Regex, chars: &mut Peekable<Chars<'_>>) -> Result<usize, NativeError> {
    let first = chars
        .next()
        .ok_or_else(|| iae("Illegal group reference: group index is missing"))?;
    if first == '{' {
        return named_group(re, chars);
    }
    let groups = re.captures_len() - 1;
    let mut n = first
        .to_digit(10)
        .ok_or_else(|| iae("Illegal group reference"))? as usize;
    // The longest group number that stays legal.
    while let Some(d) = chars.peek().and_then(|c| c.to_digit(10)) {
        let wider = n * 10 + d as usize;
        if wider > groups {
            break;
        }
        n = wider;
        chars.next();
    }
    if n > groups {
        return Err(NativeError::new(
            ThrowClass::IndexOutOfBoundsException,
            format!("No group {n}"),
        ));
    }
    Ok(n)
}

/// A `${name}` reference (after the `{`): ASCII letters and digits,
/// the closing `}`, a name that does not start with a digit and that
/// the pattern defines.
fn named_group(re: &Regex, chars: &mut Peekable<Chars<'_>>) -> Result<usize, NativeError> {
    let mut name = String::new();
    while let Some(&c) = chars.peek().filter(|c| c.is_ascii_alphanumeric()) {
        name.push(c);
        chars.next();
    }
    if name.is_empty() {
        return Err(iae("named capturing group has 0 length name"));
    }
    if chars.next() != Some('}') {
        return Err(iae("named capturing group is missing trailing '}'"));
    }
    if name.starts_with(|c: char| c.is_ascii_digit()) {
        return Err(iae(format!(
            "capturing group name {{{name}}} starts with digit character"
        )));
    }
    re.capture_names()
        .position(|n| n == Some(name.as_str()))
        .ok_or_else(|| iae(format!("No group with name {{{name}}}")))
}

/// `Matcher.replaceAll(String)` (`limit` `usize::MAX`) or
/// `replaceFirst` (`1`) over the whole input: the input itself when
/// nothing matches (the template is never checked then), the
/// template's failure when something does.
pub(crate) fn replace<'t>(
    re: &Regex,
    empty: bool,
    text: &'t str,
    rep: &str,
    limit: usize,
) -> Result<Cow<'t, str>, NativeError> {
    let template = Template::parse(re, rep);
    if let Some(err) = template.error() {
        return if re.is_match(text) {
            Err(err.clone())
        } else {
            Ok(Cow::Borrowed(text))
        };
    }
    if template.needs_groups() {
        return splice(text, capture_finds(re, empty, text), limit, |out, caps| {
            template.expand_into(out, |g| caps.get(g).map(|m| m.as_str()));
            Ok(())
        });
    }
    splice(text, finds(re, empty, text), limit, |out, m| {
        template.expand_into(out, |_| Some(m.as_str()));
        Ok(())
    })
}

/// A `Matcher.quoteReplacement` argument as its `String`: nil is the
/// quote's NPE, anything else fails the parameter cast.
pub(crate) fn replacement_string(v: &Value) -> Result<Arc<str>, NativeError> {
    match v {
        Value::Str(s) => Ok(Arc::clone(s)),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.indexOf(int)\" because \"s\" is null".to_owned(),
        )),
        _ => Err(crate::core::cce_err(v, "java.lang.String")),
    }
}

/// `Matcher.quoteReplacement`: `\` and `$` quoted.
pub(crate) fn quote_replacement(s: &str) -> Cow<'_, str> {
    if !s.contains(['\\', '$']) {
        return Cow::Borrowed(s);
    }
    let mut out = String::with_capacity(s.len() + 2);
    for c in s.chars() {
        if c == '\\' || c == '$' {
            out.push('\\');
        }
        out.push(c);
    }
    Cow::Owned(out)
}

/// A byte offset as the JDK's UTF-16 index (for index texts).
pub(crate) fn utf16_index(text: &str, byte: usize) -> usize {
    text[..byte].encode_utf16().count()
}
