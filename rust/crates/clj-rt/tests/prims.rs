// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Primitive helpers, called through the real FFI boundary (no
//! LLVM): every case runs BOTH the helper and the interpreter on
//! the same form — the renders must match each other AND the
//! pinned literal (errors render as `ERROR` on both sides; the
//! exact stderr bytes are pinned by the S8D corpus replay).

mod support;

use clj_eval::{Evaluator, Prim, boot};
use clj_value::{
    Keyword, Transient, TransientKind, TransientMap, TransientVector, Value, Vector, new_map,
    print_to_string,
};
use cljrt::{
    prim_symbol, rt_add, rt_assoc_bang, rt_conj, rt_conj_bang, rt_count, rt_drop, rt_eq, rt_ge,
    rt_get, rt_gt, rt_i64_add, rt_i64_mul, rt_i64_sub, rt_int_cast, rt_le, rt_long, rt_lt, rt_mul,
    rt_nth, rt_persistent_bang, rt_quot, rt_sub, rt_transient,
};
use support::{inbox, render};

type Shim =
    unsafe extern "C" fn(*const *mut Value, usize, *const u8, usize, u32, u32) -> *mut Value;

type Shim64 = unsafe extern "C" fn(i64, i64, *mut u8) -> i64;

/// Call a helper exactly like generated code does: the one
/// unsafe block every `ffi*` wrapper funnels through (the
/// `path:line:col:` prefix is pinned by the `error_line` unit
/// tests and the S8D replay).
fn raw_call(shim: Shim, boxes: &[*mut Value]) -> *mut Value {
    let path = b"f.clj";
    unsafe { shim(boxes.as_ptr(), boxes.len(), path.as_ptr(), path.len(), 3, 9) }
}

/// Call a helper exactly like generated code does (error renders
/// as `ERROR`).
fn ffi(shim: Shim, args: Vec<Value>) -> String {
    let boxes: Vec<*mut Value> = args.into_iter().map(inbox).collect();
    let out = raw_call(shim, &boxes);
    if out.is_null() {
        "ERROR".to_owned()
    } else {
        render(out)
    }
}

/// Evaluate the same form in the interpreter (error → `ERROR`).
fn interp(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Ok(values) => values
            .iter()
            .map(print_to_string)
            .collect::<Vec<_>>()
            .join(" "),
        Err(_) => "ERROR".to_owned(),
    }
}

/// Pin one case three ways: helper ≡ interpreter ≡ literal.
fn check(eval: &mut Evaluator, shim: Shim, args: Vec<Value>, src: &str, want: &str) {
    assert_eq!(ffi(shim, args), want, "{src} via helper");
    assert_eq!(interp(eval, src), want, "{src} via interpreter");
}

/// Call two helpers in sequence like generated code does
/// (`first`'s owned return becomes `second`'s single arg).
fn ffi_chain(first: Shim, args: Vec<Value>, second: Shim) -> String {
    let boxes: Vec<*mut Value> = args.into_iter().map(inbox).collect();
    let mid = raw_call(first, &boxes);
    if mid.is_null() {
        return "ERROR".to_owned();
    }
    let mids = [mid];
    let out = raw_call(second, &mids);
    if out.is_null() {
        "ERROR".to_owned()
    } else {
        render(out)
    }
}

/// Pin one two-helper case three ways (transients print with
/// identity, so success paths freeze through `persistent!`).
fn check_chain(
    eval: &mut Evaluator,
    first: Shim,
    args: Vec<Value>,
    second: Shim,
    src: &str,
    want: &str,
) {
    assert_eq!(ffi_chain(first, args, second), want, "{src} via helpers");
    assert_eq!(interp(eval, src), want, "{src} via interpreter");
}

