// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The [`Value`] enum: every runtime value is one variant, immutable
//! and `Send + Sync`, shared by `Arc` (no tracing GC — drops reclaim).
//!
//! Equality is Clojure `=` ([`PartialEq`]): numeric tower comparison
//! across [`Value::Int`]/[`Value::Double`], sequential comparison
//! across [`Value::List`]/[`Value::Vector`], and identity comparison
//! for [`Value::Regex`] (mirroring `java.util.regex.Pattern`, which
//! has no value equality: two separately read patterns are never
//! `=`, while two handles to the same value are).
//!
//! [`Hash`] is equiv-consistent: values that are `=` hash equally,
//! so `rpds` maps and sets obey Clojure lookup semantics (`(get {1
//! :a} 1.0)` finds `:a`). The one wrinkle is NaN: like Clojure,
//! NaN is never `=` to anything including itself, so NaN keys never
//! match on lookup — but NaN hashes deterministically (canonical
//! bits), keeping map behavior well-defined.

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{Keyword, Symbol};

/// A persistent list of values (thread-safe rpds list).
pub type List = rpds::ListSync<Value>;
/// A persistent vector of values (thread-safe rpds vector).
pub type Vector = rpds::VectorSync<Value>;
/// Deterministic Fx hasher for maps/sets: iteration order is stable
/// across runs (rpds's default RandomState is not), which keeps
/// printed output and tests reproducible. Same non-crypto posture as
/// the JVM's murmur-based hashing.
pub type FxBuildHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;

/// A persistent map of values (thread-safe rpds hash-trie map).
pub type Map = rpds::HashTrieMapSync<Value, Value, FxBuildHasher>;
/// A persistent set of values (thread-safe rpds hash-trie set).
pub type Set = rpds::HashTrieSetSync<Value, FxBuildHasher>;

/// An empty [`Map`] (rpds's `new_sync` only covers the default
/// hasher, so construction goes through the explicit hasher form).
#[must_use]
pub fn new_map() -> Map {
    Map::new_with_hasher_and_ptr_kind(FxBuildHasher::default())
}

/// An empty [`Set`].
#[must_use]
pub fn new_set() -> Set {
    Set::new_with_hasher_with_ptr_kind(FxBuildHasher::default())
}

/// A Clojure runtime value (S2 foundation; S3+ adds functions, vars,
/// refs, and the concurrency primitives as new variants).
#[derive(Clone, Debug)]
pub enum Value {
    /// `nil`.
    Nil,
    /// `true` / `false`.
    Bool(bool),
    /// 64-bit integer (checked arithmetic; overflow errors, never wraps).
    Int(i64),
    /// IEEE-754 double.
    Double(f64),
    /// String.
    Str(Arc<str>),
    /// Character.
    Char(char),
    /// Symbol.
    Symbol(Symbol),
    /// Keyword.
    Keyword(Keyword),
    /// Persistent list.
    List(List),
    /// Persistent vector.
    Vector(Vector),
    /// Persistent map.
    Map(Map),
    /// Persistent set.
    Set(Set),
    /// Regex pattern source (matching arrives with `re-*` in S5).
    Regex(Arc<str>),
}

impl Value {
    /// Clojure truthiness: only `nil` and `false` are false.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// Numeric view for `=` and hashing: integers that fit exactly
    /// compare/hash as doubles do, matching JVM mixed comparison
    /// (which converts the long to double).
    fn as_f64(&self) -> Option<f64> {
        match self {
            Value::Int(i) => Some(*i as f64),
            Value::Double(d) => Some(*d),
            _ => None,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            (Value::Double(a), Value::Double(b)) => a == b,
            // Mixed numerics compare through f64, as on the JVM.
            (Value::Int(_), Value::Double(_)) | (Value::Double(_), Value::Int(_)) => {
                self.as_f64() == other.as_f64()
            }
            (Value::Str(a), Value::Str(b)) => a == b,
            (Value::Char(a), Value::Char(b)) => a == b,
            (Value::Symbol(a), Value::Symbol(b)) => a == b,
            (Value::Keyword(a), Value::Keyword(b)) => a == b,
            // Sequential equality crosses List/Vector.
            (Value::List(a), Value::List(b)) => a.iter().eq(b.iter()),
            (Value::Vector(a), Value::Vector(b)) => a.iter().eq(b.iter()),
            (Value::List(a), Value::Vector(b)) | (Value::Vector(b), Value::List(a)) => {
                a.iter().eq(b.iter())
            }
            (Value::Map(a), Value::Map(b)) => {
                a.size() == b.size() && a.iter().all(|(k, v)| b.get(k) == Some(v))
            }
            (Value::Set(a), Value::Set(b)) => {
                a.size() == b.size() && a.iter().all(|v| b.contains(v))
            }
            // Identity semantics like java.util.regex.Pattern.
            (Value::Regex(a), Value::Regex(b)) => Arc::ptr_eq(a, b),
            _ => false,
        }
    }
}

