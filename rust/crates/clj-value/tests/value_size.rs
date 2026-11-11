// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Value layout pins (#221 R1): every `Value` pays the largest
//! variant's size inline in every vector, transient, and frame slot,
//! so the large payloads (map/set sidecars, transient/future/promise
//! metadata) ride behind `Arc`s. These pins fail loudly if a variant
//! regrows past the budget.

use clj_value::{
    Array, ArrayDesc, ArrayStore, BaseType, Elem8, Future, Map, PrimitiveVector, Promise, Set,
    Transient, Value, Vector,
};

/// The whole-enum budget: 32 bytes (was 112 before the sidecar and
/// metadata arcs, 72 before map identity moved onto the entries
/// `Arc`, 64 before the collection/symbol payloads became one-word
/// `Arc` handles in #254). Lower is welcome — tighten the bound if
/// a variant shrinks for real.
#[test]
fn value_stays_compact() {
    assert!(
        size_of::<Value>() <= 32,
        "Value grew to {} bytes; keep large payloads behind Arc (see #221 R1)",
        size_of::<Value>()
    );
}

/// The one-word handle types keep their pre-handle `Debug` shape
/// (struct name and field names, #254): the manual impls print
/// through the `Arc` exactly like the old derives.
#[test]
fn handle_debug_matches_struct_shape() {
    let v = format!("{:?}", Vector::from_vec(vec![Value::Int(1)]));
    assert!(v.starts_with("Vector { inner: "), "{v}");
    let s = format!("{:?}", clj_value::new_set().insert(Value::Int(1)));
    assert!(s.starts_with("Set { inner: "), "{s}");
    let a = format!(
        "{:?}",
        Array::new(ArrayDesc::flat(BaseType::Int), ArrayStore::Ints(vec![1]))
    );
    assert!(a.starts_with("Array { id: "), "{a}");
    let p = format!("{:?}", PrimitiveVector::empty(Elem8::Int));
    assert!(p.starts_with("PrimitiveVector { elem: "), "{p}");
}

/// Per-struct budgets behind the enum pin: each large payload stays
/// indirect, so no single struct can silently re-inflate `Value`.
#[test]
fn large_payloads_stay_indirect() {
    for (name, size, budget) in [
        ("Map", size_of::<Map>(), 8),
        ("Set", size_of::<Set>(), 8),
        ("Transient", size_of::<Transient>(), 24),
        ("Future", size_of::<Future>(), 24),
        ("Promise", size_of::<Promise>(), 24),
    ] {
        assert!(
            size <= budget,
            "{name} grew to {size} bytes (budget {budget}); keep large payloads behind Arc (see #221 R1)"
        );
    }
}
