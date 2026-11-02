// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `java.util.Random` and `java.security.SecureRandom` (#205).
//!
//! `Random` is the JDK's 48-bit linear congruential generator,
//! transcribed exactly (seed scrambling, the `nextInt(bound)`
//! rejection loop, the cached Box-Muller gaussian): seeded
//! streams are bit-identical to the JVM, which the algorithms
//! suite's seeded tests rely on. The gaussian tail uses the
//! platform `sqrt`/`ln` (the JDK specifies `StrictMath` there —
//! a last-ulp narrowing the suite's statistical assertions
//! tolerate, pinned in the cover test). Unseeded construction
//! and `SecureRandom` draw from the port's `rand` dependency
//! (no seed surface — out of the suite's reach).

use std::sync::Mutex;
use std::sync::atomic::{AtomicU64, Ordering};

use clj_value::{ArrayStore, ExtensionValue, ThrowClass, Value};
use rand::{Rng, SeedableRng};

use crate::error::NativeError;

/// The LCG multiplier, addend, and 48-bit mask (verbatim).
const MULTIPLIER: u64 = 0x5DEECE66D;
const ADDEND: u64 = 0xB;
const MASK: u64 = (1 << 48) - 1;

/// `1.0 / 2^53` (the `nextDouble` unit).
const DOUBLE_UNIT: f64 = 1.1102230246251565e-16;

/// A `java.util.Random`: identity plus the mutable 48-bit seed
/// and cached gaussian (interior mutability — instance methods
/// take `&self`, like `StringBuilder`).
#[derive(Debug)]
pub(crate) struct EmuRandom {
    id: u64,
    state: Mutex<RandomState>,
}

/// The mutable half (split so construction stays total).
#[derive(Debug)]
struct RandomState {
    seed: u64,
    next_gaussian: Option<f64>,
}

static NEXT_RANDOM: AtomicU64 = AtomicU64::new(1);

impl EmuRandom {
    /// A generator over `seed` (scrambled, like the `(long)` ctor).
    pub(crate) fn seeded(seed: i64) -> Self {
        Self {
            id: NEXT_RANDOM.fetch_add(1, Ordering::Relaxed),
            state: Mutex::new(RandomState {
                seed: (seed as u64 ^ MULTIPLIER) & MASK,
                next_gaussian: None,
            }),
        }
    }

    /// A nondeterministically seeded generator (the no-arg ctor).
    pub(crate) fn fresh() -> Self {
        Self::seeded(rand::rng().random::<i64>())
    }

    /// The raw `next(bits)` step (callers hold no other locks).
    fn next_bits(state: &mut RandomState, bits: u32) -> i32 {
        state.seed = state.seed.wrapping_mul(MULTIPLIER).wrapping_add(ADDEND) & MASK;
        (state.seed >> (48 - bits)) as i32
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, RandomState> {
        self.state.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `nextInt()`: 32 raw bits.
    fn next_int(&self) -> i32 {
        Self::next_bits(&mut self.lock(), 32)
    }

    /// `nextInt(bound)`: the power-of-two fast path plus the
    /// rejection loop (`wrapping` throughout, like the JDK's
    /// ints — the debug build must not panic on the carry).
    fn next_int_bound(&self, bound: i32) -> Result<i32, NativeError> {
        if bound <= 0 {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "bound must be positive".to_owned(),
            ));
        }
        let mut state = self.lock();
        if (bound.wrapping_neg() & bound) == bound {
            let bits = Self::next_bits(&mut state, 31);
            return Ok(((i64::from(bound) * i64::from(bits)) >> 31) as i32);
        }
        loop {
            let bits = Self::next_bits(&mut state, 31);
            let val = bits % bound;
            if (bits - val).wrapping_add(bound - 1) >= 0 {
                return Ok(val);
            }
        }
    }

    /// `nextDouble()`: 53 bits over `2^53`.
    fn next_double(&self) -> f64 {
        Self::next_double_locked(&mut self.lock())
    }

    /// The double draw over an already-held state.
    fn next_double_locked(state: &mut RandomState) -> f64 {
        let hi = Self::next_bits(state, 26) as i64;
        let lo = Self::next_bits(state, 27) as i64;
        ((hi << 27) + lo) as f64 * DOUBLE_UNIT
    }

