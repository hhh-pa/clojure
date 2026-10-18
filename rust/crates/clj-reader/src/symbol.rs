// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Tokens, symbols, and keywords (`reader-contract.md` §2, §4).
//!
//! `readToken` accumulates until EOF/whitespace/terminating macro
//! (`#`, `'`, `%` are non-terminating, so `a'b` is one token).
//! `interpretToken` maps `nil`/`true`/`false`, else `matchSymbol` — a
//! hand-rolled `symbolPat` (with Java greediness and backtracking
//! order) plus validation, `::` auto-resolution, `Symbol::intern`
//! first-`/` split, and the `arraySymbolPat` fallback.

use clj_value::{Keyword, Symbol, Value};

use crate::ReaderOpts;
use crate::stream::CharStream;

/// `isWhitespace`: `Character.isWhitespace` plus `,`, as an explicit
/// table — NOT Unicode `White_Space` (U+0085, U+00A0, U+2007, U+202F
/// are excluded) and NOT `char::is_whitespace` (which disagrees both
/// ways).
#[must_use]
pub fn is_whitespace(c: char) -> bool {
    matches!(
        c,
        '\u{09}'
            | '\u{0A}'
            | '\u{0B}'
            | '\u{0C}'
            | '\u{0D}'
            | '\u{1C}'
            | '\u{1D}'
            | '\u{1E}'
            | '\u{1F}'
            | '\u{20}'
            | '\u{1680}'
            | '\u{2000}'
            | '\u{2001}'
            | '\u{2002}'
            | '\u{2003}'
            | '\u{2004}'
            | '\u{2005}'
            | '\u{2006}'
            | '\u{2008}'
            | '\u{2009}'
            | '\u{200A}'
            | '\u{2028}'
            | '\u{2029}'
            | '\u{205F}'
            | '\u{3000}'
            | ','
    )
}

/// Chars with a `macros[]` entry (ASCII only — `getMacro` returns
/// null past 255).
#[must_use]
pub fn is_macro(c: char) -> bool {
    matches!(
        c,
        '"' | ';'
            | '\''
            | '@'
            | '^'
            | '`'
            | '~'
            | '('
            | ')'
            | '['
            | ']'
            | '{'
            | '}'
            | '\\'
            | '%'
            | '#'
    )
}

/// Macro chars that terminate tokens (`#`, `'`, `%` do not).
#[must_use]
pub fn is_terminating_macro(c: char) -> bool {
    c != '#' && c != '\'' && c != '%' && is_macro(c)
}

/// `Character.isDigit` for the number path: ASCII digits plus
/// `is_numeric` (non-ASCII decimal digits take the number path, then
/// fail; exotic Nl/No chars diverge — §24.9).
#[must_use]
pub fn is_digit_start(c: char) -> bool {
    c.is_ascii_digit() || c.is_numeric()
}

/// Accumulate a token from `initch` until EOF/whitespace/terminating
/// macro (unreading the terminator unless EOF).
pub fn read_token(stream: &mut CharStream, initch: char) -> String {
    let mut token = String::from(initch);
    loop {
        match stream.read() {
            None => return token,
            Some(c) if is_whitespace(c) || is_terminating_macro(c) => {
                stream.unread(c);
                return token;
            }
            Some(c) => token.push(c),
        }
    }
}

/// `interpretToken`: `nil`/`true`/`false`, else `matchSymbol`
/// (numbers never come through here); `None` means `Invalid token`.
#[must_use]
pub fn interpret_token(s: &str, opts: &ReaderOpts) -> Option<Value> {
    match s {
        "nil" => Some(Value::Nil),
        "true" => Some(Value::Bool(true)),
        "false" => Some(Value::Bool(false)),
        _ => match_symbol(s, opts),
    }
}

/// `matchSymbol`: `None` means `Invalid token`.
pub fn match_symbol(s: &str, opts: &ReaderOpts) -> Option<Value> {
    if let Some((ns, name)) = match_symbol_pat(s) {
        // Validation first: a match that fails here returns null
        // WITHOUT trying `arraySymbolPat`.
        if ns.is_some_and(|ns| ns.ends_with(":/"))
            || name.ends_with(':')
            || after_first_char(s).contains("::")
        {
            return None;
        }
        if let Some(rest) = s.strip_prefix("::") {
            let ks = Symbol::intern(rest);
            let resolved = match ks.namespace() {
                // A qualified name looks its head up as an ALIAS.
                Some(alias) => opts.aliases.get(alias).cloned(),
                None => Some(opts.current_ns.clone()),
            };
            return resolved.map(|ns| Value::Keyword(Keyword::new(Some(&ns), ks.name())));
        }
        let is_keyword = s.starts_with(':');
        let sym = Symbol::intern(if is_keyword { &s[1..] } else { s });
        if is_keyword {
            Some(Value::Keyword(Keyword::new(sym.namespace(), sym.name())))
        } else {
            Some(Value::Symbol(sym))
        }
    } else {
        let (ns, name) = match_array_symbol_pat(s)?;
        Some(Value::Symbol(Symbol::new(Some(&ns), &name)))
    }
}

/// Everything after the first char (for the `indexOf("::", 1)` rule;
/// char-based so multibyte heads cannot panic the slice).
fn after_first_char(s: &str) -> &str {
    match s.chars().next() {
        Some(c) => &s[c.len_utf8()..],
        None => "",
    }
}