fn time(label: &str, n: usize, mut step: impl FnMut()) {
    use std::time::Instant;
    for _ in 0..n {
        step();
    }
    // Best of 5 (noise only ever adds).
    let mut best = f64::INFINITY;
    for _ in 0..5 {
        let start = Instant::now();
        for _ in 0..n {
            step();
        }
        best = best.min(start.elapsed().as_secs_f64() * 1e9 / n as f64);
    }
    eprintln!("clj-rt {label}: best-of-5 {best:.1} ns/call");
}

// The lean call (no render alloc): box, shim, free — the
// generated-code shape, so the delta is the shim's own.
fn ffi_drop(shim: Shim, args: Vec<Value>) {
    let boxes: Vec<*mut Value> = args.into_iter().map(inbox).collect();
    let out = raw_call(shim, &boxes);
    assert!(!out.is_null());
    unsafe { rt_drop(out) };
}

// Interleaved A/B: alternating batches share each load
// window, and the median over rounds rejects migrated or
// preempted batches, so the pair stays comparable on a busy
// machine.
fn compare(
    a_label: &str,
    b_label: &str,
    rounds: usize,
    n: usize,
    mut a: impl FnMut(),
    mut b: impl FnMut(),
) {
    use std::time::Instant;
    for _ in 0..n {
        a();
        b();
    }
    let (mut tas, mut tbs) = (Vec::with_capacity(rounds), Vec::with_capacity(rounds));
    for _ in 0..rounds {
        let start = Instant::now();
        for _ in 0..n {
            a();
        }
        tas.push(start.elapsed().as_secs_f64());
        let start = Instant::now();
        for _ in 0..n {
            b();
        }
        tbs.push(start.elapsed().as_secs_f64());
    }
    tas.sort_by(f64::total_cmp);
    tbs.sort_by(f64::total_cmp);
    let span = n as f64;
    eprintln!(
        "clj-rt {a_label}: median-of-{rounds} {:.1} ns/call",
        tas[rounds / 2] * 1e9 / span
    );
    eprintln!(
        "clj-rt {b_label}: median-of-{rounds} {:.1} ns/call",
        tbs[rounds / 2] * 1e9 / span
    );
}

fn int(n: i64) -> Value {
    Value::Int(n)
}

fn str_(s: &str) -> Value {
    Value::Str(s.into())
}

fn vec_of(items: Vec<Value>) -> Value {
    Value::Vector(Vector::from_vec(items))
}

/// A fresh vector transient cell (shared by the value builders).
fn transient_vec_cell(items: Vec<Value>) -> Transient {
    Transient::new(TransientKind::Vector(TransientVector::from_vector(
        &Vector::from_vec(items),
    )))
}

fn transient_vec(items: Vec<Value>) -> Value {
    Value::Transient(transient_vec_cell(items))
}

fn transient_map(pairs: Vec<(Value, Value)>) -> Value {
    let mut map = new_map();
    for (k, v) in pairs {
        map = map.insert(k, v);
    }
    Value::Transient(Transient::with_meta(
        TransientKind::Map(TransientMap::from_map(&map)),
        None,
    ))
}

/// A transient already spent by `persistent!` (any further use errors).
fn spent_transient_vec(items: Vec<Value>) -> Value {
    let t = transient_vec_cell(items);
    {
        let mut guard = t.lock();
        if let TransientKind::Vector(tv) = &mut *guard {
            tv.persistent().expect("fresh transient persists");
        }
    }
    Value::Transient(t)
}

