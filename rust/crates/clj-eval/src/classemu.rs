// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Class emulation (S16, issue #60): `Class` values over the emulated
//! JVM lattice — `instance?`/`type`/`class`/`bases`/`supers`/
//! `class?`/`cast`/`bean`/`identical?`, the `import`/`..`/`memfn`
//! macros (defined in `core.clj`, backed by the hidden `__import*`
//! native here), and the `..` member surface consulted by `__meth*`
//! before the host dispatch.
//!
//! The lattice is one static table (transcribed verbatim from
//! `docs/zero-oos-s16-classemu/probes/lattice.out`, plus the five
//! documented synthetic rows) with four dynamic rules: the HostDef
//! rule (host types and their claimed names), the fn rule (closures
//! by rest-presence, natives unconditionally `AFunction`), the future
//! / promise / iteration stable names (static reify rows), and the
//! struct carve-outs (`PersistentStructMap` / `$Def`). Walks recurse
//! through the registry, skip names that no longer resolve (the
//! redefinition leniency), and carry a visited set (redefinition can
//! link `HostDef`s cyclically).

use std::collections::HashSet;
use std::sync::Arc;

use clj_value::{
    ClassVal, HostDef, HostKind, HostVal, Keyword, List, Map, SeqKind, Symbol, ThrowClass, Value,
    Vector, new_map, new_set,
};

use crate::error::{NativeError, jvm_class_of, simple_name_of};
use crate::eval::Evaluator;
use crate::ext::{Closure, NativeFn};
use crate::ns::{NativeFnPtr, check_arity, signal_to_native};

/// The S16 natives: class queries, `bean`, and `identical?`
/// (`PrintWriter-on` lives with the writers in [`crate::io`]).
pub(crate) const CLASS_EMU_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("instance?", native_instance),
    ("type", native_type),
    ("class", native_class),
    ("bases", native_bases),
    ("supers", native_supers),
    ("class?", native_classp),
    ("cast", native_cast),
    ("bean", native_bean),
    ("identical?", native_identical),
];

/// Hidden natives: `__import*` is an implementation detail of the
/// `import` macro in the standing of `__new*` (public so macro
/// expansions analyze, unlisted in the matrix).
pub(crate) const CLASS_EMU_HIDDEN: &[(&str, NativeFnPtr)] = &[("__import*", native_import_star)];

/// One static lattice row: binary name, interface flag, and the
/// ordered direct-supertype names, transcribed verbatim from the
/// lattice probe output (the 214 static rows; the 13 dynamic rows
/// live in the construction rules below).
struct StaticRow {
    name: &'static str,
    iface: bool,
    bases: &'static [&'static str],
}

