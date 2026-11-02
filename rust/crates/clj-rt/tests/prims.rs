// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

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
    prim_symbol, rt_add, rt_assoc_bang, rt_conj, rt_conj_bang, rt_count, rt_eq, rt_ge, rt_get,
    rt_gt, rt_int_cast, rt_le, rt_long, rt_lt, rt_mul, rt_nth, rt_persistent_bang, rt_quot, rt_sub,
    rt_transient,
};
use support::{inbox, render};

type Shim =
    unsafe extern "C" fn(*const *mut Value, usize, *const u8, usize, u32, u32) -> *mut Value;

/// Call a helper exactly like generated code does (error renders
/// as `ERROR`; the `path:line:col:` prefix is pinned by the
/// `error_line` unit tests and the S8D replay).
fn ffi(shim: Shim, args: Vec<Value>) -> String {
    let boxes: Vec<*mut Value> = args.into_iter().map(inbox).collect();
    let path = b"f.clj";
    let out = unsafe { shim(boxes.as_ptr(), boxes.len(), path.as_ptr(), path.len(), 3, 9) };
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
    let path = b"f.clj";
    let mid = unsafe { first(boxes.as_ptr(), boxes.len(), path.as_ptr(), path.len(), 3, 9) };
    if mid.is_null() {
        return "ERROR".to_owned();
    }
    let mids = [mid];
    let out = unsafe { second(mids.as_ptr(), mids.len(), path.as_ptr(), path.len(), 3, 9) };
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
