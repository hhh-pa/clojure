// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure.core/format`: `java.util.Formatter` parity over the
//! `%[index$][flags][width][.precision][t]conversion` grammar.
//!
//! Everything here is oracle-pinned (`docs/t1174/s5d-pins.md`
//! "Format", corpus 12/13/14, the `s5d-fmt*` probe batches):
//! - Parse failures (end of string or a non-`[a-zA-Z%]` char where
//!   the conversion belongs) report the char AFTER `%` (or `%`
//!   itself when nothing follows); `%t` consumes a second char.
//! - Flag checks run before argument lookup, in order: duplicates
//!   (during parse, every flag including `<`) → the `+`/space pair
//!   (numeric/`%n`/`%%` paths only) → mismatch/flags-error → the
//!   `-`/`0` width requirement → precision.
//! - Mismatch messages are per-conversion (`s`/`a` drop a
//!   non-sole `#`, `t`/`c` front it, `b`/`h` truncate after it,
//!   `x`/`o` collapse to a present `,`); `%n`/`%%` use the
//!   flags-error classes instead (string/reverse order).
//! - Floats render from the shortest decimal with `HALF_UP`
//!   (`Double.toString` semantics — `%.2f` of `2.675` is `2.68`);
//!   `%g` never strips; `%a` is bit-exact hex-float; `%t` renders
//!   UTC civil time.
//!
//! Known divergences (all probed): astral `%s` precision counts
//! Unicode scalars, not UTF-16 units; lone surrogates are
//! unrepresentable; `%c` takes only `Character` (the port has no
//! `Integer`); `%ts` of negatives truncates toward zero.

use std::sync::Arc;

use clj_value::Value;

use crate::core::{cce_err, str_of_one};
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::lazy::realize_deep;
use crate::ns::signal_to_native;
use clj_value::ThrowClass;

/// `clojure.core/format` (variadic: the format string plus args).
pub(crate) fn native_format(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/format", 0));
    }
    let fmt = format_first_arg(args)?;
    let mut out = String::new();
    let mut rest = fmt.as_str();
    let mut auto_idx = 0usize;
    let mut last_idx: Option<usize> = None;
    while let Some(pct) = rest.find('%') {
        out.push_str(&rest[..pct]);
        rest = &rest[pct + 1..];
        let (spec, after) = parse_spec(rest)?;
        rest = after;
        if spec.conversion == 'n' || spec.conversion == '%' {
            emit_bare_spec(&spec, &mut out)?;
            continue;
        }
        check_spec(&spec)?;
        let arg_idx = resolve_arg_index(&spec, &mut auto_idx, last_idx)?;
        last_idx = Some(arg_idx);
        let arg = fetch_format_arg(args, arg_idx, &spec)?;
        out.push_str(&apply_spec(eval, &spec, arg)?);
    }
    out.push_str(rest);
    Ok(Value::Str(Arc::from(out.as_str())))
}

/// The format string (argument 0 is a `String`, never nil).
fn format_first_arg(args: &[Value]) -> Result<String, NativeError> {
    match &args[0] {
        Value::Str(s) => Ok(s.to_string()),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"String.length()\" because \"s\" is null".to_owned(),
        )),
        v => Err(cce_err(v, "java.lang.String")),
    }
}

/// Emit `%n`/`%%` (no argument; neither the auto index nor the
/// previous-argument slot is disturbed).
fn emit_bare_spec(spec: &FormatSpec, out: &mut String) -> Result<(), NativeError> {
    check_spec(spec)?;
    if spec.conversion == 'n' {
        out.push_str(line_separator());
    } else {
        out.push_str(&pad("%", spec));
    }
    Ok(())
}

/// Resolve the 0-based argument index (explicit, `<`, or auto).
fn resolve_arg_index(
    spec: &FormatSpec,
    auto_idx: &mut usize,
    last_idx: Option<usize>,
) -> Result<usize, NativeError> {
    match spec.index {
        Some(i) => Ok(i),
        None if spec.reuse_last => last_idx.ok_or_else(|| missing_format_arg(spec)),
        None => {
            let i = *auto_idx;
            *auto_idx += 1;
            Ok(i)
        }
    }
}

/// Fetch the indexed argument (index 0 is `args[1]`).
fn fetch_format_arg<'a>(
    args: &'a [Value],
    arg_idx: usize,
    spec: &FormatSpec,
) -> Result<&'a Value, NativeError> {
    args.get(arg_idx + 1)
        .ok_or_else(|| missing_format_arg(spec))
}

/// `MissingFormatArgumentException` (the full spec text).
fn missing_format_arg(spec: &FormatSpec) -> NativeError {
    NativeError::new(
        ThrowClass::MissingFormatArgumentException,
        format!("Format specifier '{}'", spec.text),
    )
}

/// The platform line separator (`%n` renders it).
fn line_separator() -> &'static str {
    if cfg!(windows) { "\r\n" } else { "\n" }
}

/// One parsed format specifier.
struct FormatSpec {
    /// The full spec text (`%...`, for `MissingFormatArgument`).
    text: String,
    /// Explicit 0-based argument index (`2$`).
    index: Option<usize>,
    /// The `<` previous-argument flag.
    reuse_last: bool,
    /// Flags in string order (`<` excluded).
    flags: Vec<char>,
    width: Option<usize>,
    precision: Option<usize>,
    /// The conversion (`%t`/`%T` report the prefix here).
    conversion: char,
    /// The `%t`/`%T` suffix (both chars consumed).
    suffix: Option<char>,
}

/// Parse one specifier (the text AFTER `%`).
fn parse_spec(s: &str) -> Result<(FormatSpec, &str), NativeError> {
    let (index, pos) = parse_arg_index(s)?;
    let (flags, reuse_last, pos) = parse_flags(s, pos)?;
    let (width, pos) = parse_width(s, pos)?;
    let (precision, pos) = parse_precision(s, pos)?;
    let (conversion, suffix, pos) = parse_conversion(s, pos)?;
    let text = format!("%{}", &s[..pos]);
    Ok((
        FormatSpec {
            text,
            index,
            reuse_last,
            flags,
            width,
            precision,
            conversion,
            suffix,
        },
        &s[pos..],
    ))
}

