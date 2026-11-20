// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Integration coverage for the `#[no_mangle] extern "C"` `clj-bc` shims.
//!
//! Kept outside `cfg(test)` in `clj-bc` so `libtest` and `rlib` do not emit
//! colliding unmangled symbols during `llvm-cov`.

use std::mem::MaybeUninit;
use std::sync::Arc;
use std::sync::atomic::AtomicU64;

use clj_bc::{
    bc_fx_symbol, bc_ns_op_symbol, bc_op_symbol, rt_bc_1_bit_not, rt_bc_1_count, rt_bc_1_dec,
    rt_bc_1_double, rt_bc_1_empty_p, rt_bc_1_even_p, rt_bc_1_first, rt_bc_1_inc, rt_bc_1_int,
    rt_bc_1_long, rt_bc_1_neg, rt_bc_1_neg_p, rt_bc_1_nil_p, rt_bc_1_not, rt_bc_1_odd_p,
    rt_bc_1_peek, rt_bc_1_pop_bang, rt_bc_1_pop_bang_fx, rt_bc_1_pos_p, rt_bc_1_some_p,
    rt_bc_1_string_p, rt_bc_1_zero_p, rt_bc_2_add, rt_bc_2_bit_and, rt_bc_2_bit_or,
    rt_bc_2_bit_xor, rt_bc_2_conj_bang, rt_bc_2_conj_bang_fx, rt_bc_2_contains, rt_bc_2_eq,
    rt_bc_2_get, rt_bc_2_gt, rt_bc_2_gte, rt_bc_2_lt, rt_bc_2_lte, rt_bc_2_max, rt_bc_2_min,
    rt_bc_2_mod, rt_bc_2_mul, rt_bc_2_not_eq, rt_bc_2_nth, rt_bc_2_num_eq, rt_bc_2_quot,
    rt_bc_2_rem, rt_bc_2_shl, rt_bc_2_shr, rt_bc_2_str_ends_with, rt_bc_2_str_includes,
    rt_bc_2_str_starts_with, rt_bc_2_sub, rt_bc_2_ushr, rt_bc_3_assoc_bang, rt_bc_3_assoc_bang_fx,
    rt_bc_3_get, rt_bc_3_nth, rt_bc_clone, rt_bc_deref, rt_bc_is_pod, rt_bc_scalar_assoc,
    rt_bc_scalar_conj, rt_bc_scalar_count, rt_bc_scalar_gvec, rt_bc_scalar_ivec, rt_bc_scalar_nth,
    rt_bc_scalar_nth2, rt_bc_scalar_pop, rt_bc_scalar_same_transient, rt_bc_scalar_str,
    rt_bc_scalar_tvec_lock, rt_bc_scalar_tvec_unlock,
};
use clj_value::{BindCounts, FastVars, Transient, TransientKind, TransientVector, Value, Vector};

