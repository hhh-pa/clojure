// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Transient model tests (S3 TDD): scripted op sequences against a
//! naive oracle, plus the probe19 error pins.

use clj_value::{
    Map, Set, TransientError, TransientMap, TransientSet, TransientVector, Value, Vector,
};

fn int(i: i64) -> Value {
    Value::Int(i)
}

#[test]
fn vector_transient_models_vec() {
    let mut tv = TransientVector::new();
    let mut model: Vec<Value> = Vec::new();
    // Scripted ops: (op, arg) — 0=conj, 1=pop, 2=assoc.
    let script = [
        (0, 10),
        (0, 20),
        (0, 30),
        (1, 0),
        (2, 99),
        (0, 40),
        (2, 77),
        (1, 0),
        (1, 0),
        (0, 50),
    ];
    for (step, (op, arg)) in script.iter().enumerate() {
        match op {
            0 => {
                tv.conj(int(*arg)).unwrap();
                model.push(int(*arg));
            }
            1 => {
                tv.pop().unwrap();
                model.pop();
            }
            _ => {
                let idx = (*arg as usize) % model.len();
                tv.assoc(idx, int(*arg)).unwrap();
                model[idx] = int(*arg);
            }
        }
        assert_eq!(tv.len().unwrap(), model.len(), "step {step}");
        for (i, want) in model.iter().enumerate() {
            assert_eq!(tv.nth(i).unwrap(), Some(want), "step {step} idx {i}");
        }
    }
    let frozen = tv.persistent().unwrap();
    assert_eq!(frozen.len(), model.len());
    for (i, want) in model.iter().enumerate() {
        assert_eq!(frozen.iter().nth(i), Some(want));
    }
}

#[test]
fn vector_transient_errors() {
    // pop on empty.
    let mut tv = TransientVector::new();
    let err = tv.pop().unwrap_err();
    assert!(matches!(err, TransientError::EmptyPop));
    assert_eq!(err.message(), "Can't pop empty vector");
    assert_eq!(err.jvm_class(), "java.lang.IllegalStateException");
    // assoc past the end (idx == len appends, beyond errors).
    tv.conj(int(1)).unwrap();
    tv.assoc(1, int(2)).unwrap();
    assert_eq!(tv.len().unwrap(), 2);
    let err = tv.assoc(5, int(9)).unwrap_err();
    assert!(matches!(err, TransientError::IndexOutOfBounds));
    assert_eq!(err.message(), "");
    assert_eq!(err.jvm_class(), "java.lang.IndexOutOfBoundsException");
    // Use after persistent (mutating op).
    let _ = tv.persistent().unwrap();
    let err = tv.conj(int(3)).unwrap_err();
    assert!(matches!(err, TransientError::UseAfterPersistent));
    assert_eq!(err.message(), "Transient used after persistent! call");
    assert_eq!(err.jvm_class(), "java.lang.IllegalAccessError");
    // Double persistent.
    let err = tv.persistent().unwrap_err();
    assert!(matches!(err, TransientError::UseAfterPersistent));
}

#[test]
fn vector_transient_reads_check_validity() {
    let mut tv = TransientVector::from_vector(&Vector::from_vec(vec![int(1), int(2)]));
    let _ = tv.persistent().unwrap();
    assert!(matches!(tv.nth(0), Err(TransientError::UseAfterPersistent)));
    assert!(matches!(tv.len(), Err(TransientError::UseAfterPersistent)));
}

#[test]
fn map_transient_models_assoc_list() {
    let mut tm = TransientMap::new();
    // Naive oracle: insertion-ordered assoc list.
    let mut model: Vec<(Value, Value)> = Vec::new();
    let script = [
        (0, 1, 10),
        (0, 2, 20),
        (0, 1, 11),
        (1, 2, 0),
        (0, 3, 30),
        (0, 2, 22),
    ];
    for (step, (op, k, v)) in script.iter().enumerate() {
        if *op == 0 {
            tm.assoc(int(*k), int(*v)).unwrap();
            match model.iter_mut().find(|(ek, _)| ek == &int(*k)) {
                Some(slot) => slot.1 = int(*v),
                None => model.push((int(*k), int(*v))),
            }
        } else {
            tm.dissoc(&int(*k)).unwrap();
            model.retain(|(ek, _)| ek != &int(*k));
        }
        assert_eq!(tm.len().unwrap(), model.len(), "step {step}");
        for (k, want) in &model {
            assert_eq!(tm.get(k).unwrap(), Some(want), "step {step} key {k:?}");
        }
    }
    let frozen: Map = tm.persistent().unwrap();
    assert_eq!(frozen.size(), model.len());
    let order: Vec<Value> = frozen.iter().map(|(k, _)| k.clone()).collect();
    let want: Vec<Value> = model.iter().map(|(k, _)| k.clone()).collect();
    assert_eq!(order, want);
}

#[test]
fn map_transient_errors() {
    let mut tm = TransientMap::new();
    tm.assoc(int(1), int(2)).unwrap();
    let _ = tm.persistent().unwrap();
    assert!(matches!(
        tm.assoc(int(3), int(4)),
        Err(TransientError::UseAfterPersistent)
    ));
    assert!(matches!(
        tm.get(&int(1)),
        Err(TransientError::UseAfterPersistent)
    ));
}

#[test]
fn set_transient_models_vec_set() {
    let mut ts = TransientSet::new();
    let mut model: Vec<Value> = Vec::new();
    let script = [(0, 1), (0, 2), (0, 1), (1, 2), (0, 3), (1, 9)];
    for (step, (op, v)) in script.iter().enumerate() {
        if *op == 0 {
            ts.conj(int(*v)).unwrap();
            if !model.contains(&int(*v)) {
                model.push(int(*v));
            }
        } else {
            ts.disj(&int(*v)).unwrap();
            model.retain(|m| m != &int(*v));
        }
        assert_eq!(ts.len().unwrap(), model.len(), "step {step}");
        for m in &model {
            assert!(ts.contains(m).unwrap(), "step {step} member {m:?}");
        }
    }
    let frozen: Set = ts.persistent().unwrap();
    assert_eq!(frozen.size(), model.len());
    let order: Vec<Value> = frozen.iter().cloned().collect();
    assert_eq!(order, model);
}

#[test]
fn set_transient_errors() {
    let mut ts = TransientSet::new();
    ts.conj(int(1)).unwrap();
    let _ = ts.persistent().unwrap();
    assert!(matches!(
        ts.conj(int(2)),
        Err(TransientError::UseAfterPersistent)
    ));
    assert!(matches!(
        ts.contains(&int(1)),
        Err(TransientError::UseAfterPersistent)
    ));
}

#[test]
fn transient_round_trip_preserves_collections() {
    let v = Vector::from_vec(vec![int(1), int(2), int(3)]);
    let mut tv = TransientVector::from_vector(&v);
    assert_eq!(tv.persistent().unwrap(), v);
    let m = Map::from_kvs_checked(vec![(int(1), int(2)), (int(3), int(4))]).unwrap();
    let mut tm = TransientMap::from_map(&m);
    assert_eq!(tm.persistent().unwrap(), m);
    let s = Set::from_items_checked(vec![int(1), int(2)]).unwrap();
    let mut ts = TransientSet::from_set(&s);
    assert_eq!(ts.persistent().unwrap(), s);
}
