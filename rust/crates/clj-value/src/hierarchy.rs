// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Hierarchies (zero-oos S13, issue #57): the pure engine under
//! `derive` / `isa?` / `parents` / `ancestors` / `descendants` /
//! `underive` and hierarchy-aware multimethod dispatch.
//!
//! A hierarchy is an ordinary map with `:parents` / `:ancestors`
//! / `:descendants` relation maps (tag → set). Every function
//! here is a line port of `core.clj` with the oracle's touch
//! order: relation values are *invoked* like the JVM invokes
//! them, so a missing key NPEs with the call-site text and a
//! non-map value fails the `IFn` cast (probes `h-badh-*`,
//! `n-derive-*`, `n-isa-badanc`).
//!
//! Classes are class-name strings in the lattice below plus
//! `HostDef` descriptors (instances are never classes).
//! Hierarchy keys identify classes by name: descriptors fold
//! to their name string at the boundary, so a type derived by
//! descriptor is found by name and vice versa.

use std::sync::Arc;

use crate::{HostDef, HostKind, HostVal, Keyword, Map, Set, ThrowClass, Value, new_map, new_set};

/// A hierarchy failure. NPE / assert / `Exception` texts are
/// fully built here (verbatim oracle strings); casts render
/// evaluator-side (the `Value` → JVM-name map lives there).
#[derive(Clone, Debug)]
pub struct HierError {
    pub class: ThrowClass,
    pub message: HierMessage,
}

/// A built message or a cast awaiting its loader text (the
/// `from` value boxes down the error size for the
/// `result_large_err` lint).
#[derive(Clone, Debug)]
pub enum HierMessage {
    Text(String),
    Cast { from: Box<Value>, to: &'static str },
}

impl HierError {
    fn text(class: ThrowClass, message: String) -> Self {
        Self {
            class,
            message: HierMessage::Text(message),
        }
    }

    /// A helpful-NPE mirror (verbatim oracle text).
    pub fn npe(message: &'static str) -> Self {
        Self::text(ThrowClass::NullPointerException, message.to_owned())
    }

    /// A cast failure (loader text renders evaluator-side).
    pub fn cce(from: Value, to: &'static str) -> Self {
        Self {
            class: ThrowClass::ClassCastException,
            message: HierMessage::Cast {
                from: Box::new(from),
                to,
            },
        }
    }

    /// A `derive` assertion failure (verbatim oracle text).
    fn assert_failed(form: &str) -> Self {
        Self::text(ThrowClass::AssertionError, format!("Assert failed: {form}"))
    }

    /// A plain `Exception` (already-has / cyclic, `print-str`
    /// rendering like the JVM).
    fn exception(message: String) -> Self {
        Self::text(ThrowClass::Exception, message)
    }
}

/// One lattice row: JVM-truthful ancestry for a class-name
/// string (`bases` = `parents`, `supers` = `ancestors`).
#[derive(Clone, Copy, Debug)]
pub struct LatticeEntry {
    pub interface: bool,
    pub bases: &'static [&'static str],
    pub supers: &'static [&'static str],
}

/// A bare interface (seen in probes, ancestry unprobed):
/// `isa?` to `Object` holds, bases/supers read empty.
const BARE_IFACE: LatticeEntry = LatticeEntry {
    interface: true,
    bases: &[],
    supers: &[],
};

