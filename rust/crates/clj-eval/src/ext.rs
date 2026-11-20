// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Callable extension values: [`Closure`] (interpreted `fn`) and
//! [`NativeFn`] (Rust callables: S5 core fns plus test doubles).
//! Both print `#object[…]` with the munged display name and a
//! per-evaluator sequence id as the hex (deterministic for a fixed
//! program, §E10).

use std::any::Any;
use std::hash::Hasher;
use std::sync::Arc;

use clj_value::{ExtensionValue, Map, Value};

use crate::env::{Bindings, Env};
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::expr::Arity;

/// `Compiler.munge` for display names (`-` → `_`, `/` → `$`,
/// punctuation → `_NAME_`). Common names are exact; exotic ones
/// follow the same table.
#[must_use]
pub fn munge(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for c in display.chars() {
        munge_char(c, &mut out);
    }
    out
}

/// First half of the `Compiler.munge` char table.
fn munge_char(c: char, out: &mut String) {
    match c {
        '-' => out.push('_'),
        '/' => out.push('$'),
        ':' => out.push_str("_COLON_"),
        '+' => out.push_str("_PLUS_"),
        '>' => out.push_str("_GT_"),
        '<' => out.push_str("_LT_"),
        '=' => out.push_str("_EQ_"),
        '~' => out.push_str("_TILDE_"),
        '!' => out.push_str("_BANG_"),
        '@' => out.push_str("_CIRCA_"),
        '#' => out.push_str("_SHARP_"),
        '\'' => out.push_str("_SINGLEQUOTE_"),
        other => munge_char_rest(other, out),
    }
}

/// Rest of the `Compiler.munge` char table. Unlisted chars pass through.
fn munge_char_rest(c: char, out: &mut String) {
    match c {
        '"' => out.push_str("_DOUBLEQUOTE_"),
        '%' => out.push_str("_PERCENT_"),
        '^' => out.push_str("_CARET_"),
        '&' => out.push_str("_AMPERSAND_"),
        '*' => out.push_str("_STAR_"),
        '|' => out.push_str("_BAR_"),
        '{' => out.push_str("_LBRACE_"),
        '}' => out.push_str("_RBRACE_"),
        '[' => out.push_str("_LBRACK_"),
        ']' => out.push_str("_RBRACK_"),
        '?' => out.push_str("_QMARK_"),
        _ => out.push(c),
    }
}

/// The true `Compiler/munge` for the `munge` var (S4b, oracle
/// `mg-*` in `docs/spec/zero-oos-s4b-macros/probes/macros.out`).
/// Same table as the display [`munge`] except `/` (which is
/// `_SLASH_` here, `$` for display names) and `\`
/// (`_BSLASH_` here, passthrough for display).
#[must_use]
pub fn compiler_munge(display: &str) -> String {
    let mut out = String::with_capacity(display.len());
    for c in display.chars() {
        compiler_munge_char(c, &mut out);
    }
    out
}

/// First half of the `Compiler/munge` char table.
fn compiler_munge_char(c: char, out: &mut String) {
    match c {
        '-' => out.push('_'),
        '/' => out.push_str("_SLASH_"),
        '\\' => out.push_str("_BSLASH_"),
        ':' => out.push_str("_COLON_"),
        '+' => out.push_str("_PLUS_"),
        '>' => out.push_str("_GT_"),
        '<' => out.push_str("_LT_"),
        '=' => out.push_str("_EQ_"),
        '~' => out.push_str("_TILDE_"),
        '!' => out.push_str("_BANG_"),
        '@' => out.push_str("_CIRCA_"),
        '#' => out.push_str("_SHARP_"),
        '\'' => out.push_str("_SINGLEQUOTE_"),
        other => compiler_munge_char_rest(other, out),
    }
}

/// Rest of the `Compiler/munge` char table. Unlisted chars
/// (letters, digits, `.`, `,`, `;`, parens, `$`, whitespace,
/// non-ASCII) pass through.
fn compiler_munge_char_rest(c: char, out: &mut String) {
    match c {
        '"' => out.push_str("_DOUBLEQUOTE_"),
        '%' => out.push_str("_PERCENT_"),
        '^' => out.push_str("_CARET_"),
        '&' => out.push_str("_AMPERSAND_"),
        '*' => out.push_str("_STAR_"),
        '|' => out.push_str("_BAR_"),
        '{' => out.push_str("_LBRACE_"),
        '}' => out.push_str("_RBRACE_"),
        '[' => out.push_str("_LBRACK_"),
        ']' => out.push_str("_RBRACK_"),
        '?' => out.push_str("_QMARK_"),
        _ => out.push(c),
    }
}

