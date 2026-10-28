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
use crate::hooks::{EvalReadError, ReadHooks, ResolverOp};
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
/// (numbers never come through here); `Ok(None)` means `Invalid
/// token`, `Err` a failed resolver consult.
pub fn interpret_token(
    s: &str,
    opts: &ReaderOpts,
    hook: &mut dyn ReadHooks,
) -> Result<Option<Value>, EvalReadError> {
    match s {
        "nil" => Ok(Some(Value::Nil)),
        "true" => Ok(Some(Value::Bool(true))),
        "false" => Ok(Some(Value::Bool(false))),
        _ => match_symbol(s, opts, hook),
    }
}

/// `matchSymbol`: `Ok(None)` means `Invalid token`. A bound
/// `*reader-resolver*` answers `::` auto-resolution instead of the
/// maps (the answer's NAME becomes the keyword ns — probes
/// `f-alias`, `f-current`, `f-alias-2part`, `f-nilalias`).
pub fn match_symbol(
    s: &str,
    opts: &ReaderOpts,
    hook: &mut dyn ReadHooks,
) -> Result<Option<Value>, EvalReadError> {
    if let Some((ns, name)) = match_symbol_pat(s) {
        // Validation first: a match that fails here returns null
        // WITHOUT trying `arraySymbolPat`.
        if ns.is_some_and(|ns| ns.ends_with(":/"))
            || name.ends_with(':')
            || after_first_char(s).contains("::")
        {
            return Ok(None);
        }
        if let Some(rest) = s.strip_prefix("::") {
            return match_auto_keyword(rest, opts, hook);
        }
        let is_keyword = s.starts_with(':');
        let sym = Symbol::intern(if is_keyword { &s[1..] } else { s });
        if is_keyword {
            Ok(Some(Value::Keyword(Keyword::new(
                sym.namespace(),
                sym.name(),
            ))))
        } else {
            Ok(Some(Value::Symbol(sym)))
        }
    } else {
        match match_array_symbol_pat(s) {
            Some((ns, name)) => Ok(Some(Value::Symbol(Symbol::new(Some(&ns), &name)))),
            None => Ok(None),
        }
    }
}

/// `::` auto-resolution (probes `f-*`): a bound `*reader-resolver*`
/// answers instead of the maps (see [`resolve_auto`]).
fn match_auto_keyword(
    rest: &str,
    opts: &ReaderOpts,
    hook: &mut dyn ReadHooks,
) -> Result<Option<Value>, EvalReadError> {
    let ks = Symbol::intern(rest);
    if opts.resolver_bound {
        return resolve_auto(&ks, hook);
    }
    let resolved = match ks.namespace() {
        // A qualified name looks its head up as an ALIAS.
        Some(alias) => opts.aliases.get(alias).cloned(),
        None => Some(opts.current_ns.clone()),
    };
    Ok(resolved.map(|ns| Value::Keyword(Keyword::new(Some(&ns), ks.name()))))
}

