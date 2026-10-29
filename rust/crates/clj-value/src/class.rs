// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! A JVM class as a value (S16): `ClassVal` carries the binary name,
//! the interface flag, and the ordered direct-supertype names (the
//! `getSuperclass`/`getInterfaces` order from
//! `docs/zero-oos-s16-classemu/probes/lattice.out`). The lattice
//! itself (name → `(iface, bases)`, dynamic rules, closure walks)
//! lives in `clj-eval`'s `classemu` module; this struct is the inert
//! carrier shared across crates.

use std::sync::Arc;

/// A JVM class as a value: binary name, interface flag, and ordered
/// direct-supertype (base) names. Equality and hashing are by name
/// alone (see `Value`'s impls); classes carry no metadata.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClassVal {
    name: Arc<str>,
    iface: bool,
    bases: Arc<[Arc<str>]>,
}

impl ClassVal {
    /// A class value with the given binary name, interface flag, and
    /// ordered direct-supertype names.
    #[must_use]
    pub fn new(name: Arc<str>, iface: bool, bases: Arc<[Arc<str>]>) -> Self {
        Self { name, iface, bases }
    }

    /// The binary name (`java.lang.String`, `user.Point`).
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Whether the class is an interface.
    #[must_use]
    pub fn is_iface(&self) -> bool {
        self.iface
    }

    /// The ordered direct-supertype names (superclass first for
    /// classes, interfaces only — possibly empty — for interfaces).
    #[must_use]
    pub fn bases(&self) -> &[Arc<str>] {
        &self.bases
    }
}
