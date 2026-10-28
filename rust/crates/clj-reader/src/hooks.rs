// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! [`ReadHooks`]: the evaluator callbacks the reader needs but must
//! not name (issue #50, S7): `#=` application and
//! `*reader-resolver*` consults. The hook rides the read stack as a
//! `&mut dyn` parameter (never stored: the caller's borrow stays
//! reentrant, so `#=` bodies may read again), keeping this crate
//! eval-free.

use clj_value::{Symbol, Throwable, Value};

/// Which `*reader-resolver*` method a consult stands for (the JVM
/// `LispReader$Resolver` interface, mirrored one for one).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ResolverOp {
    /// `currentNS()` (the symbol argument is always `None`).
    CurrentNs,
    /// `resolveAlias(sym)`.
    ResolveAlias,
    /// `resolveClass(sym)`.
    ResolveClass,
    /// `resolveVar(sym)`.
    ResolveVar,
}

/// A `#=`/resolver failure: either no evaluator was installed (the
/// pure-reader contexts) or the application threw (the throwable
/// travels intact, so `ex-data` and causes survive the read).
#[derive(Clone, Debug)]
pub enum EvalReadError {
    /// No evaluator installed (pure-reader unit tests, the
    /// differential harness): `#=` cannot apply.
    NoHook,
    /// The application threw (a `#=` outcome or a resolver fn
    /// failure): propagates through the read unwrapped.
    Failed(Throwable),
}

impl PartialEq for EvalReadError {
    fn eq(&self, other: &Self) -> bool {
        match (self, other) {
            (Self::NoHook, Self::NoHook) => true,
            (Self::Failed(a), Self::Failed(b)) => a.id == b.id,
            _ => false,
        }
    }
}

impl Eq for EvalReadError {}

/// The evaluator side of `#=` and `*reader-resolver*`, implemented
/// in `clj-eval` (which owns var resolution and fn application).
pub trait ReadHooks {
    /// Apply one `#=` operand: resolve its head in the current
    /// namespace and apply the var to the raw operand forms (JVM
    /// `EvalReader`, probes `c-*`/`d-*`).
    fn eval_read(&mut self, form: &Value) -> Result<Value, EvalReadError>;
    /// Consult the bound `*reader-resolver*` (probes `f-*`): `sym`
    /// is `None` for [`ResolverOp::CurrentNs`]; the answer is a
    /// symbol or nil.
    fn resolve(
        &mut self,
        op: ResolverOp,
        sym: Option<&Symbol>,
    ) -> Result<Option<Symbol>, EvalReadError>;
    /// A fresh throwable identity (the evaluator's own counter, so
    /// reader-built throwables never collide with eval-built ones).
    fn fresh_id(&mut self) -> u64;
}

/// The hookless evaluator: `#=` cannot apply and resolver consults
/// never happen (callers check the bound flag first, so `resolve`
/// only fires defensively).
pub struct NoHooks;

impl ReadHooks for NoHooks {
    fn eval_read(&mut self, _form: &Value) -> Result<Value, EvalReadError> {
        Err(EvalReadError::NoHook)
    }

    fn resolve(
        &mut self,
        _op: ResolverOp,
        _sym: Option<&Symbol>,
    ) -> Result<Option<Symbol>, EvalReadError> {
        Err(EvalReadError::NoHook)
    }

    fn fresh_id(&mut self) -> u64 {
        crate::next_id()
    }
}

/// Scripted hooks for unit tests: canned `#=` outcomes plus a
/// resolve queue (consulted in order, calls recorded for
/// order pins; an empty queue answers nil).
#[cfg(test)]
pub(crate) struct ScriptedHooks {
    /// The `#=` outcome (cloned per call).
    pub eval: Result<Value, EvalReadError>,
    /// Queued resolve answers (popped per consult).
    pub resolve_queue: std::collections::VecDeque<Result<Option<Symbol>, EvalReadError>>,
    /// Recorded consults (`(op, sym)` in call order).
    pub calls: Vec<(ResolverOp, Option<Symbol>)>,
    next: u64,
}

#[cfg(test)]
impl ScriptedHooks {
    /// A hook that applies `#=` to `eval` and answers every
    /// consult from `resolve_queue`.
    pub(crate) fn new(
        eval: Result<Value, EvalReadError>,
        resolve_queue: Vec<Result<Option<Symbol>, EvalReadError>>,
    ) -> Self {
        Self {
            eval,
            resolve_queue: resolve_queue.into(),
            calls: Vec::new(),
            next: 0,
        }
    }
}

#[cfg(test)]
impl ReadHooks for ScriptedHooks {
    fn eval_read(&mut self, _form: &Value) -> Result<Value, EvalReadError> {
        self.eval.clone()
    }

    fn resolve(
        &mut self,
        op: ResolverOp,
        sym: Option<&Symbol>,
    ) -> Result<Option<Symbol>, EvalReadError> {
        self.calls.push((op, sym.cloned()));
        self.resolve_queue.pop_front().unwrap_or(Ok(None))
    }

    fn fresh_id(&mut self) -> u64 {
        self.next += 1;
        self.next
    }
}

#[cfg(test)]
mod tests {
    use super::{EvalReadError, NoHooks, ReadHooks, ResolverOp};
    use clj_value::{Symbol, ThrowClass, Throwable, Value};

    fn failed(id: u64) -> EvalReadError {
        EvalReadError::Failed(Throwable {
            id,
            class: ThrowClass::RuntimeException,
            message: None,
            data: None,
            cause: None,
        })
    }

    #[test]
    fn eval_read_error_eq_is_identity() {
        // Throwables compare by id only (like the JVM); `NoHook`
        // equals itself and nothing else.
        assert_eq!(EvalReadError::NoHook, EvalReadError::NoHook);
        assert_eq!(failed(1), failed(1));
        assert_ne!(failed(1), failed(2));
        assert_ne!(EvalReadError::NoHook, failed(1));
        assert_ne!(failed(1), EvalReadError::NoHook);
    }

    #[test]
    fn no_hooks_denies_and_mints() {
        // The hookless evaluator: both consults fail, ids still flow.
        let mut hooks = NoHooks;
        assert_eq!(
            hooks.eval_read(&Value::Nil).unwrap_err(),
            EvalReadError::NoHook
        );
        assert_eq!(
            hooks.resolve(ResolverOp::CurrentNs, None).unwrap_err(),
            EvalReadError::NoHook
        );
        let sym = Symbol::new(None, "x");
        assert_eq!(
            hooks
                .resolve(ResolverOp::ResolveAlias, Some(&sym))
                .unwrap_err(),
            EvalReadError::NoHook
        );
        assert!(hooks.fresh_id() > 0);
    }
}
