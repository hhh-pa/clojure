// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! [`ReaderError`]: every read failure with its span, root-cause
//! message, and root-cause JVM class (`reader-contract.md` §22).
//!
//! The JVM wraps the root cause in a `ReaderException` carrying
//! ex-data line/column; here each error carries its [`Span`] directly
//! (captured from the live tracker at the throw site, so positions —
//! including column 0 and the EOF bump — match exactly). [`message`](ReaderError::message)
//! and [`jvm_class`](ReaderError::jvm_class) are what the differential
//! harness compares.

use std::fmt;

/// A 1-based `(line, column)` position; column 0 is observable after
/// `unread('\n')` (see [`CharStream`](crate::stream::CharStream)).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Span {
    /// 1-based line.
    pub line: u32,
    /// 1-based column (0 after `unread('\n')`).
    pub col: u32,
}

impl Span {
    /// Build a span (the reader captures these from its tracker).
    #[must_use]
    pub fn new(line: u32, col: u32) -> Self {
        Self { line, col }
    }
}

impl fmt::Display for Span {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}:{}", self.line, self.col)
    }
}

/// Every read failure, each with its [`Span`].
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ReaderError {
    /// `EOF while reading`.
    Eof { span: Span },
    /// `EOF while reading, starting at line {line}` (`line` is the
    /// delimited list's first line; the span is the EOF position).
    EofDelimited { span: Span, line: u32 },
    /// `EOF while reading character`.
    EofChar { span: Span },
    /// `EOF while reading string`.
    EofString { span: Span },
    /// `EOF while reading regex`.
    EofRegex { span: Span },
    /// `Invalid token: {token}`.
    InvalidToken { span: Span, token: String },
    /// `Invalid number: {token}`.
    InvalidNumber { span: Span, token: String },
    /// `Radix out of range`.
    RadixRange { span: Span },
    /// `For input string: "{digits}" under radix {radix}` (radix 10
    /// omits the suffix — a JDK `BigInteger` message quirk).
    RadixDigit {
        span: Span,
        digits: String,
        radix: u32,
    },
    /// `Unsupported character: \{token}`.
    InvalidChar { span: Span, token: String },
    /// `Invalid character constant: \u{hex}` (lowercase hex).
    InvalidCharConst { span: Span, hex: String },
    /// `Invalid unicode character: \{token}`.
    BadUnicodeChar { span: Span, token: String },
    /// `Invalid digit: {c}`.
    BadDigit { span: Span, c: char },
    /// `Invalid octal escape sequence length: {len}`.
    BadOctalLen { span: Span, len: usize },
    /// `Octal escape sequence must be in range [0, 377].`
    OctalRange { span: Span },
    /// `Unsupported escape character: \{c}`.
    BadEscape { span: Span, c: char },
    /// `Invalid unicode escape: \u{c}` (EOF renders U+FFFF).
    BadUnicodeEscape { span: Span, c: char },
    /// `Invalid character length: {got}, should be: {want}`.
    BadUnicodeLen { span: Span, got: usize, want: usize },
    /// `Unmatched delimiter: {c}`.
    UnmatchedDelim { span: Span, c: char },
    /// `Map literal must contain an even number of forms`.
    MapOdd { span: Span },
    /// `Duplicate key: {key}` (key pre-rendered with `java_debug`).
    DupKey { span: Span, key: String },
    /// `Metadata must be Symbol,Keyword,String,Vector or Map`.
    BadMeta { span: Span },
    /// `Metadata can only be applied to IMetas`.
    MetaTarget { span: Span },
    /// `Nested #()s are not allowed`.
    NestedFn { span: Span },
    /// `arg literal must be %, %& or %integer`.
    BadArg { span: Span },
    /// `splice not in list` (top-level syntax-quote splice).
    SpliceTop { span: Span },
    /// `Unknown symbolic value: ##{sym}`.
    UnknownSymbolic { span: Span, sym: String },
    /// `Invalid token: ##{dbg}`.
    BadSymbolic { span: Span, dbg: String },
    /// `Reader tag must be a symbol`.
    TagNotSymbol { span: Span },
    /// `No reader function for tag {tag}`.
    UnknownTag { span: Span, tag: String },
    /// `Unreadable form`.
    Unreadable { span: Span },
    /// `Conditional read not allowed`.
    CondNotAllowed { span: Span },
    /// `read-cond body must be a list`.
    CondBody { span: Span },
    /// `Feature name {kw} is reserved.`
    ReservedFeature { span: Span, kw: String },
    /// `Feature should be a keyword: {dbg}`.
    BadFeature { span: Span, dbg: String },
    /// Even-forms error, with the body's first line when tracked.
    CondEven { span: Span, line: Option<u32> },
    /// `Spliced form list in read-cond-splicing must implement java.util.List`.
    SpliceType { span: Span },
    /// `Reader conditional splicing not allowed at the top level.`
    SpliceTopLevel { span: Span },
    /// `Namespaced map must specify a namespace`.
    NsMapNs { span: Span },
    /// `Namespaced map must specify a map`.
    NsMapMap { span: Span },
    /// `Namespaced map must specify a valid namespace: {dbg}`.
    NsMapValidNs { span: Span, dbg: String },
    /// `Unknown auto-resolved namespace alias: {sym}`.
    NsMapAlias { span: Span, sym: String },
    /// `Namespaced map literal must contain an even number of forms`.
    NsMapOdd { span: Span },
    /// `Integer.parseInt` overflow on a `%digits` arg (beyond §22:
    /// `For input string: "{token}"`, class `NumberFormatException`).
    ArgNumber { span: Span, token: String },
    /// Ratio deferral (big-number follow-up N3).
    Ratio { span: Span, token: String },
    /// BigInt deferral (big-number follow-up N3).
    BigInt { span: Span, token: String },
    /// BigDecimal deferral (big-number follow-up N3).
    BigDecimal { span: Span, token: String },
    /// `#=` deferral (data-reader follow-up N5).
    ReadEval { span: Span },
    /// `#inst`/`#uuid` deferral (data-reader follow-up N5).
    DataReader { span: Span, tag: String },
    /// Record-tag deferral (S5).
    Record { span: Span, tag: String },
    /// Lone surrogate in a string `\u` escape (JVM accepts; `str`
    /// cannot represent it).
    LoneSurrogate { span: Span, hex: String },
}

