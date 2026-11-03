// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Value layout pins (#221 R1): every `Value` pays the largest
//! variant's size inline in every vector, transient, and frame slot,
//! so the large payloads (map/set sidecars, transient/future/promise
//! metadata) ride behind `Arc`s. These pins fail loudly if a variant
//! regrows past the budget.

use clj_value::{Future, Map, Promise, Set, Transient, Value};

/// The whole-enum budget: 72 bytes (was 112 before the sidecar and
/// metadata arcs). Lower is welcome — tighten the bound if a variant
/// shrinks for real.
#[test]
fn value_stays_compact() {
    assert!(
        size_of::<Value>() <= 72,
        "Value grew to {} bytes; keep large payloads behind Arc (see #221 R1)",
        size_of::<Value>()
    );
}

/// Per-struct budgets behind the enum pin: each large payload stays
/// indirect, so no single struct can silently re-inflate `Value`.
#[test]
fn large_payloads_stay_indirect() {
    for (name, size, budget) in [
        ("Map", size_of::<Map>(), 72),
        ("Set", size_of::<Set>(), 64),
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