#[test]
fn abi_shims_exercise_pod_ops_and_fallbacks() {
    assert_eq!(rt_bc_is_pod(&Value::Nil), 1);
    assert_eq!(rt_bc_is_pod(&Value::Str(Arc::from("s"))), 0);

    let reg_epoch = Arc::new(AtomicU64::new(0));
    let bind_counts = Arc::new(BindCounts::new());
    let vars = FastVars::new(reg_epoch, bind_counts, 2);
    vars.record(0, 0, 7, &Value::Int(77));

    let mut raw = MaybeUninit::uninit();
    assert_eq!(rt_bc_clone(&mut raw, &Value::Int(3)), 1);
    assert_eq!(rt_bc_deref(&mut raw, &vars, 0), 1);
    assert_eq!(rt_bc_deref(&mut raw, &vars, 9), 0);

    extern "C" fn test_str_ascii(v: &Value) -> u8 {
        match v {
            Value::Str(s) if s.is_ascii() => 1,
            _ => 0,
        }
    }

    let empty = Value::Vector(Vector::new_sync());
    assert_eq!(
        rt_bc_3_nth(
            &mut raw,
            &empty,
            &Value::Int(5),
            &Value::Int(9),
            test_str_ascii
        ),
        1
    );

    let ops1: [(&str, extern "C" fn(&mut MaybeUninit<Value>, &Value) -> i8); 20] = [
        ("rt_bc_1_inc", rt_bc_1_inc),
        ("rt_bc_1_dec", rt_bc_1_dec),
        ("rt_bc_1_neg", rt_bc_1_neg),
        ("rt_bc_1_bit_not", rt_bc_1_bit_not),
        ("rt_bc_1_zero_p", rt_bc_1_zero_p),
        ("rt_bc_1_pos_p", rt_bc_1_pos_p),
        ("rt_bc_1_neg_p", rt_bc_1_neg_p),
        ("rt_bc_1_even_p", rt_bc_1_even_p),
        ("rt_bc_1_odd_p", rt_bc_1_odd_p),
        ("rt_bc_1_not", rt_bc_1_not),
        ("rt_bc_1_nil_p", rt_bc_1_nil_p),
        ("rt_bc_1_some_p", rt_bc_1_some_p),
        ("rt_bc_1_string_p", rt_bc_1_string_p),
        ("rt_bc_1_int", rt_bc_1_int),
        ("rt_bc_1_long", rt_bc_1_long),
        ("rt_bc_1_double", rt_bc_1_double),
        ("rt_bc_1_empty_p", rt_bc_1_empty_p),
        ("rt_bc_1_peek", rt_bc_1_peek),
        ("rt_bc_1_first", rt_bc_1_first),
        ("rt_bc_1_pop_bang", rt_bc_1_pop_bang),
    ];
    assert_eq!(bc_op_symbol("inc", 1), Some("rt_bc_1_inc"));
    assert_eq!(
        bc_ns_op_symbol(Some("clojure.string"), "starts-with?", 2),
        Some("rt_bc_2_str_starts_with")
    );
    for (_, f) in ops1 {
        assert!(matches!(f(&mut raw, &Value::Nil), 0 | 1));
    }
    assert_eq!(rt_bc_1_count(&mut raw, &empty, test_str_ascii), 1);

    let ops2: [(
        &str,
        extern "C" fn(&mut MaybeUninit<Value>, &Value, &Value) -> i8,
    ); 26] = [
        ("rt_bc_2_add", rt_bc_2_add),
        ("rt_bc_2_sub", rt_bc_2_sub),
        ("rt_bc_2_mul", rt_bc_2_mul),
        ("rt_bc_2_lt", rt_bc_2_lt),
        ("rt_bc_2_lte", rt_bc_2_lte),
        ("rt_bc_2_gt", rt_bc_2_gt),
        ("rt_bc_2_gte", rt_bc_2_gte),
        ("rt_bc_2_num_eq", rt_bc_2_num_eq),
        ("rt_bc_2_eq", rt_bc_2_eq),
        ("rt_bc_2_not_eq", rt_bc_2_not_eq),
        ("rt_bc_2_quot", rt_bc_2_quot),
        ("rt_bc_2_rem", rt_bc_2_rem),
        ("rt_bc_2_mod", rt_bc_2_mod),
        ("rt_bc_2_bit_and", rt_bc_2_bit_and),
        ("rt_bc_2_bit_or", rt_bc_2_bit_or),
        ("rt_bc_2_bit_xor", rt_bc_2_bit_xor),
        ("rt_bc_2_shl", rt_bc_2_shl),
        ("rt_bc_2_shr", rt_bc_2_shr),
        ("rt_bc_2_ushr", rt_bc_2_ushr),
        ("rt_bc_2_min", rt_bc_2_min),
        ("rt_bc_2_max", rt_bc_2_max),
        ("rt_bc_2_contains", rt_bc_2_contains),
        ("rt_bc_2_str_starts_with", rt_bc_2_str_starts_with),
        ("rt_bc_2_str_ends_with", rt_bc_2_str_ends_with),
        ("rt_bc_2_str_includes", rt_bc_2_str_includes),
        ("rt_bc_2_conj_bang", rt_bc_2_conj_bang),
    ];
    for (_, f) in ops2 {
        assert!(matches!(f(&mut raw, &Value::Int(1), &Value::Int(1)), 0 | 1));
    }
    assert_eq!(
        rt_bc_2_nth(&mut raw, &empty, &Value::Int(0), test_str_ascii),
        0
    );
    assert_eq!(
        rt_bc_2_get(&mut raw, &empty, &Value::Int(0), test_str_ascii),
        1
    );

    assert_eq!(
        rt_bc_3_assoc_bang(&mut raw, &Value::Nil, &Value::Nil, &Value::Nil),
        0
    );
    assert_eq!(
        rt_bc_3_get(
            &mut raw,
            &empty,
            &Value::Int(0),
            &Value::Int(9),
            test_str_ascii
        ),
        1
    );

    assert_eq!(bc_fx_symbol("pop!", 1), Some("rt_bc_1_pop_bang_fx"));
    assert_eq!(bc_fx_symbol("conj!", 2), Some("rt_bc_2_conj_bang_fx"));
    let mut fx_tv = TransientVector::new();
    assert!(fx_tv.conj_int_opt(1));
    let fx_tval = Value::Transient(Transient::new(TransientKind::Vector(fx_tv)));
    assert_eq!(rt_bc_2_conj_bang_fx(&mut raw, &fx_tval, &Value::Int(2)), 1);
    assert_eq!(
        rt_bc_3_assoc_bang_fx(&mut raw, &fx_tval, &Value::Int(0), &Value::Int(9)),
        1
    );
    assert_eq!(rt_bc_1_pop_bang_fx(&mut raw, &fx_tval), 1);
    assert_eq!(rt_bc_1_pop_bang_fx(&mut raw, &Value::Nil), 0);
    assert_eq!(
        rt_bc_3_assoc_bang_fx(&mut raw, &Value::Nil, &Value::Nil, &Value::Nil),
        0
    );

    let s = Value::Str(Arc::from("abc"));
    let mut ptr = std::ptr::null();
    let mut len = 0;
    assert_eq!(
        rt_bc_scalar_str(&s, &mut ptr, &mut len, Some(test_str_ascii)),
        1
    );
    assert_eq!(len, 3);
    assert_eq!(rt_bc_scalar_count(&s, &mut len, Some(test_str_ascii)), 1);
    let s_utf = Value::Str(Arc::from("é"));
    assert_eq!(
        rt_bc_scalar_count(&s_utf, &mut len, Some(test_str_ascii)),
        0
    );
    let mut k = 0;
    let mut val = 0;
    assert_eq!(
        rt_bc_scalar_nth(&s, 1, &mut k, &mut val, Some(test_str_ascii), 0),
        1
    );
    assert_eq!((k, val), (1, i64::from(b'b')));
    assert_eq!(rt_bc_scalar_conj(&fx_tval, 3), 1);
    assert_eq!(rt_bc_scalar_assoc(&fx_tval, 0, 4), 1);
    assert_eq!(rt_bc_scalar_pop(&fx_tval), 1);
    let nested = Value::Vector(Vector::from_vec(vec![Value::Vector(Vector::from_ints(
        vec![42],
    ))]));
    assert_eq!(rt_bc_scalar_nth2(&nested, 0, 0, &mut k, &mut val, 0), 1);
    assert_eq!((k, val), (0, 42));

    let iv = Value::Vector(Vector::from_ints(vec![10, 20, 30]));
    let mut iptr = std::ptr::null();
    assert_eq!(rt_bc_scalar_ivec(&iv, &mut iptr, &mut len), 1);
    assert_eq!(len, 3);
    assert_eq!(rt_bc_scalar_ivec(&s, &mut iptr, &mut len), 0);

    let gv_vec = Vector::from_vec(vec![Value::Double(1.25), Value::Double(2.5)]);
    let gv = Value::Vector(gv_vec.clone());
    let mut gptr = std::ptr::null();
    assert_eq!(rt_bc_scalar_gvec(&gv, &mut gptr, &mut len), 1);
    assert_eq!(len, 2);
    assert_eq!(rt_bc_scalar_gvec(&iv, &mut gptr, &mut len), 0);
    assert_eq!(rt_bc_scalar_gvec(&s, &mut gptr, &mut len), 0);

    let dbl_tv = Value::Transient(Transient::new(TransientKind::Vector(
        TransientVector::from_vector(&gv_vec),
    )));
    assert_eq!(
        rt_bc_3_assoc_bang_fx(&mut raw, &dbl_tv, &Value::Int(0), &Value::Double(9.5)),
        1
    );
    assert_eq!(
        rt_bc_3_assoc_bang_fx(&mut raw, &dbl_tv, &Value::Int(0), &Value::Bool(true)),
        0
    );

    assert_eq!(rt_bc_scalar_same_transient(&fx_tval, &fx_tval), 1);
    assert_eq!(rt_bc_scalar_same_transient(&fx_tval, &iv), 0);
    let mut mptr = std::ptr::null_mut();
    assert_eq!(rt_bc_scalar_tvec_lock(&fx_tval, &mut mptr, &mut len), 1);
    assert!(!mptr.is_null());
    rt_bc_scalar_tvec_unlock(&fx_tval);
    rt_bc_scalar_tvec_unlock(&iv);
}
