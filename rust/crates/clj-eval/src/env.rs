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

/// One namespace: vars by id, qualified symbols to ids, aliases,
/// and refers (simple name → qualified target). Var ids come from
/// the registry counter, never per-namespace, so ids stay unique
/// across namespaces (and double as the `#object` hex for `Unbound`
/// sentinels).
#[derive(Clone, Debug)]
pub struct Namespace {
    name: String,
    vars: HashMap<u64, VarState>,
    ids: HashMap<Symbol, u64>,
    aliases: HashMap<String, String>,
    refers: HashMap<String, Symbol>,
}

impl Namespace {
    /// A fresh namespace.
    #[must_use]
    pub fn new(name: &str) -> Self {
        Self {
            name: name.to_owned(),
            vars: HashMap::new(),
            ids: HashMap::new(),
            aliases: HashMap::new(),
            refers: HashMap::new(),
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

    /// Intern (find or create) a var in this namespace (the symbol
    /// must already name it). Creation leaves root/meta fresh;
    /// `eval` fills them. Ids come from the registry counter.
    pub fn intern(&mut self, sym: &Symbol, next_id: &mut u64) -> u64 {
        if let Some(id) = self.ids.get(sym) {
            return *id;
        }
        let id = *next_id;
        *next_id += 1;
        self.vars.insert(id, VarState::fresh());
        self.ids.insert(sym.clone(), id);
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

/// The namespace registry (S5A): all namespaces plus the current
/// one. Var ids come from one global counter and are never reused
/// across namespaces, so `Value::Var`/`Unbound` identity and
/// [`Bindings`] stay sound.
#[derive(Clone, Debug, Default)]
pub struct NamespaceRegistry {
    namespaces: HashMap<String, Namespace>,
    current: String,
    next_id: u64,
    by_id: HashMap<u64, String>,
}

impl NamespaceRegistry {
    /// An empty registry (boot ensures namespaces explicitly).
    /// Ids start at 1, like the S4 namespace counter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            ..Self::default()
        }
    }

    /// The current namespace name.
    #[must_use]
    pub fn current(&self) -> &str {
        &self.current
    }

    /// Switch the current namespace (it must exist).
    pub fn set_current(&mut self, name: &str) {
        self.current = name.to_owned();
    }

    /// Find or create a namespace.
    pub fn ensure_ns(&mut self, name: &str) -> &mut Namespace {
        self.namespaces
            .entry(name.to_owned())
            .or_insert_with(|| Namespace::new(name))
    }

    /// Look up a namespace by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<&Namespace> {
        self.namespaces.get(name)
    }

    /// Look up a namespace by name, mutably.
    pub fn get_mut(&mut self, name: &str) -> Option<&mut Namespace> {
        self.namespaces.get_mut(name)
    }

    /// Drop a namespace (aliases pointing at it dangle, like the
    /// JVM; var ids are never reused).
    pub fn remove_ns(&mut self, name: &str) -> bool {
        let Some(ns) = self.namespaces.remove(name) else {
            return false;
        };
        for id in ns.vars.keys() {
            self.by_id.remove(id);
        }
        true
    }

    /// Qualify an unqualified symbol into the current namespace;
    /// explicit namespaces pass through untouched.
    #[must_use]
    pub fn qualify(&self, sym: &Symbol) -> Symbol {
        if sym.namespace().is_some() {
            sym.clone()
        } else {
            Symbol::new(Some(&self.current), sym.name())
        }
    }

    /// The namespace a qualifier names: aliases win over real
    /// namespaces (probe34 `ns-alias-shadow`); `None` is genuinely
    /// unknown.
    #[must_use]
    pub fn namespace_target<'a>(&'a self, part: &'a str) -> Option<&'a str> {
        self.namespaces
            .get(&self.current)
            .and_then(|ns| ns.aliases.get(part))
            .map(String::as_str)
            .or_else(|| self.namespaces.contains_key(part).then_some(part))
    }

    /// Resolve a symbol to its qualified target: qualified symbols
    /// via alias-or-namespace, unqualified via the current
    /// namespace then refers. `None` is unresolvable (callers pick
    /// the error shape).
    #[must_use]
    pub fn resolve(&self, sym: &Symbol) -> Option<Symbol> {
        match sym.namespace() {
            Some(part) => {
                let target = self.namespace_target(part)?;
                let ns = self.namespaces.get(target)?;
                let q = Symbol::new(Some(target), sym.name());
                ns.ids.contains_key(&q).then_some(q)
            }
            None => {
                let current = self.namespaces.get(&self.current)?;
                let q = Symbol::new(Some(&self.current), sym.name());
                if current.ids.contains_key(&q) {
                    return Some(q);
                }
                // Refers validate live: a refer into a removed
                // namespace stops resolving (the JVM keeps the var
                // object alive — fenced in namespaces-contract —
                // but must never resurrect the namespace here).
                current
                    .refers
                    .get(sym.name())
                    .filter(|target| {
                        target
                            .namespace()
                            .and_then(|ns| self.namespaces.get(ns))
                            .is_some_and(|ns| ns.ids.contains_key(target))
                    })
                    .cloned()
            }
        }
    }

    /// Resolve a symbol to its var id (`None` is unresolvable).
    #[must_use]
    pub fn lookup(&self, sym: &Symbol) -> Option<u64> {
        let target = self.resolve(sym)?;
        let ns = self.namespaces.get(target.namespace()?)?;
        ns.ids.get(&target).copied()
    }

    /// Intern (find or create) a var: unqualified symbols land in
    /// the current namespace, qualified ones name theirs (created
    /// when missing — internal callers always hit existing ones).
    pub fn intern(&mut self, sym: &Symbol) -> u64 {
        let q = self.qualify(sym);
        let ns_name = q.namespace().unwrap_or(&self.current).to_owned();
        let Self {
            namespaces,
            next_id,
            by_id,
            ..
        } = self;
        let ns = namespaces
            .entry(ns_name.clone())
            .or_insert_with(|| Namespace::new(&ns_name));
        let id = ns.intern(&q, next_id);
        by_id.insert(id, ns_name);
        id
    }

    /// The var's state, if interned.
    #[must_use]
    pub fn state(&self, id: u64) -> Option<&VarState> {
        let ns = self.namespaces.get(self.by_id.get(&id)?)?;
        ns.state(id)
    }

    /// The var's state, mutably.
    pub fn state_mut(&mut self, id: u64) -> Option<&mut VarState> {
        let name = self.by_id.get(&id)?.clone();
        self.namespaces.get_mut(&name)?.state_mut(id)
    }

    /// `Var.isBound` (minus thread bindings): a root that is absent
    /// or ANY `Unbound` sentinel counts as unbound.
    #[must_use]
    pub fn has_root(&self, id: u64) -> bool {
        self.state(id)
            .is_some_and(|s| !matches!(s.root, None | Some(Value::Unbound(_))))
    }

    /// `Var.deref` (minus thread bindings): the root, or the var's
    /// own `Unbound` sentinel when rootless. Never throws.
    #[must_use]
    pub fn root_or_unbound(&self, id: u64, sym: &Symbol) -> Value {
        match self.state(id).and_then(|s| s.root.clone()) {
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

#[cfg(test)]
mod tests {
    use super::*;

    fn sym(ns: Option<&str>, name: &str) -> Symbol {
        Symbol::new(ns, name)
    }

    /// Registry with `user` current, `lib` holding `v`, and `user`
    /// aliasing `q` → `lib` plus a `w` refer → `lib/v`.
    fn rigged() -> NamespaceRegistry {
        let mut reg = NamespaceRegistry::new();
        reg.ensure_ns("user");
        reg.set_current("user");
        reg.ensure_ns("lib");
        reg.intern(&sym(Some("lib"), "v"));
        reg.get_mut("user")
            .unwrap()
            .aliases
            .insert("q".to_owned(), "lib".to_owned());
        reg.get_mut("user")
            .unwrap()
            .refers
            .insert("w".to_owned(), sym(Some("lib"), "v"));
        reg
    }

    #[test]
    fn resolve_current_alias_refer() {
        let reg = rigged();
        assert_eq!(
            reg.resolve(&sym(Some("lib"), "v")),
            Some(sym(Some("lib"), "v"))
        );
        assert_eq!(
            reg.resolve(&sym(Some("q"), "v")),
            Some(sym(Some("lib"), "v"))
        );
        assert_eq!(reg.resolve(&sym(None, "w")), Some(sym(Some("lib"), "v")));
        assert_eq!(reg.resolve(&sym(None, "missing")), None);
        assert_eq!(reg.resolve(&sym(Some("lib"), "missing")), None);
        assert_eq!(reg.resolve(&sym(Some("nope"), "v")), None);
    }

    #[test]
    fn aliases_win_over_namespaces() {
        let mut reg = rigged();
        reg.ensure_ns("q");
        reg.intern(&sym(Some("q"), "v"));
        // `q/v` resolves through the alias (probe34
        // `ns-alias-shadow`), not the real `q` namespace.
        assert_eq!(
            reg.resolve(&sym(Some("q"), "v")),
            Some(sym(Some("lib"), "v"))
        );
        assert_eq!(reg.namespace_target("q"), Some("lib"));
    }

    #[test]
    fn dangling_alias_stays_namespace_shaped() {
        let mut reg = rigged();
        assert!(reg.remove_ns("lib"));
        // The alias still names a namespace (probe34
        // `ns-alias-removed`); the var itself is gone.
        assert_eq!(reg.namespace_target("q"), Some("lib"));
        assert_eq!(reg.resolve(&sym(Some("q"), "v")), None);
        assert_eq!(reg.namespace_target("lib"), None);
        // Stale refers stop resolving instead of resurrecting
        // the namespace (fenced: the JVM keeps the var object).
        assert_eq!(reg.resolve(&sym(None, "w")), None);
        assert!(!reg.namespaces.contains_key("lib"));
    }

    #[test]
    fn var_ids_are_globally_unique() {
        let mut reg = rigged();
        let a = reg.intern(&sym(None, "a"));
        let b = reg.intern(&sym(Some("lib"), "b"));
        assert_ne!(a, b);
        assert!(reg.state(a).is_some());
        assert!(reg.state(b).is_some());
        assert!(reg.remove_ns("lib"));
        assert!(reg.state(a).is_some());
        assert!(reg.state(b).is_none());
    }

    #[test]
    fn qualify_uses_current() {
        let reg = rigged();
        assert_eq!(reg.qualify(&sym(None, "a")), sym(Some("user"), "a"));
        assert_eq!(reg.qualify(&sym(Some("lib"), "a")), sym(Some("lib"), "a"));
    }
}
