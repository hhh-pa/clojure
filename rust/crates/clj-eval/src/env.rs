// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! The S4 namespace (one: `user`), var states, and the thread-binding
//! stack. Mirrors `Var.java`: a fresh var's root is its per-var
//! `Unbound` sentinel, `deref` never throws, and `hasRoot` is false
//! for ANY `Unbound` root (so `(def x x)` stays unbound, probe25).

use std::collections::HashMap;
use std::sync::Arc;

use clj_value::{Map, Symbol, Unbound, Value, new_map};

/// A var's evaluator-side state: the handle in [`Value::Var`] is
/// just `(id, sym)`; everything mutable lives here.
#[derive(Clone, Debug)]
pub struct VarState {
    /// The root (`None` = freshly interned, never set).
    pub root: Option<Value>,
    /// The var's metadata (auto `:line`/`:column`/`:file`/`:name`
    /// plus user meta; never `:ns` — S4 has no namespace objects).
    pub meta: Map,
    /// `^:dynamic`: required for `binding`/`set!`.
    pub dynamic: bool,
}

impl VarState {
    fn fresh() -> Self {
        Self {
            root: None,
            meta: new_map(),
            dynamic: false,
        }
    }
}

/// The single S4 namespace: vars by id, symbols to ids, and the
/// deterministic var-id counter (doubles as the `#object` hex for
/// `Unbound` sentinels).
#[derive(Clone, Debug)]
pub struct Namespace {
    name: String,
    vars: HashMap<u64, VarState>,
    ids: HashMap<Symbol, u64>,
    next_id: u64,
}

impl Namespace {
    /// A fresh namespace (S4 always names it `user`).
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            vars: HashMap::new(),
            ids: HashMap::new(),
            next_id: 1,
        }
    }

    /// This namespace's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// Qualify an unqualified symbol into this namespace.
    #[must_use]
    pub fn qualify(&self, sym: &Symbol) -> Symbol {
        if sym.namespace().is_some() {
            sym.clone()
        } else {
            Symbol::new(Some(&self.name), sym.name())
        }
    }

    /// Look up a var id by symbol (unqualified resolves here).
    #[must_use]
    pub fn lookup(&self, sym: &Symbol) -> Option<u64> {
        let q = self.qualify(sym);
        if q.namespace() == Some(self.name.as_str()) {
            self.ids.get(&q).copied()
        } else {
            None
        }
    }

    /// Intern (find or create) a var. Creation leaves root/meta
    /// fresh; `eval` fills them.
    pub fn intern(&mut self, sym: &Symbol) -> u64 {
        let q = self.qualify(sym);
        if let Some(id) = self.ids.get(&q) {
            return *id;
        }
        let id = self.next_id;
        self.next_id += 1;
        self.vars.insert(id, VarState::fresh());
        self.ids.insert(q, id);
        id
    }

    /// The var's state, if interned.
    #[must_use]
    pub fn state(&self, id: u64) -> Option<&VarState> {
        self.vars.get(&id)
    }

    /// The var's state, mutably.
    pub fn state_mut(&mut self, id: u64) -> Option<&mut VarState> {
        self.vars.get_mut(&id)
    }

    /// The qualified symbol behind an id (for printing sentinels).
    #[must_use]
    pub fn symbol_of(&self, id: u64) -> Option<Symbol> {
        self.ids
            .iter()
            .find_map(|(s, i)| (*i == id).then(|| s.clone()))
    }

    /// `Var.isBound` (minus thread bindings, which live in
    /// [`Bindings`]): a root that is absent or ANY `Unbound`
    /// sentinel counts as unbound (`Var.hasRoot`).
    #[must_use]
    pub fn has_root(&self, id: u64) -> bool {
        match self.vars.get(&id).and_then(|s| s.root.as_ref()) {
            None | Some(Value::Unbound(_)) => false,
            Some(_) => true,
        }
    }

    /// `Var.deref` (minus thread bindings): the root, or the var's
    /// own `Unbound` sentinel when rootless. Never throws.
    #[must_use]
    pub fn root_or_unbound(&self, id: u64, sym: &Symbol) -> Value {
        match self.vars.get(&id).and_then(|s| s.root.clone()) {
            Some(v) => v,
            None => Value::Unbound(Unbound {
                sym: sym.clone(),
                var_id: id,
            }),
        }
    }
}

/// The thread-binding stack (`binding` push/pop). Entries shadow by
/// recency; `set!` writes the topmost frame for its var.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    stack: Vec<(u64, Value)>,
}

impl Bindings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The depth (for restoring after a `binding` body).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.stack.len()
    }

    /// Push one frame.
    pub fn push(&mut self, id: u64, value: Value) {
        self.stack.push((id, value));
    }

    /// Pop back to an earlier depth.
    pub fn truncate(&mut self, depth: usize) {
        self.stack.truncate(depth);
    }

    /// The visible value for a var, if bound.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Value> {
        self.stack
            .iter()
            .rev()
            .find_map(|(i, v)| (*i == id).then_some(v))
    }

    /// `set!`: replace the topmost frame for a var (false when no
    /// frame exists — the evaluator raises then).
    pub fn set_top(&mut self, id: u64, value: Value) -> bool {
        match self.stack.iter_mut().rev().find(|(i, _)| *i == id) {
            Some((_, slot)) => {
                *slot = value;
                true
            }
            None => false,
        }
    }
}

/// Local slots: shared (`Arc`) so closures capture their frame,
/// mutable (`Mutex`) only for `letfn`/named-`fn` patching (before
/// any observer exists). Extension and `recur` never mutate in
/// place (they clone), so iteration N's closures never observe
/// iteration N+1's rebinds. Locks are never held across evaluation.
#[derive(Clone, Debug, Default)]
pub struct Env {
    slots: Arc<std::sync::Mutex<Vec<Value>>>,
}

impl Env {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The slot count.
    #[must_use]
    pub fn len(&self) -> usize {
        self.slots.lock().map_or(0, |slots| slots.len())
    }

    #[must_use]
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Read a slot (cloned; missing slots read `nil`, unreachable
    /// by construction).
    #[must_use]
    pub fn get(&self, idx: usize) -> Value {
        self.slots
            .lock()
            .ok()
            .and_then(|slots| slots.get(idx).cloned())
            .unwrap_or(Value::Nil)
    }

    /// Extend with fresh slots (the receiver is untouched).
    #[must_use]
    pub fn extend(&self, values: Vec<Value>) -> Self {
        let mut slots = self
            .slots
            .lock()
            .map(|slots| slots.clone())
            .unwrap_or_default();
        slots.extend(values);
        Self {
            slots: Arc::new(std::sync::Mutex::new(slots)),
        }
    }

    /// A private copy of this frame (copy-on-`recur`: the old
    /// frame stays intact for closures that captured it).
    #[must_use]
    pub fn duplicate(&self) -> Self {
        let slots = self
            .slots
            .lock()
            .map(|slots| slots.clone())
            .unwrap_or_default();
        Self {
            slots: Arc::new(std::sync::Mutex::new(slots)),
        }
    }

    /// Overwrite one slot (`letfn`/named-`fn` patching only; `recur`
    /// duplicates first).
    pub fn set(&self, idx: usize, value: Value) {
        let Ok(mut slots) = self.slots.lock() else {
            return;
        };
        if let Some(slot) = slots.get_mut(idx) {
            *slot = value;
        }
    }
}