#[test]
fn prim_symbol_names_each_helper() {
    let pairs = [
        (Prim::Add, "rt_add"),
        (Prim::Sub, "rt_sub"),
        (Prim::Mul, "rt_mul"),
        (Prim::Quot, "rt_quot"),
        (Prim::Lt, "rt_lt"),
        (Prim::Le, "rt_le"),
        (Prim::Gt, "rt_gt"),
        (Prim::Ge, "rt_ge"),
        (Prim::Eq, "rt_eq"),
        (Prim::Count, "rt_count"),
        (Prim::Nth, "rt_nth"),
        (Prim::Get, "rt_get"),
        (Prim::Conj, "rt_conj"),
        (Prim::Long, "rt_long"),
        // `rt_int` is the integer-literal constructor; the `int`
        // cast shim takes the suffixed symbol.
        (Prim::Int, "rt_int_cast"),
        (Prim::Transient, "rt_transient"),
        (Prim::ConjBang, "rt_conj_bang"),
        (Prim::AssocBang, "rt_assoc_bang"),
        (Prim::PersistentBang, "rt_persistent_bang"),
    ];
    for (prim, symbol) in pairs {
        assert_eq!(prim_symbol(prim), symbol);
    }
}

#[test]
fn add_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_add,
        vec![int(1), int(2), int(3)],
        "(+ 1 2 3)",
        "6",
    );
    check(&mut eval, rt_add, vec![], "(+)", "0");
    check(&mut eval, rt_add, vec![str_("s")], "(+ \"s\")", "s");
}

#[test]
fn sub_delegates() {
    let mut eval = boot();
    check(&mut eval, rt_sub, vec![int(5), int(8)], "(- 5 8)", "-3");
    check(&mut eval, rt_sub, vec![int(5)], "(- 5)", "-5");
    check(&mut eval, rt_sub, vec![], "(-)", "ERROR");
    check(&mut eval, rt_sub, vec![str_("s")], "(- \"s\")", "ERROR");
}

#[test]
fn mul_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_mul,
        vec![int(2), int(3), int(4)],
        "(* 2 3 4)",
        "24",
    );
    check(&mut eval, rt_mul, vec![], "(*)", "1");
    check(&mut eval, rt_mul, vec![str_("s")], "(* \"s\")", "s");
}

#[test]
fn quot_delegates() {
    let mut eval = boot();
    check(&mut eval, rt_quot, vec![int(7), int(2)], "(quot 7 2)", "3");
    check(
        &mut eval,
        rt_quot,
        vec![int(1), int(0)],
        "(quot 1 0)",
        "ERROR",
    );
}

#[test]
fn comparisons_delegate_with_early_exit() {
    let mut eval = boot();
    check(&mut eval, rt_lt, vec![int(1), int(2)], "(< 1 2)", "true");
    check(&mut eval, rt_lt, vec![str_("s")], "(< \"s\")", "true");
    check(
        &mut eval,
        rt_lt,
        vec![int(2), int(1), str_("s")],
        "(< 2 1 \"s\")",
        "false",
    );
    check(
        &mut eval,
        rt_le,
        vec![int(1), int(1), int(2)],
        "(<= 1 1 2)",
        "true",
    );
    check(
        &mut eval,
        rt_gt,
        vec![int(3), int(2), int(1)],
        "(> 3 2 1)",
        "true",
    );
    check(
        &mut eval,
        rt_ge,
        vec![int(2), int(2), int(1)],
        "(>= 2 2 1)",
        "true",
    );
    check(&mut eval, rt_lt, vec![], "(<)", "ERROR");
}

#[test]
fn eq_compares_by_category() {
    let mut eval = boot();
    check(&mut eval, rt_eq, vec![int(1), int(1)], "(= 1 1)", "true");
    check(
        &mut eval,
        rt_eq,
        vec![int(1), Value::Double(1.0)],
        "(= 1 1.0)",
        "false",
    );
    check(&mut eval, rt_eq, vec![], "(=)", "ERROR");
}

#[test]
fn count_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_count,
        vec![str_("hello")],
        "(count \"hello\")",
        "5",
    );
    check(
        &mut eval,
        rt_count,
        vec![vec_of(vec![int(1), int(2)])],
        "(count [1 2])",
        "2",
    );
    check(&mut eval, rt_count, vec![int(1)], "(count 1)", "ERROR");
}