/// Explicit argument index (`2$`): the 0-based index plus the
/// position after the `$` (or 0 when no index is present).
fn parse_arg_index(s: &str) -> Result<(Option<usize>, usize), NativeError> {
    let bytes = s.as_bytes();
    let mut end = 0usize;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    if end > 0 && bytes.get(end) == Some(&b'$') {
        let digits = &s[..end];
        let index = match digits.parse::<u64>() {
            Ok(n) if n <= i32::MAX as u64 => {
                if n == 0 {
                    return Err(NativeError::new(
                        ThrowClass::IllegalFormatArgumentIndexException,
                        "Illegal format argument index = 0".to_owned(),
                    ));
                }
                n as usize - 1
            }
            _ => {
                return Err(NativeError::new(
                    ThrowClass::IllegalFormatArgumentIndexException,
                    "Format argument index: (not representable as int)".to_owned(),
                ));
            }
        };
        return Ok((Some(index), end + 1));
    }
    Ok((None, 0))
}

/// Flags (duplicates, including `<`, fail during parse).
fn parse_flags(s: &str, mut pos: usize) -> Result<(Vec<char>, bool, usize), NativeError> {
    let mut flags = Vec::new();
    let mut reuse_last = false;
    while let Some(c) = s[pos..].chars().next() {
        if c != '<' && !matches!(c, '-' | '#' | '+' | ' ' | '0' | ',' | '(') {
            break;
        }
        if c == '<' {
            if reuse_last {
                return Err(duplicate_flag('<'));
            }
            reuse_last = true;
            pos += 1;
        } else {
            if flags.contains(&c) {
                return Err(duplicate_flag(c));
            }
            flags.push(c);
            pos += c.len_utf8();
        }
    }
    Ok((flags, reuse_last, pos))
}

/// Width (overflowing the `int` is the constant `MIN_VALUE`).
fn parse_width(s: &str, pos: usize) -> Result<(Option<usize>, usize), NativeError> {
    let bytes = s.as_bytes();
    let mut end = pos;
    while end < bytes.len() && bytes[end].is_ascii_digit() {
        end += 1;
    }
    let width = if end > pos {
        match s[pos..end].parse::<u64>() {
            Ok(n) if n <= i32::MAX as u64 => Some(n as usize),
            _ => {
                return Err(NativeError::new(
                    ThrowClass::IllegalFormatWidthException,
                    i32::MIN.to_string(),
                ));
            }
        }
    } else {
        None
    };
    Ok((width, end))
}

/// Precision (`.` plus at least one digit, else the `.` stays for
/// the conversion slot and the parse fails below).
fn parse_precision(s: &str, mut pos: usize) -> Result<(Option<usize>, usize), NativeError> {
    let bytes = s.as_bytes();
    let mut precision = None;
    if s[pos..].starts_with('.') {
        let mut digits = pos + 1;
        while digits < bytes.len() && bytes[digits].is_ascii_digit() {
            digits += 1;
        }
        if digits > pos + 1 {
            match s[pos + 1..digits].parse::<u64>() {
                Ok(n) if n <= i32::MAX as u64 => precision = Some(n as usize),
                _ => {
                    return Err(NativeError::new(
                        ThrowClass::IllegalFormatPrecisionException,
                        i32::MIN.to_string(),
                    ));
                }
            }
            pos = digits;
        }
    }
    Ok((precision, pos))
}

/// Conversion (`[a-zA-Z%]`, plus the `%t`/`%T` suffix): else the
/// char after `%` is the unknown conversion (`%` itself when the
/// string ends there).
fn parse_conversion(s: &str, mut pos: usize) -> Result<(char, Option<char>, usize), NativeError> {
    let Some(conversion) = s[pos..].chars().next() else {
        return Err(unknown_conversion(s.chars().next().unwrap_or('%'), None));
    };
    if !conversion.is_ascii_alphabetic() && conversion != '%' {
        return Err(unknown_conversion(s.chars().next().unwrap_or('%'), None));
    }
    pos += conversion.len_utf8();
    let mut suffix = None;
    if conversion == 't' || conversion == 'T' {
        let Some(second) = s[pos..].chars().next() else {
            return Err(unknown_conversion(conversion, None));
        };
        if !is_time_suffix(second) {
            return Err(unknown_conversion(conversion, Some(second)));
        }
        suffix = Some(second);
        pos += second.len_utf8();
    }
    Ok((conversion, suffix, pos))
}

/// `DuplicateFormatFlagsException` (`Flags = 'X'`).
fn duplicate_flag(c: char) -> NativeError {
    NativeError::new(
        ThrowClass::DuplicateFormatFlagsException,
        format!("Flags = '{c}'"),
    )
}

/// `UnknownFormatConversionException` (one char, or the `%t` pair).
fn unknown_conversion(first: char, second: Option<char>) -> NativeError {
    let text = match second {
        Some(c) => format!("{first}{c}"),
        None => first.to_string(),
    };
    NativeError::new(
        ThrowClass::UnknownFormatConversionException,
        format!("Conversion = '{text}'"),
    )
}

/// The `%t` suffix set (anything else pairs into the unknown).
fn is_time_suffix(c: char) -> bool {
    matches!(
        c,
        'H' | 'I'
            | 'k'
            | 'l'
            | 'M'
            | 'S'
            | 'L'
            | 'N'
            | 'p'
            | 'z'
            | 'Z'
            | 's'
            | 'Q'
            | 'B'
            | 'b'
            | 'h'
            | 'A'
            | 'a'
            | 'C'
            | 'Y'
            | 'y'
            | 'j'
            | 'm'
            | 'd'
            | 'e'
            | 'R'
            | 'T'
            | 'D'
            | 'F'
            | 'c'
    )
}

/// Canonical flag order (`-`, `#`, `+`, space, `0`, `,`, `(`):
/// every flag-list message sorts by it.
fn flag_rank(c: char) -> usize {
    // Only the seven parsed flags ever reach here (anything else
    // sorts last).
    "-#+ 0,(".find(c).unwrap_or(7)
}

/// Flags in canonical order.
fn canonical_flags(flags: &[char]) -> Vec<char> {
    let mut out = flags.to_vec();
    out.sort_by_key(|c| flag_rank(*c));
    out
}

/// Flag/spec checks (all run before argument lookup). Numerics:
/// width → pair → mismatch → precision. `%s`: dash-width →
/// mismatch → precision. `%b`/`%h`: dash-and-zero-width →
/// mismatch → precision. `%c`/`%t`: mismatch → width →
/// precision. `%n`: precision → width → pair → flags. `%%`:
/// precision → pair → flags → width.
fn check_spec(spec: &FormatSpec) -> Result<(), NativeError> {
    let conv = spec.conversion;
    if conv == 'n' {
        return check_line_spec(spec);
    }
    if conv == '%' {
        return check_percent_spec(spec);
    }
    check_spec_arg_kind(spec)?;
    if let Some(p) = spec.precision.filter(|_| precision_illegal(conv)) {
        return Err(precision_err(p));
    }
    Ok(())
}