// `Eq`'s reflexivity technically fails for NaN (never `=` to itself,
// exactly as in Clojure); the marker is required for map/set keys,
// where NaN behaves identically to the JVM: it hashes
// deterministically but never matches on lookup.
impl Eq for Value {}

/// Canonical bits for hashing doubles: integral values that fit in
/// `i64` hash as that integer (unifying `Int`/`Double` hashes for
/// `=` values); NaN hashes its canonical bits; anything else hashes
/// its own bits.
fn double_hash_bits(d: f64) -> u64 {
    if d.is_nan() {
        f64::NAN.to_bits()
    } else if d.fract() == 0.0 && d >= i64::MIN as f64 && d <= i64::MAX as f64 {
            // `d as i64` is exact here: integral and in range. Note
            // `i64::MAX as f64` rounds up to 2^63, so exclude it:
            // `2^63 as i64` saturates instead of equaling.
            let as_int = d as i64;
            if as_int as f64 == d {
                return (as_int as u64) ^ 0x9e37_79b9_7f4a_7c15;
            }
            d.to_bits()
        } else {
            d.to_bits()
        }
}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Family markers keep unequal types apart; numbers share one
        // marker with canonicalized payloads (see `double_hash_bits`).
        match self {
            Value::Nil => state.write_u8(0),
            Value::Bool(b) => {
                state.write_u8(1);
                state.write_u8(u8::from(*b));
            }
            // Through the double canonicalizer: `=` compares mixed
            // numerics via `i as f64`, so hashing the f64 image is the
            // only consistent choice (large i64s round-trip through
            // f64 exactly like their `=` partners).
            Value::Int(i) => {
                state.write_u8(2);
                state.write_u64(double_hash_bits(*i as f64));
            }
            Value::Double(d) => {
                state.write_u8(2);
                state.write_u64(double_hash_bits(*d));
            }
            Value::Str(s) => {
                state.write_u8(3);
                s.hash(state);
            }
            Value::Char(c) => {
                state.write_u8(4);
                c.hash(state);
            }
            Value::Symbol(s) => {
                state.write_u8(5);
                s.hash(state);
            }
            Value::Keyword(k) => {
                state.write_u8(6);
                k.hash(state);
            }
            // Sequential hash ignores List-vs-Vector (they can be `=`).
            Value::List(l) => {
                state.write_u8(7);
                state.write_usize(l.len());
                for v in l.iter() {
                    v.hash(state);
                }
            }
            Value::Vector(v) => {
                state.write_u8(7);
                state.write_usize(v.len());
                for x in v.iter() {
                    x.hash(state);
                }
            }
            // Order-insensitive: sums commute, so iteration order
            // cannot leak into the hash.
            Value::Map(m) => {
                state.write_u8(8);
                state.write_usize(m.size());
                let mut acc: u64 = 0;
                for (k, v) in m.iter() {
                    acc = acc.wrapping_add(entry_hash(k, v));
                }
                state.write_u64(acc);
            }
            Value::Set(s) => {
                state.write_u8(9);
                state.write_usize(s.size());
                let mut acc: u64 = 0;
                for v in s.iter() {
                    acc = acc.wrapping_add(entry_hash(v, v));
                }
                state.write_u64(acc);
            }
            // Same-pointer values share the pattern string, so the
            // string hash is consistent with pointer equality.
            Value::Regex(pattern) => {
                state.write_u8(10);
                pattern.hash(state);
            }
        }
    }
}

/// Hash of one map/set entry with a fixed seed, for the
/// order-insensitive sums above.
fn entry_hash(k: &Value, v: &Value) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    let mut h = DefaultHasher::new();
    k.hash(&mut h);
    v.hash(&mut h);
    h.finish()
}

#[cfg(test)]
mod tests {
    use super::{Value, new_map, new_set};
    use crate::{Keyword, Symbol};
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    use std::sync::Arc;

    fn hash_of(v: &Value) -> u64 {
        let mut h = DefaultHasher::new();
        v.hash(&mut h);
        h.finish()
    }

    fn assert_hash_consistent(a: &Value, b: &Value) {
        assert_eq!(a, b, "{a:?} should equal {b:?}");
        assert_eq!(hash_of(a), hash_of(b), "equal values must hash equally");
    }

    #[test]
    fn scalars_equiv() {
        assert_eq!(Value::Nil, Value::Nil);
        assert_eq!(Value::Bool(true), Value::Bool(true));
        assert_ne!(Value::Bool(true), Value::Bool(false));
        assert_eq!(Value::Int(1), Value::Int(1));
        assert_eq!(Value::Double(1.5), Value::Double(1.5));
        assert_eq!(
            Value::Str(Arc::from("a")),
            Value::Str(Arc::from("a"))
        );
        assert_eq!(Value::Char('a'), Value::Char('a'));
        assert_eq!(
            Value::Symbol(Symbol::new(None, "a")),
            Value::Symbol(Symbol::new(None, "a"))
        );
        assert_eq!(
            Value::Keyword(Keyword::new(None, "a")),
            Value::Keyword(Keyword::new(None, "a"))
        );
        // Cross-type values are never `=`.
        assert_ne!(Value::Int(1), Value::Bool(true));
        assert_ne!(Value::Nil, Value::Bool(false));
        assert_ne!(
            Value::Symbol(Symbol::new(None, "a")),
            Value::Keyword(Keyword::new(None, "a"))
        );
        assert_ne!(Value::Str(Arc::from("a")), Value::Char('a'));
    }

