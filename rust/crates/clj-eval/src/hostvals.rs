// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Host object values: `Locale` constants, `StringBuilder` /
//! `StringBuffer` buffers, and `PersistentQueue` queues. Locales
//! are singletons read through value-position static fields
//! (`java.util.Locale/US`) and consumed by `String/format`;
//! builders are mutable buffers (interior mutability behind the
//! shared extension handle, like the matcher state's) with the
//! `append`/`length`/`reverse` surface the corpus uses; queues
//! are persistent values (`conj` appends, `peek`/`pop` take the
//! front) behind `clojure.lang.PersistentQueue/EMPTY`.

use std::hash::Hasher;
use std::sync::{Arc, Mutex};

use clj_value::{ExtensionValue, Value, Vector};

/// A `java.util.Locale` constant (only `US` is reachable — the
/// sole locale the snippet corpus formats with). Equality is by
/// tag (the JVM hands out singletons).
#[derive(Debug)]
pub(crate) struct LocaleValue {
    tag: &'static str,
}

impl LocaleValue {
    /// The `Locale/US` value (`en_US`).
    pub(crate) fn us() -> Value {
        Value::Extension(Arc::new(Self { tag: "US" }))
    }
}

impl ExtensionValue for LocaleValue {
    fn class_name(&self) -> String {
        "java.util.Locale".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.util.Locale 0x{:x} \"en_US\"]",
            self.tag.as_ptr() as usize
        )
    }

    fn print_str(&self) -> Option<String> {
        Some("en_US".to_owned())
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| o.tag == self.tag)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write(self.tag.as_bytes());
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a locale, if it is one.
pub(crate) fn as_locale(v: &Value) -> Option<&LocaleValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<LocaleValue>(),
        _ => None,
    }
}

/// A `StringBuilder`/`StringBuffer` buffer: shared mutable text
/// (interior mutability behind the extension handle, like the
/// matcher state). Identity equality and hash (the JVM overrides
/// neither).
#[derive(Debug)]
pub(crate) struct BuilderValue {
    class: &'static str,
    text: Mutex<String>,
}

impl BuilderValue {
    /// A fresh buffer of `class` holding `text`.
    pub(crate) fn with_text(class: &'static str, text: String) -> Value {
        Value::Extension(Arc::new(Self {
            class,
            text: Mutex::new(text),
        }))
    }

    /// A locked guard (a poisoned mutex still yields its data,
    /// like the stream state).
    fn lock(&self) -> std::sync::MutexGuard<'_, String> {
        self.text.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The current contents.
    pub(crate) fn contents(&self) -> String {
        self.lock().clone()
    }

    /// Append `s` (mutates, like the JVM).
    pub(crate) fn push_str(&self, s: &str) {
        self.lock().push_str(s);
    }

    /// Reverse in place (astral chars stay whole — lone
    /// surrogates are unrepresentable, so the JVM's pair
    /// preservation falls out of the char model).
    pub(crate) fn reverse_in_place(&self) {
        let rev: String = self.lock().chars().rev().collect();
        *self.lock() = rev;
    }

    /// The UTF-16 length (the `length` twin).
    pub(crate) fn utf16_len(&self) -> i64 {
        self.lock().encode_utf16().count() as i64
    }
}

impl ExtensionValue for BuilderValue {
    fn class_name(&self) -> String {
        self.class.to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[{} 0x{:x} \"{}\"]",
            self.class,
            &self.text as *const _ as usize,
            self.lock()
        )
    }

    fn print_str(&self) -> Option<String> {
        Some(self.contents())
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Self>()
            .is_some_and(|o| std::ptr::eq(&self.text, &o.text))
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_usize(&self.text as *const _ as usize);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a string builder/buffer, if it is one.
pub(crate) fn as_builder(v: &Value) -> Option<&BuilderValue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<BuilderValue>(),
        _ => None,
    }
}

/// A `clojure.lang.PersistentQueue`: items front-to-back in a
/// persistent vector. Unlike builders this is a value — every op
/// returns a fresh handle, and equality is element-wise (against
/// queues only: `=` against vectors/lists answers false since the
/// lazy stepper has no queue arm — a documented narrowing; the
/// corpus never compares queues). `pop` of empty stays empty
/// (oracle `/tmp/queue3.clj`).
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
        // The sequential-family digest (mirrors the seq
        // extensions): deterministic and consistent with
        // `dyn_eq`, though never equal to a vector's hash (the
        // `=` narrowing above).
        let mut h = DefaultHasher::new();
        8u8.hash(&mut h);
        self.items.len().hash(&mut h);
        for item in self.items.iter() {
            item.hash(&mut h);
        }
        state.write_u64(h.finish());
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