impl ReaderError {
    /// The error span (live tracker position at the throw site).
    #[must_use]
    pub fn span(&self) -> Span {
        match *self {
            Self::Eof { span }
            | Self::EofChar { span }
            | Self::EofString { span }
            | Self::EofRegex { span }
            | Self::RadixRange { span }
            | Self::OctalRange { span }
            | Self::MapOdd { span }
            | Self::BadMeta { span }
            | Self::MetaTarget { span }
            | Self::NestedFn { span }
            | Self::BadArg { span }
            | Self::SpliceTop { span }
            | Self::TagNotSymbol { span }
            | Self::Unreadable { span }
            | Self::CondNotAllowed { span }
            | Self::CondBody { span }
            | Self::SpliceType { span }
            | Self::SpliceTopLevel { span }
            | Self::NsMapNs { span }
            | Self::NsMapMap { span }
            | Self::NsMapOdd { span }
            | Self::ReadEval { span } => span,
            Self::EofDelimited { span, .. }
            | Self::InvalidToken { span, .. }
            | Self::InvalidNumber { span, .. }
            | Self::RadixDigit { span, .. }
            | Self::InvalidChar { span, .. }
            | Self::InvalidCharConst { span, .. }
            | Self::BadUnicodeChar { span, .. }
            | Self::BadDigit { span, .. }
            | Self::BadOctalLen { span, .. }
            | Self::BadEscape { span, .. }
            | Self::BadUnicodeEscape { span, .. }
            | Self::BadUnicodeLen { span, .. }
            | Self::UnmatchedDelim { span, .. }
            | Self::DupKey { span, .. }
            | Self::UnknownSymbolic { span, .. }
            | Self::BadSymbolic { span, .. }
            | Self::UnknownTag { span, .. }
            | Self::ReservedFeature { span, .. }
            | Self::BadFeature { span, .. }
            | Self::CondEven { span, .. }
            | Self::NsMapValidNs { span, .. }
            | Self::NsMapAlias { span, .. }
            | Self::ArgNumber { span, .. }
            | Self::Ratio { span, .. }
            | Self::BigInt { span, .. }
            | Self::BigDecimal { span, .. }
            | Self::DataReader { span, .. }
            | Self::Record { span, .. }
            | Self::LoneSurrogate { span, .. } => span,
        }
    }

