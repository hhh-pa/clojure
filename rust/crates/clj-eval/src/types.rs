// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Hidden natives behind the S-typechain stage-1 surface: the
//! `deftype` / `defrecord` / `reify` / `proxy` / `gen-class` /
//! `gen-interface` / `definterface` macros in `core.clj` expand to
//! these, and the analyzer's fenced dot interception (`(Type. …)`,
//! `(.-field x)`, `(.method x …)`) synthesizes calls to `__new*` /
//! `__field*` / `__meth*`.
//!
//! The namespace is the registry: defining macros `def` a
//! descriptor, and construction/dispatch resolve that var at call
//! time — so a type is never usable before its `def` evaluates
//! (like the JVM's class load), and a re-`def` mints a new type.
//! Resolution failures use the analyzer's `Unable to resolve
//! symbol` text, minus the compile wrapper.

use std::collections::HashMap;
use std::sync::Arc;

use clj_value::{
    HostDef, HostInst, HostKind, HostMethod, HostVal, Map, PrintOpts, Symbol, ThrowClass, Value,
    class_name, fallback_str, find_method, is_field, new_map, pr_to_string,
};

use crate::core::cce_err;
use crate::error::{NativeError, jvm_class_of};
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

/// Hidden natives (unlisted helpers — the coverage matrix only
/// checks listed rows, so these stay out of it by design).
pub const TYPES_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("__deftype*", native_deftype),
    ("__record*", native_record),
    ("__interface*", native_interface),
    ("__genclass*", native_genclass),
    ("__reify*", native_reify),
    ("__proxy*", native_proxy),
    ("__new*", native_new),
    ("__field*", native_field),
    ("__meth*", native_meth),
    ("__record_map*", native_record_map),
];

/// Read a var root without interning (`None` is unresolvable).
fn var_root(eval: &Evaluator, sym: &Symbol) -> Option<Value> {
    let cur = eval.current_ns_name();
    let q = eval.registry.resolve_in(&cur, sym)?;
    let id = eval.registry.lookup(&q)?;
    Some(eval.deref_var(id, sym))
}

/// The `Unable to resolve symbol` runtime error (the analyzer's
/// text, without the compile wrapper).
fn unresolvable(sym: &Symbol) -> NativeError {
    NativeError::new(
        ThrowClass::RuntimeException,
        format!("Unable to resolve symbol: {sym} in this context"),
    )
}

/// Expect a string argument.
fn str_arg(v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Str(s) => Ok(s.to_string()),
        _ => Err(cce_err(v, "java.lang.String")),
    }
}

/// Expect a symbol argument.
fn sym_arg(v: &Value) -> Result<Symbol, NativeError> {
    match v {
        Value::Symbol(s) => Ok(s.clone()),
        _ => Err(cce_err(v, "clojure.lang.Symbol")),
    }
}

/// Expect a map argument.
fn map_arg<'a>(v: &'a Value, to: &str) -> Result<&'a clj_value::Map, NativeError> {
    match v {
        Value::Map(m) => Ok(m),
        _ => Err(cce_err(v, to)),
    }
}

/// Expect an int argument (method arities can only be counts).
fn int_arg(v: &Value) -> Result<usize, NativeError> {
    match v {
        Value::Int(n) if *n >= 0 => Ok(*n as usize),
        _ => Err(cce_err(v, "java.lang.Long")),
    }
}

/// The current root of a name (`None` is unresolvable): a dotted
/// name with no namespace (`user.Point`) resolves as namespace
/// `user` plus name `Point` (split at the last dot), so JVM-style
/// fully-qualified construction works without an import table;
/// slash-qualified symbols (`user/Point`) ride `resolve_in`
/// (aliases included); anything else checks the var first, then the
/// S16 import table (construction and interface position resolve
/// imports after vars). Shared by [`resolve_def`] and the
/// interface-position check.
fn lookup_root(eval: &Evaluator, sym: &Symbol) -> Option<Value> {
    if sym.namespace().is_none() {
        if let Some((ns_part, name_part)) = sym.name().rsplit_once('.') {
            let simple = Symbol::new(None, name_part);
            return eval
                .registry
                .resolve_in(ns_part, &simple)
                .and_then(|q| eval.registry.lookup(&q).map(|id| (id, q)))
                .map(|(id, q)| eval.deref_var(id, &q));
        }
        if let Some(root) = var_root(eval, sym) {
            return Some(root);
        }
        let cur = eval.current_ns_name();
        return crate::classemu::import_in(eval, &cur, sym.name()).map(Value::Class);
    }
    var_root(eval, sym)
}

/// A `HostDef` by qualified name (dotted split + registry, reusing
/// the import-or-var lookup): the `Value::Class` arm of construction
/// and the lattice's HostDef rule. Dotless names never match (the
/// JVM's `Class.forName` likewise rejects them), and import entries
/// resolve to classes, never descriptors, so this cannot regress.
pub(crate) fn hostdef_by_name(eval: &Evaluator, name: &str) -> Option<Arc<HostDef>> {
    if !name.contains('.') {
        return None;
    }
    let dotted = Symbol::new(None, name);
    match lookup_root(eval, &dotted) {
        Some(Value::HostType(h)) => match h.as_ref() {
            HostVal::Def(d) => Some(d.clone()),
            HostVal::Inst(_) => None,
        },
        _ => None,
    }
}

/// Resolve a symbol to a type descriptor value (`None` details in
/// the error: unresolvable vs bound-to-something-else).
fn resolve_def(eval: &Evaluator, sym: &Symbol) -> Result<Arc<HostDef>, NativeError> {
    def_of_root(eval, lookup_root(eval, sym), sym)
}

