// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The big-number tower past `Int`/`Double`: `BigInt`
//! (`num_bigint::BigInt` directly), `BigDec` (an unscaled `BigInt`
//! plus a decimal scale, like `java.math.BigDecimal`), and exact
//! rationals (`num_rational::BigRational`, always normalized).
//!
//! All conversions are exact unless named otherwise. `BigDec`
//! printing follows `BigDecimal.toString` (plain while the scale
//! is non-negative, `1E+3` engineering form below it); parsing
//! follows `BigDecimal(String)` (digits, one point, one `e`
//! exponent, leading sign).

use std::cmp::Ordering;

use num_bigint::BigInt;
use num_rational::BigRational;
use num_traits::{Pow, Signed, ToPrimitive, Zero};

/// An arbitrary-precision decimal: `unscaled * 10^-scale`, exactly
/// like `java.math.BigDecimal` (scale may be negative).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BigDec {
    unscaled: BigInt,
    scale: i64,
}

/// `BigDecimal` division with no exact decimal result.
pub const NON_TERMINATING: &str =
    "Non-terminating decimal expansion; no exact representable decimal result.";

fn pow10(exp: u32) -> BigInt {
    BigInt::from(10).pow(exp)
}

/// The `BigDecimal(String)` failure for a bad character.
/// Chained so each parse fn stays under the complexity gate.
fn bad_decimal_char(c: char) -> String {
    format!(
        "Character {c} is neither a decimal digit number, decimal point, nor \"e\" notation exponential mark."
    )
}

