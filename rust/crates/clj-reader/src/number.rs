// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Numbers (`reader-contract.md` §5).
//!
//! `readNumber` accumulates until EOF/whitespace/any macro (even
//! `(`, so `1(2)` reads `1` then a list), then `matchNumber`: the
//! `intPat` match short-circuits (the `0[0-9]+` poison alternative
//! fails invalid octal as `Invalid number`), else `floatPat` (`M`
//! defers), else `ratioPat` (defers — shape recognized, never
//! `Invalid number`), else `Invalid number`. Radix errors replicate
//! the JDK `BigInteger` messages exactly (radix 10 omits the
//! `under radix` suffix); radix digits eat a trailing `N` greedily
//! (`36r1N` is an `Int`, `2r1N` errors on `"1N"`). Overflow past
//! `i64` defers to BigInt. All in-range integers are `Int` (the
//! JVM reader likewise always yields `Long`, even int-sized —
//! probe17 — so no `Integer` split exists).

use clj_value::Value;
use num_traits::{ToPrimitive, Zero};

use crate::stream::CharStream;
use crate::symbol::{is_macro, is_whitespace};

/// A matched number literal (deferrals carry no payload; the caller
/// has the token for the message).
#[derive(Clone, Debug, PartialEq)]
pub enum NumLit {
    /// In-range integer.
    Int(i64),
    /// Double (`f64` parse matches `Double.parseDouble` here).
    Double(f64),
    /// BigInt deferral (N3).
    BigInt,
    /// BigDecimal deferral (N3).
    BigDecimal,
    /// Ratio deferral (N3).
    Ratio,
}

/// A failed number match (payloads for the message; the caller adds
/// the token and span).
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum NumErr {
    /// `Invalid number`.
    InvalidNumber,
    /// `Radix out of range`.
    RadixRange,
    /// Bad radix digits (full digits part plus radix).
    RadixDigit { digits: String, radix: u32 },
}

/// Accumulate a number token from `initch` (see module docs).
pub fn read_number_token(stream: &mut CharStream, initch: char) -> String {
    let mut token = String::from(initch);
    loop {
        match stream.read() {
            None => return token,
            Some(c) if is_whitespace(c) || is_macro(c) => {
                stream.unread(c);
                return token;
            }
            Some(c) => token.push(c),
        }
    }
}

/// `matchNumber` (see module docs for the order and short-circuit).
pub fn match_number(s: &str) -> Result<NumLit, NumErr> {
    if let Some(r) = match_int(s) {
        return r;
    }
    if let Some(r) = match_float(s) {
        return r;
    }
    if is_ratio(s) {
        return Ok(NumLit::Ratio);
    }
    Err(NumErr::InvalidNumber)
}

/// Convert a matched literal to a [`Value`] (deferrals never reach
/// here — the reader raises them first).
#[must_use]
pub fn lit_value(lit: &NumLit) -> Value {
    match lit {
        NumLit::Int(i) => Value::Int(*i),
        NumLit::Double(d) => Value::Double(*d),
        NumLit::BigInt | NumLit::BigDecimal | NumLit::Ratio => {
            unreachable!("deferrals raise before conversion")
        }
    }
}

/// A `BigInt`-accepted token as a value. Shapes are pre-validated
/// by [`match_number`] (decimal/`N`, hex, octal, radix, sign), so
/// parsing cannot fail: `expect` documents the invariant
/// coverably (the `floatPat` precedent above).
#[must_use]
pub fn bigint_token_value(token: &str) -> Value {
    let (neg, rest) = match token.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, token.strip_prefix('+').unwrap_or(token)),
    };
    let body = rest.strip_suffix('N').unwrap_or(rest);
    let magnitude: num_bigint::BigInt =
        if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
            parse_radix(hex, 16)
        } else if let Some(oct) = body.strip_prefix('0') {
            if oct.is_empty() {
                num_bigint::BigInt::ZERO
            } else {
                parse_radix(oct, 8)
            }
        } else if let Some(i) = body.find(['r', 'R']) {
            let radix: u32 = body[..i]
                .parse()
                .expect("radix already validated by match_number");
            parse_radix(&body[i + 1..], radix)
        } else {
            parse_radix(body, 10)
        };
    Value::BigInt(if neg { -magnitude } else { magnitude })
}