/// The static lattice: the 214 oracle rows plus the documented
/// synthetic rows (the future/promise/iteration reifies, the
/// nonexistent MissingFormatConversionException, and
/// NoSuchElementException for the S16 getFirst/getLast variant).
const STATIC_LATTICE: &[StaticRow] = &[
    StaticRow {
        name: "clojure.core.Eduction",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.lang.Iterable",
            "clojure.lang.IReduceInit",
            "clojure.lang.Sequential",
            "clojure.lang.IType",
        ],
    },
    StaticRow {
        name: "clojure.lang.AFn",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.IFn"],
    },
    StaticRow {
        name: "clojure.lang.AFunction",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.IObj",
            "java.util.Comparator",
            "clojure.lang.Fn",
            "java.io.Serializable",
        ],
    },
    StaticRow {
        name: "clojure.lang.AMapEntry",
        iface: false,
        bases: &["clojure.lang.APersistentVector", "clojure.lang.IMapEntry"],
    },
    StaticRow {
        name: "clojure.lang.APersistentMap",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.IPersistentMap",
            "java.util.Map",
            "java.lang.Iterable",
            "java.io.Serializable",
            "clojure.lang.MapEquivalence",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.APersistentMap$KeySeq",
        iface: false,
        bases: &["clojure.lang.ASeq"],
    },
    StaticRow {
        name: "clojure.lang.APersistentMap$ValSeq",
        iface: false,
        bases: &["clojure.lang.ASeq"],
    },
    StaticRow {
        name: "clojure.lang.APersistentSet",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.IPersistentSet",
            "java.util.Collection",
            "java.util.Set",
            "java.io.Serializable",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.APersistentVector",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.IPersistentVector",
            "java.lang.Iterable",
            "java.util.List",
            "java.util.RandomAccess",
            "java.lang.Comparable",
            "java.io.Serializable",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.APersistentVector$RSeq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.Counted",
        ],
    },
    StaticRow {
        name: "clojure.lang.APersistentVector$Seq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.IReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.APersistentVector$SubVector",
        iface: false,
        bases: &[
            "clojure.lang.APersistentVector",
            "clojure.lang.IObj",
            "clojure.lang.IKVReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.ARef",
        iface: false,
        bases: &["clojure.lang.AReference", "clojure.lang.IRef"],
    },
    StaticRow {
        name: "clojure.lang.AReference",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.IReference"],
    },
    StaticRow {
        name: "clojure.lang.ASeq",
        iface: false,
        bases: &[
            "clojure.lang.Obj",
            "clojure.lang.ISeq",
            "clojure.lang.Sequential",
            "java.util.List",
            "java.io.Serializable",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.ATransientMap",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.ITransientMap",
            "clojure.lang.ITransientAssociative2",
        ],
    },
    StaticRow {
        name: "clojure.lang.ATransientSet",
        iface: false,
        bases: &["clojure.lang.AFn", "clojure.lang.ITransientSet"],
    },
    StaticRow {
        name: "clojure.lang.Agent",
        iface: false,
        bases: &["clojure.lang.ARef"],
    },
    StaticRow {
        name: "clojure.lang.ArityException",
        iface: false,
        bases: &["java.lang.IllegalArgumentException"],
    },
    StaticRow {
        name: "clojure.lang.ArrayChunk",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IChunk",
            "java.io.Serializable",
        ],
    },
    StaticRow {
        name: "clojure.lang.ArraySeq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.IReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.Associative",
        iface: true,
        bases: &["clojure.lang.IPersistentCollection", "clojure.lang.ILookup"],
    },
    StaticRow {
        name: "clojure.lang.Atom",
        iface: false,
        bases: &["clojure.lang.ARef", "clojure.lang.IAtom2"],
    },
    StaticRow {
        name: "clojure.lang.BigInt",
        iface: false,
        bases: &["java.lang.Number", "clojure.lang.IHashEq"],
    },
    StaticRow {
        name: "clojure.lang.ChunkBuffer",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.Counted"],
    },
    StaticRow {
        name: "clojure.lang.ChunkedCons",
        iface: false,
        bases: &["clojure.lang.ASeq", "clojure.lang.IChunkedSeq"],
    },
    StaticRow {
        name: "clojure.lang.Compiler$CompilerException",
        iface: false,
        bases: &["java.lang.RuntimeException", "clojure.lang.IExceptionInfo"],
    },
    StaticRow {
        name: "clojure.lang.Cons",
        iface: false,
        bases: &["clojure.lang.ASeq", "java.io.Serializable"],
    },
    StaticRow {
        name: "clojure.lang.Counted",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Cycle",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IReduce",
            "clojure.lang.IPending",
        ],
    },
    StaticRow {
        name: "clojure.lang.Delay",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IDeref",
            "clojure.lang.IPending",
        ],
    },
    StaticRow {
        name: "clojure.lang.ExceptionInfo",
        iface: false,
        bases: &["java.lang.RuntimeException", "clojure.lang.IExceptionInfo"],
    },
    StaticRow {
        name: "clojure.lang.Fn",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IAtom",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IAtom2",
        iface: true,
        bases: &["clojure.lang.IAtom"],
    },
    StaticRow {
        name: "clojure.lang.IBlockingDeref",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IChunk",
        iface: true,
        bases: &["clojure.lang.Indexed"],
    },
    StaticRow {
        name: "clojure.lang.IChunkedSeq",
        iface: true,
        bases: &["clojure.lang.ISeq", "clojure.lang.Sequential"],
    },
    StaticRow {
        name: "clojure.lang.IDeref",
        iface: true,
        bases: &[
            "java.util.function.Supplier",
            "java.util.function.BooleanSupplier",
            "java.util.function.IntSupplier",
            "java.util.function.LongSupplier",
            "java.util.function.DoubleSupplier",
        ],
    },
    StaticRow {
        name: "clojure.lang.IDrop",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IEditableCollection",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IExceptionInfo",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IFn",
        iface: true,
        bases: &["java.util.concurrent.Callable", "java.lang.Runnable"],
    },
    StaticRow {
        name: "clojure.lang.IHashEq",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IKVReduce",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IKeywordLookup",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.ILookup",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IMapEntry",
        iface: true,
        bases: &["java.util.Map$Entry"],
    },
    StaticRow {
        name: "clojure.lang.IMapIterable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IMeta",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IObj",
        iface: true,
        bases: &["clojure.lang.IMeta"],
    },
    StaticRow {
        name: "clojure.lang.IPending",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IPersistentCollection",
        iface: true,
        bases: &["clojure.lang.Seqable"],
    },
    StaticRow {
        name: "clojure.lang.IPersistentList",
        iface: true,
        bases: &["clojure.lang.Sequential", "clojure.lang.IPersistentStack"],
    },
    StaticRow {
        name: "clojure.lang.IPersistentMap",
        iface: true,
        bases: &[
            "java.lang.Iterable",
            "clojure.lang.Associative",
            "clojure.lang.Counted",
        ],
    },
    StaticRow {
        name: "clojure.lang.IPersistentSet",
        iface: true,
        bases: &[
            "clojure.lang.IPersistentCollection",
            "clojure.lang.Counted",
            "clojure.lang.ILookup",
        ],
    },
    StaticRow {
        name: "clojure.lang.IPersistentStack",
        iface: true,
        bases: &["clojure.lang.IPersistentCollection"],
    },
    StaticRow {
        name: "clojure.lang.IPersistentVector",
        iface: true,
        bases: &[
            "clojure.lang.Associative",
            "clojure.lang.Sequential",
            "clojure.lang.IPersistentStack",
            "clojure.lang.Reversible",
            "clojure.lang.Indexed",
        ],
    },
    StaticRow {
        name: "clojure.lang.IRecord",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IReduce",
        iface: true,
        bases: &["clojure.lang.IReduceInit"],
    },
    StaticRow {
        name: "clojure.lang.IReduceInit",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.IRef",
        iface: true,
        bases: &["clojure.lang.IDeref"],
    },
    StaticRow {
        name: "clojure.lang.IReference",
        iface: true,
        bases: &["clojure.lang.IMeta"],
    },
    StaticRow {
        name: "clojure.lang.ISeq",
        iface: true,
        bases: &["clojure.lang.IPersistentCollection"],
    },
    StaticRow {
        name: "clojure.lang.ITransientAssociative",
        iface: true,
        bases: &["clojure.lang.ITransientCollection", "clojure.lang.ILookup"],
    },
    StaticRow {
        name: "clojure.lang.ITransientAssociative2",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.ITransientCollection",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.ITransientMap",
        iface: true,
        bases: &["clojure.lang.ITransientAssociative", "clojure.lang.Counted"],
    },
    StaticRow {
        name: "clojure.lang.ITransientSet",
        iface: true,
        bases: &[
            "clojure.lang.ITransientCollection",
            "clojure.lang.Counted",
            "clojure.lang.ILookup",
        ],
    },
    StaticRow {
        name: "clojure.lang.ITransientVector",
        iface: true,
        bases: &["clojure.lang.ITransientAssociative", "clojure.lang.Indexed"],
    },
    StaticRow {
        name: "clojure.lang.IType",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Indexed",
        iface: true,
        bases: &["clojure.lang.Counted"],
    },
    StaticRow {
        name: "clojure.lang.IndexedSeq",
        iface: true,
        bases: &[
            "clojure.lang.ISeq",
            "clojure.lang.Sequential",
            "clojure.lang.Counted",
        ],
    },
    StaticRow {
        name: "clojure.lang.Iterate",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IReduce",
            "clojure.lang.IPending",
        ],
    },
    StaticRow {
        name: "clojure.lang.Keyword",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IFn",
            "java.lang.Comparable",
            "clojure.lang.Named",
            "java.io.Serializable",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.LazySeq",
        iface: false,
        bases: &[
            "clojure.lang.Obj",
            "clojure.lang.ISeq",
            "clojure.lang.Sequential",
            "java.util.List",
            "clojure.lang.IPending",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.LineNumberingPushbackReader",
        iface: false,
        bases: &["java.io.PushbackReader"],
    },
    StaticRow {
        name: "clojure.lang.LispReader$ReaderException",
        iface: false,
        bases: &["java.lang.RuntimeException", "clojure.lang.IExceptionInfo"],
    },
    StaticRow {
        name: "clojure.lang.LongRange",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.MapEntry",
        iface: false,
        bases: &["clojure.lang.AMapEntry"],
    },
    StaticRow {
        name: "clojure.lang.MapEquivalence",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.MultiFn",
        iface: false,
        bases: &["clojure.lang.AFn"],
    },
    StaticRow {
        name: "clojure.lang.Named",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Namespace",
        iface: false,
        bases: &["clojure.lang.AReference", "java.io.Serializable"],
    },
    StaticRow {
        name: "clojure.lang.Obj",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IObj",
            "java.io.Serializable",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentArrayMap",
        iface: false,
        bases: &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IObj",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IMapIterable",
            "clojure.lang.IKVReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentArrayMap$Seq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.Counted",
            "clojure.lang.IReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentArrayMap$TransientArrayMap",
        iface: false,
        bases: &["clojure.lang.ATransientMap"],
    },
    StaticRow {
        name: "clojure.lang.PersistentHashMap",
        iface: false,
        bases: &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IObj",
            "clojure.lang.IMapIterable",
            "clojure.lang.IKVReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentHashMap$ArrayNode$Seq",
        iface: false,
        bases: &["clojure.lang.ASeq"],
    },
    StaticRow {
        name: "clojure.lang.PersistentHashMap$TransientHashMap",
        iface: false,
        bases: &["clojure.lang.ATransientMap"],
    },
    StaticRow {
        name: "clojure.lang.PersistentHashSet",
        iface: false,
        bases: &[
            "clojure.lang.APersistentSet",
            "clojure.lang.IObj",
            "clojure.lang.IEditableCollection",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentHashSet$TransientHashSet",
        iface: false,
        bases: &["clojure.lang.ATransientSet"],
    },
    StaticRow {
        name: "clojure.lang.PersistentList",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IPersistentList",
            "clojure.lang.IReduce",
            "java.util.List",
            "clojure.lang.Counted",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentList$EmptyList",
        iface: false,
        bases: &[
            "clojure.lang.Obj",
            "clojure.lang.IPersistentList",
            "java.util.List",
            "clojure.lang.ISeq",
            "clojure.lang.Counted",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentStructMap",
        iface: false,
        bases: &["clojure.lang.APersistentMap", "clojure.lang.IObj"],
    },
    StaticRow {
        name: "clojure.lang.PersistentStructMap$1",
        iface: false,
        bases: &["clojure.lang.AFn"],
    },
    StaticRow {
        name: "clojure.lang.PersistentStructMap$Def",
        iface: false,
        bases: &["java.lang.Object", "java.io.Serializable"],
    },
    StaticRow {
        name: "clojure.lang.PersistentTreeMap",
        iface: false,
        bases: &[
            "clojure.lang.APersistentMap",
            "clojure.lang.IObj",
            "clojure.lang.Reversible",
            "clojure.lang.Sorted",
            "clojure.lang.IKVReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentTreeMap$Seq",
        iface: false,
        bases: &["clojure.lang.ASeq"],
    },
    StaticRow {
        name: "clojure.lang.PersistentTreeSet",
        iface: false,
        bases: &[
            "clojure.lang.APersistentSet",
            "clojure.lang.IObj",
            "clojure.lang.Reversible",
            "clojure.lang.Sorted",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentVector",
        iface: false,
        bases: &[
            "clojure.lang.APersistentVector",
            "clojure.lang.IObj",
            "clojure.lang.IEditableCollection",
            "clojure.lang.IReduce",
            "clojure.lang.IKVReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentVector$ChunkedSeq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.Counted",
            "clojure.lang.IReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.PersistentVector$TransientVector",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.ITransientVector",
            "clojure.lang.ITransientAssociative2",
            "clojure.lang.Counted",
        ],
    },
    StaticRow {
        name: "clojure.lang.Range",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IChunkedSeq",
            "clojure.lang.IReduce",
        ],
    },
    StaticRow {
        name: "clojure.lang.Ratio",
        iface: false,
        bases: &["java.lang.Number", "java.lang.Comparable"],
    },
    StaticRow {
        name: "clojure.lang.ReaderConditional",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.ILookup"],
    },
    StaticRow {
        name: "clojure.lang.Reduced",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.IDeref"],
    },
    StaticRow {
        name: "clojure.lang.Repeat",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IReduce",
            "clojure.lang.IDrop",
        ],
    },
    StaticRow {
        name: "clojure.lang.RestFn",
        iface: false,
        bases: &["clojure.lang.AFunction"],
    },
    StaticRow {
        name: "clojure.lang.Reversible",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Seqable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Sequential",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Settable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.Sorted",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "clojure.lang.StringSeq",
        iface: false,
        bases: &[
            "clojure.lang.ASeq",
            "clojure.lang.IndexedSeq",
            "clojure.lang.IDrop",
            "clojure.lang.IReduceInit",
        ],
    },
    StaticRow {
        name: "clojure.lang.Symbol",
        iface: false,
        bases: &[
            "clojure.lang.AFn",
            "clojure.lang.IObj",
            "java.lang.Comparable",
            "clojure.lang.Named",
            "java.io.Serializable",
            "clojure.lang.IHashEq",
        ],
    },
    StaticRow {
        name: "clojure.lang.TaggedLiteral",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.ILookup"],
    },
    StaticRow {
        name: "clojure.lang.Var",
        iface: false,
        bases: &[
            "clojure.lang.ARef",
            "clojure.lang.IFn",
            "clojure.lang.IRef",
            "clojure.lang.Settable",
            "java.io.Serializable",
        ],
    },
    StaticRow {
        name: "clojure.lang.Var$Unbound",
        iface: false,
        bases: &["clojure.lang.AFn"],
    },
    StaticRow {
        name: "clojure.lang.Volatile",
        iface: false,
        bases: &["java.lang.Object", "clojure.lang.IDeref"],
    },
    StaticRow {
        name: "java.io.Closeable",
        iface: true,
        bases: &["java.lang.AutoCloseable"],
    },
    StaticRow {
        name: "java.io.FileNotFoundException",
        iface: false,
        bases: &["java.io.IOException"],
    },
    StaticRow {
        name: "java.io.FilterReader",
        iface: false,
        bases: &["java.io.Reader"],
    },
    StaticRow {
        name: "java.io.Flushable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.io.IOException",
        iface: false,
        bases: &["java.lang.Exception"],
    },
    StaticRow {
        name: "java.io.OutputStreamWriter",
        iface: false,
        bases: &["java.io.Writer"],
    },
    StaticRow {
        name: "java.io.PrintWriter",
        iface: false,
        bases: &["java.io.Writer"],
    },
    StaticRow {
        name: "java.io.PushbackReader",
        iface: false,
        bases: &["java.io.FilterReader"],
    },
    StaticRow {
        name: "java.io.Reader",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.lang.Readable",
            "java.io.Closeable",
        ],
    },
    StaticRow {
        name: "java.io.Serializable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.io.StringWriter",
        iface: false,
        bases: &["java.io.Writer"],
    },
    StaticRow {
        name: "java.io.Writer",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.lang.Appendable",
            "java.io.Closeable",
            "java.io.Flushable",
        ],
    },
    StaticRow {
        name: "java.lang.Appendable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.ArithmeticException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.ArrayIndexOutOfBoundsException",
        iface: false,
        bases: &["java.lang.IndexOutOfBoundsException"],
    },
    StaticRow {
        name: "java.lang.AssertionError",
        iface: false,
        bases: &["java.lang.Error"],
    },
    StaticRow {
        name: "java.lang.AutoCloseable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.Boolean",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
        ],
    },
    StaticRow {
        name: "java.lang.CharSequence",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.Character",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
        ],
    },
    StaticRow {
        name: "java.lang.Class",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.reflect.GenericDeclaration",
            "java.lang.reflect.Type",
            "java.lang.reflect.AnnotatedElement",
            "java.lang.invoke.TypeDescriptor$OfField",
            "java.lang.constant.Constable",
        ],
    },
    StaticRow {
        name: "java.lang.ClassCastException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.ClassNotFoundException",
        iface: false,
        bases: &["java.lang.ReflectiveOperationException"],
    },
    StaticRow {
        name: "java.lang.Cloneable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.Comparable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.Double",
        iface: false,
        bases: &[
            "java.lang.Number",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    },
    StaticRow {
        name: "java.lang.Error",
        iface: false,
        bases: &["java.lang.Throwable"],
    },
    StaticRow {
        name: "java.lang.Exception",
        iface: false,
        bases: &["java.lang.Throwable"],
    },
    StaticRow {
        name: "java.lang.IllegalAccessError",
        iface: false,
        bases: &["java.lang.IncompatibleClassChangeError"],
    },
    StaticRow {
        name: "java.lang.IllegalArgumentException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.IllegalStateException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.IncompatibleClassChangeError",
        iface: false,
        bases: &["java.lang.LinkageError"],
    },
    StaticRow {
        name: "java.lang.IndexOutOfBoundsException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.Iterable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.LinkageError",
        iface: false,
        bases: &["java.lang.Error"],
    },
    StaticRow {
        name: "java.lang.Long",
        iface: false,
        bases: &[
            "java.lang.Number",
            "java.lang.Comparable",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    },
    StaticRow {
        name: "java.lang.NegativeArraySizeException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.NullPointerException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.Number",
        iface: false,
        bases: &["java.lang.Object", "java.io.Serializable"],
    },
    StaticRow {
        name: "java.lang.NumberFormatException",
        iface: false,
        bases: &["java.lang.IllegalArgumentException"],
    },
    StaticRow {
        name: "java.lang.Object",
        iface: false,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.Readable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.ReflectiveOperationException",
        iface: false,
        bases: &["java.lang.Exception"],
    },
    StaticRow {
        name: "java.lang.Runnable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.RuntimeException",
        iface: false,
        bases: &["java.lang.Exception"],
    },
    StaticRow {
        name: "java.lang.String",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Comparable",
            "java.lang.CharSequence",
            "java.lang.constant.Constable",
            "java.lang.constant.ConstantDesc",
        ],
    },
    StaticRow {
        name: "java.lang.StringIndexOutOfBoundsException",
        iface: false,
        bases: &["java.lang.IndexOutOfBoundsException"],
    },
    StaticRow {
        name: "java.lang.Throwable",
        iface: false,
        bases: &["java.lang.Object", "java.io.Serializable"],
    },
    StaticRow {
        name: "java.lang.UnsupportedOperationException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.lang.constant.Constable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.constant.ConstantDesc",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.invoke.TypeDescriptor",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.invoke.TypeDescriptor$OfField",
        iface: true,
        bases: &["java.lang.invoke.TypeDescriptor"],
    },
    StaticRow {
        name: "java.lang.reflect.AnnotatedElement",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.lang.reflect.GenericDeclaration",
        iface: true,
        bases: &["java.lang.reflect.AnnotatedElement"],
    },
    StaticRow {
        name: "java.lang.reflect.Type",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.math.BigDecimal",
        iface: false,
        bases: &["java.lang.Number", "java.lang.Comparable"],
    },
    StaticRow {
        name: "java.math.BigInteger",
        iface: false,
        bases: &["java.lang.Number", "java.lang.Comparable"],
    },
    StaticRow {
        name: "java.sql.Date",
        iface: false,
        bases: &["java.util.Date"],
    },
    StaticRow {
        name: "java.util.Calendar",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Cloneable",
            "java.lang.Comparable",
        ],
    },
    StaticRow {
        name: "java.util.Collection",
        iface: true,
        bases: &["java.lang.Iterable"],
    },
    StaticRow {
        name: "java.util.Comparator",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.Date",
        iface: false,
        bases: &[
            "java.lang.Object",
            "java.io.Serializable",
            "java.lang.Cloneable",
            "java.lang.Comparable",
        ],
    },
    StaticRow {
        name: "java.util.DuplicateFormatFlagsException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.FormatFlagsConversionMismatchException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatArgumentIndexException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatCodePointException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatConversionException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatException",
        iface: false,
        bases: &["java.lang.IllegalArgumentException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatFlagsException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatPrecisionException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.IllegalFormatWidthException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.List",
        iface: true,
        bases: &["java.util.SequencedCollection"],
    },
    StaticRow {
        name: "java.util.Map",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.Map$Entry",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.MissingFormatArgumentException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.MissingFormatWidthException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.RandomAccess",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.SequencedCollection",
        iface: true,
        bases: &["java.util.Collection"],
    },
    StaticRow {
        name: "java.util.Set",
        iface: true,
        bases: &["java.util.Collection"],
    },
    StaticRow {
        name: "java.util.UnknownFormatConversionException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    StaticRow {
        name: "java.util.concurrent.Callable",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.concurrent.CancellationException",
        iface: false,
        bases: &["java.lang.IllegalStateException"],
    },
    StaticRow {
        name: "java.util.concurrent.ExecutionException",
        iface: false,
        bases: &["java.lang.Exception"],
    },
    StaticRow {
        name: "java.util.concurrent.Future",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.concurrent.RejectedExecutionException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
    StaticRow {
        name: "java.util.function.BooleanSupplier",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.function.DoubleSupplier",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.function.IntSupplier",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.function.LongSupplier",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.function.Supplier",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.regex.MatchResult",
        iface: true,
        bases: &[],
    },
    StaticRow {
        name: "java.util.regex.Matcher",
        iface: false,
        bases: &["java.lang.Object", "java.util.regex.MatchResult"],
    },
    StaticRow {
        name: "java.util.regex.Pattern",
        iface: false,
        bases: &["java.lang.Object", "java.io.Serializable"],
    },
    StaticRow {
        name: "java.util.regex.PatternSyntaxException",
        iface: false,
        bases: &["java.lang.IllegalArgumentException"],
    },
    // Synthetic: future reify (JVM ancestry copied from the l-future probe row).
    StaticRow {
        name: "clojure.core$future_call$reify",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IPending",
            "clojure.lang.IBlockingDeref",
            "java.util.concurrent.Future",
            "clojure.lang.IDeref",
            "clojure.lang.IObj",
        ],
    },
    // Synthetic: promise reify (JVM ancestry copied from the l-promise probe row).
    StaticRow {
        name: "clojure.core$promise$reify",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IPending",
            "clojure.lang.IBlockingDeref",
            "clojure.lang.IDeref",
            "clojure.lang.IFn",
            "clojure.lang.IObj",
        ],
    },
    // Synthetic: iteration (JVM ancestry copied from the l-iteration probe row).
    StaticRow {
        name: "clojure.core$iteration",
        iface: false,
        bases: &[
            "java.lang.Object",
            "clojure.lang.IReduceInit",
            "clojure.lang.Seqable",
            "clojure.lang.IObj",
        ],
    },
    // Synthetic: the nonexistent MissingFormatConversionException (throw-chain shape).
    StaticRow {
        name: "java.util.MissingFormatConversionException",
        iface: false,
        bases: &["java.util.IllegalFormatException"],
    },
    // Synthetic: NoSuchElementException (live-JVM ancestry; the S16 getFirst/getLast variant).
    StaticRow {
        name: "java.util.NoSuchElementException",
        iface: false,
        bases: &["java.lang.RuntimeException"],
    },
];

