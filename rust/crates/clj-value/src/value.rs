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
//! Equality is Clojure `=` ([`PartialEq`]): same-type numerics
//! (`Int`/`Int` by value, `Double`/`Double` by `==` — `(= 1 1.0)`
//! is false, `(= 0.0 -0.0)` true, `(= ##NaN ##NaN)` false),
//! sequential comparison across [`Value::List`]/[`Value::Vector`],
//! and identity comparison for [`Value::Regex`] (mirroring
//! `java.util.regex.Pattern`, which has no value equality: two
//! separately read patterns are never `=`, while two handles to the
//! same value are).
//!
//! [`Hash`] is equiv-consistent: values that are `=` hash equally,
//! so `rpds` maps and sets obey Clojure lookup semantics (mixed
//! numerics are distinct keys: `(get {1 :a} 1.0)` misses and
//! `{1 :a 1.0 :b}` keeps both). NaN hashes deterministically
//! (canonical bits); `-0.0` hashes as `0.0` (they are `=`).
//!
//! Collection literals are duplicate-checked on construction
//! ([`Map::from_kvs_checked`], [`Set::from_items_checked`]) with the
//! JVM `RT.map` / `PersistentHashSet.createWithCheck` rules
//! (`docs/t1174/reader-contract.md` §19): a small-map pairwise scan
//! reporting the FIRST key, otherwise an incremental scan reporting
//! the NEW key, comparing with a line-by-line port of
//! `Util.equiv`/`Util.equals` (see [`jvm_equiv`]).

use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::{
    Atom, Delay, ExtensionValue, Future, Keyword, Promise, Symbol, Throwable, Transient, Unbound,
    Var,
};

/// Deterministic Fx hasher for maps/sets (rpds's default RandomState
/// is not stable across runs). Same non-crypto posture as the JVM's
/// murmur-based hashing. (Iteration order comes from the insertion
/// sidecars below, not the hasher.)
pub type FxBuildHasher = std::hash::BuildHasherDefault<rustc_hash::FxHasher>;

type RpdsList = rpds::ListSync<Value>;
type RpdsVector = rpds::VectorSync<Value>;
type RpdsMap = rpds::HashTrieMapSync<Value, Value, FxBuildHasher>;
type RpdsSet = rpds::HashTrieSetSync<Value, FxBuildHasher>;

/// A persistent list of values (thread-safe rpds list) with optional
/// metadata. Method names match the bare rpds type so readers keep
/// compiling; equality and hashing ignore the metadata.
#[derive(Clone, Debug)]
pub struct List {
    inner: RpdsList,
    meta: Option<Arc<Map>>,
}

/// A persistent vector of values (thread-safe rpds vector) with
/// optional metadata. See [`List`].
#[derive(Clone, Debug)]
pub struct Vector {
    inner: RpdsVector,
    meta: Option<Arc<Map>>,
}

/// A persistent map of values (thread-safe rpds hash-trie map) with
/// optional metadata. See [`List`].
///
/// `keys` is the insertion-order sidecar: the JVM's small maps are
/// insertion-ordered array-maps, and that order is observable —
/// syntax-quote flattens maps in seq order, and `pr` prints small
/// maps in insertion order. Re-inserting an existing key keeps its
/// original position (and key object), like `assoc`. (Big JVM maps
/// iterate in murmur-hash order, which no insertion record can
/// reproduce; the harness canonicalizes map/set order away, so only
/// syntax-quote expansion and rendering observe this.)
///
/// `hashy` reports `PersistentHashMap` instead of
/// `PersistentArrayMap`: promotion is one-way and history-dependent
/// (content alone cannot decide — `dissoc` never demotes and
/// `array-map` forces array), so it is stored, not derived.
#[derive(Clone, Debug)]
pub struct Map {
    inner: RpdsMap,
    meta: Option<Arc<Map>>,
    keys: Vec<Value>,
    hashy: bool,
}

/// A persistent set of values (thread-safe rpds hash-trie set) with
/// optional metadata. See [`List`]. `members` is the insertion-order
/// sidecar (small JVM sets seq in insertion order); re-inserting a
/// member is a no-op, like `conj`.
#[derive(Clone, Debug)]
pub struct Set {
    inner: RpdsSet,
    meta: Option<Arc<Map>>,
    members: Vec<Value>,
}

impl List {
    /// An empty list.
    #[must_use]
    pub fn new_sync() -> Self {
        Self {
            inner: RpdsList::new_sync(),
            meta: None,
        }
    }

    /// Build a list from items in order: `from_vec([a, b])` is `(a b)`.
    #[must_use]
    pub fn from_vec(items: Vec<Value>) -> Self {
        let mut list = Self::new_sync();
        for v in items.into_iter().rev() {
            list = list.push_front(v);
        }
        list
    }

    /// Prepend a value.
    #[must_use]
    pub fn push_front(self, v: Value) -> Self {
        Self {
            inner: self.inner.push_front(v),
            meta: self.meta,
        }
    }

    /// The first value, if any.
    #[must_use]
    pub fn first(&self) -> Option<&Value> {
        self.inner.iter().next()
    }

    /// The list without its first value (`None` when empty;
    /// metadata is preserved — `rest` strips it separately).
    #[must_use]
    pub fn drop_first(&self) -> Option<Self> {
        self.inner.drop_first().map(|inner| Self {
            inner,
            meta: self.meta.clone(),
        })
    }

    /// Iterate front to back.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.inner.iter()
    }

    /// The element count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the list is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Return this list with `meta` attached (`None` clears it).
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}

impl Vector {
    /// An empty vector.
    #[must_use]
    pub fn new_sync() -> Self {
        Self {
            inner: RpdsVector::new_sync(),
            meta: None,
        }
    }

    /// Build a vector from items in order.
    #[must_use]
    pub fn from_vec(items: Vec<Value>) -> Self {
        let mut vector = Self::new_sync();
        for v in items {
            vector = vector.push_back(v);
        }
        vector
    }

    /// Append a value.
    #[must_use]
    pub fn push_back(self, v: Value) -> Self {
        Self {
            inner: self.inner.push_back(v),
            meta: self.meta,
        }
    }