/// A resolved root must be a descriptor (an instance or any other
/// value is `Not a type`; missing or rootless is unresolvable). A
/// class resolves through its name to a `HostDef` (a JDK name with
/// no `HostDef` names the class, mirroring [`not_a_type`]).
fn def_of_root(
    eval: &Evaluator,
    root: Option<Value>,
    sym: &Symbol,
) -> Result<Arc<HostDef>, NativeError> {
    match root {
        Some(Value::HostType(h)) => match h.as_ref() {
            HostVal::Def(d) => Ok(d.clone()),
            HostVal::Inst(_) => Err(not_a_type(sym)),
        },
        Some(Value::Class(c)) => {
            hostdef_by_name(eval, c.name()).ok_or_else(|| not_a_class(c.name()))
        }
        Some(Value::Unbound(_)) | None => Err(unresolvable(sym)),
        Some(_) => Err(not_a_type(sym)),
    }
}

/// The bound-to-something-else error.
fn not_a_type(sym: &Symbol) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Not a type: {sym}"),
    )
}

/// The no-`HostDef` error for a class (mirroring [`not_a_type`]).
fn not_a_class(name: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("Not a type: {name}"),
    )
}

/// Resolve a symbol to an interface descriptor (for `reify` /
/// `proxy` / definition interface lists). An instance root is not
/// a type at all (its `Not a type` stands); any other bound
/// non-descriptor (like a string or number var) IS a type, just
/// not an interface; missing names stay unresolvable. Classes
/// resolve through their names (the `Value::Class` arm of
/// [`def_of_root`]) with the same miss shapes.
fn resolve_iface(eval: &Evaluator, sym: &Symbol) -> Result<Arc<HostDef>, NativeError> {
    let d = match resolve_def(eval, sym) {
        Ok(d) => d,
        Err(e) if e.class == ThrowClass::IllegalArgumentException => {
            if root_is_instance(eval, sym) {
                return Err(e);
            }
            return Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!("Not an interface: {sym}"),
            ));
        }
        Err(e) => return Err(e),
    };
    if d.kind != HostKind::Interface {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Not an interface: {sym}"),
        ));
    }
    Ok(d)
}

/// Whether a name resolves to an instance (split from
/// [`resolve_iface`] for the complexity budget): instances are not
/// types, so the `Not a type` error stands in interface position.
fn root_is_instance(eval: &Evaluator, sym: &Symbol) -> bool {
    matches!(lookup_root(eval, sym), Some(Value::HostType(h)) if matches!(h.as_ref(), HostVal::Inst(_)))
}

/// The `java.io.Closeable` contract as an interface descriptor
/// (split for budget): the only host interface inline bodies and
/// `with-open` may name — `close/0`, no interop import needed.
fn closeable_def() -> Arc<HostDef> {
    let mut sigs = HashMap::new();
    sigs.insert(Arc::from("close"), vec![0usize]);
    Arc::new(HostDef {
        name: Arc::from("java.io.Closeable"),
        kind: HostKind::Interface,
        fields: Vec::new(),
        methods: HashMap::new(),
        interfaces: Vec::new(),
        prefix: None,
        impl_ns: None,
        sigs,
        takes_this: true,
        struct_keys: Vec::new(),
    })
}

/// An interface-list entry: bare `Object` names the fence
/// superclass (skipped — its methods are the `object_method` set,
/// so `(defrecord R [x] Object (toString …))` keeps working);
/// `Closeable` (bare or `java.io`-qualified) is the close contract;
/// a protocol contributes its `:interface` descriptor (whose name
/// is the `:on` claim and whose sigs check the inline methods);
/// anything else must resolve to an interface.
fn resolve_iface_sym(eval: &mut Evaluator, s: &Value) -> Result<Option<Arc<HostDef>>, NativeError> {
    let sym = sym_arg(s)?;
    if sym.namespace().is_none() && sym.name() == "Object" {
        return Ok(None);
    }
    if sym.namespace().is_none() && sym.name() == "Closeable"
        || sym.namespace().is_none() && sym.name() == "java.io.Closeable"
        || sym.namespace().is_some_and(|n| n == "java.io") && sym.name() == "Closeable"
    {
        return Ok(Some(closeable_def()));
    }
    if let Some(p) = crate::protocol::proto_of(eval, &sym) {
        return Ok(Some(crate::protocol::proto_interface(&p.map)));
    }
    resolve_iface(eval, &sym).map(Some)
}

/// `Object` methods a body may define without an interface (the
/// fence superclass): `toString/0`, `hashCode/0`, `equals/1`.
fn object_method(name: &str, argc: usize) -> bool {
    matches!(
        (name, argc),
        ("toString", 0) | ("hashCode", 0) | ("equals", 1)
    )
}

/// Parse the methods map `{name-str {argc-int fn}}` (macros emit
/// exactly this; direct calls get the same validation).
fn parse_methods(v: &Value) -> Result<HashMap<Arc<str>, Vec<HostMethod>>, NativeError> {
    let m = map_arg(v, "clojure.lang.IPersistentMap")?;
    let mut out = HashMap::new();
    for (k, fns) in m.iter() {
        let name = match k {
            Value::Str(s) => Arc::from(s.as_ref()),
            _ => return Err(cce_err(k, "java.lang.String")),
        };
        let inner = map_arg(fns, "clojure.lang.IPersistentMap")?;
        let mut overloads = Vec::new();
        for (ak, f) in inner.iter() {
            overloads.push(HostMethod {
                arity: int_arg(ak)?,
                func: f.clone(),
            });
        }
        out.insert(name, overloads);
    }
    Ok(out)
}