/// An interpreted function value: display name, arities, captured
/// frame, optional metadata (`with-meta`), and optional conveyed
/// thread bindings (`bound-fn` snapshots these at creation and
/// re-pushes them around every invocation). Identity is the
/// sequence `id`.
#[derive(Clone, Debug)]
pub struct Closure {
    pub id: u64,
    pub display: Arc<str>,
    pub arities: Arc<[Arity]>,
    pub env: Env,
    pub meta: Option<Map>,
    pub conveyed: Option<Bindings>,
    /// The register VM's code and captured values (`None` for
    /// closures the tree evaluator made, which run `arities` over
    /// `env`).
    pub(crate) vm: Option<crate::vm::VmCode>,
}

impl ExtensionValue for Closure {
    fn class_name(&self) -> String {
        munge(&self.display)
    }

    fn print(&self) -> String {
        let munged = munge(&self.display);
        format!(
            "#object[{munged} 0x{:x} \"{munged}@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<Closure>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dup_shape(&self) -> Option<String> {
        let munged = munge(&self.display);
        Some(format!("#=({munged}. )"))
    }
}

/// A Rust callable: `display` names arity errors, `func` runs the
/// call with the live evaluator (natives can intern, resolve, and
/// eval §N vars). Failures are [`NativeError`]s (the evaluator
/// stamps the identity when throwing).
pub type NativeImpl = dyn Fn(&mut Evaluator, &[Value]) -> Result<Value, NativeError> + Send + Sync;

/// An owned-args native's argument buffer (moved into the call).
pub type OwnedArgs = smallvec::SmallVec<[Value; 4]>;

/// The owned-args entry of a head-releasing looping native
/// (`reduce`, `into`, ...): taking the arg buffer by value lets
/// the walk nil out the coll slot after seqing, so memoized seq
/// chunks free as the walk advances (like the JVM's dead locals)
/// instead of accumulating behind the head held in `args` (#221).
/// Borrow-path callers (`apply`, direct `func` users) run the
/// compat `func` shim (correct, retaining).
pub type OwnedNativeImpl =
    dyn Fn(&mut Evaluator, OwnedArgs) -> Result<Value, NativeError> + Send + Sync;

/// An inline-able integer operation for direct-threaded calls
/// (#226): when a validated direct call site targets one of these
/// genuine core natives with all-`Int` args, the evaluator
/// computes the result without the `Num` tower or the `dyn`
/// call. Identity is the installing fn pointer (see
/// [`crate::core::fast_arith_for`], never the name), so a
/// redefined root installing lookalike fakes never mis-tags.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum FastArith {
    /// No inline op (the general snapshot call).
    #[default]
    None,
    /// `+`/`-`/`*` (2+ arg checked `Int` op; overflow falls back
    /// to the native, so `long overflow` and `*unchecked-math*`
    /// stay the native's).
    Add,
    Sub,
    Mul,
    /// `/` (2+ arg exact `Int` or `f64` divide; non-divisible
    /// integer ratios and `/ by zero` fall back to the native).
    Div,
    /// `<`/`<=`/`>`/`>=` (2+ arg numeric compare).
    Lt,
    Lte,
    Gt,
    Gte,
    /// `inc`/`dec` (1-arg checked `Int` step; overflow falls
    /// back to the native).
    Inc,
    Dec,
}

/// A [`NativeFn`]'s display name (`"ns/name"`): a shared `Arc<str>` or
/// a byte span of a shared table (`Tables::fn_names` in AOT, avoiding
/// a per-fn string allocation, #271).
#[derive(Clone)]
pub enum DisplayName {
    Shared(Arc<str>),
    Span { buf: Arc<str>, start: u32, len: u32 },
}

impl DisplayName {
    /// A shared display name from any string slice.
    #[must_use]
    pub fn shared(s: &str) -> Self {
        Self::Shared(Arc::from(s))
    }

    /// A display name slicing `buf[start..start + len]`.
    #[must_use]
    pub fn span(buf: Arc<str>, start: u32, len: u32) -> Self {
        Self::Span { buf, start, len }
    }
}

impl std::ops::Deref for DisplayName {
    type Target = str;

    fn deref(&self) -> &str {
        match self {
            Self::Shared(s) => s,
            Self::Span { buf, start, len } => {
                let at = *start as usize;
                buf.get(at..at + *len as usize).unwrap_or("")
            }
        }
    }
}

impl AsRef<str> for DisplayName {
    fn as_ref(&self) -> &str {
        self
    }
}

impl std::fmt::Debug for DisplayName {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        std::fmt::Debug::fmt(&**self, f)
    }
}