    /// The root-cause message, exactly as on the JVM (or the S2
    /// deferral text, which has no JVM counterpart).
    #[must_use]
    pub fn message(&self) -> String {
        match self {
            Self::Eof { .. } => "EOF while reading".to_owned(),
            Self::EofDelimited { line, .. } => {
                format!("EOF while reading, starting at line {line}")
            }
            Self::EofChar { .. } => "EOF while reading character".to_owned(),
            Self::EofString { .. } => "EOF while reading string".to_owned(),
            Self::EofRegex { .. } => "EOF while reading regex".to_owned(),
            Self::InvalidToken { token, .. } => format!("Invalid token: {token}"),
            Self::InvalidNumber { token, .. } => format!("Invalid number: {token}"),
            Self::RadixRange { .. } => "Radix out of range".to_owned(),
            Self::RadixDigit { digits, radix, .. } => {
                if *radix == 10 {
                    format!("For input string: \"{digits}\"")
                } else {
                    format!("For input string: \"{digits}\" under radix {radix}")
                }
            }
            Self::InvalidChar { token, .. } => format!("Unsupported character: \\{token}"),
            Self::InvalidCharConst { hex, .. } => {
                format!("Invalid character constant: \\u{hex}")
            }
            Self::BadUnicodeChar { token, .. } => {
                format!("Invalid unicode character: \\{token}")
            }
            Self::BadDigit { c, .. } => format!("Invalid digit: {c}"),
            Self::BadOctalLen { len, .. } => {
                format!("Invalid octal escape sequence length: {len}")
            }
            Self::OctalRange { .. } => {
                "Octal escape sequence must be in range [0, 377].".to_owned()
            }
            Self::BadEscape { c, .. } => format!("Unsupported escape character: \\{c}"),
            Self::BadUnicodeEscape { c, .. } => format!("Invalid unicode escape: \\u{c}"),
            Self::BadUnicodeLen { got, want, .. } => {
                format!("Invalid character length: {got}, should be: {want}")
            }
            Self::UnmatchedDelim { c, .. } => format!("Unmatched delimiter: {c}"),
            Self::MapOdd { .. } => "Map literal must contain an even number of forms".to_owned(),
            Self::DupKey { key, .. } => format!("Duplicate key: {key}"),
            Self::BadMeta { .. } => {
                "Metadata must be Symbol,Keyword,String,Vector or Map".to_owned()
            }
            Self::MetaTarget { .. } => "Metadata can only be applied to IMetas".to_owned(),
            Self::NestedFn { .. } => "Nested #()s are not allowed".to_owned(),
            Self::BadArg { .. } => "arg literal must be %, %& or %integer".to_owned(),
            Self::SpliceTop { .. } => "splice not in list".to_owned(),
            Self::UnknownSymbolic { sym, .. } => format!("Unknown symbolic value: ##{sym}"),
            Self::BadSymbolic { dbg, .. } => format!("Invalid token: ##{dbg}"),
            Self::TagNotSymbol { .. } => "Reader tag must be a symbol".to_owned(),
            Self::UnknownTag { tag, .. } => format!("No reader function for tag {tag}"),
            Self::Unreadable { .. } => "Unreadable form".to_owned(),
            Self::CondNotAllowed { .. } => "Conditional read not allowed".to_owned(),
            Self::CondBody { .. } => "read-cond body must be a list".to_owned(),
            Self::ReservedFeature { kw, .. } => format!("Feature name {kw} is reserved."),
            Self::BadFeature { dbg, .. } => format!("Feature should be a keyword: {dbg}"),
            Self::CondEven { line, .. } => match line {
                Some(l) => {
                    format!("read-cond starting on line {l} requires an even number of forms")
                }
                None => "read-cond requires an even number of forms.".to_owned(),
            },
            Self::SpliceType { .. } => {
                "Spliced form list in read-cond-splicing must implement java.util.List".to_owned()
            }
            Self::SpliceTopLevel { .. } => {
                "Reader conditional splicing not allowed at the top level.".to_owned()
            }
            Self::NsMapNs { .. } => "Namespaced map must specify a namespace".to_owned(),
            Self::NsMapMap { .. } => "Namespaced map must specify a map".to_owned(),
            Self::NsMapValidNs { dbg, .. } => {
                format!("Namespaced map must specify a valid namespace: {dbg}")
            }
            Self::NsMapAlias { sym, .. } => {
                format!("Unknown auto-resolved namespace alias: {sym}")
            }
            Self::NsMapOdd { .. } => {
                "Namespaced map literal must contain an even number of forms".to_owned()
            }
            Self::ArgNumber { token, .. } => format!("For input string: \"{token}\""),
            Self::Ratio { token, .. } => format!(
                "Ratio literals are not supported in this build (big-number follow-up N3): {token}"
            ),
            Self::BigInt { token, .. } => format!(
                "BigInt literals are not supported in this build (big-number follow-up N3): {token}"
            ),
            Self::BigDecimal { token, .. } => format!(
                "BigDecimal literals are not supported in this build (big-number follow-up N3): {token}"
            ),
            Self::ReadEval { .. } => {
                "Eval reader #= is not supported in this build (data-reader follow-up N5)"
                    .to_owned()
            }
            Self::DataReader { tag, .. } => format!(
                "Reader for tag {tag} is not supported in this build (data-reader follow-up N5)"
            ),
            Self::Record { tag, .. } => {
                format!("Record construction is not supported in this build (S5): #{tag}")
            }
            Self::LoneSurrogate { hex, .. } => {
                format!("Lone surrogate \\u{hex} in string escape is not representable in str")
            }
        }
    }

