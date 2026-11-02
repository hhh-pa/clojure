// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Host object values: `PersistentQueue` queues — persistent
//! values (`conj` appends, `peek`/`pop` take the front) behind
//! `clojure.lang.PersistentQueue/EMPTY`. Queues print
//! `#object[…]` and compare/hash sequentially (like the JVM:
//! `(= pq [1 2])` is true — oracles `/tmp/pqsem.clj`,
//! `/tmp/pqsem2.clj`).

use std::hash::Hasher;
use std::sync::Arc;

use clj_value::{ExtensionValue, Value, Vector};

/// A `clojure.lang.PersistentQueue`: items front-to-back in a
/// persistent vector. A value — every op returns a fresh handle
/// — with sequential `=`/`hash` (queues equal vectors/lists
/// with the same items, like the JVM). `pop` of empty stays
/// empty (oracle `/tmp/queue3.clj`).
#[derive(Debug)]
pub(crate) struct QueueValue {
    items: Vector,
}

impl QueueValue {
    /// The empty queue (`PersistentQueue/EMPTY`).
    pub(crate) fn empty() -> Value {
        Value::Extension(Arc::new(Self {
            items: Vector::from_vec(Vec::new()),
        }))
    }

    /// The queue with `item` appended (metadata preserved, like
    /// the JVM's `cons`).
    pub(crate) fn conj(&self, item: Value) -> Value {
        Value::Extension(Arc::new(Self {
            items: self.items.clone().push_back(item),
        }))
    }

    /// The queue without its front item (empty stays empty).
    pub(crate) fn pop(&self) -> Value {
        let items: Vec<Value> = self.items.iter().skip(1).cloned().collect();
        Value::Extension(Arc::new(Self {
            items: Vector::from_vec(items).with_meta(self.items.meta().cloned()),
        }))
    }

    /// The front item, if any.
    pub(crate) fn peek(&self) -> Option<&Value> {
        self.items.iter().next()
    }

    /// The item count.
    pub(crate) fn len(&self) -> usize {
        self.items.len()
    }

    /// The item at `index` front-to-back, if in bounds.
    pub(crate) fn nth(&self, index: usize) -> Option<Value> {
        self.items.iter().nth(index).cloned()
    }

    /// The items front-to-back.
    pub(crate) fn items(&self) -> Vec<Value> {
        self.items.iter().cloned().collect()
    }
}

impl ExtensionValue for QueueValue {
    fn class_name(&self) -> String {
        "clojure.lang.PersistentQueue".to_owned()
    }

    fn print(&self) -> String {
        let parts: Vec<String> = self
            .items
            .iter()
            .map(|v| clj_value::pr_to_string(v, &clj_value::PrintOpts::pr()))
            .collect();
        format!(
            "#object[clojure.lang.PersistentQueue 0x{:x} \"({})\"]",
            self as *const _ as usize,
            parts.join(" ")
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| self.items.iter().eq(o.items.iter()))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        use std::collections::hash_map::DefaultHasher;
        use std::hash::{Hash, Hasher};
        // Unreachable via `hash_extension` (`equiv_items` below
        // takes the direct sequential write); kept consistent
        // with `dyn_eq` anyway.
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        self.items.len().hash(&mut h);
        for item in self.items.iter() {
            item.hash(&mut h);
        }
        state.write_u64(h.finish());
    }

    fn equiv_items(&self) -> Option<Vec<Value>> {
        Some(self.items())
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a persistent queue, if it is one.
pub(crate) fn as_queue(v: &Value) -> Option<&QueueValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<QueueValue>(),
        _ => None,
    }
}
