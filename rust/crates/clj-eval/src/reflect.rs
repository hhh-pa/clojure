// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! `clojure.reflect` backing (#120 sub-slice 4): the `__reflect*`
//! and `__reflect_typename*` hidden natives behind the embedded
//! `reflect.clj` facsimile. Reflection reads the emulated lattice
//! only — never a JVM: `:bases` are the `ClassVal` bases as
//! symbols, `:flags-raw` the emulated modifier bits (the `.clj`
//! side parses them through the upstream `parse-flags`), and
//! `:members` the emulated declared members below. Member coverage
//! is exactly the emulated surface (`..` members, `Class/forName`,
//! the `System` statics, the `String`/`Object` ctors): every reported
//! member is invokable/constructible through the port, and nothing
//! else is reported (pins invoke the members, proving invokability).
//! New emulated members must add a row here plus a pin in
//! `tests/reflect.rs`.

use std::sync::Arc;

use clj_value::{ClassVal, Keyword, Symbol, ThrowClass, Value, Vector, new_map, new_set};

use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity};

/// The reflect hidden natives (an implementation detail of the
/// embedded `clojure.reflect`, in the standing of `__import*`:
/// public so the facsimile calls analyze, unlisted in the matrix).
pub(crate) const REFLECT_HIDDEN: &[(&str, NativeFnPtr)] = &[
    ("__reflect*", native_reflect),
    ("__reflect_typename*", native_typename),
];

/// The emulated modifier bits (a subset of the JVM access flags —
/// exactly what `parse-flags` in `reflect.clj` can observe).
const MOD_PUBLIC: i64 = 0x0001;
const MOD_STATIC: i64 = 0x0008;
const MOD_INTERFACE: i64 = 0x0200;
const MOD_ABSTRACT: i64 = 0x0400;

/// One emulated declared member: ctors render with the binary name
/// (upstream `Constructor.getName`), methods and statics with
/// `name`.
struct Member {
    kind: MemberKind,
    name: &'static str,
    ret: &'static str,
    params: &'static [&'static str],
}

/// A ctor, an instance method, or a static method.
#[derive(Clone, Copy, PartialEq, Eq)]
enum MemberKind {
    Ctor,
    Method,
    Static,
}

