// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Port-defined types (S-typechain stage 1): `deftype` / `defrecord`
//! descriptors and instances, `reify` / `proxy` instances, and
//! `gen-class` / `gen-interface` / `definterface` descriptors.
//!
//! The namespace is the registry: a `def` of a descriptor makes the
//! type name resolvable, and the fenced call surface (`(Type. …)`,
//! `(.-field x)`, `(.method x …)`, in `expr.rs`) resolves through
//! ordinary vars — there is no separate type table.
//!
//! A record instance is its descriptor plus one content map: declared
//! fields first (in order, missing fields read `nil`), then the
//! extension map. Order, equality, hashing, and printing all derive
//! from that single map, so the four can never disagree.

use std::collections::HashMap;
use std::sync::Arc;

use crate::{Map, Value};

/// What a [`HostDef`] describes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum HostKind {
    /// `deftype`: positional fields, identity equality, not a map.
    Type,
    /// `defrecord`: a map with the type name on it.
    Record,
    /// `create-struct`: a legacy structmap basis (S12). The
    /// instance is map-identical in print, `=`, and `hash`;
    /// only the basis keys (nil-defaulted, dissoc-guarded) make
    /// it a struct.
    Struct,
    /// `definterface` / `gen-interface`: method signatures only.
    Interface,
    /// `gen-class`: constructable, methods forward to `prefix` fns.
    GenClass,
}

/// One method implementation: fixed arity plus the closure value.
/// (`deftype` / `reify` / `proxy` methods are never variadic — the
/// `core.clj` macros reject `&` up front, like the JVM.)
#[derive(Clone, Debug)]
pub struct HostMethod {
    /// Number of args after `this`.
    pub arity: usize,
    /// The `(fn [this args…] …)` closure.
    pub func: Value,
}

/// A type or interface descriptor: the `def`ined value of
/// `deftype` / `defrecord` / `definterface` / `gen-interface` /
/// `gen-class`, and what `(Type. …)` resolves through.
#[derive(Clone, Debug)]
pub struct HostDef {
    /// JVM class-name analog: `munge(ns) + "." + name`, as written
    /// (only the namespace part is munged — oracle `n-sym-dash`).
    pub name: Arc<str>,
    pub kind: HostKind,
    /// Positional field names, in order (types and records).
    pub fields: Vec<Arc<str>>,
    /// Method name → overloads, for instances (empty on interfaces).
    pub methods: HashMap<Arc<str>, Vec<HostMethod>>,
    /// Claimed interface class names (stage 2 protocols read these).
    pub interfaces: Vec<Arc<str>>,
    /// `gen-class` method-fn prefix (`None` elsewhere).
    pub prefix: Option<Arc<str>>,
    /// `gen-class` implementation namespace (`None` elsewhere).
    pub impl_ns: Option<Arc<str>>,
    /// Interface signatures: method → arities (interfaces only).
    pub sigs: HashMap<Arc<str>, Vec<usize>>,
    /// Method fns take the instance as `this` (everything but
    /// `proxy`, whose bodies exclude it — oracle `proxy-str`).
    pub takes_this: bool,
    /// Struct basis keys, in order (S12; empty for every other
    /// kind). Instances carry these nil-defaulted ahead of any
    /// extras; `dissoc` of one throws, `accessor` checks
    /// membership here.
    pub struct_keys: Vec<Value>,
}

/// A live instance: descriptor plus content (declared fields first,
/// then extension entries) plus metadata.
#[derive(Clone, Debug)]
pub struct HostInst {
    pub def: Arc<HostDef>,
    pub content: Map,
    pub meta: Option<Map>,
}

/// A port-defined type value: descriptor or instance.
#[derive(Clone, Debug)]
pub enum HostVal {
    Def(Arc<HostDef>),
    Inst(HostInst),
}

impl HostVal {
    /// The class name, for descriptors and instances alike.
    #[must_use]
    pub fn type_name(&self) -> &str {
        match self {
            HostVal::Def(d) => &d.name,
            HostVal::Inst(i) => &i.def.name,
        }
    }

    /// The instance payload, if this is one.
    #[must_use]
    pub fn as_inst(&self) -> Option<&HostInst> {
        match self {
            HostVal::Inst(i) => Some(i),
            HostVal::Def(_) => None,
        }
    }
}

/// `Compiler.munge` over a namespace: `-` becomes `_`, and every
/// other non-`[A-Za-z0-9.]` character takes its stock expansion.
/// Only the namespace part is munged (the type name stays as
/// written — oracle `n-sym-dash`, `n-hash-dashed`).
const MUNGE_TABLE: &[(char, &str)] = &[
    ('-', "_"),
    (':', "_COLON_"),
    ('+', "_PLUS_"),
    ('>', "_GT_"),
    ('<', "_LT_"),
    ('=', "_EQ_"),
    ('~', "_TILDE_"),
    ('!', "_BANG_"),
    ('@', "_CIRCA_"),
    ('#', "_SHARP_"),
    ('\'', "_SINGLEQUOTE_"),
    ('"', "_DOUBLEQUOTE_"),
    ('%', "_PERCENT_"),
    ('^', "_CARET_"),
    ('&', "_AMPERSAND_"),
    ('*', "_STAR_"),
    ('|', "_PIPE_"),
    ('{', "_LBRACE_"),
    ('}', "_RBRACE_"),
    ('[', "_LBRACK_"),
    (']', "_RBRACK_"),
    ('/', "_SLASH_"),
    ('\\', "_BSLASH_"),
    ('?', "_QMARK_"),
];

