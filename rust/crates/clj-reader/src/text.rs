// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Characters, strings, and regexes (`reader-contract.md` §6–§8).
//!
//! - Characters (`\`): one char plus a token; length 1 wins before
//!   `u`/`o` handling (bare `\u` is `u`); named words; `u` + exactly
//!   4 hex (surrogates rejected, lowercase hex); `o` + up to 3 octal
//!   digits (length 4+ rejected, `> 0o377` rejected).
//! - Strings (`"`): escapes `t r n \ " b f`, `u` + 4 hex (first digit
//!   checked eagerly with the single-char message; EOF there renders
//!   U+FFFF), ASCII-decimal octal (up to 3 digits; non-octal digits
//!   error rather than terminate; `> 0o377` rejected), else
//!   `Unsupported escape`. Lone surrogates raise our loud error (the
//!   JVM accepts them — §24.6).
//! - Regexes (`#"`): a raw loop, never the string reader —
//!   backslash pairs append verbatim. Stored without compiling (no
//!   `regex` dependency; invalid patterns read fine — §24.5).

use std::sync::Arc;

use clj_value::Value;

use crate::Reader;
use crate::error::{ReaderError, Span};
use crate::stream::CharStream;
use crate::symbol::{is_macro, is_whitespace, read_token};

impl Reader {
    /// `CharacterReader`.
    pub(crate) fn char_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let Some(first) = self.stream.read() else {
            return Err(ReaderError::EofChar { span: self.span() });
        };
        let token = read_token(&mut self.stream, first);
        let span = self.span();
        // Lengths are char counts (JVM `String.length`) — `\é` is one char.
        if token.chars().count() == 1 {
            return Ok(Some(Value::Char(token.chars().next().unwrap())));
        }
        match token.as_str() {
            "newline" => return Ok(Some(Value::Char('\n'))),
            "space" => return Ok(Some(Value::Char(' '))),
            "tab" => return Ok(Some(Value::Char('\t'))),
            "backspace" => return Ok(Some(Value::Char('\u{8}'))),
            "formfeed" => return Ok(Some(Value::Char('\u{c}'))),
            "return" => return Ok(Some(Value::Char('\r'))),
            _ => {}
        }
        if let Some(rest) = token.strip_prefix('u') {
            let v = unicode_token_value(&token, rest, 16, 4, span)?;
            if matches!(v, 0xD800..=0xDFFF) {
                return Err(ReaderError::InvalidCharConst {
                    span,
                    hex: format!("{v:x}"),
                });
            }
            return Ok(Some(Value::Char(char::from_u32(v).unwrap())));
        }
        if let Some(rest) = token.strip_prefix('o') {
            let len = rest.chars().count();
            if len > 3 {
                return Err(ReaderError::BadOctalLen { span, len });
            }
            let v = unicode_token_value(&token, rest, 8, len, span)?;
            if v > 0o377 {
                return Err(ReaderError::OctalRange { span });
            }
            return Ok(Some(Value::Char(char::from_u32(v).unwrap())));
        }
        Err(ReaderError::InvalidChar { span, token })
    }

    /// `StringReader`.
    pub(crate) fn string_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let mut out = String::new();
        loop {
            let Some(c) = self.stream.read() else {
                return Err(ReaderError::EofString { span: self.span() });
            };
            if c == '"' {
                return Ok(Some(Value::Str(Arc::from(out.as_str()))));
            }
            if c != '\\' {
                out.push(c);
                continue;
            }
            let Some(e) = self.stream.read() else {
                return Err(ReaderError::EofString { span: self.span() });
            };
            match e {
                't' => out.push('\t'),
                'r' => out.push('\r'),
                'n' => out.push('\n'),
                '\\' => out.push('\\'),
                '"' => out.push('"'),
                'b' => out.push('\u{8}'),
                'f' => out.push('\u{c}'),
                'u' => {
                    let c = self.string_unicode_escape()?;
                    out.push(c);
                }
                c if c.is_ascii_digit() => {
                    let v = self.string_octal_escape(c)?;
                    out.push(char::from_u32(v).unwrap());
                }
                c => {
                    return Err(ReaderError::BadEscape {
                        span: self.span(),
                        c,
                    });
                }
            }
        }
    }

    /// `\u` plus 4 hex in a string (first digit eager, exact length).
    fn string_unicode_escape(&mut self) -> Result<char, ReaderError> {
        match self.stream.read() {
            Some(c) if c.is_ascii_hexdigit() => {
                let v = pushback_unicode_value(&mut self.stream, c, 16, 4, true)?;
                match char::from_u32(v) {
                    Some(c) => Ok(c),
                    None => Err(ReaderError::LoneSurrogate {
                        span: self.span(),
                        hex: format!("{v:04x}"),
                    }),
                }
            }
            // EOF renders U+FFFF (`(char) -1`).
            other => Err(ReaderError::BadUnicodeEscape {
                span: self.span(),
                c: other.unwrap_or('\u{FFFF}'),
            }),
        }
    }

    /// Octal escape in a string (up to 3 digits, inexact length).
    fn string_octal_escape(&mut self, first: char) -> Result<u32, ReaderError> {
        let v = pushback_unicode_value(&mut self.stream, first, 8, 3, false)?;
        if v > 0o377 {
            return Err(ReaderError::OctalRange { span: self.span() });
        }
        Ok(v)
    }

    /// `RegexReader`: raw loop, backslash pairs verbatim, EOF is its
    /// own message; the source is stored without compiling.
    pub(crate) fn regex_reader(&mut self) -> Result<Option<Value>, ReaderError> {
        let mut out = String::new();
        loop {
            let Some(c) = self.stream.read() else {
                return Err(ReaderError::EofRegex { span: self.span() });
            };
            if c == '"' {
                return Ok(Some(Value::Regex(Arc::from(out.as_str()))));
            }
            out.push(c);
            if c == '\\' {
                let Some(next) = self.stream.read() else {
                    return Err(ReaderError::EofRegex { span: self.span() });
                };
                out.push(next);
            }
        }
    }
}

