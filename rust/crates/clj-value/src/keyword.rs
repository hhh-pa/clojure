// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Clojure keywords: like symbols but self-evaluating, with a leading
//! colon in their printed form. Value-compared, never interned (see
//! [`Symbol`](crate::Symbol)).

use std::cmp::Ordering;
use std::fmt;
use std::hash::{Hash, Hasher};
use std::sync::Arc;

/// A Clojure keyword such as `:ok` or `:user/id`.
#[derive(Clone, Debug)]
pub struct Keyword {
    ns: Option<Arc<str>>,
    name: Arc<str>,
}

impl Keyword {
    /// Build a keyword from namespace and name parts.
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

// Manual impls over `(ns, name)`, mirroring `Symbol` (keywords never
// carry metadata, so these equal the derived ones; they are explicit
// so a future field cannot silently join the identity).
impl PartialEq for Keyword {
    fn eq(&self, other: &Self) -> bool {
        self.ns == other.ns && self.name == other.name
    }
}

impl Eq for Keyword {}

impl Hash for Keyword {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.ns.hash(state);
        self.name.hash(state);
    }
}

impl PartialOrd for Keyword {
    fn partial_cmp(&self, other: &Self) -> Option<Ordering> {
        Some(self.cmp(other))
    }
}

impl Ord for Keyword {
    fn cmp(&self, other: &Self) -> Ordering {
        (self.ns.as_deref(), self.name.as_ref()).cmp(&(other.ns.as_deref(), other.name.as_ref()))
    }
}

impl fmt::Display for Keyword {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(":")?;
        if let Some(ns) = &self.ns {
            write!(f, "{ns}/")?;
        }
        write!(f, "{}", self.name)
    }
}

#[cfg(test)]
mod tests {
    use super::Keyword;

    #[test]
    fn parts_and_display() {
        let plain = Keyword::new(None, "ok");
        assert_eq!(plain.namespace(), None);
        assert_eq!(plain.name(), "ok");
        assert_eq!(plain.to_string(), ":ok");

        let qualified = Keyword::new(Some("user"), "id");
        assert_eq!(qualified.to_string(), ":user/id");

        assert!(Keyword::new(None, "a") < Keyword::new(None, "b"));
        assert!(Keyword::new(None, "b") > Keyword::new(None, "a"));
    }

    #[test]
    fn equality_is_by_value() {
        assert_eq!(Keyword::new(None, "a"), Keyword::new(None, "a"));
        assert_ne!(Keyword::new(None, "a"), Keyword::new(Some("n"), "a"));
    }
}
