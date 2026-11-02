// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Emulated construction (#120 slice 2): `(new Class …)`,
//! `(Class. …)`, and `(Class/new …)` over the emulated lattice.
//! The allow-list is classes with exact port values —
//! `java.lang.String` (the full port-representable overload
//! matrix) and `java.lang.Object` (0-arg) — plus the shared miss
//! shapes (`No matching ctor`, nil ambiguity, bounds, charset
//! lookup) every construction spelling funnels through. Port
//! types keep their own path in [`crate::types`]; non-allow-listed
//! lattice classes miss uniformly (spec N4).

use std::sync::Arc;

use clj_value::{ArrayStore, ThrowClass, Throwable, Value};

use crate::core::int_exact_long;
use crate::error::NativeError;
use crate::eval::Evaluator;

/// The emulated ctor arity table (parallel to
/// [`crate::classemu::static_member_arities`]): the analyzer
/// arity-checks allow-listed classes at compile time (oracles
/// `ctor-str-arity5`, `ctor-obj-arity`, `cn-arity5`,
/// `m-obj-new-arity`, `s-new-arity5`); every other class resolves
/// at runtime. Wire new emulated classes here plus an arm in the
/// [`crate::types`] dispatch.
pub(crate) fn ctor_arities(fqn: &str) -> Option<&'static [usize]> {
    match fqn {
        "java.lang.String" => Some(&[0, 1, 2, 3, 4]),
        "java.lang.Object" => Some(&[0]),
        "java.lang.StringBuilder" => Some(&[0, 1]),
        "java.lang.StringBuffer" => Some(&[0, 1]),
        "java.lang.IllegalArgumentException" => Some(&[0, 1, 2]),
        _ => None,
    }
}

/// The oracle's missing-ctor error: interfaces read `for
/// interface` (oracle `ctor-iface`), everything else `for class`
/// (oracles `ctor-abstract`, `ctor-private`, `ctor-str-arity2`).
pub(crate) fn no_ctor(fqn: &str, is_iface: bool) -> NativeError {
    let kind = if is_iface { "interface" } else { "class" };
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("No matching ctor found for {kind} {fqn}"),
    )
}

/// The nil-overload ambiguity error, verbatim (the JVM says
/// `method` for ctors too — oracles `ctor-str-nil1/2/3`,
/// `d-bytes-nil`, `cs-nil-name`, `cn-nil1`, `s-new-nil`).
fn ambiguity(fqn: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("More than one matching method found: {fqn}"),
    )
}

/// A fresh heap string (never the argument `Arc`: every
/// construction is a new object — oracles `ctor-str-ident`,
/// `fr-e`, `fr-empty`, `fr-chararr`).
fn fresh_str(s: &str) -> Value {
    Value::Str(Arc::from(s))
}

/// A fresh heap string from an owned buffer (same rule).
fn fresh_string(s: String) -> Value {
    Value::Str(Arc::from(s))
}

/// Build `(String. …)`: dispatch on arity (surface callers gate
/// arities at compile time, so 5+ arrives only through direct
/// `__new*` calls and misses like any unresolvable shape).
pub(crate) fn build_string(args: &[Value]) -> Result<Value, NativeError> {
    match args.len() {
        0 => Ok(fresh_str("")),
        1 => string_one(&args[0]),
        2 => string_two(&args[0], &args[1]),
        3 => string_three(&args[0], &args[1], &args[2]),
        4 => string_four(&args[0], &args[1], &args[2], &args[3]),
        _ => Err(no_ctor("java.lang.String", false)),
    }
}

/// Build `(Object. …)`: 0-arg mints a fresh object per call;
/// anything else misses (surface callers gate the arity at
/// compile time — oracles `ctor-obj-arity`, `new-obj-arity`).
pub(crate) fn build_object(args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        Ok(Value::Extension(Arc::new(
            crate::fx4_host::Fx4Object::fresh(),
        )))
    } else {
        Err(no_ctor("java.lang.Object", false))
    }
}

