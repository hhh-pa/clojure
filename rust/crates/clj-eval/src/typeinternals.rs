// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Zero-oos S14 type internals (issue #58): JVM-internal factories
//! (`->ArrayChunk`, `->Eduction`, `->Vec`, `->VecNode`,
//! `->VecSeq`), proxy helpers (`construct-proxy`,
//! `get-proxy-class`, `init-proxy`, `proxy-call-with-super`,
//! `proxy-mappings`, `proxy-name`, `update-proxy`, `method-sig`
//! — plus the `proxy-super` macro and the `EMPTY-NODE` def in
//! `core.clj`), and protocol introspection
//! (`-cache-protocol-fn`, `-reset-methods`, `find-protocol-impl`,
//! `find-protocol-method`, `extenders`).
//!
//! Handles are `HostKind::Handle` instances over shared synthetic
//! defs (value equality); proxies carry their fn map in the
//! `HostInst::proxy_map` cell, which `__meth*` dispatch consults;
//! the protocol class walk runs over the probed lattice below
//! (see `docs/zero-oos-s14-typeinternals/`).

use std::collections::{BTreeSet, HashMap};
use std::sync::{Arc, LazyLock};

use clj_value::{
    Atom, HostDef, HostInst, HostKind, HostVal, Keyword, Map, Symbol, ThrowClass, Value, hasheq,
    new_map,
};

use crate::core::{cce_err, is_ifn};
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

pub const TYPEINTERNALS_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("->ArrayChunk", native_array_chunk),
    ("->Eduction", native_eduction_factory),
    ("->Vec", native_vec),
    ("->VecNode", native_vec_node),
    ("->VecSeq", native_vec_seq),
    ("-cache-protocol-fn", native_cache_protocol_fn),
    ("-reset-methods", native_reset_methods),
    ("__protocol_method*", native_protocol_method),
    ("construct-proxy", native_construct_proxy),
    ("extenders", native_extenders),
    ("find-protocol-impl", native_find_protocol_impl),
    ("find-protocol-method", native_find_protocol_method),
    ("get-proxy-class", native_get_proxy_class),
    ("init-proxy", native_init_proxy),
    ("method-sig", native_method_sig),
    ("proxy-call-with-super", native_proxy_call_with_super),
    ("proxy-mappings", native_proxy_mappings),
    ("proxy-name", native_proxy_name),
    ("update-proxy", native_update_proxy),
];

/// A protocol method fn's cache identity (what the JVM keeps in
/// the `MethodImplCache`): the protocol var plus the method name.
/// Registered by `__protocol_method*` (emitted by `defprotocol`);
/// read by `-cache-protocol-fn`.
#[derive(Clone, Debug)]
pub(crate) struct ProtocolFnInfo {
    pub var_id: u64,
    pub var_sym: Symbol,
    pub method: String,
}

/// One synthetic handle descriptor (shared by every instance its
/// factory mints, so value equality holds across calls).
fn handle_def(name: &'static str, fields: &[&'static str]) -> HostDef {
    HostDef {
        name: Arc::from(name),
        kind: HostKind::Handle,
        fields: fields.iter().map(|f| Arc::from(*f)).collect(),
        methods: HashMap::new(),
        interfaces: Vec::new(),
        prefix: None,
        impl_ns: None,
        sigs: HashMap::new(),
        takes_this: true,
        struct_keys: Vec::new(),
    }
}

static VEC_DEF: LazyLock<Arc<HostDef>> = LazyLock::new(|| {
    Arc::new(handle_def(
        "clojure.core.Vec",
        &["am", "cnt", "shift", "root", "tail", "_meta"],
    ))
});
static VECNODE_DEF: LazyLock<Arc<HostDef>> =
    LazyLock::new(|| Arc::new(handle_def("clojure.core.VecNode", &["edit", "arr"])));
static VECSEQ_DEF: LazyLock<Arc<HostDef>> = LazyLock::new(|| {
    Arc::new(handle_def(
        "clojure.core.VecSeq",
        &["am", "vec", "anode", "i", "offset", "_meta"],
    ))
});
static ARRAYCHUNK_DEF: LazyLock<Arc<HostDef>> = LazyLock::new(|| {
    Arc::new(handle_def(
        "clojure.core.ArrayChunk",
        &["am", "arr", "off", "end"],
    ))
});

/// Build a handle instance: positional args zip the declared
/// fields under keyword keys (like `deftype` construction, so
/// `.-field` reads work).
fn handle_inst(def: &Arc<HostDef>, args: &[Value]) -> Value {
    let mut content = new_map();
    for (f, v) in def.fields.iter().zip(args.iter()) {
        content = content.insert(Value::Keyword(Keyword::new(None, f)), v.clone());
    }
    Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: def.clone(),
        content,
        meta: None,
        proxy_map: None,
    })))
}

fn native_vec(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/->Vec", args, &[6])?;
    Ok(handle_inst(&VEC_DEF, args))
}

fn native_vec_seq(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/->VecSeq", args, &[6])?;
    Ok(handle_inst(&VECSEQ_DEF, args))
}

fn native_array_chunk(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/->ArrayChunk", args, &[4])?;
    Ok(handle_inst(&ARRAYCHUNK_DEF, args))
}