/// A static lattice row by binary name.
fn static_row(name: &str) -> Option<&'static StaticRow> {
    STATIC_LATTICE.iter().find(|r| r.name == name)
}

/// A static lattice row as a class value.
fn static_class(name: &str) -> Option<Arc<ClassVal>> {
    static_row(name).map(|r| {
        Arc::new(ClassVal::new(
            Arc::from(r.name),
            r.iface,
            r.bases.iter().map(|b| Arc::from(*b)).collect(),
        ))
    })
}

/// A class value from parts (the dynamic rules).
fn dynamic_class(name: &str, iface: bool, bases: &[&str]) -> Arc<ClassVal> {
    Arc::new(ClassVal::new(
        Arc::from(name),
        iface,
        bases.iter().map(|b| Arc::from(*b)).collect(),
    ))
}

/// A static row, falling back to a bare `Object` child for names
/// with no row and no rule (totality: every name the port reports is
/// tabled, so the fallback only fires for foreign extensions —
/// unit-tested directly).
fn static_or_fallback(name: &str) -> Arc<ClassVal> {
    static_class(name).unwrap_or_else(|| dynamic_class(name, false, &["java.lang.Object"]))
}

/// The record implicits, in bases order (the JVM adds all eight to
/// every record class).
const RECORD_IMPLICITS: &[&str] = &[
    "clojure.lang.IRecord",
    "clojure.lang.IHashEq",
    "clojure.lang.IObj",
    "clojure.lang.ILookup",
    "clojure.lang.IKeywordLookup",
    "clojure.lang.IPersistentMap",
    "java.util.Map",
    "java.io.Serializable",
];