/// Every method lives in a listed interface (at a listed arity) or
/// is an `Object` method — else the oracle's `Can't define method
/// not in interfaces` (see `ifprobe2`).
fn check_methods(
    methods: &HashMap<Arc<str>, Vec<HostMethod>>,
    ifaces: &[Arc<HostDef>],
) -> Result<(), NativeError> {
    for (name, overloads) in methods {
        for o in overloads {
            if object_method(name, o.arity) {
                continue;
            }
            let listed = ifaces.iter().any(|d| {
                d.sigs
                    .get(name.as_ref())
                    .is_some_and(|arities| arities.contains(&o.arity))
            });
            if !listed {
                return Err(NativeError::new(
                    ThrowClass::IllegalArgumentException,
                    format!("Can't define method not in interfaces: {name}"),
                ));
            }
        }
    }
    Ok(())
}

/// Shared `deftype` / `defrecord` definition: args are
/// `[ns-str name-str fields-vec-of-symbols methods-map
/// ifaces-vec-of-symbols]`.
fn define_type(
    eval: &mut Evaluator,
    display: &str,
    kind: HostKind,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity(display, args, &[5])?;
    let ns = str_arg(&args[0])?;
    let name = str_arg(&args[1])?;
    let fields = parse_fields(&args[2])?;
    let methods = parse_methods(&args[3])?;
    let ifaces = resolve_ifaces(eval, &args[4])?;
    check_methods(&methods, &ifaces)?;
    let interfaces = ifaces.iter().map(|d| d.name.clone()).collect();
    let def = HostDef {
        name: Arc::from(class_name(&ns, &name)),
        kind,
        fields,
        methods,
        interfaces,
        prefix: None,
        impl_ns: None,
        sigs: HashMap::new(),
        takes_this: true,
        struct_keys: Vec::new(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Def(Arc::new(def)))))
}

/// `deftype` / `defrecord` field names (split from [`define_type`]
/// for the complexity budget): a vector of simple symbols.
fn parse_fields(v: &Value) -> Result<Vec<Arc<str>>, NativeError> {
    let fields_v = match v {
        Value::Vector(v) => v,
        _ => return Err(cce_err(v, "clojure.lang.IPersistentVector")),
    };
    let mut fields = Vec::with_capacity(fields_v.len());
    for f in fields_v.iter() {
        match f {
            Value::Symbol(s) if s.namespace().is_none() => fields.push(Arc::from(s.name())),
            _ => {
                return Err(NativeError::new(
                    ThrowClass::IllegalArgumentException,
                    "Field names must be simple symbols".to_owned(),
                ));
            }
        }
    }
    Ok(fields)
}

/// `deftype` / `defrecord` interface lists (split from
/// [`define_type`] for the complexity budget): bare `Object`
/// entries skip (the fence superclass); the rest resolve.
fn resolve_ifaces(eval: &mut Evaluator, v: &Value) -> Result<Vec<Arc<HostDef>>, NativeError> {
    let ifaces_v = match v {
        Value::Vector(v) => v,
        _ => return Err(cce_err(v, "clojure.lang.IPersistentVector")),
    };
    let mut ifaces = Vec::with_capacity(ifaces_v.len());
    for s in ifaces_v.iter() {
        if let Some(d) = resolve_iface_sym(eval, s)? {
            ifaces.push(d);
        }
    }
    Ok(ifaces)
}

fn native_deftype(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    define_type(eval, "clojure.core/__deftype*", HostKind::Type, args)
}

fn native_record(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    define_type(eval, "clojure.core/__record*", HostKind::Record, args)
}

/// Expect a vector argument.
fn vec_arg(v: &Value) -> Result<&clj_value::Vector, NativeError> {
    match v {
        Value::Vector(x) => Ok(x),
        _ => Err(cce_err(v, "clojure.lang.IPersistentVector")),
    }
}

/// Parse `{name-str [argc-int …]}` signatures (macros normalize
/// both `definterface` sigs and `gen-class` `:methods` to this).
fn parse_sigs(v: &Value) -> Result<HashMap<Arc<str>, Vec<usize>>, NativeError> {
    let m = map_arg(v, "clojure.lang.IPersistentMap")?;
    let mut out = HashMap::new();
    for (k, v) in m.iter() {
        let name = match k {
            Value::Str(s) => Arc::from(s.as_ref()),
            _ => return Err(cce_err(k, "java.lang.String")),
        };
        let arities_v = vec_arg(v)?;
        let mut arities = Vec::with_capacity(arities_v.len());
        for a in arities_v.iter() {
            arities.push(int_arg(a)?);
        }
        out.insert(name, arities);
    }
    Ok(out)
}

/// Shared `definterface` / `gen-interface` definition: args are
/// `[ns-str name-str sigs-map]`. A dotted name is an explicit
/// fully-qualified class name (used verbatim — `gen-interface`
/// `:name`); otherwise the name qualifies with `ns` (munged).
fn native_interface(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__interface*", args, &[3])?;
    let ns = str_arg(&args[0])?;
    let name = str_arg(&args[1])?;
    let full = if name.contains('.') {
        name
    } else {
        class_name(&ns, &name)
    };
    let sigs = parse_sigs(&args[2])?;
    let def = HostDef {
        name: Arc::from(full),
        kind: HostKind::Interface,
        fields: Vec::new(),
        methods: HashMap::new(),
        interfaces: Vec::new(),
        prefix: None,
        impl_ns: None,
        sigs,
        takes_this: true,
        struct_keys: Vec::new(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Def(Arc::new(def)))))
}