/// The S13 class lattice, ASCII-sorted for binary search: 41
/// full rows plus every other interface the probes observed
/// (bare — the `isa?`-to-`Object` rule holds, ancestry reads
/// empty). Names outside the table are not classes.
static LATTICE: &[(&str, LatticeEntry)] = &[
    (
        "clojure.lang.APersistentVector$SubVector",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentVector",
                "clojure.lang.IKVReduce",
                "clojure.lang.IObj",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.Agent",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.ARef"],
            supers: &[
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
        },
    ),
    ("clojure.lang.Associative", BARE_IFACE),
    (
        "clojure.lang.Atom",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.ARef", "clojure.lang.IAtom2"],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.BigInt",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Number", "clojure.lang.IHashEq"],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "clojure.lang.IHashEq",
                "java.io.Serializable",
            ],
        },
    ),
    ("clojure.lang.Counted", BARE_IFACE),
    (
        "clojure.lang.Delay",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Object",
                "clojure.lang.IDeref",
                "clojure.lang.IPending",
            ],
            supers: &[
                "java.lang.Object",
                "clojure.lang.IDeref",
                "clojure.lang.IPending",
                "java.util.function.BooleanSupplier",
                "java.util.function.DoubleSupplier",
                "java.util.function.IntSupplier",
                "java.util.function.LongSupplier",
                "java.util.function.Supplier",
            ],
        },
    ),
    ("clojure.lang.IAtom", BARE_IFACE),
    ("clojure.lang.IAtom2", BARE_IFACE),
    ("clojure.lang.IChunkedSeq", BARE_IFACE),
    ("clojure.lang.IDeref", BARE_IFACE),
    ("clojure.lang.IDrop", BARE_IFACE),
    ("clojure.lang.IEditableCollection", BARE_IFACE),
    (
        "clojure.lang.IFn",
        LatticeEntry {
            interface: true,
            bases: &["java.lang.Runnable", "java.util.concurrent.Callable"],
            supers: &["java.lang.Runnable", "java.util.concurrent.Callable"],
        },
    ),
    ("clojure.lang.IHashEq", BARE_IFACE),
    ("clojure.lang.IKVReduce", BARE_IFACE),
    ("clojure.lang.ILookup", BARE_IFACE),
    ("clojure.lang.IMapEntry", BARE_IFACE),
    ("clojure.lang.IMapIterable", BARE_IFACE),
    ("clojure.lang.IMeta", BARE_IFACE),
    ("clojure.lang.IObj", BARE_IFACE),
    ("clojure.lang.IPending", BARE_IFACE),
    ("clojure.lang.IPersistentCollection", BARE_IFACE),
    ("clojure.lang.IPersistentList", BARE_IFACE),
    ("clojure.lang.IPersistentMap", BARE_IFACE),
    ("clojure.lang.IPersistentSet", BARE_IFACE),
    ("clojure.lang.IPersistentStack", BARE_IFACE),
    ("clojure.lang.IPersistentVector", BARE_IFACE),
    ("clojure.lang.IReduce", BARE_IFACE),
    ("clojure.lang.IReduceInit", BARE_IFACE),
    ("clojure.lang.IRef", BARE_IFACE),
    ("clojure.lang.IReference", BARE_IFACE),
    (
        "clojure.lang.ISeq",
        LatticeEntry {
            interface: true,
            bases: &["clojure.lang.IPersistentCollection"],
            supers: &["clojure.lang.IPersistentCollection", "clojure.lang.Seqable"],
        },
    ),
    ("clojure.lang.ITransientAssociative", BARE_IFACE),
    ("clojure.lang.ITransientAssociative2", BARE_IFACE),
    ("clojure.lang.ITransientCollection", BARE_IFACE),
    ("clojure.lang.ITransientMap", BARE_IFACE),
    ("clojure.lang.ITransientSet", BARE_IFACE),
    ("clojure.lang.ITransientVector", BARE_IFACE),
    ("clojure.lang.Indexed", BARE_IFACE),
    (
        "clojure.lang.Keyword",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Object",
                "clojure.lang.IFn",
                "clojure.lang.IHashEq",
                "clojure.lang.Named",
                "java.io.Serializable",
                "java.lang.Comparable",
            ],
            supers: &[
                "java.lang.Object",
                "clojure.lang.IFn",
                "clojure.lang.IHashEq",
                "clojure.lang.Named",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.Runnable",
                "java.util.concurrent.Callable",
            ],
        },
    ),
    (
        "clojure.lang.LazySeq",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.Obj",
                "clojure.lang.IHashEq",
                "clojure.lang.IPending",
                "clojure.lang.ISeq",
                "clojure.lang.Sequential",
                "java.util.List",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.LongRange",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.ASeq",
                "clojure.lang.Counted",
                "clojure.lang.IChunkedSeq",
                "clojure.lang.IDrop",
                "clojure.lang.IReduce",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.MapEntry",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.AMapEntry"],
            supers: &[
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
        },
    ),
    ("clojure.lang.MapEquivalence", BARE_IFACE),
    (
        "clojure.lang.MultiFn",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.AFn"],
            supers: &[
                "clojure.lang.AFn",
                "java.lang.Object",
                "clojure.lang.IFn",
                "java.lang.Runnable",
                "java.util.concurrent.Callable",
            ],
        },
    ),
    ("clojure.lang.Named", BARE_IFACE),
    (
        "clojure.lang.PersistentArrayMap",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentMap",
                "clojure.lang.IDrop",
                "clojure.lang.IEditableCollection",
                "clojure.lang.IKVReduce",
                "clojure.lang.IMapIterable",
                "clojure.lang.IObj",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentArrayMap$TransientArrayMap",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.ATransientMap"],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentHashMap",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentMap",
                "clojure.lang.IEditableCollection",
                "clojure.lang.IKVReduce",
                "clojure.lang.IMapIterable",
                "clojure.lang.IObj",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentHashSet",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentSet",
                "clojure.lang.IEditableCollection",
                "clojure.lang.IObj",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentHashSet$TransientHashSet",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.ATransientSet"],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentList",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.ASeq",
                "clojure.lang.Counted",
                "clojure.lang.IPersistentList",
                "clojure.lang.IReduce",
                "java.util.List",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentList$EmptyList",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.Obj",
                "clojure.lang.Counted",
                "clojure.lang.IHashEq",
                "clojure.lang.IPersistentList",
                "clojure.lang.ISeq",
                "java.util.List",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentTreeMap",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentMap",
                "clojure.lang.IKVReduce",
                "clojure.lang.IObj",
                "clojure.lang.Reversible",
                "clojure.lang.Sorted",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentTreeSet",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentSet",
                "clojure.lang.IObj",
                "clojure.lang.Reversible",
                "clojure.lang.Sorted",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentVector",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.APersistentVector",
                "clojure.lang.IDrop",
                "clojure.lang.IEditableCollection",
                "clojure.lang.IKVReduce",
                "clojure.lang.IObj",
                "clojure.lang.IReduce",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.PersistentVector$TransientVector",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.AFn",
                "clojure.lang.Counted",
                "clojure.lang.ITransientAssociative2",
                "clojure.lang.ITransientVector",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.Range",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.ASeq",
                "clojure.lang.IChunkedSeq",
                "clojure.lang.IReduce",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.Ratio",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Number", "java.lang.Comparable"],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
            ],
        },
    ),
    (
        "clojure.lang.ReaderConditional",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Object", "clojure.lang.ILookup"],
            supers: &["java.lang.Object", "clojure.lang.ILookup"],
        },
    ),
    ("clojure.lang.Reversible", BARE_IFACE),
    ("clojure.lang.Seqable", BARE_IFACE),
    ("clojure.lang.Sequential", BARE_IFACE),
    ("clojure.lang.Settable", BARE_IFACE),
    ("clojure.lang.Sorted", BARE_IFACE),
    (
        "clojure.lang.Symbol",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.AFn",
                "clojure.lang.IHashEq",
                "clojure.lang.IObj",
                "clojure.lang.Named",
                "java.io.Serializable",
                "java.lang.Comparable",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.TaggedLiteral",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Object", "clojure.lang.ILookup"],
            supers: &["java.lang.Object", "clojure.lang.ILookup"],
        },
    ),
    (
        "clojure.lang.Var",
        LatticeEntry {
            interface: false,
            bases: &[
                "clojure.lang.ARef",
                "clojure.lang.IFn",
                "clojure.lang.IRef",
                "clojure.lang.Settable",
                "java.io.Serializable",
            ],
            supers: &[
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
        },
    ),
    (
        "clojure.lang.Var$Unbound",
        LatticeEntry {
            interface: false,
            bases: &["clojure.lang.AFn"],
            supers: &[
                "clojure.lang.AFn",
                "java.lang.Object",
                "clojure.lang.IFn",
                "java.lang.Runnable",
                "java.util.concurrent.Callable",
            ],
        },
    ),
    ("java.io.Serializable", BARE_IFACE),
    (
        "java.lang.Boolean",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
            ],
            supers: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
            ],
        },
    ),
    (
        "java.lang.CharSequence",
        LatticeEntry {
            interface: true,
            bases: &[],
            supers: &[],
        },
    ),
    (
        "java.lang.Character",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
            ],
            supers: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
            ],
        },
    ),
    ("java.lang.Comparable", BARE_IFACE),
    (
        "java.lang.Double",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Number",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
        },
    ),
    ("java.lang.Iterable", BARE_IFACE),
    (
        "java.lang.Long",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Number",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
        },
    ),
    (
        "java.lang.Number",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Object", "java.io.Serializable"],
            supers: &["java.lang.Object", "java.io.Serializable"],
        },
    ),
    (
        "java.lang.Object",
        LatticeEntry {
            interface: false,
            bases: &[],
            supers: &[],
        },
    ),
    ("java.lang.Runnable", BARE_IFACE),
    (
        "java.lang.String",
        LatticeEntry {
            interface: false,
            bases: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.CharSequence",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
            supers: &[
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.CharSequence",
                "java.lang.Comparable",
                "java.lang.constant.Constable",
                "java.lang.constant.ConstantDesc",
            ],
        },
    ),
    ("java.lang.constant.Constable", BARE_IFACE),
    ("java.lang.constant.ConstantDesc", BARE_IFACE),
    (
        "java.math.BigDecimal",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Number", "java.lang.Comparable"],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
            ],
        },
    ),
    (
        "java.math.BigInteger",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Number", "java.lang.Comparable"],
            supers: &[
                "java.lang.Number",
                "java.lang.Object",
                "java.io.Serializable",
                "java.lang.Comparable",
            ],
        },
    ),
    ("java.util.Collection", BARE_IFACE),
    ("java.util.List", BARE_IFACE),
    ("java.util.Map", BARE_IFACE),
    ("java.util.Map$Entry", BARE_IFACE),
    ("java.util.RandomAccess", BARE_IFACE),
    ("java.util.SequencedCollection", BARE_IFACE),
    ("java.util.Set", BARE_IFACE),
    ("java.util.concurrent.Callable", BARE_IFACE),
    ("java.util.function.BooleanSupplier", BARE_IFACE),
    ("java.util.function.DoubleSupplier", BARE_IFACE),
    ("java.util.function.IntSupplier", BARE_IFACE),
    ("java.util.function.LongSupplier", BARE_IFACE),
    ("java.util.function.Supplier", BARE_IFACE),
    (
        "java.util.regex.Pattern",
        LatticeEntry {
            interface: false,
            bases: &["java.lang.Object", "java.io.Serializable"],
            supers: &["java.lang.Object", "java.io.Serializable"],
        },
    ),
];

