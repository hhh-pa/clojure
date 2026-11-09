// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Issue #32 regex natives: `re-pattern` compiles (or passes a
//! `Pattern` through); `re-find` / `re-matches` / `re-seq` match
//! over the `Value::Regex` source with JVM result shapes (a bare
//! string vs a group vector, nil holes, `re-seq` nil-on-no-match).
//! Oracle: the alpha10 `clojure.core` sources (`re-matcher` casts,
//! `re-groups` shapes); the `regex` crate rejects look-around at
//! compile time (a documented divergence surfacing as
//! `PatternSyntaxException`, like the JVM) — except in `split`,
//! which evaluates pure-boundary patterns (top-level `|` of
//! single-atom look-around runs, #205 natural-sort) with a
//! boundary scanner.

use std::sync::Arc;

use clj_value::{Array, ArrayDesc, ArrayStore, BaseType, SeqKind, ThrowClass, Value, Vector};
use regex::Regex;

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::lazy::realized_value;
use crate::ns::{NativeFnPtr, check_arity};

/// The §N regex natives installed into `clojure.core` by
/// [`boot`](crate::ns::boot).
pub(crate) const RE_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("re-pattern", native_re_pattern),
    ("re-find", native_re_find),
    ("re-matches", native_re_matches),
    ("re-seq", native_re_seq),
];

/// A `regex`-crate compile failure as the JVM's
/// `PatternSyntaxException` (the message is the crate report —
/// the JVM's own text is engine-specific).
fn pattern_syntax(err: regex::Error) -> NativeError {
    NativeError::new(ThrowClass::PatternSyntaxException, err.to_string())
}

/// Compile a pattern source (a stored `Pattern` source is always
/// compilable — only the reader mints `Value::Regex` — but a
/// stale value still reports `PatternSyntaxException`).
/// `\Q...\E` quotations translate first (the crate has no
/// quotation syntax — `Pattern/quote` output must compile).
pub(crate) fn compile_source(source: &str) -> Result<Arc<Regex>, NativeError> {
    // Memoized per thread: a compile costs far more than the matches
    // one call runs (a `split` per input line recompiled its literal
    // every line). Handed out shared, never cloned: each `Regex`
    // clone starts an empty search-cache pool, so a clone per call
    // rebuilt the lazy DFA cache on every match. A full table
    // clears, bounding programs that mint patterns. Failures are not
    // cached (each use reports its own syntax error).
    thread_local! {
        static COMPILED: std::cell::RefCell<crate::env::FxHashMap<String, Arc<Regex>>> =
            std::cell::RefCell::new(crate::env::FxHashMap::default());
    }
    if let Some(re) = COMPILED.with(|c| c.borrow().get(source).cloned()) {
        return Ok(re);
    }
    let re = Arc::new(Regex::new(&translate_quotations(source)).map_err(pattern_syntax)?);
    COMPILED.with(|c| {
        let mut c = c.borrow_mut();
        if c.len() >= 256 {
            c.clear();
        }
        c.insert(source.to_owned(), Arc::clone(&re));
    });
    Ok(re)
}

/// Translate `\Q...\E` quotation spans to escaped literals
/// (#205): the first `\E` closes (no nesting, like the JDK —
/// `quote` relies on it); an unclosed `\Q` quotes to the end;
/// a `\Q` behind an odd backslash run stays literal. A lone
/// `\E` passes through to the engine (which rejects it, like
/// the JDK).
pub(crate) fn translate_quotations(source: &str) -> std::borrow::Cow<'_, str> {
    if !source.contains("\\Q") {
        return std::borrow::Cow::Borrowed(source);
    }
    let bytes = source.as_bytes();
    let mut out = String::with_capacity(source.len());
    let mut i = 0;
    let mut seg = 0;
    while i < bytes.len() {
        let opens = bytes[i] == b'\\'
            && i + 1 < bytes.len()
            && bytes[i + 1] == b'Q'
            && backslashes_before(bytes, i) % 2 == 0;
        if !opens {
            i += 1;
            continue;
        }
        out.push_str(&source[seg..i]);
        let lit = i + 2;
        match source[lit..].find("\\E") {
            Some(j) => {
                out.push_str(&regex::escape(&source[lit..lit + j]));
                i = lit + j + 2;
                seg = i;
            }
            None => {
                out.push_str(&regex::escape(&source[lit..]));
                seg = source.len();
                break;
            }
        }
    }
    out.push_str(&source[seg..]);
    std::borrow::Cow::Owned(out)
}