/// The deftype implicits: `IType` alone (deftypes carry no `IObj` —
/// they reject `with-meta`, like the JVM).
const TYPE_IMPLICITS: &[&str] = &["clojure.lang.IType"];

/// The kind implicits of the HostDef rule (interfaces and gen-class
/// carry none; structs never reach here — the carve-outs resolve
/// first — and share the empty arm to stay total).
fn implicit_bases(kind: HostKind) -> &'static [&'static str] {
    match kind {
        HostKind::Record => RECORD_IMPLICITS,
        HostKind::Type => TYPE_IMPLICITS,
        _ => &[],
    }
}

/// The HostDef rule: `[Object]` (absent for interfaces) plus the
/// claimed names verbatim plus the kind implicits.
fn hostdef_class(d: &HostDef) -> Arc<ClassVal> {
    let iface = d.kind == HostKind::Interface;
    let mut bases: Vec<Arc<str>> = Vec::new();
    if !iface {
        bases.push(Arc::from("java.lang.Object"));
    }
    bases.extend(d.interfaces.iter().cloned());
    bases.extend(implicit_bases(d.kind).iter().map(|n| Arc::from(*n)));
    Arc::new(ClassVal::new(d.name.clone(), iface, bases.into()))
}

/// The class value for a supertype name: a static hit, else the
/// HostDef rule by qualified name (struct names carve to
/// `PersistentStructMap$Def`), else `None` (callers skip — the
/// redefinition leniency).
fn class_for_name(eval: &Evaluator, name: &str) -> Option<Arc<ClassVal>> {
    if let Some(class) = static_class(name) {
        return Some(class);
    }
    let def = crate::types::hostdef_by_name(eval, name)?;
    if def.kind == HostKind::Struct {
        return static_class("clojure.lang.PersistentStructMap$Def");
    }
    Some(hostdef_class(&def))
}

/// The class of a host value: the struct carve-outs (instances
/// report `PersistentStructMap`, descriptors `$Def`), else the
/// HostDef rule off the definition itself.
fn host_class_of(h: &Arc<HostVal>) -> Arc<ClassVal> {
    match h.as_ref() {
        HostVal::Def(d) if d.kind == HostKind::Struct => {
            static_or_fallback("clojure.lang.PersistentStructMap$Def")
        }
        HostVal::Inst(i) if i.def.kind == HostKind::Struct => {
            static_or_fallback("clojure.lang.PersistentStructMap")
        }
        HostVal::Def(d) => hostdef_class(d),
        HostVal::Inst(i) => hostdef_class(&i.def),
    }
}

/// The class of an extension value: the fn rule for closures (any
/// rest arity → `RestFn`, else `AFunction`) and natives
/// (unconditionally `AFunction`), both under their own display
/// names; every other kind reports a tabled name.
fn extension_class_of(e: &Arc<dyn clj_value::ExtensionValue>) -> Arc<ClassVal> {
    let any = e.as_any();
    if let Some(c) = any.downcast_ref::<Closure>() {
        let rest = c.arities.iter().any(|a| a.rest.is_some());
        let base = if rest {
            "clojure.lang.RestFn"
        } else {
            "clojure.lang.AFunction"
        };
        return dynamic_class(&e.class_name(), false, &[base]);
    }
    if any.downcast_ref::<NativeFn>().is_some() {
        return dynamic_class(&e.class_name(), false, &["clojure.lang.AFunction"]);
    }
    static_or_fallback(&e.class_name())
}

/// The class of a value (`None` for nil): classes answer
/// `java.lang.Class`, host values fold through [`host_class_of`],
/// extensions through [`extension_class_of`], and everything else
/// reports its `jvm_class_of` name (tabled).
fn class_of_value(v: &Value) -> Option<Arc<ClassVal>> {
    match v {
        Value::Nil => None,
        Value::Class(_) => static_class("java.lang.Class"),
        Value::HostType(h) => Some(host_class_of(h)),
        Value::Extension(e) => Some(extension_class_of(e)),
        _ => Some(static_or_fallback(&jvm_class_of(v))),
    }
}

/// The class of a (non-nil) value as a value.
fn class_value_of(v: &Value) -> Value {
    class_of_value(v).map(Value::Class).unwrap_or(Value::Nil)
}

/// Whether `target` names the class of `x` or one of its transitive
/// supertypes (the walk recurses through the registry, skips names
/// that no longer resolve, and carries a visited set).
pub(crate) fn is_instance_of(eval: &Evaluator, target: &str, x: &Value) -> bool {
    let Some(class) = class_of_value(x) else {
        return false;
    };
    class.name() == target || closure_contains(eval, class.bases(), target)
}

/// Whether the transitive supertypes of `bases` contain `target`.
fn closure_contains(eval: &Evaluator, bases: &[Arc<str>], target: &str) -> bool {
    let mut visited: HashSet<Arc<str>> = HashSet::new();
    let mut stack: Vec<Arc<str>> = bases.to_vec();
    while let Some(name) = stack.pop() {
        if name.as_ref() == target {
            return true;
        }
        if !visited.insert(name.clone()) {
            continue;
        }
        if let Some(shape) = class_for_name(eval, &name) {
            stack.extend(shape.bases().iter().cloned());
        }
    }
    false
}

