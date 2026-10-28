// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Volatile cells (S10): single-threaded shared cells mirroring
//! `clojure.lang.Volatile` (`docs/zero-oos-s10-cells/specification.md`
//! §1a).
//!
//! The cell holds the current value only — no watches, validators,
//! or meta (the probes pin the minus: `c-meta` nil, identity-only
//! equality, neither IFn nor counted). Single-threaded semantics
//! need no lock protocol beyond the mutex itself (the established
//! interior mutability — `xform.rs` counters). Equality is
//! shared-cell identity; printing follows the transient
//! `#object[…]` convention (never the JVM's `{:status …}`
//! rendering — see the S10 spec §2f).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};

use crate::Value;

/// Fresh volatile identities (print-only; equality is shared-cell).
static VOLATILE_ID: AtomicU64 = AtomicU64::new(1);

/// A first-class volatile value: an identity plus the shared cell.
/// Identity equality (same cell); `counted?` is false, like the JVM.
#[derive(Clone, Debug)]
pub struct Volatile {
    id: u64,
    cell: Arc<Mutex<Value>>,
}

impl Volatile {
    /// A volatile over `value` (fresh identity).
    #[must_use]
    pub fn new(value: Value) -> Self {
        Self {
            id: VOLATILE_ID.fetch_add(1, Ordering::Relaxed),
            cell: Arc::new(Mutex::new(value)),
        }
    }

    /// The print identity.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
    }

    /// Whether `other` shares this volatile's cell.
    #[must_use]
    pub fn same_cell(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.cell, &other.cell)
    }

    /// Lock the cell (short critical sections only — poison is
    /// unreachable).
    pub fn lock(&self) -> MutexGuard<'_, Value> {
        self.cell.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The JVM class name.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        "clojure.lang.Volatile"
    }
}