    /// Replace the item at `index` (`None` when out of bounds).
    #[must_use]
    pub fn set_nth(&self, index: usize, v: Value) -> Option<Self> {
        self.inner.set(index, v).map(|inner| Self {
            inner,
            meta: self.meta.clone(),
        })
    }

    /// The vector without its last item (`None` when empty).
    #[must_use]
    pub fn drop_last(&self) -> Option<Self> {
        self.inner.drop_last().map(|inner| Self {
            inner,
            meta: self.meta.clone(),
        })
    }

    /// Iterate front to back.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.inner.iter()
    }

    /// The element count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the vector is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.len() == 0
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Return this vector with `meta` attached (`None` clears it).
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}

impl Map {
    /// Build a map from key/value pairs, duplicate-checking them with
    /// the JVM literal rules: `Err` is the offending key (the FIRST
    /// of the pair for small maps, the NEW key otherwise). Literals
    /// take their class from `canBePAM` (`RT.map`/`mapUniqueKeys`).
    pub fn from_kvs_checked(pairs: Vec<(Value, Value)>) -> Result<Self, Value> {
        let keys: Vec<&Value> = pairs.iter().map(|(k, _)| k).collect();
        if let Some(dup) = find_map_dup(&keys) {
            return Err(dup.clone());
        }
        let hashy = !can_be_small_map(&keys);
        let mut map = new_map();
        for (k, v) in pairs {
            map = map.insert(k, v);
        }
        map.hashy = hashy;
        Ok(map)
    }

    /// Assoc a key/value pair. A new key appends to the insertion
    /// order; an existing key keeps its position (and key object).
    /// Promotion follows `PersistentArrayMap.assoc`: past 8 pairs a
    /// non-keyword key promotes, past 64 pairs anything does.
    #[must_use]
    pub fn insert(self, k: Value, v: Value) -> Self {
        if self.inner.get(&k).is_some() {
            return Self {
                inner: self.inner.insert(k, v),
                meta: self.meta,
                keys: self.keys,
                hashy: self.hashy,
            };
        }
        let size = self.inner.size();
        let promote = !self.hashy && (size >= 64 || (size >= 8 && !matches!(k, Value::Keyword(_))));
        let mut keys = self.keys;
        keys.push(k.clone());
        Self {
            inner: self.inner.insert(k, v),
            meta: self.meta,
            keys,
            hashy: self.hashy || promote,
        }
    }

    /// Remove a key (`dissoc` one key). A missing key is a no-op;
    /// otherwise both the entries and the insertion-order sidecar
    /// drop it, and later positions shift down. The class never
    /// demotes (a shrunk hash-map stays a hash-map).
    #[must_use]
    pub fn without(mut self, k: &Value) -> Self {
        if self.inner.get(k).is_none() {
            return self;
        }
        self.keys.retain(|key| key != k);
        Self {
            inner: self.inner.remove(k),
            meta: self.meta,
            keys: self.keys,
            hashy: self.hashy,
        }
    }

    /// Whether this reports `PersistentHashMap` (else array-map).
    #[must_use]
    pub fn is_hashy(&self) -> bool {
        self.hashy
    }

    /// Force the reported class (`hash-map` forces hash,
    /// `array-map`/transient-array forces array).
    #[must_use]
    pub fn with_hashy(mut self, hashy: bool) -> Self {
        self.hashy = hashy;
        self
    }

    /// Look up a key.
    #[must_use]
    pub fn get(&self, k: &Value) -> Option<&Value> {
        self.inner.get(k)
    }

    /// Whether a key is present.
    #[must_use]
    pub fn contains_key(&self, k: &Value) -> bool {
        self.inner.get(k).is_some()
    }

    /// The entry count.
    #[must_use]
    pub fn size(&self) -> usize {
        self.inner.size()
    }

    /// Whether the map is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.size() == 0
    }

    /// Iterate entries in insertion order (the JVM small-map seq
    /// order; see the `keys` sidecar).
    pub fn iter(&self) -> impl Iterator<Item = (&Value, &Value)> {
        self.keys
            .iter()
            .filter_map(|k| self.inner.get(k).map(|v| (k, v)))
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Return this map with `meta` attached (`None` clears it).
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}

impl Set {
    /// Build a set from items, duplicate-checking them with the JVM
    /// literal rules: `Err` is the NEW (repeated) item.
    pub fn from_items_checked(items: Vec<Value>) -> Result<Self, Value> {
        if let Some(dup) = find_set_dup(&items) {
            return Err(dup.clone());
        }
        let mut set = new_set();
        for v in items {
            set = set.insert(v);
        }
        Ok(set)
    }

    /// Add a value. A new member appends to the insertion order;
    /// a present member is a no-op (keeps the original object).
    #[must_use]
    pub fn insert(self, v: Value) -> Self {
        if self.inner.contains(&v) {
            self
        } else {
            let mut members = self.members;
            members.push(v.clone());
            Self {
                inner: self.inner.insert(v),
                meta: self.meta,
                members,
            }
        }
    }

    /// Whether a value is present.
    #[must_use]
    pub fn contains(&self, v: &Value) -> bool {
        self.inner.contains(v)
    }

    /// The stored member equal to `v`, if present (`get` on a set
    /// returns the member, not the key).
    #[must_use]
    pub fn get(&self, v: &Value) -> Option<&Value> {
        self.members.iter().find(|m| *m == v)
    }

    /// Remove a value (`disj` one item). A missing value is a no-op;
    /// otherwise both the entries and the insertion-order sidecar
    /// drop it, and later positions shift down.
    #[must_use]
    pub fn without(mut self, v: &Value) -> Self {
        if !self.inner.contains(v) {
            return self;
        }
        self.members.retain(|m| m != v);
        Self {
            inner: self.inner.remove(v),
            meta: self.meta,
            members: self.members,
        }
    }

    /// The element count.
    #[must_use]
    pub fn size(&self) -> usize {
        self.inner.size()
    }