    /// `nextGaussian()`: cached Box-Muller (polar form).
    fn next_gaussian(&self) -> f64 {
        let mut state = self.lock();
        if let Some(g) = state.next_gaussian.take() {
            return g;
        }
        loop {
            let v1 = 2.0 * Self::next_double_locked(&mut state) - 1.0;
            let v2 = 2.0 * Self::next_double_locked(&mut state) - 1.0;
            let s = v1 * v1 + v2 * v2;
            if s < 1.0 && s != 0.0 {
                let m = (-2.0 * s.ln() / s).sqrt();
                state.next_gaussian = Some(v2 * m);
                return v1 * m;
            }
        }
    }

    /// `nextBytes(bytes)`: four `nextInt` bytes per draw.
    fn next_bytes(&self, bytes: &mut [i8]) {
        let mut state = self.lock();
        let mut i = 0;
        while i < bytes.len() {
            let mut rnd = Self::next_bits(&mut state, 32);
            let n = (bytes.len() - i).min(4);
            for k in 0..n {
                bytes[i + k] = rnd as i8;
                rnd >>= 8;
            }
            i += n;
        }
    }
}

impl ExtensionValue for EmuRandom {
    fn class_name(&self) -> String {
        "java.util.Random".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.util.Random 0x{:x} \"java.util.Random@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuRandom>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A `java.security.SecureRandom`: unseeded OS-chaired draws
/// (the suite only needs `nextInt(bound)` and `nextBytes`).
#[derive(Debug)]
pub(crate) struct EmuSecureRandom {
    id: u64,
    rng: Mutex<rand::rngs::StdRng>,
}

static NEXT_SECURE: AtomicU64 = AtomicU64::new(1);

impl EmuSecureRandom {
    pub(crate) fn fresh() -> Self {
        Self {
            id: NEXT_SECURE.fetch_add(1, Ordering::Relaxed),
            rng: Mutex::new(rand::rngs::StdRng::from_os_rng()),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, rand::rngs::StdRng> {
        self.rng.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn next_int_bound(&self, bound: i32) -> Result<i32, NativeError> {
        if bound <= 0 {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "bound must be positive".to_owned(),
            ));
        }
        Ok(self.lock().random_range(0..bound))
    }

    fn next_bytes(&self, bytes: &mut [i8]) {
        let mut rng = self.lock();
        for b in bytes.iter_mut() {
            *b = rng.random::<i8>();
        }
    }
}

impl ExtensionValue for EmuSecureRandom {
    fn class_name(&self) -> String {
        "java.security.SecureRandom".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.security.SecureRandom 0x{:x} \"java.security.SecureRandom@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<EmuSecureRandom>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast an extension to a random generator (either kind).
fn as_random(v: &Value) -> Option<&EmuRandom> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuRandom>(),
        _ => None,
    }
}

/// Downcast an extension to a secure generator.
fn as_secure(v: &Value) -> Option<&EmuSecureRandom> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<EmuSecureRandom>(),
        _ => None,
    }
}

/// Whether `v` is a `Random`/`SecureRandom` (the `(int,
/// Random)` BigInteger ctor's overload check).
pub(crate) fn is_random(v: &Value) -> bool {
    as_random(v).is_some() || as_secure(v).is_some()
}

/// Fill `n` bytes from a `Random`/`SecureRandom` value (the
/// `(int, Random)` BigInteger ctor): non-randoms draw blanks
/// (the ctor guards with [`is_random`], so only the tail runs
/// cold — every line stays covered).
pub(crate) fn random_bytes(v: &Value, n: usize) -> Vec<i8> {
    let mut out = vec![0; n];
    if let Some(r) = as_random(v) {
        r.next_bytes(&mut out);
    } else if let Some(r) = as_secure(v) {
        r.next_bytes(&mut out);
    }
    out
}

/// `(Random.)` / `(Random. seed)` (the seed takes the boxArg
/// `long` coercion, like every `(long)` overload).
pub(crate) fn build_random(args: &[Value]) -> Result<Value, NativeError> {
    match args.len() {
        0 => Ok(Value::Extension(std::sync::Arc::new(EmuRandom::fresh()))),
        1 => {
            let seed = match crate::classemu::long_arg(&args[0]) {
                Ok(seed) => seed,
                // Nil unboxes to NPE (the primitive param);
                // anything else is not a `(long)` overload.
                Err(e) if e.class == ThrowClass::NullPointerException => return Err(e),
                Err(_) => return Err(crate::ctor::no_ctor("java.util.Random", false)),
            };
            Ok(Value::Extension(std::sync::Arc::new(EmuRandom::seeded(
                seed,
            ))))
        }
        _ => Err(crate::ctor::no_ctor("java.util.Random", false)),
    }
}

/// `(SecureRandom.)`.
pub(crate) fn build_secure_random(args: &[Value]) -> Result<Value, NativeError> {
    match args.len() {
        0 => Ok(Value::Extension(std::sync::Arc::new(
            EmuSecureRandom::fresh(),
        ))),
        _ => Err(crate::ctor::no_ctor("java.security.SecureRandom", false)),
    }
}

/// A `Random`/`SecureRandom` instance call, or `None` when the
/// target is not ours (the generic miss).
pub(crate) fn random_method(
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    if let Some(r) = as_random(target) {
        return Some(random_call(r, meth, rest));
    }
    if let Some(r) = as_secure(target) {
        return Some(random_call_secure(r, meth, rest));
    }
    None
}

/// A bounded draw from either RNG kind (`None` for non-RNGs —
/// `Collections/shuffle` rides here; its guard runs first so
/// the `None` arm only fires from the unit test).
pub(crate) fn random_next_int(v: &Value, bound: i32) -> Option<Result<i32, NativeError>> {
    if let Some(r) = as_random(v) {
        return Some(r.next_int_bound(bound));
    }
    if let Some(r) = as_secure(v) {
        return Some(r.next_int_bound(bound));
    }
    None
}

/// Dispatch over a `Random` (split from `random_method` for the
/// complexity budget).
fn random_call(r: &EmuRandom, meth: &str, rest: &[Value]) -> Result<Value, NativeError> {
    const CLASS: &str = "java.util.Random";
    match (meth, rest.len()) {
        ("nextInt", 0) => Ok(Value::Int(i64::from(r.next_int()))),
        ("nextInt", 1) => {
            let bound = int_arg(&rest[0], CLASS)?;
            r.next_int_bound(bound).map(|n| Value::Int(i64::from(n)))
        }
        ("nextDouble", 0) => Ok(Value::Double(r.next_double())),
        ("nextGaussian", 0) => Ok(Value::Double(r.next_gaussian())),
        ("nextBytes", 1) => with_bytes(&rest[0], CLASS, |cells| r.next_bytes(cells)),
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// Dispatch over a `SecureRandom` (same surface, OS draws).
fn random_call_secure(
    r: &EmuSecureRandom,
    meth: &str,
    rest: &[Value],
) -> Result<Value, NativeError> {
    const CLASS: &str = "java.security.SecureRandom";
    match (meth, rest.len()) {
        ("nextInt", 1) => {
            let bound = int_arg(&rest[0], CLASS)?;
            r.next_int_bound(bound).map(|n| Value::Int(i64::from(n)))
        }
        ("nextBytes", 1) => with_bytes(&rest[0], CLASS, |cells| r.next_bytes(cells)),
        _ => Err(crate::types::no_method(meth, rest.len(), CLASS)),
    }
}

/// The `nextInt` bound as an `i32` (nil unboxes to NPE, like
/// the JDK; anything else is the overload miss).
fn int_arg(v: &Value, class: &str) -> Result<i32, NativeError> {
    match v {
        Value::Int(i) => Ok(*i as i32),
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot invoke \"java.lang.Integer.intValue()\" because \"bound\" is null".to_owned(),
        )),
        _ => Err(crate::types::no_method("nextInt", 1, class)),
    }
}

/// Run `f` over a `nextBytes` destination cell (a `byte[]` —
/// nil is the body's NPE, anything else the overload miss).
fn with_bytes(v: &Value, class: &str, f: impl FnOnce(&mut [i8])) -> Result<Value, NativeError> {
    match v {
        Value::Array(a) => {
            let mut guard = a.lock();
            match &mut *guard {
                ArrayStore::Bytes(cells) => {
                    f(cells);
                    Ok(Value::Nil)
                }
                _ => Err(crate::types::no_method("nextBytes", 1, class)),
            }
        }
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            "Cannot read the array length because \"bytes\" is null".to_owned(),
        )),
        _ => Err(crate::types::no_method("nextBytes", 1, class)),
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, random_next_int};

    #[test]
    fn non_rng_declines_bounded_draw() {
        // The shuffle guard runs first, so only this test fires
        // the `None` arm (non-RNGs never draw).
        assert!(random_next_int(&Value::Int(5), 2).is_none());
        assert!(random_next_int(&Value::Nil, 2).is_none());
    }
}
