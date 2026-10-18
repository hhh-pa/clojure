// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Analyzer and tree-walking interpreter for Clojure (T1174 S4).
//!
//! [`Analyzer`] turns reader values into [`Expr`]s enforcing the
//! `docs/t1174/evaluator-contract.md` shape rules; [`Evaluator`]
//! runs them. Single namespace (`user`), no prelude yet (S5).

pub mod env;
pub mod error;
pub mod eval;
pub mod expr;
pub mod ext;
pub mod ns;

pub use env::{Bindings, Env, Namespace, NamespaceRegistry, VarState};
pub use error::{CompileError, NativeError, Signal, Verb, jvm_class_of, simple_class_of, span_of};
pub use eval::{Evaluator, SourceError};
pub use expr::{
    Analyzer, Arity, Catch, Expr, FunExpr, KeyBinding, MapPattern, Pattern, VecPattern,
};
pub use ext::{Closure, NativeFn, NativeImpl, NsHandle, as_closure, as_native, as_ns, munge};
pub use ns::boot;