/// Build `(StringBuilder. …)` / `(StringBuffer. …)`: empty,
/// copy (strings and fellow builders — the `CharSequence`
/// overload), or capacity.
pub(crate) fn build_builder(args: &[Value], class: &'static str) -> Result<Value, NativeError> {
    match args.len() {
        0 => Ok(crate::hostvals::BuilderValue::with_text(
            class,
            String::new(),
        )),
        1 => builder_one(&args[0], class),
        _ => Err(no_ctor(class, false)),
    }
}

/// The 1-arg builder overloads: strings copy, nils take the
/// `String` overload and NPE in its body, longs size exactly
/// (negatives fail `NegativeArraySize`), fellow builders copy,
/// anything else misses (doubles and tower values never unbox
/// here — oracle `sb-bad`).
fn builder_one(v: &Value, class: &'static str) -> Result<Value, NativeError> {
    if let Some(b) = crate::hostvals::as_builder(v) {
        return Ok(crate::hostvals::BuilderValue::with_text(
            class,
            b.contents(),
        ));
    }
    match v {
        Value::Str(s) => Ok(crate::hostvals::BuilderValue::with_text(
            class,
            s.to_string(),
        )),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.length()\" because \"str\" is null".to_owned(),
        )),
        Value::Int(i) => {
            let capacity = crate::core::int_exact_long(*i)?;
            if capacity < 0 {
                return Err(NativeError::new(
                    ThrowClass::NegativeArraySizeException,
                    capacity.to_string(),
                ));
            }
            Ok(crate::hostvals::BuilderValue::with_text(
                class,
                String::new(),
            ))
        }
        _ => Err(no_ctor(class, false)),
    }
}

/// An `IllegalArgumentException` throwable value.
fn iae_value(eval: &mut Evaluator, message: Option<String>, cause: Option<Throwable>) -> Value {
    Value::Throwable(Throwable {
        id: eval.next_id(),
        class: ThrowClass::IllegalArgumentException,
        message: message.map(Arc::from),
        data: None,
        cause: cause.map(Box::new),
    })
}

/// Build `(IllegalArgumentException. …)`: blank, message,
/// cause, or both (a lone nil disambiguates nothing — the JVM
/// reports the overload ambiguity).
pub(crate) fn build_iae(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    match args.len() {
        0 => Ok(iae_value(eval, None, None)),
        1 => match &args[0] {
            Value::Nil => Err(ambiguity("java.lang.IllegalArgumentException")),
            Value::Str(s) => Ok(iae_value(eval, Some(s.to_string()), None)),
            Value::Throwable(t) => Ok(iae_value(eval, None, Some(t.clone()))),
            _ => Err(no_ctor("java.lang.IllegalArgumentException", false)),
        },
        2 => match (&args[0], &args[1]) {
            (Value::Str(s), Value::Throwable(t)) => {
                Ok(iae_value(eval, Some(s.to_string()), Some(t.clone())))
            }
            (Value::Str(s), Value::Nil) => Ok(iae_value(eval, Some(s.to_string()), None)),
            (Value::Nil, Value::Throwable(t)) => Ok(iae_value(eval, None, Some(t.clone()))),
            (Value::Nil, Value::Nil) => Ok(iae_value(eval, None, None)),
            _ => Err(no_ctor("java.lang.IllegalArgumentException", false)),
        },
        _ => Err(no_ctor("java.lang.IllegalArgumentException", false)),
    }
}

/// A ctor-relevant array's cloned slots (`None` for non-arrays
/// and non-primitive stores — multidimensional arrays ride the
/// `Objs` store, so they never match a primitive overload).
enum CtorArray {
    Bytes(Vec<i8>),
    Chars(Vec<char>),
    Ints(Vec<i32>),
}