    /// Whether the set is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.size() == 0
    }

    /// Iterate members in insertion order (the JVM small-set seq
    /// order; see the `members` sidecar).
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.members.iter()
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Return this set with `meta` attached (`None` clears it).
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}

// Equality and hashing ignore metadata on all four newtypes.
impl PartialEq for List {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for List {}

impl Hash for List {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for v in self.iter() {
            v.hash(state);
        }
    }
}

impl PartialEq for Vector {
    fn eq(&self, other: &Self) -> bool {
        self.iter().eq(other.iter())
    }
}

impl Eq for Vector {}

impl Hash for Vector {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.len());
        for v in self.iter() {
            v.hash(state);
        }
    }
}

impl PartialEq for Map {
    fn eq(&self, other: &Self) -> bool {
        self.size() == other.size() && self.iter().all(|(k, v)| other.get(k) == Some(v))
    }
}

impl Eq for Map {}

impl Hash for Map {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.size());
        let mut acc: u64 = 0;
        for (k, v) in self.iter() {
            acc = acc.wrapping_add(entry_hash(k, v));
        }
        state.write_u64(acc);
    }
}

impl PartialEq for Set {
    fn eq(&self, other: &Self) -> bool {
        self.size() == other.size() && self.iter().all(|v| other.contains(v))
    }
}

impl Eq for Set {}

impl Hash for Set {
    fn hash<H: Hasher>(&self, state: &mut H) {
        state.write_usize(self.size());
        let mut acc: u64 = 0;
        for v in self.iter() {
            acc = acc.wrapping_add(entry_hash(v, v));
        }
        state.write_u64(acc);
    }
}

/// An empty [`Map`] (rpds's `new_sync` only covers the default
/// hasher, so construction goes through the explicit hasher form).
#[must_use]
pub fn new_map() -> Map {
    Map {
        inner: RpdsMap::new_with_hasher_and_ptr_kind(FxBuildHasher::default()),
        meta: None,
        keys: Vec::new(),
        hashy: false,
    }
}

/// An empty [`Set`].
#[must_use]
pub fn new_set() -> Set {
    Set {
        inner: RpdsSet::new_with_hasher_with_ptr_kind(FxBuildHasher::default()),
        meta: None,
        members: Vec::new(),
    }
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
    /// `#tag form` under `:read-cond :preserve` (or suppressed reads).
    TaggedLiteral { tag: Symbol, form: Box<Value> },
    /// `#?` / `#?@` under `:read-cond :preserve`.
    ReaderConditional { form: Box<Value>, splicing: bool },
    /// A namespace Var (S4): identity-compared by `id`, like the JVM.
    Var(Var),
    /// An unbound Var's sentinel root (S4): compared by `var_id`.
    Unbound(Unbound),
    /// A throwable (S4): identity-compared by `id`, like the JVM.
    Throwable(Throwable),
    /// A transient buffer (S5D): identity equality (shared cell).
    Transient(Transient),
    /// An atom (S6A): identity equality (shared cell).
    Atom(Atom),
    /// A delay (S6A): identity equality (shared cell).
    Delay(Delay),
    /// A future (S6B): identity equality (shared cell).
    Future(Future),
    /// A promise (S6B): identity equality (shared cell).
    Promise(Promise),
    /// A host-defined value behind the [`ExtensionValue`] trait (S4).
    Extension(Arc<dyn ExtensionValue>),
}

impl Value {
    /// Clojure truthiness: only `nil` and `false` are false.
    #[must_use]
    pub fn is_truthy(&self) -> bool {
        !matches!(self, Value::Nil | Value::Bool(false))
    }