/// `::` auto-resolution through a bound `*reader-resolver*`:
/// qualified heads consult `resolveAlias`, bare names consult
/// `currentNS`; either way the answer's NAME becomes the keyword
/// namespace and nil means `Invalid token`.
fn resolve_auto(ks: &Symbol, hook: &mut dyn ReadHooks) -> Result<Option<Value>, EvalReadError> {
    let resolved = match ks.namespace() {
        Some(alias) => {
            let head = Symbol::new(None, alias);
            hook.resolve(ResolverOp::ResolveAlias, Some(&head))?
        }
        None => hook.resolve(ResolverOp::CurrentNs, None)?,
    };
    Ok(resolved.map(|r| Value::Keyword(Keyword::new(Some(r.name()), ks.name()))))
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
    use super::{after_first_char, interpret_token, is_whitespace};
    use crate::{NoHooks, ReaderOpts};
    use clj_value::Value;

    #[test]
    fn after_first_char_handles_empty() {
        assert_eq!(after_first_char(""), "");
        assert_eq!(after_first_char("ab"), "b");
    }

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
        assert_eq!(
            interpret_token("nil", &opts, &mut NoHooks),
            Ok(Some(Value::Nil))
        );
        assert_eq!(
            interpret_token("true", &opts, &mut NoHooks),
            Ok(Some(Value::Bool(true)))
        );
        assert_eq!(
            interpret_token("false", &opts, &mut NoHooks),
            Ok(Some(Value::Bool(false)))
        );
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
            let got = interpret_token(input, &opts, &mut NoHooks).unwrap();
            assert_eq!(
                got.map(|v| clj_value::java_debug(&v)),
                Some(want.to_owned()),
                "{input}"
            );
        }
        // Digit-led names are legal keywords.
        assert_eq!(
            interpret_token(":1", &opts, &mut NoHooks)
                .unwrap()
                .map(|v| clj_value::java_debug(&v)),
            Some(":1".to_owned())
        );
        // Array symbols.
        assert_eq!(
            interpret_token("a/1", &opts, &mut NoHooks)
                .unwrap()
                .map(|v| clj_value::java_debug(&v)),
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
            assert_eq!(
                interpret_token(input, &opts, &mut NoHooks),
                Ok(None),
                "{input}"
            );
        }
        // Unknown alias, and a qualified name looked up as an alias.
        assert_eq!(interpret_token("::nope/x", &opts, &mut NoHooks), Ok(None));
        assert_eq!(
            interpret_token("::clojure.string/foo", &opts, &mut NoHooks),
            Ok(None)
        );
    }

    #[test]
    fn auto_resolve() {
        let mut opts = ReaderOpts::default();
        opts.aliases
            .insert("str".to_owned(), "clojure.string".to_owned());
        assert_eq!(
            interpret_token("::foo", &opts, &mut NoHooks)
                .unwrap()
                .map(|v| clj_value::java_debug(&v)),
            Some(":user/foo".to_owned())
        );
        assert_eq!(
            interpret_token("::str/foo", &opts, &mut NoHooks)
                .unwrap()
                .map(|v| clj_value::java_debug(&v)),
            Some(":clojure.string/foo".to_owned())
        );
    }

    #[test]
    fn auto_resolve_through_the_hook() {
        // S7 `*reader-resolver*` (probes `f-alias`, `f-current`,
        // `f-alias-2part`, `f-nilalias`, `f-current-nil`): qualified
        // heads consult `resolveAlias`, bare names `currentNS`; the
        // answer's NAME becomes the keyword ns; nil is invalid.
        use crate::hooks::{ResolverOp, ScriptedHooks};
        use clj_value::Symbol;

        let opts = ReaderOpts {
            resolver_bound: true,
            ..ReaderOpts::default()
        };
        let aliased = Symbol::new(Some("A"), "B");
        let mut hooks =
            ScriptedHooks::new(Err(crate::EvalReadError::NoHook), vec![Ok(Some(aliased))]);
        let got = interpret_token("::str/foo", &opts, &mut hooks).unwrap();
        assert_eq!(
            got.map(|v| clj_value::java_debug(&v)),
            Some(":B/foo".to_owned())
        );
        assert_eq!(
            hooks.calls,
            vec![(ResolverOp::ResolveAlias, Some(Symbol::new(None, "str")))]
        );
        let current = Symbol::new(None, "cur-ns");
        let mut hooks =
            ScriptedHooks::new(Err(crate::EvalReadError::NoHook), vec![Ok(Some(current))]);
        let got = interpret_token("::foo", &opts, &mut hooks).unwrap();
        assert_eq!(
            got.map(|v| clj_value::java_debug(&v)),
            Some(":cur-ns/foo".to_owned())
        );
        assert_eq!(hooks.calls, vec![(ResolverOp::CurrentNs, None)]);
        // Nil answers are invalid tokens.
        let mut hooks = ScriptedHooks::new(Err(crate::EvalReadError::NoHook), vec![Ok(None)]);
        assert_eq!(
            interpret_token("::str/foo", &opts, &mut hooks).unwrap(),
            None
        );
        let mut hooks = ScriptedHooks::new(Err(crate::EvalReadError::NoHook), vec![Ok(None)]);
        assert_eq!(interpret_token("::foo", &opts, &mut hooks).unwrap(), None);
        // Hook failures propagate.
        let mut hooks = ScriptedHooks::new(
            Err(crate::EvalReadError::NoHook),
            vec![Err(crate::EvalReadError::NoHook)],
        );
        assert_eq!(
            interpret_token("::foo", &opts, &mut hooks).unwrap_err(),
            crate::EvalReadError::NoHook
        );
    }
}
