// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #205 `java.util` collection emulations: `ArrayDeque`,
//! `ArrayList`, and `PriorityQueue` behind the `Extension`
//! facade. Deques/queues answer the suite's ends/membership
//! members; lists collect, seq (see `list_snapshot`, wired into
//! `seq_value_extension`), and mutate under
//! `Collections/shuffle`. Priority queues order by a reified
//! `Comparator` (invoked through `meth_call`) or the JVM
//! natural ordering (`jvm_compare`), extracted by linear scan
//! (suite heaps are small — the O(n) poll is honest).

use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use clj_value::{ExtensionValue, ThrowClass, Value};

use crate::error::NativeError;
use crate::eval::Evaluator;

/// An emulated `java.util.ArrayDeque`: identity plus the ends.
/// Null elements NPE (like the JDK).
#[derive(Debug)]
pub(crate) struct JavaDeque {
    id: u64,
    items: Mutex<VecDeque<Value>>,
}

/// An emulated `java.util.ArrayList`: identity plus the slots
/// (nil elements welcome, like the JDK).
#[derive(Debug)]
pub(crate) struct JavaList {
    id: u64,
    items: Mutex<Vec<Value>>,
}

/// An emulated `java.util.PriorityQueue`: identity, the heap
/// slots, and the optional comparator — a fn plus, for reified
/// comparators, the instance it closes over (`None` orders
/// naturally). Null elements NPE (like the JDK).
#[derive(Debug)]
pub(crate) struct JavaPQueue {
    id: u64,
    items: Mutex<Vec<Value>>,
    cmp: Option<Value>,
    this: Option<Value>,
}

macro_rules! java_coll_value {
    ($t:ty, $class:literal) => {
        impl ExtensionValue for $t {
            fn class_name(&self) -> String {
                $class.to_owned()
            }

            fn print(&self) -> String {
                format!(
                    "#object[{class} 0x{:x} \"{class}@{:x}\"]",
                    self.id,
                    self.id,
                    class = $class
                )
            }

            fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
                other
                    .as_any()
                    .downcast_ref::<Self>()
                    .is_some_and(|o| o.id == self.id)
            }

            fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
                state.write_u64(self.id);
            }

            fn as_any(&self) -> &dyn std::any::Any {
                self
            }
        }
    };
}

java_coll_value!(JavaDeque, "java.util.ArrayDeque");
java_coll_value!(JavaList, "java.util.ArrayList");
java_coll_value!(JavaPQueue, "java.util.PriorityQueue");

/// Downcast a deque value, if any.
pub(crate) fn as_deque(v: &Value) -> Option<&JavaDeque> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JavaDeque>(),
        _ => None,
    }
}

/// Downcast a list value, if any.
pub(crate) fn as_list(v: &Value) -> Option<&JavaList> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JavaList>(),
        _ => None,
    }
}

/// Downcast a priority-queue value, if any.
pub(crate) fn as_pqueue(v: &Value) -> Option<&JavaPQueue> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<JavaPQueue>(),
        _ => None,
    }
}

/// Lock a deque's ends (poison-tolerant, like every cell lock).
fn lock_deque(d: &JavaDeque) -> std::sync::MutexGuard<'_, VecDeque<Value>> {
    d.items.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lock a list's slots.
fn lock_list(l: &JavaList) -> std::sync::MutexGuard<'_, Vec<Value>> {
    l.items.lock().unwrap_or_else(|e| e.into_inner())
}

/// Lock a queue's slots.
fn lock_queue(q: &JavaPQueue) -> std::sync::MutexGuard<'_, Vec<Value>> {
    q.items.lock().unwrap_or_else(|e| e.into_inner())
}

/// The bare null-element NPE (`Objects.requireNonNull`, no message).
fn null_element() -> NativeError {
    NativeError::new(ThrowClass::NullPointerException, String::new())
}

/// Build `(ArrayDeque. …)`: 0-arg is empty, 1-arg collects a
/// collection (nil NPEs — the JDK's `(Collection)` overload —
/// anything unseqable takes the `ISeq` error).
pub(crate) fn build_deque(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    const CLASS: &str = "java.util.ArrayDeque";
    match args {
        [] => Ok(Value::Extension(Arc::new(JavaDeque {
            id: eval.next_id(),
            items: Mutex::new(VecDeque::new()),
        }))),
        [Value::Nil] => Err(null_element()),
        [coll] => {
            let items = crate::lazy::collect_all(eval, coll).map_err(crate::ns::signal_to_native)?;
            Ok(Value::Extension(Arc::new(JavaDeque {
                id: eval.next_id(),
                items: Mutex::new(items.into_iter().collect()),
            })))
        }
        _ => Err(crate::ctor::no_ctor(CLASS, false)),
    }
}