/// Look up a class name (binary search over the sorted table).
fn lattice(name: &str) -> Option<&'static LatticeEntry> {
    LATTICE
        .binary_search_by(|(n, _)| n.cmp(&name))
        .ok()
        .map(|i| &LATTICE[i].1)
}

/// A resolved class: lattice row or host descriptor.
#[derive(Clone, Copy, Debug)]
enum Class<'a> {
    Lattice(&'static LatticeEntry),
    Host(&'a HostDef),
}

/// Resolve a value to a class (tabled name strings, class
/// values (S16, by name through the same lattice), and `HostDef`
/// descriptors; instances and unknown names are not classes) with
/// its canonical name.
fn class_of(v: &Value) -> Option<(String, Class<'_>)> {
    match v {
        Value::Str(s) => lattice(s).map(|e| (s.to_string(), Class::Lattice(e))),
        Value::Class(c) => lattice(c.name()).map(|e| (c.name().to_string(), Class::Lattice(e))),
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => Some((d.name.to_string(), Class::Host(d))),
            HostVal::Inst(_) => None,
        },
        _ => None,
    }
}

/// Whether a value counts as a class (the `class?` analog).
#[must_use]
pub fn is_class_value(v: &Value) -> bool {
    class_of(v).is_some()
}

/// Whether a class is an interface.
fn is_interface(class: &Class) -> bool {
    match class {
        Class::Lattice(e) => e.interface,
        Class::Host(d) => d.kind == HostKind::Interface,
    }
}