/// `%n` checks: precision → width → pair → flags.
fn check_line_spec(spec: &FormatSpec) -> Result<(), NativeError> {
    if let Some(p) = spec.precision {
        return Err(precision_err(p));
    }
    if let Some(w) = spec.width {
        return Err(NativeError::new(
            ThrowClass::IllegalFormatWidthException,
            w.to_string(),
        ));
    }
    check_sign_pair(spec)?;
    if !spec.flags.is_empty() {
        let all: String = canonical_flags(&spec.flags).into_iter().collect();
        return Err(NativeError::new(
            ThrowClass::IllegalFormatFlagsException,
            format!("Flags = '{all}'"),
        ));
    }
    Ok(())
}

/// `%%` checks: precision → pair → flags → width.
fn check_percent_spec(spec: &FormatSpec) -> Result<(), NativeError> {
    if let Some(p) = spec.precision {
        return Err(precision_err(p));
    }
    check_sign_pair(spec)?;
    let bad_flag = allowed_flags(spec.conversion)
        .is_some_and(|allowed| spec.flags.iter().any(|f| !allowed.contains(f)));
    if bad_flag {
        let all: String = canonical_flags(&spec.flags).into_iter().collect();
        return Err(NativeError::new(
            ThrowClass::IllegalFormatFlagsException,
            format!("Flags = '{all}'"),
        ));
    }
    check_width_required(spec)
}

/// Per-kind check order for conversions that take an argument.
fn check_spec_arg_kind(spec: &FormatSpec) -> Result<(), NativeError> {
    let conv = spec.conversion;
    if is_numeric_conv(conv) {
        check_width_required(spec)?;
        check_sign_pair(spec)?;
        check_mismatch(spec)?;
    } else if matches!(conv, 's' | 'S') {
        check_width_if(spec, spec.flags.contains(&'-'))?;
        check_mismatch(spec)?;
    } else if matches!(conv, 'b' | 'B' | 'h' | 'H') {
        check_width_if(spec, spec.flags.contains(&'-') && spec.flags.contains(&'0'))?;
        check_mismatch(spec)?;
    } else {
        check_mismatch(spec)?;
        check_width_required(spec)?;
    }
    Ok(())
}

/// The width requirement when `trigger` holds (the reconstructed
/// `%` + canonical flags + index + precision + conversion).
fn check_width_if(spec: &FormatSpec, trigger: bool) -> Result<(), NativeError> {
    if trigger {
        check_width_required(spec)
    } else {
        Ok(())
    }
}

/// The mismatch/sign-pair family (`d`/`o`/`x`/`e`/`f`/`g`/`a`).
fn is_numeric_conv(conv: char) -> bool {
    matches!(
        conv,
        'd' | 'o' | 'x' | 'X' | 'e' | 'E' | 'f' | 'g' | 'G' | 'a' | 'A'
    )
}

/// The `+`/space pair (all flags, canonical order).
fn check_sign_pair(spec: &FormatSpec) -> Result<(), NativeError> {
    if spec.flags.contains(&'+') && spec.flags.contains(&' ') {
        let all: String = canonical_flags(&spec.flags).into_iter().collect();
        return Err(NativeError::new(
            ThrowClass::IllegalFormatFlagsException,
            format!("Flags = '{all}'"),
        ));
    }
    Ok(())
}

/// `-`/`0` without a width (`%` + canonical flags + index +
/// precision + conversion, reconstructed).
fn check_width_required(spec: &FormatSpec) -> Result<(), NativeError> {
    if spec.width.is_none() && spec.flags.iter().any(|f| *f == '-' || *f == '0') {
        let mut text = String::from("%");
        text.extend(canonical_flags(&spec.flags));
        if let Some(i) = spec.index {
            text.push_str(&format!("{}$", i + 1));
        }
        if let Some(p) = spec.precision {
            text.push_str(&format!(".{p}"));
        }
        text.push(spec.conversion);
        if let Some(s) = spec.suffix {
            text.push(s);
        }
        return Err(NativeError::new(
            ThrowClass::MissingFormatWidthException,
            text,
        ));
    }
    Ok(())
}

/// `IllegalFormatPrecisionException` (the message is the number).
fn precision_err(p: usize) -> NativeError {
    NativeError::new(ThrowClass::IllegalFormatPrecisionException, p.to_string())
}

/// Whether precision is illegal for the conversion.
fn precision_illegal(conv: char) -> bool {
    matches!(
        conv,
        'd' | 'o' | 'x' | 'X' | 'c' | 'C' | 't' | 'T' | 'n' | '%'
    )
}

/// Allowed flags per conversion (`%t` allows only `-`; `%n` has no
/// mismatch check; `%%` allows only `-`).
fn allowed_flags(conv: char) -> Option<&'static [char]> {
    Some(match conv {
        'b' | 'B' | 'h' | 'H' | 's' | 'S' | 'c' | 'C' | 't' | 'T' | '%' => &['-'],
        'd' => &['-', '+', ' ', '0', ',', '('],
        'o' | 'x' | 'X' => &['-', '#', '0'],
        'e' | 'E' | 'f' => &['-', '+', ' ', '#', '0', ',', '('],
        'g' | 'G' => &['-', '+', ' ', '0', ',', '('],
        'a' | 'A' => &['-', '+', ' ', '#', '0'],
        _ => return None,
    })
}

/// The mismatch check (`Conversion = {c}, Flags = {bad}`; `%t`
/// names the suffix; bad flags in canonical order).
fn check_mismatch(spec: &FormatSpec) -> Result<(), NativeError> {
    let conv = spec.conversion;
    let Some(allowed) = allowed_flags(conv) else {
        return Ok(());
    };
    let mut bad: Vec<char> = spec
        .flags
        .iter()
        .copied()
        .filter(|f| !allowed.contains(f))
        .collect();
    if bad.is_empty() {
        return Ok(());
    }
    bad = canonical_flags(&bad);
    // Per-conversion message shapes (each probed twice or more).
    let shown = match conv {
        's' | 'S' | 'a' | 'A' => {
            bad.retain(|f| *f != '#');
            if bad.is_empty() {
                "#".to_owned()
            } else {
                bad.iter().collect()
            }
        }
        'b' | 'B' | 'h' | 'H' => match bad.iter().position(|f| *f == '#') {
            Some(i) => bad[..=i].iter().collect(),
            None => bad.iter().collect(),
        },
        'x' | 'X' | 'o' => {
            if bad.contains(&',') {
                ",".to_owned()
            } else {
                bad.iter().collect()
            }
        }
        _ => bad.iter().collect(),
    };
    let name = spec.suffix.unwrap_or(conv);
    Err(NativeError::new(
        ThrowClass::FormatFlagsConversionMismatchException,
        format!("Conversion = {name}, Flags = {shown}"),
    ))
}

