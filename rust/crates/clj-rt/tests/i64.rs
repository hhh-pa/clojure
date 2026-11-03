// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! The unboxed-i64 spike, called through the real FFI boundary (no
//! LLVM): guard/unbox pins, checked-step pins, interpreter
//! agreement, and a loop-carried sum proving the raw-i64 path end
//! to end at the helper level (codegen to emit it is the follow-up;
//! the timing prints to stderr — run with `-- --nocapture`).

mod support;

use clj_value::{Value, Vector, print_to_string};
use cljrt::{rt_add, rt_i64_add, rt_i64_mul, rt_i64_sub, rt_int, rt_is_int, rt_unbox_int};
use support::{inbox, render};

type Shim64 = unsafe extern "C" fn(i64, i64, *mut u8) -> i64;

/// One checked step through the FFI out-flag (value, flag).
fn flagged(shim: Shim64, a: i64, b: i64) -> (i64, u8) {
    let mut flag = 0u8;
    let value = unsafe { shim(a, b, &mut flag) };
    (value, flag)
}

#[test]
fn is_int_guards_every_shape() {
    let cases = [
        (Value::Int(0), 1),
        (Value::Int(i64::MAX), 1),
        (Value::Int(i64::MIN), 1),
        (Value::Str("1".into()), 0),
        (Value::Double(1.0), 0),
        (Value::Bool(true), 0),
        (Value::Nil, 0),
        (Value::Vector(Vector::from_vec(Vec::new())), 0),
    ];
    for (value, want) in cases {
        // The guard borrows — the box stays valid for `render`.
        let owned = inbox(value);
        assert_eq!(unsafe { rt_is_int(owned as *const Value) }, want);
        let _ = render(owned);
    }
}

#[test]
fn unbox_roundtrips_ints_and_degrades() {
    for n in [0, 1, -1, 42, i64::MAX, i64::MIN] {
        let owned = inbox(Value::Int(n));
        assert_eq!(unsafe { rt_unbox_int(owned as *const Value) }, n);
        let _ = render(owned);
    }
    // Non-`Int` is a caller bug (guards make it unreachable) and
    // degrades to `0`, the `rt_vec_push` precedent.
    for value in [Value::Str("x".into()), Value::Bool(true), Value::Nil] {
        let owned = inbox(value);
        assert_eq!(unsafe { rt_unbox_int(owned as *const Value) }, 0);
        let _ = render(owned);
    }
}

#[test]
fn i64_steps_pin_values_and_flags() {
    for (shim, cases) in [
        (
            rt_i64_add as Shim64,
            [(1, 2, 3, 0), (5, -8, -3, 0), (i64::MAX, 1, 0, 1)],
        ),
        (
            rt_i64_sub as Shim64,
            [(5, 8, -3, 0), (0, 0, 0, 0), (i64::MIN, 1, 0, 1)],
        ),
        (
            rt_i64_mul as Shim64,
            [(6, 7, 42, 0), (0, i64::MAX, 0, 0), (i64::MAX, 2, 0, 1)],
        ),
    ] {
        for (a, b, want, flag) in cases {
            assert_eq!(flagged(shim, a, b), (want, flag), "{a} {b}");
        }
    }
}

#[test]
fn i64_add_agrees_with_interpreter() {
    // Value agreement (flag-clear pairs): helper ≡ interpreter ≡
    // literal, the `prims.rs` convention.
    let mut eval = clj_eval::boot();
    for (a, b) in [(1, 2), (5, -8), (0, 0), (-3, -4)] {
        let (n, flag) = flagged(rt_i64_add as Shim64, a, b);
        assert_eq!(flag, 0);
        let literal = (a + b).to_string();
        assert_eq!(render(rt_int(n)), literal, "helper {a} {b}");
        let got: Vec<String> = eval
            .eval_source(&format!("(+ {a} {b})"))
            .expect("interp adds")
            .iter()
            .map(print_to_string)
            .collect();
        assert_eq!(got.join(" "), literal, "interp {a} {b}");
    }
}

#[test]
fn unboxed_loop_carries_i64() {
    // The spike's end-to-end shape at helper level: a loop-carried
    // sum over raw `i64` (what unboxed codegen emits per step)
    // against the boxed `rt_add` loop it replaces.
    use std::hint::black_box;
    use std::time::Instant;
    let n = 1_000_000i64;
    let m = 20_000i64;
    let path = b"f.clj";
    // Warmth (outside the timers): branch/allocator steady state,
    // plus the runtime's one-time boot before the boxed loop.
    let mut flag = 0u8;
    for i in 1..=10_000 {
        black_box(unsafe { rt_i64_add(black_box(i), black_box(1), &mut flag) });
    }
    let argv = [inbox(Value::Int(0)), inbox(Value::Int(1))];
    let warm = unsafe { rt_add(argv.as_ptr(), 2, path.as_ptr(), path.len(), 3, 9) };
    assert_eq!(render(warm), "1");
    let mut acc = 0i64;
    let start = Instant::now();
    for i in 1..=n {
        acc = unsafe { rt_i64_add(acc, black_box(i), &mut flag) };
        assert_eq!(flag, 0);
    }
    let unboxed = start.elapsed();
    assert_eq!(acc, n * (n + 1) / 2);
    let mut boxed = inbox(Value::Int(0));
    let start = Instant::now();
    for i in 1..=m {
        let argv = [boxed, inbox(Value::Int(black_box(i)))];
        boxed = unsafe { rt_add(argv.as_ptr(), 2, path.as_ptr(), path.len(), 3, 9) };
        assert!(!boxed.is_null());
    }
    let over_boxed = start.elapsed();
    assert_eq!(render(boxed), (m * (m + 1) / 2).to_string());
    eprintln!(
        "clj-rt unboxed i64 add loop: {:.1} ns/iter",
        unboxed.as_secs_f64() * 1e9 / n as f64
    );
    eprintln!(
        "clj-rt boxed rt_add loop: {:.1} ns/iter",
        over_boxed.as_secs_f64() * 1e9 / m as f64
    );
}