/// Build `(ArrayList. …)`: 0-arg is empty, 1-arg collects a
/// collection (nil NPEs, like the JDK).
pub(crate) fn build_list(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    const CLASS: &str = "java.util.ArrayList";
    match args {
        [] => Ok(Value::Extension(Arc::new(JavaList {
            id: eval.next_id(),
            items: Mutex::new(Vec::new()),
        }))),
        [Value::Nil] => Err(null_element()),
        [coll] => {
            let items = crate::lazy::collect_all(eval, coll).map_err(crate::ns::signal_to_native)?;
            Ok(Value::Extension(Arc::new(JavaList {
                id: eval.next_id(),
                items: Mutex::new(items),
            })))
        }
        _ => Err(crate::ctor::no_ctor(CLASS, false)),
    }
}

/// A host instance's 2-arity `compare` closure (`None` for
/// anything else — plain fns ride `is_fn_val` directly).
fn compare_method(v: &Value) -> Option<Value> {
    let Value::HostType(h) = v else {
        return None;
    };
    let i = h.as_inst()?;
    i.def
        .methods
        .get("compare")?
        .iter()
        .find(|m| m.arity == 2)
        .map(|m| m.func.clone())
}

/// Build `(PriorityQueue. …)`: 1-arg takes a `Comparator` (a
/// reified `compare` method or a plain fn) or a collection for
/// natural ordering (nil is ambiguous — `(Collection)` vs
/// `(Comparator)` — like the JVM).
pub(crate) fn build_pqueue(
    eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    const CLASS: &str = "java.util.PriorityQueue";
    match args {
        [Value::Nil] => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("More than one matching method found: {CLASS}"),
        )),
        [cmp] if crate::core::is_fn_val(cmp) => Ok(Value::Extension(Arc::new(JavaPQueue {
            id: eval.next_id(),
            items: Mutex::new(Vec::new()),
            cmp: Some(cmp.clone()),
            this: None,
        }))),
        [inst] if compare_method(inst).is_some() => {
            let func = compare_method(inst).expect("a compare closure");
            Ok(Value::Extension(Arc::new(JavaPQueue {
                id: eval.next_id(),
                items: Mutex::new(Vec::new()),
                cmp: Some(func),
                this: Some(inst.clone()),
            })))
        }
        [coll] => {
            let items = crate::lazy::collect_all(eval, coll).map_err(crate::ns::signal_to_native)?;
            Ok(Value::Extension(Arc::new(JavaPQueue {
                id: eval.next_id(),
                items: Mutex::new(items),
                cmp: None,
                this: None,
            })))
        }
        _ => Err(crate::ctor::no_ctor(CLASS, false)),
    }
}

/// The deque members (`None` for non-deques — the miss
/// shapes): ends add/poll/peek plus `add`/`poll`/`peek`
/// aliases and `isEmpty` (empty ends answer nil, like the
/// JDK).
pub(crate) fn deque_method(
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    let d = as_deque(target)?;
    match (meth, rest.len()) {
        ("add" | "addLast" | "offer" | "offerLast", 1) => {
            if matches!(&rest[0], Value::Nil) {
                return Some(Err(null_element()));
            }
            lock_deque(d).push_back(rest[0].clone());
            Some(Ok(Value::Bool(true)))
        }
        ("addFirst" | "offerFirst", 1) => {
            if matches!(&rest[0], Value::Nil) {
                return Some(Err(null_element()));
            }
            lock_deque(d).push_front(rest[0].clone());
            Some(Ok(Value::Bool(true)))
        }
        ("poll" | "pollFirst", 0) => Some(Ok(lock_deque(d).pop_front().unwrap_or(Value::Nil))),
        ("pollLast", 0) => Some(Ok(lock_deque(d).pop_back().unwrap_or(Value::Nil))),
        ("peek" | "peekFirst", 0) => Some(Ok(lock_deque(d).front().cloned().unwrap_or(Value::Nil))),
        ("peekLast", 0) => Some(Ok(lock_deque(d).back().cloned().unwrap_or(Value::Nil))),
        ("isEmpty", 0) => Some(Ok(Value::Bool(lock_deque(d).is_empty()))),
        _ => None,
    }
}

