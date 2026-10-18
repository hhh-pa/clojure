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

use std::fmt;
use std::sync::Arc;

/// A Clojure symbol such as `foo` or `clojure.core/map`.
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct Symbol {
    ns: Option<Arc<str>>,
    name: Arc<str>,
}

impl Symbol {
    /// Build a symbol from namespace and name parts.
    #[must_use]
    pub fn new(ns: Option<&str>, name: &str) -> Self {
        Self {
            ns: ns.map(Arc::from),
            name: Arc::from(name),
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
    fn slash_only_symbol() {
        // `/` alone is a valid symbol (division fn).
        assert_eq!(Symbol::new(None, "/").to_string(), "/");
    }
}