    /// The metadata map, if this value carries one (Symbol, List,
    /// Vector, Map, Set, realized seqs — never Keyword).
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        match self {
            Value::Symbol(s) => s.meta(),
            Value::List(l) => l.meta(),
            Value::Vector(v) => v.meta(),
            Value::Map(m) => m.meta(),
            Value::Set(s) => s.meta(),
            Value::Extension(e) => {
                let any = e.as_any();
                any.downcast_ref::<crate::RealizedSeq>()
                    .and_then(crate::RealizedSeq::meta)
                    .or_else(|| {
                        any.downcast_ref::<crate::RangeValue>()
                            .and_then(crate::RangeValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::RepeatValue>()
                            .and_then(crate::RepeatValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::CycleValue>()
                            .and_then(crate::CycleValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::IterateValue>()
                            .and_then(crate::IterateValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::ConsValue>()
                            .and_then(crate::ConsValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::KeySeqValue>()
                            .and_then(crate::KeySeqValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::ValSeqValue>()
                            .and_then(crate::ValSeqValue::meta)
                    })
                    .or_else(|| {
                        any.downcast_ref::<crate::SubVectorValue>()
                            .and_then(crate::SubVectorValue::meta)
                    })
            }
            _ => None,
        }
    }

    /// Attach `meta` to this value, which must support metadata
    /// ([`meta`](Value::meta)); other values are returned unchanged.
    #[must_use]
    pub fn with_meta(self, meta: Option<Map>) -> Self {
        match self {
            Value::Symbol(s) => Value::Symbol(s.with_meta(meta)),
            Value::List(l) => Value::List(l.with_meta(meta)),
            Value::Vector(v) => Value::Vector(v.with_meta(meta)),
            Value::Map(m) => Value::Map(m.with_meta(meta)),
            Value::Set(s) => Value::Set(s.with_meta(meta)),
            Value::Extension(e) => {
                let any = e.as_any();
                macro_rules! retag {
                    ($t:ty) => {
                        if let Some(v) = any.downcast_ref::<$t>() {
                            return Value::Extension(std::sync::Arc::new(
                                v.clone().with_meta(meta),
                            ));
                        }
                    };
                }
                retag!(crate::RealizedSeq);
                retag!(crate::RangeValue);
                retag!(crate::RepeatValue);
                retag!(crate::CycleValue);
                retag!(crate::IterateValue);
                retag!(crate::ConsValue);
                retag!(crate::KeySeqValue);
                retag!(crate::ValSeqValue);
                retag!(crate::SubVectorValue);
                Value::Extension(e)
            }
            other => other,
        }
    }
}

impl PartialEq for Value {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Value::Nil, Value::Nil) => true,
            (Value::Bool(a), Value::Bool(b)) => a == b,
            (Value::Int(a), Value::Int(b)) => a == b,
            // `=` on doubles is `==` (`0.0` ≡ `-0.0`, NaN never).
            // Mixed `Int`/`Double` is never `=` (`Numbers.equal`
            // requires one category; `(== 1 1.0)` is the value
            // comparison, arriving with numerics in S4).
            (Value::Double(a), Value::Double(b)) => a == b,
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
            // Realized seqs compare sequentially against
            // List/Vector/realized seqs (never maps, sets, or
            // pending seqs, which `=` realizes first).
            (a, b) if seq_items(a).is_some() || seq_items(b).is_some() => {
                match (seq_items(a), seq_items(b)) {
                    (Some(x), Some(y)) => x == y,
                    _ => false,
                }
            }
            (Value::Map(a), Value::Map(b)) => {
                a.size() == b.size() && a.iter().all(|(k, v)| b.get(k) == Some(v))
            }
            (Value::Set(a), Value::Set(b)) => {
                a.size() == b.size() && a.iter().all(|v| b.contains(v))
            }
            // Identity semantics like java.util.regex.Pattern.
            (Value::Regex(a), Value::Regex(b)) => Arc::ptr_eq(a, b),
            // `TaggedLiteral.equals`/`ReaderConditional.equals`
            // delegate to the form's `.equals` (doubles by bits, so
            // `[0.0]` ≠ `[-0.0]` but `[##NaN]` ≡ `[##NaN]`); `=`
            // reaches them through the `Util.equiv` `.equals`
            // fallback. The `Hash` impl stays `equiv`-based: bits-equal
            // forms hash equally, so the `Eq`/`Hash` contract holds.
            (
                Value::TaggedLiteral { tag: t1, form: f1 },
                Value::TaggedLiteral { tag: t2, form: f2 },
            ) => t1 == t2 && plain_equals(f1, f2),
            (
                Value::ReaderConditional {
                    form: f1,
                    splicing: s1,
                },
                Value::ReaderConditional {
                    form: f2,
                    splicing: s2,
                },
            ) => s1 == s2 && plain_equals(f1, f2),
            // S4 identity semantics, mirroring `=`: ids only, never
            // payloads; extensions dispatch through the trait.
            (Value::Var(a), Value::Var(b)) => a.id == b.id,
            (Value::Unbound(a), Value::Unbound(b)) => a.var_id == b.var_id,
            (Value::Throwable(a), Value::Throwable(b)) => a.id == b.id,
            (Value::Transient(a), Value::Transient(b)) => a.same_cell(b),
            (Value::Atom(a), Value::Atom(b)) => a.same_cell(b),
            (Value::Delay(a), Value::Delay(b)) => a.same_cell(b),
            (Value::Future(a), Value::Future(b)) => a.same_cell(b),
            (Value::Promise(a), Value::Promise(b)) => a.same_cell(b),
            (Value::Extension(a), Value::Extension(b)) => a.dyn_eq(&**b),
            _ => false,
        }
    }
}

// `Eq`'s reflexivity technically fails for NaN (never `=` to itself,
// exactly as in Clojure); the marker is required for map/set keys,
// where NaN behaves identically to the JVM: it hashes
// deterministically but never matches on lookup.
impl Eq for Value {}

/// Canonical bits for hashing doubles: `-0.0` hashes as `0.0`
/// (they are `=`), NaN its canonical bits, anything else its own
/// bits. (`Int` hashes separately — mixed numerics are never `=`.)
fn double_hash_bits(d: f64) -> u64 {
    if d.is_nan() {
        f64::NAN.to_bits()
    } else if d == 0.0 {
        0.0f64.to_bits()
    } else {
        d.to_bits()
    }
}

impl Hash for Value {
    fn hash<H: Hasher>(&self, state: &mut H) {
        // Family markers keep unequal types apart (`Int` and
        // `Double` hash separately — they are never `=`).
        match self {
            Value::Nil => state.write_u8(0),
            Value::Bool(b) => {
                state.write_u8(1);
                state.write_u8(u8::from(*b));
            }
            Value::Int(i) => {
                state.write_u8(2);
                state.write_i64(*i);
            }
            Value::Double(d) => {
                state.write_u8(3);
                state.write_u64(double_hash_bits(*d));
            }
            Value::Str(s) => {
                state.write_u8(4);
                s.hash(state);
            }
            Value::Char(c) => {
                state.write_u8(5);
                c.hash(state);
            }
            Value::Symbol(s) => {
                state.write_u8(6);
                s.hash(state);
            }
            Value::Keyword(k) => {
                state.write_u8(7);
                k.hash(state);
            }
            // Sequential hash ignores List-vs-Vector (they can be `=`).
            Value::List(l) => {
                state.write_u8(8);
                l.hash(state);
            }
            Value::Vector(v) => {
                state.write_u8(8);
                v.hash(state);
            }
            // Order-insensitive: sums commute, so iteration order
            // cannot leak into the hash.
            Value::Map(m) => {
                state.write_u8(9);
                m.hash(state);
            }
            Value::Set(s) => {
                state.write_u8(10);
                s.hash(state);
            }
            // Same-pointer values share the pattern string, so the
            // string hash is consistent with pointer equality.
            Value::Regex(pattern) => {
                state.write_u8(11);
                pattern.hash(state);
            }
            Value::TaggedLiteral { tag, form } => {
                state.write_u8(12);
                tag.hash(state);
                form.hash(state);
            }
            Value::ReaderConditional { form, splicing } => {
                state.write_u8(13);
                splicing.hash(state);
                form.hash(state);
            }
            Value::Var(v) => {
                state.write_u8(14);
                state.write_u64(v.id);
            }
            Value::Unbound(u) => {
                state.write_u8(15);
                state.write_u64(u.var_id);
            }
            Value::Throwable(t) => {
                state.write_u8(16);
                state.write_u64(t.id);
            }
            Value::Transient(t) => {
                state.write_u8(17);
                state.write_u64(t.id());
            }
            Value::Atom(a) => {
                state.write_u8(18);
                state.write_u64(a.id());
            }
            Value::Delay(d) => {
                state.write_u8(19);
                state.write_u64(d.id());
            }
            Value::Future(f) => {
                state.write_u8(20);
                state.write_u64(f.id());
            }
            Value::Promise(p) => {
                state.write_u8(21);
                state.write_u64(p.id());
            }
            Value::Extension(e) => {
                // Realized seqs hash as sequential family (marker 8
                // plus length-prefixed elements — identical writes
                // to `List`, so sequential equals hash equally).
                if let Some(s) = e.as_any().downcast_ref::<crate::RealizedSeq>() {
                    state.write_u8(8);
                    let items = s.items();
                    items.len().hash(state);
                    for item in &items {
                        item.hash(state);
                    }
                } else if let Some(r) = e.as_any().downcast_ref::<crate::RangeValue>() {
                    state.write_u8(8);
                    r.walked_len().hash(state);
                    r.for_each(&mut |v| v.hash(state));
                } else if let Some(r) = e.as_any().downcast_ref::<crate::RepeatValue>() {
                    state.write_u8(8);
                    r.walked_len().hash(state);
                    r.for_each(&mut |v| v.hash(state));
                } else if let Some(sv) = e.as_any().downcast_ref::<crate::SubVectorValue>() {
                    state.write_u8(8);
                    sv.len().hash(state);
                    for item in sv.iter() {
                        item.hash(state);
                    }
                } else {
                    state.write_u8(17);
                    e.dyn_hash(state);
                }
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

// ---------------------------------------------------------------------------
// Duplicate-key rules for collection literals
// ---------------------------------------------------------------------------

/// `PersistentArrayMap.canBePAM`: at most 16 forms, or at most 128
/// forms whose every key at even index ≥ 16 is a Keyword.
fn can_be_small_map(keys: &[&Value]) -> bool {
    let forms = keys.len() * 2;
    if forms <= 16 {
        return true;
    }
    if forms > 128 {
        return false;
    }
    keys.iter()
        .enumerate()
        .all(|(i, k)| i * 2 < 16 || matches!(k, Value::Keyword(_)))
}

/// Small-map scan (`PersistentArrayMap.createWithCheck`): pairwise
/// over earlier/later pairs, reporting the FIRST key.
fn find_map_dup<'a>(keys: &[&'a Value]) -> Option<&'a Value> {
    if can_be_small_map(keys) {
        for (i, a) in keys.iter().enumerate() {
            for b in &keys[i + 1..] {
                if equal_key(a, b) {
                    return Some(a);
                }
            }
        }
        None
    } else {
        // Big-map scan (`PersistentHashMap.createWithCheck`):
        // incremental, reporting the NEW key. The JVM looks keys up
        // by hash, but equivalent keys always hash equally, so a
        // linear scan finds exactly the same duplicates.
        for (i, b) in keys.iter().enumerate() {
            if keys[..i].iter().any(|a| equal_key(a, b)) {
                return Some(b);
            }
        }
        None
    }
}

/// Set scan (`PersistentHashSet.createWithCheck`, transient
/// `indexOf`): incremental over previous members, reporting the NEW
/// item — same hash/equiv equivalence argument as big maps.
fn find_set_dup(items: &[Value]) -> Option<&Value> {
    for (i, b) in items.iter().enumerate() {
        if items[..i].iter().any(|a| equal_key(a, b)) {
            return Some(b);
        }
    }
    None
}

/// `PersistentArrayMap.equalKey`: keywords compare by identity (value
/// equality under interning); everything else by `Util.equiv`.
fn equal_key(a: &Value, b: &Value) -> bool {
    if matches!(a, Value::Keyword(_)) {
        a == b
    } else {
        jvm_equiv(a, b)
    }
}

/// Port of `Util.equiv`, the comparison behind every duplicate check
/// (small maps pairwise, big maps and sets incrementally):
///
/// - Doubles compare by `==` (`0.0` ≡ `-0.0`); NaN ≡ NaN holds
///   because every S2 NaN is the one `##NaN` (on the JVM, the shared
///   `Double.NaN` instance, caught by the `k1 == k2` identity probe).
/// - Mixed `Int`/`Double` never compare: `Numbers.equal` requires
///   the same category.
/// - Collections compare through `pcequiv` (`ASeq.equiv` /
///   `doEquiv` / `APersistentMap.equiv` / `APersistentSet.equiv`),
///   recursing with full `equiv` — sequential comparison crosses
///   `List`/`Vector` in both orders, at every nesting level.
/// - Anything else falls back to Java `.equals` ([`plain_equals`]).
fn jvm_equiv(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Double(x), Value::Double(y)) => {
            if x.is_nan() && y.is_nan() {
                true
            } else {
                x == y
            }
        }
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Int(_), Value::Double(_)) | (Value::Double(_), Value::Int(_)) => false,
        _ => {
            if is_collection(a) {
                coll_equiv(a, b)
            } else if is_collection(b) {
                coll_equiv(b, a)
            } else {
                plain_equals(a, b)
            }
        }
    }
}