fn parse_radix(digits: &str, radix: u32) -> num_bigint::BigInt {
    num_bigint::BigInt::parse_bytes(digits.as_bytes(), radix)
        .expect("digits already validated by match_number")
}

/// A `BigDecimal`-accepted token as a value (the `M` suffix is
/// stripped; the grammar is pre-validated, same coverability
/// argument as above).
#[must_use]
pub fn bigdec_token_value(token: &str) -> Value {
    let body = token.strip_suffix('M').unwrap_or(token);
    Value::BigDec(
        clj_value::BigDec::parse(body).expect("grammar already validated by match_number"),
    )
}

/// A ratio token: divisible ratios reduce to `Int`/`BigInt`
/// (oracle `4/2` → `2`), a zero denominator is the reader
/// `Divide by zero`.
pub enum RatioToken {
    Value(Value),
    ZeroDenominator,
}

/// A `ratioPat`-accepted token as a value (`is_ratio`
/// pre-validated the digit shapes).
#[must_use]
pub fn ratio_token_value(token: &str) -> RatioToken {
    let rest = token.strip_prefix(['+', '-']).unwrap_or(token);
    let slash = rest
        .find('/')
        .expect("slash already validated by match_number");
    let (num, den) = (&rest[..slash], &rest[slash + 1..]);
    let numer = num_bigint::BigInt::parse_bytes(num.as_bytes(), 10)
        .expect("numerator already validated by match_number");
    let denom = num_bigint::BigInt::parse_bytes(den.as_bytes(), 10)
        .expect("denominator already validated by match_number");
    if denom.is_zero() {
        return RatioToken::ZeroDenominator;
    }
    let neg = token.starts_with('-');
    let (numer, denom) = if neg { (-numer, denom) } else { (numer, denom) };
    if numer.clone() % denom.clone() == num_bigint::BigInt::ZERO {
        let q = numer / denom;
        return RatioToken::Value(match q.to_i64() {
            Some(i) => Value::Int(i),
            None => Value::BigInt(q),
        });
    }
    RatioToken::Value(Value::Ratio(num_rational::BigRational::new(numer, denom)))
}

fn strip_sign(s: &str) -> (bool, &str) {
    if let Some(rest) = s.strip_prefix('-') {
        (true, rest)
    } else if let Some(rest) = s.strip_prefix('+') {
        (false, rest)
    } else {
        (false, s)
    }
}

/// `intPat`, `Some` when it matches (short-circuiting float/ratio).
/// `None` means no match — fall through to `floatPat`.
fn match_int(s: &str) -> Option<Result<NumLit, NumErr>> {
    let (neg, rest) = strip_sign(s);
    // Radix (`r`/`R` cannot appear in any other alternative, float,
    // or ratio, so its presence decides int-vs-invalid here).
    if rest.contains(['r', 'R']) {
        return Some(try_radix(rest, neg).unwrap_or(Err(NumErr::InvalidNumber)));
    }
    let (body, has_n) = match rest.strip_suffix('N') {
        Some(b) => (b, true),
        None => (rest, false),
    };
    if let Some(r) = match_int_zero_prefixed(body, neg, has_n) {
        return Some(r);
    }
    // Decimal.
    if is_dec(body) {
        return Some(int_value(body, 10, neg, has_n));
    }
    None
}

/// `intPat` zero-prefixed alternatives: `(0)`, hex, octal, and the
/// `0[0-9]+` poison. `None` falls through to decimal/float.
fn match_int_zero_prefixed(body: &str, neg: bool, has_n: bool) -> Option<Result<NumLit, NumErr>> {
    // `(0)`, alone or with `N`.
    if body == "0" {
        return Some(if has_n {
            Ok(NumLit::BigInt)
        } else {
            Ok(NumLit::Int(0))
        });
    }
    // Hex.
    if let Some(hex) = body.strip_prefix("0x").or_else(|| body.strip_prefix("0X")) {
        if !hex.is_empty() && hex.bytes().all(|b| b.is_ascii_hexdigit()) {
            return Some(int_value(hex, 16, neg, has_n));
        }
        return None;
    }
    // Octal (`00` takes this path, not the poison one).
    if let Some(oct) = body.strip_prefix('0') {
        if !oct.is_empty() && oct.bytes().all(|b| matches!(b, b'0'..=b'7')) {
            return Some(int_value(oct, 8, neg, has_n));
        }
        // `0[0-9]+` poison: matches so the token fails as invalid.
        if !oct.is_empty() && oct.bytes().all(|b| b.is_ascii_digit()) {
            return Some(Err(NumErr::InvalidNumber));
        }
        return None;
    }
    None
}