/// Interpret one `Comparator.compare` result (numbers order by
/// sign — the `compare` ints the suite's reified comparators
/// return — anything else fails to `Number`).
fn comparator_order(r: Value) -> Result<std::cmp::Ordering, NativeError> {
    match r {
        Value::Int(n) => Ok(n.cmp(&0)),
        Value::Double(d) => Ok(if d.trunc() > 0.0 {
            std::cmp::Ordering::Greater
        } else if d.trunc() < 0.0 {
            std::cmp::Ordering::Less
        } else {
            std::cmp::Ordering::Equal
        }),
        _ => Err(crate::core::cce_err(&r, "java.lang.Number")),
    }
}

/// One queue comparison: the natural ordering, a plain fn
/// over `(a b)`, or a reified closure over `(this a b)`.
fn pqueue_cmp(
    eval: &mut Evaluator,
    cmp: &Option<Value>,
    this: &Option<Value>,
    a: &Value,
    b: &Value,
) -> Result<std::cmp::Ordering, NativeError> {
    match cmp {
        None => crate::core::jvm_compare(a, b),
        Some(f) => {
            let r = match this {
                None => crate::seqs::call_checked(eval, f, &[a.clone(), b.clone()])?,
                Some(t) => {
                    crate::seqs::call_checked(eval, f, &[t.clone(), a.clone(), b.clone()])?
                }
            };
            comparator_order(r)
        }
    }
}

/// The least slot's index (linear scan — suite heaps are
/// small; a throwing comparator aborts before any removal).
fn pqueue_min(
    eval: &mut Evaluator,
    cmp: &Option<Value>,
    this: &Option<Value>,
    items: &[Value],
) -> Result<Option<usize>, NativeError> {
    let mut best: Option<usize> = None;
    for (i, v) in items.iter().enumerate() {
        let less = match best {
            None => true,
            Some(b) => pqueue_cmp(eval, cmp, this, v, &items[b])? == std::cmp::Ordering::Less,
        };
        if less {
            best = Some(i);
        }
    }
    Ok(best)
}

/// The queue members (`None` for non-queues): `add`/`offer`,
/// `poll`/`peek`, `isEmpty` (empty polls/peeks answer nil).
pub(crate) fn pqueue_method(
    eval: &mut Evaluator,
    meth: &str,
    target: &Value,
    rest: &[Value],
) -> Option<Result<Value, NativeError>> {
    let q = as_pqueue(target)?;
    match (meth, rest.len()) {
        ("add" | "offer", 1) => {
            if matches!(&rest[0], Value::Nil) {
                return Some(Err(null_element()));
            }
            lock_queue(q).push(rest[0].clone());
            Some(Ok(Value::Bool(true)))
        }
        ("poll", 0) => {
            let at = match pqueue_min(eval, &q.cmp, &q.this, &lock_queue(q)) {
                Ok(at) => at,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(at.map_or(Value::Nil, |i| lock_queue(q).remove(i))))
        }
        ("peek", 0) => {
            let guard = lock_queue(q);
            let at = match pqueue_min(eval, &q.cmp, &q.this, &guard) {
                Ok(at) => at,
                Err(e) => return Some(Err(e)),
            };
            Some(Ok(at.map_or(Value::Nil, |i| guard[i].clone())))
        }
        ("isEmpty", 0) => Some(Ok(Value::Bool(lock_queue(q).is_empty()))),
        _ => None,
    }
}

/// Snapshot a list's slots for `seq` (`None` for non-lists).
pub(crate) fn list_snapshot(v: &Value) -> Option<Vec<Value>> {
    as_list(v).map(|l| lock_list(l).clone())
}

/// `Collections/shuffle` (2-arg): the JDK Fisher-Yates in
/// place (`nextInt(i)` for `i` from the size down — seeded
/// streams match exactly). Nils NPE, mistypes fail their
/// casts, and the call answers nil (void).
pub(crate) fn shuffle_list(list: &Value, rnd: &Value) -> Result<Value, NativeError> {
    if matches!(list, Value::Nil) {
        return Err(null_element());
    }
    if matches!(rnd, Value::Nil) {
        return Err(null_element());
    }
    let Some(l) = as_list(list) else {
        return Err(crate::core::cce_err(list, "java.util.List"));
    };
    // A draw-free check: probing must not consume the stream
    // (seeded shuffles match the JDK draw-for-draw).
    if !crate::emurand::is_random(rnd) {
        return Err(crate::core::cce_err(rnd, "java.util.Random"));
    }
    let mut guard = lock_list(l);
    let len = guard.len();
    for i in (1..len).rev() {
        // Bounds run 2..=len — always positive, so never fail.
        let j = crate::emurand::random_next_int(rnd, (i + 1) as i32)
            .expect("an RNG draws")
            .expect("shuffle bounds are positive");
        guard.swap(i, j as usize);
    }
    Ok(Value::Nil)
}