/// Apply one checked spec to its argument.
fn apply_spec(eval: &mut Evaluator, spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    match spec.conversion {
        'b' | 'B' => apply_bool_spec(spec, arg),
        'h' | 'H' => apply_hash_spec(eval, spec, arg),
        's' | 'S' => apply_str_spec(eval, spec, arg),
        'c' | 'C' => apply_char_spec(spec, arg),
        'd' => apply_dec_spec(spec, arg),
        'o' | 'x' | 'X' => apply_radix_spec(spec, arg),
        'e' | 'E' | 'f' | 'g' | 'G' => apply_decimal_float_spec(spec, arg),
        'a' | 'A' => apply_hexfloat_spec(spec, arg),
        't' | 'T' => apply_time_spec(spec, arg),
        conv => Err(NativeError::new(
            ThrowClass::UnknownFormatConversionException,
            format!("Conversion = '{conv}'"),
        )),
    }
}

/// `%b`/`%B` (nil is `false`; anything non-boolean is `true`).
fn apply_bool_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let mut s = match arg {
        Value::Nil => "false".to_owned(),
        Value::Bool(b) => b.to_string(),
        _ => "true".to_owned(),
    };
    if let Some(p) = spec.precision {
        s = truncate_chars(&s, p);
    }
    if spec.conversion == 'B' {
        s = s.to_uppercase();
    }
    Ok(pad(&s, spec))
}

/// `%h`/`%H` (`.hashCode` in hex over the realized value).
fn apply_hash_spec(
    eval: &mut Evaluator,
    spec: &FormatSpec,
    arg: &Value,
) -> Result<String, NativeError> {
    let mut s = match arg {
        Value::Nil => "null".to_owned(),
        v => {
            // `.hashCode` over the realized value (infinite
            // lazies hang, like the JVM hash walk).
            let r = realize_deep(eval, v).map_err(signal_to_native)?;
            let h = clj_value::hash_code(&r);
            if spec.conversion == 'H' {
                format!("{:X}", h as u32)
            } else {
                format!("{:x}", h as u32)
            }
        }
    };
    if let Some(p) = spec.precision {
        s = truncate_chars(&s, p);
    }
    Ok(pad(&s, spec))
}

/// `%s`/`%S` (`str` of one value).
fn apply_str_spec(
    eval: &mut Evaluator,
    spec: &FormatSpec,
    arg: &Value,
) -> Result<String, NativeError> {
    let mut s = match arg {
        Value::Nil => "null".to_owned(),
        Value::Str(st) => st.to_string(),
        v => str_of_one(eval, v)?,
    };
    if let Some(p) = spec.precision {
        s = truncate_chars(&s, p);
    }
    if spec.conversion == 'S' {
        s = s.to_uppercase();
    }
    Ok(pad(&s, spec))
}

/// `%c`/`%C` (only `Character`: a Clojure `Long` is `Long`, not
/// `int` — the port has no `Integer` at all).
fn apply_char_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let conv = spec.conversion;
    let mut s = match arg {
        Value::Nil => "null".to_owned(),
        Value::Char(c) => c.to_string(),
        v => return Err(conversion_err(conv, v)),
    };
    if conv == 'C' {
        s = s.to_uppercase();
    }
    Ok(pad(&s, spec))
}

/// `%d` over a Clojure `Long`.
fn apply_dec_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let n = match arg {
        Value::Nil => return Ok(pad("null", spec)),
        Value::Int(n) => *n,
        v => return Err(conversion_err(spec.conversion, v)),
    };
    Ok(format_int(n, spec))
}

/// `%o`/`%x`/`%X` over a Clojure `Long` (two's complement).
fn apply_radix_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let conv = spec.conversion;
    let n = match arg {
        Value::Nil => return Ok(pad("null", spec)),
        Value::Int(n) => *n as u64,
        v => return Err(conversion_err(conv, v)),
    };
    let digits = match conv {
        'o' => format!("{n:o}"),
        'x' => format!("{n:x}"),
        _ => format!("{n:X}"),
    };
    let prefix = if spec.flags.contains(&'#') {
        match conv {
            'o' => "0",
            'x' => "0x",
            _ => "0X",
        }
    } else {
        ""
    };
    Ok(pad_zero_prefixed(prefix, &digits, spec))
}

/// `%e`/`%E`/`%f`/`%g`/`%G` over a Clojure `Double`.
fn apply_decimal_float_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let conv = spec.conversion;
    let d = match arg {
        Value::Nil => return Ok(pad("null", spec)),
        Value::Double(d) => *d,
        v => return Err(conversion_err(conv, v)),
    };
    let s = format_float(d, conv, spec.precision.unwrap_or(6), spec);
    Ok(if conv.is_ascii_uppercase() {
        s.to_uppercase()
    } else {
        s
    })
}

/// `%a`/`%A` over a Clojure `Double`.
fn apply_hexfloat_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let conv = spec.conversion;
    let d = match arg {
        Value::Nil => return Ok(pad("null", spec)),
        Value::Double(d) => *d,
        v => return Err(conversion_err(conv, v)),
    };
    let s = format_hexfloat(d, spec.precision, spec);
    Ok(if conv == 'A' { s.to_uppercase() } else { s })
}

/// `%t`/`%T` over epoch millis (a Clojure `Long`).
fn apply_time_spec(spec: &FormatSpec, arg: &Value) -> Result<String, NativeError> {
    let conv = spec.conversion;
    let suffix = spec.suffix.unwrap_or('Y');
    let millis = match arg {
        Value::Nil => return Ok(pad("null", spec)),
        Value::Int(n) => *n,
        v => return Err(conversion_err(suffix, v)),
    };
    let s = format_time(millis, suffix);
    let s = if conv == 'T' { s.to_uppercase() } else { s };
    Ok(pad(&s, spec))
}