/// Direct parents (JVM `bases`): lattice rows verbatim; host
/// types claim `Object` plus their interfaces, host
/// interfaces claim nothing (no superclass).
fn class_bases<'a>(class: &'a Class) -> Vec<&'a str> {
    match class {
        Class::Lattice(e) => e.bases.to_vec(),
        Class::Host(d) if d.kind == HostKind::Interface => Vec::new(),
        Class::Host(d) => {
            let mut out = vec!["java.lang.Object"];
            out.extend(d.interfaces.iter().map(AsRef::as_ref));
            out
        }
    }
}

/// Transitive supers (JVM `supers`): lattice rows verbatim;
/// host types their interfaces plus `Object`, host
/// interfaces nothing.
fn class_supers<'a>(class: &'a Class) -> Vec<&'a str> {
    match class {
        Class::Lattice(e) => e.supers.to_vec(),
        Class::Host(d) if d.kind == HostKind::Interface => Vec::new(),
        Class::Host(d) => {
            let mut out: Vec<&str> = d.interfaces.iter().map(AsRef::as_ref).collect();
            out.push("java.lang.Object");
            out
        }
    }
}

/// Class assignability (`isAssignableFrom`): same name, or
/// the parent among the child's transitive supers, or an
/// interface child with parent `Object` (probe `n-iface-obj`).
fn assignable_to(child_name: &str, child: &Class, parent_name: &str) -> bool {
    if child_name == parent_name {
        return true;
    }
    if class_supers(child).contains(&parent_name) {
        return true;
    }
    parent_name == "java.lang.Object" && is_interface(child)
}