/// `->VecNode`: the JVM 2-arity plus the 1-arity port extension
/// (`arr` defaults to the 32-nil vector, so `(->VecNode nil)`
/// equals `EMPTY-NODE`).
fn native_vec_node(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/->VecNode", args, &[1, 2])?;
    if args.len() == 2 {
        return Ok(handle_inst(&VECNODE_DEF, args));
    }
    let arr = Value::Vector(clj_value::Vector::from_vec(vec![Value::Nil; 32]));
    Ok(handle_inst(&VECNODE_DEF, &[args[0].clone(), arr]))
}

fn native_eduction_factory(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/->Eduction", args, &[2])?;
    Ok(crate::xform::Eduction::of(args[0].clone(), args[1].clone()))
}

/// A proxy instance (else the house `IProxy` cast): `takes_this
/// == false` plus the fn map cell.
pub(crate) fn as_proxy_inst(v: &Value) -> Result<&HostInst, NativeError> {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) if !i.def.takes_this && i.proxy_map.is_some() => Ok(i),
            _ => Err(cce_err(v, "clojure.lang.IProxy")),
        },
        _ => Err(cce_err(v, "clojure.lang.IProxy")),
    }
}

/// The fn map cell of a validated proxy instance.
fn proxy_cell(inst: &HostInst) -> &Atom {
    // Validated by `as_proxy_inst` (which requires the cell), so
    // this only fires on hand-built shapes that natives never mint.
    inst.proxy_map.as_ref().expect("validated proxy has a cell")
}

/// Read the fn map verbatim (`nil` until initialized).
fn cell_map(cell: &Atom) -> Value {
    cell.lock().value.clone()
}

/// Replace the fn map, answering the proxy (shared by
/// `init-proxy` and the `proxy-call-with-super` finally).
fn cell_store(cell: &Atom, map: Value) {
    cell.lock().value = map;
}

/// Assoc `entries` onto the map (`nil`/non-map bases start from
/// `{}`, like the JVM's `assoc` over a null map).
fn cell_assoc(cell: &Atom, entries: &Map) {
    let mut base = match cell_map(cell) {
        Value::Map(m) => m,
        _ => new_map(),
    };
    for (k, v) in entries.iter() {
        base = base.insert(k.clone(), v.clone());
    }
    cell_store(cell, Value::Map(base));
}

/// The `__proxy*` starting map: single-overload methods collapse
/// to their fn (the JVM `{name → fn}` shape); multi-overload
/// methods keep the `{argc → fn}` submap (a port shape, pinned).
pub(crate) fn proxy_cell_from_methods(methods_map: &Value) -> Value {
    let mut out = new_map();
    let Value::Map(m) = methods_map else {
        return Value::Map(out);
    };
    for (name, fns) in m.iter() {
        let Value::Map(inner) = fns else {
            continue;
        };
        let entries: Vec<(&Value, &Value)> = inner.iter().collect();
        if entries.len() == 1 {
            out = out.insert(name.clone(), entries[0].1.clone());
        } else {
            out = out.insert(name.clone(), Value::Map(inner.clone()));
        }
    }
    Value::Map(out)
}

/// What proxy dispatch does with a cell entry.
pub(crate) enum ProxyHit {
    /// Call this (without `this`).
    Call(Value),
    /// Fall to the `Object` super/default path.
    Default,
}

/// Dispatch consult for proxies (shared by `__meth*` and
/// `str`): a fn (or any non-map non-nil value, called as-is)
/// wins; a `{argc → fn}` submap dispatches per arity; nil and
/// missing fall through — never to the baked table, so removal
/// truly reverts to super.
pub(crate) fn proxy_cell_hit(inst: &HostInst, method: &str, argc: usize) -> ProxyHit {
    let Some(cell) = inst.proxy_map.as_ref() else {
        return ProxyHit::Default;
    };
    let entry = match cell_map(cell) {
        Value::Map(m) => m.get(&Value::Str(Arc::from(method))).cloned(),
        _ => None,
    };
    match entry {
        None | Some(Value::Nil) => ProxyHit::Default,
        Some(Value::Map(am)) => match am.get(&Value::Int(argc as i64)) {
            Some(f) => ProxyHit::Call(f.clone()),
            None => ProxyHit::Default,
        },
        Some(other) => ProxyHit::Call(other),
    }
}

fn proxy_mappings_arg(v: &Value) -> Result<Map, NativeError> {
    match v {
        Value::Map(m) => Ok(m.clone()),
        _ => Err(cce_err(v, "clojure.lang.IPersistentMap")),
    }
}

fn native_init_proxy(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/init-proxy", args, &[2])?;
    let inst = as_proxy_inst(&args[0])?;
    let mappings = proxy_mappings_arg(&args[1])?;
    cell_store(proxy_cell(inst), Value::Map(mappings));
    Ok(args[0].clone())
}

fn native_update_proxy(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/update-proxy", args, &[2])?;
    let inst = as_proxy_inst(&args[0])?;
    let mappings = proxy_mappings_arg(&args[1])?;
    cell_assoc(proxy_cell(inst), &mappings);
    Ok(args[0].clone())
}

fn native_proxy_mappings(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/proxy-mappings", args, &[1])?;
    let inst = as_proxy_inst(&args[0])?;
    Ok(cell_map(proxy_cell(inst)))
}

fn native_proxy_call_with_super(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity("clojure.core/proxy-call-with-super", args, &[3])?;
    let inst = as_proxy_inst(&args[1])?;
    let cell = proxy_cell(inst).clone();
    let saved = cell_map(&cell);
    let mut masked = match &saved {
        Value::Map(m) => m.clone(),
        _ => new_map(),
    };
    masked = masked.insert(args[2].clone(), Value::Nil);
    cell_store(&cell, Value::Map(masked));
    let out = eval.call(&args[0], &[]).map_err(signal_to_native);
    cell_store(&cell, saved);
    out
}