/// Snapshot a ctor-relevant array under the cell lock (short
/// critical section — clone out, decode after).
fn ctor_array(v: &Value) -> Option<CtorArray> {
    let Value::Array(a) = v else {
        return None;
    };
    match &*a.lock() {
        ArrayStore::Bytes(v) => Some(CtorArray::Bytes(v.clone())),
        ArrayStore::Chars(v) => Some(CtorArray::Chars(v.clone())),
        ArrayStore::Ints(v) => Some(CtorArray::Ints(v.clone())),
        _ => None,
    }
}

/// Snapshot a `byte[]`'s unsigned slots (`None` for anything
/// else, including nil).
fn array_bytes(v: &Value) -> Option<Vec<u8>> {
    match ctor_array(v) {
        Some(CtorArray::Bytes(v)) => Some(v.iter().map(|b| *b as u8).collect()),
        _ => None,
    }
}

/// The 1-arg overloads: `(String)` copies, `(char[])` collects,
/// `(byte[])` decodes UTF-8; nil is ambiguous
/// (`String`/`char[]`/`byte[]`/`StringBuffer`/`StringBuilder`
/// all apply — oracle `ctor-str-nil1`); `int[]` and everything
/// else never match at arity 1 (oracle `n-intarr-1`).
fn string_one(a0: &Value) -> Result<Value, NativeError> {
    if let Value::Str(s) = a0 {
        return Ok(fresh_str(s));
    }
    if matches!(a0, Value::Nil) {
        return Err(ambiguity("java.lang.String"));
    }
    match ctor_array(a0) {
        Some(CtorArray::Chars(v)) => Ok(fresh_string(v.into_iter().collect())),
        Some(CtorArray::Bytes(v)) => {
            let bytes: Vec<u8> = v.iter().map(|b| *b as u8).collect();
            Ok(fresh_string(decode_utf8(&bytes)))
        }
        _ => Err(no_ctor("java.lang.String", false)),
    }
}

/// The 2-arg overloads: `(bytes,int)` hibyte, `(bytes,String)`
/// named decode; `(bytes,nil)` is ambiguous
/// (`(byte[],String)` vs `(byte[],Charset)` — oracle
/// `d-bytes-nil`); `(nil,int)` and `(nil,String)` match uniquely
/// and then NPE on the bytes (oracles `d-nil-0`, `d-nil-utf8`);
/// `(nil,nil)` is ambiguous (oracle `ctor-str-nil2`); anything
/// else misses (oracles `n-chars-int`, `n-nil-char`).
fn string_two(a0: &Value, a1: &Value) -> Result<Value, NativeError> {
    if let Some(bytes) = array_bytes(a0) {
        return string_two_bytes(&bytes, a1);
    }
    if !matches!(a0, Value::Nil) {
        return Err(no_ctor("java.lang.String", false));
    }
    if let Value::Int(i) = a1 {
        let _ = int_exact_long(*i)?;
        return Err(npe_bytes());
    }
    if matches!(a1, Value::Str(_)) {
        return Err(npe_bytes());
    }
    if matches!(a1, Value::Nil) {
        return Err(ambiguity("java.lang.String"));
    }
    Err(no_ctor("java.lang.String", false))
}

/// The 2-arg overloads over a non-nil `byte[]` (split for the
/// complexity budget).
fn string_two_bytes(bytes: &[u8], a1: &Value) -> Result<Value, NativeError> {
    if let Value::Int(i) = a1 {
        let hibyte = int_exact_long(*i)?;
        return Ok(fresh_string(hibyte_string(bytes, hibyte)));
    }
    if let Value::Str(name) = a1 {
        let cs = lookup_charset(name)?;
        return Ok(fresh_string(decode(cs, bytes)));
    }
    if matches!(a1, Value::Nil) {
        return Err(ambiguity("java.lang.String"));
    }
    Err(no_ctor("java.lang.String", false))
}

