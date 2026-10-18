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
use std::collections::hash_map::Entry;
use std::sync::Arc;

use clj_value::{Keyword, Map, Symbol, Unbound, Value, Var, new_map};

use crate::ext::NsHandle;

/// A var's evaluator-side state: the handle in [`Value::Var`] is
/// just `(id, sym)`; everything mutable lives here.
#[derive(Clone, Debug)]
pub struct VarState {
    /// The root (`None` = freshly interned, never set).
    pub root: Option<Value>,
    /// The var's metadata (`None` = cleared via `reset-meta!`/
    /// `alter-meta!`; fresh interns carry `:ns`/`:name`, `def`
    /// stamps the auto keys plus user meta).
    pub meta: Option<Map>,
    /// `^:dynamic`: required for `binding`/`set!`.
    pub dynamic: bool,
}

impl VarState {
    fn fresh() -> Self {
        Self {
            root: None,
            meta: None,
            dynamic: false,
        }
    }
}

/// One namespace: vars by id, qualified symbols to ids, aliases,
/// and refers (simple name → qualified target). Var ids come from
/// the registry counter, never per-namespace, so ids stay unique
/// across namespaces (and double as the `#object` hex for `Unbound`
/// sentinels). The namespace `id` (also registry-issued, never
/// reused while live) is the `#object` hex for namespace values.
#[derive(Clone, Debug)]
pub struct Namespace {
    name: String,
    id: u64,
    vars: HashMap<u64, VarState>,
    ids: HashMap<Symbol, u64>,
    aliases: HashMap<String, (String, u64)>,
    refers: HashMap<String, (Symbol, u64)>,
    meta: Option<Map>,
}

impl Namespace {
    /// A fresh namespace with a registry-issued id (`nil` meta,
    /// like the JVM).
    #[must_use]
    pub fn new(name: &str, id: u64) -> Self {
        Self {
            name: name.to_owned(),
            id,
            vars: HashMap::new(),
            ids: HashMap::new(),
            aliases: HashMap::new(),
            refers: HashMap::new(),
            meta: None,
        }
    }

    /// This namespace's name.
    #[must_use]
    pub fn name(&self) -> &str {
        &self.name
    }

    /// This namespace's registry id.
    #[must_use]
    pub fn id(&self) -> u64 {
        self.id
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
    /// must already name it). Creation stamps the `:ns`/`:name`
    /// meta (JVM `Var.intern`) and drops any refer of the same name
    /// (`def`/`intern` silently replace refers, like
    /// `Namespace.reference` minus its `*err*` warning, which waits
    /// for S5D). Ids come from the registry counter.
    pub fn intern(&mut self, sym: &Symbol, next_id: &mut u64) -> u64 {
        if let Some(id) = self.ids.get(sym) {
            return *id;
        }
        let id = *next_id;
        *next_id += 1;
        let mut state = VarState::fresh();
        let ns_key = Value::Keyword(Keyword::new(None, "ns"));
        let ns_val = Value::Extension(std::sync::Arc::new(NsHandle {
            name: std::sync::Arc::from(self.name.as_str()),
            id: self.id,
        }));
        let name_key = Value::Keyword(Keyword::new(None, "name"));
        let name_val = Value::Symbol(Symbol::new(None, sym.name()));
        state.meta = Some(new_map().insert(ns_key, ns_val).insert(name_key, name_val));
        self.vars.insert(id, state);
        self.ids.insert(sym.clone(), id);
        self.refers.remove(sym.name());
        id
    }

    /// Alias → target-namespace-name (for reader options).
    #[must_use]
    pub fn alias_map(&self) -> HashMap<String, String> {
        self.aliases
            .iter()
            .map(|(alias, (target, _))| (alias.clone(), target.clone()))
            .collect()
    }

    /// Referred name → (home namespace, name) (for reader
    /// options; syntax-quote qualifies refers home).
    #[must_use]
    pub fn refer_map(&self) -> HashMap<String, (String, String)> {
        self.refers
            .iter()
            .filter_map(|(name, (target, _))| {
                target
                    .namespace()
                    .map(|ns| (name.clone(), (ns.to_owned(), target.name().to_owned())))
            })
            .collect()
    }

    /// This namespace's metadata (`None` = `nil`, like the JVM).
    #[must_use]
    pub fn meta(&self) -> Option<&Map> {
        self.meta.as_ref()
    }

    /// Replace this namespace's metadata (`alter-meta!`/
    /// `reset-meta!`).
    pub fn set_meta(&mut self, meta: Option<Map>) {
        self.meta = meta;
    }

    /// Install an alias: fresh and identical-target installs are
    /// `Ok`; a conflicting target is `Err` with the old target
    /// (the `alias` error names it). The target id is pinned at
    /// install so `ns-aliases` shows the aliased object even after
    /// the target is removed and recreated.
    pub fn add_alias(&mut self, alias: &str, target: &str, target_id: u64) -> Result<(), String> {
        match self.aliases.get(alias) {
            None => {
                self.aliases
                    .insert(alias.to_owned(), (target.to_owned(), target_id));
                Ok(())
            }
            Some((old, _)) if old == target => Ok(()),
            Some((old, _)) => Err(old.clone()),
        }
    }

    /// Drop an alias (missing is a silent no-op).
    pub fn remove_alias(&mut self, alias: &str) {
        self.aliases.remove(alias);
    }

    /// Install a refer (replaces silently; callers keep interns —
    /// an intern of the same name must never reach this). The var
    /// id is pinned at install so `ns-refers`/`ns-resolve` see the
    /// referred var even after the target is re-interned.
    pub fn add_refer(&mut self, name: &str, target: Symbol, target_id: u64) {
        self.refers.insert(name.to_owned(), (target, target_id));
    }

    /// Drop the intern and refer mappings for a simple name (the
    /// var state survives, like the JVM — only the mapping goes).
    pub fn unmap(&mut self, name: &str) {
        let q = Symbol::new(Some(&self.name), name);
        self.ids.remove(&q);
        self.refers.remove(name);
    }

    /// Interned mappings: qualified symbol to var id.
    pub fn interns(&self) -> impl Iterator<Item = (&Symbol, u64)> + '_ {
        self.ids.iter().map(|(s, i)| (s, *i))
    }