fn is_dec(s: &str) -> bool {
    let mut bytes = s.bytes();
    matches!(bytes.next(), Some(b'1'..=b'9')) && bytes.all(|b| b.is_ascii_digit())
}

/// Radix prefix `[1-9][0-9]?` (1–99).
fn is_radix_prefix(prefix: &str) -> bool {
    let bytes = prefix.as_bytes();
    (bytes.len() == 1 || bytes.len() == 2)
        && matches!(bytes[0], b'1'..=b'9')
        && (bytes.len() == 1 || bytes[1].is_ascii_digit())
}

/// `([1-9][0-9]?)[rR]([0-9A-Za-z]+)`: `None` when the shape does not
/// match; digits are greedy (a trailing `N` is a digit, never the
/// suffix).
fn try_radix(rest: &str, neg: bool) -> Option<Result<NumLit, NumErr>> {
    let i = rest.find(['r', 'R'])?;
    let (prefix, after) = rest.split_at(i);
    let digits = &after[1..];
    if !is_radix_prefix(prefix) {
        return None;
    }
    if digits.is_empty() || !digits.bytes().all(|b| b.is_ascii_alphanumeric()) {
        return None;
    }
    let radix: u32 = prefix.parse().ok()?;
    if !(2..=36).contains(&radix) {
        return Some(Err(NumErr::RadixRange));
    }
    // Radix paths never see the `N` suffix (greedy digits ate it).
    Some(int_value_radix(digits, radix, neg))
}

/// Validate radix digits (JDK message order: range, then digits) and
/// accumulate the magnitude.
fn int_value_radix(digits: &str, radix: u32, neg: bool) -> Result<NumLit, NumErr> {
    for c in digits.chars() {
        if c.to_digit(radix).is_none() {
            return Err(NumErr::RadixDigit {
                digits: digits.to_owned(),
                radix,
            });
        }
    }
    let values = digits
        .chars()
        .map(|c| u128::from(c.to_digit(radix).unwrap()));
    Ok(accumulate(values, u128::from(radix), neg, false))
}

fn digit_value(c: char, radix: u32) -> u32 {
    c.to_digit(radix).unwrap_or(0)
}

/// Accumulate validated digits in `u128` with early-out past `i64`
/// (never panics); `has_n` forces the BigInt deferral.
fn int_value(digits: &str, radix: u32, neg: bool, has_n: bool) -> Result<NumLit, NumErr> {
    let values = digits.chars().map(|c| u128::from(digit_value(c, radix)));
    Ok(accumulate(values, u128::from(radix), neg, has_n))
}

fn accumulate(values: impl Iterator<Item = u128>, radix: u128, neg: bool, has_n: bool) -> NumLit {
    // `bitLength < 64` ⟺ fits in `i64` (two's complement: `-2^63` ok).
    let limit: u128 = if neg {
        0x8000_0000_0000_0000
    } else {
        i64::MAX as u128
    };
    let mut acc: u128 = 0;
    for d in values {
        acc = acc
            .checked_mul(radix)
            .and_then(|a| a.checked_add(d))
            .unwrap_or(u128::MAX);
        if acc > limit {
            return NumLit::BigInt;
        }
    }
    if has_n {
        return NumLit::BigInt;
    }
    let magnitude = i64::try_from(acc).unwrap_or(i64::MIN);
    NumLit::Int(if neg {
        magnitude.wrapping_neg()
    } else {
        magnitude
    })
}

