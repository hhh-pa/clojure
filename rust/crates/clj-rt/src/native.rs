// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Runtime of natively compiled programs (issue #259), the safe half.
//!
//! Generated code holds `Value`s by value in 32-byte slots and calls
//! the `extern "C"` shims in [`crate::abi`] (`rt_n_*`), which convert
//! raw slots and delegate here. Everything here is safe Rust over
//! owned values and the active evaluator ([`crate::runtime::with_active`]).
//!
//! Program tables (vars, constants, fn descriptors) are declared by
//! the generated `main` before the program runs and frozen after
//! constant construction, so the hot path reads them lock-free.
//!
//! Errors: a failing step stores its `Signal` in the thread's pending
//! slot and the shim returns 1; generated code propagates the 1 to
//! its caller, and the boundary back into the interpreter (closure
//! calls, the program entry) takes the pending signal again.

use std::cell::RefCell;
use std::sync::{Arc, Mutex, OnceLock};

use clj_eval::native_abi::{self, Step};
use clj_eval::{Evaluator, NativeError, NativeFn, Signal};
use clj_value::{Value, Var};

use crate::runtime::{ActiveScope, with_active};

/// A compiled arity: `(ret, args, argc, captured) -> status`.
pub type Code = extern "C" fn(*mut Value, *const Value, usize, *const Value) -> i32;

/// One arity of a compiled fn.
#[derive(Clone, Copy)]
pub(crate) struct ArityDesc {
    pub(crate) fixed: usize,
    pub(crate) variadic: bool,
    pub(crate) code: Code,
}

/// A compiled fn's static description.
#[derive(Clone)]
pub(crate) struct FnDesc {
    pub(crate) display: Arc<str>,
    pub(crate) arities: Arc<[ArityDesc]>,
}

/// One declared var: its qualified name, interned on first use.
pub(crate) struct VarSlot {
    ns: String,
    name: String,
    var: OnceLock<Var>,
}

/// Tables under construction (startup only).
#[derive(Default)]
struct Building {
    vars: Vec<VarSlot>,
    consts: Vec<Value>,
    fns: Vec<Option<FnDesc>>,
}

static BUILDING: Mutex<Option<Building>> = Mutex::new(None);

/// The frozen program tables.
pub(crate) struct Tables {
    vars: Box<[VarSlot]>,
    consts: Box<[Value]>,
    fns: Box<[FnDesc]>,
}

static TABLES: OnceLock<Tables> = OnceLock::new();

thread_local! {
    /// The throw a failing step left for generated code to propagate.
    static PENDING: RefCell<Option<Signal>> = const { RefCell::new(None) };
}

fn building<R>(f: impl FnOnce(&mut Building) -> R) -> R {
    let mut guard = BUILDING.lock().expect("clj-rt native tables lock");
    f(guard.get_or_insert_with(Building::default))
}

fn tables() -> &'static Tables {
    TABLES
        .get()
        .expect("native program tables frozen before use")
}

/// Store a failing step's signal; returns the shim status (1).
pub(crate) fn fail(signal: Signal) -> i32 {
    PENDING.with(|p| *p.borrow_mut() = Some(signal));
    1
}

/// Take the pending signal (a missing one is an internal error).
pub(crate) fn take_pending() -> Signal {
    PENDING.with(|p| p.borrow_mut().take()).unwrap_or_else(|| {
        with_active(|eval| {
            native_abi::native_throw(
                eval,
                NativeError::new(
                    clj_value::ThrowClass::Error,
                    "internal error: native code failed without a pending throw".to_owned(),
                ),
            )
        })
    })
}

/// A step's outcome as `(status, value)`.
pub(crate) fn settle(step: Step) -> (i32, Value) {
    match step {
        Ok(v) => (0, v),
        Err(s) => (fail(s), Value::Nil),
    }
}

/// Declare var `k` (`ns/name`).
pub(crate) fn declare_var(k: usize, ns: String, name: String) {
    building(|b| {
        if b.vars.len() <= k {
            b.vars.resize_with(k + 1, || VarSlot {
                ns: String::new(),
                name: String::new(),
                var: OnceLock::new(),
            });
        }
        b.vars[k] = VarSlot {
            ns,
            name,
            var: OnceLock::new(),
        };
    });
}