/// Digits with at most one point: `(int, frac, seen-any-digit)`.
/// Chained so each parse fn stays under the complexity gate.
fn parse_mantissa(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> (String, String, bool) {
    let mut int_digits = String::new();
    let mut frac_digits = String::new();
    let mut seen_point = false;
    let mut seen_digit = false;
    while let Some(&c) = chars.peek() {
        if c.is_ascii_digit() {
            seen_digit = true;
            if seen_point {
                frac_digits.push(c);
            } else {
                int_digits.push(c);
            }
            chars.next();
        } else if c == '.' && !seen_point {
            seen_point = true;
            chars.next();
        } else {
            break;
        }
    }
    (int_digits, frac_digits, seen_digit)
}

/// An `e`/`E` exponent tail (the `e` already consumed): optional
/// sign, then digits. A bare `e` fails with no message and an
/// overflowing one with `Too many nonzero exponent digits`
/// (oracles `bigdec-expe`, `bigdec-exp`). Chained so each parse
/// fn stays under the complexity gate.
fn parse_exp(chars: &mut std::iter::Peekable<std::str::Chars<'_>>) -> Result<i64, String> {
    let mut exp_neg = false;
    match chars.peek() {
        Some('+') => {
            chars.next();
        }
        Some('-') => {
            exp_neg = true;
            chars.next();
        }
        _ => {}
    }
    let mut exp_digits = String::new();
    while let Some(&d) = chars.peek() {
        if !d.is_ascii_digit() {
            return Err(bad_decimal_char(d));
        }
        exp_digits.push(d);
        chars.next();
    }
    if exp_digits.is_empty() {
        return Err(String::new());
    }
    // The JDK counts nonzero exponent digits: eleven digits fail
    // even when the value would fit (`1e99999999999`), while
    // zero-padded ones parse (`1e00000000001` is `1E+1M`).
    let significant = exp_digits.trim_start_matches('0');
    if significant.len() > 10 {
        return Err("Too many nonzero exponent digits.".to_owned());
    }
    // Ten digits always fit `i64`; the mapping only satisfies the type.
    let mut exp = significant.parse::<i64>().unwrap_or(0);
    if exp_neg {
        exp = -exp;
    }
    Ok(exp)
}

/// Assemble the mantissa digits and exponent into the
/// `(unscaled, scale)` pair. Chained so each parse fn stays
/// under the complexity gate.
fn finish_parse(
    int_digits: String,
    frac_digits: &str,
    exp: i64,
    neg: bool,
) -> Result<BigDec, String> {
    let mut digits = int_digits;
    digits.push_str(frac_digits);
    let stripped = digits.trim_start_matches('0');
    let unscaled = if stripped.is_empty() {
        BigInt::ZERO
    } else {
        BigInt::parse_bytes(stripped.as_bytes(), 10).unwrap_or(BigInt::ZERO)
    };
    let unscaled = if neg { -unscaled } else { unscaled };
    let scale = frac_digits.len() as i64 - exp;
    // The scale is an `int` on the JVM: past it the parse fails
    // with `Exponent overflow` (oracles `tower-r2`).
    if scale > i64::from(i32::MAX) || scale < i64::from(i32::MIN) {
        return Err("Exponent overflow.".to_owned());
    }
    Ok(BigDec { unscaled, scale })
}

impl BigDec {
    /// `BigDecimal(long)` / `BigDecimal(BigInteger)`: integer scale.
    #[must_use]
    pub fn from_bigint(n: BigInt) -> Self {
        Self {
            unscaled: n,
            scale: 0,
        }
    }

    /// `BigDecimal.valueOf(double)`: the canonical short rendering,
    /// so `(bigdec 5)` is `5.0M` and infinities fail like the JVM.
    pub fn value_of_f64(d: f64) -> Result<Self, &'static str> {
        if !d.is_finite() {
            return Err("Infinite or NaN");
        }
        Self::parse(&crate::print::java_double_string(d)).map_err(|_| "Infinite or NaN")
    }

    /// `BigDecimal(String)`: optional sign, digits with at most one
    /// point, at most one `e`/`E` exponent. Failures carry the
    /// `BigDecimal(String)` message shape.
    pub fn parse(s: &str) -> Result<Self, String> {
        let mut chars = s.chars().peekable();
        match chars.peek() {
            Some('+') | Some('-') => {
                chars.next();
            }
            _ => {}
        }
        let (int_digits, frac_digits, seen_digit) = parse_mantissa(&mut chars);
        if !seen_digit {
            return Err(match s.chars().find(|c| !matches!(c, '+' | '-')) {
                Some(c) => bad_decimal_char(c),
                None => "Zero-length BigDecimal".to_owned(),
            });
        }
        let mut exp: i64 = 0;
        if let Some(&c) = chars.peek() {
            if c == 'e' || c == 'E' {
                chars.next();
                exp = parse_exp(&mut chars)?;
            } else {
                return Err(bad_decimal_char(c));
            }
        }
        finish_parse(int_digits, &frac_digits, exp, s.starts_with('-'))
    }

    /// `BigDecimal.toString`: plain while the scale is
    /// non-negative, adjusted-exponent form below it.
    #[must_use]
    pub fn to_bigdec_string(&self) -> String {
        let neg = self.unscaled.is_negative();
        let digits = self.unscaled.abs().to_string();
        let mut out = String::new();
        if neg {
            out.push('-');
        }
        if self.scale == 0 {
            out.push_str(&digits);
        } else if self.scale > 0 {
            let scale = self.scale as usize;
            if scale < digits.len() {
                let split = digits.len() - scale;
                out.push_str(&digits[..split]);
                out.push('.');
                out.push_str(&digits[split..]);
            } else {
                out.push_str("0.");
                for _ in 0..(scale - digits.len()) {
                    out.push('0');
                }
                out.push_str(&digits);
            }
        } else {
            // Adjusted exponent: one digit, then the point, then
            // `E<adjusted>` (`1E+3`, `1.50E+5`).
            let adjusted = (-self.scale) + digits.len() as i64 - 1;
            let mut coeff = digits.chars();
            out.push(coeff.next().unwrap_or('0'));
            let rest: String = coeff.collect();
            if !rest.is_empty() {
                out.push('.');
                out.push_str(&rest);
            }
            out.push_str(&format!("E{adjusted:+}"));
        }
        out
    }

    /// `compareTo`: exact, scale-insensitive. Differing signs
    /// (including zero) decide without expansion — this also
    /// keeps astronomical scales cheap, like the JDK's signum
    /// short-circuit. Same-sign pairs compare decimal magnitudes
    /// first; tied magnitudes expand exactly.
    #[must_use]
    pub fn compare(&self, other: &Self) -> Ordering {
        if self.scale == other.scale {
            return self.unscaled.cmp(&other.unscaled);
        }
        let (s, o) = (self.sign(), other.sign());
        if s != o {
            return s.cmp(&o);
        }
        if s == Ordering::Equal {
            return Ordering::Equal;
        }
        let (ms, mo) = (self.magnitude(), other.magnitude());
        if ms != mo {
            return if s == Ordering::Greater {
                ms.cmp(&mo)
            } else {
                mo.cmp(&ms)
            };
        }
        if self.scale > other.scale {
            let factor = pow10(gap_u32(self.scale, other.scale));
            self.unscaled.cmp(&(other.unscaled.clone() * factor))
        } else {
            let factor = pow10(gap_u32(other.scale, self.scale));
            (self.unscaled.clone() * factor).cmp(&other.unscaled)
        }
    }

    /// Decimal magnitude (`digits - scale`): exact ordering for
    /// same-sign pairs, with no expansion.
    fn magnitude(&self) -> i64 {
        self.unscaled.abs().to_string().len() as i64 - self.scale
    }

    /// Trailing-zero-stripped form (hashing: equiv decimals strip
    /// to the same pair — zeros canonicalize to `(0, 0)`, and
    /// trailing zeros strip at any scale sign, like
    /// `stripTrailingZeros`; oracle `tower-r2`).
    #[must_use]
    pub fn stripped(&self) -> (BigInt, i64) {
        if self.unscaled.is_zero() {
            return (BigInt::ZERO, 0);
        }
        let mut unscaled = self.unscaled.clone();
        let mut scale = self.scale;
        while (&unscaled % 10) == BigInt::ZERO {
            unscaled /= 10;
            scale -= 1;
        }
        (unscaled, scale)
    }

    #[must_use]
    pub fn is_zero(&self) -> bool {
        self.unscaled.is_zero()
    }

    #[must_use]
    pub fn sign(&self) -> Ordering {
        if self.unscaled.is_zero() {
            Ordering::Equal
        } else if self.unscaled.is_negative() {
            Ordering::Less
        } else {
            Ordering::Greater
        }
    }

    #[must_use]
    pub fn neg(&self) -> Self {
        Self {
            unscaled: -self.unscaled.clone(),
            scale: self.scale,
        }
    }

    /// Exact rational value (`unscaled / 10^scale`).
    #[must_use]
    pub fn to_rational(&self) -> BigRational {
        if self.scale >= 0 {
            BigRational::new(self.unscaled.clone(), pow10(gap_u32(self.scale, 0)))
        } else {
            BigRational::from_integer(self.unscaled.clone() * pow10(gap_u32(0, self.scale)))
        }
    }

    /// Exact decimal for a rational, or the non-terminating error.
    /// The result is minimal-scale (`1/2` → `0.5M`, like the JVM
    /// exact division).
    pub fn from_rational_exact(r: &BigRational) -> Result<Self, &'static str> {
        let mut numer = r.numer().clone();
        let mut denom = r.denom().clone();
        let mut scale: i64 = 0;
        while &denom % 2 == BigInt::ZERO {
            numer *= 5;
            denom /= 2;
            scale += 1;
        }
        while &denom % 5 == BigInt::ZERO {
            numer *= 2;
            denom /= 5;
            scale += 1;
        }
        if denom != BigInt::from(1) {
            return Err(NON_TERMINATING);
        }
        let out = Self {
            unscaled: numer,
            scale,
        };
        let (unscaled, scale) = out.stripped();
        Ok(Self { unscaled, scale })
    }

    /// `toBigInteger`: fraction discarded toward zero.
    #[must_use]
    pub fn to_bigint_trunc(&self) -> BigInt {
        if self.scale >= 0 {
            self.unscaled.clone() / pow10(gap_u32(self.scale, 0))
        } else {
            self.unscaled.clone() * pow10(gap_u32(0, self.scale))
        }
    }

    /// Approximate `f64` (overflows to infinity, like the JVM).
    /// Astronomical scales never expand: past `10^1000` the value
    /// is overflow (`±inf`) or underflow (`±0.0`) by inspection.
    #[must_use]
    pub fn to_f64(&self) -> f64 {
        if !self.unscaled.is_zero() {
            let neg = self.sign() == Ordering::Less;
            if self.scale < -1000 {
                return if neg {
                    f64::NEG_INFINITY
                } else {
                    f64::INFINITY
                };
            }
            let digits = self.unscaled.abs().to_string().len() as i64;
            if self.scale - digits > 1000 {
                return if neg { -0.0 } else { 0.0 };
            }
        }
        self.to_rational().to_f64().unwrap_or(f64::NAN)
    }

    fn aligned(&self, other: &Self) -> (BigInt, BigInt, i64) {
        if self.scale >= other.scale {
            let factor = pow10(gap_u32(self.scale, other.scale));
            (
                self.unscaled.clone(),
                other.unscaled.clone() * factor,
                self.scale,
            )
        } else {
            let factor = pow10(gap_u32(other.scale, self.scale));
            (
                self.unscaled.clone() * factor,
                other.unscaled.clone(),
                other.scale,
            )
        }
    }

    #[must_use]
    pub fn add(&self, other: &Self) -> Self {
        let (a, b, scale) = self.aligned(other);
        Self {
            unscaled: a + b,
            scale,
        }
    }

    #[must_use]
    pub fn sub(&self, other: &Self) -> Self {
        let (a, b, scale) = self.aligned(other);
        Self {
            unscaled: a - b,
            scale,
        }
    }

    #[must_use]
    pub fn mul(&self, other: &Self) -> Self {
        Self {
            unscaled: self.unscaled.clone() * other.unscaled.clone(),
            scale: self.scale + other.scale,
        }
    }

    /// Exact quotient, or the non-terminating error.
    pub fn div_exact(&self, other: &Self) -> Result<Self, &'static str> {
        if other.is_zero() {
            return Err("Divide by zero");
        }
        Self::from_rational_exact(&(self.to_rational() / other.to_rational()))
    }

    /// `BigDecimal.remainder`: exact, at the wider scale
    /// (callers guard the zero divisor).
    #[must_use]
    pub fn rem(&self, other: &Self) -> Self {
        let (a, b, scale) = self.aligned(other);
        Self {
            unscaled: a % b,
            scale,
        }
    }

    /// Truncating quotient toward zero at integer scale
    /// (`(quot 5M 2)` → `2M`).
    #[must_use]
    pub fn quot_trunc(&self, other: &Self) -> Self {
        let q = self.to_rational() / other.to_rational();
        let trunc = q.numer() / q.denom();
        Self {
            unscaled: trunc,
            scale: 0,
        }
    }
}