/// The transitive supertype names of `bases` (dangling names are
/// collected here and skipped at materialization).
fn closure_set(eval: &Evaluator, bases: &[Arc<str>]) -> HashSet<Arc<str>> {
    let mut out: HashSet<Arc<str>> = HashSet::new();
    let mut stack: Vec<Arc<str>> = bases.to_vec();
    while let Some(name) = stack.pop() {
        if !out.insert(name.clone()) {
            continue;
        }
        if let Some(shape) = class_for_name(eval, &name) {
            stack.extend(shape.bases().iter().cloned());
        }
    }
    out
}

/// A class-position argument: nil is the caller's NPE, classes pass
/// through, descriptors fold through the registry (struct
/// descriptors and instances are cast failures, like any other
/// non-class).
fn class_position(
    v: &Value,
    nil_msg: &'static str,
    eval: &Evaluator,
) -> Result<Arc<ClassVal>, NativeError> {
    match v {
        Value::Nil => Err(NativeError::new(
            ThrowClass::NullPointerException,
            nil_msg.to_owned(),
        )),
        Value::Class(c) => Ok(c.clone()),
        Value::HostType(h) => fold_descriptor(h, v, eval),
        _ => Err(crate::core::cce_err(v, "java.lang.Class")),
    }
}

/// Fold a descriptor in class position to its registry shape (a
/// folded name with no registry entry is the cast failure, like a
/// struct descriptor or an instance).
fn fold_descriptor(
    h: &Arc<HostVal>,
    v: &Value,
    eval: &Evaluator,
) -> Result<Arc<ClassVal>, NativeError> {
    match h.as_ref() {
        HostVal::Def(d) if d.kind != HostKind::Struct => {
            class_for_name(eval, &d.name).ok_or_else(|| crate::core::cce_err(v, "java.lang.Class"))
        }
        _ => Err(crate::core::cce_err(v, "java.lang.Class")),
    }
}

/// `instance?`: the class of `x` is `c` or has it as a supertype.
fn native_instance(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/instance?", args, &[2])?;
    let class = class_position(
        &args[0],
        "Cannot invoke \"java.lang.Class.isInstance(Object)\" because \"c\" is null",
        eval,
    )?;
    Ok(Value::Bool(is_instance_of(eval, class.name(), &args[1])))
}

/// Cast `x` to `c`: nil passes through, members pass through, and
/// anything else is the short-shape miss.
fn cast_to_class(eval: &Evaluator, c: &Arc<ClassVal>, x: &Value) -> Result<Value, NativeError> {
    if matches!(x, Value::Nil) {
        return Ok(x.clone());
    }
    if is_instance_of(eval, c.name(), x) {
        Ok(x.clone())
    } else {
        Err(NativeError::cast(&jvm_class_of(x), c.name()))
    }
}

/// `cast`: like the `Class.cast` member through [`cast_to_class`].
fn native_cast(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/cast", args, &[2])?;
    let class = class_position(
        &args[0],
        "Cannot invoke \"java.lang.Class.cast(Object)\" because \"c\" is null",
        eval,
    )?;
    cast_to_class(eval, &class, &args[1])
}

/// The `:type` metadata entry, when present.
fn meta_type(v: &Value) -> Option<Value> {
    let meta = v.meta()?;
    let probe = Value::Keyword(Keyword::new(None, "type"));
    meta.get(&probe).cloned()
}

/// `type`: a truthy `:type` entry wins, else the class.
fn native_type(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/type", args, &[1])?;
    if let Some(t) = meta_type(&args[0]) {
        if t.is_truthy() {
            return Ok(t);
        }
    }
    Ok(class_value_of(&args[0]))
}

/// `class`: the class of the value, nil for nil.
fn native_class(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/class", args, &[1])?;
    Ok(class_value_of(&args[0]))
}

/// A `bases`/`supers` argument: nil is nil (not the NPE — unlike
/// `instance?`/`cast`), classes pass through, descriptors fold.
fn bases_class_arg(v: &Value, eval: &Evaluator) -> Result<Option<Arc<ClassVal>>, NativeError> {
    match v {
        Value::Nil => Ok(None),
        Value::Class(c) => Ok(Some(c.clone())),
        Value::HostType(h) => Ok(Some(fold_descriptor(h, v, eval)?)),
        _ => Err(crate::core::cce_err(v, "java.lang.Class")),
    }
}

/// `bases`: the ordered direct supertypes as a seq, nil for empty
/// ancestry (dangling names skip — the redefinition leniency).
fn native_bases(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/bases", args, &[1])?;
    let Some(class) = bases_class_arg(&args[0], eval)? else {
        return Ok(Value::Nil);
    };
    if class.bases().is_empty() {
        return Ok(Value::Nil);
    }
    let mut items = Vec::new();
    for base in class.bases() {
        if let Some(c) = class_for_name(eval, base) {
            items.push(Value::Class(c));
        }
    }
    Ok(Value::List(List::from_vec(items)))
}

/// `supers`: the transitive supertypes as a set, nil for empty
/// ancestry (dangling names skip — the redefinition leniency).
fn native_supers(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/supers", args, &[1])?;
    let Some(class) = bases_class_arg(&args[0], eval)? else {
        return Ok(Value::Nil);
    };
    let names = closure_set(eval, class.bases());
    if names.is_empty() {
        return Ok(Value::Nil);
    }
    let mut set = new_set();
    for name in &names {
        if let Some(c) = class_for_name(eval, name) {
            set = set.insert(Value::Class(c));
        }
    }
    Ok(Value::Set(set))
}

/// `class?`: whether the value is a class.
fn native_classp(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/class?", args, &[1])?;
    Ok(Value::Bool(matches!(&args[0], Value::Class(_))))
}

/// `identical?`: collections compare minted ids, extensions / host
/// values / multimethods compare allocations, classes compare names,
/// and every other same-kind pair compares like `=` (cross-kind
/// pairs are never identical — `1` and `1N` are `=` but distinct
/// boxes).
pub(crate) fn identical_values(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::List(x), Value::List(y)) => x.id() == y.id(),
        (Value::Vector(x), Value::Vector(y)) => x.id() == y.id(),
        (Value::Map(x), Value::Map(y)) => x.id() == y.id(),
        (Value::Set(x), Value::Set(y)) => x.id() == y.id(),
        (Value::Extension(x), Value::Extension(y)) => Arc::ptr_eq(x, y),
        (Value::HostType(x), Value::HostType(y)) => Arc::ptr_eq(x, y),
        (Value::MultiFn(x), Value::MultiFn(y)) => Arc::ptr_eq(x, y),
        (Value::Class(x), Value::Class(y)) => x.name() == y.name(),
        _ => std::mem::discriminant(a) == std::mem::discriminant(b) && a == b,
    }
}

/// `identical?` (the analyzer's direct-call rewrite lowers to this
/// same var, so the two cannot drift).
fn native_identical(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/identical?", args, &[2])?;
    Ok(Value::Bool(identical_values(&args[0], &args[1])))
}

/// Resolve an import name: a static hit, else a `HostDef` of that
/// qualified name (struct names carve to
/// `PersistentStructMap$Def`), else unresolvable.
fn resolve_import(eval: &Evaluator, fqn: &str) -> Option<Arc<ClassVal>> {
    if let Some(class) = static_class(fqn) {
        return Some(class);
    }
    let def = crate::types::hostdef_by_name(eval, fqn)?;
    if def.kind == HostKind::Struct {
        return static_class("clojure.lang.PersistentStructMap$Def");
    }
    Some(hostdef_class(&def))
}

/// Whether a var root is the descriptor of the imported name itself
/// (importing a type in its defining namespace no-ops instead of
/// conflicting).
fn def_of_same_name(root: &Value, fqn: &str) -> bool {
    match root {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => d.name.as_ref() == fqn,
            HostVal::Inst(_) => false,
        },
        _ => false,
    }
}

/// `__import*`: import one fully-qualified name into the current
/// namespace, returning the class. Unknown names are
/// `ClassNotFoundException` (bare name); a var mapping (def or
/// refer) is the `Var` → `Class` cast failure, except the same-name
/// descriptor leniency (no-op success); an import of the same class
/// is a no-op, and of a different class the `already refers to`
/// state error.
fn native_import_star(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/__import*", args, &[1])?;
    let Value::Str(fqn) = &args[0] else {
        return Err(crate::core::cce_err(&args[0], "java.lang.String"));
    };
    import_class(eval, fqn)
}

