// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #205 `java.security.MessageDigest`: pure-Rust MD5,
//! SHA-256, and SHA-512 (the suite's three algorithms —
//! blockchains, Merkle trees, Diffie-Hellman, bloom filters,
//! Monte Carlo dice). The SHA round tables derive from the
//! FIPS 180-4 definitions at first use (fractional square/cube
//! roots of sieved primes through exact `BigUint` roots — no
//! memorized constants); MD5's sine table is transcribed and
//! pinned by the RFC 1321 vectors. `digest()` resets, like the
//! JDK.

use std::sync::{Arc, Mutex, OnceLock};

use clj_value::{ExtensionValue, ThrowClass, Value};

use crate::error::NativeError;
use crate::eval::Evaluator;

/// The first `n` primes (Eratosthenes over an over-large
/// sieve — the FIPS root definitions need 80).
fn first_primes(n: usize) -> Vec<u64> {
    let mut sieve = vec![true; 600];
    sieve[0] = false;
    sieve[1] = false;
    let mut primes = Vec::with_capacity(n);
    for p in 2..sieve.len() {
        if sieve[p] {
            primes.push(p as u64);
            if primes.len() == n {
                break;
            }
            let mut m = p * p;
            while m < sieve.len() {
                sieve[m] = false;
                m += p;
            }
        }
    }
    primes
}

/// `floor(root(p * 2^shift)) mod 2^bits`: one FIPS fractional-root
/// word, exactly (the `sqrt`/`cbrt` are exact integer roots).
fn root_word(p: u64, shift: u32, bits: u32, cbrt: bool) -> u64 {
    use num_bigint::BigUint;
    let scaled = BigUint::from(p) << (shift as usize);
    let root = if cbrt { scaled.cbrt() } else { scaled.sqrt() };
    let mask = (BigUint::from(1u32) << (bits as usize)) - 1u32;
    (root & mask).iter_u64_digits().next().unwrap_or(0)
}

/// The SHA-256 tables: 8 root-H words plus 64 round-K words.
fn sha256_tables() -> &'static ([u32; 8], [u32; 64]) {
    static TABLES: OnceLock<([u32; 8], [u32; 64])> = OnceLock::new();
    TABLES.get_or_init(|| {
        let primes = first_primes(64);
        let mut h = [0u32; 8];
        for (i, h_i) in h.iter_mut().enumerate() {
            *h_i = root_word(primes[i], 64, 32, false) as u32;
        }
        let mut k = [0u32; 64];
        for (i, k_i) in k.iter_mut().enumerate() {
            *k_i = root_word(primes[i], 96, 32, true) as u32;
        }
        (h, k)
    })
}

/// The SHA-512 tables: 8 root-H words plus 80 round-K words.
fn sha512_tables() -> &'static ([u64; 8], [u64; 80]) {
    static TABLES: OnceLock<([u64; 8], [u64; 80])> = OnceLock::new();
    TABLES.get_or_init(|| {
        let primes = first_primes(80);
        let mut h = [0u64; 8];
        for (i, h_i) in h.iter_mut().enumerate() {
            *h_i = root_word(primes[i], 128, 64, false);
        }
        let mut k = [0u64; 80];
        for (i, k_i) in k.iter_mut().enumerate() {
            *k_i = root_word(primes[i], 192, 64, true);
        }
        (h, k)
    })
}