/// `gen-class`: args are `[class-name-str prefix-str sigs-map
/// impl-ns-str ifaces-vec-of-symbols]`. The macros validate options;
/// the descriptor is constructable (a documented divergence — the
/// JVM only materializes the class under AOT).
fn native_genclass(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__genclass*", args, &[5])?;
    let name = str_arg(&args[0])?;
    let prefix = str_arg(&args[1])?;
    let sigs = parse_sigs(&args[2])?;
    let impl_ns = str_arg(&args[3])?;
    let ifaces_v = vec_arg(&args[4])?;
    let mut ifaces = Vec::with_capacity(ifaces_v.len());
    for s in ifaces_v.iter() {
        if let Some(d) = resolve_iface_sym(eval, s)? {
            ifaces.push(d);
        }
    }
    let interfaces = ifaces.iter().map(|d| d.name.clone()).collect();
    let def = HostDef {
        name: Arc::from(name),
        kind: HostKind::GenClass,
        fields: Vec::new(),
        methods: HashMap::new(),
        interfaces,
        prefix: Some(Arc::from(prefix)),
        impl_ns: Some(Arc::from(impl_ns)),
        sigs,
        takes_this: true,
        struct_keys: Vec::new(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Def(Arc::new(def)))))
}

/// Resolve the `reify` leading symbols through the shared
/// interface-list rule (`Object` skipped, the rest interfaces).
fn reify_ifaces(
    eval: &mut Evaluator,
    specs: &clj_value::Vector,
) -> Result<Vec<Arc<HostDef>>, NativeError> {
    let mut ifaces = Vec::with_capacity(specs.len());
    for s in specs.iter() {
        if let Some(d) = resolve_iface_sym(eval, s)? {
            ifaces.push(d);
        }
    }
    Ok(ifaces)
}

/// `reify`: args are `[specs-vec-of-symbols methods-map]`. The
/// instance is anonymous (`{ns}$reify` — no eval counters, so the
/// name stays stable across runs).
fn native_reify(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__reify*", args, &[2])?;
    let ifaces = reify_ifaces(eval, vec_arg(&args[0])?)?;
    let methods = parse_methods(&args[1])?;
    check_methods(&methods, &ifaces)?;
    let ns = eval.current_ns_name();
    let def = HostDef {
        name: Arc::from(format!("{}$reify", clj_value::munge_ns(&ns))),
        kind: HostKind::Type,
        fields: Vec::new(),
        methods,
        interfaces: ifaces.iter().map(|d| d.name.clone()).collect(),
        prefix: None,
        impl_ns: None,
        sigs: HashMap::new(),
        takes_this: true,
        struct_keys: Vec::new(),
    };
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: Arc::new(def),
        content: new_map(),
        meta: None,
    }))))
}

/// Build an instance: positional args zip the declared fields (the
/// analyzer guarantees the call shape; direct calls get the same
/// arity error as the JVM's missing-constructor path).
fn construct(def: &Arc<HostDef>, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() != def.fields.len() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No matching ctor found for class {}", def.name),
        ));
    }
    let mut content = new_map();
    for (f, v) in def.fields.iter().zip(args.iter()) {
        content = content.insert(Value::Keyword(clj_value::Keyword::new(None, f)), v.clone());
    }
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: def.clone(),
        content,
        meta: None,
    }))))
}

/// `(Type. …)`: args are `[type-sym-or-def args…]`. A descriptor
/// value works directly (factories capture it, so they build the
/// defining type from any namespace). Interfaces have no
/// constructors (the JVM's missing-ctor error); anything else
/// non-constructable is `Not a type`.
fn native_new(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.is_empty() {
        return Err(NativeError::arity("clojure.core/__new*", 0));
    }
    let def = match &args[0] {
        Value::Symbol(sym) => resolve_def(eval, sym)?,
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => d.clone(),
            HostVal::Inst(_) => {
                return Err(NativeError::new(
                    ThrowClass::IllegalArgumentException,
                    "Not a type".to_owned(),
                ));
            }
        },
        Value::Class(c) => hostdef_by_name(eval, c.name()).ok_or_else(|| not_a_class(c.name()))?,
        _ => return Err(cce_err(&args[0], "clojure.lang.Symbol")),
    };
    if def.kind == HostKind::Interface {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("No matching ctor found for class {}", def.name),
        ));
    }
    construct(&def, &args[1..])
}

/// `(.-field x)`: args are `[field-str target]`. Reads a positional
/// field (even a `nil`-valued one); anything else is the oracle's
/// `No matching field` error, with the target's class.
fn native_field(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__field*", args, &[2])?;
    let field = match &args[0] {
        Value::Str(s) => s.to_string(),
        Value::Symbol(s) => s.name().to_owned(),
        _ => return Err(cce_err(&args[0], "java.lang.String")),
    };
    match &args[1] {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) if is_field(&i.def, &field) => {
                let key = Value::Keyword(clj_value::Keyword::new(None, &field));
                Ok(i.content.get(&key).cloned().unwrap_or(Value::Nil))
            }
            _ => Err(no_field(&field, h.type_name())),
        },
        other => Err(no_field(&field, &jvm_class_of(other))),
    }
}

/// The oracle's missing-field error.
fn no_field(field: &str, class: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("No matching field found: {field} for class {class}"),
    )
}

/// A method call on a descriptor (split from [`native_meth`] for
/// the complexity budget): zero-arg calls read the field error
/// (the reflector reads fields first) unless the member is real
/// (then method-shape, like every non-nil miss), longer calls the
/// method error — descriptors never dispatch.
fn desc_miss(class: &str, method: &str, argc: usize) -> Result<Value, NativeError> {
    Err(
        if argc == 0 && !crate::classemu::surface_member_name(method) {
            no_field(method, class)
        } else {
            no_method(method, argc, class)
        },
    )
}