/// A scale gap as `u32` for exact `pow10` expansion. Gaps past
/// `u32::MAX` need multi-GB digit counts (only reachable through
/// arithmetic-grown scales — literals cap at `i32`), where the
/// JVM exhausts memory too, so expansion panics loud instead of
/// wrapping silently.
fn gap_u32(hi: i64, lo: i64) -> u32 {
    match u32::try_from(hi.saturating_sub(lo)) {
        Ok(g) => g,
        Err(_) => panic!("BigDec scale gap exceeds u32::MAX; exact expansion infeasible"),
    }
}

/// `BigInteger(String)`: decimal with a leading sign, JVM message.
/// Leading zeros are skipped first (like the JDK), so
/// `(bigint "0x10")` fails naming `"x10"` (oracle
/// `bigint-hexstr`); an all-zero remainder is zero, while a bare
/// sign (or nothing) fails with `Zero length BigInteger`
/// (oracle `tower-gap`). Only one leading sign is stripped: a
/// second one fails with `Illegal embedded sign character`
/// (oracle `tower-r2`).
pub fn bigint_parse(s: &str) -> Result<BigInt, String> {
    // One leading sign at most (`+`/`-` are one byte, so the
    // split is always on a boundary).
    let signed = match s.as_bytes().first() {
        Some(b'+') | Some(b'-') => &s[1..],
        _ => s,
    };
    let digits = signed.trim_start_matches('0');
    if digits.is_empty() {
        // All zeros is zero — but a bare sign (or nothing) is the
        // JDK's zero-length failure.
        if !signed.is_empty() {
            return Ok(BigInt::ZERO);
        }
        return Err("Zero length BigInteger".to_owned());
    }
    if signed.bytes().any(|b| b == b'+' || b == b'-') {
        return Err("Illegal embedded sign character".to_owned());
    }
    if !digits.bytes().all(|b| b.is_ascii_digit()) {
        return Err(format!("For input string: \"{digits}\""));
    }
    BigInt::parse_bytes(s.as_bytes(), 10).ok_or_else(|| format!("For input string: \"{digits}\""))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parse_keeps_scale() {
        assert_eq!(BigDec::parse("5").unwrap().to_bigdec_string(), "5");
        assert_eq!(BigDec::parse("5.0").unwrap().to_bigdec_string(), "5.0");
        assert_eq!(BigDec::parse("1e3").unwrap().to_bigdec_string(), "1E+3");
        assert_eq!(
            BigDec::parse("1.5e-3").unwrap().to_bigdec_string(),
            "0.0015"
        );
        assert_eq!(BigDec::parse("+5").unwrap().to_bigdec_string(), "5");
        assert_eq!(BigDec::parse(".5").unwrap().to_bigdec_string(), "0.5");
    }

    #[test]
    fn parse_rejects_like_bigdecimal() {
        assert!(BigDec::parse("x").is_err());
        assert!(BigDec::parse("").is_err());
        assert!(BigDec::parse("1e").is_err());
        assert!(BigDec::parse("--5").is_err());
    }

    #[test]
    fn value_of_f64_matches_shortest_repr() {
        assert_eq!(BigDec::value_of_f64(5.0).unwrap().to_bigdec_string(), "5.0");
        assert!(BigDec::value_of_f64(f64::NAN).is_err());
        assert!(BigDec::value_of_f64(f64::INFINITY).is_err());
    }

    #[test]
    fn rationals_round_trip() {
        let half = BigRational::new(BigInt::from(1), BigInt::from(2));
        assert_eq!(
            BigDec::from_rational_exact(&half)
                .unwrap()
                .to_bigdec_string(),
            "0.5"
        );
        let third = BigRational::new(BigInt::from(1), BigInt::from(3));
        assert_eq!(
            BigDec::from_rational_exact(&third).unwrap_err(),
            NON_TERMINATING
        );
        assert_eq!(
            BigDec::parse("5.1").unwrap().to_rational(),
            BigRational::new(BigInt::from(51), BigInt::from(10))
        );
    }

    #[test]
    fn bigdec_sign_pins_zero() {
        // `BigDecimal.signum`: zero (any scale) is `Equal`
        // (oracle `tower-gap`); the nonzero arms ride the
        // `bigdec_mod` suite.
        assert_eq!(BigDec::parse("0.0").unwrap().sign(), Ordering::Equal);
        assert_eq!(BigDec::parse("-2.5").unwrap().sign(), Ordering::Less);
        assert_eq!(BigDec::parse("3").unwrap().sign(), Ordering::Greater);
    }

    /// Arithmetic-grown scales (literals cap at `i32`, products do
    /// not): exact expansion past `u32` panics loud instead of
    /// wrapping silently (the JVM exhausts memory there).
    fn monster() -> BigDec {
        let e2 = BigDec::parse("1E+2000000000").unwrap();
        e2.mul(&e2).mul(&e2)
    }

    #[test]
    #[should_panic(expected = "scale gap")]
    fn monster_add_panics_loud() {
        let _ = monster().add(&BigDec::parse("1").unwrap());
    }

    #[test]
    #[should_panic(expected = "scale gap")]
    fn monster_to_rational_panics_loud() {
        let _ = monster().to_rational();
    }

    #[test]
    #[should_panic(expected = "scale gap")]
    fn monster_to_bigint_trunc_panics_loud() {
        let _ = monster().to_bigint_trunc();
    }
}