/// `{conversion} != {arg JVM class}`.
fn conversion_err(conv: char, arg: &Value) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalFormatConversionException,
        format!("{conv} != {}", jvm_class_of(arg)),
    )
}

/// First `p` characters (precision truncation).
fn truncate_chars(s: &str, p: usize) -> String {
    s.chars().take(p).collect()
}

/// Space-pad to width (`-` left-justifies).
fn pad(s: &str, spec: &FormatSpec) -> String {
    let width = spec.width.unwrap_or(0);
    let len = s.chars().count();
    if len >= width {
        return s.to_owned();
    }
    let fill: String = " ".repeat(width - len);
    if spec.flags.contains(&'-') {
        format!("{s}{fill}")
    } else {
        format!("{fill}{s}")
    }
}

/// Zero-pad to width with a `0x`/`0` prefix (zeros go after the
/// prefix: `%#08x` of 255 is `0x0000ff`).
fn pad_zero_prefixed(prefix: &str, digits: &str, spec: &FormatSpec) -> String {
    let width = spec.width.unwrap_or(0);
    let len = prefix.chars().count() + digits.chars().count();
    if len >= width {
        return format!("{prefix}{digits}");
    }
    if spec.flags.contains(&'-') {
        return format!("{prefix}{digits}{}", " ".repeat(width - len));
    }
    if spec.flags.contains(&'0') {
        return format!("{prefix}{}{digits}", "0".repeat(width - len));
    }
    format!("{}{prefix}{digits}", " ".repeat(width - len))
}