/// The backslash run immediately before `i` (quotation
/// openers behind an odd run are escaped literals).
fn backslashes_before(bytes: &[u8], i: usize) -> usize {
    let mut n = 0;
    let mut k = i;
    while k > 0 && bytes[k - 1] == b'\\' {
        n += 1;
        k -= 1;
    }
    n
}

/// A `re-find`/`re-matches`/`re-seq` pattern: a stored
/// `Pattern` only (a `String` is NOT coerced — the JVM casts,
/// oracle `refind-strpat`).
fn pattern_source(v: &Value) -> Result<&str, NativeError> {
    match v {
        Value::Regex(source) => Ok(source),
        other => Err(cce_err(other, "java.util.regex.Pattern")),
    }
}

/// A `re-find`/`re-matches`/`re-seq` subject: `String` text (nil
/// trips the `Matcher.find` NPE, anything else the
/// `CharSequence` cast — both oracle-verbatim).
fn match_text(v: &Value) -> Result<&str, NativeError> {
    match v {
        Value::Str(s) => Ok(s),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.CharSequence.length()\" because \"this.text\" is null"
                .to_owned(),
        )),
        other => Err(cce_err(other, "java.lang.CharSequence")),
    }
}

/// One `re-groups`: the whole match when the pattern holds no
/// groups, else the `[full g1 ...]` vector (nil for unmatched
/// groups, like `Matcher.group` returning null).
fn groups_value(caps: &regex::Captures<'_>) -> Value {
    if caps.len() == 1 {
        let text = caps.get(0).map_or("", |m| m.as_str());
        return Value::Str(Arc::from(text));
    }
    let mut items = Vec::with_capacity(caps.len());
    for i in 0..caps.len() {
        match caps.get(i) {
            Some(m) => items.push(Value::Str(Arc::from(m.as_str()))),
            None => items.push(Value::Nil),
        }
    }
    Value::Vector(Vector::from_vec(items))
}

/// Split `text` around matches of the `source` pattern (the
/// JDK `Pattern.java#split` loop over the crate's `find_iter`,
/// whose empty-match advance matches `Matcher.find`): a
/// zero-width match at the start yields no empty leader, no
/// match at all yields the whole input, a positive `limit` caps
/// the parts (the last keeps the remainder), `0` drops trailing
/// empties, negative keeps everything. Bad patterns report
/// `PatternSyntaxException` (the crate text, like `re-pattern`).
/// The running state of the JDK split loop (split out so each
/// half stays under the complexity ratchet).
fn cached_num_str(s: &str) -> Option<Value> {
    let bytes = s.as_bytes();
    if bytes.len() >= 2 && bytes.len() <= 4 && bytes.iter().all(|b| b.is_ascii_digit()) {
        static NUM_CACHE: std::sync::OnceLock<Vec<Arc<str>>> = std::sync::OnceLock::new();
        let cache = NUM_CACHE.get_or_init(|| {
            (0..=1024)
                .map(|n| Arc::from(n.to_string().as_str()))
                .collect()
        });
        let n: usize = s.parse().ok()?;
        return cache.get(n).map(|arc| Value::Str(Arc::clone(arc)));
    }
    None
}