    /// Refer mappings: simple name to qualified target and the
    /// var id pinned at install.
    pub fn refers(&self) -> impl Iterator<Item = (&str, &Symbol, u64)> + '_ {
        self.refers.iter().map(|(n, (s, i))| (n.as_str(), s, *i))
    }

    /// Aliases: alias to target namespace name and the id pinned
    /// at install.
    pub fn aliases(&self) -> impl Iterator<Item = (&str, &str, u64)> + '_ {
        self.aliases
            .iter()
            .map(|(a, (t, i))| (a.as_str(), t.as_str(), *i))
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

/// Find or create a namespace in `namespaces`, issuing a fresh
/// id on creation (split out so `intern` can share it without
/// tripping the borrow checker).
fn ensure_ns_in<'a>(
    namespaces: &'a mut HashMap<String, Namespace>,
    next_ns_id: &mut u64,
    name: &str,
) -> &'a mut Namespace {
    match namespaces.entry(name.to_owned()) {
        Entry::Occupied(e) => e.into_mut(),
        Entry::Vacant(e) => {
            let id = *next_ns_id;
            *next_ns_id += 1;
            e.insert(Namespace::new(name, id))
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
    next_ns_id: u64,
    by_id: HashMap<u64, String>,
    removed: HashMap<String, Namespace>,
}

impl NamespaceRegistry {
    /// An empty registry (boot ensures namespaces explicitly).
    /// Ids start at 1, like the S4 namespace counter.
    #[must_use]
    pub fn new() -> Self {
        Self {
            next_id: 1,
            next_ns_id: 1,
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

    /// Find or create a namespace (creation issues a fresh id).
    pub fn ensure_ns(&mut self, name: &str) -> &mut Namespace {
        let Self {
            namespaces,
            next_ns_id,
            ..
        } = self;
        ensure_ns_in(namespaces, next_ns_id, name)
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

    /// A live namespace's id (`None` when missing or removed).
    #[must_use]
    pub fn ns_id(&self, name: &str) -> Option<u64> {
        self.namespaces.get(name).map(Namespace::id)
    }

    /// All live namespaces as `(name, id)`, sorted by name (the
    /// JVM order is hash-unspecified; the port is deterministic).
    #[must_use]
    pub fn all_ns_sorted(&self) -> Vec<(String, u64)> {
        let mut out: Vec<(String, u64)> = self
            .namespaces
            .iter()
            .map(|(n, ns)| (n.clone(), ns.id()))
            .collect();
        out.sort_by(|a, b| a.0.cmp(&b.0));
        out
    }

    /// A live or removed namespace by name (removed ones stay
    /// readable through stale handles, like the JVM's detached
    /// objects; only the latest removal is kept).
    #[must_use]
    pub fn ns_live_or_removed(&self, name: &str) -> Option<&Namespace> {
        self.namespaces.get(name).or_else(|| self.removed.get(name))
    }

    /// A live or removed namespace by name, mutably (`intern` and
    /// `ns-unmap` through a stale handle mutate the tombstone,
    /// like the JVM's detached objects).
    pub fn ns_live_or_removed_mut(&mut self, name: &str) -> Option<&mut Namespace> {
        if self.namespaces.contains_key(name) {
            self.namespaces.get_mut(name)
        } else {
            self.removed.get_mut(name)
        }
    }

    /// Drop a namespace (aliases pointing at it dangle, like the
    /// JVM; var ids are never reused). The namespace survives as
    /// a tombstone, so stale handles and captured vars keep
    /// reading their frozen state; only resolution treats the
    /// name as gone.
    pub fn remove_ns(&mut self, name: &str) -> bool {
        let Some(ns) = self.namespaces.remove(name) else {
            return false;
        };
        self.removed.insert(name.to_owned(), ns);
        true
    }

    /// Qualify an unqualified symbol into the current namespace;
    /// explicit namespaces pass through untouched.
    #[must_use]
    pub fn qualify(&self, sym: &Symbol) -> Symbol {
        self.qualify_in(&self.current, sym)
    }

    /// [`qualify`](Self::qualify) against an explicit namespace
    /// (the analyzer passes the `*ns*`-synced name).
    #[must_use]
    pub fn qualify_in(&self, ns_name: &str, sym: &Symbol) -> Symbol {
        if sym.namespace().is_some() {
            sym.clone()
        } else {
            Symbol::new(Some(ns_name), sym.name())
        }
    }

    /// The namespace a qualifier names: aliases win over real
    /// namespaces (probe34 `ns-alias-shadow`); `None` is genuinely
    /// unknown.
    #[must_use]
    pub fn namespace_target<'a>(&'a self, part: &'a str) -> Option<&'a str> {
        self.namespace_target_in(&self.current, part)
    }

    /// [`namespace_target`](Self::namespace_target) against an
    /// explicit namespace (`ns-resolve` takes aliases from its
    /// namespace argument, not the current one).
    #[must_use]
    pub fn namespace_target_in<'a>(&'a self, ns_name: &str, part: &'a str) -> Option<&'a str> {
        self.namespaces
            .get(ns_name)
            .and_then(|ns| ns.aliases.get(part))
            .map(|(target, _)| target.as_str())
            .or_else(|| self.namespaces.contains_key(part).then_some(part))
    }

    /// Resolve a symbol to its qualified target: qualified symbols
    /// via alias-or-namespace, unqualified via the current
    /// namespace then refers. `None` is unresolvable (callers pick
    /// the error shape).
    #[must_use]
    pub fn resolve(&self, sym: &Symbol) -> Option<Symbol> {
        self.resolve_in(&self.current, sym)
    }

    /// [`resolve`](Self::resolve) against an explicit namespace.
    #[must_use]
    pub fn resolve_in(&self, ns_name: &str, sym: &Symbol) -> Option<Symbol> {
        match sym.namespace() {
            Some(part) => {
                let target = self.namespace_target_in(ns_name, part)?;
                let ns = self.namespaces.get(target)?;
                let q = Symbol::new(Some(target), sym.name());
                ns.ids.contains_key(&q).then_some(q)
            }
            None => {
                let home = self.namespaces.get(ns_name)?;
                let q = Symbol::new(Some(ns_name), sym.name());
                if home.ids.contains_key(&q) {
                    return Some(q);
                }
                // Refers validate live: a refer into a removed
                // namespace stops resolving (the JVM keeps the var
                // object alive — fenced in namespaces-contract —
                // but must never resurrect the namespace here).
                home.refers
                    .get(sym.name())
                    .filter(|(target, _)| {
                        target
                            .namespace()
                            .and_then(|ns| self.namespaces.get(ns))
                            .is_some_and(|ns| ns.ids.contains_key(target))
                    })
                    .map(|(target, _)| target.clone())
            }
        }
    }

    /// Resolve a symbol to its var id (`None` is unresolvable).
    #[must_use]
    pub fn lookup(&self, sym: &Symbol) -> Option<u64> {
        self.lookup_in(&self.current, sym)
    }

    /// [`lookup`](Self::lookup) against an explicit namespace (the
    /// analyzer passes the `*ns*`-synced name).
    #[must_use]
    pub fn lookup_in(&self, ns_name: &str, sym: &Symbol) -> Option<u64> {
        let target = self.resolve_in(ns_name, sym)?;
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
            next_ns_id,
            by_id,
            ..
        } = self;
        let ns = ensure_ns_in(namespaces, next_ns_id, &ns_name);
        let id = ns.intern(&q, next_id);
        by_id.insert(id, ns_name);
        id
    }

    /// Intern a var into a live or removed namespace (never
    /// creates: callers resolve through `the-ns` first, and stale
    /// handles land in the tombstone). Returns `None` only when
    /// the namespace is neither live nor removed.
    pub fn intern_in(&mut self, ns_name: &str, sym: &Symbol) -> Option<u64> {
        let Self {
            namespaces,
            removed,
            next_id,
            by_id,
            ..
        } = self;
        let ns = namespaces
            .get_mut(ns_name)
            .or_else(|| removed.get_mut(ns_name))?;
        let id = ns.intern(sym, next_id);
        by_id.insert(id, ns_name.to_owned());
        Some(id)
    }

    /// The var's state, if interned (tombstones answer for vars
    /// whose namespace was removed, and for unmapped-then-kept
    /// states, like the JVM's surviving var objects).
    #[must_use]
    pub fn state(&self, id: u64) -> Option<&VarState> {
        let name = self.by_id.get(&id)?;
        self.namespaces
            .get(name)
            .and_then(|ns| ns.state(id))
            .or_else(|| self.removed.get(name).and_then(|ns| ns.state(id)))
    }

    /// The var's state, mutably.
    pub fn state_mut(&mut self, id: u64) -> Option<&mut VarState> {
        let name = self.by_id.get(&id)?.clone();
        if self
            .namespaces
            .get(&name)
            .and_then(|ns| ns.state(id))
            .is_some()
        {
            return self.namespaces.get_mut(&name)?.state_mut(id);
        }
        self.removed.get_mut(&name)?.state_mut(id)
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

/// The thread-binding stack: one frame per `binding` /
/// `push-thread-bindings` / `with-bindings*` / `bound-fn`
/// invocation. Entries shadow by recency across frames; `set!`
/// writes the topmost entry for its var.
#[derive(Clone, Debug, Default)]
pub struct Bindings {
    frames: Vec<Vec<(Var, Value)>>,
}

impl Bindings {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// The frame depth (for restoring after a `binding` body).
    #[must_use]
    pub fn depth(&self) -> usize {
        self.frames.len()
    }

    /// Push one frame (possibly empty, like the JVM).
    pub fn push_frame(&mut self, entries: Vec<(Var, Value)>) {
        self.frames.push(entries);
    }

    /// Pop one frame (`None` when the stack is empty —
    /// `pop-thread-bindings` raises then).
    pub fn pop_frame(&mut self) -> Option<Vec<(Var, Value)>> {
        self.frames.pop()
    }

    /// Pop back to an earlier depth.
    pub fn truncate(&mut self, depth: usize) {
        self.frames.truncate(depth);
    }

    /// The visible value for a var, if bound.
    #[must_use]
    pub fn get(&self, id: u64) -> Option<&Value> {
        self.frames.iter().rev().find_map(|frame| {
            frame
                .iter()
                .rev()
                .find_map(|(var, v)| (var.id == id).then_some(v))
        })
    }

    /// Every entry oldest-first (for snapshots: later entries win
    /// per var when folded into a map).
    pub fn pairs(&self) -> impl Iterator<Item = (&Var, &Value)> + '_ {
        self.frames
            .iter()
            .flat_map(|frame| frame.iter().map(|(var, v)| (var, v)))
    }

    /// `set!`: replace the topmost entry for a var (false when no
    /// entry exists — the evaluator raises then).
    pub fn set_top(&mut self, id: u64, value: Value) -> bool {
        match self
            .frames
            .iter_mut()
            .rev()
            .find_map(|frame| frame.iter_mut().rev().find(|(var, _)| var.id == id))
        {
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
        let v_id = reg.intern(&sym(Some("lib"), "v"));
        let lib_id = reg.ns_id("lib").unwrap();
        reg.get_mut("user")
            .unwrap()
            .add_alias("q", "lib", lib_id)
            .unwrap();
        reg.get_mut("user")
            .unwrap()
            .add_refer("w", sym(Some("lib"), "v"), v_id);
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
        // S5A2 tombstones supersede S5A1's drop: the state survives
        // removal (the JVM keeps the var object alive, so stale
        // handles and captured vars keep reading it) — only
        // resolution treats the name as gone.
        assert!(reg.state(b).is_some());
        assert_eq!(reg.resolve(&sym(Some("lib"), "b")), None);
    }

    #[test]
    fn qualify_uses_current() {
        let reg = rigged();
        assert_eq!(reg.qualify(&sym(None, "a")), sym(Some("user"), "a"));
        assert_eq!(reg.qualify(&sym(Some("lib"), "a")), sym(Some("lib"), "a"));
    }
}