/// `readUnicodeChar(String token, offset, length, base)`: exact
/// digit count (char count), then per-digit validation.
fn unicode_token_value(
    token: &str,
    digits: &str,
    base: u32,
    want: usize,
    span: Span,
) -> Result<u32, ReaderError> {
    if digits.chars().count() != want {
        return Err(ReaderError::BadUnicodeChar {
            span,
            token: token.to_owned(),
        });
    }
    let mut value: u32 = 0;
    for c in digits.chars() {
        match c.to_digit(base) {
            Some(d) => value = value * base + d,
            None => return Err(ReaderError::BadDigit { span, c }),
        }
    }
    Ok(value)
}

/// `readUnicodeChar(PushbackReader r, initch, base, length, exact)`:
/// the first digit is consumed; further chars stop (unread) at
/// EOF/whitespace/macro, error on non-digits, and short reads error
/// only when exact.
fn pushback_unicode_value(
    stream: &mut CharStream,
    first: char,
    base: u32,
    length: usize,
    exact: bool,
) -> Result<u32, ReaderError> {
    // The first digit can already fail (`"\8"`, `"\18"`).
    let Some(digit) = first.to_digit(base) else {
        return Err(ReaderError::BadDigit {
            span: Span::new(stream.line(), stream.col()),
            c: first,
        });
    };
    let mut value = digit;
    let mut count = 1;
    while count < length {
        match stream.read() {
            None => {
                // EOF stops the escape (no unread of EOF).
                break;
            }
            Some(c) if is_whitespace(c) || is_macro(c) => {
                stream.unread(c);
                break;
            }
            Some(c) => match c.to_digit(base) {
                Some(d) => {
                    value = value * base + d;
                    count += 1;
                }
                None => {
                    return Err(ReaderError::BadDigit {
                        span: Span::new(stream.line(), stream.col()),
                        c,
                    });
                }
            },
        }
    }
    if exact && count != length {
        return Err(ReaderError::BadUnicodeLen {
            span: Span::new(stream.line(), stream.col()),
            got: count,
            want: length,
        });
    }
    Ok(value)
}