    /// The root-cause simple class name (`RuntimeException`,
    /// `NumberFormatException`, `IllegalArgumentException`,
    /// `IllegalStateException`; deferrals report `ReaderError`, which
    /// has no JVM counterpart).
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        match self {
            Self::InvalidNumber { .. }
            | Self::RadixRange { .. }
            | Self::RadixDigit { .. }
            | Self::ArgNumber { .. } => "NumberFormatException",
            Self::BadUnicodeChar { .. }
            | Self::BadDigit { .. }
            | Self::BadUnicodeLen { .. }
            | Self::DupKey { .. }
            | Self::BadMeta { .. }
            | Self::MetaTarget { .. } => "IllegalArgumentException",
            Self::NestedFn { .. } | Self::BadArg { .. } | Self::SpliceTop { .. } => {
                "IllegalStateException"
            }
            Self::Ratio { .. }
            | Self::BigInt { .. }
            | Self::BigDecimal { .. }
            | Self::ReadEval { .. }
            | Self::DataReader { .. }
            | Self::Record { .. }
            | Self::LoneSurrogate { .. } => "ReaderError",
            _ => "RuntimeException",
        }
    }
}

impl fmt::Display for ReaderError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.span(), self.message())
    }
}

impl std::error::Error for ReaderError {}

#[cfg(test)]
mod tests {
    use super::{ReaderError, Span};

    #[test]
    fn messages_and_classes() {
        let span = Span::new(1, 1);
        let cases: Vec<(ReaderError, &str, &str)> = vec![
            (
                ReaderError::Eof { span },
                "EOF while reading",
                "RuntimeException",
            ),
            (
                ReaderError::EofDelimited { span, line: 3 },
                "EOF while reading, starting at line 3",
                "RuntimeException",
            ),
            (
                ReaderError::InvalidNumber {
                    span,
                    token: "1a".to_owned(),
                },
                "Invalid number: 1a",
                "NumberFormatException",
            ),
            (
                ReaderError::RadixDigit {
                    span,
                    digits: "2".to_owned(),
                    radix: 2,
                },
                "For input string: \"2\" under radix 2",
                "NumberFormatException",
            ),
            // Radix 10 omits the suffix (JDK quirk).
            (
                ReaderError::RadixDigit {
                    span,
                    digits: "A".to_owned(),
                    radix: 10,
                },
                "For input string: \"A\"",
                "NumberFormatException",
            ),
            (
                ReaderError::DupKey {
                    span,
                    key: ":a".to_owned(),
                },
                "Duplicate key: :a",
                "IllegalArgumentException",
            ),
            (
                ReaderError::BadArg { span },
                "arg literal must be %, %& or %integer",
                "IllegalStateException",
            ),
            (
                ReaderError::CondEven {
                    span,
                    line: Some(1),
                },
                "read-cond starting on line 1 requires an even number of forms",
                "RuntimeException",
            ),
            (
                ReaderError::CondEven { span, line: None },
                "read-cond requires an even number of forms.",
                "RuntimeException",
            ),
            (
                ReaderError::Ratio {
                    span,
                    token: "1/2".to_owned(),
                },
                "Ratio literals are not supported in this build (big-number follow-up N3): 1/2",
                "ReaderError",
            ),
            (
                ReaderError::LoneSurrogate {
                    span,
                    hex: "d800".to_owned(),
                },
                "Lone surrogate \\ud800 in string escape is not representable in str",
                "ReaderError",
            ),
        ];
        for (err, message, class) in cases {
            assert_eq!(err.message(), message);
            assert_eq!(err.jvm_class(), class);
            assert_eq!(err.span(), span);
        }
    }
}