/// Whether two values hit the class arm (both classes and
/// assignable).
fn class_pair_hit(child: &Value, parent: &Value) -> bool {
    match (class_of(child), class_of(parent)) {
        (Some((cn, cc)), Some((pn, _))) => assignable_to(&cn, &cc, &pn),
        _ => false,
    }
}

/// Hierarchy keys identify classes by name: descriptors and
/// class values (S16) fold to their name string, everything else
/// passes through.
fn hier_key(v: &Value) -> Value {
    match v {
        Value::HostType(h) => match h.as_ref() {
            HostVal::Def(d) => Value::Str(d.name.clone()),
            HostVal::Inst(_) => v.clone(),
        },
        Value::Class(c) => Value::Str(Arc::from(c.name())),
        _ => v.clone(),
    }
}

/// One relation keyword.
fn rel_key(name: &str) -> Value {
    Value::Keyword(Keyword::new(None, name))
}

/// `(:rel h)`: the relation value, or `None` when `h` is not
/// a map or lacks the key (JVM keyword-get tolerates both).
fn rel_opt<'a>(h: &'a Value, key: &Value) -> Option<&'a Value> {
    match h {
        Value::Map(m) => m.get(key),
        _ => None,
    }
}

/// A relation value as an invokable map (`(tp tag)`): maps
/// pass through, missing/nil NPEs with the call-site text,
/// anything else fails the `IFn` cast.
fn as_map_opt<'a>(m: Option<&'a Value>, nil_text: &'static str) -> Result<&'a Map, HierError> {
    match m {
        Some(Value::Map(mm)) => Ok(mm),
        Some(Value::Nil) | None => Err(HierError::npe(nil_text)),
        Some(other) => Err(HierError::cce(other.clone(), "clojure.lang.IFn")),
    }
}

/// Membership (`contains?` tolerates non-sets as absent).
fn set_has(v: Option<&Value>, member: &Value) -> bool {
    matches!(v, Some(Value::Set(s)) if s.contains(member))
}

/// A relation value as a set (missing → empty; anything else
/// is the `IPersistentSet` cast — derived garbage-in rule).
fn as_set(v: Option<&Value>) -> Result<Set, HierError> {
    match v {
        None | Some(Value::Nil) => Ok(new_set()),
        Some(Value::Set(s)) => Ok(s.clone()),
        Some(other) => Err(HierError::cce(other.clone(), "clojure.lang.IPersistentSet")),
    }
}

/// A view's map entry (tolerant `get`, strict set): the
/// relation set, `None` when missing/empty, else the set
/// cast (derived garbage-in rule).
fn view_entry(h: &Value, key: &Value, tag: &Value) -> Result<Option<Set>, HierError> {
    let rel = rel_opt(h, key);
    let entry = match rel {
        None | Some(Value::Nil) => None,
        Some(Value::Map(m)) => m.get(&hier_key(tag)),
        // JVM `get` on a non-map is nil (probe `h-badh-views`).
        Some(_) => None,
    };
    match entry {
        None | Some(Value::Nil) => Ok(None),
        Some(Value::Set(s)) if s.is_empty() => Ok(None),
        Some(Value::Set(s)) => Ok(Some(s.clone())),
        Some(other) => Err(HierError::cce(other.clone(), "clojure.lang.IPersistentSet")),
    }
}

/// Vector-like items: vectors in order, map entries as
/// pairs (a `MapEntry` is an `APersistentVector`, probe
/// `l-mapentry`).
fn vec_items(v: &Value) -> Option<Vec<&Value>> {
    match v {
        Value::Vector(vv) => Some(vv.iter().collect()),
        Value::MapEntry { key, val } => Some(vec![key, val]),
        _ => None,
    }
}

const ISA_NPE: &str = "Cannot invoke \"clojure.lang.IFn.invoke(Object)\"";
const TP_NPE: &str = "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"tp\" is null";
const TA_NPE: &str = "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"ta\" is null";
const SOURCES_NPE: &str =
    "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"sources\" is null";
const PARENTMAP_NPE: &str =
    "Cannot invoke \"clojure.lang.IFn.invoke(Object)\" because \"parentMap\" is null";

/// `isa?`: `=`, then class assignability, then the ancestors
/// map (which NPEs/CCEs like the JVM touch), then supers'
/// map entries for class children, then element-wise
/// vectors. The map validated once runs the rest
/// infallibly (same `h` throughout).
pub fn isa_in(h: &Value, child: &Value, parent: &Value) -> Result<bool, HierError> {
    if child == parent {
        return Ok(true);
    }
    if class_pair_hit(child, parent) {
        return Ok(true);
    }
    let am = as_map_opt(rel_opt(h, &rel_key("ancestors")), ISA_NPE)?;
    Ok(isa_with_map(am, child, parent))
}

