// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Atoms (S6A): shared mutable cells mirroring `clojure.lang.Atom`
//! (`docs/t1174/concurrency-contract.md`, `s6-pins.md` P2).
//!
//! The cell holds the current value, the `:validator` fn (if any),
//! the `:meta` map (if any), and the watches map (`{key fn}`, fired
//! in insertion order). Equality is shared-cell identity (atoms have
//! no value equality on the JVM); printing follows the transient
//! `#object[…]` convention (the JVM's `deref-as-map` rendering is
//! deliberately not reproduced — see the concurrency contract).
//! Metadata lives behind the lock (it is mutable via
//! `alter-meta!`/`reset-meta!`), so [`Value::meta`] — which borrows —
//! cannot serve atoms; readers use [`Atom::meta`] (a clone) instead.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::{Map, Value};

/// Fresh atom identities (print-only; equality is shared-cell).
static ATOM_ID: AtomicU64 = AtomicU64::new(1);

/// The mutable cell behind an [`Atom`]: the current value plus the
/// validator, metadata, watches, and publish generation.
#[derive(Clone, Debug)]
pub struct AtomState {
    /// The current value.
    pub value: Value,
    /// The validator fn (`None` = none installed — falsy `:validator`
    /// options never install, like the JVM's `setup-reference`).
    pub validator: Option<Value>,
    /// The metadata map (`None` = none).
    pub meta: Option<Map>,
    /// Watch fns keyed opaquely (nil fns install but never fire, like
    /// the JVM's `notifyWatches` null check).
    pub watches: Map,
}

/// A first-class atom value: an identity plus the shared cell.
/// Identity equality (same cell); `counted?` is false, like the JVM.
#[derive(Clone, Debug)]
pub struct Atom {
    id: u64,
    cell: Arc<Mutex<AtomState>>,
}

impl Atom {
    /// An atom over `value` (fresh identity, no validator, no meta,
    /// no watches).
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self {
            id: ATOM_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(Mutex::new(AtomState {
                value,
                validator: None,
                meta: None,
                watches: crate::new_map(),
            })),
        }
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this atom's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only; never across a
    /// `swap!` fn/validator/watch call — poison is unreachable).
    pub fn lock(&self) -> MutexGuard<'_, AtomState> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The metadata map, if any (cloned from behind the lock, for
    /// `meta` and `*print-meta*`).
    #[must_use]
    pub fn meta(&self) -> Option<Map> {
        self.lock().meta.clone()
    }

    /// The JVM class name.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.lang.Atom"
    }
}