/// How a [`NativeFn`] runs borrowed-argument calls (#271): a plain fn
/// pointer (zero heap allocation, direct call for every booted native),
/// a compiled AOT closure (dispatched through [`CompiledDispatch`] with
/// no per-fn closure allocation), or a dynamic Rust closure (`comp`,
/// `partial`, transducers, tests).
#[derive(Clone)]
pub enum NativeTarget {
    Ptr(crate::ns::NativeFnPtr),
    Compiled { desc: usize, captured: Arc<[Value]> },
    Dyn(Arc<NativeImpl>),
}

impl NativeTarget {
    /// Invoke this target over borrowed `args`.
    #[inline]
    pub fn call(&self, eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
        match self {
            Self::Ptr(f) => f(eval, args),
            Self::Compiled { desc, captured } => {
                (compiled_dispatch().call)(eval, *desc, captured, args)
            }
            Self::Dyn(f) => f(eval, args),
        }
    }
}

/// How a [`NativeFn`] runs owned-argument calls (#271): a plain fn
/// pointer (zero heap allocation) or a single-arity compiled AOT
/// closure (dispatched through [`CompiledDispatch`], zero closure
/// allocation).
#[derive(Clone)]
pub enum OwnedTarget {
    Ptr(crate::ns::OwnedNativeFnPtr),
    Compiled { desc: usize, captured: Arc<[Value]> },
}

impl OwnedTarget {
    /// Invoke this target taking ownership of `args`.
    #[inline]
    pub fn call(&self, eval: &mut Evaluator, args: OwnedArgs) -> Result<Value, NativeError> {
        match self {
            Self::Ptr(f) => f(eval, args),
            Self::Compiled { desc, captured } => {
                (compiled_dispatch().call_owned)(eval, *desc, captured, args)
            }
        }
    }
}

#[derive(Clone)]
pub struct NativeFn {
    pub id: u64,
    pub display: DisplayName,
    pub func: NativeTarget,
    /// The owned-args entry (`None` for borrow-only natives).
    pub owned: Option<OwnedTarget>,
    pub meta: Option<Map>,
    /// The inline integer op (`None` unless installed from a
    /// genuine core arith/compare fn pointer).
    pub fast: FastArith,
    /// A compiled closure's single fixed-arity entry, for compiled call
    /// sites to call directly (#271); `None` for every other native.
    pub compiled: Option<CompiledEntry>,
}

impl NativeFn {
    /// Invoke with owned `args`: takes `owned` when present, else borrows `args`.
    #[inline]
    pub(crate) fn call_owned(
        &self,
        eval: &mut Evaluator,
        args: OwnedArgs,
    ) -> Result<Value, NativeError> {
        match &self.owned {
            Some(owned) => owned.call(eval, args),
            None => self.func.call(eval, &args),
        }
    }
}

/// Compiled code's entry point (`out`, argv, argc with flags, captured
/// values), as the native runtime emits and calls it.
pub type CodePtr = extern "C" fn(*mut Value, *const Value, usize, *const Value) -> i32;

/// A compiled closure's code, captured values and parameter count (#271).
#[derive(Clone)]
pub struct CompiledEntry {
    pub code: CodePtr,
    pub captured: Arc<[Value]>,
    pub fixed: usize,
}

/// Compiled code called over an argument array the caller owns, which it
/// may take its params from (leaving nil; the caller drops the rest):
/// `Prepared` hands one or two args straight over, with no argument
/// buffer moved through the owned entry (#271: three 144-byte copies per
/// `map`/`filter`/`reduce` step).
pub type InPlaceCall =
    fn(&mut Evaluator, CodePtr, &[Value], &mut [Value]) -> Result<Value, NativeError>;

/// Borrowed-argument entry for a compiled AOT closure.
pub type CompiledBorrowCall =
    fn(&mut Evaluator, usize, &[Value], &[Value]) -> Result<Value, NativeError>;

/// Owned-argument entry for a compiled AOT closure.
pub type CompiledOwnedCall =
    fn(&mut Evaluator, usize, &[Value], OwnedArgs) -> Result<Value, NativeError>;

/// Process-wide entry points registered once by the native runtime for
/// compiled AOT closures (#271): borrowed, owned, and in-place calls
/// without allocating a closure per compiled fn.
#[derive(Clone, Copy)]
pub struct CompiledDispatch {
    pub call: CompiledBorrowCall,
    pub call_owned: CompiledOwnedCall,
    pub call_in_place: InPlaceCall,
}

static COMPILED: std::sync::OnceLock<CompiledDispatch> = std::sync::OnceLock::new();

/// Register the process's [`CompiledDispatch`] (the first registration stands).
pub fn set_compiled_dispatch(d: CompiledDispatch) {
    let _ = COMPILED.set(d);
}