/// The 3-arg overloads: `(chars|bytes|ints,int,int)` (oracles
/// `ctor-str-chararr-off`, `ctor-str-bytearr-off`,
/// `ctor-str-intarr`); `(nil,int,int)` is ambiguous (all three
/// array types apply — oracles `ctor-str-nil3`,
/// `ctor-str-bytearr-nil`); a non-`Int` offset/count never
/// matches (oracles `d-chars-nil-1`, `n-3nil`).
fn string_three(a0: &Value, a1: &Value, a2: &Value) -> Result<Value, NativeError> {
    let (Value::Int(off), Value::Int(count)) = (a1, a2) else {
        return Err(no_ctor("java.lang.String", false));
    };
    if matches!(a0, Value::Nil) {
        return Err(ambiguity("java.lang.String"));
    }
    match ctor_array(a0) {
        Some(CtorArray::Chars(v)) => chars_range(&v, *off, *count),
        Some(CtorArray::Bytes(v)) => bytes_range(&v, *off, *count),
        Some(CtorArray::Ints(v)) => codepoints_range(&v, *off, *count),
        _ => Err(no_ctor("java.lang.String", false)),
    }
}

/// The 4-arg overload: `(bytes,int,int,String)` (oracle
/// `cs-4arg`); a nil charset name is ambiguous
/// (`(byte[],int,int,String)` vs `(byte[],int,int,Charset)` —
/// oracles `ctor-str-nil3` shape, `d-nil-0-0-nil`). Check order
/// is structural match, int coerce, charset lookup, null-bytes
/// NPE, bounds (oracles `e-badboth`, `e-nil-badcs`,
/// `e-nil-badbounds`).
fn string_four(a0: &Value, a1: &Value, a2: &Value, a3: &Value) -> Result<Value, NativeError> {
    let (Value::Int(off), Value::Int(count)) = (a1, a2) else {
        return Err(no_ctor("java.lang.String", false));
    };
    let bytes_nil = matches!(a0, Value::Nil);
    let bytes = if bytes_nil { None } else { array_bytes(a0) };
    if !bytes_nil && bytes.is_none() {
        return Err(no_ctor("java.lang.String", false));
    }
    let Value::Str(name) = a3 else {
        if matches!(a3, Value::Nil) {
            return Err(ambiguity("java.lang.String"));
        }
        return Err(no_ctor("java.lang.String", false));
    };
    let (off, count) = (int_exact_long(*off)?, int_exact_long(*count)?);
    let cs = lookup_charset(name)?;
    let Some(bytes) = bytes else {
        return Err(npe_bytes());
    };
    let (start, len) = check_bounds(off, count, bytes.len())?;
    Ok(fresh_string(decode(cs, &bytes[start..start + len])))
}

/// `(char[],off,count)` (oracle `ctor-str-chararr-off`).
fn chars_range(v: &[char], off: i64, count: i64) -> Result<Value, NativeError> {
    let (off, count) = (int_exact_long(off)?, int_exact_long(count)?);
    let (start, len) = check_bounds(off, count, v.len())?;
    Ok(fresh_string(v[start..start + len].iter().collect()))
}

/// `(byte[],off,len)` over the default charset (UTF-8 — oracle
/// `ctor-str-bytearr-off`).
fn bytes_range(v: &[i8], off: i64, count: i64) -> Result<Value, NativeError> {
    let (off, count) = (int_exact_long(off)?, int_exact_long(count)?);
    let bytes: Vec<u8> = v.iter().map(|b| *b as u8).collect();
    let (start, len) = check_bounds(off, count, bytes.len())?;
    Ok(fresh_string(decode_utf8(&bytes[start..start + len])))
}