#[test]
fn count_str_fast_path_matches() {
    // Every string arg takes the native fast path (no mutex, no
    // evaluator); UTF-16 units, astral chars count 2.
    let mut eval = boot();
    check(&mut eval, rt_count, vec![str_("")], "(count \"\")", "0");
    check(
        &mut eval,
        rt_count,
        vec![str_("héllo")],
        "(count \"héllo\")",
        "5",
    );
    check(&mut eval, rt_count, vec![str_("𝄞")], "(count \"𝄞\")", "2");
    check(
        &mut eval,
        rt_count,
        vec![str_("a𝄞b")],
        "(count \"a𝄞b\")",
        "4",
    );
    check(&mut eval, rt_count, vec![Value::Nil], "(count nil)", "0");
    check(&mut eval, rt_count, vec![], "(count)", "ERROR");
}

#[test]
fn nth_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_nth,
        vec![vec_of(vec![int(10), int(20)]), int(1)],
        "(nth [10 20] 1)",
        "20",
    );
    check(
        &mut eval,
        rt_nth,
        vec![vec_of(vec![]), int(5)],
        "(nth [] 5)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_nth,
        vec![transient_vec(vec![int(10), int(20)]), int(1)],
        "(nth (transient [10 20]) 1)",
        "20",
    );
}

#[test]
fn nth_str_fast_path_matches() {
    // Every string collection takes the native fast path (the
    // index still coerces first through the shared function).
    let mut eval = boot();
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(0)],
        "(nth \"hello\" 0)",
        "h",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(1)],
        "(nth \"hello\" 1)",
        "e",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(4)],
        "(nth \"hello\" 4)",
        "o",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), Value::Double(1.9)],
        "(nth \"hello\" 1.9)",
        "e",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("héllo"), int(1)],
        "(nth \"héllo\" 1)",
        "é",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("𝄞x"), int(0)],
        "(nth \"𝄞x\" 0)",
        "𝄞",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(9)],
        "(nth \"hello\" 9)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(-1)],
        "(nth \"hello\" -1)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), Value::Nil],
        "(nth \"hello\" nil)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello"), int(9), str_("nf")],
        "(nth \"hello\" 9 \"nf\")",
        "nf",
    );
    check(
        &mut eval,
        rt_nth,
        vec![
            str_("hello"),
            int(-1),
            Value::Keyword(Keyword::new(None, "d")),
        ],
        "(nth \"hello\" -1 :d)",
        ":d",
    );
    check(
        &mut eval,
        rt_nth,
        vec![Value::Nil, int(0)],
        "(nth nil 0)",
        "nil",
    );
    check(
        &mut eval,
        rt_nth,
        vec![str_("hello")],
        "(nth \"hello\")",
        "ERROR",
    );
}

#[test]
fn i64_overflow_flag_matches_boxed_error() {
    // The unboxed spike flags exactly the pairs the boxed helper
    // (and the interpreter) reject with `long overflow`.
    let mut eval = boot();
    for (shim, op, a, b) in [
        (rt_i64_add as Shim64, "+", i64::MAX, 1_i64),
        (rt_i64_add as Shim64, "+", i64::MIN, -1),
        (rt_i64_sub as Shim64, "-", i64::MIN, 1),
        (rt_i64_sub as Shim64, "-", i64::MAX, -1),
        (rt_i64_mul as Shim64, "*", i64::MAX, 2),
        (rt_i64_mul as Shim64, "*", i64::MIN, -1),
    ] {
        let mut flag = 0u8;
        let slot = unsafe { shim(a, b, &mut flag) };
        assert_eq!((slot, flag), (0, 1), "({op} {a} {b}) flags");
        let (boxed, src) = match op {
            "+" => (rt_add as Shim, format!("(+ {a} {b})")),
            "-" => (rt_sub as Shim, format!("(- {a} {b})")),
            _ => (rt_mul as Shim, format!("(* {a} {b})")),
        };
        check(&mut eval, boxed, vec![int(a), int(b)], &src, "ERROR");
    }
}