    #[test]
    fn mixed_numerics_equiv_and_hash() {
        assert_hash_consistent(&Value::Int(1), &Value::Double(1.0));
        assert_hash_consistent(&Value::Int(-3), &Value::Double(-3.0));
        assert_hash_consistent(&Value::Int(0), &Value::Double(-0.0));
        assert_ne!(Value::Int(1), Value::Double(1.5));
        assert_ne!(Value::Double(1.5), Value::Double(2.5));
        // JVM converts the long to double for mixed comparison.
        let big = 9_007_199_254_740_993_i64; // 2^53 + 1, not exactly representable
        assert_eq!(Value::Int(big), Value::Double(big as f64));
        assert_eq!(hash_of(&Value::Int(big)), hash_of(&Value::Double(big as f64)));
    }

    #[test]
    fn nan_never_equiv_but_hashes() {
        let nan = Value::Double(f64::NAN);
        assert_ne!(nan, nan);
        // Deterministic: same bits every time.
        assert_eq!(hash_of(&nan), hash_of(&Value::Double(f64::NAN)));
        assert_ne!(Value::Double(f64::NAN), Value::Double(1.0));
    }

    #[test]
    fn sequential_cross_equiv_and_hash() {
        let list = Value::List(vec_into_list(vec![Value::Int(1), Value::Int(2)]));
        let vector = Value::Vector(vec_into_vector(vec![Value::Int(1), Value::Int(2)]));
        assert_hash_consistent(&list, &vector);
        let other = Value::Vector(vec_into_vector(vec![Value::Int(2), Value::Int(1)]));
        assert_ne!(list, other);
        let empty_list = Value::List(super::List::new_sync());
        let empty_vector = Value::Vector(super::Vector::new_sync());
        assert_hash_consistent(&empty_list, &empty_vector);
    }

    #[test]
    fn map_equiv_and_lookup() {
        let m = Value::Map(
            new_map()
                .insert(Value::Int(1), Value::Str(Arc::from("a")))
                .insert(Value::Keyword(Keyword::new(None, "k")), Value::Nil),
        );
        // Cross-type numeric keys match, as on the JVM.
        if let Value::Map(m) = &m {
            assert_eq!(m.get(&Value::Double(1.0)), Some(&Value::Str(Arc::from("a"))));
            assert_eq!(m.size(), 2);
        } else {
            panic!("expected map");
        }
        let swapped = Value::Map(
            new_map()
                .insert(Value::Keyword(Keyword::new(None, "k")), Value::Nil)
                .insert(Value::Double(1.0), Value::Str(Arc::from("a"))),
        );
        assert_hash_consistent(&m, &swapped);
    }

    #[test]
    fn set_equiv() {
        let a = Value::Set(new_set().insert(Value::Int(1)).insert(Value::Int(2)));
        let b = Value::Set(new_set().insert(Value::Double(2.0)).insert(Value::Int(1)));
        assert_hash_consistent(&a, &b);
        if let Value::Set(s) = &a {
            assert!(s.contains(&Value::Double(1.0)));
            assert!(!s.contains(&Value::Int(3)));
        } else {
            panic!("expected set");
        }
    }

    #[test]
    fn regex_is_identity_compared() {
        let re = Value::Regex(Arc::from("a+"));
        assert_eq!(re, re.clone());
        // Separately built identical patterns are NOT `=` (mirrors
        // java.util.regex.Pattern, which has no value equality).
        assert_ne!(re, Value::Regex(Arc::from("a+")));
    }

    #[test]
    fn truthiness() {
        assert!(!Value::Nil.is_truthy());
        assert!(!Value::Bool(false).is_truthy());
        assert!(Value::Bool(true).is_truthy());
        assert!(Value::Int(0).is_truthy());
        assert!(Value::Str(Arc::from("")).is_truthy());
    }

    #[test]
    fn value_is_send_sync() {
        fn assert_send_sync<T: Send + Sync>() {}
        assert_send_sync::<Value>();
    }

    fn vec_into_list(items: Vec<Value>) -> super::List {
        let mut list = super::List::new_sync();
        for v in items.into_iter().rev() {
            list = list.push_front(v);
        }
        list
    }

    fn vec_into_vector(items: Vec<Value>) -> super::Vector {
        let mut vector = super::Vector::new_sync();
        for v in items {
            vector = vector.push_back(v);
        }
        vector
    }
}