/// A split piece as a string value. Single-byte pieces (always
/// ASCII) share one cached allocation, mirroring CPython's
/// single-char string cache: token-heavy splits over short tokens
/// then allocate nothing per piece. Small numeric tokens share
/// static interned string allocations.
fn piece_value(s: &str) -> Value {
    if s.len() == 1 {
        static CACHE: std::sync::OnceLock<Vec<Arc<str>>> = std::sync::OnceLock::new();
        let cache = CACHE.get_or_init(|| {
            (0u8..128)
                .map(|b| Arc::from(char::from(b).to_string().as_str()))
                .collect()
        });
        return Value::Str(cache[s.as_bytes()[0] as usize].clone());
    }
    if let Some(v) = cached_num_str(s) {
        return v;
    }
    Value::Str(Arc::from(s))
}

struct SplitAcc {
    parts: Vec<Value>,
    index: usize,
    count: i64,
}

/// One split-loop step over a match `(start, end)`: matches
/// before the cap push the text before them (a zero-width match
/// at the start yields no empty leader); once a positive limit
/// is reached, the next match pushes the remainder instead and
/// later matches are dead (the `count == cap` arm fires exactly
/// once).
fn split_step(
    acc: &mut SplitAcc,
    text: &str,
    m_start: usize,
    m_end: usize,
    limited: bool,
    cap: i64,
) {
    if !limited || acc.count < cap {
        let leader = acc.index == 0 && m_start == 0 && m_start == m_end;
        if !leader {
            acc.parts.push(piece_value(&text[acc.index..m_start]));
            acc.index = m_end;
            acc.count += 1;
        }
    } else if acc.count == cap {
        acc.parts.push(piece_value(&text[acc.index..]));
        acc.index = m_end;
        acc.count += 1;
    }
}

pub(crate) fn pattern_split(text: &str, source: &str, limit: i32) -> Result<Value, NativeError> {
    let pieces = pattern_pieces(text, source, limit)?;
    Ok(Value::Vector(Vector::from_vec(pieces)))
}

/// JVM `String.split`/`Pattern.split`: the pieces as a `String[]`
/// (an `Object`-store array narrowed to `java.lang.String`, so
/// the class reads `[Ljava.lang.String;` and stores check).
pub(crate) fn pattern_split_array(
    text: &str,
    source: &str,
    limit: i32,
) -> Result<Value, NativeError> {
    let store = ArrayStore::Objs(pattern_pieces(text, source, limit)?);
    Ok(Value::Array(Array::new(
        ArrayDesc {
            base: BaseType::Object,
            dims: 1,
            component: Some(Arc::from("java.lang.String")),
        },
        store,
    )))
}

const WS_TR: u64 = (1 << b' ') | (1 << b'\t') | (1 << b'\n') | (1 << b'\r');
const WS_TRF: u64 = WS_TR | (1 << 0x0C);
const WS_ASCII: u64 = WS_TRF | (1 << 0x0B);

/// The hand-scanned split patterns (#226): the whitespace classes,
/// `\r?\n`, and `\|` the snippet suite splits input on. Each finds
/// exactly the crate's leftmost-first `find_iter` ranges, without
/// compiling (and faulting in) the regex engine.
#[derive(Clone, Copy)]
enum CutKind {
    /// A maximal run of bytes in an ASCII control/space mask.
    Run(u64),
    /// `\r?\n`.
    Newline,
    /// One literal byte.
    Byte(u8),
}

fn cut_kind(text: &str, source: &str) -> Option<CutKind> {
    Some(match source {
        // `\s` is Unicode in the crate: only all-ASCII text reduces
        // to the ASCII whitespace class.
        r"\s+" if text.is_ascii() => CutKind::Run(WS_ASCII),
        r"[ \t\n\r\f\x0B]+" => CutKind::Run(WS_ASCII),
        r"[ \t\n\r\f]+" => CutKind::Run(WS_TRF),
        r"[ \t\n\r]+" => CutKind::Run(WS_TR),
        r"[\x00-\x20]+" => CutKind::Run((1u64 << 0x21) - 1),
        r"\r?\n" => CutKind::Newline,
        r"\|" => CutKind::Byte(b'|'),
        _ => return None,
    })
}