/// `symbolPat = [:]?([\D&&[^/]].*/)?(/|[\D&&[^/]][^/]*)`, full match.
/// Returns the ns group (with trailing `/`) and the name group.
/// Greediness/backtracking order replicated: with-`:` before
/// without, longest ns group first, then no group.
fn match_symbol_pat(s: &str) -> Option<(Option<&str>, &str)> {
    let mut rests = Vec::with_capacity(2);
    if let Some(stripped) = s.strip_prefix(':') {
        rests.push(stripped);
    }
    rests.push(s);
    for rest in rests {
        let slashes: Vec<usize> = rest.match_indices('/').map(|(i, _)| i).collect();
        for i in slashes.iter().rev() {
            let ns = &rest[..i + 1];
            let name = &rest[i + 1..];
            if ns_head_valid(rest) && name_valid(name) {
                return Some((Some(ns), name));
            }
        }
        if name_valid(rest) {
            return Some((None, rest));
        }
    }
    None
}

/// `[\D&&[^/]]`: Java `\D` is ASCII `[^0-9]`; also excludes `/`.
fn head_char_valid(c: char) -> bool {
    !c.is_ascii_digit() && c != '/'
}

/// The ns group's head is the first char of the whole rest.
fn ns_head_valid(rest: &str) -> bool {
    rest.chars().next().is_some_and(head_char_valid)
}

/// `/|[\D&&[^/]][^/]*`.
fn name_valid(name: &str) -> bool {
    if name == "/" {
        return true;
    }
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if head_char_valid(c) => chars.all(|c| c != '/'),
        _ => false,
    }
}

/// `arraySymbolPat = ([\D&&[^/:]].*)/([1-9])`, full match, group 1
/// greedy to the last `/`.
fn match_array_symbol_pat(s: &str) -> Option<(String, String)> {
    let last = s.chars().last()?;
    if !matches!(last, '1'..='9') {
        return None;
    }
    let body = &s[..s.len() - last.len_utf8()];
    let slash = body.rfind('/')?;
    // The `/` sits immediately before the trailing digit.
    if slash + 1 != body.len() {
        return None;
    }
    let group = &body[..slash];
    match group.chars().next() {
        Some(c) if head_char_valid(c) && c != ':' => Some((group.to_owned(), last.to_string())),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{interpret_token, is_whitespace};
    use crate::ReaderOpts;
    use clj_value::Value;

    #[test]
    fn whitespace_table() {
        // In the set.
        for c in [
            '\u{09}', '\u{0A}', '\u{0B}', '\u{0C}', '\u{0D}', '\u{1C}', '\u{1D}', '\u{1E}',
            '\u{1F}', '\u{20}', '\u{1680}', '\u{2000}', '\u{2001}', '\u{2002}', '\u{2003}',
            '\u{2004}', '\u{2005}', '\u{2006}', '\u{2008}', '\u{2009}', '\u{200A}', '\u{2028}',
            '\u{2029}', '\u{205F}', '\u{3000}', ',',
        ] {
            assert!(is_whitespace(c), "{c:?} should be whitespace");
        }
        // Excluded (where `char::is_whitespace` disagrees it says yes).
        for c in [
            '\u{85}', '\u{A0}', '\u{2007}', '\u{202F}', '\u{FEFF}', 'a', '/', ':',
        ] {
            assert!(!is_whitespace(c), "{c:?} should not be whitespace");
        }
    }

    #[test]
    fn literals_and_symbols() {
        let opts = ReaderOpts::default();
        assert_eq!(interpret_token("nil", &opts), Some(Value::Nil));
        assert_eq!(interpret_token("true", &opts), Some(Value::Bool(true)));
        assert_eq!(interpret_token("false", &opts), Some(Value::Bool(false)));
        // Non-terminating macro chars join tokens.
        for (input, want) in [
            ("a'b", "a'b"),
            ("a#b", "a#b"),
            ("%", "%"),
            ("%&", "%&"),
            ("%1", "%1"),
            ("a/b/c", "a/b/c"),
            ("/", "/"),
            ("...", "..."),
            (".", "."),
            ("e5", "e5"),
            (".5", ".5"),
            ("+a", "+a"),
            ("--", "--"),
        ] {
            let got = interpret_token(input, &opts);
            assert_eq!(
                got.map(|v| clj_value::java_debug(&v)),
                Some(want.to_owned()),
                "{input}"
            );
        }
        // Digit-led names are legal keywords.
        assert_eq!(
            interpret_token(":1", &opts).map(|v| clj_value::java_debug(&v)),
            Some(":1".to_owned())
        );
        // Array symbols.
        assert_eq!(
            interpret_token("a/1", &opts).map(|v| clj_value::java_debug(&v)),
            Some("a/1".to_owned())
        );
    }

    #[test]
    fn invalid_tokens() {
        let opts = ReaderOpts::default();
        for input in [
            "foo:/bar", ":a::b", "//", "a/", "/a", "/1", "ab/12", "::", ":::", ":", "a:", ":a:",
            "a::b", "a/b/", ":a/", "1:2", "/.5",
        ] {
            assert_eq!(interpret_token(input, &opts), None, "{input}");
        }
        // Unknown alias, and a qualified name looked up as an alias.
        assert_eq!(interpret_token("::nope/x", &opts), None);
        assert_eq!(interpret_token("::clojure.string/foo", &opts), None);
    }

    #[test]
    fn auto_resolve() {
        let mut opts = ReaderOpts::default();
        opts.aliases
            .insert("str".to_owned(), "clojure.string".to_owned());
        assert_eq!(
            interpret_token("::foo", &opts).map(|v| clj_value::java_debug(&v)),
            Some(":user/foo".to_owned())
        );
        assert_eq!(
            interpret_token("::str/foo", &opts).map(|v| clj_value::java_debug(&v)),
            Some(":clojure.string/foo".to_owned())
        );
    }
}