/// A class-name string or `HostDef` value as its class name (the
/// S16 seam: classes travel as strings until `Class` values
/// land). Anything else is the house `Class` cast.
fn class_name_of(v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Str(s) => Ok(s.to_string()),
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => Ok(d.name.to_string()),
            _ => Err(cce_err(v, "java.lang.Class")),
        },
        _ => Err(cce_err(v, "java.lang.Class")),
    }
}

/// The simple name (past the last `.`), like the JVM's
/// interface-name mapping in `proxy-name`.
fn simple_name(class_name: &str) -> &str {
    class_name.rsplit('.').next().unwrap_or(class_name)
}

/// Realize the interfaces arg to class names (any seqable; `nil`
/// is empty, like the JVM's `map` over nil).
fn iface_names(eval: &mut Evaluator, v: &Value) -> Result<Vec<String>, NativeError> {
    let mut out = Vec::new();
    let mut cur = crate::lazy::seq_value(eval, v).map_err(signal_to_native)?;
    loop {
        if matches!(cur, Value::Nil) {
            return Ok(out);
        }
        let head = crate::lazy::first_value(eval, &cur).map_err(signal_to_native)?;
        out.push(class_name_of(&head)?);
        cur = crate::lazy::next_value(eval, &cur).map_err(signal_to_native)?;
    }
}

/// The JVM `proxy-name` string: munged `*ns*` (`-` → `_` only),
/// `.proxy`, then the super name, sorted simple interface
/// names, and the lower-hex set hash, `$`-joined.
fn proxy_name_str(ns: &str, super_name: &str, ifaces: &[String]) -> String {
    let inames: BTreeSet<&str> = ifaces.iter().map(String::as_str).collect();
    let mut set = clj_value::new_set();
    for name in &inames {
        set = set.insert(Value::Str(Arc::from(*name)));
    }
    let hex = format!("{:x}", hasheq(&Value::Set(set)) as u32);
    let mut out = ns.replace('-', "_");
    out.push_str(".proxy$");
    out.push_str(super_name);
    for name in &inames {
        out.push('$');
        out.push_str(simple_name(name));
    }
    out.push('$');
    out.push_str(&hex);
    out
}

fn native_proxy_name(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/proxy-name", args, &[2])?;
    let super_name = class_name_of(&args[0])?;
    let ifaces = iface_names(eval, &args[1])?;
    let ns = eval.current_ns_name();
    Ok(Value::Str(Arc::from(
        proxy_name_str(&ns, &super_name, &ifaces).as_str(),
    )))
}

/// One interface base: interface defs by name, strings as-is
/// (unverifiable until S16), non-interface defs rejected.
fn proxy_iface_base(v: &Value) -> Result<String, NativeError> {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) if d.kind == HostKind::Interface => Ok(d.name.to_string()),
            HostVal::Def(d) => Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!("Not an interface: {}", d.name),
            )),
            _ => Err(cce_err(v, "java.lang.Class")),
        },
        Value::Str(s) => Ok(s.to_string()),
        _ => Err(cce_err(v, "java.lang.Class")),
    }
}

fn proxy_ifaces(bases: &[Value]) -> Result<Vec<String>, NativeError> {
    bases.iter().map(proxy_iface_base).collect()
}

/// Split `get-proxy-class` bases into (super, interfaces),
/// mirroring `get-super-and-interfaces`: an interface def first
/// defaults the superclass to `Object`; a first string names the
/// superclass only when it is exactly `java.lang.Object`
/// (strings cannot report `isInterface`, and any other
/// superclass string would be illegal under the `Object` fence
/// anyway). Zero bases default the same way (the docstring's
/// contract; the JVM NPEs there).
fn proxy_bases_split(args: &[Value]) -> Result<(String, Vec<String>), NativeError> {
    if args.is_empty() {
        return Ok(("java.lang.Object".to_owned(), Vec::new()));
    }
    if args_first_is_iface(&args[0]) {
        return Ok(("java.lang.Object".to_owned(), proxy_ifaces(args)?));
    }
    match &args[0] {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => Ok((d.name.to_string(), proxy_ifaces(&args[1..])?)),
            _ => Err(cce_err(&args[0], "java.lang.Class")),
        },
        Value::Str(s) => Ok((s.to_string(), proxy_ifaces(&args[1..])?)),
        _ => Err(cce_err(&args[0], "java.lang.Class")),
    }
}

/// Whether the first base takes the interface slot (an
/// interface def, or any string but the `Object` one).
fn args_first_is_iface(first: &Value) -> bool {
    match first {
        Value::HostType(h) => {
            matches!(h.as_ref(), HostVal::Def(d) if d.kind == HostKind::Interface)
        }
        Value::Str(s) => s.as_ref() != "java.lang.Object",
        _ => false,
    }
}

fn native_get_proxy_class(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let (super_name, ifaces) = proxy_bases_split(args)?;
    if super_name != "java.lang.Object" {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot extend {super_name}: proxy supports Object only"),
        ));
    }
    let ns = eval.current_ns_name();
    Ok(Value::Str(Arc::from(
        proxy_name_str(&ns, &super_name, &ifaces).as_str(),
    )))
}