fn in_mask(b: u8, mask: u64) -> bool {
    b <= 0x20 && mask & (1u64 << b) != 0
}

/// The match ranges of a [`CutKind`], streamed (no cut buffer).
struct ByteCuts<'a> {
    bytes: &'a [u8],
    pos: usize,
    kind: CutKind,
}

impl ByteCuts<'_> {
    /// The end of a match starting at `i`, if one does.
    fn match_end(&self, i: usize) -> Option<usize> {
        let b = self.bytes[i];
        match self.kind {
            CutKind::Run(mask) if in_mask(b, mask) => Some(
                i + 1
                    + self.bytes[i + 1..]
                        .iter()
                        .take_while(|&&c| in_mask(c, mask))
                        .count(),
            ),
            CutKind::Newline if b == b'\n' => Some(i + 1),
            CutKind::Newline if b == b'\r' && self.bytes.get(i + 1) == Some(&b'\n') => Some(i + 2),
            CutKind::Byte(c) if b == c => Some(i + 1),
            _ => None,
        }
    }
}

impl Iterator for ByteCuts<'_> {
    type Item = (usize, usize);

    fn next(&mut self) -> Option<(usize, usize)> {
        while self.pos < self.bytes.len() {
            let start = self.pos;
            if let Some(end) = self.match_end(start) {
                self.pos = end;
                return Some((start, end));
            }
            self.pos += 1;
        }
        None
    }
}

/// The split pieces (engine-agnostic — the `regex` crate and the
/// boundary scanner share the JDK loop in [`split_pieces`]).
fn pattern_pieces(text: &str, source: &str, limit: i32) -> Result<Vec<Value>, NativeError> {
    if let Some(kind) = cut_kind(text, source) {
        let cuts = ByteCuts {
            bytes: text.as_bytes(),
            pos: 0,
            kind,
        };
        return Ok(split_pieces(text, cuts, limit));
    }
    // Pure-boundary patterns never reach the crate (which cannot
    // compile look-around); anything else splits as before.
    if let Some(alts) = boundary_alternatives(source) {
        let cuts = boundary_cuts(text, &alts)?;
        return Ok(split_pieces(text, cuts, limit));
    }
    let re = compile_source(source)?;
    Ok(split_pieces(
        text,
        re.find_iter(text).map(|m| (m.start(), m.end())),
        limit,
    ))
}

/// Run the JDK split loop over byte ranges, collecting the piece
/// strings (callers wrap them as a vector or a `String[]`).
fn split_pieces(
    text: &str,
    cuts: impl IntoIterator<Item = (usize, usize)>,
    limit: i32,
) -> Vec<Value> {
    let limited = limit > 0;
    let cap = i64::from(limit) - 1;
    let mut acc = SplitAcc {
        parts: Vec::new(),
        index: 0,
        count: 0,
    };
    for (start, end) in cuts {
        split_step(&mut acc, text, start, end, limited, cap);
    }
    if acc.index == 0 {
        return vec![Value::Str(Arc::from(text))];
    }
    if !limited || acc.count < i64::from(limit) {
        acc.parts.push(piece_value(&text[acc.index..]));
    }
    if limit == 0 {
        while acc
            .parts
            .last()
            .is_some_and(|p| matches!(p, Value::Str(s) if s.is_empty()))
        {
            acc.parts.pop();
        }
    }
    acc.parts
}

/// A single-char boundary atom, kept as source so atom tests run
/// through the `regex` crate itself (Perl-class semantics stay
/// identical to the rest of the port by construction).
#[derive(Clone, Debug, PartialEq, Eq)]
struct BoundaryAtom {
    src: String,
}

/// One zero-width assertion: the side it inspects plus polarity.
#[derive(Clone, Debug)]
struct BoundaryAssert {
    behind: bool,
    negated: bool,
    atom: BoundaryAtom,
}