/// The oracle's missing-method error (the count always reads
/// `{n} args`, even for one — oracle `k-meth-arity`).
pub(crate) fn no_method(method: &str, argc: usize, class: &str) -> NativeError {
    NativeError::new(
        ThrowClass::IllegalArgumentException,
        format!("No matching method {method} found taking {argc} args for class {class}"),
    )
}

/// Call a method closure with the instance as `this` (shared
/// with protocol inline dispatch).
pub(crate) fn call_method(
    eval: &mut Evaluator,
    func: &Value,
    this: Value,
    args: &[Value],
) -> Result<Value, NativeError> {
    let mut full = Vec::with_capacity(args.len() + 1);
    full.push(this);
    full.extend(args.iter().cloned());
    eval.call(func, &full).map_err(signal_to_native)
}

/// The nil-target NPE (the JVM's reflector text for a nil
/// receiver — the member resolved, but there is no target).
fn npe_target() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"Object.getClass()\" because \"target\" is null".to_owned(),
    )
}

/// `(.method x args…)`: args are `[method-str target args…]`. The
/// S16 member surface goes first (nil targets take the R1 rule:
/// resolved members NPE, the rest miss); zero-arg host calls then
/// read a field first (like the JVM's reflector); `gen-class`
/// instances forward to their `prefix` fns; anything missing is the
/// oracle's field/method error. Non-host targets get the same shapes
/// with their own class (uniform — including `nil`).
fn native_meth(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    if args.len() < 2 {
        return Err(NativeError::arity("clojure.core/__meth*", args.len()));
    }
    let method = match &args[0] {
        Value::Str(s) => s.to_string(),
        Value::Symbol(s) => s.name().to_owned(),
        _ => return Err(cce_err(&args[0], "java.lang.String")),
    };
    let target = &args[1];
    let rest = &args[2..];
    if let Some(hit) = surface_probe(eval, &method, target, rest) {
        return hit;
    }
    // The descriptor plus the witness that the target is host (so
    // downstream dispatch never re-matches a proven fact).
    let host = match target {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) => Some((i.def.clone(), h)),
            HostVal::Def(d) => Some((d.clone(), h)),
        },
        _ => None,
    };
    match host {
        Some((d, h)) => meth_on_def(eval, &d, h, target.clone(), &method, rest),
        None => {
            let class = jvm_class_of(target);
            meth_miss(&method, target, rest, &class)
        }
    }
}

/// The S16 surface dispatch (split from [`native_meth`] for the
/// complexity budget): nil targets NPE on known surface members;
/// anything else on the surface dispatches there.
fn surface_probe(
    eval: &mut Evaluator,
    method: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    if matches!(target, Value::Nil) {
        if crate::classemu::surface_member_hit(method, rest.len()) {
            return Some(Err(npe_target()));
        }
        None
    } else {
        crate::classemu::surface_meth(eval, method, target, rest)
    }
}

/// The non-host miss report (split from [`native_meth`] for the
/// complexity budget): longer calls name the method; nil keeps
/// field-first (the R1 rule); every other 0-arg miss on a real
/// surface member reads method-shape (wrong kind or arity, not
/// absent).
fn meth_miss(
    method: &str,
    target: &Value,
    rest: &[Value],
    class: &str,
) -> Result<Value, NativeError> {
    if !rest.is_empty() {
        Err(no_method(method, rest.len(), class))
    } else if matches!(target, Value::Nil) {
        Err(no_field(method, class))
    } else if crate::classemu::surface_member_name(method) {
        Err(no_method(method, 0, class))
    } else {
        Err(no_field(method, class))
    }
}

/// Method dispatch on a host target (split from [`native_meth`]
/// for the complexity budget): descriptors miss (they carry no
/// instance methods, like the JVM's classes — never an invoke with
/// the descriptor as `this`); gen-class instances forward to their
/// prefix fns; anything else dispatches on the instance.
fn meth_on_def(
    eval: &mut Evaluator,
    d: &Arc<HostDef>,
    h: &Arc<HostVal>,
    target: Value,
    method: &str,
    rest: &[Value],
) -> Result<Value, NativeError> {
    if matches!(h.as_ref(), HostVal::Def(_)) {
        return desc_miss(&d.name, method, rest.len());
    }
    if d.kind == HostKind::GenClass {
        return genclass_call(eval, d, method, target, rest);
    }
    meth_on_host(eval, d, h.as_ref(), target, method, rest)
}

/// Run a method closure honoring the `takes_this` convention
/// (proxy fns exclude `this` and run with the args as written;
/// every other kind prepends the instance). Shared by `__meth*`
/// and `bean` so the two cannot drift.
fn call_with_this(
    eval: &mut Evaluator,
    d: &HostDef,
    func: &Value,
    target: Value,
    rest: &[Value],
) -> Result<Value, NativeError> {
    if d.takes_this {
        call_method(eval, func, target, rest)
    } else {
        eval.call(func, rest).map_err(signal_to_native)
    }
}

/// Call a 0-arg host method by name for `bean` (S16): descriptors
/// miss (they carry no instance methods), instances dispatch their
/// listed 0-arg overload (gen-class defs list none, so they never
/// reach here).
pub(crate) fn call_zero_arg_method(
    eval: &mut Evaluator,
    h: &Arc<HostVal>,
    target: Value,
    name: &str,
) -> Result<Value, NativeError> {
    match h.as_ref() {
        HostVal::Def(d) => desc_miss(&d.name, name, 0),
        HostVal::Inst(i) => {
            let Some(m) = find_method(&i.def, name, 0) else {
                return Err(no_method(name, 0, &i.def.name));
            };
            let func = m.func.clone();
            call_with_this(eval, &i.def, &func, target, &[])
        }
    }
}