#[test]
fn string_fast_paths_report_costs() {
    // Per-call delegation breakdown (numbers to stderr; run with
    // `-- --nocapture`): each shim does identical O(1) native work
    // on both sides, so the string/vector delta is the delegation
    // itself (mutex + `Arc` + `Vec` + evaluator dispatch).
    use std::hint::black_box;
    use std::sync::Arc;
    let big: String = "abcdefgh".repeat(128 * 1024);
    assert_eq!(big.len(), 1_048_576);
    // One shared string (each call clones the `Arc`, like the
    // generated `rt_clone` — never the bytes).
    let big_value = Value::Str(Arc::from(big.as_str()));
    let items: Vec<Value> = (0..131_072).map(int).collect();
    let vec = vec_of(items);
    // Warmth: boot the runtime, build the scan cache entry.
    assert_eq!(ffi(rt_count, vec![big_value.clone()]), "1048576");
    assert_eq!(ffi(rt_count, vec![vec.clone()]), "131072");
    assert_eq!(ffi(rt_nth, vec![big_value.clone(), int(900_000)]), "a");
    assert_eq!(ffi(rt_nth, vec![vec.clone(), int(90_000)]), "90000");
    compare(
        "rt_count(str 1MB, fast)",
        "rt_count(vec 128K, delegated)",
        50,
        2_000,
        || ffi_drop(rt_count, vec![black_box(big_value.clone())]),
        || ffi_drop(rt_count, vec![black_box(vec.clone())]),
    );
    compare(
        "rt_nth(str idx 900K, fast)",
        "rt_nth(vec idx 90K, delegated)",
        50,
        2_000,
        || {
            ffi_drop(rt_nth, vec![black_box(big_value.clone()), int(900_000)]);
        },
        || ffi_drop(rt_nth, vec![black_box(vec.clone()), int(90_000)]),
    );
    // Fairness check: the delegated baseline clones this per call.
    time("Vector(128K) clone", 100_000, || {
        black_box(black_box(vec.clone()));
    });
    // Delegation components in isolation (same process shape).
    let lock = std::sync::Mutex::new(0u64);
    time("Mutex lock/unlock", 200_000, || {
        *black_box(lock.lock().expect("bench lock")) += 1;
    });
    time("Box alloc + free (Int)", 200_000, || {
        unsafe { rt_drop(black_box(inbox(int(1)))) };
    });
    time("Vec alloc 2 Values + free", 200_000, || {
        black_box(vec![int(1), int(2)]);
    });
}

#[test]
fn get_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_get,
        vec![vec_of(vec![int(7), int(8)]), int(1)],
        "(get [7 8] 1)",
        "8",
    );
    check(
        &mut eval,
        rt_get,
        vec![vec_of(vec![int(7)]), int(5)],
        "(get [7] 5)",
        "nil",
    );
    check(
        &mut eval,
        rt_get,
        vec![
            vec_of(vec![int(7)]),
            int(5),
            Value::Keyword(Keyword::new(None, "d")),
        ],
        "(get [7] 5 :d)",
        ":d",
    );
}

#[test]
fn conj_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_conj,
        vec![vec_of(vec![int(1)]), int(2), int(3)],
        "(conj [1] 2 3)",
        "[1 2 3]",
    );
    check(
        &mut eval,
        rt_conj,
        vec![Value::Nil, int(1)],
        "(conj nil 1)",
        "(1)",
    );
}

#[test]
fn long_delegates() {
    let mut eval = boot();
    check(&mut eval, rt_long, vec![int(5)], "(long 5)", "5");
    check(
        &mut eval,
        rt_long,
        vec![Value::Double(5.9)],
        "(long 5.9)",
        "5",
    );
    check(
        &mut eval,
        rt_long,
        vec![Value::Char('A')],
        "(long \\A)",
        "65",
    );
    check(&mut eval, rt_long, vec![Value::Nil], "(long nil)", "ERROR");
    check(&mut eval, rt_long, vec![str_("s")], "(long \"s\")", "ERROR");
    check(&mut eval, rt_long, vec![], "(long)", "ERROR");
}