/// One top-level alternative: a conjunction of assertions (all
/// must hold at the boundary).
type BoundaryAlt = Vec<BoundaryAssert>;

/// Parse a pure-boundary pattern (`None` when the pattern is not
/// one — the caller falls through to the `regex` crate, which
/// reports its own `PatternSyntaxException` for look-around it
/// cannot compile). Supported: top-level `|` of assertion runs
/// over single atoms (`\d\D\s\S\w\W`, `.`, literals, simple
/// escapes). Unsupported (honest fallthrough): character
/// classes, groups, anchors, flags, multi-char assertion bodies,
/// alternation inside assertions, quantified assertions.
fn boundary_alternatives(source: &str) -> Option<Vec<BoundaryAlt>> {
    if !source.contains("(?") {
        return None;
    }
    let bytes = source.as_bytes();
    let mut alts = Vec::new();
    let mut depth = 0usize;
    let mut start = 0usize;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'\\' => i = i.saturating_add(2).min(bytes.len()),
            b'(' => {
                depth += 1;
                i += 1;
            }
            b')' => {
                depth = depth.checked_sub(1)?;
                i += 1;
            }
            b'|' if depth == 0 => {
                alts.push(&source[start..i]);
                start = i + 1;
                i += 1;
            }
            _ => i += 1,
        }
    }
    if depth != 0 {
        return None;
    }
    alts.push(&source[start..]);
    alts.into_iter().map(boundary_alt).collect()
}

/// Parse one alternative: a non-empty run of assertions.
fn boundary_alt(alt: &str) -> Option<BoundaryAlt> {
    if alt.is_empty() {
        return None;
    }
    let mut asserts = Vec::new();
    let mut rest = alt;
    while !rest.is_empty() {
        let (assert, tail) = boundary_assert(rest)?;
        asserts.push(assert);
        rest = tail;
    }
    Some(asserts)
}

/// Parse one assertion (`(?<=A)`/`(?<!A)`/`(?=A)`/`(?!A)` with a
/// single atom `A`), answering the rest past `)` (`None` for
/// anything else, including quantified assertions).
fn boundary_assert(s: &str) -> Option<(BoundaryAssert, &str)> {
    // Peel `(?`, the optional `<`, then `=`/`!` (openers run
    // three to four bytes, so lengths stay derived).
    let rest = s.strip_prefix("(?")?;
    let (behind, rest) = match rest.strip_prefix('<') {
        Some(rest) => (true, rest),
        None => (false, rest),
    };
    let (negated, after) = match rest.chars().next() {
        Some('=') => (false, &rest[1..]),
        Some('!') => (true, &rest[1..]),
        _ => return None,
    };
    let (atom, after) = boundary_atom(after)?;
    let after = after.strip_prefix(')')?;
    if let Some(c) = after.chars().next() {
        if matches!(c, '*' | '+' | '?' | '{') {
            return None;
        }
    }
    Some((
        BoundaryAssert {
            behind,
            negated,
            atom,
        },
        after,
    ))
}

/// Parse one atom: a Perl class, `.`, a simple escape, or a bare
/// literal (structural chars decline — classes, groups, and
/// alternation inside assertions stay unsupported).
fn boundary_atom(s: &str) -> Option<(BoundaryAtom, &str)> {
    let mut chars = s.chars();
    let c = chars.next()?;
    if c == '\\' {
        let e = chars.next()?;
        let src = match e {
            'd' | 'D' | 's' | 'S' | 'w' | 'W' => format!("\\{e}"),
            'n' => "\n".to_owned(),
            't' => "\t".to_owned(),
            'r' => "\r".to_owned(),
            'f' => "\u{C}".to_owned(),
            '0' => "\0".to_owned(),
            '\\' | '.' | '|' | '(' | ')' | '[' | ']' | '*' | '+' | '?' | '{' | '}' | '^' | '$' => {
                regex::escape(&e.to_string())
            }
            _ => return None,
        };
        return Some((BoundaryAtom { src }, &s[c.len_utf8() + e.len_utf8()..]));
    }
    if c == '.' {
        return Some((
            BoundaryAtom {
                src: ".".to_owned(),
            },
            &s[1..],
        ));
    }
    if matches!(
        c,
        '(' | ')' | '[' | ']' | '|' | '*' | '+' | '?' | '{' | '}' | '^' | '$'
    ) {
        return None;
    }
    let end = c.len_utf8();
    Some((
        BoundaryAtom {
            src: regex::escape(&s[..end]),
        },
        &s[end..],
    ))
}