/// Instance method/field dispatch (split from [`native_meth`] for
/// the complexity budget): zero-arg calls read a field first (like
/// the JVM's reflector), then a zero-arg method, else the `Object`
/// fallback; longer calls need a listed overload. Proxy fns exclude
/// `this`, so they run with the args as written; every other kind
/// prepends the instance.
fn meth_on_host(
    eval: &mut Evaluator,
    d: &Arc<HostDef>,
    h: &HostVal,
    target: Value,
    method: &str,
    rest: &[Value],
) -> Result<Value, NativeError> {
    if rest.is_empty() {
        if let Some(v) = field_hit(d, h, method) {
            return Ok(v);
        }
        if let Some(m) = find_method(d, method, 0) {
            let func = m.func.clone();
            return call_with_this(eval, d, &func, target, rest);
        }
        return object_fallback(h, method, &d.name);
    }
    match find_method(d, method, rest.len()) {
        Some(m) => {
            let func = m.func.clone();
            call_with_this(eval, d, &func, target, rest)
        }
        None => Err(no_method(method, rest.len(), &d.name)),
    }
}

/// Zero-arg field read (split from [`meth_on_host`] for the
/// complexity budget): the declared field value when the target is
/// an instance; descriptors never reach here (`native_meth` misses
/// first — the arm keeps the match total).
fn field_hit(d: &Arc<HostDef>, h: &HostVal, method: &str) -> Option<Value> {
    if !is_field(d, method) {
        return None;
    }
    match h {
        HostVal::Inst(i) => {
            let key = Value::Keyword(clj_value::Keyword::new(None, method));
            Some(i.content.get(&key).cloned().unwrap_or(Value::Nil))
        }
        // Descriptors fall through to the method lookup.
        HostVal::Def(_) => None,
    }
}

/// The 0-arg host-instance miss report (the JVM resolves `Object`
/// methods with no interface entry, but `toString/0` and
/// `hashCode/0` never reach here — the S16 member surface answers
/// them first): real surface members read method-shape, anything
/// else — and descriptors, which have no instance methods — the
/// oracle's field error.
fn object_fallback(h: &HostVal, method: &str, class: &str) -> Result<Value, NativeError> {
    match h {
        HostVal::Inst(_) => {
            if crate::classemu::surface_member_name(method) {
                return Err(no_method(method, 0, class));
            }
            Err(no_field(method, class))
        }
        HostVal::Def(_) => Err(no_field(method, class)),
    }
}

/// Forward a `gen-class` method call to its `prefix` fn in the
/// implementation namespace (the fn carries its own arities — a
/// missing or unbound fn is the stock unbound-fn error).
fn genclass_call(
    eval: &mut Evaluator,
    def: &Arc<HostDef>,
    method: &str,
    target: Value,
    rest: &[Value],
) -> Result<Value, NativeError> {
    let listed = def.sigs.keys().any(|m| m.as_ref() == method);
    if !listed {
        return Err(no_method(method, rest.len(), &def.name));
    }
    let prefix = def.prefix.as_deref().unwrap_or("");
    let impl_ns = def.impl_ns.as_deref().unwrap_or("");
    let fname = format!("{prefix}{method}");
    let sym = Symbol::new(Some(impl_ns), &fname);
    let root = var_root(eval, &sym).ok_or_else(|| unresolvable(&sym))?;
    match root {
        // A missing or rootless fn is the `resolve` miss (a bound
        // fn that is itself unbound fails inside `invoke_var` with
        // the stock unbound-fn error).
        Value::Unbound(_) => Err(unresolvable(&sym)),
        func => call_method(eval, &func, target, rest),
    }
}

/// Whether a map key is a declared field (plain keywords only —
/// a `"x"` string key is extension, even when a field is `x`).
pub(crate) fn is_field_key(field: &str, k: &Value) -> bool {
    matches!(k, Value::Keyword(kw) if kw.namespace().is_none() && kw.name() == field)
}

/// True for a record instance (the only map-like host value).
pub(crate) fn is_host_record(v: &Value) -> bool {
    record_inst(v).is_some()
}

/// The record instance inside a value, if it is one.
pub(crate) fn record_inst(v: &Value) -> Option<&HostInst> {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) if i.def.kind == HostKind::Record => Some(i),
            _ => None,
        },
        _ => None,
    }
}

/// True for a struct instance (S12 — the other map-like host value).
pub(crate) fn is_host_struct(v: &Value) -> bool {
    struct_inst(v).is_some()
}

/// The struct instance inside a value, if it is one (S12).
pub(crate) fn struct_inst(v: &Value) -> Option<&HostInst> {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) if i.def.kind == HostKind::Struct => Some(i),
            _ => None,
        },
        _ => None,
    }
}

/// The map-like host instance inside a value: a record or a struct
/// (S12 structmaps follow the record path for every map op except
/// `dissoc` of a basis key, which throws).
pub(crate) fn maplike_inst(v: &Value) -> Option<&HostInst> {
    record_inst(v).or_else(|| struct_inst(v))
}

/// Whether `k` is one of the struct's basis keys (S12; `=` over
/// the ordered basis — `dissoc` throws on these, `accessor`
/// requires one).
pub(crate) fn is_struct_key(inst: &HostInst, k: &Value) -> bool {
    inst.def.struct_keys.iter().any(|b| b == k)
}

/// Rebuild a record around new content (metadata travels — the
/// ad-hoc oracle: `assoc-meta`, `conj-meta`, `dissoc-meta`).
pub(crate) fn rewrap_record(inst: &HostInst, content: Map) -> Value {
    Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: inst.def.clone(),
        content,
        meta: inst.meta.clone(),
    })))
}