/// Import `fqn` into the current namespace (split for the complexity
/// budget).
fn import_class(eval: &mut Evaluator, fqn: &str) -> Result<Value, NativeError> {
    let class = resolve_import(eval, fqn)
        .ok_or_else(|| NativeError::new(ThrowClass::ClassNotFoundException, fqn.to_owned()))?;
    let simple = fqn.rsplit('.').next().unwrap_or(fqn);
    let cur = eval.current_ns_name();
    let sym = Symbol::new(None, simple);
    if let Some(target) = eval.registry.resolve_in(&cur, &sym)
        && let Some(id) = eval.registry.lookup(&target)
    {
        let root = eval.deref_var(id, &target);
        if def_of_same_name(&root, fqn) {
            return Ok(Value::Class(class));
        }
        let var = crate::ns::var_value(id, &target);
        return Err(crate::core::cce_err(&var, "java.lang.Class"));
    }
    if let Some(existing) = import_in(eval, &cur, simple) {
        if existing.name() == class.name() {
            return Ok(Value::Class(class));
        }
        return Err(NativeError::new(
            ThrowClass::IllegalStateException,
            format!(
                "{simple} already refers to: {} in namespace: {cur}",
                clj_value::class_java_debug(&existing),
            ),
        ));
    }
    eval.registry
        .with_ns_mut(&cur, |ns| ns.import_class(simple, class.clone()));
    Ok(Value::Class(class))
}

/// Read an import mapping (shared with name resolution).
pub(crate) fn import_in(eval: &Evaluator, ns: &str, simple: &str) -> Option<Arc<ClassVal>> {
    eval.registry
        .with_ns_mut(ns, |ns| ns.get_import(simple))
        .flatten()
}