/// US thousands grouping (`1,234,567`).
fn group_thousands(n: u64) -> String {
    let digits = n.to_string();
    let mut out = String::new();
    for (i, c) in digits.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// `%d` with grouping/sign/parens/zero-pad (US locale).
fn format_int(n: i64, spec: &FormatSpec) -> String {
    let negative = n < 0;
    let digits = if spec.flags.contains(&',') {
        group_thousands(n.unsigned_abs())
    } else {
        n.unsigned_abs().to_string()
    };
    if spec.flags.contains(&'(') {
        return format_int_paren(&digits, negative, spec);
    }
    let sign = if negative {
        "-"
    } else if spec.flags.contains(&'+') {
        "+"
    } else if spec.flags.contains(&' ') {
        " "
    } else {
        ""
    };
    if spec.flags.contains(&'0') && !spec.flags.contains(&'-') {
        let width = spec.width.unwrap_or(0);
        let len = digits.chars().count() + sign.chars().count();
        if len >= width {
            return format!("{sign}{digits}");
        }
        return format!("{sign}{}{digits}", "0".repeat(width - len));
    }
    pad(&format!("{sign}{digits}"), spec)
}

/// `%d` with the `(` flag (negatives in parens; `0` zero-pads
/// inside the parens).
fn format_int_paren(digits: &str, negative: bool, spec: &FormatSpec) -> String {
    let inner = if spec.flags.contains(&'0') {
        let width = spec.width.unwrap_or(0).saturating_sub(2);
        let len = digits.chars().count();
        if len >= width {
            digits.to_owned()
        } else {
            format!("{}{digits}", "0".repeat(width - len))
        }
    } else {
        digits.to_owned()
    };
    let s = if negative {
        format!("({inner})")
    } else {
        inner
    };
    pad(&s, spec)
}

/// Group the integer part of a rendered float (`1,234,567.891`).
fn group_float_int_part(s: &str) -> String {
    match s.split_once('.') {
        Some((head, tail)) => format!("{}.{}", group_head(head), tail),
        None => match s.split_once('e') {
            Some((head, tail)) => format!("{}e{tail}", group_head(head)),
            None => group_head(s),
        },
    }
}

/// Group one run of ASCII digits (sign-agnostic: callers pass the
/// unsigned body).
fn group_head(head: &str) -> String {
    let mut out = String::new();
    for (i, c) in head.chars().rev().enumerate() {
        if i > 0 && i % 3 == 0 {
            out.push(',');
        }
        out.push(c);
    }
    out.chars().rev().collect()
}

/// A shortest-decimal float: `digits` (no leading/trailing zeros)
/// with the point after `exp` of them (`0.digits × 10^exp` with the
/// point shifted: value = digits × 10^(exp - len)).
struct Dec {
    /// Significant digits, ASCII, most significant first.
    digits: Vec<u8>,
    /// Count of integer digits (may be ≤ 0).
    exp: i32,
}

/// Split a finite nonzero `f64` into its shortest digits (`Display`
/// is shortest-round-trip, plain decimal, like `Double.toString`
/// with the exponent expanded).
fn dec_split(d: f64) -> Dec {
    debug_assert!(d.is_finite() && d != 0.0);
    let text = format!("{}", d.abs());
    let (head, tail) = match text.split_once('.') {
        Some((h, t)) => (h, t),
        None => (text.as_str(), ""),
    };
    let mut digits: Vec<u8> = head
        .bytes()
        .chain(tail.bytes())
        .skip_while(|b| *b == b'0')
        .collect();
    while digits.last() == Some(&b'0') {
        digits.pop();
    }
    debug_assert!(!digits.is_empty());
    let head_nz = head.trim_start_matches('0').len() as i32;
    let exp = if head_nz > 0 {
        head_nz
    } else {
        -(tail.bytes().take_while(|b| *b == b'0').count() as i32)
    };
    Dec { digits, exp }
}

/// Round `digits` to `keep` significant digits, half-up; returns the
/// kept digits plus whether the value carried past the leading
/// digit (caller bumps `exp` then).
fn round_half_up(digits: &[u8], keep: usize) -> (Vec<u8>, bool) {
    if digits.len() <= keep {
        let mut out = digits.to_vec();
        out.resize(keep, b'0');
        return (out, false);
    }
    let mut head = digits[..keep].to_vec();
    let rest = &digits[keep..];
    // `rest >= half` (same-length lexicographic digit compare;
    // exact ties round up).
    let mut up = false;
    for (i, b) in rest.iter().enumerate() {
        let half = if i == 0 { b'5' } else { b'0' };
        if *b != half {
            up = *b > half;
            break;
        }
        if i + 1 == rest.len() {
            up = true;
        }
    }
    if !up {
        return (head, false);
    }
    for b in head.iter_mut().rev() {
        if *b == b'9' {
            *b = b'0';
        } else {
            *b += 1;
            return (head, false);
        }
    }
    head.resize(keep, b'0');
    // `keep` is positive at every call site, so the head is
    // non-empty here.
    *head.first_mut().expect("rounded head is non-empty") = b'1';
    (head, true)
}

/// Render `%f` (unsigned body; `prec` fractional digits; no
/// point when `prec` is 0 — `#` adds it back).
fn render_f(digits: &[u8], exp: i32, prec: usize) -> String {
    let mut out = String::new();
    if exp <= 0 {
        out.push('0');
    } else {
        for i in 0..exp as usize {
            out.push(digits.get(i).copied().unwrap_or(b'0') as char);
        }
    }
    if prec == 0 {
        return out;
    }
    out.push('.');
    let mut written = 0usize;
    if exp <= 0 {
        for _ in 0..(-exp) as usize {
            if written >= prec {
                break;
            }
            out.push('0');
            written += 1;
        }
        // The surviving digits exactly fill the rest (every caller
        // rounds to `prec` first, or collapsed to zero above).
        for b in digits {
            out.push(*b as char);
        }
    } else {
        for i in 0..prec {
            out.push(digits.get(exp as usize + i).copied().unwrap_or(b'0') as char);
        }
    }
    out
}

/// Render `%e` (unsigned body; `prec` fractional digits).
fn render_e(digits: &[u8], exp: i32, prec: usize) -> String {
    let mut out = String::new();
    out.push(digits.first().copied().unwrap_or(b'0') as char);
    if prec > 0 {
        out.push('.');
        for i in 0..prec {
            out.push(digits.get(1 + i).copied().unwrap_or(b'0') as char);
        }
    }
    let power = exp - 1;
    if power < 0 {
        out.push_str(&format!("e-{:02}", -power));
    } else {
        out.push_str(&format!("e+{:02}", power));
    }
    out
}

/// `%e`/`%f`/`%g` over the shortest decimal (`HALF_UP`, like
/// `BigDecimal(Double.toString)`).
fn format_float(d: f64, conv: char, precision: usize, spec: &FormatSpec) -> String {
    if let Some(s) = format_float_nonfinite(d, spec) {
        return s;
    }
    let neg = d.is_sign_negative();
    let body = if d == 0.0 {
        float_zero_body(conv, precision)
    } else {
        float_nonzero_body(d, conv, precision)
    };
    finish_float_body(body, neg, conv, spec)
}

/// `NaN`/`Infinity` (`NaN` ignores all sign flags; Infinity
/// honors them — `%(f` of `-Inf` is `(Infinity)`).
fn format_float_nonfinite(d: f64, spec: &FormatSpec) -> Option<String> {
    if d.is_nan() {
        return Some(pad("NaN", spec));
    }
    if d.is_infinite() {
        return Some(format_infinity(d.is_sign_negative(), spec));
    }
    None
}

/// Infinity with sign flags (and `(` for negatives).
fn format_infinity(neg: bool, spec: &FormatSpec) -> String {
    if neg && spec.flags.contains(&'(') {
        return pad("(Infinity)", spec);
    }
    pad(&apply_float_sign("Infinity".to_owned(), neg, spec), spec)
}

/// The unsigned `%e`/`%f`/`%g` body of a zero.
fn float_zero_body(conv: char, precision: usize) -> String {
    match conv {
        'f' => format!("0.{}", "0".repeat(precision)),
        'e' | 'E' => format!("0.{}e+00", "0".repeat(precision)),
        _ => {
            let p = precision.max(1);
            format!("0.{}", "0".repeat(p - 1))
        }
    }
}

/// The unsigned `%e`/`%f`/`%g` body of a finite nonzero value.
fn float_nonzero_body(d: f64, conv: char, precision: usize) -> String {
    let dec = dec_split(d);
    match conv {
        'f' => float_f_body(&dec, precision),
        'e' | 'E' => {
            let (digits, carried) = round_half_up(&dec.digits, 1 + precision);
            let exp = dec.exp + i32::from(carried);
            render_e(&digits, exp, precision)
        }
        _ => float_g_body(&dec, precision),
    }
}

/// The unsigned `%f` body (rounded to `precision` fraction digits).
fn float_f_body(dec: &Dec, precision: usize) -> String {
    let keep = dec.exp as i64 + precision as i64;
    if keep <= 0 {
        // Rounds to zero or one ulp (`0.009` at prec 2
        // is `0.01`: half-up on the leading digit).
        let up = dec.digits.first().is_some_and(|b| *b >= b'5');
        if up {
            render_f(b"1", 1 - precision as i32, precision)
        } else {
            render_f(&[], dec.exp, precision)
        }
    } else {
        let (digits, carried) = round_half_up(&dec.digits, keep as usize);
        let exp = dec.exp + i32::from(carried);
        render_f(&digits, exp, precision)
    }
}

/// The unsigned `%g` body (`max(1, p)` significant digits).
fn float_g_body(dec: &Dec, precision: usize) -> String {
    let p = precision.max(1);
    let (digits, carried) = round_half_up(&dec.digits, p);
    let exp = dec.exp + i32::from(carried);
    // Scientific when the decimal exponent is `< -4`
    // or `>= p`; otherwise `%f`-shaped (never stripped).
    if exp - 1 < -4 || exp > p as i32 {
        render_e(&digits, exp, p - 1)
    } else {
        render_f(&digits, exp, (p as i32 - exp).max(0) as usize)
    }
}

/// Finish a float body: `#` point, grouping, sign, parens,
/// zero-pad.
fn finish_float_body(body: String, neg: bool, conv: char, spec: &FormatSpec) -> String {
    let mut body = force_float_point(body, conv, spec);
    if spec.flags.contains(&',') {
        body = group_float_int_part(&body);
    }
    let body = apply_float_sign(body, neg, spec);
    if spec.flags.contains(&'(') && neg {
        let paren = format!("({})", body.trim_start_matches('-'));
        return pad(&paren, spec);
    }
    pad_zero_sign_aware(&body, spec)
}

/// `#` forces the decimal point (`%#.0f` → `5.`).
fn force_float_point(mut body: String, conv: char, spec: &FormatSpec) -> String {
    if spec.flags.contains(&'#') && matches!(conv, 'f' | 'e' | 'E') && !body.contains('.') {
        if let Some(e) = body.find('e') {
            body.insert(e, '.');
        } else {
            body.push('.');
        }
    }
    body
}

/// Prefix the `-`/`+`/space sign.
fn apply_float_sign(body: String, neg: bool, spec: &FormatSpec) -> String {
    if neg {
        format!("-{body}")
    } else if spec.flags.contains(&'+') {
        format!("+{body}")
    } else if spec.flags.contains(&' ') {
        format!(" {body}")
    } else {
        body
    }
}

/// Zero-pad to width after the sign (else space-pad).
fn pad_zero_sign_aware(body: &str, spec: &FormatSpec) -> String {
    if spec.flags.contains(&'0') && !spec.flags.contains(&'-') {
        let width = spec.width.unwrap_or(0);
        let len = body.chars().count();
        if len < width {
            let (sign, rest) = split_sign(body);
            return format!("{sign}{}{rest}", "0".repeat(width - len));
        }
        return body.to_owned();
    }
    pad(body, spec)
}

/// Split one leading `-`/`+`/space sign off the body.
fn split_sign(body: &str) -> (&str, &str) {
    if let Some(r) = body.strip_prefix('-') {
        ("-", r)
    } else if let Some(r) = body.strip_prefix('+') {
        ("+", r)
    } else if let Some(r) = body.strip_prefix(' ') {
        (" ", r)
    } else {
        ("", body)
    }
}

/// `%a`: `Double.toHexString` (bit-exact; precision is hex digits
/// after the point, `max(1, p)`, half-up in hex).
fn format_hexfloat(d: f64, precision: Option<usize>, spec: &FormatSpec) -> String {
    if let Some(s) = format_hexfloat_nonfinite(d, spec) {
        return s;
    }
    let body = hexfloat_body(d.abs(), precision);
    let body = apply_float_sign(body, d.is_sign_negative(), spec);
    pad_zero_sign_aware(&body, spec)
}

/// `%a` `NaN`/`Infinity` (sign flags honored, no parens).
fn format_hexfloat_nonfinite(d: f64, spec: &FormatSpec) -> Option<String> {
    if d.is_nan() {
        return Some(pad("NaN", spec));
    }
    if d.is_infinite() {
        let body = apply_float_sign("Infinity".to_owned(), d.is_sign_negative(), spec);
        return Some(pad(&body, spec));
    }
    None
}

/// The unsigned `%a` body (`0xh.hhhp<exp>`) of a finite value.
fn hexfloat_body(abs: f64, precision: Option<usize>) -> String {
    let bits = abs.to_bits();
    let raw_exp = ((bits >> 52) & 0x7ff) as i32;
    let mant = bits & 0x000f_ffff_ffff_ffff;
    let (lead, exp) = hexfloat_lead_exp(raw_exp, mant);
    let (lead, digits, exp) = match precision {
        Some(p) => round_hex(lead, mant, raw_exp == 0, p.max(1), exp),
        None => {
            let mut digits = format!("{mant:013x}").into_bytes();
            strip_hex_zeros(&mut digits);
            (lead, digits, exp)
        }
    };
    let mut body = String::from("0x");
    body.push(lead as char);
    body.push('.');
    body.extend(digits.iter().map(|b| *b as char));
    body.push('p');
    body.push_str(&exp.to_string());
    body
}

/// The `%a` leading digit and binary exponent (zero has exponent
/// `0`; subnormals have exponent `-1022`).
fn hexfloat_lead_exp(raw_exp: i32, mant: u64) -> (u8, i32) {
    if raw_exp == 0 {
        (b'0', if mant == 0 { 0 } else { -1022 })
    } else {
        (b'1', raw_exp - 1023)
    }
}

/// Strip trailing hex zeros (at least one digit remains).
fn strip_hex_zeros(digits: &mut Vec<u8>) {
    while digits.len() > 1 && digits.last() == Some(&b'0') {
        digits.pop();
    }
}

/// Round the 13-digit mantissa hex to exactly `keep` digits
/// (half-up in hex: the first dropped digit decides, `>= 8`
/// rounds up; explicit precision never strips).
fn round_hex(lead: u8, mant: u64, subnormal: bool, keep: usize, exp: i32) -> (u8, Vec<u8>, i32) {
    let full = format!("{mant:013x}").into_bytes();
    if full.len() <= keep {
        let mut out = full;
        out.resize(keep, b'0');
        return (lead, out, exp);
    }
    let mut head = full[..keep].to_vec();
    let dropped = &full[keep..];
    let up = dropped[0] >= b'8';
    if !up {
        return (lead, head, exp);
    }
    for b in head.iter_mut().rev() {
        if *b == b'f' {
            *b = b'0';
        } else if *b == b'9' {
            *b = b'a';
            return (lead, head, exp);
        } else {
            *b += 1;
            return (lead, head, exp);
        }
    }
    // Carried past the point: `0x1.fff…` → `0x1.0p(exp+1)`;
    // a subnormal `0x0.fff…` normalizes to `0x1.0p-1022`.
    let zeros = vec![b'0'; keep];
    if subnormal {
        (b'1', zeros, exp)
    } else {
        (b'1', zeros, exp + 1)
    }
}

/// Month/day names (en_US).
const MONTHS_FULL: [&str; 12] = [
    "January",
    "February",
    "March",
    "April",
    "May",
    "June",
    "July",
    "August",
    "September",
    "October",
    "November",
    "December",
];
const MONTHS_ABBR: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];
const DAYS_FULL: [&str; 7] = [
    "Sunday",
    "Monday",
    "Tuesday",
    "Wednesday",
    "Thursday",
    "Friday",
    "Saturday",
];
const DAYS_ABBR: [&str; 7] = ["Sun", "Mon", "Tue", "Wed", "Thu", "Fri", "Sat"];