/// Rebuild a map-like host value around new content: records and
/// structs alike stay what they are (S12 `assoc` always stays a
/// struct — oracles `sm-assoc-class`, `sm-assoc-extra-class`).
/// Metadata travels, like `rewrap_record`.
pub(crate) fn rewrap_maplike(inst: &HostInst, content: Map) -> Value {
    Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: inst.def.clone(),
        content,
        meta: inst.meta.clone(),
    })))
}

/// The `(empty struct)` shape (S12): the same basis with
/// nil-defaulted basis keys in basis order, extras dropped,
/// meta dropped — still a structmap, still dissoc-guarded
/// (JVM-verified in the R1 fixup round).
pub(crate) fn empty_struct(inst: &HostInst) -> Value {
    let mut content = new_map();
    for k in inst.def.struct_keys.iter() {
        content = content.insert(k.clone(), Value::Nil);
    }
    Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: inst.def.clone(),
        content,
        meta: None,
    })))
}

/// Rebuild a map-like host value around new content with new
/// metadata (S12 `with-meta` on structs).
pub(crate) fn rewrap_maplike_with_meta(inst: &HostInst, content: Map, meta: Option<Map>) -> Value {
    Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def: inst.def.clone(),
        content,
        meta,
    })))
}

/// `(map->Type m)`: args are `[type-sym m]`. `nil` is the empty
/// record; maps split into fields first (in order, missing reads
/// `nil`) plus the extension map in iteration order; anything else
/// flattens one level through `seq` first (the ad-hoc oracle:
/// unseqable inputs — `map->int` — and unseqable elements —
/// `map->vec`, `map->str` — fail with the stock `ISeq` error).
/// Resolve a `map->Type` target (split from [`native_record_map`]
/// for the complexity budget): a symbol resolves through the
/// registry; factories pass the captured descriptor value.
fn record_def(eval: &mut Evaluator, v: &Value) -> Result<Arc<HostDef>, NativeError> {
    match v {
        Value::Symbol(sym) => {
            let def = resolve_def(eval, sym)?;
            if def.kind != HostKind::Record {
                return Err(NativeError::new(
                    ThrowClass::IllegalArgumentException,
                    format!("Not a record: {sym}"),
                ));
            }
            Ok(def)
        }
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) if d.kind == HostKind::Record => Ok(d.clone()),
            HostVal::Def(d) => Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                format!("Not a record: {}", d.name),
            )),
            HostVal::Inst(_) => Err(NativeError::new(
                ThrowClass::IllegalArgumentException,
                "Not a record".to_owned(),
            )),
        },
        _ => Err(cce_err(v, "clojure.lang.Symbol")),
    }
}

fn native_record_map(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__record_map*", args, &[2])?;
    let def = record_def(eval, &args[0])?;
    let m = match &args[1] {
        Value::Nil => new_map(),
        Value::Map(mm) => mm.clone(),
        other => flatten_entries(eval, other)?,
    };
    let mut content = new_map();
    for f in def.fields.iter() {
        let key = Value::Keyword(clj_value::Keyword::new(None, f));
        let v = m.get(&key).cloned().unwrap_or(Value::Nil);
        content = content.insert(key, v);
    }
    for (k, v) in m.iter() {
        if def.fields.iter().any(|f| is_field_key(f, k)) {
            continue;
        }
        content = content.insert(k.clone(), v.clone());
    }
    Ok(Value::HostType(Arc::new(HostVal::Inst(HostInst {
        def,
        content,
        meta: None,
    }))))
}

/// Flatten one level through `seq` into a map (odd item counts fail
/// in `hash-map`, exactly like the JVM's `map->` construction).
fn flatten_entries(eval: &mut Evaluator, v: &Value) -> Result<clj_value::Map, NativeError> {
    use crate::lazy::{first_value, next_value, seq_value};
    let mut flat = Vec::new();
    let mut cur = seq_value(eval, v).map_err(signal_to_native)?;
    loop {
        if matches!(cur, Value::Nil) {
            break;
        }
        let head = first_value(eval, &cur).map_err(signal_to_native)?;
        let mut pair = seq_value(eval, &head).map_err(signal_to_native)?;
        loop {
            if matches!(pair, Value::Nil) {
                break;
            }
            flat.push(first_value(eval, &pair).map_err(signal_to_native)?);
            pair = next_value(eval, &pair).map_err(signal_to_native)?;
        }
        cur = next_value(eval, &cur).map_err(signal_to_native)?;
    }
    if flat.len() % 2 != 0 {
        let last = flat.last().cloned().unwrap_or(Value::Nil);
        return Err(crate::coll::odd_keyvals_err(&last));
    }
    let mut out = new_map();
    for pair in flat.chunks(2) {
        out = out.insert(pair[0].clone(), pair[1].clone());
    }
    Ok(out)
}

/// `str` of a host value: descriptors render their name; instances
/// run a zero-arg `toString` method when defined (a non-string
/// result is the stock cast error), else the `Name@hex` fallback.
/// Struct instances render their content map readably (S12 — the
/// JVM's single-arg `str` is `.toString`, the readable map print,
/// exactly like `str` of a plain map).
pub(crate) fn host_str(
    eval: &mut Evaluator,
    h: &HostVal,
    this: Value,
) -> Result<String, NativeError> {
    match h {
        HostVal::Def(d) if d.kind == HostKind::Interface => Ok(format!("interface {}", d.name)),
        HostVal::Def(d) => Ok(d.name.to_string()),
        HostVal::Inst(i) if i.def.kind == HostKind::Struct => Ok(pr_to_string(
            &Value::Map(i.content.clone()),
            &PrintOpts::pr(),
        )),
        HostVal::Inst(i) => match find_method(&i.def, "toString", 0) {
            Some(m) => {
                let func = m.func.clone();
                // Proxy bodies exclude `this`, like in `native_meth`.
                let out = if i.def.takes_this {
                    call_method(eval, &func, this, &[])
                } else {
                    eval.call(&func, &[]).map_err(signal_to_native)
                };
                match out {
                    Ok(Value::Str(s)) => Ok(s.to_string()),
                    Ok(other) => Err(cce_err(&other, "java.lang.String")),
                    Err(e) => Err(e),
                }
            }
            None => Ok(fallback_str(h)),
        },
    }
}