/// A bean key.
fn kw(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

/// The throwable message as a value (nil when absent).
fn throwable_message(t: &clj_value::Throwable) -> Value {
    t.message.clone().map(Value::Str).unwrap_or(Value::Nil)
}

/// The throwable cause as a value (nil when absent).
fn throwable_cause(t: &clj_value::Throwable) -> Value {
    t.cause
        .clone()
        .map(|c| Value::Throwable(*c))
        .unwrap_or(Value::Nil)
}

/// The superclass name (interfaces and baseless classes have none).
fn superclass_name(c: &Arc<ClassVal>) -> Option<&str> {
    if c.is_iface() || c.bases().is_empty() {
        None
    } else {
        Some(c.bases()[0].as_ref())
    }
}

/// The superclass as a value (nil when absent or dangling).
fn superclass_value(eval: &Evaluator, c: &Arc<ClassVal>) -> Value {
    superclass_name(c)
        .and_then(|n| class_for_name(eval, n))
        .map(Value::Class)
        .unwrap_or(Value::Nil)
}

/// The interfaces as a vector in lattice order (dangling names skip).
fn interfaces_vector(eval: &Evaluator, c: &Arc<ClassVal>) -> Value {
    let rest = if c.is_iface() || c.bases().is_empty() {
        c.bases()
    } else {
        &c.bases()[1..]
    };
    let mut items = Vec::new();
    for base in rest {
        if let Some(ic) = class_for_name(eval, base) {
            items.push(Value::Class(ic));
        }
    }
    Value::Vector(Vector::from_vec(items))
}

/// The class keys of `bean` (`:class` itself is pushed by the caller).
fn class_bean_keys(eval: &Evaluator, c: &Arc<ClassVal>, out: &mut Vec<(Value, Value)>) {
    out.push((kw("array"), Value::Bool(false)));
    out.push((kw("interface"), Value::Bool(c.is_iface())));
    out.push((kw("interfaces"), interfaces_vector(eval, c)));
    out.push((kw("name"), Value::Str(Arc::from(c.name()))));
    out.push((
        kw("simpleName"),
        Value::Str(Arc::from(simple_name_of(c.name()).as_str())),
    ));
    out.push((kw("superclass"), superclass_value(eval, c)));
}

/// The throwable keys of `bean` (`:data` for `ExceptionInfo` only).
fn throwable_bean_keys(t: &clj_value::Throwable, out: &mut Vec<(Value, Value)>) {
    out.push((kw("cause"), throwable_cause(t)));
    out.push((kw("localizedMessage"), throwable_message(t)));
    out.push((kw("message"), throwable_message(t)));
    if t.class == ThrowClass::ExceptionInfo {
        out.push((kw("data"), t.data.clone().map(|d| *d).unwrap_or(Value::Nil)));
    }
}

/// Whether a host method name is a bean getter (`get`/`is` plus an
/// uppercase letter, like the JVM's property reflection).
fn bean_suffix(name: &str) -> Option<&str> {
    let rest = name
        .strip_prefix("get")
        .or_else(|| name.strip_prefix("is"))?;
    let first = rest.chars().next()?;
    if first.is_ascii_uppercase() {
        Some(rest)
    } else {
        None
    }
}

/// JavaBeans decapitalization: a leading all-caps run stays (`URL`
/// → `URL`), else the first letter lowercases (`Count` → `count`).
fn decapitalize(s: &str) -> String {
    let mut chars = s.chars();
    match (chars.next(), chars.next()) {
        (Some(first), Some(second))
            if first.is_ascii_uppercase() && second.is_ascii_uppercase() =>
        {
            s.to_owned()
        }
        (Some(first), _) => first.to_lowercase().collect::<String>() + &s[first.len_utf8()..],
        (None, _) => s.to_owned(),
    }
}

/// The host-method keys of `bean`: one eager 0-arg call per
/// getter-named method (sorted for determinism; throwing getters
/// fail `bean` itself).
fn host_method_bean_keys(
    eval: &mut Evaluator,
    h: &Arc<HostVal>,
    target: &Value,
    out: &mut Vec<(Value, Value)>,
) -> Result<(), NativeError> {
    let HostVal::Inst(i) = h.as_ref() else {
        return Ok(());
    };
    let mut getters: Vec<(&str, &str)> = i
        .def
        .methods
        .iter()
        .filter_map(|(name, overloads)| {
            let suffix = bean_suffix(name.as_ref())?;
            overloads
                .iter()
                .any(|o| o.arity == 0)
                .then_some((name.as_ref(), suffix))
        })
        .collect();
    getters.sort_unstable();
    for (name, suffix) in getters {
        let val = crate::types::call_zero_arg_method(eval, h, target.clone(), name)?;
        out.push((kw(&decapitalize(suffix)), val));
    }
    Ok(())
}

/// The host keys of `bean`: `:empty` for records and structs, plus
/// the 0-arg getter methods of instances (descriptors carry `:class`
/// alone).
fn host_bean_keys(
    eval: &mut Evaluator,
    h: &Arc<HostVal>,
    target: &Value,
    out: &mut Vec<(Value, Value)>,
) -> Result<(), NativeError> {
    if let HostVal::Inst(i) = h.as_ref() {
        if matches!(i.def.kind, HostKind::Record | HostKind::Struct) {
            out.push((kw("empty"), Value::Bool(i.content.is_empty())));
        }
    }
    host_method_bean_keys(eval, h, target, out)
}

/// The gate keys of `bean`: `:empty` for strings and
/// Collection/Map members, `:first`/`:last` for List members,
/// `:realized` for IPending members.
fn collection_bean_keys(
    eval: &mut Evaluator,
    v: &Value,
    out: &mut Vec<(Value, Value)>,
) -> Result<(), NativeError> {
    if matches!(v, Value::Str(_))
        || is_instance_of(eval, "java.util.Collection", v)
        || is_instance_of(eval, "java.util.Map", v)
    {
        out.push((kw("empty"), Value::Bool(surface_is_empty(eval, v)?)));
    }
    if is_instance_of(eval, "java.util.List", v) {
        out.push((kw("first"), surface_first(eval, v)?));
        out.push((kw("last"), surface_last(eval, v)?));
    }
    if is_instance_of(eval, "clojure.lang.IPending", v) {
        out.push((kw("realized"), Value::Bool(realized_flag(v))));
    }
    Ok(())
}

/// The non-`:class` keys of `bean` (split for the complexity
/// budget): classes, throwables, host values, and named values take
/// their fixed keys; everything else takes the gate keys (`:blank`
/// for strings, `:key`/`:value` for entries, and the
/// Collection/List/Pending gates).
fn bean_gated_keys(
    eval: &mut Evaluator,
    v: &Value,
    out: &mut Vec<(Value, Value)>,
) -> Result<(), NativeError> {
    match v {
        Value::Class(c) => {
            class_bean_keys(eval, c, out);
            Ok(())
        }
        Value::Throwable(t) => {
            throwable_bean_keys(t, out);
            Ok(())
        }
        Value::HostType(h) => host_bean_keys(eval, h, v, out),
        Value::Keyword(k) => {
            named_bean_keys(k.name(), k.namespace(), out);
            Ok(())
        }
        Value::Symbol(s) => {
            named_bean_keys(s.name(), s.namespace(), out);
            Ok(())
        }
        _ => {
            if let Value::Str(s) = v {
                out.push((kw("blank"), Value::Bool(is_blank(s))));
            }
            collection_bean_keys(eval, v, out)?;
            if let Value::MapEntry { key, val } = v {
                out.push((kw("key"), key.as_ref().clone()));
                out.push((kw("value"), val.as_ref().clone()));
            }
            Ok(())
        }
    }
}

/// The named keys of `bean` (all three keys, always — the namespace
/// is nil when absent).
fn named_bean_keys(name: &str, namespace: Option<&str>, out: &mut Vec<(Value, Value)>) {
    out.push((kw("name"), Value::Str(Arc::from(name))));
    out.push((
        kw("namespace"),
        namespace
            .map(|n| Value::Str(Arc::from(n)))
            .unwrap_or(Value::Nil),
    ));
}

/// `bean`: an eager plain map of the value's getters — `:class`
/// always, plus the kind's fixed keys (nil is the `getClass` NPE,
/// like the JVM).
fn native_bean(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core/bean", args, &[1])?;
    let v = &args[0];
    if matches!(v, Value::Nil) {
        return Err(crate::core::npe_getclass());
    }
    let mut entries: Vec<(Value, Value)> = vec![(kw("class"), class_value_of(v))];
    bean_gated_keys(eval, v, &mut entries)?;
    let mut map = new_map();
    for (k, val) in entries {
        map = map.insert(k, val);
    }
    Ok(Value::Map(map))
}

/// Whether either side of `equals` is a numeric-tower value (the
/// Java-exact rule covers `Long`/`Double`/`BigInt`/`BigDec`/`Ratio`).
fn is_tower_value(v: &Value) -> bool {
    matches!(
        v,
        Value::Int(_) | Value::Double(_) | Value::BigInt(_) | Value::BigDec(_) | Value::Ratio(_)
    )
}

/// `equals` on a numeric-tower pair (Java-exact): same classes
/// compare (`double` by bits — NaN equals NaN, signed zeros differ;
/// `BigDec` scale-sensitively), cross classes never do.
fn tower_equals(a: &Value, b: &Value) -> bool {
    match (a, b) {
        (Value::Int(x), Value::Int(y)) => x == y,
        (Value::Double(x), Value::Double(y)) => x.to_bits() == y.to_bits(),
        (Value::BigInt(x), Value::BigInt(y)) => x == y,
        (Value::BigDec(x), Value::BigDec(y)) => x == y,
        (Value::Ratio(x), Value::Ratio(y)) => x == y,
        _ => false,
    }
}

/// The map content for `equals` branch 2 (records and structs
/// compare contents, maps compare themselves).
fn record_content(v: &Value) -> Option<Map> {
    match v {
        Value::Map(m) => Some(m.clone()),
        Value::HostType(h) => match h.as_ref() {
            HostVal::Inst(i) if matches!(i.def.kind, HostKind::Record | HostKind::Struct) => {
                Some(i.content.clone())
            }
            _ => None,
        },
        _ => None,
    }
}

/// `equals`: the tower rule for numerics, the map-content rule when
/// either side is a record, else port `=` (realizing, like the
/// native — pending seqs compare by content).
fn equals_member(eval: &mut Evaluator, a: &Value, b: &Value) -> Result<bool, NativeError> {
    if is_tower_value(a) || is_tower_value(b) {
        return Ok(tower_equals(a, b));
    }
    if crate::types::is_host_record(a) || crate::types::is_host_record(b) {
        return Ok(match (record_content(a), record_content(b)) {
            (Some(x), Some(y)) => x == y,
            _ => false,
        });
    }
    crate::lazy::equiv_vals(eval, a, b).map_err(signal_to_native)
}

/// `hashCode`: Java `.hashCode` over the realized value (mirroring
/// the private `hash-code` native's realize-then-hash contract,
/// including the infinite-seq refusal).
fn hashcode_member(eval: &mut Evaluator, target: &Value) -> Result<Value, NativeError> {
    if clj_value::seq_hash_unsupported(target) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    let v = eval.realize_deep(target).map_err(signal_to_native)?;
    if clj_value::seq_hash_unsupported(&v) {
        return Err(NativeError::new(
            ThrowClass::UnsupportedOperationException,
            String::new(),
        ));
    }
    Ok(Value::Int(i64::from(clj_value::hash_code(&v))))
}

/// The empty-list `NoSuchElementException` (empty message, like the
/// JVM's nil).
fn nsee_empty() -> NativeError {
    NativeError::new(ThrowClass::NoSuchElementException, String::new())
}

/// `getFirst` by position (empty lists throw).
fn surface_first(eval: &mut Evaluator, v: &Value) -> Result<Value, NativeError> {
    let seq = crate::lazy::seq_value(eval, v).map_err(signal_to_native)?;
    if matches!(seq, Value::Nil) {
        return Err(nsee_empty());
    }
    crate::lazy::first_value(eval, &seq).map_err(signal_to_native)
}

/// `getLast` by position (empty lists throw).
fn surface_last(eval: &mut Evaluator, v: &Value) -> Result<Value, NativeError> {
    let seq = crate::lazy::seq_value(eval, v).map_err(signal_to_native)?;
    if matches!(seq, Value::Nil) {
        return Err(nsee_empty());
    }
    let mut cur = seq;
    loop {
        let nxt = crate::lazy::next_value(eval, &cur).map_err(signal_to_native)?;
        if matches!(nxt, Value::Nil) {
            return crate::lazy::first_value(eval, &cur).map_err(signal_to_native);
        }
        cur = nxt;
    }
}

/// `isEmpty` (mirroring `empty?`: counted kinds compare the count,
/// the rest check for a first element).
fn surface_is_empty(eval: &mut Evaluator, v: &Value) -> Result<bool, NativeError> {
    if crate::core::is_counted(v) {
        Ok(crate::coll::count_of(eval, v)? == 0)
    } else {
        let s = crate::lazy::seq_value(eval, v).map_err(signal_to_native)?;
        Ok(matches!(s, Value::Nil))
    }
}

/// `isBlank` (empty or all whitespace).
fn is_blank(s: &str) -> bool {
    s.chars().all(|c| c.is_whitespace())
}

/// The `isRealized` flag (the IPending gate admits only delays,
/// futures, promises, lazy seqs, and cycles/iterates).
fn realized_flag(v: &Value) -> bool {
    match v {
        Value::Delay(d) => d.is_realized(),
        Value::Promise(p) => p.is_realized(),
        Value::Future(f) => f.is_done(),
        Value::Extension(_) => extension_realized(v),
        _ => false,
    }
}

/// The `isRealized` flag for extensions: pending seqs report their
/// flag, realized Lazy-kind seqs are realized by construction, and
/// cycles/iterates report realized (the JVM's
/// `Cycle/Iterate.isRealized` is unconditionally true —
/// JVM-verified live; the only other IPending extensions).
fn extension_realized(v: &Value) -> bool {
    if let Some(p) = crate::lazy::as_pending(v) {
        return p.is_realized();
    }
    crate::lazy::as_realized(v).is_none_or(|r| r.kind() == SeqKind::Lazy)
}

/// The universal members (`toString` is `str`, `equals` the three
/// rules, `hashCode` Java's, `getClass` the class).
fn surface_universal(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    match (meth, rest.len()) {
        ("toString", 0) => {
            Some(crate::core::str_of_one(eval, target).map(|s| Value::Str(Arc::from(s.as_str()))))
        }
        ("equals", 1) => Some(equals_member(eval, target, &rest[0]).map(Value::Bool)),
        ("hashCode", 0) => Some(hashcode_member(eval, target)),
        ("getClass", 0) => Some(Ok(class_value_of(target))),
        _ => None,
    }
}

/// `isAssignableFrom`: whether the argument class has `c` as itself
/// or a supertype (nil is the empty-message NPE, non-classes the
/// short cast failure).
fn assignable_from(eval: &Evaluator, c: &Arc<ClassVal>, arg: &Value) -> Result<Value, NativeError> {
    let Value::Class(d) = arg else {
        if matches!(arg, Value::Nil) {
            return Err(NativeError::new(
                ThrowClass::NullPointerException,
                String::new(),
            ));
        }
        return Err(NativeError::cast(
            &crate::error::jvm_class_of(arg),
            "java.lang.Class",
        ));
    };
    Ok(Value::Bool(
        d.name() == c.name() || closure_contains(eval, d.bases(), c.name()),
    ))
}

/// The class members (classes only — anything else misses through).
fn surface_class_member(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    let Value::Class(c) = target else {
        return None;
    };
    match (meth, rest.len()) {
        ("getName", 0) => Some(Ok(Value::Str(Arc::from(c.name())))),
        ("getSimpleName", 0) => Some(Ok(Value::Str(Arc::from(simple_name_of(c.name()).as_str())))),
        ("getSuperclass", 0) => Some(Ok(superclass_value(eval, c))),
        ("getInterfaces", 0) => Some(Ok(interfaces_vector(eval, c))),
        ("isInterface", 0) => Some(Ok(Value::Bool(c.is_iface()))),
        ("isArray", 0) => Some(Ok(Value::Bool(false))),
        ("isInstance", 1) => Some(Ok(Value::Bool(is_instance_of(eval, c.name(), &rest[0])))),
        ("isAssignableFrom", 1) => Some(assignable_from(eval, c, &rest[0])),
        ("cast", 1) => Some(cast_to_class(eval, c, &rest[0])),
        // Classes miss method-shape at every arity (unlike other
        // values, whose 0-arg misses read field-first).
        _ => Some(Err(crate::types::no_method(
            meth,
            rest.len(),
            &crate::error::jvm_class_of(target),
        ))),
    }
}

/// `isEmpty`: strings and Collection/Map members.
fn gated_empty(eval: &mut Evaluator, target: &Value) -> Option<Result<Value, NativeError>> {
    if matches!(target, Value::Str(_))
        || is_instance_of(eval, "java.util.Collection", target)
        || is_instance_of(eval, "java.util.Map", target)
    {
        Some(surface_is_empty(eval, target).map(Value::Bool))
    } else {
        None
    }
}

/// `getName` on named values (classes take the class member above).
fn gated_named_name(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Keyword(k) => Some(Ok(Value::Str(Arc::from(k.name())))),
        Value::Symbol(s) => Some(Ok(Value::Str(Arc::from(s.name())))),
        _ => None,
    }
}