/// `isa?` against a validated ancestors map (infallible).
fn isa_with_map(am: &Map, child: &Value, parent: &Value) -> bool {
    if child == parent {
        return true;
    }
    if class_pair_hit(child, parent) {
        return true;
    }
    if set_has(am.get(&hier_key(child)), parent) {
        return true;
    }
    if let Some((_, cc)) = class_of(child) {
        for sup in class_supers(&cc) {
            if set_has(am.get(&Value::Str(Arc::from(sup))), parent) {
                return true;
            }
        }
    }
    match (vec_items(child), vec_items(parent)) {
        (Some(c), Some(p)) => {
            c.len() == p.len() && c.iter().zip(p.iter()).all(|(x, y)| isa_with_map(am, x, y))
        }
        _ => false,
    }
}

/// Whether a value is `Named` (keyword or symbol).
fn is_named(v: &Value) -> bool {
    matches!(v, Value::Keyword(_) | Value::Symbol(_))
}

/// The explicit-`derive` tag assert (class or `Named`).
fn assert_tag(tag: &Value) -> Result<(), HierError> {
    if is_class_value(tag) || is_named(tag) {
        Ok(())
    } else {
        Err(HierError::assert_failed(
            "(or (class? tag) (instance? clojure.lang.Named tag))",
        ))
    }
}

/// The explicit-`derive` parent assert (`Named` only —
/// classes are rejected, probe `h-classparent`).
fn assert_parent(parent: &Value) -> Result<(), HierError> {
    if is_named(parent) {
        Ok(())
    } else {
        Err(HierError::assert_failed(
            "(instance? clojure.lang.Named parent)",
        ))
    }
}

/// The global-`derive` asserts: the parent runs through
/// `namespace` semantics (hinted `Named` on the JVM — a
/// namespace-less `Named` fails the assert, nil NPEs,
/// anything else fails the `Named` cast), and the tag must
/// be a class or a namespaced `Named` (probes
/// `h-gparent-plain`, `h-gtag-plain`).
pub fn assert_global_edge(tag: &Value, parent: &Value) -> Result<(), HierError> {
    match parent {
        Value::Keyword(k) if k.namespace().is_some() => {}
        Value::Symbol(s) if s.namespace().is_some() => {}
        Value::Keyword(_) | Value::Symbol(_) => {
            return Err(HierError::assert_failed("(namespace parent)"));
        }
        Value::Nil => {
            return Err(HierError::npe(
                "Cannot invoke \"clojure.lang.Named.getNamespace()\" because \"x\" is null",
            ));
        }
        other => {
            return Err(HierError::cce(other.clone(), "clojure.lang.Named"));
        }
    }
    let tag_ok = is_class_value(tag)
        || match tag {
            Value::Keyword(k) => k.namespace().is_some(),
            Value::Symbol(s) => s.namespace().is_some(),
            _ => false,
        };
    if tag_ok {
        Ok(())
    } else {
        Err(HierError::assert_failed(
            "(or (class? tag) (and (instance? clojure.lang.Named tag) (namespace tag)))",
        ))
    }
}

/// A fresh hierarchy map (probe `h-make` order).
#[must_use]
pub fn make_hierarchy_map() -> Map {
    new_map()
        .insert(rel_key("parents"), Value::Map(new_map()))
        .insert(rel_key("descendants"), Value::Map(new_map()))
        .insert(rel_key("ancestors"), Value::Map(new_map()))
}

/// `derive` in the JVM touch order: asserts, the `:parents`
/// touch (an existing edge returns `h` before any other
/// touch), the `:ancestors` touches for the already-has /
/// cyclic errors, then the transitive build. Descriptors
/// store under their name string; messages print the
/// original tag (`print-str` rendering).
pub fn derive_in(h: &Value, tag: &Value, parent: &Value) -> Result<Map, HierError> {
    if tag == parent {
        return Err(HierError::assert_failed("(not= tag parent)"));
    }
    assert_tag(tag)?;
    assert_parent(parent)?;
    match h {
        Value::Map(hm) => derive_in_map(hm, &hier_key(tag), tag, parent),
        // `(:parents h)` is nil → `(nil tag)` NPEs.
        _ => Err(HierError::npe(TP_NPE)),
    }
}