/// The emulated declared members by binary name (one row per class
/// with emulated members; every other lattice class reflects an
/// empty member set). Types are `typename` spellings (primitives
/// bare, arrays `<>`-suffixed); every entry is pinned by
/// `tests/reflect.rs`.
const CLASS_MEMBERS: &[(&str, &[Member])] = &[
    (
        "java.lang.Object",
        &[
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "equals",
                ret: "boolean",
                params: &["java.lang.Object"],
            },
            Member {
                kind: MemberKind::Method,
                name: "hashCode",
                ret: "int",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "toString",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getClass",
                ret: "java.lang.Class",
                params: &[],
            },
        ],
    ),
    (
        "java.lang.Class",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getName",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getSimpleName",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getSuperclass",
                ret: "java.lang.Class",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getInterfaces",
                ret: "java.lang.Class<>",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "isInterface",
                ret: "boolean",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "isArray",
                ret: "boolean",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "isInstance",
                ret: "boolean",
                params: &["java.lang.Object"],
            },
            Member {
                kind: MemberKind::Method,
                name: "isAssignableFrom",
                ret: "boolean",
                params: &["java.lang.Class"],
            },
            Member {
                kind: MemberKind::Method,
                name: "cast",
                ret: "java.lang.Object",
                params: &["java.lang.Object"],
            },
            Member {
                kind: MemberKind::Static,
                name: "forName",
                ret: "java.lang.Class",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Static,
                name: "forName",
                ret: "java.lang.Class",
                params: &["java.lang.Module", "java.lang.String"],
            },
            Member {
                kind: MemberKind::Static,
                name: "forName",
                ret: "java.lang.Class",
                params: &["java.lang.String", "boolean", "java.lang.ClassLoader"],
            },
        ],
    ),
    (
        "java.lang.String",
        &[
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &[],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["char<>"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["byte<>"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["byte<>", "int"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["byte<>", "java.lang.String"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["char<>", "int", "int"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["byte<>", "int", "int"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["int<>", "int", "int"],
            },
            Member {
                kind: MemberKind::Ctor,
                name: "",
                ret: "",
                params: &["byte<>", "int", "int", "java.lang.String"],
            },
            Member {
                kind: MemberKind::Method,
                name: "isEmpty",
                ret: "boolean",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "isBlank",
                ret: "boolean",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "length",
                ret: "int",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "charAt",
                ret: "char",
                params: &["int"],
            },
            Member {
                kind: MemberKind::Method,
                name: "substring",
                ret: "java.lang.String",
                params: &["int"],
            },
            Member {
                kind: MemberKind::Method,
                name: "substring",
                ret: "java.lang.String",
                params: &["int", "int"],
            },
            Member {
                kind: MemberKind::Method,
                name: "startsWith",
                ret: "boolean",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Method,
                name: "startsWith",
                ret: "boolean",
                params: &["java.lang.String", "int"],
            },
            Member {
                kind: MemberKind::Method,
                name: "endsWith",
                ret: "boolean",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Method,
                name: "split",
                ret: "java.lang.String<>",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Method,
                name: "split",
                ret: "java.lang.String<>",
                params: &["java.lang.String", "int"],
            },
            Member {
                kind: MemberKind::Method,
                name: "trim",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "toUpperCase",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "toLowerCase",
                ret: "java.lang.String",
                params: &[],
            },
        ],
    ),
    (
        "java.lang.System",
        &[
            Member {
                kind: MemberKind::Static,
                name: "getProperty",
                ret: "java.lang.String",
                params: &["java.lang.String"],
            },
            Member {
                kind: MemberKind::Static,
                name: "getProperty",
                ret: "java.lang.String",
                params: &["java.lang.String", "java.lang.String"],
            },
            Member {
                kind: MemberKind::Static,
                name: "lineSeparator",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Static,
                name: "identityHashCode",
                ret: "int",
                params: &["java.lang.Object"],
            },
        ],
    ),
    (
        "java.util.Collection",
        &[Member {
            kind: MemberKind::Method,
            name: "isEmpty",
            ret: "boolean",
            params: &[],
        }],
    ),
    (
        "java.util.Map",
        &[Member {
            kind: MemberKind::Method,
            name: "isEmpty",
            ret: "boolean",
            params: &[],
        }],
    ),
    (
        "java.util.List",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getFirst",
                ret: "java.lang.Object",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getLast",
                ret: "java.lang.Object",
                params: &[],
            },
        ],
    ),
    (
        "java.util.Map$Entry",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getKey",
                ret: "java.lang.Object",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getValue",
                ret: "java.lang.Object",
                params: &[],
            },
        ],
    ),
    (
        "clojure.lang.Named",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getName",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getNamespace",
                ret: "java.lang.String",
                params: &[],
            },
        ],
    ),
    (
        "clojure.lang.Keyword",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getName",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getNamespace",
                ret: "java.lang.String",
                params: &[],
            },
        ],
    ),
    (
        "clojure.lang.Symbol",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getName",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getNamespace",
                ret: "java.lang.String",
                params: &[],
            },
        ],
    ),
    (
        "java.lang.Throwable",
        &[
            Member {
                kind: MemberKind::Method,
                name: "getMessage",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getLocalizedMessage",
                ret: "java.lang.String",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "getCause",
                ret: "java.lang.Throwable",
                params: &[],
            },
        ],
    ),
    (
        "clojure.lang.ExceptionInfo",
        &[Member {
            kind: MemberKind::Method,
            name: "getData",
            ret: "java.lang.Object",
            params: &[],
        }],
    ),
    (
        "clojure.lang.IPending",
        &[Member {
            kind: MemberKind::Method,
            name: "isRealized",
            ret: "boolean",
            params: &[],
        }],
    ),
    (
        "java.io.Writer",
        &[
            Member {
                kind: MemberKind::Method,
                name: "close",
                ret: "void",
                params: &[],
            },
            Member {
                kind: MemberKind::Method,
                name: "flush",
                ret: "void",
                params: &[],
            },
        ],
    ),
];

/// The emulated declared members of a lattice class (split for the
/// complexity budget: the table stays a linear scan like the lattice
/// itself).
fn class_members(name: &str) -> Option<&'static [Member]> {
    CLASS_MEMBERS
        .iter()
        .find(|(n, _)| *n == name)
        .map(|(_, m)| *m)
}

/// `__reflect*`: reflect a typeref (class, symbol, or `typename`
/// string — the facsimile's `do-reflect` resolves through
/// `typename`, exactly like upstream's `typeref->class`) over the
/// emulated lattice. Unknown names are `ClassNotFoundException`
/// (the `import`/`forName` shape); nil is the house NPE, anything
/// else the `String` CCE (both reachable only through direct
/// hidden-native calls — the facsimile validates through the
/// `TypeReference` protocol first).
fn native_reflect(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__reflect*", args, &[1])?;
    let name = typeref_name(&args[0])?;
    let class = crate::classemu::resolve_class_name(eval, &name)
        .ok_or_else(|| NativeError::new(ThrowClass::ClassNotFoundException, name))?;
    Ok(reflect_map(&class))
}

/// `__reflect_typename*`: the upstream `typename` mapping (binary
/// name for classes, `<>`-restored text for symbols, strings
/// through) behind the facsimile's `TypeReference` extensions.
fn native_typename(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__reflect_typename*", args, &[1])?;
    typeref_name(&args[0]).map(|n| Value::Str(Arc::from(n.as_str())))
}

