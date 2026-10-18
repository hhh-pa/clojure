// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Transients (S3): bulk-build buffers mirroring
//! `TransientVector`/`TransientArrayMap`/`TransientHashSet`
//! (`docs/t1174/probes/probe19.clj`).
//!
//! Every op validates the edit flag, exactly like the JVM: any use
//! after [`TransientVector::persistent`] (mutating ops, reads, or a
//! second `persistent`) raises
//! [`TransientError::UseAfterPersistent`]. Methods take `&mut self`,
//! so the borrow checker enforces the JVM's single-threaded-use
//! rule statically. `persistent` drops metadata (a fresh transient
//! carries none).

use std::collections::{HashMap, HashSet};
use std::fmt;

use crate::{FxBuildHasher, Map, Set, Value, Vector, new_map, new_set};

/// A transient failure: exact JVM message + class (probe19). The
/// evaluator (S4) raises these with the same text.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransientError {
    /// `Transient used after persistent! call` (any op after
    /// `persistent`, including reads and a second call).
    UseAfterPersistent,
    /// `Can't pop empty vector`.
    EmptyPop,
    /// `assoc!` past the end (empty message, like the JDK).
    IndexOutOfBounds,
}

impl TransientError {
    /// The JVM message text.
    #[must_use]
    pub fn message(&self) -> &'static str {
        match self {
            TransientError::UseAfterPersistent => "Transient used after persistent! call",
            TransientError::EmptyPop => "Can't pop empty vector",
            TransientError::IndexOutOfBounds => "",
        }
    }

    /// The JVM exception class.
    #[must_use]
    pub fn jvm_class(&self) -> &'static str {
        match self {
            TransientError::UseAfterPersistent => "java.lang.IllegalAccessError",
            TransientError::EmptyPop => "java.lang.IllegalStateException",
            TransientError::IndexOutOfBounds => "java.lang.IndexOutOfBoundsException",
        }
    }
}

impl fmt::Display for TransientError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.message())
    }
}

impl std::error::Error for TransientError {}

fn ensure(valid: bool) -> Result<(), TransientError> {
    if valid {
        Ok(())
    } else {
        Err(TransientError::UseAfterPersistent)
    }
}

/// A transient vector: `conj!`/`pop!`/`assoc!` buffer.
#[derive(Clone, Debug)]
pub struct TransientVector {
    items: Vec<Value>,
    valid: bool,
}

impl TransientVector {
    /// An empty transient vector.
    #[must_use]
    pub fn new() -> Self {
        Self {
            items: Vec::new(),
            valid: true,
        }
    }

    /// A transient over `v`'s items.
    #[must_use]
    pub fn from_vector(v: &Vector) -> Self {
        Self {
            items: v.iter().cloned().collect(),
            valid: true,
        }
    }

    fn check(&self) -> Result<(), TransientError> {
        ensure(self.valid)
    }

    /// Append one value (`conj!` takes exactly one; arity is the
    /// core fn's job in S5).
    pub fn conj(&mut self, v: Value) -> Result<(), TransientError> {
        self.check()?;
        self.items.push(v);
        Ok(())
    }

    /// Drop the last value (empty errors).
    pub fn pop(&mut self) -> Result<(), TransientError> {
        self.check()?;
        self.items.pop().map(|_| ()).ok_or(TransientError::EmptyPop)
    }

    /// Set index `idx` (`idx == len` appends, past that errors).
    pub fn assoc(&mut self, idx: usize, v: Value) -> Result<(), TransientError> {
        self.check()?;
        if idx > self.items.len() {
            return Err(TransientError::IndexOutOfBounds);
        }
        if idx == self.items.len() {
            self.items.push(v);
        } else {
            self.items[idx] = v;
        }
        Ok(())
    }

    /// Read index `idx` (`None` past the end).
    pub fn nth(&self, idx: usize) -> Result<Option<&Value>, TransientError> {
        self.check()?;
        Ok(self.items.get(idx))
    }

    /// The item count.
    pub fn len(&self) -> Result<usize, TransientError> {
        self.check()?;
        Ok(self.items.len())
    }

    /// Whether the vector is empty.
    pub fn is_empty(&self) -> Result<bool, TransientError> {
        Ok(self.len()? == 0)
    }

    /// Freeze into a persistent vector (invalidates this transient).
    pub fn persistent(&mut self) -> Result<Vector, TransientError> {
        self.check()?;
        self.valid = false;
        Ok(Vector::from_vec(std::mem::take(&mut self.items)))
    }
}

impl Default for TransientVector {
    fn default() -> Self {
        Self::new()
    }
}

/// A transient map: `assoc!`/`dissoc!` buffer with insertion-ordered
/// freeze (re-assoc keeps the original key and position).
#[derive(Clone, Debug)]
pub struct TransientMap {
    entries: Vec<(Value, Value)>,
    index: HashMap<Value, usize, FxBuildHasher>,
    valid: bool,
}