/// Evaluate boundary alternatives over `text`, answering the
/// matching (empty) byte ranges in order. Each distinct atom
/// compiles once (`^(?:atom)$`); a boundary matches when some
/// alternative's every assertion holds (absent sides fail
/// positives and pass negatives, like the JDK).
fn boundary_cuts(text: &str, alts: &[BoundaryAlt]) -> Result<Vec<(usize, usize)>, NativeError> {
    let atoms = compile_boundary_atoms(alts)?;
    let chars: Vec<char> = text.chars().collect();
    let bytes: Vec<usize> = text.char_indices().map(|(b, _)| b).collect();
    let mut cuts = Vec::new();
    for i in 0..=chars.len() {
        let left = if i > 0 { Some(chars[i - 1]) } else { None };
        let right = chars.get(i).copied();
        if boundary_matched(&atoms, alts, left, right) {
            let b = bytes.get(i).copied().unwrap_or(text.len());
            cuts.push((b, b));
        }
    }
    Ok(cuts)
}

/// Compile each distinct boundary atom once (split from
/// [`boundary_cuts`] for the complexity budget).
fn compile_boundary_atoms(alts: &[BoundaryAlt]) -> Result<Vec<(String, Regex)>, NativeError> {
    let mut atoms: Vec<(String, Regex)> = Vec::new();
    for alt in alts {
        for a in alt {
            if !atoms.iter().any(|(s, _)| s == &a.atom.src) {
                let re = Regex::new(&format!("^(?:{})$", a.atom.src)).map_err(pattern_syntax)?;
                atoms.push((a.atom.src.clone(), re));
            }
        }
    }
    Ok(atoms)
}

/// Test one char against a compiled atom (split from
/// [`boundary_cuts`] for the complexity budget).
fn boundary_atom_test(atoms: &[(String, Regex)], src: &str, c: char) -> bool {
    let re = atoms
        .iter()
        .find(|(s, _)| s == src)
        .map(|(_, r)| r)
        .expect("atom compiled above");
    let mut buf = [0u8; 4];
    re.is_match(c.encode_utf8(&mut buf))
}

/// Whether some alternative's every assertion holds at a
/// boundary (split from [`boundary_cuts`] for the complexity
/// budget).
fn boundary_matched(
    atoms: &[(String, Regex)],
    alts: &[BoundaryAlt],
    left: Option<char>,
    right: Option<char>,
) -> bool {
    alts.iter()
        .any(|alt| boundary_alt_holds(atoms, alt, left, right))
}

/// Whether one alternative's every assertion holds (split from
/// [`boundary_cuts`] for the complexity budget).
fn boundary_alt_holds(
    atoms: &[(String, Regex)],
    alt: &BoundaryAlt,
    left: Option<char>,
    right: Option<char>,
) -> bool {
    alt.iter()
        .all(|a| boundary_assert_holds(atoms, a, left, right))
}

/// Whether one assertion holds (absent sides fail positives
/// and pass negatives, like the JDK — split from
/// [`boundary_cuts`] for the complexity budget).
fn boundary_assert_holds(
    atoms: &[(String, Regex)],
    a: &BoundaryAssert,
    left: Option<char>,
    right: Option<char>,
) -> bool {
    let c = if a.behind { left } else { right };
    match c {
        None => a.negated,
        Some(c) => boundary_atom_test(atoms, &a.atom.src, c) != a.negated,
    }
}