/// Declare fn descriptor `k`.
pub(crate) fn declare_fn(k: usize, desc: FnDesc) {
    building(|b| {
        if b.fns.len() <= k {
            b.fns.resize(k + 1, None);
        }
        b.fns[k] = Some(desc);
    });
}

/// Store constant `k` (constant construction, before freezing).
pub(crate) fn set_const(k: usize, value: Value) {
    building(|b| {
        if b.consts.len() <= k {
            b.consts.resize(k + 1, Value::Nil);
        }
        b.consts[k] = value;
    });
}

/// Freeze the tables (after constant construction).
pub(crate) fn freeze() {
    let b = building(std::mem::take);
    let fns: Vec<FnDesc> = b
        .fns
        .into_iter()
        .map(|d| d.expect("every declared fn index is filled"))
        .collect();
    let _ = TABLES.set(Tables {
        vars: b.vars.into_boxed_slice(),
        consts: b.consts.into_boxed_slice(),
        fns: fns.into_boxed_slice(),
    });
}

/// Constant `k`.
pub(crate) fn constant(k: usize) -> &'static Value {
    &tables().consts[k]
}

/// Var `k`, interned on first use (in the then-current registry).
pub(crate) fn var(k: usize) -> Var {
    let slot = &tables().vars[k];
    slot.var
        .get_or_init(|| with_active(|eval| native_abi::intern_var(eval, &slot.ns, &slot.name)))
        .clone()
}

/// A closure over compiled fn `k` with its captured values: a
/// `NativeFn` the interpreter calls like any fn (arity dispatch and
/// errors as for interpreted closures).
pub(crate) fn make_fn(eval: &mut Evaluator, k: usize, captured: Vec<Value>) -> Value {
    let desc = tables().fns[k].clone();
    let display = Arc::clone(&desc.display);
    let captured: Arc<[Value]> = captured.into();
    let func = Arc::new(move |eval: &mut Evaluator, args: &[Value]| {
        call_compiled(eval, &desc, &captured, args).map_err(native_abi::signal_to_native)
    });
    Value::Extension(Arc::new(NativeFn {
        id: eval.next_id(),
        display,
        func,
        owned: None,
        meta: None,
        fast: Default::default(),
    }))
}

/// Select the arity for `args` and run its code on this evaluator.
fn call_compiled(eval: &mut Evaluator, desc: &FnDesc, captured: &[Value], args: &[Value]) -> Step {
    let exact = desc
        .arities
        .iter()
        .find(|a| !a.variadic && a.fixed == args.len());
    let arity = match exact {
        Some(a) => *a,
        None => match desc
            .arities
            .iter()
            .find(|a| a.variadic && args.len() >= a.fixed)
        {
            Some(a) => *a,
            None => return Err(native_abi::arity_error(eval, &desc.display, args.len())),
        },
    };
    native_abi::guarded(eval, |eval| {
        let mut argv: Vec<Value> = args[..arity.fixed].to_vec();
        if arity.variadic {
            argv.push(native_abi::rest_args(&args[arity.fixed..]));
        }
        let mut ret = Value::Nil;
        let status = {
            let _active = ActiveScope::enter(eval);
            (arity.code)(&mut ret, argv.as_ptr(), argv.len(), captured.as_ptr())
        };
        if status == 0 {
            Ok(ret)
        } else {
            Err(take_pending())
        }
    })
}

/// Run a compiled program: boot, `run`'s frame, constants, then the
/// top-level thunks in load order inside the `load-file` frame.
/// Returns the process exit code (0, or 1 after the diagnostic).
pub(crate) fn run_main(path: &str, args: &[String], init: Code, tops: Vec<Code>) -> i32 {
    let mut eval = clj_eval::boot();
    let file: Arc<str> = Arc::from(path);
    let tops: Arc<[Code]> = tops.into();
    eval.install_native(
        clj_eval::MIXED_INTERN_NS,
        "run-native",
        Arc::new(move |eval: &mut Evaluator, _: &[Value]| {
            let tops = Arc::clone(&tops);
            clj_eval::load_native(eval, &file, move |eval| {
                run_code(eval, init)?;
                freeze();
                let mut last = Value::Nil;
                for top in tops.iter() {
                    last = run_code(eval, *top)?;
                }
                Ok(last)
            })
        }),
    );
    let script =
        clj_eval::script::run_script(args, &format!("({}/run-native)", clj_eval::MIXED_INTERN_NS));
    let code = match eval.eval_source(&script) {
        Ok(_) => 0,
        Err(e) => {
            eprintln!("{}", clj_eval::script::run_diagnostic(path, &e));
            1
        }
    };
    eval.join_drainers();
    std::mem::forget(eval);
    let _flushed = std::io::Write::flush(&mut std::io::stdout());
    code
}