impl TransientMap {
    /// An empty transient map.
    #[must_use]
    pub fn new() -> Self {
        Self {
            entries: Vec::new(),
            index: HashMap::default(),
            valid: true,
        }
    }

    /// A transient over `m`'s entries (insertion order kept).
    #[must_use]
    pub fn from_map(m: &Map) -> Self {
        let mut t = Self::new();
        for (k, v) in m.iter() {
            t.index.insert(k.clone(), t.entries.len());
            t.entries.push((k.clone(), v.clone()));
        }
        t
    }

    fn check(&self) -> Result<(), TransientError> {
        ensure(self.valid)
    }

    /// Assoc a key/value pair.
    pub fn assoc(&mut self, k: Value, v: Value) -> Result<(), TransientError> {
        self.check()?;
        match self.index.get(&k) {
            Some(&i) => self.entries[i].1 = v,
            None => {
                self.index.insert(k.clone(), self.entries.len());
                self.entries.push((k, v));
            }
        }
        Ok(())
    }

    /// Drop a key (missing is a no-op).
    pub fn dissoc(&mut self, k: &Value) -> Result<(), TransientError> {
        self.check()?;
        if let Some(i) = self.index.remove(k) {
            self.entries.remove(i);
            for (j, (ek, _)) in self.entries.iter().enumerate().skip(i) {
                self.index.insert(ek.clone(), j);
            }
        }
        Ok(())
    }

    /// Look up a key.
    pub fn get(&self, k: &Value) -> Result<Option<&Value>, TransientError> {
        self.check()?;
        Ok(self.index.get(k).map(|&i| &self.entries[i].1))
    }

    /// The entry count.
    pub fn len(&self) -> Result<usize, TransientError> {
        self.check()?;
        Ok(self.entries.len())
    }

    /// Whether the map is empty.
    pub fn is_empty(&self) -> Result<bool, TransientError> {
        Ok(self.len()? == 0)
    }

    /// Freeze into a persistent map (invalidates this transient).
    pub fn persistent(&mut self) -> Result<Map, TransientError> {
        self.check()?;
        self.valid = false;
        let mut map = new_map();
        for (k, v) in std::mem::take(&mut self.entries) {
            map = map.insert(k, v);
        }
        self.index.clear();
        Ok(map)
    }
}

impl Default for TransientMap {
    fn default() -> Self {
        Self::new()
    }
}

/// A transient set: `conj!`/`disj!` buffer with insertion-ordered
/// freeze (re-conj keeps the original member).
#[derive(Clone, Debug)]
pub struct TransientSet {
    members: Vec<Value>,
    index: HashSet<Value, FxBuildHasher>,
    valid: bool,
}

impl TransientSet {
    /// An empty transient set.
    #[must_use]
    pub fn new() -> Self {
        Self {
            members: Vec::new(),
            index: HashSet::default(),
            valid: true,
        }
    }

    /// A transient over `s`'s members (insertion order kept).
    #[must_use]
    pub fn from_set(s: &Set) -> Self {
        let mut t = Self::new();
        for m in s.iter() {
            t.index.insert(m.clone());
            t.members.push(m.clone());
        }
        t
    }

    fn check(&self) -> Result<(), TransientError> {
        ensure(self.valid)
    }

    /// Add a member (present is a no-op, keeping the original).
    pub fn conj(&mut self, v: Value) -> Result<(), TransientError> {
        self.check()?;
        if self.index.insert(v.clone()) {
            self.members.push(v);
        }
        Ok(())
    }

    /// Drop a member (missing is a no-op).
    pub fn disj(&mut self, v: &Value) -> Result<(), TransientError> {
        self.check()?;
        if self.index.remove(v) {
            self.members.retain(|m| m != v);
        }
        Ok(())
    }

    /// Whether a member is present.
    pub fn contains(&self, v: &Value) -> Result<bool, TransientError> {
        self.check()?;
        Ok(self.index.contains(v))
    }

    /// The member count.
    pub fn len(&self) -> Result<usize, TransientError> {
        self.check()?;
        Ok(self.members.len())
    }

    /// Whether the set is empty.
    pub fn is_empty(&self) -> Result<bool, TransientError> {
        Ok(self.len()? == 0)
    }

    /// Freeze into a persistent set (invalidates this transient).
    pub fn persistent(&mut self) -> Result<Set, TransientError> {
        self.check()?;
        self.valid = false;
        let mut set = new_set();
        for v in std::mem::take(&mut self.members) {
            set = set.insert(v);
        }
        self.index.clear();
        Ok(set)
    }
}

impl Default for TransientSet {
    fn default() -> Self {
        Self::new()
    }
}