/// `(int[],off,count)` over code points (oracle
/// `ctor-str-intarr`): below 0 / above `0x10FFFF` is `IAE |
/// <int>` (oracles `ctor-str-badcp`, `cp-toobig`); unpaired
/// surrogates become U+FFFD — the JVM embeds them (oracle
/// `sg-hex`), unrepresentable in Rust `char` (spec N5).
fn codepoints_range(v: &[i32], off: i64, count: i64) -> Result<Value, NativeError> {
    let (off, count) = (int_exact_long(off)?, int_exact_long(count)?);
    let (start, len) = check_bounds(off, count, v.len())?;
    let mut s = String::new();
    for cp in &v[start..start + len] {
        if *cp < 0 || *cp > 0x10FFFF {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                cp.to_string(),
            ));
        }
        s.push(char::from_u32(*cp as u32).unwrap_or('\u{FFFD}'));
    }
    Ok(fresh_string(s))
}

/// `(byte[],hibyte)` (oracles `ctor-str-hibyte`, `hb-2`,
/// `hb-neg1`, `hb-negbyte`): `(char)((hibyte << 8) | (b &
/// 0xff))` with Java int wrap; lone surrogates become U+FFFD
/// (spec N5).
fn hibyte_string(bytes: &[u8], hibyte: i32) -> String {
    bytes
        .iter()
        .map(|b| {
            let u = (hibyte.wrapping_shl(8) | i32::from(*b)) as u16;
            char::from_u32(u32::from(u)).unwrap_or('\u{FFFD}')
        })
        .collect()
}

/// `Objects.checkFromIndexSize`: negatives fail, `off + count`
/// past the end fails (sums in `u64` — no overflow); the message
/// prints the expression unreduced, exactly like the JDK
/// (oracles `ctor-str-off-oob`, `ctor-str-off-neg`,
/// `ctor-str-cnt-oob`, `cs-off-oob`).
fn check_bounds(off: i32, count: i32, len: usize) -> Result<(usize, usize), NativeError> {
    let ok = off >= 0 && count >= 0 && off as u64 + count as u64 <= len as u64;
    if ok {
        Ok((off as usize, count as usize))
    } else {
        Err(NativeError::new(
            ThrowClass::StringIndexOutOfBoundsException,
            format!("Range [{off}, {off} + {count}) out of bounds for length {len}"),
        ))
    }
}

/// The null-bytes NPE (port-pinned house style — the JDK text is
/// an internal deref message and varies by JDK; oracles
/// `d-nil-utf8`, `d-nil-0`, `e-nil-badbounds`,
/// `cs-nil-bytes`).
fn npe_bytes() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"java.lang.String.<init>(byte[])\" because \"bytes\" is null".to_owned(),
    )
}

/// A probed charset (spec §3.7 — exactly the oracle-pinned names
/// ship; everything else is UEE like an unknown name, spec N6).
/// Shared with the `clojure.java.io` facsimile (gap-120 slice 3),
/// which decodes file bytes and encodes writer output the same way.
#[derive(Clone, Copy, Debug)]
pub(crate) enum Charset {
    Utf8,
    Ascii,
    Latin1,
    Utf16,
    Utf16Be,
    Utf16Le,
    Utf32,
    Utf32Be,
    Utf32Le,
}

/// Charset names and their probed aliases, matched
/// case-insensitively (oracles `cs-alias`, `a-*`, `u32-*`).
const CHARSET_ALIASES: &[(&str, Charset)] = &[
    ("utf-8", Charset::Utf8),
    ("utf8", Charset::Utf8),
    ("us-ascii", Charset::Ascii),
    ("ascii", Charset::Ascii),
    ("iso-8859-1", Charset::Latin1),
    ("iso8859-1", Charset::Latin1),
    ("latin1", Charset::Latin1),
    ("utf-16", Charset::Utf16),
    ("utf16", Charset::Utf16),
    ("unicode", Charset::Utf16),
    ("utf-16be", Charset::Utf16Be),
    ("unicodebig", Charset::Utf16Be),
    ("utf-16le", Charset::Utf16Le),
    ("unicodelittle", Charset::Utf16Le),
    ("utf-32", Charset::Utf32),
    ("utf-32be", Charset::Utf32Be),
    ("utf-32le", Charset::Utf32Le),
];