/// `floatPat = ([-+]?[0-9]+(\.[0-9]*)?([eE][-+]?[0-9]+)?)(M)?`, full
/// match; `None` means no match.
fn match_float(s: &str) -> Option<Result<NumLit, NumErr>> {
    let rest = s.strip_prefix(['+', '-']).unwrap_or(s);
    let (_, mut rest) = split_digits(rest)?;
    if let Some(frac) = rest.strip_prefix('.') {
        rest = split_digits_or_empty(frac);
    }
    if let Some(exp) = rest.strip_prefix(['e', 'E']) {
        let exp = exp.strip_prefix(['+', '-']).unwrap_or(exp);
        (_, rest) = split_digits(exp)?;
    }
    let (has_m, rest) = match rest.strip_prefix('M') {
        Some(r) => (true, r),
        None => (false, rest),
    };
    if !rest.is_empty() {
        return None;
    }
    if has_m {
        return Some(Ok(NumLit::BigDecimal));
    }
    // `floatPat` matches a strict subset of the `f64` grammar
    // (overflow saturates to inf, underflow to 0 — verified
    // against `str::parse`), so parsing cannot fail: `expect`
    // documents the invariant coverably (100% line gate).
    Some(Ok(NumLit::Double(
        s.parse::<f64>().expect("floatPat matches f64 grammar"),
    )))
}

/// Split leading ASCII digits (≥1) from the rest.
fn split_digits(s: &str) -> Option<(&str, &str)> {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    if end == 0 {
        return None;
    }
    Some(s.split_at(end))
}

/// Split leading ASCII digits (possibly none) from the rest.
fn split_digits_or_empty(s: &str) -> &str {
    let end = s.bytes().take_while(u8::is_ascii_digit).count();
    &s[end..]
}

/// `ratioPat = ([-+]?[0-9]+)/([0-9]+)`, full match.
fn is_ratio(s: &str) -> bool {
    let rest = s.strip_prefix(['+', '-']).unwrap_or(s);
    let Some(slash) = rest.find('/') else {
        return false;
    };
    let (num, den) = (&rest[..slash], &rest[slash + 1..]);
    !num.is_empty()
        && num.bytes().all(|b| b.is_ascii_digit())
        && !den.is_empty()
        && den.bytes().all(|b| b.is_ascii_digit())
}

#[cfg(test)]
mod tests {
    use super::{NumErr, NumLit, lit_value, match_number};
    use clj_value::Value;

    fn num(s: &str) -> Result<NumLit, NumErr> {
        match_number(s)
    }

    #[test]
    fn integers() {
        assert_eq!(num("0"), Ok(NumLit::Int(0)));
        assert_eq!(num("00"), Ok(NumLit::Int(0)));
        assert_eq!(num("+42"), Ok(NumLit::Int(42)));
        assert_eq!(num("017"), Ok(NumLit::Int(15)));
        assert_eq!(num("007"), Ok(NumLit::Int(7)));
        assert_eq!(num("0xFF"), Ok(NumLit::Int(255)));
        assert_eq!(num("-0x10"), Ok(NumLit::Int(-16)));
        assert_eq!(num("0X1F"), Ok(NumLit::Int(31)));
        assert_eq!(num("8r17"), Ok(NumLit::Int(15)));
        assert_eq!(num("2r101"), Ok(NumLit::Int(5)));
        assert_eq!(num("2R10"), Ok(NumLit::Int(2)));
        assert_eq!(num("10r99"), Ok(NumLit::Int(99)));
        assert_eq!(num("36rzz"), Ok(NumLit::Int(1295)));
        assert_eq!(num("36rZZ"), Ok(NumLit::Int(1295)));
        assert_eq!(num("9r8"), Ok(NumLit::Int(8)));
        assert_eq!(num("+2r10"), Ok(NumLit::Int(2)));
        assert_eq!(num("-2r10"), Ok(NumLit::Int(-2)));
        assert_eq!(num("+0x10"), Ok(NumLit::Int(16)));
        assert_eq!(num("-0"), Ok(NumLit::Int(0)));
        // Greedy digits eat the N: values, not BigInt.
        assert_eq!(num("36r1N"), Ok(NumLit::Int(59)));
        assert_eq!(num("36rZZN"), Ok(NumLit::Int(46643)));
    }

