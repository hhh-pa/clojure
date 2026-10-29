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
use std::sync::{Arc, Condvar, Mutex, MutexGuard};

use clj_value::{ClassVal, Keyword, Map, Symbol, Unbound, Value, Var, new_map};

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
/// refers (simple name → qualified target), and imports (S16:
/// simple name → class). Var ids come from the registry counter,
/// never per-namespace, so ids stay unique across namespaces (and
/// double as the `#object` hex for `Unbound` sentinels). The
/// namespace `id` (also registry-issued, never reused while live)
/// is the `#object` hex for namespace values.
#[derive(Clone, Debug)]
pub struct Namespace {
    name: String,
    id: u64,
    vars: HashMap<u64, VarState>,
    ids: HashMap<Symbol, u64>,
    aliases: HashMap<String, (String, u64)>,
    refers: HashMap<String, (Symbol, u64)>,
    imports: HashMap<String, Arc<ClassVal>>,
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
            imports: HashMap::new(),
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
    /// meta (JVM `Var.intern`) and drops any refer or import of the
    /// same name (`def`/`intern` silently replace refers and imports,
    /// like `Namespace.reference` minus its `*err*` warning, which
    /// waits for S5D). Ids come from the registry counter.
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
        self.imports.remove(sym.name());
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
    /// referred var even after the target is re-interned. Any import
    /// of the same name goes (imports live under refers, like the
    /// JVM's single mapping).
    pub fn add_refer(&mut self, name: &str, target: Symbol, target_id: u64) {
        self.refers.insert(name.to_owned(), (target, target_id));
        self.imports.remove(name);
    }

    /// Drop the intern, refer, and import mappings for a simple name
    /// (the var state survives, like the JVM — only the mappings
    /// go; imports are mappings too).
    pub fn unmap(&mut self, name: &str) {
        let q = Symbol::new(Some(&self.name), name);
        self.ids.remove(&q);
        self.refers.remove(name);
        self.imports.remove(name);
    }

    /// Install an import: simple name → class (S16 `import`).
    pub fn import_class(&mut self, simple: &str, class: Arc<ClassVal>) {
        self.imports.insert(simple.to_owned(), class);
    }

    /// The import for a simple name, if any.
    #[must_use]
    pub fn get_import(&self, simple: &str) -> Option<Arc<ClassVal>> {
        self.imports.get(simple).cloned()
    }

    /// Import mappings: simple name to class.
    pub fn imports(&self) -> impl Iterator<Item = (&str, &Arc<ClassVal>)> + '_ {
        self.imports.iter().map(|(n, c)| (n.as_str(), c))
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
    /// Anonymous var states (S4b `with-local-vars`): ids with no
    /// namespace home, so `all-ns`/`ns-interns` never see them.
    /// [`state`](Self::state)/[`state_mut`](Self::state_mut)
    /// answer from here when `by_id` misses.
    anon: HashMap<u64, VarState>,
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

    /// A live or removed namespace by name, mutably (name-based
    /// callers only; handle-pinned writers use
    /// [`NamespaceRegistry::ns_for_handle_mut`]).
    pub fn ns_live_or_removed_mut(&mut self, name: &str) -> Option<&mut Namespace> {
        if self.namespaces.contains_key(name) {
            self.namespaces.get_mut(name)
        } else {
            self.removed.get_mut(name)
        }
    }

    /// A namespace through a handle-pinned id: the live namespace
    /// when its generation matches, else the tombstone when it
    /// matches (stale handles read frozen mappings after
    /// remove+recreate, like the JVM's detached objects). `None`
    /// when neither generation matches (only the latest removal
    /// is kept, so older handles dangle).
    #[must_use]
    pub fn ns_for_handle(&self, name: &str, id: u64) -> Option<&Namespace> {
        // (Match guards, not let-chains: MSRV 1.85.)
        match self.namespaces.get(name) {
            Some(ns) if ns.id() == id => return Some(ns),
            _ => {}
        }
        self.removed.get(name).filter(|ns| ns.id() == id)
    }

