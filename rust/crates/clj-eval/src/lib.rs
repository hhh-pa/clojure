// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Analyzer and tree-walking interpreter for Clojure (T1174 S4).
//!
//! [`Analyzer`] turns reader values into [`Expr`]s enforcing the
//! `docs/spec/t1174/evaluator-contract.md` shape rules; [`Evaluator`]
//! runs them. Single namespace (`user`), no prelude yet (S5).

/// mimalloc (#226) off Linux, for every artifact that links the
/// interpreter — the `clojure-rs` binary and the `clj-rt` runtime
/// archive AOT binaries link alike (#258: the archive used the
/// platform allocator, so AOT binaries paid more per allocation
/// than `run`): the interpreter allocates many small,
/// short-lived blocks (reader lists, analyzed forms, seq steps), where
/// it is markedly cheaper than the platform allocators. Linux keeps
/// glibc's allocator for peak RSS: under the default `madvise`
/// transparent-huge-page mode mimalloc's arenas take huge pages (a
/// trivial run peaked at 19 MB instead of 9), and even without them a
/// trivial run peaks 0.7 MB higher and data-heavy runs 20-30% higher.
#[cfg(not(target_os = "linux"))]
#[global_allocator]
static GLOBAL: mimalloc::MiMalloc = mimalloc::MiMalloc;

pub mod aot_front;
pub mod aot_subset;
pub mod arrays;
#[path = "async.rs"]
pub mod async_ns;
pub mod builtin_expand;
pub mod classemu;
pub mod coll;
pub mod collmisc;
pub mod compiler;
pub mod concurrency;
pub mod core;
pub(crate) mod core_lazy;
pub(crate) mod core_macros;
pub mod ctor;
pub mod destructure_expand;
pub mod emucoll;
pub mod emudigest;
pub mod emurand;
pub mod emutime;
pub mod env;
pub mod error;
pub mod eval;
pub mod expr;
pub mod ext;
pub mod fmt;
pub mod fx4_form;
pub mod fx4_host;
pub mod hierarchy;
pub mod io;
pub mod jdkio;
pub mod jio;
pub mod lazy;
pub mod macros;
pub mod multi;
pub mod native_abi;

/// The owned-args buffer native calls take (see `native_abi`).
pub type SmallArgs = smallvec::SmallVec<[clj_value::Value; 4]>;
pub mod ns;
pub mod portdeps;
pub(crate) mod printsys;
pub mod protocol;
pub mod re;
pub mod reader;
pub mod reflect;
pub mod rng;
pub mod scalars;
pub mod script;
pub mod seqs;
pub mod shell;
pub(crate) mod stdlib_natives;
pub mod stm;
pub mod typeinternals;
pub mod types;
pub mod vars;
pub(crate) mod vm;
pub(crate) mod vm_intrinsics;
pub(crate) mod vm_live;
pub(crate) mod vm_ops;
pub(crate) mod vm_scalar;
// W6 (runtime-ish) — do not merge by hand, integrator resolves.
pub mod w6_runtime;
pub mod xform;

pub use aot_subset::{
    MIXED_INTERN_NS, MixedProgram, PlanError, Prim, SubsetError, SubsetExpr, SubsetFn,
    SubsetProgram, intern_call, plan_mixed, plan_source,
};
pub use env::{Bindings, Env, Namespace, NamespaceRegistry, SharedRegistry, VarState};
pub use error::{
    CompileError, NativeError, Signal, Verb, compile_diagnostic, compile_head, jvm_class_of,
    simple_class_of, span_of,
};
pub use eval::{Evaluator, LiveForms, SourceError};
pub use expr::{
    Analyzer, Arity, Catch, Expr, FunExpr, KeyBinding, MapPattern, Pattern, VecPattern,
};
pub use ext::{Closure, NativeFn, NativeImpl, NsHandle, as_closure, as_native, as_ns, munge};
pub use ns::{boot, load_native, load_text_as_file};