/// Look up a charset name: unknown, empty, and illegal names all
/// throw `UnsupportedEncodingException` carrying the name
/// verbatim — no trimming (oracles `ctor-str-badcs/4`,
/// `v-empty`, `v-badchars`, `v-colon`, `s-spcs`).
pub(crate) fn lookup_charset(name: &str) -> Result<Charset, NativeError> {
    CHARSET_ALIASES
        .iter()
        .find(|(alias, _)| alias.eq_ignore_ascii_case(name))
        .map(|(_, cs)| *cs)
        .ok_or_else(|| NativeError::new(ThrowClass::UnsupportedEncodingException, name.to_owned()))
}

/// Decode with REPLACE (malformed/truncated input becomes
/// U+FFFD, never an error — oracles `cs-ascii-high`,
/// `cs-utf16-odd`, `u32-trunc`, `u32-badcp`).
pub(crate) fn decode(cs: Charset, bytes: &[u8]) -> String {
    match cs {
        Charset::Utf8 => decode_utf8(bytes),
        Charset::Ascii => bytes
            .iter()
            .map(|b| if *b < 128 { *b as char } else { '\u{FFFD}' })
            .collect(),
        Charset::Latin1 => bytes.iter().map(|b| *b as char).collect(),
        Charset::Utf16 => decode_utf16_auto(bytes),
        Charset::Utf16Be => decode_utf16(bytes, false),
        Charset::Utf16Le => decode_utf16(bytes, true),
        Charset::Utf32 => decode_utf32_auto(bytes),
        Charset::Utf32Be => decode_utf32(bytes, false),
        Charset::Utf32Le => decode_utf32(bytes, true),
    }
}