/// `getNamespace` on named values.
fn gated_named_ns(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Keyword(k) => Some(Ok(k
            .namespace()
            .map(|ns| Value::Str(Arc::from(ns)))
            .unwrap_or(Value::Nil))),
        Value::Symbol(s) => Some(Ok(s
            .namespace()
            .map(|ns| Value::Str(Arc::from(ns)))
            .unwrap_or(Value::Nil))),
        _ => None,
    }
}

/// `getFirst`/`getLast` on List members.
fn gated_list_end(
    eval: &mut Evaluator,
    target: &Value,
    first: bool,
) -> Option<Result<Value, NativeError>> {
    if is_instance_of(eval, "java.util.List", target) {
        Some(if first {
            surface_first(eval, target)
        } else {
            surface_last(eval, target)
        })
    } else {
        None
    }
}

/// `getData` on `ExceptionInfo` (other throwables miss).
fn gated_exinfo_data(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Throwable(t) if t.class == ThrowClass::ExceptionInfo => {
            Some(Ok(t.data.clone().map(|d| *d).unwrap_or(Value::Nil)))
        }
        _ => None,
    }
}

/// The lattice-gated members plus the writer `close`/`flush`
/// (fields and host methods fall through to `__meth*`).
fn surface_gated(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    // Split by family for the complexity budget (arms are disjoint,
    // so order is irrelevant).
    surface_gated_coll(eval, meth, target, rest)
        .or_else(|| surface_gated_misc(eval, meth, target, rest))
}

/// The collection-gated members (`String`, `Named`, `List`,
/// `MapEntry`).
fn surface_gated_coll(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    match (meth, rest.len()) {
        ("isEmpty", 0) => gated_empty(eval, target),
        ("isBlank", 0) => gated_blank(target),
        ("getName", 0) => gated_named_name(target),
        ("getNamespace", 0) => gated_named_ns(target),
        ("getFirst", 0) => gated_list_end(eval, target, true),
        ("getLast", 0) => gated_list_end(eval, target, false),
        ("getKey", 0) => gated_entry_key(target),
        ("getValue", 0) => gated_entry_val(target),
        _ => None,
    }
}

/// The throwable/pending/writer-gated members.
fn surface_gated_misc(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    match (meth, rest.len()) {
        ("getMessage" | "getLocalizedMessage", 0) => gated_throwable_msg(target),
        ("getCause", 0) => gated_throwable_cause(target),
        ("getData", 0) => gated_exinfo_data(target),
        ("isRealized", 0) => gated_realized(eval, target),
        ("close", 0) => gated_writer_close(eval, target),
        ("flush", 0) => gated_writer_flush(eval, target),
        _ => None,
    }
}

/// `isBlank` on strings.
fn gated_blank(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Str(s) => Some(Ok(Value::Bool(is_blank(s)))),
        _ => None,
    }
}

/// `getKey`/`getValue` on entries.
fn gated_entry_key(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::MapEntry { key, .. } => Some(Ok(key.as_ref().clone())),
        _ => None,
    }
}

/// `getValue` on entries.
fn gated_entry_val(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::MapEntry { val, .. } => Some(Ok(val.as_ref().clone())),
        _ => None,
    }
}

/// `getMessage`/`getLocalizedMessage` on throwables.
fn gated_throwable_msg(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Throwable(t) => Some(Ok(throwable_message(t))),
        _ => None,
    }
}

/// `getCause` on throwables.
fn gated_throwable_cause(target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Throwable(t) => Some(Ok(throwable_cause(t))),
        _ => None,
    }
}

/// `isRealized` on IPending members.
fn gated_realized(eval: &Evaluator, target: &Value) -> Option<Result<Value, NativeError>> {
    if is_instance_of(eval, "clojure.lang.IPending", target) {
        Some(Ok(Value::Bool(realized_flag(target))))
    } else {
        None
    }
}

/// `close` on writers (void — callable, never a bean key).
fn gated_writer_close(eval: &mut Evaluator, target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Extension(e) => crate::io::close_writer_value(eval, e),
        _ => None,
    }
}

/// `flush` on writers (void — callable, never a bean key).
fn gated_writer_flush(eval: &mut Evaluator, target: &Value) -> Option<Result<Value, NativeError>> {
    match target {
        Value::Extension(e) => crate::io::flush_writer_value(eval, e),
        _ => None,
    }
}

/// The member surface for `__meth*`: `None` when the member misses
/// (the caller falls through to the host dispatch and its miss
/// shapes).
pub(crate) fn surface_meth(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    if let Some(hit) = surface_universal(eval, meth, target, rest) {
        return Some(hit);
    }
    if let Some(hit) = surface_class_member(eval, meth, target, rest) {
        return Some(hit);
    }
    surface_gated(eval, meth, target, rest)
}

/// The surface as `(member, argc)` pairs (sorted for the unit test's
/// nil-target sweep).
pub(crate) const SURFACE_MEMBERS: &[(&str, usize)] = &[
    ("cast", 1),
    ("close", 0),
    ("equals", 1),
    ("flush", 0),
    ("getCause", 0),
    ("getClass", 0),
    ("getData", 0),
    ("getFirst", 0),
    ("getInterfaces", 0),
    ("getKey", 0),
    ("getLast", 0),
    ("getLocalizedMessage", 0),
    ("getMessage", 0),
    ("getName", 0),
    ("getNamespace", 0),
    ("getSimpleName", 0),
    ("getSuperclass", 0),
    ("getValue", 0),
    ("hashCode", 0),
    ("isArray", 0),
    ("isAssignableFrom", 1),
    ("isBlank", 0),
    ("isEmpty", 0),
    ("isInstance", 1),
    ("isInterface", 0),
    ("isRealized", 0),
    ("toString", 0),
];

/// Whether `(member, argc)` resolves against the surface (the
/// nil-target rule consults this ignoring the target's kind — keep
/// in sync with [`surface_meth`]; the unit test sweeps every pair).
pub(crate) fn surface_member_hit(meth: &str, argc: usize) -> bool {
    SURFACE_MEMBERS.contains(&(meth, argc))
}

/// Whether `meth` names any surface member (any arity): 0-arg
/// misses on real members read method-shape.
pub(crate) fn surface_member_name(meth: &str) -> bool {
    SURFACE_MEMBERS.iter().any(|(name, _)| *name == meth)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn static_or_fallback_mints_for_missing() {
        // Every reported name is tabled (§1.4), so the fallback only
        // fires defensively: a missing name mints a plain
        // `Object`-child class of that name.
        let c = static_or_fallback("missing.Name");
        assert_eq!(c.name(), "missing.Name");
        assert!(!c.is_iface());
        assert_eq!(c.bases(), &["java.lang.Object".into()]);
    }

    #[test]
    fn decapitalize_total_on_empty() {
        // Defensive arm: `bean_suffix` never yields an empty suffix
        // (it needs a first char), so the empty input only arrives
        // here — total by construction.
        assert_eq!(decapitalize(""), "");
        assert_eq!(decapitalize("URL"), "URL");
        assert_eq!(decapitalize("Count"), "count");
    }

    #[test]
    fn realized_flag_defensive_arm() {
        // Defensive arm: the IPending gate admits only
        // delays/promises/futures/extensions, so anything else only
        // arrives here — never realized.
        assert!(!realized_flag(&Value::Int(1)));
        assert!(!realized_flag(&Value::Nil));
    }
}