#[test]
fn int_delegates() {
    let mut eval = boot();
    check(&mut eval, rt_int_cast, vec![int(7)], "(int 7)", "7");
    check(
        &mut eval,
        rt_int_cast,
        vec![Value::Double(7.9)],
        "(int 7.9)",
        "7",
    );
    check(
        &mut eval,
        rt_int_cast,
        vec![Value::Nil],
        "(int nil)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_int_cast,
        vec![str_("s")],
        "(int \"s\")",
        "ERROR",
    );
    check(
        &mut eval,
        rt_int_cast,
        vec![int(1), int(2)],
        "(int 1 2)",
        "ERROR",
    );
}

#[test]
fn transient_delegates() {
    let mut eval = boot();
    check_chain(
        &mut eval,
        rt_transient,
        vec![vec_of(vec![int(1), int(2)])],
        rt_persistent_bang,
        "(persistent! (transient [1 2]))",
        "[1 2]",
    );
    check(
        &mut eval,
        rt_transient,
        vec![int(1)],
        "(transient 1)",
        "ERROR",
    );
    check(&mut eval, rt_transient, vec![], "(transient)", "ERROR");
}

#[test]
fn conj_bang_delegates() {
    let mut eval = boot();
    check_chain(
        &mut eval,
        rt_conj_bang,
        vec![transient_vec(vec![int(1)]), int(2), int(3)],
        rt_persistent_bang,
        "(persistent! (conj! (transient [1]) 2 3))",
        "[1 2 3]",
    );
    check(
        &mut eval,
        rt_conj_bang,
        vec![vec_of(vec![int(1)]), int(2)],
        "(conj! [1] 2)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_conj_bang,
        vec![spent_transient_vec(vec![int(1)]), int(2)],
        "(let [t (transient [1])] (do (persistent! t) (conj! t 2)))",
        "ERROR",
    );
}

#[test]
fn assoc_bang_delegates() {
    let mut eval = boot();
    check_chain(
        &mut eval,
        rt_assoc_bang,
        vec![transient_vec(vec![int(1), int(2)]), int(0), int(9)],
        rt_persistent_bang,
        "(persistent! (assoc! (transient [1 2]) 0 9))",
        "[9 2]",
    );
    check_chain(
        &mut eval,
        rt_assoc_bang,
        vec![
            transient_map(Vec::new()),
            Value::Keyword(Keyword::new(None, "b")),
            int(2),
        ],
        rt_persistent_bang,
        "(persistent! (assoc! (transient {}) :b 2))",
        "{:b 2}",
    );
    check(
        &mut eval,
        rt_assoc_bang,
        vec![vec_of(vec![int(1)]), int(0), int(9)],
        "(assoc! [1] 0 9)",
        "ERROR",
    );
    check(
        &mut eval,
        rt_assoc_bang,
        vec![spent_transient_vec(vec![int(1)]), int(0), int(9)],
        "(let [t (transient [1])] (do (persistent! t) (assoc! t 0 9)))",
        "ERROR",
    );
}

#[test]
fn persistent_bang_delegates() {
    let mut eval = boot();
    check(
        &mut eval,
        rt_persistent_bang,
        vec![transient_vec(vec![int(1), int(2)])],
        "(persistent! (transient [1 2]))",
        "[1 2]",
    );
    check(
        &mut eval,
        rt_persistent_bang,
        vec![vec_of(vec![int(1)])],
        "(persistent! [1])",
        "ERROR",
    );
    check(
        &mut eval,
        rt_persistent_bang,
        vec![spent_transient_vec(vec![int(1)])],
        "(let [t (transient [1])] (do (persistent! t) (persistent! t)))",
        "ERROR",
    );
}