    /// [`NamespaceRegistry::ns_for_handle`], mutably.
    pub fn ns_for_handle_mut(&mut self, name: &str, id: u64) -> Option<&mut Namespace> {
        match self.namespaces.get_mut(name) {
            Some(ns) if ns.id() == id => return Some(ns),
            _ => {}
        }
        self.removed.get_mut(name).filter(|ns| ns.id() == id)
    }

    /// Drop a namespace (aliases pointing at it dangle, like the
    /// JVM; var ids are never reused). The namespace survives as
    /// a tombstone, so stale handles, captured vars, and the
    /// removed current namespace's own unqualified resolution keep
    /// reading their frozen state; only qualified resolution
    /// treats the name as gone.
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
        self.ns_live_or_removed(ns_name)
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
                // Live-or-removed: aliases dangle into tombstones
                // (upstream `namespaceFor` returns the alias object
                // liveness-blind; #81). Global names resolve live
                // only — `namespace_target_in` never yields a
                // removed one — so only alias targets read frozen
                // state here.
                let ns = self.ns_live_or_removed(target)?;
                let q = Symbol::new(Some(target), sym.name());
                ns.ids.contains_key(&q).then_some(q)
            }
            None => {
                // A removed current namespace still resolves its own
                // mappings (like the JVM's detached object — the
                // `*ns*` handle outlives the name).
                let home = self.ns_live_or_removed(ns_name)?;
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
        let ns = self.ns_live_or_removed(target.namespace()?)?;
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

    /// Intern a `def`-family target: qualify into the current
    /// namespace, then intern live-or-removed (a removed current
    /// namespace's tombstone absorbs the `def` — the JVM's
    /// detached `*ns*` object — instead of resurrecting the
    /// name); create only when the name is in neither map
    /// (T73 finding 4 round 2: `def`/`declare`/`defmacro` after
    /// `remove-ns` must not bring the namespace back).
    pub fn intern_def_target(&mut self, sym: &Symbol) -> u64 {
        let q = self.qualify(sym);
        let ns_name = q.namespace().unwrap_or(&self.current).to_owned();
        self.intern_in(&ns_name, &q)
            .unwrap_or_else(|| self.intern(&q))
    }

    /// Intern a var into a live or removed namespace (never
    /// creates: callers resolve through `the-ns` first; live wins
    /// over the tombstone by name). Returns `None` only when
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

    /// Intern a var through a handle-pinned id (stale handles land
    /// in the tombstone after remove+recreate). Returns the var id
    /// and the home namespace id, or `None` when neither
    /// generation matches the pin.
    pub fn intern_in_pinned(
        &mut self,
        ns_name: &str,
        expected: u64,
        sym: &Symbol,
    ) -> Option<(u64, u64)> {
        let Self {
            namespaces,
            removed,
            next_id,
            by_id,
            ..
        } = self;
        let ns = match namespaces.get_mut(ns_name) {
            Some(ns) if ns.id() == expected => ns,
            _ => removed.get_mut(ns_name).filter(|ns| ns.id() == expected)?,
        };
        let home = ns.id();
        let id = ns.intern(sym, next_id);
        by_id.insert(id, ns_name.to_owned());
        Some((id, home))
    }

    /// The var's state, if interned (tombstones answer for vars
    /// whose namespace was removed, and for unmapped-then-kept
    /// states, like the JVM's surviving var objects).
    #[must_use]
    pub fn state(&self, id: u64) -> Option<&VarState> {
        self.by_id
            .get(&id)
            .and_then(|name| {
                self.namespaces
                    .get(name)
                    .and_then(|ns| ns.state(id))
                    .or_else(|| self.removed.get(name).and_then(|ns| ns.state(id)))
            })
            .or_else(|| self.anon.get(&id))
    }

    /// Allocate an anonymous dynamic var (S4b
    /// `with-local-vars`, the JVM `Var.create().setDynamic()`):
    /// a registry-issued id with no namespace home, no root,
    /// `dynamic` set, and the JVM create-time meta (`{:ns nil,
    /// :name nil}`, oracle `wv-meta`).
    pub fn fresh_anonymous(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        let meta = new_map()
            .insert(Value::Keyword(Keyword::new(None, "ns")), Value::Nil)
            .insert(Value::Keyword(Keyword::new(None, "name")), Value::Nil);
        self.anon.insert(
            id,
            VarState {
                root: None,
                meta: Some(meta),
                dynamic: true,
            },
        );
        id
    }

    /// The var's state, mutably.
    pub fn state_mut(&mut self, id: u64) -> Option<&mut VarState> {
        if self.anon.contains_key(&id) {
            return self.anon.get_mut(&id);
        }
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

/// Per-lineage shared evaluator state (S6B, D1 spike choice): the
/// namespace registry plus the identity counters behind one mutex.
/// Cloning shares (`future` bodies fork an evaluator over the same
/// handle, so `def`s inside a body are globally visible and bodies
/// see every committed `def`, like the JVM's global namespaces).
/// Bindings stay per-evaluator (conveyed explicitly at fork, like
/// `bound-fn`); only the registry and counters are lineage-global.
///
/// This is spike option (a) — per-thread evaluator clones over
/// shared `Arc<Mutex<..>>` state. Message-passing (b) would
/// serialize every global lookup through the spawning thread, and
/// `thread::scope` borrowing (c) cannot serve detached `future`
/// threads (the spawner returns before the body finishes).
///
/// Lock discipline: one lock per method call, never spanning
/// evaluation (a body that `def`s while another thread holds the
/// lock would deadlock — `Mutex` is not reentrant), and the
/// `with_*` closures below must neither touch the registry nor
/// evaluate. Compound read-modify-write sequences (e.g.
/// `alter-var-root`'s read-call-install) are NOT atomic across
/// threads; single calls (`intern`, publishes) are.
/// The registry `current` namespace is lineage-global, but the
/// per-evaluator `*ns*` binding shadows it through
/// `current_ns_name`, so the global fallback is unobservable
/// while `*ns*` stays bound (boot binds it permanently).
#[derive(Clone, Debug)]
pub struct SharedRegistry {
    inner: Arc<Mutex<SharedInner>>,
}

/// The lineage-global contents: the registry plus the evaluator
/// identity counters (throwable/closure/native ids, `fn` display
/// numbers, `gensym`, form numbers — shared so forked evaluators
/// never stamp colliding identities).
#[derive(Debug)]
struct SharedInner {
    registry: NamespaceRegistry,
    next_id: u64,
    fn_seq: u64,
    gensym: u64,
    eval_seq: u64,
    alter_locks: HashMap<u64, Arc<VarAlterLock>>,
    protocol_fn_methods: HashMap<u64, crate::typeinternals::ProtocolFnInfo>,
}

/// Per-var reentrant alter lock (T1192 FX1): mirrors the JVM's
/// per-var monitor for `alter-var-root` — read-f-install runs
/// under exclusion, same-thread reentry nests. Textbook
/// mutex+condvar reentrant lock; the loom mirror
/// (`clj-eval/tests/loom_var_lock.rs`) explores every schedule.
#[derive(Debug)]
struct VarAlterLock {
    state: Mutex<AlterOwner>,
    cvar: Condvar,
}

#[derive(Debug)]
struct AlterOwner {
    owner: Option<std::thread::ThreadId>,
    depth: u32,
}

/// RAII release for [`SharedRegistry::lock_var_for_alter`].
#[derive(Debug)]
pub(crate) struct VarAlterGuard {
    lock: Arc<VarAlterLock>,
}

impl Drop for VarAlterGuard {
    fn drop(&mut self) {
        let mut owner = self.lock.state.lock().unwrap_or_else(|e| e.into_inner());
        owner.depth -= 1;
        if owner.depth == 0 {
            owner.owner = None;
            self.lock.cvar.notify_one();
        }
    }
}

impl Default for SharedRegistry {
    /// A fresh lineage (same seeds as [`SharedRegistry::new`]).
    fn default() -> Self {
        Self {
            inner: Arc::new(Mutex::new(SharedInner {
                registry: NamespaceRegistry::new(),
                next_id: 1,
                fn_seq: 0,
                gensym: 1,
                eval_seq: 0,
                alter_locks: HashMap::new(),
                protocol_fn_methods: HashMap::new(),
            })),
        }
    }
}

impl SharedRegistry {
    /// A fresh lineage: an empty registry with evaluator counters
    /// seeded exactly like [`Evaluator::new_bare`](crate::eval::Evaluator::new_bare).
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    /// Lock the lineage state (short critical sections only — poison
    /// is unreachable).
    fn lock(&self) -> MutexGuard<'_, SharedInner> {
        self.inner.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// The next throwable/closure/native identity.
    pub fn next_id(&self) -> u64 {
        let mut inner = self.lock();
        let id = inner.next_id;
        inner.next_id += 1;
        id
    }

    /// The next `fn` display sequence number.
    pub fn next_fn_seq(&self) -> u64 {
        let mut inner = self.lock();
        inner.fn_seq += 1;
        inner.fn_seq
    }

    /// Record a protocol method fn's cache identity (S14
    /// `__protocol_method*`, emitted by `defprotocol`).
    pub(crate) fn note_protocol_fn(&self, id: u64, info: crate::typeinternals::ProtocolFnInfo) {
        self.lock().protocol_fn_methods.insert(id, info);
    }

    /// A protocol method fn's cache identity, if registered
    /// (S14 `-cache-protocol-fn`).
    pub(crate) fn protocol_fn_info(&self, id: u64) -> Option<crate::typeinternals::ProtocolFnInfo> {
        self.lock().protocol_fn_methods.get(&id).cloned()
    }

    /// The next `gensym` counter value.
    pub fn next_gensym(&self) -> u64 {
        let mut inner = self.lock();
        let n = inner.gensym;
        inner.gensym += 1;
        n
    }

    /// Reset the `gensym` counter to 1 (boot restarts the
    /// user-visible counter after loading `core.clj`).
    pub fn reset_gensym(&self) {
        self.lock().gensym = 1;
    }

    /// The current top-level form number.
    #[must_use]
    pub fn eval_seq(&self) -> u64 {
        self.lock().eval_seq
    }

    /// Number the next top-level form, returning its number.
    pub fn bump_eval_seq(&self) -> u64 {
        let mut inner = self.lock();
        inner.eval_seq += 1;
        inner.eval_seq
    }

    /// The current namespace name.
    #[must_use]
    pub fn current(&self) -> String {
        self.lock().registry.current().to_owned()
    }

    /// Switch the current namespace (it must exist).
    pub fn set_current(&self, name: &str) {
        self.lock().registry.set_current(name);
    }

    /// Find or create a namespace, returning the (owned) namespace.
    pub fn ensure_ns(&self, name: &str) -> Namespace {
        self.lock().registry.ensure_ns(name).clone()
    }

    /// Look up a namespace by name.
    #[must_use]
    pub fn get(&self, name: &str) -> Option<Namespace> {
        self.lock().registry.get(name).cloned()
    }

    /// A live namespace's id (`None` when missing or removed).
    #[must_use]
    pub fn ns_id(&self, name: &str) -> Option<u64> {
        self.lock().registry.ns_id(name)
    }

    /// All live namespaces as `(name, id)`, sorted by name.
    #[must_use]
    pub fn all_ns_sorted(&self) -> Vec<(String, u64)> {
        self.lock().registry.all_ns_sorted()
    }

    /// A live or removed namespace by name (see
    /// [`NamespaceRegistry::ns_live_or_removed`]).
    #[must_use]
    pub fn ns_live_or_removed(&self, name: &str) -> Option<Namespace> {
        self.lock().registry.ns_live_or_removed(name).cloned()
    }

    /// A namespace through a handle-pinned id (see
    /// [`NamespaceRegistry::ns_for_handle`]).
    #[must_use]
    pub fn ns_for_handle(&self, name: &str, id: u64) -> Option<Namespace> {
        self.lock().registry.ns_for_handle(name, id).cloned()
    }

    /// Drop a namespace (see [`NamespaceRegistry::remove_ns`]).
    pub fn remove_ns(&self, name: &str) -> bool {
        self.lock().registry.remove_ns(name)
    }

    /// Qualify an unqualified symbol against an explicit namespace.
    #[must_use]
    pub fn qualify_in(&self, ns_name: &str, sym: &Symbol) -> Symbol {
        self.lock().registry.qualify_in(ns_name, sym)
    }

    /// The namespace a qualifier names, against an explicit namespace.
    #[must_use]
    pub fn namespace_target_in(&self, ns_name: &str, part: &str) -> Option<String> {
        self.lock()
            .registry
            .namespace_target_in(ns_name, part)
            .map(str::to_owned)
    }

    /// Resolve a symbol to its qualified target, against an explicit namespace.
    #[must_use]
    pub fn resolve_in(&self, ns_name: &str, sym: &Symbol) -> Option<Symbol> {
        self.lock().registry.resolve_in(ns_name, sym)
    }

    /// Resolve a symbol to its var id (`None` is unresolvable).
    #[must_use]
    pub fn lookup(&self, sym: &Symbol) -> Option<u64> {
        self.lock().registry.lookup(sym)
    }

    /// [`lookup`](Self::lookup) against an explicit namespace (the
    /// analyzer passes the `*ns*`-synced name).
    #[must_use]
    pub fn lookup_in(&self, ns_name: &str, sym: &Symbol) -> Option<u64> {
        self.lock().registry.lookup_in(ns_name, sym)
    }

    /// Intern (find or create) a var (see
    /// [`NamespaceRegistry::intern`]).
    pub fn intern(&self, sym: &Symbol) -> u64 {
        self.lock().registry.intern(sym)
    }

    /// Allocate an anonymous dynamic var (see
    /// [`NamespaceRegistry::fresh_anonymous`]).
    pub fn fresh_anonymous(&self) -> u64 {
        self.lock().registry.fresh_anonymous()
    }

    /// Intern a var into a live or removed namespace (see
    /// [`NamespaceRegistry::intern_in`]).
    pub fn intern_in(&self, ns_name: &str, sym: &Symbol) -> Option<u64> {
        self.lock().registry.intern_in(ns_name, sym)
    }

    /// Intern a `def`-family target (see
    /// [`NamespaceRegistry::intern_def_target`]).
    pub fn intern_def_target(&self, sym: &Symbol) -> u64 {
        self.lock().registry.intern_def_target(sym)
    }

    /// Intern a var through a handle-pinned id (see
    /// [`NamespaceRegistry::intern_in_pinned`]).
    pub fn intern_in_pinned(
        &self,
        ns_name: &str,
        expected: u64,
        sym: &Symbol,
    ) -> Option<(u64, u64)> {
        self.lock()
            .registry
            .intern_in_pinned(ns_name, expected, sym)
    }

    /// The var's state, if interned (an owned copy — mutate through
    /// [`with_state_mut`](Self::with_state_mut)).
    #[must_use]
    pub fn state(&self, id: u64) -> Option<VarState> {
        self.lock().registry.state(id).cloned()
    }

    /// `Var.isBound` (minus thread bindings).
    #[must_use]
    pub fn has_root(&self, id: u64) -> bool {
        self.lock().registry.has_root(id)
    }

    /// `Var.deref` (minus thread bindings). Never throws.
    #[must_use]
    pub fn root_or_unbound(&self, id: u64, sym: &Symbol) -> Value {
        self.lock().registry.root_or_unbound(id, sym)
    }

    /// Acquire a var's alter lock (reentrant exclusion for
    /// `alter-var-root`'s read-f-install, like the JVM's var
    /// monitor). Blocks while another thread alters the same var;
    /// same-thread reentry nests. The table lookup holds the
    /// lineage lock only briefly — never across the wait.
    pub(crate) fn lock_var_for_alter(&self, id: u64) -> VarAlterGuard {
        let lock = {
            let mut inner = self.lock();
            inner
                .alter_locks
                .entry(id)
                .or_insert_with(|| {
                    Arc::new(VarAlterLock {
                        state: Mutex::new(AlterOwner {
                            owner: None,
                            depth: 0,
                        }),
                        cvar: Condvar::new(),
                    })
                })
                .clone()
        };
        let current = std::thread::current().id();
        let mut owner = lock.state.lock().unwrap_or_else(|e| e.into_inner());
        loop {
            if matches!(&owner.owner, Some(held) if held == &current) {
                owner.depth += 1;
                break;
            }
            if owner.owner.is_some() {
                owner = lock.cvar.wait(owner).unwrap_or_else(|e| e.into_inner());
            } else {
                owner.owner = Some(current);
                owner.depth = 1;
                break;
            }
        }
        drop(owner);
        VarAlterGuard { lock }
    }

    /// Run `f` over a var's state (`None` when uninterned). The
    /// closure must neither touch the registry nor evaluate (the
    /// lock is held throughout).
    pub fn with_state_mut<R>(&self, id: u64, f: impl FnOnce(&mut VarState) -> R) -> Option<R> {
        let mut inner = self.lock();
        let state = inner.registry.state_mut(id)?;
        Some(f(state))
    }

    /// Run `f` over a live namespace (`None` when missing). Same
    /// closure discipline as [`with_state_mut`](Self::with_state_mut).
    pub fn with_ns_mut<R>(&self, name: &str, f: impl FnOnce(&mut Namespace) -> R) -> Option<R> {
        let mut inner = self.lock();
        let ns = inner.registry.get_mut(name)?;
        Some(f(ns))
    }

    /// Run `f` over a live or removed namespace (`None` when
    /// neither). Same closure discipline as
    /// [`with_state_mut`](Self::with_state_mut).
    pub fn with_live_or_removed_mut<R>(
        &self,
        name: &str,
        f: impl FnOnce(&mut Namespace) -> R,
    ) -> Option<R> {
        let mut inner = self.lock();
        let ns = inner.registry.ns_live_or_removed_mut(name)?;
        Some(f(ns))
    }

    /// Run `f` over a handle-pinned namespace (`None` when neither
    /// generation matches). Same closure discipline as
    /// [`with_state_mut`](Self::with_state_mut).
    pub fn with_handle_mut<R>(
        &self,
        name: &str,
        id: u64,
        f: impl FnOnce(&mut Namespace) -> R,
    ) -> Option<R> {
        let mut inner = self.lock();
        let ns = inner.registry.ns_for_handle_mut(name, id)?;
        Some(f(ns))
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
        // Poison is unreachable (nothing panics while holding a
        // frame lock); recover like `SharedRegistry::lock` so a
        // poisoned lock keeps the write instead of dropping it.
        let mut slots = self.slots.lock().unwrap_or_else(|e| e.into_inner());
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
        // `ns-alias-removed`): it resolves to the detached object
        // (upstream `namespaceFor` returns the alias object
        // liveness-blind), so an existing var is found while a
        // missing one keeps the `No such var` shape (#81).
        assert_eq!(reg.namespace_target("q"), Some("lib"));
        assert_eq!(
            reg.resolve(&sym(Some("q"), "v")),
            Some(sym(Some("lib"), "v"))
        );
        assert_eq!(reg.resolve(&sym(Some("q"), "missing")), None);
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

    #[test]
    fn lookup_qualifies_here_and_rejects_foreign() {
        let reg = rigged();
        // Unqualified resolves here; foreign qualifiers never do.
        let lib = reg.get("lib").unwrap();
        assert_eq!(
            lib.lookup(&sym(None, "v")),
            lib.lookup(&sym(Some("lib"), "v"))
        );
        assert!(lib.lookup(&sym(None, "v")).is_some());
        assert_eq!(
            reg.get("user").unwrap().lookup(&sym(Some("lib"), "v")),
            None
        );
    }

    #[test]
    fn live_or_removed_mut_answers_tombstones() {
        let mut reg = rigged();
        assert!(reg.remove_ns("lib"));
        // Live names take the live branch; removed ones fall to the
        // tombstone.
        assert!(reg.ns_live_or_removed_mut("user").is_some());
        assert!(reg.ns_live_or_removed_mut("lib").is_some());
        assert!(reg.ns_live_or_removed_mut("missing").is_none());
    }

    #[test]
    fn env_empty_tracks_slots() {
        let env = Env::new();
        assert!(env.is_empty());
        assert!(!env.extend(vec![Value::Nil]).is_empty());
    }
}