/// The registered [`CompiledDispatch`] (registered before any compiled
/// closure is created).
fn compiled_dispatch() -> CompiledDispatch {
    *COMPILED.get().expect("compiled dispatch registered")
}

/// The registered [`InPlaceCall`] (`None` before any compiled code).
pub(crate) fn in_place_call() -> Option<InPlaceCall> {
    COMPILED.get().map(|d| d.call_in_place)
}

impl std::fmt::Debug for NativeFn {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("NativeFn")
            .field("id", &self.id)
            .field("display", &self.display)
            .finish_non_exhaustive()
    }
}

impl ExtensionValue for NativeFn {
    fn class_name(&self) -> String {
        munge(&self.display)
    }

    fn print(&self) -> String {
        let munged = munge(&self.display);
        format!(
            "#object[{munged} 0x{:x} \"{munged}@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<NativeFn>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dup_shape(&self) -> Option<String> {
        let munged = munge(&self.display);
        Some(format!("#=({munged}. )"))
    }
}

/// Downcast a value to a closure, if it is one.
#[must_use]
pub fn as_closure(v: &Value) -> Option<&Closure> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<Closure>(),
        _ => None,
    }
}

/// Downcast a value to a native fn, if it is one.
#[must_use]
pub fn as_native(v: &Value) -> Option<&NativeFn> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<NativeFn>(),
        _ => None,
    }
}

/// A namespace handle: `name` plus the registry creation `id`.
/// Equality and hashing use the `id` only, like JVM identity — a
/// removed-then-recreated namespace is a new value.
#[derive(Clone, Debug)]
pub struct NsHandle {
    pub name: Arc<str>,
    pub id: u64,
}

impl ExtensionValue for NsHandle {
    fn class_name(&self) -> String {
        "clojure.lang.Namespace".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[clojure.lang.Namespace 0x{:x} \"{}\"]",
            self.id, self.name
        )
    }

    /// `print-object` writes the `toString` unquoted when not
    /// readably.
    fn print_str(&self) -> Option<String> {
        Some(format!(
            "#object[clojure.lang.Namespace 0x{:x} {}]",
            self.id, self.name
        ))
    }

    /// `Namespace.toString` is the name.
    fn java_str(&self) -> Option<String> {
        Some(self.name.to_string())
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<NsHandle>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn Any {
        self
    }

    fn dup_shape(&self) -> Option<String> {
        Some(format!("#=(find-ns {})", self.name))
    }
}

/// Downcast a value to a namespace handle, if it is one.
#[must_use]
pub fn as_ns(v: &Value) -> Option<&NsHandle> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<NsHandle>(),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The `CHAR_MAP` table, oracle-pinned (every arm matches
    /// `Compiler/munge`; dots pass through like the JVM; `/` is the
    /// display's namespace separator by design).
    #[test]
    fn munge_matches_compiler_table() {
        assert_eq!(
            munge(":+><=~!@#\"'%^&*|{}[]?"),
            "_COLON__PLUS__GT__LT__EQ__TILDE__BANG__CIRCA__SHARP__DOUBLEQUOTE__SINGLEQUOTE__PERCENT__CARET__AMPERSAND__STAR__BAR__LBRACE__RBRACE__LBRACK__RBRACK__QMARK_"
        );
        // Dots pass through (namespace separators, like
        // `Compiler/munge`); this is also `(pr-str +)`'s class.
        assert_eq!(munge("clojure.core/+"), "clojure.core$_PLUS_");
    }

    #[test]
    fn native_fn_debug_names_id_and_display() {
        let func: Arc<NativeImpl> = Arc::new(|_, _| Ok(Value::Nil));
        let f = NativeFn {
            id: 7,
            display: DisplayName::shared("user/dbl"),
            func: NativeTarget::Dyn(func),
            owned: None,
            meta: None,
            fast: FastArith::None,
            compiled: None,
        };
        let s = format!("{f:?}");
        assert!(s.contains("NativeFn"), "{s}");
        assert!(s.contains('7'), "{s}");
        assert!(s.contains("user/dbl"), "{s}");
        let buf: Arc<str> = Arc::from("xxuser/spanyy");
        let sp = DisplayName::span(buf, 2, 9);
        assert_eq!(&*sp, "user/span");
        assert_eq!(sp.as_ref(), "user/span");
        assert!(format!("{sp:?}").contains("user/span"));
    }

    #[test]
    fn downcasts_reject_other_values() {
        assert!(as_closure(&Value::Int(1)).is_none());
        assert!(as_native(&Value::Int(1)).is_none());
    }
}