/// Days since the Unix epoch → civil date (Howard Hinnant's
/// algorithm; proleptic Gregorian, valid for the full `i64` range).
fn civil_from_days(days: i64) -> (i32, u32, u32) {
    let z = days + 719_468;
    let era = z.div_euclid(146_097);
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = if mp < 10 { mp + 3 } else { mp - 9 } as u32;
    let y = if m <= 2 { y + 1 } else { y } as i32;
    (y, m, d)
}

/// Civil date → days since the Unix epoch.
fn days_from_civil(y: i32, m: u32, d: u32) -> i64 {
    let y = if m <= 2 { y as i64 - 1 } else { y as i64 };
    let era = y.div_euclid(400);
    let yoe = (y - era * 400) as u64;
    let mp = (m as u64 + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d as u64 - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe as i64 - 719_468
}

/// The `%t` render inputs, precomputed once per call.
struct TimeParts {
    millis: i64,
    year: i32,
    month: u32,
    day: u32,
    hour: u32,
    minute: u32,
    second: u32,
    ms: u32,
    dow: usize,
    doy: u32,
    hour12: u32,
}

fn time_hour(p: &TimeParts) -> String {
    format!("{:02}", p.hour)
}
fn time_hour12(p: &TimeParts) -> String {
    format!("{:02}", p.hour12)
}
fn time_hour_unpadded(p: &TimeParts) -> String {
    p.hour.to_string()
}
fn time_hour12_unpadded(p: &TimeParts) -> String {
    p.hour12.to_string()
}
fn time_minute(p: &TimeParts) -> String {
    format!("{:02}", p.minute)
}
fn time_second(p: &TimeParts) -> String {
    format!("{:02}", p.second)
}
fn time_millis(p: &TimeParts) -> String {
    format!("{:03}", p.ms)
}
fn time_nanos(p: &TimeParts) -> String {
    format!("{:09}", p.ms * 1_000_000)
}
fn time_ampm(p: &TimeParts) -> String {
    if p.hour < 12 {
        "am".to_owned()
    } else {
        "pm".to_owned()
    }
}
fn time_zone_offset(_: &TimeParts) -> String {
    "+0000".to_owned()
}
fn time_zone_name(_: &TimeParts) -> String {
    "UTC".to_owned()
}
fn time_epoch_secs(p: &TimeParts) -> String {
    // Truncation toward zero (`%ts` of `-1` is `0`).
    (p.millis / 1_000).to_string()
}
fn time_epoch_millis(p: &TimeParts) -> String {
    p.millis.to_string()
}
fn time_month_full(p: &TimeParts) -> String {
    MONTHS_FULL[p.month as usize - 1].to_owned()
}
fn time_month_abbr(p: &TimeParts) -> String {
    MONTHS_ABBR[p.month as usize - 1].to_owned()
}
fn time_day_full(p: &TimeParts) -> String {
    DAYS_FULL[p.dow].to_owned()
}
fn time_day_abbr(p: &TimeParts) -> String {
    DAYS_ABBR[p.dow].to_owned()
}
fn time_century(p: &TimeParts) -> String {
    format!("{:02}", p.year / 100)
}
fn time_year(p: &TimeParts) -> String {
    format_time_year(p.year)
}
fn time_year2(p: &TimeParts) -> String {
    format!("{:02}", p.year.rem_euclid(100))
}
fn time_day_of_year(p: &TimeParts) -> String {
    format!("{:03}", p.doy)
}
fn time_month(p: &TimeParts) -> String {
    format!("{:02}", p.month)
}
fn time_day(p: &TimeParts) -> String {
    format!("{:02}", p.day)
}
fn time_day_unpadded(p: &TimeParts) -> String {
    p.day.to_string()
}
fn time_hhmm(p: &TimeParts) -> String {
    format!("{:02}:{:02}", p.hour, p.minute)
}
fn time_hhmmss(p: &TimeParts) -> String {
    format!("{:02}:{:02}:{:02}", p.hour, p.minute, p.second)
}
fn time_us_date(p: &TimeParts) -> String {
    format!("{:02}/{:02}/{:02}", p.month, p.day, p.year.rem_euclid(100))
}
fn time_iso_date(p: &TimeParts) -> String {
    format!("{}-{:02}-{:02}", format_time_year(p.year), p.month, p.day)
}
fn time_ctime(p: &TimeParts) -> String {
    format!(
        "{} {} {:02} {:02}:{:02}:{:02} UTC {}",
        DAYS_ABBR[p.dow],
        MONTHS_ABBR[p.month as usize - 1],
        p.day,
        p.hour,
        p.minute,
        p.second,
        format_time_year(p.year)
    )
}

/// One validated `%t` suffix and its renderer.
type TimeSuffixRender = (char, fn(&TimeParts) -> String);

/// The validated suffix set (mirrors `is_time_suffix`; the parse
/// rejects anything else, so the lookup in `format_time` cannot
/// miss and needs no fallback arm).
const TIME_SUFFIXES: &[TimeSuffixRender] = &[
    ('H', time_hour),
    ('I', time_hour12),
    ('k', time_hour_unpadded),
    ('l', time_hour12_unpadded),
    ('M', time_minute),
    ('S', time_second),
    ('L', time_millis),
    ('N', time_nanos),
    ('p', time_ampm),
    ('z', time_zone_offset),
    ('Z', time_zone_name),
    ('s', time_epoch_secs),
    ('Q', time_epoch_millis),
    ('B', time_month_full),
    ('b', time_month_abbr),
    ('h', time_month_abbr),
    ('A', time_day_full),
    ('a', time_day_abbr),
    ('C', time_century),
    ('Y', time_year),
    ('y', time_year2),
    ('j', time_day_of_year),
    ('m', time_month),
    ('d', time_day),
    ('e', time_day_unpadded),
    ('R', time_hhmm),
    ('T', time_hhmmss),
    ('D', time_us_date),
    ('F', time_iso_date),
    ('c', time_ctime),
];

/// `%t`/`%T` over epoch millis (UTC only).
fn format_time(millis: i64, suffix: char) -> String {
    let days = millis.div_euclid(86_400_000);
    let time = millis.rem_euclid(86_400_000);
    let (year, month, day) = civil_from_days(days);
    let hour = (time / 3_600_000) as u32;
    let minute = ((time % 3_600_000) / 60_000) as u32;
    let second = ((time % 60_000) / 1_000) as u32;
    let ms = (time % 1_000) as u32;
    // Thursday epoch: Sunday-0 weekday.
    let dow = ((days + 4).rem_euclid(7)) as usize;
    let doy = (days - days_from_civil(year, 1, 1) + 1) as u32;
    let hour12 = match hour % 12 {
        0 => 12,
        h => h,
    };
    let parts = TimeParts {
        millis,
        year,
        month,
        day,
        hour,
        minute,
        second,
        ms,
        dow,
        doy,
        hour12,
    };
    let (_, render) = TIME_SUFFIXES
        .iter()
        .find(|(c, _)| *c == suffix)
        .expect("suffix validated at parse");
    render(&parts)
}

/// `%tY`/`%tF`/`%tc` year (>= 4 digits, zero-padded).
fn format_time_year(year: i32) -> String {
    if year < 0 {
        year.to_string()
    } else {
        format!("{year:04}")
    }
}
