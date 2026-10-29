// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Primitive vectors (S15 `vector-of`): an immutable kind behind
//! the JVM `clojure.core.Vec` — a persistent vector whose items
//! are coerced to one of eight primitive types on the way in.
//! Items hold the *coerced* values (the unboxed storage is
//! unobservable once reads box); `conj`/`assoc`/`pop` answer new
//! vectors of the same type, like the JVM trie.

use std::sync::Arc;

use crate::{Map, Value};

type RpdsVector = rpds::VectorSync<Value>;

/// The eight `vector-of` element types.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Elem8 {
    /// `:boolean`.
    Boolean,
    /// `:byte`.
    Byte,
    /// `:char`.
    Char,
    /// `:short`.
    Short,
    /// `:int`.
    Int,
    /// `:long`.
    Long,
    /// `:float`.
    Float,
    /// `:double`.
    Double,
}

impl Elem8 {
    /// The keyword name (`:int`, …).
    #[must_use]
    pub fn keyword(self) -> &'static str {
        match self {
            Self::Boolean => ":boolean",
            Self::Byte => ":byte",
            Self::Char => ":char",
            Self::Short => ":short",
            Self::Int => ":int",
            Self::Long => ":long",
            Self::Float => ":float",
            Self::Double => ":double",
        }
    }
}

/// An immutable primitive vector: element type plus persistent
/// items plus metadata.
#[derive(Clone, Debug)]
pub struct PrimitiveVector {
    elem: Elem8,
    inner: RpdsVector,
    meta: Option<Arc<Map>>,
}

impl PrimitiveVector {
    /// A vector over `items` (already coerced by the caller).
    #[must_use]
    pub fn new(elem: Elem8, items: Vec<Value>, meta: Option<Arc<Map>>) -> Self {
        let mut inner = RpdsVector::new_sync();
        for v in items {
            inner = inner.push_back(v);
        }
        Self { elem, inner, meta }
    }

    /// An empty vector of `elem` (fresh, no meta).
    #[must_use]
    pub fn empty(elem: Elem8) -> Self {
        Self {
            elem,
            inner: RpdsVector::new_sync(),
            meta: None,
        }
    }

    /// The element type.
    #[must_use]
    pub fn elem(&self) -> Elem8 {
        self.elem
    }

    /// The item count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.inner.len()
    }

    /// Whether the vector is empty.
    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.inner.is_empty()
    }

    /// Iterate front to back.
    pub fn iter(&self) -> impl Iterator<Item = &Value> {
        self.inner.iter()
    }

    /// The item at `index` (`None` when out of bounds).
    #[must_use]
    pub fn get(&self, index: usize) -> Option<&Value> {
        self.inner.get(index)
    }

    /// The items in order.
    #[must_use]
    pub fn items(&self) -> Vec<Value> {
        self.iter().cloned().collect()
    }

    /// Append a (coerced) value.
    #[must_use]
    pub fn push_back(&self, v: Value) -> Self {
        Self {
            elem: self.elem,
            inner: self.inner.clone().push_back(v),
            meta: self.meta.clone(),
        }
    }

    /// Replace the item at `index` (`None` when out of bounds).
    #[must_use]
    pub fn set_nth(&self, index: usize, v: Value) -> Option<Self> {
        self.inner.set(index, v).map(|inner| Self {
            elem: self.elem,
            inner,
            meta: self.meta.clone(),
        })
    }

    /// The vector without its last item (`None` when empty).
    #[must_use]
    pub fn drop_last(&self) -> Option<Self> {
        self.inner.drop_last().map(|inner| Self {
            elem: self.elem,
            inner,
            meta: self.meta.clone(),
        })
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Rebuild with new metadata.
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}