/// `derive` against a map `h` (split for the complexity
/// budget): validated relation maps, then the build.
fn derive_in_map(
    hm: &Map,
    tag: &Value,
    tag_orig: &Value,
    parent: &Value,
) -> Result<Map, HierError> {
    let parents_key = rel_key("parents");
    let ancestors_key = rel_key("ancestors");
    let tp = as_map_opt(hm.get(&parents_key), TP_NPE)?;
    if set_has(tp.get(tag), parent) {
        return Ok(hm.clone());
    }
    let ta = as_map_opt(hm.get(&ancestors_key), TA_NPE)?;
    if set_has(ta.get(tag), parent) {
        return Err(HierError::exception(format!(
            "{} already has {} as ancestor",
            crate::print_to_string(tag_orig),
            crate::print_to_string(parent),
        )));
    }
    if set_has(ta.get(parent), tag) {
        return Err(HierError::exception(format!(
            "Cyclic derivation: {} has {} as ancestor",
            crate::print_to_string(parent),
            crate::print_to_string(tag_orig),
        )));
    }
    let td = as_map_opt(hm.get(&rel_key("descendants")), SOURCES_NPE)?;
    let mut new_parents = tp.clone();
    new_parents = new_parents.insert(
        tag.clone(),
        Value::Set(as_set(tp.get(tag))?.insert(parent.clone())),
    );
    let new_ancestors = derive_tf(ta, tag, td, parent, ta)?;
    let new_descendants = derive_tf(td, parent, ta, tag, td)?;
    Ok(new_map()
        .insert(parents_key, Value::Map(new_parents))
        .insert(ancestors_key, Value::Map(new_ancestors))
        .insert(rel_key("descendants"), Value::Map(new_descendants)))
}

/// The `derive` transitive step (`tf` in `core.clj`): every
/// `source` plus its `sources`-entries gains `target` plus
/// `target`'s `targets`-entries.
fn derive_tf(
    m: &Map,
    source: &Value,
    sources: &Map,
    target: &Value,
    targets: &Map,
) -> Result<Map, HierError> {
    let mut out = m.clone();
    let target_set = as_set(targets.get(target))?;
    let mut keys = vec![source.clone()];
    for extra in as_set(sources.get(source))?.iter() {
        keys.push(extra.clone());
    }
    for k in &keys {
        let mut s = as_set(targets.get(k))?;
        s = s.insert(target.clone());
        for v in target_set.iter() {
            s = s.insert(v.clone());
        }
        out = out.insert(k.clone(), Value::Set(s));
    }
    Ok(out)
}

/// `underive`: drop the edge and rebuild from the surviving
/// parent edges (like the JVM `reduce1 derive` replay); a
/// missing edge returns `h` untouched.
pub fn underive_in(h: &Value, tag: &Value, parent: &Value) -> Result<Map, HierError> {
    let tag = hier_key(tag);
    let hm = match h {
        Value::Map(m) => m,
        // `(parentMap tag)` with a nil map NPEs.
        _ => return Err(HierError::npe(PARENTMAP_NPE)),
    };
    let pm = as_map_opt(hm.get(&rel_key("parents")), PARENTMAP_NPE)?;
    if !set_has(pm.get(&tag), parent) {
        return Ok(hm.clone());
    }
    let mut edges: Vec<(Value, Value)> = Vec::new();
    for (t, _) in pm.iter() {
        let kept = if t == &tag {
            as_set(pm.get(&tag))?.without(parent)
        } else {
            as_set(pm.get(t))?
        };
        for p in kept.iter() {
            edges.push((t.clone(), p.clone()));
        }
    }
    let mut acc = make_hierarchy_map();
    for (t, p) in &edges {
        acc = derive_in(&Value::Map(acc), t, p)?;
    }
    Ok(acc)
}

/// `parents`: the map entry, unioned with lattice bases /
/// claimed interfaces for class tags (probes `h-p-*`).
pub fn parents_in(h: &Value, tag: &Value) -> Result<Option<Set>, HierError> {
    let own = view_entry(h, &rel_key("parents"), tag)?;
    match class_of(tag) {
        None => Ok(own.filter(|s| !s.is_empty())),
        Some((_, c)) => {
            let mut acc = new_set();
            for b in class_bases(&c) {
                acc = acc.insert(Value::Str(Arc::from(b)));
            }
            if let Some(s) = own {
                for v in s.iter() {
                    acc = acc.insert(v.clone());
                }
            }
            Ok(if acc.is_empty() { None } else { Some(acc) })
        }
    }
}

/// `ancestors`: the map entry; class tags add the
/// transitive supers plus every super's map entry (probes
/// `h-a-*`).
pub fn ancestors_in(h: &Value, tag: &Value) -> Result<Option<Set>, HierError> {
    let ancestors_key = rel_key("ancestors");
    let own = view_entry(h, &ancestors_key, tag)?;
    match class_of(tag) {
        None => Ok(own.filter(|s| !s.is_empty())),
        Some((_, c)) => ancestors_of_class(h, &ancestors_key, &c, own),
    }
}