#[must_use]
pub fn munge_ns(ns: &str) -> String {
    let mut out = String::with_capacity(ns.len());
    for c in ns.chars() {
        match MUNGE_TABLE.iter().find(|(k, _)| *k == c) {
            Some((_, rep)) => out.push_str(rep),
            None => out.push(c),
        }
    }
    out
}

/// The class name for a type defined as `name` in namespace `ns`.
#[must_use]
pub fn class_name(ns: &str, name: &str) -> String {
    format!("{}.{}", munge_ns(ns), name)
}

/// Host `=`: descriptors compare by identity (a re-`def` mints a new
/// type, like the JVM's new class); records compare same-type plus
/// content; struct instances compare by content alone (even across
/// bases — oracle `sm-cross-basis`); every other instance compares
/// by identity, so two separately constructed values are never `=`
/// (oracle `dtor-eq`). Records never equal plain maps (oracle
/// `rec-eq-map`); structs do (oracle `sm-eq-map`), via the
/// struct/map arms in `value.rs` (host pairs only meet here).
/// Callers check `Arc::ptr_eq` first, so `(= p p)` stays true.
#[must_use]
pub fn host_eq(a: &HostVal, b: &HostVal) -> bool {
    match (a, b) {
        (HostVal::Def(x), HostVal::Def(y)) => Arc::ptr_eq(x, y),
        (HostVal::Inst(x), HostVal::Inst(y)) => {
            if x.def.kind == HostKind::Struct && y.def.kind == HostKind::Struct {
                return x.content == y.content;
            }
            Arc::ptr_eq(&x.def, &y.def)
                && x.def.kind == HostKind::Record
                && y.def.kind == HostKind::Record
                && x.content == y.content
        }
        _ => false,
    }
}

/// `(hash record)`: the content-map hash XOR the plain
/// munged-class-name symbol hash (solved from the oracle:
/// `l-hash-rec`, `l-hash-p13`, `l-hash-swapped`, `foo-bar/R2` —
/// the symbol is never namespace-qualified, even when the
/// namespace needs munging).
#[must_use]
pub fn record_hasheq(content: &Map, class_name: &str) -> i32 {
    let map_hash = crate::hasheq(&Value::Map(content.clone()));
    let sym_hash = crate::hasheq(&Value::Symbol(crate::Symbol::new(None, class_name)));
    map_hash ^ sym_hash
}

/// `(hash non-record-instance)`: no oracle exists (the JVM uses the
/// identity hash), so a deterministic content mix — the class-name
/// symbol plus each content value in order. Deterministic and
/// `=`-consistent (identical instances share the `Arc`).
#[must_use]
pub fn inst_hasheq(h: &HostVal) -> i32 {
    let mut items = vec![Value::Symbol(crate::Symbol::new(None, h.type_name()))];
    match h {
        HostVal::Inst(i) => items.extend(i.content.iter().map(|(_, v)| v.clone())),
        HostVal::Def(_) => {}
    }
    crate::hasheq(&Value::Vector(crate::Vector::from_vec(items)))
}

/// Deterministic identity-hash digits for the `#object[Name 0xH …]`
/// and `Name@H` slots (the JVM prints the identity hash — same
/// shape, stable digits here).
#[must_use]
pub fn identity_hex(h: &HostVal) -> String {
    // Descriptors never reach here (they print their name); only
    // instances take the fallback slots.
    let n = match h {
        HostVal::Inst(i) if i.def.kind == HostKind::Record => {
            record_hasheq(&i.content, &i.def.name) as u32
        }
        other => inst_hasheq(other) as u32,
    };
    format!("{n:x}")
}

/// The `str`/`toString` fallback (no `toString` method defined):
/// `Name@hex`.
#[must_use]
pub fn fallback_str(h: &HostVal) -> String {
    format!("{}@{}", h.type_name(), identity_hex(h))
}

/// Find a method overload by arg count (after `this`).
#[must_use]
pub fn find_method<'a>(def: &'a HostDef, name: &str, argc: usize) -> Option<&'a HostMethod> {
    def.methods
        .get(name)
        .and_then(|overloads| overloads.iter().find(|m| m.arity == argc))
}

/// Whether `name` names a positional field of this descriptor.
#[must_use]
pub fn is_field(def: &HostDef, name: &str) -> bool {
    def.fields.iter().any(|f| f.as_ref() == name)
}