    #[test]
    fn boundaries_and_bigint_deferrals() {
        assert_eq!(num("9223372036854775807"), Ok(NumLit::Int(i64::MAX)));
        assert_eq!(num("-9223372036854775808"), Ok(NumLit::Int(i64::MIN)));
        assert_eq!(num("0x7FFFFFFFFFFFFFFF"), Ok(NumLit::Int(i64::MAX)));
        assert_eq!(num("-0x8000000000000000"), Ok(NumLit::Int(i64::MIN)));
        for s in [
            "9223372036854775808",
            "-9223372036854775809",
            "0xFFFFFFFFFFFFFFFF",
            "0x8000000000000000",
            "0x10000000000000000",
            "12345678901234567890123",
            "1N",
            "00N",
            "0N",
            "-0N",
            "2r1000000000000000000000000000000000000000000000000000000000000000",
        ] {
            assert_eq!(num(s), Ok(NumLit::BigInt), "{s}");
        }
        assert_eq!(
            num("2r111111111111111111111111111111111111111111111111111111111111111"),
            Ok(NumLit::Int(i64::MAX))
        );
        assert_eq!(
            num("-2r1000000000000000000000000000000000000000000000000000000000000000"),
            Ok(NumLit::Int(i64::MIN))
        );
        // One zero short of MIN is exactly `-2^62` (oracle-pinned).
        assert_eq!(
            num("-2r100000000000000000000000000000000000000000000000000000000000000"),
            Ok(NumLit::Int(-4611686018427387904))
        );
    }

    #[test]
    fn radix_errors() {
        assert_eq!(num("37r10"), Err(NumErr::RadixRange));
        assert_eq!(num("1r1"), Err(NumErr::RadixRange));
        assert_eq!(
            num("2r2"),
            Err(NumErr::RadixDigit {
                digits: "2".to_owned(),
                radix: 2
            })
        );
        // Greedy: the N joins the digits.
        assert_eq!(
            num("2r1N"),
            Err(NumErr::RadixDigit {
                digits: "1N".to_owned(),
                radix: 2
            })
        );
        assert_eq!(
            num("10r5N"),
            Err(NumErr::RadixDigit {
                digits: "5N".to_owned(),
                radix: 10
            })
        );
    }

    #[test]
    fn floats_and_bigdecimal_deferrals() {
        assert_eq!(num("5."), Ok(NumLit::Double(5.0)));
        assert_eq!(num("0.0"), Ok(NumLit::Double(0.0)));
        assert_eq!(num("-0.0"), Ok(NumLit::Double(-0.0)));
        assert_eq!(num("1e5"), Ok(NumLit::Double(100_000.0)));
        assert_eq!(num("1E5"), Ok(NumLit::Double(100_000.0)));
        assert_eq!(num("1e309"), Ok(NumLit::Double(f64::INFINITY)));
        assert_eq!(num("5.e3"), Ok(NumLit::Double(5000.0)));
        assert_eq!(num("01.5"), Ok(NumLit::Double(1.5)));
        assert_eq!(num("00.5"), Ok(NumLit::Double(0.5)));
        assert_eq!(num("2.5e-7"), Ok(NumLit::Double(2.5e-7)));
        for s in ["1M", "3.14M", "0M", "00M", "1e3M", "-0M", "5.M"] {
            assert_eq!(num(s), Ok(NumLit::BigDecimal), "{s}");
        }
    }

    #[test]
    fn ratios_recognized() {
        for s in [
            "1/2", "1/0", "2/4", "-3/9", "0/5", "5/0", "+1/2", "-0/5", "007/2", "10/4",
        ] {
            assert_eq!(num(s), Ok(NumLit::Ratio), "{s}");
        }
    }

    #[test]
    fn invalid_numbers() {
        for s in [
            "08", "09", "08N", "018", "1a", "1e", "1e+", "1N2", "0x", "0r1", "10r", "1r", "200r10",
            "0xG", "--5", "++5", "-5-", "+5-", "5.e", "5a", "1/2x", "5.5.5", "1:2", "36r!", "2r",
            "10r5N!", "0xFFG",
        ] {
            assert_eq!(num(s), Err(NumErr::InvalidNumber), "{s}");
        }
    }

    #[test]
    fn lit_value_converts_scalars() {
        assert_eq!(lit_value(&NumLit::Int(-7)), Value::Int(-7));
        assert_eq!(lit_value(&NumLit::Double(2.5)), Value::Double(2.5));
    }

    #[test]
    #[should_panic(expected = "deferrals raise before conversion")]
    fn lit_value_panics_on_deferrals() {
        // The reader raises before converting, so direct
        // invocation is the only trigger.
        let _ = lit_value(&NumLit::Ratio);
    }
}