/// A class tag's ancestors (split for the complexity
/// budget): transitive supers, the tag's own map entry,
/// and every super's map entry.
fn ancestors_of_class(
    h: &Value,
    ancestors_key: &Value,
    class: &Class,
    own: Option<Set>,
) -> Result<Option<Set>, HierError> {
    let mut acc = new_set();
    for s in class_supers(class) {
        acc = acc.insert(Value::Str(Arc::from(s)));
    }
    if let Some(s) = &own {
        for v in s.iter() {
            acc = acc.insert(v.clone());
        }
    }
    for s in class_supers(class) {
        let extra = view_entry(h, ancestors_key, &Value::Str(Arc::from(s)))?;
        if let Some(extra) = extra {
            for v in extra.iter() {
                acc = acc.insert(v.clone());
            }
        }
    }
    Ok(if acc.is_empty() { None } else { Some(acc) })
}

/// `descendants`: classes throw (probe `h-d-string`),
/// anything else reads the map.
pub fn descendants_in(h: &Value, tag: &Value) -> Result<Option<Set>, HierError> {
    if is_class_value(tag) {
        return Err(HierError::text(
            ThrowClass::UnsupportedOperationException,
            "Can't get descendants of classes".to_owned(),
        ));
    }
    Ok(view_entry(h, &rel_key("descendants"), tag)?.filter(|s| !s.is_empty()))
}

/// A best-method walk outcome.
#[derive(Clone, Debug)]
pub enum MethodLookup {
    Found(Value),
    Missing,
    Ambiguous { challenger: Value, best: Value },
}

/// A `method_for` failure: ambiguity (message built) or a
/// hierarchy error (renders evaluator-side).
#[derive(Clone, Debug)]
pub enum LookupFail {
    Ambiguous(String),
    Hier(HierError),
}

/// Whether `x` is directly preferred over `y`.
fn dominates(prefers: &Map, x: &Value, y: &Value) -> bool {
    matches!(prefers.get(x), Some(Value::Set(s)) if s.contains(y))
}

/// The best-method walk over the whole table (the default
/// entry competes like any other): a more specific key or
/// a preferred key displaces the best; an unrelated,
/// unpreferred challenger is the JVM ambiguity error.
pub fn best_method(
    methods: &Map,
    prefers: &Map,
    h: &Value,
    dispatch_val: &Value,
) -> Result<MethodLookup, HierError> {
    let mut best: Option<(&Value, &Value)> = None;
    for (k, v) in methods.iter() {
        if !isa_in(h, dispatch_val, k)? {
            continue;
        }
        match best {
            None => best = Some((k, v)),
            Some((bk, _)) => {
                let challenger_wins = isa_in(h, k, bk)? || dominates(prefers, k, bk);
                let best_holds = isa_in(h, bk, k)? || dominates(prefers, bk, k);
                if challenger_wins {
                    best = Some((k, v));
                } else if !best_holds {
                    return Ok(MethodLookup::Ambiguous {
                        challenger: k.clone(),
                        best: bk.clone(),
                    });
                }
            }
        }
    }
    Ok(match best {
        Some((_, v)) => MethodLookup::Found(v.clone()),
        None => MethodLookup::Missing,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lattice_is_sorted_and_complete() {
        // Binary search needs the sort; the count pins the
        // transcription (41 full rows + 59 bare interfaces).
        assert_eq!(LATTICE.len(), 100);
        for w in LATTICE.windows(2) {
            assert!(w[0].0 < w[1].0, "lattice out of order at {}", w[1].0);
        }
        // Every full row's bases and supers resolve or are
        // knowingly abstract (spot-checks pin the shape).
        let string = lattice("java.lang.String").unwrap();
        assert!(!string.interface);
        assert!(string.supers.contains(&"java.lang.Object"));
        assert!(lattice("java.lang.CharSequence").unwrap().interface);
        assert!(lattice("java.io.Serializable").unwrap().interface);
        assert!(lattice("java.lang.Object").unwrap().supers.is_empty());
        assert!(lattice("no.such.Class").is_none());
    }

    #[test]
    fn unknown_names_are_not_classes() {
        assert!(!is_class_value(&Value::Str(Arc::from("no.such.Class"))));
        assert!(!is_class_value(&Value::Int(1)));
        assert!(is_class_value(&Value::Str(Arc::from("java.lang.String"))));
    }
}