/// One SHA-256 block compression (FIPS 180-4 §6.2).
fn sha256_block(h: &mut [u32; 8], block: &[u8; 64]) {
    let (_, k) = sha256_tables();
    let mut w = [0u32; 64];
    for (i, w_i) in w.iter_mut().enumerate().take(16) {
        *w_i = u32::from_be_bytes(block[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
    }
    for i in 16..64 {
        let s0 = w[i - 15].rotate_right(7) ^ w[i - 15].rotate_right(18) ^ (w[i - 15] >> 3);
        let s1 = w[i - 2].rotate_right(17) ^ w[i - 2].rotate_right(19) ^ (w[i - 2] >> 10);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..64 {
        let s1 = e.rotate_right(6) ^ e.rotate_right(11) ^ e.rotate_right(25);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(2) ^ a.rotate_right(13) ^ a.rotate_right(22);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// One SHA-512 block compression (FIPS 180-4 §6.4).
fn sha512_block(h: &mut [u64; 8], block: &[u8; 128]) {
    let (_, k) = sha512_tables();
    let mut w = [0u64; 80];
    for (i, w_i) in w.iter_mut().enumerate().take(16) {
        *w_i = u64::from_be_bytes(block[i * 8..i * 8 + 8].try_into().unwrap_or([0; 8]));
    }
    for i in 16..80 {
        let s0 = w[i - 15].rotate_right(1) ^ w[i - 15].rotate_right(8) ^ (w[i - 15] >> 7);
        let s1 = w[i - 2].rotate_right(19) ^ w[i - 2].rotate_right(61) ^ (w[i - 2] >> 6);
        w[i] = w[i - 16]
            .wrapping_add(s0)
            .wrapping_add(w[i - 7])
            .wrapping_add(s1);
    }
    let [mut a, mut b, mut c, mut d, mut e, mut f, mut g, mut hh] = *h;
    for i in 0..80 {
        let s1 = e.rotate_right(14) ^ e.rotate_right(18) ^ e.rotate_right(41);
        let ch = (e & f) ^ ((!e) & g);
        let t1 = hh
            .wrapping_add(s1)
            .wrapping_add(ch)
            .wrapping_add(k[i])
            .wrapping_add(w[i]);
        let s0 = a.rotate_right(28) ^ a.rotate_right(34) ^ a.rotate_right(39);
        let maj = (a & b) ^ (a & c) ^ (b & c);
        let t2 = s0.wrapping_add(maj);
        hh = g;
        g = f;
        f = e;
        e = d.wrapping_add(t1);
        d = c;
        c = b;
        b = a;
        a = t1.wrapping_add(t2);
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
    h[4] = h[4].wrapping_add(e);
    h[5] = h[5].wrapping_add(f);
    h[6] = h[6].wrapping_add(g);
    h[7] = h[7].wrapping_add(hh);
}

/// The MD5 sine table (RFC 1321 — `floor(2^32 × abs(sin(i)))`),
/// pinned by the RFC vectors below.
const MD5_K: [u32; 64] = [
    0xd76aa478, 0xe8c7b756, 0x242070db, 0xc1bdceee, 0xf57c0faf, 0x4787c62a, 0xa8304613, 0xfd469501,
    0x698098d8, 0x8b44f7af, 0xffff5bb1, 0x895cd7be, 0x6b901122, 0xfd987193, 0xa679438e, 0x49b40821,
    0xf61e2562, 0xc040b340, 0x265e5a51, 0xe9b6c7aa, 0xd62f105d, 0x02441453, 0xd8a1e681, 0xe7d3fbc8,
    0x21e1cde6, 0xc33707d6, 0xf4d50d87, 0x455a14ed, 0xa9e3e905, 0xfcefa3f8, 0x676f02d9, 0x8d2a4c8a,
    0xfffa3942, 0x8771f681, 0x6d9d6122, 0xfde5380c, 0xa4beea44, 0x4bdecfa9, 0xf6bb4b60, 0xbebfbc70,
    0x289b7ec6, 0xeaa127fa, 0xd4ef3085, 0x04881d05, 0xd9d4d039, 0xe6db99e5, 0x1fa27cf8, 0xc4ac5665,
    0xf4292244, 0x432aff97, 0xab9423a7, 0xfc93a039, 0x655b59c3, 0x8f0ccc92, 0xffeff47d, 0x85845dd1,
    0x6fa87e4f, 0xfe2ce6e0, 0xa3014314, 0x4e0811a1, 0xf7537e82, 0xbd3af235, 0x2ad7d2bb, 0xeb86d391,
];

/// The MD5 per-round rotations.
const MD5_S: [u32; 64] = [
    7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22, 7, 12, 17, 22,
    5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20, 5, 9, 14, 20,
    4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23, 4, 11, 16, 23,
    6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21, 6, 10, 15, 21,
];

/// One MD5 block compression (RFC 1321 §3.4).
fn md5_block(h: &mut [u32; 4], block: &[u8; 64]) {
    let mut m = [0u32; 16];
    for (i, m_i) in m.iter_mut().enumerate() {
        *m_i = u32::from_le_bytes(block[i * 4..i * 4 + 4].try_into().unwrap_or([0; 4]));
    }
    let [mut a, mut b, mut c, mut d] = *h;
    for i in 0..64 {
        let (f, g) = if i < 16 {
            ((b & c) | ((!b) & d), i)
        } else if i < 32 {
            ((d & b) | ((!d) & c), (5 * i + 1) % 16)
        } else if i < 48 {
            (b ^ c ^ d, (3 * i + 5) % 16)
        } else {
            (c ^ (b | (!d)), (7 * i) % 16)
        };
        let sum = a
            .wrapping_add(f)
            .wrapping_add(MD5_K[i])
            .wrapping_add(m[g]);
        a = d;
        d = c;
        c = b;
        b = b.wrapping_add(sum.rotate_left(MD5_S[i]));
    }
    h[0] = h[0].wrapping_add(a);
    h[1] = h[1].wrapping_add(b);
    h[2] = h[2].wrapping_add(c);
    h[3] = h[3].wrapping_add(d);
}

/// A running digest: the byte count, the unprocessed tail, and
/// the chain state.
#[derive(Clone, Debug)]
enum DigestState {
    Md5 { len: u64, tail: Vec<u8>, h: [u32; 4] },
    Sha256 { len: u64, tail: Vec<u8>, h: [u32; 8] },
    Sha512 { len: u64, tail: Vec<u8>, h: [u64; 8] },
}

impl DigestState {
    /// A fresh state for `algo` (`getInstance` validates first).
    fn fresh(algo: &str) -> Self {
        match algo {
            "MD5" => Self::Md5 {
                len: 0,
                tail: Vec::new(),
                h: [0x67452301, 0xefcdab89, 0x98badcfe, 0x10325476],
            },
            "SHA-512" => Self::Sha512 {
                len: 0,
                tail: Vec::new(),
                h: *sha512_tables().0.first_chunk().expect("eight words"),
            },
            _ => Self::Sha256 {
                len: 0,
                tail: Vec::new(),
                h: *sha256_tables().0.first_chunk().expect("eight words"),
            },
        }
    }

    /// Feed bytes (whole blocks compress eagerly).
    fn update(&mut self, bytes: &[u8]) {
        let block = self.block_len();
        self.bump_len(bytes.len() as u64);
        self.tail_mut().extend_from_slice(bytes);
        while self.tail_len() >= block {
            let chunk: Vec<u8> = self.tail_mut().drain(..block).collect();
            self.compress(&chunk);
        }
    }

    /// Count freshly-fed bytes.
    fn bump_len(&mut self, n: u64) {
        let len = match self {
            Self::Md5 { len, .. } | Self::Sha256 { len, .. } | Self::Sha512 { len, .. } => len,
        };
        *len += n;
    }

    /// The unprocessed tail, exclusively.
    fn tail_mut(&mut self) -> &mut Vec<u8> {
        match self {
            Self::Md5 { tail, .. } | Self::Sha256 { tail, .. } | Self::Sha512 { tail, .. } => tail,
        }
    }

    /// The unprocessed tail's length.
    fn tail_len(&self) -> usize {
        match self {
            Self::Md5 { tail, .. } | Self::Sha256 { tail, .. } | Self::Sha512 { tail, .. } => {
                tail.len()
            }
        }
    }

    /// The block length in bytes.
    fn block_len(&self) -> usize {
        match self {
            Self::Md5 { .. } | Self::Sha256 { .. } => 64,
            Self::Sha512 { .. } => 128,
        }
    }

    /// Compress one whole block from `update`.
    fn compress(&mut self, chunk: &[u8]) {
        match self {
            Self::Md5 { h, .. } => {
                md5_block(h, chunk.first_chunk().expect("a whole block"));
            }
            Self::Sha256 { h, .. } => {
                sha256_block(h, chunk.first_chunk().expect("a whole block"));
            }
            Self::Sha512 { h, .. } => {
                sha512_block(h, chunk.first_chunk().expect("a whole block"));
            }
        }
    }

    /// Pad, append the bit length, and squeeze the digest (the
    /// state is consumed — callers reset, like the JDK).
    fn finalize(self) -> Vec<u8> {
        match self {
            Self::Md5 { len, mut tail, mut h } => {
                tail.push(0x80);
                while tail.len() % 64 != 56 {
                    tail.push(0);
                }
                tail.extend_from_slice(&(len.wrapping_mul(8)).to_le_bytes());
                for block in tail.chunks_exact(64) {
                    md5_block(&mut h, block.first_chunk().expect("a whole block"));
                }
                h.iter().flat_map(|w| w.to_le_bytes()).collect()
            }
            Self::Sha256 { len, mut tail, mut h } => {
                tail.push(0x80);
                while tail.len() % 64 != 56 {
                    tail.push(0);
                }
                tail.extend_from_slice(&(len.wrapping_mul(8)).to_be_bytes());
                for block in tail.chunks_exact(64) {
                    sha256_block(&mut h, block.first_chunk().expect("a whole block"));
                }
                h.iter().flat_map(|w| w.to_be_bytes()).collect()
            }
            Self::Sha512 { len, mut tail, mut h } => {
                tail.push(0x80);
                while tail.len() % 128 != 112 {
                    tail.push(0);
                }
                let bits = (len as u128).wrapping_mul(8);
                tail.extend_from_slice(&0u64.to_be_bytes());
                tail.extend_from_slice(&(bits as u64).to_be_bytes());
                for block in tail.chunks_exact(128) {
                    sha512_block(&mut h, block.first_chunk().expect("a whole block"));
                }
                h.iter().flat_map(|w| w.to_be_bytes()).collect()
            }
        }
    }
}

/// An emulated `java.security.MessageDigest`: identity, the
/// algorithm name, and the running state behind a lock.
#[derive(Debug)]
pub(crate) struct DigestHandle {
    id: u64,
    algo: String,
    state: Mutex<DigestState>,
}

impl ExtensionValue for DigestHandle {
    fn class_name(&self) -> String {
        "java.security.MessageDigest".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.security.MessageDigest 0x{:x} \"{}\"]",
            self.id, self.algo
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<DigestHandle>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a digest value, if any.
pub(crate) fn as_digest(v: &Value) -> Option<&DigestHandle> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<DigestHandle>(),
        _ => None,
    }
}

/// Lock a digest's running state (poison-tolerant).
fn lock_state(d: &DigestHandle) -> std::sync::MutexGuard<'_, DigestState> {
    d.state.lock().unwrap_or_else(|e| e.into_inner())
}

/// Normalize a `getInstance` algorithm name (ASCII-uppercase —
/// the provider lookup is case-blind).
fn resolve_algo(name: &str) -> Option<&'static str> {
    match name.to_ascii_uppercase().as_str() {
        "MD5" => Some("MD5"),
        "SHA-256" => Some("SHA-256"),
        "SHA-512" => Some("SHA-512"),
        _ => None,
    }
}

/// `MessageDigest/getInstance` (the `(String)` overload):
/// unknown names are `NoSuchAlgorithmException` (the JDK's
/// `<algo> MessageDigest not available` text), nil NPEs.
pub(crate) fn digest_get_instance(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    if args.len() != 1 {
        return Err(crate::types::no_method(
            "getInstance",
            args.len(),
            "java.security.MessageDigest",
        ));
    }
    let Value::Str(name) = &args[0] else {
        if matches!(&args[0], Value::Nil) {
            return Err(NativeError::new(
                ThrowClass::NullPointerException,
                "Cannot invoke \"java.security.MessageDigest.getInstance(String)\" because \"algorithm\" is null"
                    .to_owned(),
            ));
        }
        return Err(crate::core::cce_err(&args[0], "java.lang.String"));
    };
    match resolve_algo(name) {
        Some(algo) => Ok(Value::Extension(Arc::new(DigestHandle {
            id: eval.next_id(),
            algo: algo.to_owned(),
            state: Mutex::new(DigestState::fresh(algo)),
        }))),
        None => Err(NativeError::new(
            ThrowClass::NoSuchAlgorithmException,
            format!("{name} MessageDigest not available"),
        )),
    }
}

/// Mint a `byte[]` from digest output.
fn digest_bytes(out: Vec<u8>) -> Value {
    Value::Array(clj_value::Array::new(
        clj_value::ArrayDesc::flat(clj_value::BaseType::Byte),
        clj_value::ArrayStore::Bytes(out.into_iter().map(|b| b as i8).collect()),
    ))
}

/// Squeeze a digest: pad, emit, and reset (like the JDK —
/// `digest()` leaves the handle reusable).
fn squeeze(d: &DigestHandle) -> Vec<u8> {
    let fresh = DigestState::fresh(&d.algo);
    let state = std::mem::replace(&mut *lock_state(d), fresh);
    state.finalize()
}

/// One digest input argument: `byte[]` bytes (nil NPEs on the
/// parameter, anything else misses the single overload).
fn digest_input(arg: &Value, meth: &str) -> Result<Vec<u8>, NativeError> {
    if matches!(arg, Value::Nil) {
        return Err(NativeError::new(
            ThrowClass::NullPointerException,
            format!(
                "Cannot invoke \"java.security.MessageDigest.{meth}(byte[])\" because \"input\" is null"
            ),
        ));
    }
    let Value::Array(a) = arg else {
        return Err(crate::types::no_method(
            meth,
            1,
            "java.security.MessageDigest",
        ));
    };
    let guard = a.lock();
    let clj_value::ArrayStore::Bytes(slots) = &*guard else {
        return Err(crate::types::no_method(
            meth,
            1,
            "java.security.MessageDigest",
        ));
    };
    Ok(slots.iter().map(|b| *b as u8).collect())
}

/// The digest members (`None` for non-digests): `update` feeds
/// bytes, `digest` squeezes (empty, or after feeding its bytes
/// argument first).
pub(crate) fn digest_method(
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    let d = as_digest(target)?;
    match (meth, rest.len()) {
        ("update", 1) => Some(digest_input(&rest[0], "update").map(|bytes| {
            lock_state(d).update(&bytes);
            Value::Nil
        })),
        ("digest", 0) => Some(Ok(digest_bytes(squeeze(d)))),
        ("digest", 1) => Some(digest_input(&rest[0], "digest").map(|bytes| {
            lock_state(d).update(&bytes);
            digest_bytes(squeeze(d))
        })),
        _ => None,
    }
}