/// Run one zero-argument compiled code (a top-level thunk or the
/// constant initializer) on `eval`.
fn run_code(eval: &mut Evaluator, code: Code) -> Step {
    let mut ret = Value::Nil;
    let status = {
        let _active = ActiveScope::enter(eval);
        code(&mut ret, std::ptr::null(), 0, std::ptr::null())
    };
    if status == 0 {
        Ok(ret)
    } else {
        Err(take_pending())
    }
}

/// Constant encoding of an `ns` form: `[name doc attr [kind vals…]…]`
/// (`doc`/`attr` nil when absent; kind 0 refer-clojure, 1 require,
/// 2 use, 3 import).
pub(crate) fn run_ns(desc: &Value) -> Step {
    use clj_eval::expr::NsClause;
    let Value::Vector(v) = desc else {
        return Ok(Value::Nil);
    };
    let items: Vec<Value> = v.iter().collect();
    let name = match items.first() {
        Some(Value::Str(s)) => s.to_string(),
        _ => String::new(),
    };
    let doc = match items.get(1) {
        Some(Value::Str(s)) => Some(s.to_string()),
        _ => None,
    };
    let attr = match items.get(2) {
        Some(Value::Map(m)) => Some(m.clone()),
        _ => None,
    };
    let mut clauses = Vec::new();
    for clause in items.iter().skip(3) {
        let Value::Vector(c) = clause else { continue };
        let mut parts = c.iter();
        let kind = match parts.next() {
            Some(Value::Int(k)) => k,
            _ => continue,
        };
        let vals: Vec<Value> = parts.collect();
        clauses.push(match kind {
            0 => NsClause::ReferClojure(vals),
            1 => NsClause::Require(vals),
            2 => NsClause::Use(vals),
            _ => NsClause::Import(vals),
        });
    }
    with_active(|eval| native_abi::ns(eval, &name, doc.as_deref(), attr.as_ref(), &clauses))
}

/// Constant encoding of a `case*` dispatch: `[shift mask test keys
/// skip]` (test 0 int, 1 hash-equiv, 2 hash-identity). Returns the
/// selector (see `rt_n_case`) and the realized key.
pub(crate) fn case_select(desc: &Value, v: &Value) -> Result<(i64, Value), Signal> {
    use clj_eval::expr::CaseTest;
    let Value::Vector(d) = desc else {
        return Ok((-1, Value::Nil));
    };
    let items: Vec<Value> = d.iter().collect();
    let int = |i: usize| match items.get(i) {
        Some(Value::Int(n)) => *n,
        _ => 0,
    };
    let ints = |i: usize| -> Vec<i32> {
        match items.get(i) {
            Some(Value::Vector(v)) => v
                .iter()
                .map(|x| match x {
                    Value::Int(n) => n as i32,
                    _ => 0,
                })
                .collect(),
            _ => Vec::new(),
        }
    };
    let test = match int(2) {
        0 => CaseTest::Int,
        1 => CaseTest::HashEquiv,
        _ => CaseTest::HashIdentity,
    };
    let (keys, skip) = (ints(3), ints(4));
    let hit = with_active(|eval| {
        native_abi::case_select(eval, v, int(0) as i32, int(1) as i32, &keys, &skip, test)
    })?;
    Ok(match hit {
        native_abi::CaseHit::Default => (-1, Value::Nil),
        native_abi::CaseHit::Direct(i) => (i as i64, Value::Nil),
        native_abi::CaseHit::Check(i, r) => (i as i64 | (1 << 32), r),
    })
}