fn is_collection(v: &Value) -> bool {
    matches!(
        v,
        Value::List(_) | Value::Vector(_) | Value::Map(_) | Value::Set(_)
    )
}

/// Same length plus pairwise comparison (either comparator).
fn seq_eq<'a>(
    a_len: usize,
    a: impl Iterator<Item = &'a Value>,
    b_len: usize,
    b: impl Iterator<Item = &'a Value>,
    mut eq: impl FnMut(&'a Value, &'a Value) -> bool,
) -> bool {
    if a_len != b_len {
        return false;
    }
    let mut count = 0;
    for (x, y) in a.zip(b) {
        if !eq(x, y) {
            return false;
        }
        count += 1;
    }
    count == a_len
}

fn seq_plain_equals<'a>(
    a_len: usize,
    a: impl Iterator<Item = &'a Value>,
    b_len: usize,
    b: impl Iterator<Item = &'a Value>,
) -> bool {
    seq_eq(a_len, a, b_len, b, plain_equals)
}

/// `pcequiv` with `c` the collection side: sequential `equiv` crosses
/// `List`/`Vector`; maps match same-size with equiv keys and values;
/// sets match same-size with every member of the OTHER side
/// equiv-contained (`APersistentSet.equiv` iterates its argument).
fn coll_equiv(c: &Value, x: &Value) -> bool {
    match c {
        Value::List(items) => match x {
            Value::List(o) => seq_eq(items.len(), items.iter(), o.len(), o.iter(), jvm_equiv),
            Value::Vector(o) => seq_eq(items.len(), items.iter(), o.len(), o.iter(), jvm_equiv),
            _ => false,
        },
        Value::Vector(items) => match x {
            Value::List(o) => seq_eq(items.len(), items.iter(), o.len(), o.iter(), jvm_equiv),
            Value::Vector(o) => seq_eq(items.len(), items.iter(), o.len(), o.iter(), jvm_equiv),
            _ => false,
        },
        Value::Map(m) => {
            let Value::Map(o) = x else {
                return false;
            };
            if m.size() != o.size() {
                return false;
            }
            // `containsKey` + equiv value per entry: hash lookup
            // finds exactly the equiv keys, so scan linearly.
            m.iter().all(|(k, v)| {
                o.iter()
                    .find(|(k2, _)| jvm_equiv(k, k2))
                    .is_some_and(|(_, v2)| jvm_equiv(v, v2))
            })
        }
        Value::Set(s) => {
            let Value::Set(o) = x else {
                return false;
            };
            s.size() == o.size() && o.iter().all(|y| s.iter().any(|z| jvm_equiv(z, y)))
        }
        _ => false,
    }
}