fn native_construct_proxy(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/construct-proxy", 0));
    }
    let Value::Str(c) = &args[0] else {
        return Err(cce_err(&args[0], "java.lang.Class"));
    };
    if !args[1..].is_empty() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No matching ctor found for class {c}"),
        ));
    }
    let def = HostDef {
        name: c.clone(),
        kind: HostKind::Type,
        fields: Vec::new(),
        methods: HashMap::new(),
        interfaces: Vec::new(),
        prefix: None,
        impl_ns: None,
        sigs: HashMap::new(),
        takes_this: false,
        struct_keys: Vec::new(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: Arc::new(def),
        content: new_map(),
        meta: None,
        proxy_map: Some(Atom::new(Value::Nil)),
    }))))
}

/// The triple items of a `method-sig` arg (a vector or list of
/// exactly three — the port descriptor contract).
fn method_sig_items(v: &Value) -> Result<Vec<Value>, NativeError> {
    let items: Vec<Value> = match v {
        Value::Vector(items) => items.iter().cloned().collect(),
        Value::List(items) => items.iter().cloned().collect(),
        _ => return Err(method_sig_shape()),
    };
    if items.len() == 3 {
        Ok(items)
    } else {
        Err(method_sig_shape())
    }
}

fn method_sig_shape() -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        "method-sig requires a [name params return] triple".to_owned(),
    )
}

fn native_method_sig(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/method-sig", args, &[1])?;
    let items = method_sig_items(&args[0])?;
    let Value::Str(name) = &items[0] else {
        return Err(cce_err(&items[0], "java.lang.String"));
    };
    let params = match &items[1] {
        Value::Nil => Value::Nil,
        Value::Vector(_) | Value::List(_) => {
            crate::lazy::seq_value(eval, &items[1]).map_err(signal_to_native)?
        }
        _ => {
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "method-sig params must be nil or sequential".to_owned(),
            ));
        }
    };
    Ok(Value::Vector(clj_value::Vector::from_vec(vec![
        Value::Str(name.clone()),
        params,
        items[2].clone(),
    ])))
}

// ---------------------------------------------------------------------------
// The class lattice (transcribed from `lattice.out`): each entry is
// `(class name, superclass chain past self, transitive supers)`.
// Sorted by class name for `binary_search` (see the sortedness
// test below). Throwables live in `THROW_PARENTS` instead (chain
// links plus `Serializable`, verified against the probed rows).
// ---------------------------------------------------------------------------

/// `(name, chain past self incl. `Object`, transitive supers)`.
type LatticeEntry = (
    &'static str,
    &'static [&'static str],
    &'static [&'static str],
);