/// `re-pattern`: a `Pattern` passes through, a `String` compiles;
/// anything else fails the `String` cast (oracle `repat-long`:
/// the JVM compiles the `String` argument, so a `Long` fails
/// against `String`, not `Pattern`).
fn native_re_pattern(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-pattern", args, &[1])?;
    match &args[0] {
        Value::Regex(_) => Ok(args[0].clone()),
        Value::Str(s) => {
            compile_source(s)?;
            Ok(Value::Regex(Arc::clone(s)))
        }
        other => Err(cce_err(other, "java.lang.String")),
    }
}

/// `re-find`: the next match (`re-groups` shape, nil on no
/// match). The 1-arity takes a `Matcher`: an S12 handle advances
/// (owner-ACKED companion arm, issue #54); every other argument
/// fails the `Matcher` cast exactly like the JVM (oracles
/// `refind-arity`, `re-r2b`).
fn native_re_find(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-find", args, &[1, 2])?;
    if args.len() == 1 {
        if let Some(found) = crate::collmisc::match_handle_find(&args[0]) {
            return found;
        }
        return Err(cce_err(&args[0], "java.util.regex.Matcher"));
    }
    let re = compile_source(pattern_source(&args[0])?)?;
    let text = match_text(&args[1])?;
    Ok(match re.captures(text) {
        Some(caps) => groups_value(&caps),
        None => Value::Nil,
    })
}

/// The `re-matches` full-match anchor: the source wrapped so a
/// top-level `|` cannot escape the match (`\A(?:src)\z`, per the
/// slice contract).
fn anchored(source: &str) -> String {
    format!("\\A(?:{source})\\z")
}

pub(crate) fn fast_re_matches(source: &str, text: &str) -> Option<bool> {
    match source {
        "[+-]?[0-9]+" => Some(match_int_digits(text.as_bytes(), true, usize::MAX)),
        "-?[0-9]+" => Some(match_int_digits(text.as_bytes(), false, usize::MAX)),
        "[+-]?[0-9]{1,18}" => Some(match_int_digits(text.as_bytes(), true, 18)),
        _ => None,
    }
}

fn match_int_digits(bytes: &[u8], allow_plus: bool, max_digits: usize) -> bool {
    let rest = match bytes.first() {
        Some(b'-') => &bytes[1..],
        Some(b'+') if allow_plus => &bytes[1..],
        _ => bytes,
    };
    !rest.is_empty() && rest.len() <= max_digits && rest.iter().all(u8::is_ascii_digit)
}

pub(crate) fn re_matches_2(pat: &Value, subj: &Value) -> Result<Value, NativeError> {
    let source = pattern_source(pat)?;
    let text = match_text(subj)?;
    if let Some(matched) = fast_re_matches(source, text) {
        return Ok(if matched { subj.clone() } else { Value::Nil });
    }
    let re = compile_source(&anchored(source))?;
    Ok(match re.captures(text) {
        Some(caps) => groups_value(&caps),
        None => Value::Nil,
    })
}

/// `re-matches`: the whole string must match (`re-groups`
/// shape, nil otherwise).
fn native_re_matches(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-matches", args, &[2])?;
    re_matches_2(&args[0], &args[1])
}

/// `re-seq`: successive `find` results (`re-groups` each — a
/// `Cons` like the JVM's, nil when nothing matches at all).
fn native_re_seq(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/re-seq", args, &[2])?;
    let re = compile_source(pattern_source(&args[0])?)?;
    let text = match_text(&args[1])?;
    let items: Vec<Value> = re
        .captures_iter(text)
        .map(|caps| groups_value(&caps))
        .collect();
    if items.is_empty() {
        Ok(Value::Nil)
    } else {
        Ok(realized_value(items, false, false, false, SeqKind::Cons))
    }
}