/// Port of Java `.equals` (`Util.equals` + the collection
/// Sequential items for equality: lists, vectors, and realized
/// seqs (anything else — maps, sets, pending seqs — is not
/// sequential here).
fn seq_items(v: &Value) -> Option<Vec<Value>> {
    match v {
        Value::List(l) => Some(l.iter().cloned().collect()),
        Value::Vector(v) => Some(v.iter().cloned().collect()),
        Value::Extension(e) => {
            if let Some(s) = e.as_any().downcast_ref::<crate::RealizedSeq>() {
                return Some(s.items());
            }
            // Strict pure seqs walk without the evaluator
            // (infinite ones hang, like the JVM's `Util.equiv`).
            if let Some(r) = e.as_any().downcast_ref::<crate::RangeValue>() {
                let mut out = Vec::new();
                r.for_each(&mut |v| out.push(v.clone()));
                return Some(out);
            }
            if let Some(r) = e.as_any().downcast_ref::<crate::RepeatValue>() {
                let mut out = Vec::new();
                r.for_each(&mut |v| out.push(v.clone()));
                return Some(out);
            }
            if let Some(sv) = e.as_any().downcast_ref::<crate::SubVectorValue>() {
                return Some(sv.items());
            }
            None
        }
        _ => None,
    }
}