/// `proxy`: args are `[super-sym ifaces-vec ctor-args-vec
/// methods-map]`. The superclass is fenced to `Object` (every other
/// host class is interop N1), which takes no constructor args.
fn native_proxy(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__proxy*", args, &[4])?;
    let super_sym = sym_arg(&args[0])?;
    if super_sym.namespace().is_some() || super_sym.name() != "Object" {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Cannot extend {super_sym}: proxy supports Object only"),
        ));
    }
    let ifaces_v = vec_arg(&args[1])?;
    let mut ifaces = Vec::with_capacity(ifaces_v.len());
    for s in ifaces_v.iter() {
        if let Some(d) = resolve_iface_sym(eval, s)? {
            ifaces.push(d);
        }
    }
    if !vec_arg(&args[2])?.is_empty() {
        return Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            "No matching ctor found for class java.lang.Object".to_owned(),
        ));
    }
    let methods = parse_methods(&args[3])?;
    check_methods(&methods, &ifaces)?;
    let ns = eval.current_ns_name();
    let def = HostDef {
        name: Arc::from(format!("{}$proxy", clj_value::munge_ns(&ns))),
        kind: HostKind::Type,
        fields: Vec::new(),
        methods,
        interfaces: ifaces.iter().map(|d| d.name.clone()).collect(),
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
    }))))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_def() -> Arc<HostDef> {
        Arc::new(HostDef {
            name: Arc::from("user.T"),
            kind: HostKind::Type,
            fields: vec![Arc::from("x")],
            methods: HashMap::new(),
            interfaces: Vec::new(),
            prefix: None,
            impl_ns: None,
            sigs: HashMap::new(),
            takes_this: true,
            struct_keys: Vec::new(),
        })
    }

    #[test]
    fn field_hit_on_descriptor_is_none() {
        // Descriptors never reach `field_hit` (`native_meth`
        // misses first); the arm keeps the match total.
        let def = test_def();
        assert_eq!(field_hit(&def, &HostVal::Def(def.clone()), "x"), None);
    }

    #[test]
    fn object_fallback_on_descriptor_misses() {
        // Likewise, the descriptor arm of `object_fallback` is
        // total-only — `native_meth` pre-empts it.
        let def = test_def();
        let err = object_fallback(&HostVal::Def(def), "m", "user.T").unwrap_err();
        assert_eq!(err.class, ThrowClass::IllegalArgumentException);
    }

    #[test]
    fn surface_nil_target_sweep() {
        // Every surface (member, argc) pair NPEs on a nil target
        // (the R1 rule); anything else keeps the miss shapes.
        let mut eval = crate::boot();
        let mut prev: Option<(&str, usize)> = None;
        for (member, argc) in crate::classemu::SURFACE_MEMBERS {
            if let Some((pm, pa)) = prev {
                assert!((pm, pa) < (*member, *argc), "unsorted surface table");
            }
            prev = Some((member, *argc));
            assert!(crate::classemu::surface_member_hit(member, *argc));
            let mut args = vec![
                Value::Symbol(clj_value::Symbol::new(None, member)),
                Value::Nil,
            ];
            args.extend(std::iter::repeat_n(Value::Nil, *argc));
            let err = native_meth(&mut eval, &args).unwrap_err();
            assert_eq!(err.class, ThrowClass::NullPointerException);
            assert_eq!(
                err.message,
                "Cannot invoke \"Object.getClass()\" because \"target\" is null"
            );
        }
        let args = vec![
            Value::Symbol(clj_value::Symbol::new(None, "nosuch_xyz")),
            Value::Nil,
        ];
        let err = native_meth(&mut eval, &args).unwrap_err();
        assert_eq!(err.class, ThrowClass::IllegalArgumentException);
    }

    #[test]
    fn call_zero_arg_method_defensive_arms() {
        // `bean` only calls listed instance methods, so the
        // descriptor arm and the unlisted-method arm need direct
        // pins: descriptors miss (field-shape for unknown members,
        // method-shape for real ones), unlisted methods miss.
        let mut eval = crate::boot();
        let def = test_def();
        let target = Value::HostType(Arc::new(HostVal::Def(def.clone())));
        let err = call_zero_arg_method(
            &mut eval,
            &Arc::new(HostVal::Def(def.clone())),
            target.clone(),
            "getX",
        )
        .unwrap_err();
        assert_eq!(
            err.message,
            "No matching field found: getX for class user.T"
        );
        let err = call_zero_arg_method(
            &mut eval,
            &Arc::new(HostVal::Def(def.clone())),
            target,
            "isEmpty",
        )
        .unwrap_err();
        assert_eq!(
            err.message,
            "No matching method isEmpty found taking 0 args for class user.T"
        );
        let inst = HostVal::Inst(HostInst {
            def: def.clone(),
            content: clj_value::new_map(),
            meta: None,
        });
        let err = call_zero_arg_method(
            &mut eval,
            &Arc::new(inst.clone()),
            Value::HostType(Arc::new(inst)),
            "getX",
        )
        .unwrap_err();
        assert_eq!(
            err.message,
            "No matching method getX found taking 0 args for class user.T"
        );
    }
}
