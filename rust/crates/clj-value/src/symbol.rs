// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Clojure symbols: an optional namespace plus a name, both shared.
//!
//! Unlike the JVM's `Symbol.intern`, construction does not globally
//! canonicalize: two separately built symbols compare by value, never
//! by identity. (Global interning is a follow-up if profiling ever
//! demands it.)
//!
//! Symbols carry optional metadata ([`with_meta`](Symbol::with_meta));
//! equality, hashing, and ordering ignore it, exactly like the JVM's
//! `Symbol.equals`/`hasheq`/`compareTo`.

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

use crate::Map;

/// A Clojure symbol such as `foo` or `clojure.core/map`.
#[derive(Clone, Debug)]
pub struct Symbol {
    ns: Option<Arc<str>>,
    name: Arc<str>,
    meta: Option<Arc<Map>>,
}

impl Symbol {
    /// Build a symbol from namespace and name parts (no metadata).
    #[must_use]
    pub fn new(ns: Option<&str>, name: &str) -> Self {
        Self {
            ns: ns.map(Arc::from),
            name: Arc::from(name),
            meta: None,
        }
    }

    /// `Symbol::intern` semantics (`docs/t1174/reader-contract.md` §4):
    /// split on the FIRST `/` (`a/b/c` ⇒ ns `a`, name `b/c`; `/`
    /// alone ⇒ no-ns name `/`). Used by the reader; API users who
    /// already split the parts want [`new`](Symbol::new).
    #[must_use]
    pub fn intern(s: &str) -> Self {
        match s.split_once('/') {
            None => Self::new(None, s),
            Some(("", _)) if s.len() == 1 => Self::new(None, s),
            Some((ns, name)) => Self::new(Some(ns), name),
        }
    }

    /// The namespace part, if any.
    #[must_use]
    pub fn namespace(&self) -> Option<&str> {
        self.ns.as_deref()
    }

    /// The name part.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The metadata map, if any.
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_deref()
    }

    /// Return this symbol with `meta` attached (`None` clears it).
    #[must_use]
    pub fn with_meta(mut self, meta: Option<Map>) -> Self {
        self.meta = meta.map(Arc::new);
        self
    }
}

// Equality, hashing, and ordering cover `(ns, name)` only: metadata
// is invisible, mirroring the JVM.
impl PartialEq for Symbol {
    fn eq(&self, other: &Self) -> bool {
        self.ns == other.ns && self.name == other.name
    }
}

impl Eq for Symbol {}

impl Hash for Symbol {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ns.hash(state);
        self.name.hash(state);
    }
}

impl PartialOrd for Symbol {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Symbol {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.ns.as_deref(), self.name.as_ref()).cmp(&(other.ns.as_deref(), other.name.as_ref()))
    }
}

impl fmt::Display for Symbol {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        if let Some(ns) = &self.ns {
            write!(f, "{ns}/")?;
        }
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::Symbol;

    #[test]
    fn parts_and_display() {
        let plain = Symbol::new(None, "map");
        assert_eq!(plain.namespace(), None);
        assert_eq!(plain.name(), "map");
        assert_eq!(plain.to_string(), "map");

        let qualified = Symbol::new(Some("clojure.core"), "map");
        assert_eq!(qualified.namespace(), Some("clojure.core"));
        assert_eq!(qualified.to_string(), "clojure.core/map");
    }

    #[test]
    fn equality_is_by_value() {
        assert_eq!(Symbol::new(None, "a"), Symbol::new(None, "a"));
        assert_ne!(Symbol::new(None, "a"), Symbol::new(None, "b"));
        assert_ne!(Symbol::new(Some("n"), "a"), Symbol::new(None, "a"));
        assert_ne!(Symbol::new(Some("n"), "a"), Symbol::new(Some("m"), "a"));
    }

    #[test]
    fn ordering_compares() {
        assert!(Symbol::new(None, "a") < Symbol::new(None, "b"));
        assert!(Symbol::new(None, "b") > Symbol::new(None, "a"));
    }

    #[test]
    fn slash_only_symbol() {
        // `/` alone is a valid symbol (division fn).
        assert_eq!(Symbol::new(None, "/").to_string(), "/");
    }

    #[test]
    fn intern_splits_on_first_slash() {
        let s = Symbol::intern("a/b/c");
        assert_eq!(s.namespace(), Some("a"));
        assert_eq!(s.name(), "b/c");
        let slash = Symbol::intern("/");
        assert_eq!(slash.namespace(), None);
        assert_eq!(slash.name(), "/");
        let plain = Symbol::intern("x");
        assert_eq!(plain.namespace(), None);
        assert_eq!(plain.name(), "x");
        // No `/` rejection (unlike `new`, which takes pre-split parts).
        let trailing = Symbol::intern("a/");
        assert_eq!(trailing.namespace(), Some("a"));
        assert_eq!(trailing.name(), "");
    }

    #[test]
    fn meta_is_ignored_by_eq_hash_ord() {
        use crate::{Keyword, Value, new_map};
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        let meta = new_map().insert(Value::Keyword(Keyword::new(None, "k")), Value::Bool(true));
        let a = Symbol::new(None, "x");
        let b = Symbol::new(None, "x").with_meta(Some(meta));
        assert_eq!(a, b);
        assert_eq!(a.cmp(&b), std::cmp::Ordering::Equal);
        let hash_of = |s: &Symbol| {
            let mut h = DefaultHasher::new();
            s.hash(&mut h);
            h.finish()
        };
        assert_eq!(hash_of(&a), hash_of(&b));
        assert!(a.meta().is_none());
        assert_eq!(b.meta().unwrap().size(), 1);
        assert!(b.clone().with_meta(None).meta().is_none());
    }
}