/// The default charset: UTF-8 lossy (oracles
/// `ctor-str-bytearr*`, `ctor-str-bytearr-bad`).
pub(crate) fn decode_utf8(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

/// Fixed-endianness UTF-16 with REPLACE; a truncated trailing
/// byte is one U+FFFD (oracles `ctor-str-bytearr-utf16be`,
/// `cs-utf16le`, `cs-utf16-odd`, `s-4arg-be`).
fn decode_utf16(bytes: &[u8], little: bool) -> String {
    let mut units = Vec::with_capacity(bytes.len() / 2 + 1);
    let mut chunks = bytes.chunks_exact(2);
    for w in &mut chunks {
        let pair = [w[0], w[1]];
        units.push(if little {
            u16::from_le_bytes(pair)
        } else {
            u16::from_be_bytes(pair)
        });
    }
    let mut s: String = char::decode_utf16(units)
        .map(|r| r.unwrap_or('\u{FFFD}'))
        .collect();
    if !chunks.remainder().is_empty() {
        s.push('\u{FFFD}');
    }
    s
}

/// `UTF-16`: strip a BOM of either endianness, else decode
/// big-endian (oracles `ctor-str-bytearr-utf16`,
/// `cs-utf16le-bom`, `bom-le/be`).
fn decode_utf16_auto(bytes: &[u8]) -> String {
    if let [0xFE, 0xFF, rest @ ..] = bytes {
        decode_utf16(rest, false)
    } else if let [0xFF, 0xFE, rest @ ..] = bytes {
        decode_utf16(rest, true)
    } else {
        decode_utf16(bytes, false)
    }
}

/// Fixed-endianness UTF-32 with REPLACE (oracles `u32-be*`,
/// `u32-le`, `u32-trunc`, `u32-badcp`).
fn decode_utf32(bytes: &[u8], little: bool) -> String {
    let mut s = String::new();
    let mut chunks = bytes.chunks_exact(4);
    for w in &mut chunks {
        let word = [w[0], w[1], w[2], w[3]];
        let u = if little {
            u32::from_le_bytes(word)
        } else {
            u32::from_be_bytes(word)
        };
        s.push(char::from_u32(u).unwrap_or('\u{FFFD}'));
    }
    if !chunks.remainder().is_empty() {
        s.push('\u{FFFD}');
    }
    s
}

/// `UTF-32`: strip a BOM of either endianness, else decode
/// big-endian (oracles `u32-bom`, `s-u32lebom`).
fn decode_utf32_auto(bytes: &[u8]) -> String {
    if let [0x00, 0x00, 0xFE, 0xFF, rest @ ..] = bytes {
        decode_utf32(rest, false)
    } else if let [0xFF, 0xFE, 0x00, 0x00, rest @ ..] = bytes {
        decode_utf32(rest, true)
    } else {
        decode_utf32(bytes, false)
    }
}

/// Encode a string the way the JDK `OutputStreamWriter` does:
/// unmappable chars become `?` (ASCII/Latin-1), the bare
/// `UTF-16`/`UTF-32` forms take a big-endian BOM when `bom`
/// (once per stream — the caller latches it), and the
/// fixed-endianness forms never do. Gap-120 slice 3
/// (`clojure.java.io` writer output).
pub(crate) fn encode(cs: Charset, s: &str, bom: bool) -> Vec<u8> {
    match cs {
        Charset::Utf8 => s.as_bytes().to_vec(),
        Charset::Ascii => s
            .chars()
            .map(|c| {
                let u = u32::from(c);
                if u < 128 { u as u8 } else { b'?' }
            })
            .collect(),
        Charset::Latin1 => encode_latin1(s),
        Charset::Utf16 => encode_utf16_units(s, false, bom),
        Charset::Utf16Be => encode_utf16_units(s, false, false),
        Charset::Utf16Le => encode_utf16_units(s, true, false),
        Charset::Utf32 => encode_utf32_words(s, false, bom),
        Charset::Utf32Be => encode_utf32_words(s, false, false),
        Charset::Utf32Le => encode_utf32_words(s, true, false),
    }
}

/// The bare-form BOM bytes (empty for every fixed-endianness
/// form): `close` on a never-written writer emits these, like
/// the JDK `OutputStreamWriter` end-of-input flush. Gap-120
/// slice 3 (`clojure.java.io` writer output).
pub(crate) fn bom_bytes(cs: Charset) -> &'static [u8] {
    match cs {
        Charset::Utf16 => &[0xFE, 0xFF],
        Charset::Utf32 => &[0x00, 0x00, 0xFE, 0xFF],
        _ => &[],
    }
}

/// Latin-1 bytes: chars past U+00FF become `?`.
fn encode_latin1(s: &str) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len());
    for c in s.chars() {
        let u = u32::from(c);
        out.push(if u <= 0xFF { u as u8 } else { b'?' });
    }
    out
}

/// UTF-16 units in the given endianness, with a BOM when `bom`.
fn encode_utf16_units(s: &str, little: bool, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 2 + 2);
    if bom {
        out.extend_from_slice(&[0xFE, 0xFF]);
    }
    let mut buf = [0u16; 2];
    for c in s.chars() {
        for unit in c.encode_utf16(&mut buf) {
            let bytes = if little {
                unit.to_le_bytes()
            } else {
                unit.to_be_bytes()
            };
            out.extend_from_slice(&bytes);
        }
    }
    out
}

/// UTF-32 words in the given endianness, with a BOM when `bom`.
fn encode_utf32_words(s: &str, little: bool, bom: bool) -> Vec<u8> {
    let mut out = Vec::with_capacity(s.len() * 4 + 4);
    if bom {
        out.extend_from_slice(&[0x00, 0x00, 0xFE, 0xFF]);
    }
    for c in s.chars() {
        let word = c as u32;
        let bytes = if little {
            word.to_le_bytes()
        } else {
            word.to_be_bytes()
        };
        out.extend_from_slice(&bytes);
    }
    out
}