/// The lattice name of a typeref (split for the complexity budget).
fn typeref_name(v: &Value) -> Result<String, NativeError> {
    match v {
        Value::Class(c) => Ok(c.name().to_owned()),
        Value::Symbol(s) => Ok(symbol_typename(s)),
        Value::Str(s) => Ok(s.to_string()),
        Value::Nil => Err(reflect_nil_npe()),
        _ => Err(crate::core::cce_err(v, "java.lang.String")),
    }
}

/// The `typename` of a symbol: its `str` text with `<>` restored to
/// `[]` (upstream `TypeReference` on symbols).
fn symbol_typename(s: &Symbol) -> String {
    let text = match s.namespace() {
        Some(ns) => format!("{ns}/{}", s.name()),
        None => s.name().to_owned(),
    };
    text.replace("<>", "[]")
}

/// The nil-typeref NPE (port-pinned house text — upstream rejects
/// nil through the protocol, so this fires only on direct
/// hidden-native calls).
fn reflect_nil_npe() -> NativeError {
    NativeError::new(
        ThrowClass::NullPointerException,
        "Cannot invoke \"clojure.reflect/do-reflect\" because \"typeref\" is null".to_owned(),
    )
}

/// The raw reflection map: `:bases` (nil when empty, like upstream's
/// `not-empty`), `:flags-raw`, and `:members` (plain maps — the
/// facsimile swaps `:flags-raw` for the parsed `:flags`).
fn reflect_map(class: &Arc<ClassVal>) -> Value {
    Value::Map(
        new_map()
            .insert(kw("bases"), bases_value(class))
            .insert(kw("flags-raw"), Value::Int(class_modifiers(class)))
            .insert(kw("members"), members_value(class)),
    )
}

/// The `:bases` set (nil when empty).
fn bases_value(class: &Arc<ClassVal>) -> Value {
    if class.bases().is_empty() {
        Value::Nil
    } else {
        let mut out = new_set();
        for base in class.bases() {
            out = out.insert(sym(base));
        }
        Value::Set(out)
    }
}

/// The emulated class modifiers: every lattice class is public;
/// interfaces read `abstract` + `interface`, like `javac` output.
fn class_modifiers(class: &Arc<ClassVal>) -> i64 {
    if class.is_iface() {
        MOD_PUBLIC | MOD_ABSTRACT | MOD_INTERFACE
    } else {
        MOD_PUBLIC
    }
}

/// The `:members` set (empty for classes without emulated members).
fn members_value(class: &Arc<ClassVal>) -> Value {
    let mut out = new_set();
    if let Some(members) = class_members(class.name()) {
        for m in members {
            out = out.insert(member_map(class, m));
        }
    }
    Value::Set(out)
}

/// One member as a plain map (upstream keys plus `:flags-raw`;
/// `:exception-types` is always empty — no emulated member declares
/// checked exceptions).
fn member_map(class: &Arc<ClassVal>, m: &Member) -> Value {
    let mut out = new_map()
        .insert(kw("name"), sym(&member_name(class, m)))
        .insert(kw("declaring-class"), sym(class.name()))
        .insert(kw("parameter-types"), sym_vec(m.params))
        .insert(kw("exception-types"), sym_vec(&[]));
    if m.kind != MemberKind::Ctor {
        out = out.insert(kw("return-type"), sym(m.ret));
    }
    Value::Map(out.insert(
        kw("flags-raw"),
        Value::Int(member_modifiers(class.is_iface(), m.kind)),
    ))
}

/// The member name: ctors render as the binary name (upstream
/// `Constructor.getName`), everything else as declared.
fn member_name(class: &Arc<ClassVal>, m: &Member) -> String {
    if m.kind == MemberKind::Ctor {
        class.name().to_owned()
    } else {
        m.name.to_owned()
    }
}

/// The emulated member modifiers: public throughout, `abstract` on
/// interface methods, `static` on statics.
fn member_modifiers(iface: bool, kind: MemberKind) -> i64 {
    match kind {
        MemberKind::Ctor => MOD_PUBLIC,
        MemberKind::Method if iface => MOD_PUBLIC | MOD_ABSTRACT,
        MemberKind::Method => MOD_PUBLIC,
        MemberKind::Static => MOD_PUBLIC | MOD_STATIC,
    }
}

/// An unqualified keyword value.
fn kw(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

/// A `typesym` value (the table already carries `<>` spellings).
fn sym(name: &str) -> Value {
    Value::Symbol(Symbol::new(None, name))
}

/// A vector of `typesym` values.
fn sym_vec(names: &[&str]) -> Value {
    Value::Vector(Vector::from_vec(names.iter().map(|n| sym(n)).collect()))
}