/// `equals`/`mapEquals`/`setEquals`): like [`jvm_equiv`] except
/// doubles compare by bits, and it is only reached for scalar pairs
/// or nested inside [`Value::TaggedLiteral`]/[`Value::ReaderConditional`]
/// forms (whose `equals` delegates to the form's `.equals`).
fn plain_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Nil, Value::Nil) => true,
        (Value::Bool(x), Value::Bool(y)) => x == y,
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Double(x), Value::Double(y)) => x.to_bits() == y.to_bits(),
        (Value::Str(x), Value::Str(y)) => x == y,
        (Value::Char(x), Value::Char(y)) => x == y,
        (Value::Symbol(x), Value::Symbol(y)) => x == y,
        (Value::Keyword(x), Value::Keyword(y)) => x == y,
        (Value::Regex(x), Value::Regex(y)) => Arc::ptr_eq(x, y),
        (
            Value::TaggedLiteral { tag: t1, form: f1 },
            Value::TaggedLiteral { tag: t2, form: f2 },
        ) => t1 == t2 && plain_equals(f1, f2),
        (
            Value::ReaderConditional {
                form: f1,
                splicing: s1,
            },
            Value::ReaderConditional {
                form: f2,
                splicing: s2,
            },
        ) => s1 == s2 && plain_equals(f1, f2),
        (Value::Var(a), Value::Var(b)) => a.id == b.id,
        (Value::Unbound(a), Value::Unbound(b)) => a.var_id == b.var_id,
        (Value::Throwable(a), Value::Throwable(b)) => a.id == b.id,
        (Value::Extension(a), Value::Extension(b)) => a.dyn_eq(&**b),
        // `.equals` is loose across List/Vector too (`doEquals` takes
        // the `java.util.List` branch; `ASeq.equals` accepts `List`).
        (Value::List(items), Value::List(o)) => {
            seq_plain_equals(items.len(), items.iter(), o.len(), o.iter())
        }
        (Value::List(items), Value::Vector(o)) => {
            seq_plain_equals(items.len(), items.iter(), o.len(), o.iter())
        }
        (Value::Vector(items), Value::List(o)) => {
            seq_plain_equals(items.len(), items.iter(), o.len(), o.iter())
        }
        (Value::Vector(items), Value::Vector(o)) => {
            seq_plain_equals(items.len(), items.iter(), o.len(), o.iter())
        }
        (Value::Map(m), Value::Map(o)) => {
            m.size() == o.size()
                && m.iter().all(|(k, v)| {
                    o.iter()
                        .find(|(k2, _)| plain_equals(k, k2))
                        .is_some_and(|(_, v2)| plain_equals(v, v2))
                })
        }
        (Value::Set(s), Value::Set(o)) => {
            s.size() == o.size() && o.iter().all(|y| s.iter().any(|z| plain_equals(z, y)))
        }
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::{Value, find_map_dup, find_set_dup, new_map, new_set};
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
        assert_eq!(Value::Str(Arc::from("a")), Value::Str(Arc::from("a")));
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
    fn numerics_equiv_and_hash() {
        // Same-type comparison (`(== 1 1.0)` is the value
        // comparison; `=` requires one category).
        assert_ne!(Value::Int(1), Value::Double(1.0));
        assert_ne!(Value::Int(0), Value::Double(-0.0));
        assert_ne!(
            Value::Int(9_007_199_254_740_993),
            Value::Double(9_007_199_254_740_993.0)
        );
        assert_hash_consistent(&Value::Int(1), &Value::Int(1));
        assert_hash_consistent(&Value::Double(1.5), &Value::Double(1.5));
        assert_hash_consistent(&Value::Double(0.0), &Value::Double(-0.0));
        assert_ne!(Value::Int(1), Value::Double(1.5));
        assert_ne!(Value::Double(1.5), Value::Double(2.5));
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
        // Mixed numerics are distinct keys: cross-type lookup misses.
        if let Value::Map(m) = &m {
            assert_eq!(m.get(&Value::Double(1.0)), None);
            assert_eq!(m.get(&Value::Int(1)), Some(&Value::Str(Arc::from("a"))));
            assert_eq!(m.size(), 2);
        } else {
            panic!("expected map");
        }
        let swapped = Value::Map(
            new_map()
                .insert(Value::Keyword(Keyword::new(None, "k")), Value::Nil)
                .insert(Value::Int(1), Value::Str(Arc::from("a"))),
        );
        assert_hash_consistent(&m, &swapped);
        // `{1 :a 1.0 :b}` keeps both keys.
        let mixed = new_map()
            .insert(Value::Int(1), Value::Str(Arc::from("a")))
            .insert(Value::Double(1.0), Value::Str(Arc::from("b")));
        assert_eq!(mixed.size(), 2);
        assert_eq!(mixed.get(&Value::Int(1)), Some(&Value::Str(Arc::from("a"))));
        assert_eq!(
            mixed.get(&Value::Double(1.0)),
            Some(&Value::Str(Arc::from("b")))
        );
    }

    #[test]
    fn set_equiv() {
        let a = Value::Set(new_set().insert(Value::Int(1)).insert(Value::Int(2)));
        let b = Value::Set(new_set().insert(Value::Int(2)).insert(Value::Int(1)));
        assert_hash_consistent(&a, &b);
        if let Value::Set(s) = &a {
            assert!(!s.contains(&Value::Double(1.0)));
            assert!(s.contains(&Value::Int(1)));
            assert!(!s.contains(&Value::Int(3)));
        } else {
            panic!("expected set");
        }
        // `#{1 1.0}` keeps both members.
        let mixed = new_set().insert(Value::Int(1)).insert(Value::Double(1.0));
        assert_eq!(mixed.size(), 2);
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
    fn tagged_and_conditional_equiv_and_hash() {
        let tl = |form| Value::TaggedLiteral {
            tag: Symbol::new(None, "inst"),
            form: Box::new(form),
        };
        assert_hash_consistent(&tl(Value::Int(1)), &tl(Value::Int(1)));
        assert_ne!(tl(Value::Int(1)), tl(Value::Int(2)));
        assert_ne!(
            tl(Value::Int(1)),
            Value::TaggedLiteral {
                tag: Symbol::new(None, "uuid"),
                form: Box::new(Value::Int(1)),
            }
        );
        let rc = |splicing| Value::ReaderConditional {
            form: Box::new(Value::Int(1)),
            splicing,
        };
        assert_hash_consistent(&rc(false), &rc(false));
        assert_ne!(rc(false), rc(true));
        assert_ne!(tl(Value::Int(1)), rc(false));
    }

    #[test]
    fn meta_ignored_by_collection_eq_and_hash() {
        let meta = new_map().insert(Value::Keyword(Keyword::new(None, "k")), Value::Bool(true));
        let a = Value::Vector(vec_into_vector(vec![Value::Int(1)]));
        let b = Value::Vector(vec_into_vector(vec![Value::Int(1)]).with_meta(Some(meta)));
        assert_hash_consistent(&a, &b);
        assert!(a.meta().is_none());
        assert!(b.meta().is_some());
        // Keywords and scalars never carry meta.
        assert!(Value::Keyword(Keyword::new(None, "k")).meta().is_none());
        assert!(Value::Int(1).meta().is_none());
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
        super::List::from_vec(items)
    }

    fn vec_into_vector(items: Vec<Value>) -> super::Vector {
        super::Vector::from_vec(items)
    }

    // -- duplicate rules --

    fn kw(name: &str) -> Value {
        Value::Keyword(Keyword::new(None, name))
    }

    fn map_keys(pairs: &[(Value, Value)]) -> Vec<&Value> {
        pairs.iter().map(|(k, _)| k).collect()
    }

    #[test]
    fn small_map_reports_first() {
        let pairs = vec![
            (Value::Double(0.0), kw("a")),
            (Value::Double(-0.0), kw("b")),
        ];
        let dup = find_map_dup(&map_keys(&pairs)).unwrap();
        assert_eq!(dup, &Value::Double(0.0));
        // Keywords by value.
        let pairs = vec![(kw("a"), Value::Int(1)), (kw("a"), Value::Int(2))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), Some(&kw("a")));
        // No duplicates.
        let pairs = vec![(Value::Int(1), kw("a")), (Value::Double(1.0), kw("b"))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), None);
    }

    #[test]
    fn small_map_seq_cross_both_orders() {
        // Oracle: `{[1] :a (1) :b}` and `{(1) :a [1] :b}` both report
        // the FIRST key (full `equiv`, no order asymmetry).
        let list1 = Value::List(vec_into_list(vec![Value::Int(1)]));
        let vec1 = Value::Vector(vec_into_vector(vec![Value::Int(1)]));
        let pairs = vec![(vec1.clone(), kw("a")), (list1.clone(), kw("b"))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), Some(&vec1));
        let pairs = vec![(list1.clone(), kw("a")), (vec1.clone(), kw("b"))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), Some(&list1));
        // Nested doubles use `==`, not bits: `{(0.0) :a [-0.0] :b}` dups.
        let l0 = Value::List(vec_into_list(vec![Value::Double(0.0)]));
        let vn0 = Value::Vector(vec_into_vector(vec![Value::Double(-0.0)]));
        let pairs = vec![(l0.clone(), kw("a")), (vn0.clone(), kw("b"))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), Some(&l0));
    }

    #[test]
    fn big_map_reports_new() {
        // 18 forms: past the 16-form small-map cutoff.
        let mut pairs: Vec<(Value, Value)> =
            (1..8).map(|i| (Value::Int(i), Value::Int(i))).collect();
        pairs.insert(0, (Value::Double(0.0), Value::Int(0)));
        pairs.push((Value::Double(-0.0), kw("b")));
        let keys = map_keys(&pairs);
        assert_eq!(keys.len(), 9);
        let dup = find_map_dup(&keys).unwrap();
        assert_eq!(dup, &Value::Double(-0.0));
    }

    #[test]
    fn keyword_run_stays_small() {
        // 18 forms, all keys keywords: small-map rule reports FIRST.
        let mut pairs: Vec<(Value, Value)> = (0..8)
            .map(|i| (kw(&format!("k{i}")), Value::Int(i)))
            .collect();
        pairs.push((kw("k0"), Value::Int(9)));
        let dup = find_map_dup(&map_keys(&pairs)).unwrap();
        assert_eq!(dup, &kw("k0"));
    }

    #[test]
    fn sets_report_new() {
        let items = vec![Value::Double(0.0), Value::Double(-0.0)];
        assert_eq!(find_set_dup(&items), Some(&Value::Double(-0.0)));
        assert_eq!(find_set_dup(&[Value::Int(1), Value::Double(1.0)]), None);
        // Distinct regex sources never collide (identity).
        assert_eq!(
            find_set_dup(&[Value::Regex(Arc::from("a")), Value::Regex(Arc::from("a"))]),
            None
        );
        // NaN ≡ NaN in dup checks (the shared `##NaN`).
        assert!(matches!(
            find_set_dup(&[Value::Double(f64::NAN), Value::Double(f64::NAN)]),
            Some(Value::Double(d)) if d.is_nan()
        ));
    }

    #[test]
    fn tagged_forms_compare_by_bits() {
        // Oracle: `{#inst [0.0] 1 #inst [-0.0] 2}` reads fine — the
        // tagged form compares with Java `.equals` (doubles by bits).
        let tl = |v: f64| Value::TaggedLiteral {
            tag: Symbol::new(None, "inst"),
            form: Box::new(Value::Vector(vec_into_vector(vec![Value::Double(v)]))),
        };
        let pairs = vec![(tl(0.0), Value::Int(1)), (tl(-0.0), Value::Int(2))];
        assert_eq!(find_map_dup(&map_keys(&pairs)), None);
        let pairs = vec![(tl(0.0), Value::Int(1)), (tl(0.0), Value::Int(2))];
        assert_eq!(find_map_dup(&map_keys(&pairs)).unwrap(), &tl(0.0));
        // `=` is bits too (`tl[NaN]` ≡ `tl[NaN]`, unlike plain NaN).
        assert_ne!(tl(0.0), tl(-0.0));
        assert_hash_consistent(&tl(f64::NAN), &tl(f64::NAN));
        // The checked constructors must not collapse bits-distinct
        // tagged keys when inserting (rpds `Eq`-based overwrite).
        let map =
            super::Map::from_kvs_checked(vec![(tl(0.0), Value::Int(1)), (tl(-0.0), Value::Int(2))])
                .unwrap();
        assert_eq!(map.size(), 2);
        let set = super::Set::from_items_checked(vec![tl(0.0), tl(-0.0)]).unwrap();
        assert_eq!(set.size(), 2);
    }

    #[test]
    fn insertion_order_sidecars() {
        // Maps iterate in insertion order (JVM small-map seq).
        let m =
            super::Map::from_kvs_checked(vec![(kw("b"), Value::Int(1)), (kw("a"), Value::Int(2))])
                .unwrap();
        let keys: Vec<Value> = m.iter().map(|(k, _)| k.clone()).collect();
        assert_eq!(keys, vec![kw("b"), kw("a")]);
        // Re-inserting keeps the original position, new value.
        let m2 = m.insert(kw("b"), Value::Int(9));
        assert_eq!(m2.get(&kw("b")), Some(&Value::Int(9)));
        let pairs: Vec<(Value, Value)> = m2.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
        assert_eq!(
            pairs,
            vec![(kw("b"), Value::Int(9)), (kw("a"), Value::Int(2))]
        );
        // Sets iterate in insertion order; re-inserting is a no-op.
        let s = super::Set::from_items_checked(vec![Value::Int(2), Value::Int(1)]).unwrap();
        let items: Vec<i64> = s
            .iter()
            .map(|v| match v {
                Value::Int(i) => *i,
                _ => unreachable!(),
            })
            .collect();
        assert_eq!(items, vec![2, 1]);
        let s2 = s.insert(Value::Int(2));
        assert_eq!(s2.size(), 2);
        // Order-insensitive equality still holds across orders.
        let a =
            super::Map::from_kvs_checked(vec![(kw("b"), Value::Int(1)), (kw("a"), Value::Int(2))])
                .unwrap();
        let b =
            super::Map::from_kvs_checked(vec![(kw("a"), Value::Int(2)), (kw("b"), Value::Int(1))])
                .unwrap();
        assert_hash_consistent(&Value::Map(a), &Value::Map(b));
    }

    #[test]
    fn checked_constructors() {
        assert!(super::Map::from_kvs_checked(vec![(kw("a"), Value::Int(1))]).is_ok());
        assert_eq!(
            super::Map::from_kvs_checked(vec![(kw("a"), Value::Int(1)), (kw("a"), Value::Int(2))]),
            Err(kw("a"))
        );
        assert!(super::Set::from_items_checked(vec![Value::Int(1)]).is_ok());
        assert_eq!(
            super::Set::from_items_checked(vec![Value::Int(1), Value::Int(1)]),
            Err(Value::Int(1))
        );
    }
}