const LATTICE: &[LatticeEntry] = &[
    (
        "clojure.core$future_call$reify",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "clojure.lang.IBlockingDeref",
            "clojure.lang.IDeref",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "java.util.concurrent.Future",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.core$promise$reify",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "clojure.lang.IBlockingDeref",
            "clojure.lang.IDeref",
            "clojure.lang.IFn",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.core.Eduction",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "clojure.lang.IReduceInit",
            "clojure.lang.IType",
            "clojure.lang.Sequential",
            "java.lang.Iterable",
        ],
    ),
    (
        "clojure.lang.APersistentMap$KeySeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.APersistentMap$ValSeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.APersistentVector$RSeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.APersistentVector$Seq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.APersistentVector$SubVector",
        &[
            "clojure.lang.APersistentVector",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentVector",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.Agent",
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
            "clojure.lang.IDeref",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.ArraySeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Associative",
        &[],
        &[
            "clojure.lang.ILookup",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.Seqable",
        ],
    ),
    (
        "clojure.lang.Atom",
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
            "clojure.lang.IAtom",
            "clojure.lang.IAtom2",
            "clojure.lang.IDeref",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.BigInt",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "java.io.Serializable",
        ],
    ),
    (
        "clojure.lang.ChunkedCons",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Cons",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    ("clojure.lang.Counted", &[], &[]),
    (
        "clojure.lang.Cycle",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Delay",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "clojure.lang.IDeref",
            "clojure.lang.IPending",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.IChunk",
        &[],
        &["clojure.lang.Counted", "clojure.lang.Indexed"],
    ),
    (
        "clojure.lang.IChunkedSeq",
        &[],
        &[
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
        ],
    ),
    (
        "clojure.lang.IFn",
        &[],
        &["java.lang.Runnable", "java.util.concurrent.Callable"],
    ),
    ("clojure.lang.ILookup", &[], &[]),
    ("clojure.lang.IMeta", &[], &[]),
    ("clojure.lang.IObj", &[], &["clojure.lang.IMeta"]),
    (
        "clojure.lang.IPersistentCollection",
        &[],
        &["clojure.lang.Seqable"],
    ),
    (
        "clojure.lang.IPersistentMap",
        &[],
        &[
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.ILookup",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.Seqable",
            "java.lang.Iterable",
        ],
    ),
    (
        "clojure.lang.IPersistentSet",
        &[],
        &[
            "clojure.lang.Counted",
            "clojure.lang.ILookup",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.Seqable",
        ],
    ),
    (
        "clojure.lang.IPersistentStack",
        &[],
        &["clojure.lang.IPersistentCollection", "clojure.lang.Seqable"],
    ),
    (
        "clojure.lang.IPersistentVector",
        &[],
        &[
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.ILookup",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
        ],
    ),
    ("clojure.lang.IReduce", &[], &["clojure.lang.IReduceInit"]),
    ("clojure.lang.IReduceInit", &[], &[]),
    (
        "clojure.lang.ISeq",
        &[],
        &["clojure.lang.IPersistentCollection", "clojure.lang.Seqable"],
    ),
    (
        "clojure.lang.Iterate",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Keyword",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.LazySeq",
        &["clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPending",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.LongRange",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.MapEntry",
        &[
            "clojure.lang.AMapEntry",
            "clojure.lang.APersistentVector",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.AMapEntry",
            "clojure.lang.APersistentVector",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMapEntry",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.Map$Entry",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.MultiFn",
        &["clojure.lang.AFn", "java.lang.Object"],
        &[
            "clojure.lang.AFn",
            "java.lang.Object",
            "clojure.lang.IFn",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentArrayMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMapIterable",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentArrayMap$Seq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentArrayMap$TransientArrayMap",
        &[
            "clojure.lang.ATransientMap",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.ATransientMap",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientAssociative",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientMap",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMapIterable",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashMap$TransientHashMap",
        &[
            "clojure.lang.ATransientMap",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.ATransientMap",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientAssociative",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientMap",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashSet",
        &[
            "clojure.lang.APersistentSet",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentSet",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentSet",
            "clojure.lang.Seqable",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.Set",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentHashSet$TransientHashSet",
        &[
            "clojure.lang.ATransientSet",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.ATransientSet",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientSet",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentList",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentList",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentList$EmptyList",
        &["clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentList",
            "clojure.lang.IPersistentStack",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentTreeMap",
        &[
            "clojure.lang.APersistentMap",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentMap",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentMap",
            "clojure.lang.MapEquivalence",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sorted",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Map",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentTreeSet",
        &[
            "clojure.lang.APersistentSet",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentSet",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentSet",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sorted",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.Set",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentVector",
        &[
            "clojure.lang.APersistentVector",
            "clojure.lang.AFn",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.AFn",
            "clojure.lang.APersistentVector",
            "java.lang.Object",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IKVReduce",
            "clojure.lang.ILookup",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IPersistentStack",
            "clojure.lang.IPersistentVector",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.Indexed",
            "clojure.lang.Reversible",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Iterable",
            "java.lang.Runnable",
            "java.util.Collection",
            "java.util.List",
            "java.util.RandomAccess",
            "java.util.SequencedCollection",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.PersistentVector$ChunkedSeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.PersistentVector$TransientVector",
        &["clojure.lang.AFn", "java.lang.Object"],
        &[
            "clojure.lang.AFn",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IFn",
            "clojure.lang.ILookup",
            "clojure.lang.ITransientAssociative",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.ITransientCollection",
            "clojure.lang.ITransientVector",
            "clojure.lang.Indexed",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.Range",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Ratio",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
        ],
    ),
    (
        "clojure.lang.ReaderConditional",
        &["java.lang.Object"],
        &["java.lang.Object", "clojure.lang.ILookup"],
    ),
    (
        "clojure.lang.Repeat",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    ("clojure.lang.Seqable", &[], &[]),
    ("clojure.lang.Sequential", &[], &[]),
    (
        "clojure.lang.StringSeq",
        &["clojure.lang.ASeq", "clojure.lang.Obj", "java.lang.Object"],
        &[
            "clojure.lang.ASeq",
            "clojure.lang.Obj",
            "java.lang.Object",
            "clojure.lang.Counted",
            "clojure.lang.IDrop",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.IPersistentCollection",
            "clojure.lang.IReduceInit",
            "clojure.lang.ISeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.Seqable",
            "clojure.lang.Sequential",
            "java.io.Serializable",
            "java.lang.Iterable",
            "java.util.Collection",
            "java.util.List",
            "java.util.SequencedCollection",
        ],
    ),
    (
        "clojure.lang.Symbol",
        &["clojure.lang.AFn", "java.lang.Object"],
        &[
            "clojure.lang.AFn",
            "java.lang.Object",
            "clojure.lang.IFn",
            "clojure.lang.IHashEq",
            "clojure.lang.IMeta",
            "clojure.lang.IObj",
            "clojure.lang.Named",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "clojure.lang.TaggedLiteral",
        &["java.lang.Object"],
        &["java.lang.Object", "clojure.lang.ILookup"],
    ),
    (
        "clojure.lang.Var",
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
        ],
        &[
            "clojure.lang.ARef",
            "clojure.lang.AReference",
            "java.lang.Object",
            "clojure.lang.IDeref",
            "clojure.lang.IFn",
            "clojure.lang.IMeta",
            "clojure.lang.IRef",
            "clojure.lang.IReference",
            "clojure.lang.Settable",
            "java.io.Serializable",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
            "java.util.function.BooleanSupplier",
            "java.util.function.DoubleSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.Supplier",
        ],
    ),
    (
        "clojure.lang.Var$Unbound",
        &["clojure.lang.AFn", "java.lang.Object"],
        &[
            "clojure.lang.AFn",
            "java.lang.Object",
            "clojure.lang.IFn",
            "java.lang.Runnable",
            "java.util.concurrent.Callable",
        ],
    ),
    (
        "java.lang.Boolean",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
        ],
    ),
    (
        "java.lang.Character",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
        ],
    ),
    (
        "java.lang.Double",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.lang.Long",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.lang.Number",
        &["java.lang.Object"],
        &["java.lang.Object", "java.io.Serializable"],
    ),
    ("java.lang.Object", &[], &[]),
    (
        "java.lang.String",
        &["java.lang.Object"],
        &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.CharSequence",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    ),
    (
        "java.math.BigDecimal",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
        ],
    ),
    (
        "java.math.BigInteger",
        &["java.lang.Number", "java.lang.Object"],
        &[
            "java.lang.Number",
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
        ],
    ),
    (
        "java.util.regex.Pattern",
        &["java.lang.Object"],
        &["java.lang.Object", "java.io.Serializable"],
    ),
];

/// Throwable parent links (`child → parent`, sorted for
/// `binary_search`): chains walk to `Object`; supers are the
/// chain plus `java.io.Serializable` (plus `IExceptionInfo` for
/// `ExceptionInfo`) — matching the probed `l-rt-ex`, `l-iae`,
/// `l-exinfo`, `l-error`, `l-exception`, `l-ioob`,
/// `l-iae-access`, and `l-ise` rows.
const THROW_PARENTS: &[(&str, &str)] = &[
    ("clojure.lang.ArityException", "java.lang.RuntimeException"),
    (
        "clojure.lang.Compiler$CompilerException",
        "java.lang.RuntimeException",
    ),
    ("clojure.lang.ExceptionInfo", "java.lang.RuntimeException"),
    (
        "clojure.lang.LispReader$ReaderException",
        "java.lang.Exception",
    ),
    ("java.io.FileNotFoundException", "java.io.IOException"),
    ("java.io.IOException", "java.lang.Exception"),
    (
        "java.lang.ArithmeticException",
        "java.lang.RuntimeException",
    ),
    ("java.lang.AssertionError", "java.lang.Error"),
    ("java.lang.ClassCastException", "java.lang.RuntimeException"),
    (
        "java.lang.ClassNotFoundException",
        "java.lang.ReflectiveOperationException",
    ),
    ("java.lang.Error", "java.lang.Throwable"),
    ("java.lang.Exception", "java.lang.Throwable"),
    (
        "java.lang.IllegalAccessError",
        "java.lang.IncompatibleClassChangeError",
    ),
    (
        "java.lang.IllegalArgumentException",
        "java.lang.RuntimeException",
    ),
    (
        "java.lang.IllegalStateException",
        "java.lang.RuntimeException",
    ),
    (
        "java.lang.IncompatibleClassChangeError",
        "java.lang.LinkageError",
    ),
    (
        "java.lang.IndexOutOfBoundsException",
        "java.lang.RuntimeException",
    ),
    ("java.lang.LinkageError", "java.lang.Error"),
    (
        "java.lang.NullPointerException",
        "java.lang.RuntimeException",
    ),
    (
        "java.lang.NumberFormatException",
        "java.lang.IllegalArgumentException",
    ),
    (
        "java.lang.ReflectiveOperationException",
        "java.lang.Exception",
    ),
    ("java.lang.RuntimeException", "java.lang.Exception"),
    (
        "java.lang.StringIndexOutOfBoundsException",
        "java.lang.IndexOutOfBoundsException",
    ),
    ("java.lang.Throwable", "java.lang.Object"),
    (
        "java.lang.UnsupportedOperationException",
        "java.lang.RuntimeException",
    ),
    (
        "java.util.DuplicateFormatFlagsException",
        "java.util.IllegalFormatFlagsException",
    ),
    (
        "java.util.FormatFlagsConversionMismatchException",
        "java.util.IllegalFormatFlagsException",
    ),
    (
        "java.util.IllegalFormatArgumentIndexException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.IllegalFormatCodePointException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.IllegalFormatConversionException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.IllegalFormatException",
        "java.lang.IllegalArgumentException",
    ),
    (
        "java.util.IllegalFormatFlagsException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.IllegalFormatPrecisionException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.IllegalFormatWidthException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.MissingFormatArgumentException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.MissingFormatConversionException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.MissingFormatWidthException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.UnknownFormatConversionException",
        "java.util.IllegalFormatException",
    ),
    (
        "java.util.concurrent.CancellationException",
        "java.lang.IllegalStateException",
    ),
    (
        "java.util.concurrent.ExecutionException",
        "java.lang.Exception",
    ),
    (
        "java.util.concurrent.RejectedExecutionException",
        "java.lang.RuntimeException",
    ),
    (
        "java.util.regex.PatternSyntaxException",
        "java.lang.IllegalArgumentException",
    ),
];

/// Look up a lattice entry by class name.
fn lattice_entry(name: &str) -> Option<&'static LatticeEntry> {
    LATTICE
        .binary_search_by_key(&name, |e| e.0)
        .ok()
        .map(|i| &LATTICE[i])
}

fn throw_parent(name: &str) -> Option<&'static str> {
    THROW_PARENTS
        .binary_search_by_key(&name, |e| e.0)
        .ok()
        .map(|i| THROW_PARENTS[i].1)
}

/// A throwable chain past self, by walking parent links
/// (`None` for non-throwables).
fn throw_chain(mut name: &str) -> Option<Vec<&'static str>> {
    let mut out = Vec::new();
    while let Some(parent) = throw_parent(name) {
        out.push(parent);
        name = parent;
    }
    if out.is_empty() { None } else { Some(out) }
}

/// The superclass chain past self (the JVM `super-chain` past
/// the head): lattice rows, throwable parent walks, else
/// `[Object]`. `null` walks nothing.
fn chain_past_self(name: &str) -> Vec<&'static str> {
    if name == "null" {
        return Vec::new();
    }
    if let Some(e) = lattice_entry(name) {
        return e.1.to_vec();
    }
    if let Some(chain) = throw_chain(name) {
        return chain;
    }
    vec!["java.lang.Object"]
}

/// Transitive supers of a class name: lattice rows, throwable
/// chains plus `Serializable` (`IExceptionInfo` for
/// `ExceptionInfo`), else `{Object}`. `null` has none.
fn supers_of_name(name: &str) -> Vec<&'static str> {
    if name == "null" {
        return Vec::new();
    }
    if let Some(e) = lattice_entry(name) {
        return e.2.to_vec();
    }
    if let Some(chain) = throw_chain(name) {
        let mut out = chain;
        out.push("java.io.Serializable");
        if name == "clojure.lang.ExceptionInfo" {
            out.push("clojure.lang.IExceptionInfo");
        }
        return out;
    }
    vec!["java.lang.Object"]
}

/// Fn values (whose class names are dynamic) walk as `AFn`s
/// (probe `l-fn`, minus the dynamic class itself).
const FN_SUPERS: &[&str] = &[
    "java.lang.Object",
    "clojure.lang.AFn",
    "clojure.lang.AFunction",
    "clojure.lang.Fn",
    "clojure.lang.IFn",
    "clojure.lang.IMeta",
    "clojure.lang.IObj",
    "java.io.Serializable",
    "java.lang.Runnable",
    "java.util.Comparator",
    "java.util.concurrent.Callable",
];

/// Whether a value is a closure or native fn (which walk as
/// `AFn`s — every other extension keeps its reported class).
fn is_fn_value(v: &Value) -> bool {
    match v {
        Value::Extension(e) => {
            e.as_any().downcast_ref::<crate::ext::Closure>().is_some()
                || e.as_any().downcast_ref::<crate::ext::NativeFn>().is_some()
        }
        _ => false,
    }
}

/// Transitive supers of a value: the lattice row for its class,
/// plus — for port instances — the listed interfaces (`IType`
/// for `deftype`/`defrecord`, `IProxy` for proxies, per the
/// probed `l-deftype`/`l-proxy` rows).
fn supers_of_value(x: &Value) -> Vec<String> {
    let mut out: BTreeSet<String> = BTreeSet::new();
    if is_fn_value(x) {
        out.extend(FN_SUPERS.iter().map(ToString::to_string));
    } else {
        out.extend(
            supers_of_name(&jvm_class_of(x))
                .iter()
                .map(|s| (*s).to_owned()),
        );
    }
    if let Value::HostType(h) = x {
        if let HostVal::Inst(i) = h.as_ref() {
            out.extend(i.def.interfaces.iter().map(|s| s.to_string()));
            if matches!(i.def.kind, HostKind::Type | HostKind::Record) {
                out.insert("clojure.lang.IType".to_owned());
            }
            if !i.def.takes_this {
                out.insert("clojure.lang.IProxy".to_owned());
            }
        }
    }
    out.into_iter().collect()
}

/// Whether `x` is an instance of the named class (for
/// `-cache-protocol-fn`'s `isInstance` shortcut).
fn is_instance_of(class: &str, x: &Value) -> bool {
    if jvm_class_of(x) == class {
        return true;
    }
    supers_of_value(x).iter().any(|s| s == class)
}

/// Whether class `a` is assignable from class `b` (for `pref`):
/// identity or membership in `b`'s supers.
fn is_assignable_from(a: &str, b: &str) -> bool {
    a == b || supers_of_name(b).contains(&a)
}

/// The JVM `pref`: the more specific of two implemented supers.
fn pref<'a>(a: &'a str, b: &'a str) -> &'a str {
    if is_assignable_from(a, b) { b } else { a }
}

/// Normalize a registry type entry (`{str → {argc → fn}}`) to
/// the JVM `{keyword → fn}` shape: empty overload maps are
/// skipped; multi-arity methods answer the smallest-arity fn.
fn normalize_impl_entry(entry: &Map) -> Value {
    let mut out = new_map();
    for (method, val) in entry.iter() {
        let name = match method {
            Value::Str(s) => s.to_string(),
            Value::Keyword(k) => k.name().to_owned(),
            _ => continue,
        };
        let Value::Map(am) = val else {
            continue;
        };
        let mut best: Option<(i64, &Value)> = None;
        for (argc, f) in am.iter() {
            let Value::Int(n) = argc else {
                continue;
            };
            if best.is_none_or(|(m, _)| *n < m) {
                best = Some((*n, f));
            }
        }
        if let Some((_, f)) = best {
            out = out.insert(Value::Keyword(Keyword::new(None, &name)), f.clone());
        }
    }
    Value::Map(out)
}

/// One registry cell (a missing — or hand-built non-map — entry
/// never shadows the rest of the walk).
fn impl_entry(impls: &Map, class: &str) -> Option<Value> {
    match impls.get(&Value::Str(Arc::from(class))) {
        Some(Value::Map(entry)) => Some(normalize_impl_entry(entry)),
        _ => None,
    }
}

/// The JVM `find-protocol-impl` walk over `:impls`: the instance
/// itself when its def claims `:on`; exact class; superclass
/// chain (excluding `Object`); `pref` over implemented supers
/// (excluding `Object` — sorted for determinism, where the JVM
/// iterates a hash set); `Object`. `nil` skips past the exact
/// step (the JVM's `(and c …)` guard).
fn find_impl_value(proto: &Map, x: &Value) -> Value {
    let (_, impls, on) = crate::protocol::proto_parts(proto);
    if let Value::HostType(h) = x {
        if let HostVal::Inst(i) = h.as_ref() {
            if crate::protocol::def_claims_proto(&i.def, on) {
                return x.clone();
            }
        }
    }
    let class = jvm_class_of(x);
    if let Some(hit) = impl_entry(impls, &class) {
        return hit;
    }
    if matches!(x, Value::Nil) {
        return Value::Nil;
    }
    for c in chain_past_self(&class) {
        if c == "java.lang.Object" {
            continue;
        }
        if let Some(hit) = impl_entry(impls, c) {
            return hit;
        }
    }
    let mut cands: Vec<String> = supers_of_value(x)
        .into_iter()
        .filter(|c| {
            c != "java.lang.Object" && impls.contains_key(&Value::Str(Arc::from(c.as_str())))
        })
        .collect();
    cands.sort();
    if let Some(best) = cands.iter().map(String::as_str).reduce(pref) {
        if let Some(hit) = impl_entry(impls, best) {
            return hit;
        }
    }
    if let Some(hit) = impl_entry(impls, "java.lang.Object") {
        return hit;
    }
    Value::Nil
}

fn native_find_protocol_impl(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/find-protocol-impl", args, &[2])?;
    let Some(proto) = crate::protocol::as_protocol_map(&args[0]) else {
        return Err(crate::protocol::not_a_protocol(&args[0]));
    };
    Ok(find_impl_value(proto, &args[1]))
}

fn native_find_protocol_method(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/find-protocol-method", args, &[3])?;
    let Some(proto) = crate::protocol::as_protocol_map(&args[0]) else {
        return Err(crate::protocol::not_a_protocol(&args[0]));
    };
    let imp = find_impl_value(proto, &args[2]);
    match crate::coll::lookup_present(eval, &imp, &args[1])? {
        Some(v) => Ok(v),
        None => Ok(Value::Nil),
    }
}

fn native_extenders(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/extenders", args, &[1])?;
    // Total like the JVM: non-protocols answer nil.
    let Some(proto) = crate::protocol::as_protocol_map(&args[0]) else {
        return Ok(Value::Nil);
    };
    let (_, impls, _) = crate::protocol::proto_parts(proto);
    crate::lazy::keys_thunk(eval, Value::Map(impls.clone()), true).map_err(signal_to_native)
}

fn native_reset_methods(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    let _ = eval;
    check_arity("clojure.core/-reset-methods", args, &[1])?;
    // No method cache exists (every protocol call reads the live
    // registry), so there is nothing to rebuild — total like the
    // JVM, which `doseq`s over a possibly-absent builder map.
    Ok(Value::Nil)
}

fn native_protocol_method(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__protocol_method*", args, &[3])?;
    let Value::Var(var) = &args[0] else {
        return Err(cce_err(&args[0], "clojure.lang.Var"));
    };
    let Value::Str(method) = &args[1] else {
        return Err(cce_err(&args[1], "java.lang.String"));
    };
    let Some(closure) = crate::ext::as_closure(&args[2]) else {
        return Err(cce_err(&args[2], "clojure.lang.AFunction"));
    };
    eval.registry.note_protocol_fn(
        closure.id,
        ProtocolFnInfo {
            var_id: var.id,
            var_sym: var.sym.clone(),
            method: method.to_string(),
        },
    );
    Ok(args[2].clone())
}

/// The oracle miss text (shared shape with `__protocol_invoke*`;
/// `nil` prints `nil`, like the JVM).
fn no_impl_text(info: &ProtocolFnInfo, x: &Value) -> String {
    let class = if matches!(x, Value::Nil) {
        "nil".to_owned()
    } else {
        jvm_class_of(x)
    };
    format!(
        "No implementation of method: :{} of protocol: #'{} found for class: {class}",
        info.method, info.var_sym
    )
}

fn native_cache_protocol_fn(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/-cache-protocol-fn", args, &[4])?;
    let Some(closure) = crate::ext::as_closure(&args[0]) else {
        return Err(cce_err(&args[0], "clojure.lang.AFunction"));
    };
    let info = eval.registry.protocol_fn_info(closure.id).ok_or_else(|| {
        NativeError::new(
            ThrowClass::IllegalArgumentException,
            "Not a protocol fn".to_owned(),
        )
    })?;
    let c_name = match &args[2] {
        Value::Str(s) => s.to_string(),
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => d.name.to_string(),
            _ => return Err(cce_err(&args[2], "java.lang.Class")),
        },
        _ => return Err(cce_err(&args[2], "java.lang.Class")),
    };
    if !is_ifn(&args[3]) {
        return Err(cce_err(&args[3], "clojure.lang.IFn"));
    }
    let root = eval.deref_var(info.var_id, &info.var_sym);
    let Some(proto) = crate::protocol::as_protocol_map(&root) else {
        return Err(crate::protocol::not_a_protocol(&root));
    };
    if is_instance_of(&c_name, &args[1]) {
        return Ok(args[3].clone());
    }
    let imp = find_impl_value(proto, &args[1]);
    let methodk = Value::Keyword(Keyword::new(None, &info.method));
    match crate::coll::lookup_present(eval, &imp, &methodk)? {
        Some(f) => Ok(f),
        None => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            no_impl_text(&info, &args[1]),
        )),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_tables_sorted() {
        let mut keys: Vec<&str> = LATTICE.iter().map(|e| e.0).collect();
        let mut sorted = keys.clone();
        sorted.sort();
        assert_eq!(keys, sorted, "LATTICE must stay sorted for binary_search");
        keys = THROW_PARENTS.iter().map(|e| e.0).collect();
        sorted = keys.clone();
        sorted.sort();
        assert_eq!(
            keys, sorted,
            "THROW_PARENTS must stay sorted for binary_search"
        );
        let _ = keys;
    }
}
