// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5C macros: probe38 replay against the booted core (`defmacro`,
//! expansion, `macroexpand-1`/`macroexpand`, `gensym`,
//! `declare`/`ns`, builtin-macro vars, early list natives).

use std::path::PathBuf;

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run(eval: &mut Evaluator, src: &str) -> Vec<String> {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| pr(&realized(eval, v)))
        .collect()
}

/// Realize before printing (like the JVM, whose print walks —
/// forcing errors surface here).
fn realized(eval: &mut Evaluator, v: &Value) -> Value {
    match eval.realize_deep(v) {
        Ok(v) => v,
        Err(clj_eval::Signal::Throw(t)) => panic!(
            "unexpected throw while printing: {} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("unexpected signal while printing: {other:?}"),
    }
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src).pop().unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => {
            format!(
                "{} | {}",
                t.class.jvm_name(),
                t.message.as_deref().unwrap_or("")
            )
        }
        Err(other) => panic!("expected an uncaught throw, got {other}"),
        Ok(vs) => {
            // Print forces (like the JVM): a lazy top-level value
            // can still throw.
            for v in &vs {
                match eval.realize_deep(v) {
                    Ok(_) => {}
                    Err(clj_eval::Signal::Throw(t)) => {
                        return format!(
                            "{} | {}",
                            t.class.jvm_name(),
                            t.message.as_deref().unwrap_or("")
                        );
                    }
                    Err(other) => panic!("unexpected signal while printing: {other:?}"),
                }
            }
            panic!("expected an uncaught throw, got values")
        }
    }
}

/// The compile chain of a direct analyzer failure.
fn compile_err(eval: &mut Evaluator, src: &str) -> Vec<String> {
    match eval.eval_source(src).unwrap_err() {
        SourceError::Compile(e) => e.chain(),
        other => panic!("expected a compile error, got {other}"),
    }
}

/// A per-test `*load-path*` dir (unique across parallel tests,
/// removed on drop).
struct Fixture {
    dir: PathBuf,
}

impl Fixture {
    fn new(name: &str) -> Self {
        let dir = std::env::temp_dir().join(format!("clj-s5c-{name}-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        Self { dir }
    }

    fn write(&self, rel: &str, text: &str) {
        let path = self.dir.join(rel);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).unwrap();
        }
        std::fs::write(path, text).unwrap();
    }

    /// The dir as a Clojure string literal.
    fn lit(&self) -> String {
        let s = self.dir.to_string_lossy();
        format!("\"{}\"", s.replace('\\', "\\\\").replace('"', "\\\""))
    }

    /// Wrap `src` in a `*load-path*` binding to this dir.
    fn within(&self, src: &str) -> String {
        format!(
            "(binding [clojure.core/*load-path* [{}]] {src})",
            self.lit()
        )
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

#[test]
fn defmacro_basics() {
    let mut eval = boot();
    // Returns the var with `:macro`/`:arglists` meta.
    assert_eq!(
        run_last(&mut eval, "(defmacro madd [x] (list 'quote x))"),
        "#'user/madd"
    );
    assert_eq!(run_last(&mut eval, "(:macro (meta #'madd))"), "true");
    assert_eq!(run_last(&mut eval, "(:arglists (meta #'madd))"), "([x])");
    // Invocation expands, then evaluates.
    assert_eq!(run_last(&mut eval, "(madd 5)"), "5");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(madd 5))"),
        "(quote 5)"
    );
    // Multi-arity, docstring, attrmap.
    assert_eq!(
        run_last(&mut eval, "(defmacro mmul \"mul\" {:k 1} ([] 0) ([x] x))"),
        "#'user/mmul"
    );
    assert_eq!(run_last(&mut eval, "(mmul)"), "0");
    assert_eq!(run_last(&mut eval, "(mmul 7)"), "7");
    assert_eq!(run_last(&mut eval, "(:doc (meta #'mmul))"), "\"mul\"");
    assert_eq!(run_last(&mut eval, "(:k (meta #'mmul))"), "1");
    assert_eq!(run_last(&mut eval, "(:arglists (meta #'mmul))"), "([] [x])");
    // The attrmap wins over the docstring for `:doc`.
    run_last(&mut eval, "(defmacro mdb \"d1\" {:doc \"d2\"} [x] x)");
    assert_eq!(run_last(&mut eval, "(:doc (meta #'mdb))"), "\"d2\"");
    // `def` with `^:macro` makes a macro too (explicit `&form`).
    run_last(
        &mut eval,
        "(def ^:macro mdm (fn [&form &env x] (list 'quote x)))",
    );
    assert_eq!(run_last(&mut eval, "(mdm 9)"), "9");
    assert_eq!(run_last(&mut eval, "(:macro (meta #'mdm))"), "true");
    // `alter-meta!` confers `:macro` as well.
    run_last(&mut eval, "(def mam (fn [&form &env x] (list 'quote x)))");
    run_last(&mut eval, "(alter-meta! (var mam) (fn [m] {:macro true}))");
    assert_eq!(run_last(&mut eval, "(mam 3)"), "3");
    // Re-`def`ing a macro as a plain fn clears `:macro`.
    run_last(&mut eval, "(defmacro mrd [x] x)");
    run_last(&mut eval, "(def mrd 5)");
    assert_eq!(run_last(&mut eval, "(:macro (meta #'mrd))"), "nil");
    assert_eq!(run_last(&mut eval, "mrd"), "5");
}

#[test]
fn macro_fn_calls() {
    let mut eval = boot();
    run_last(&mut eval, "(defmacro mfc [x] (list 'quote x))");
    // The macro fn runs directly with `(form, env, args...)` and
    // returns the expansion unevaluated.
    assert_eq!(
        run_last(&mut eval, "((var-get (var mfc)) 'F nil 5)"),
        "(quote 5)"
    );
    // Its display is the qualified var name (arity names `user/m`).
    assert_eq!(
        uncaught(&mut eval, "((var-get (var mfc)) 5)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: user/mfc"
    );
    // An explicit leading `&form` shadows positionally.
    run_last(
        &mut eval,
        "(defmacro mex [&form frm & args] (list 'quote frm))",
    );
    assert_eq!(run_last(&mut eval, "(mex a b c)"), "b");
    // Calling through the var value invokes the fn (no macro
    // treatment for non-symbol heads).
    assert_eq!(
        uncaught(&mut eval, "((var mfc) 5)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: user/mfc"
    );
}

#[test]
fn form_and_env() {
    let mut eval = boot();
    // `&form` is the whole call form, with its reader meta.
    run_last(&mut eval, "(defmacro mshow [x] (list 'quote &form))");
    assert_eq!(run_last(&mut eval, "(mshow 1)"), "(mshow 1)");
    run_last(&mut eval, "(defmacro mfm [x] (list 'quote (meta &form)))");
    assert_eq!(run_last(&mut eval, "(mfm 1)"), "{:line 1, :column 1}");
    // `&env` is nil at top, the local map inside bindings.
    run_last(&mut eval, "(defmacro menv [] (list 'quote &env))");
    assert_eq!(run_last(&mut eval, "(menv)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(let [a 1 b 2] (menv))"),
        "{a true, b true}"
    );
    assert_eq!(run_last(&mut eval, "((fn [q] (menv)) 1)"), "{q true}");
    // Outside macro calls they are ordinary unresolvable symbols.
    assert_eq!(
        compile_err(&mut eval, "&form"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: &form in this context"
                .to_owned(),
        ]
    );
}

#[test]
fn macro_value() {
    let mut eval = boot();
    run_last(&mut eval, "(defmacro mv [x] x)");
    // Macros have no value (locals still shadow).
    assert_eq!(
        compile_err(&mut eval, "mv"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Can't take value of a macro: #'user/mv".to_owned(),
        ]
    );
    assert_eq!(run_last(&mut eval, "(let [mv 99] mv)"), "99");
    assert_eq!(run_last(&mut eval, "(quote mv)"), "mv");
}

#[test]
fn defmacro_errors() {
    let mut eval = boot();
    // Bare `(defmacro)` is the arity error, unwrapped.
    assert_eq!(
        uncaught(&mut eval, "(defmacro)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/defmacro"
    );
    // Shape failures are the `defmacro` spec pair (the JVM routes
    // through `defn`; the port names its own validator — fence).
    for src in [
        "(defmacro monly)",
        "(defmacro 5 [x] x)",
        "(defmacro foo/bar [x] x)",
        "(defmacro m mismatched)",
        "(defmacro m ([x] x) 5)",
        "(defmacro m (5 x))",
    ] {
        assert_eq!(
            compile_err(&mut eval, src),
            vec![
                "Syntax error macroexpanding clojure.core/defmacro at (user:1:1).".to_owned(),
                "clojure.lang.ExceptionInfo | Call to clojure.core/defmacro did not conform to spec."
                    .to_owned(),
            ],
            "{src}"
        );
    }
}

#[test]
fn macroexpand_one() {
    let mut eval = boot();
    run_last(&mut eval, "(defmacro m1 [x] (list 'quote x))");
    // One head step; non-macros return unchanged.
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(m1 5))"), "(quote 5)");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(foo 1))"), "(foo 1)");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 5)"), "5");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 'a)"), "a");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '())"), "()");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 [])"), "[]");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(quote (m1 1)))"),
        "(quote (m1 1))"
    );
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(do 1 2))"), "(do 1 2)");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(clojure.core/do 1 2))"),
        "(clojure.core/do 1 2)"
    );
    // Builtin forms are the identity (the JVM expands the real
    // `declare`/`ns`/`fn`/`let` macros — fence).
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(let [a 1] a))"),
        "(let [a 1] a)"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(declare a))"),
        "(declare a)"
    );
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(ns foo))"), "(ns foo)");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(with-redefs [a 1] a))"),
        "(with-redefs [a 1] a)"
    );
    // `Var` operators expand when macro, else unchanged.
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 (list (var m1) 5))"),
        "(quote 5)"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 (list (var list) 5))"),
        "(#'clojure.core/list 5)"
    );
    // Weird and unresolvable operators return unchanged.
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(5 1))"), "(5 1)");
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(:k 1))"), "(:k 1)");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(no.such.ns/m 1))"),
        "(no.such.ns/m 1)"
    );
    // `&env` is nil through the native (the expansion quotes it).
    run_last(&mut eval, "(defmacro menv1 [] (list 'quote &env))");
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(menv1))"),
        "(quote nil)"
    );
    // Both arities are exactly 1.
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/macroexpand-1"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/macroexpand-1"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/macroexpand"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/macroexpand"
    );
}

#[test]
fn macroexpand_full() {
    let mut eval = boot();
    run_last(&mut eval, "(defmacro mlo [x] (list 'quote x))");
    run_last(&mut eval, "(defmacro moo [x] (list 'mlo x))");
    // One step vs the full chain.
    assert_eq!(run_last(&mut eval, "(macroexpand-1 '(moo 5))"), "(mlo 5)");
    assert_eq!(run_last(&mut eval, "(macroexpand '(moo 5))"), "(quote 5)");
    // Only the head expands, never nested forms.
    assert_eq!(
        run_last(&mut eval, "(macroexpand '(list (moo 5)))"),
        "(list (moo 5))"
    );
    assert_eq!(run_last(&mut eval, "(macroexpand '(foo 1))"), "(foo 1)");
    assert_eq!(run_last(&mut eval, "(macroexpand 5)"), "5");
}

#[test]
fn eval_expands() {
    let mut eval = boot();
    run_last(&mut eval, "(defmacro me1 [x] (list 'quote x))");
    assert_eq!(run_last(&mut eval, "(eval '(me1 5))"), "5");
    // `quote` blocks expansion.
    assert_eq!(run_last(&mut eval, "(eval ''(me1 5))"), "(me1 5)");
    // Qualified specials are var lookups that miss.
    assert_eq!(
        compile_err(&mut eval, "(user/do 1 2)"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | No such var: user/do".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "if"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: if in this context".to_owned(),
        ]
    );
    // `do` splits past macroexpansion: the inner `defmacro`
    // evaluates before the later element analyzes.
    run_last(
        &mut eval,
        "(defmacro mdo2 [] (quote (do (defmacro mdoinner [] 42) (mdoinner))))",
    );
    assert_eq!(run_last(&mut eval, "(mdo2)"), "42");
    assert_eq!(run_last(&mut eval, "(eval '(mdo2))"), "42");
}

#[test]
fn expansion_wraps() {
    let mut eval = boot();
    // The macro's own arity strips 2 and rethrows bare.
    run_last(&mut eval, "(defmacro ma1 [x] x)");
    assert_eq!(
        uncaught(&mut eval, "(ma1)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: user/ma1"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(ma1))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: user/ma1"
    );
    // An inner fn's arity rethrows bare as-is (name differs).
    run_last(&mut eval, "(defmacro mia [] (meta 1 2))");
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(mia))"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/meta"
    );
    // `IllegalStateException` wraps as `Syntax error`.
    run_last(&mut eval, "(def vsx 1)");
    run_last(&mut eval, "(defmacro mISE [] (var-set (var vsx) 1))");
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(mISE))"),
        "java.lang.IllegalStateException | Syntax error macroexpanding mISE at (user:1:1).\njava.lang.IllegalStateException | Can't change/establish root binding of: vsx with set"
    );
    assert_eq!(
        compile_err(&mut eval, "(mISE)"),
        vec![
            "Syntax error macroexpanding mISE at (user:1:1).".to_owned(),
            "java.lang.IllegalStateException | Can't change/establish root binding of: vsx with set"
                .to_owned(),
        ]
    );
    // `IllegalArgumentException` likewise.
    run_last(&mut eval, "(defmacro mIAE [] (seq 5))");
    assert_eq!(
        compile_err(&mut eval, "(mIAE)"),
        vec![
            "Syntax error macroexpanding mIAE at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
                .to_owned(),
        ]
    );
    // `ClassCastException` wraps as `Unexpected error`.
    run_last(&mut eval, "(defmacro mCCE [] (with-meta 1 {}))");
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(mCCE))"),
        "java.lang.ClassCastException | Unexpected error macroexpanding mCCE at (user:1:1).\njava.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
    assert_eq!(
        compile_err(&mut eval, "(mCCE)"),
        vec![
            "Unexpected error macroexpanding mCCE at (user:1:1).".to_owned(),
            "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
                .to_owned(),
        ]
    );
}

#[test]
fn private_macros_and_fns() {
    let mut eval = boot();
    run_last(&mut eval, "(create-ns 'fx)");
    run_last(&mut eval, "(ns fx)");
    run_last(&mut eval, "(def ^:private privfn (fn [x] x))");
    run_last(&mut eval, "(defmacro ^:private privm [x] (list 'quote x))");
    run_last(&mut eval, "(ns user)");
    // Cross-namespace private invocation fails with the operator
    // as written and the `#'` target (probed).
    assert_eq!(
        compile_err(&mut eval, "(fx/privfn 1)"),
        vec![
            "Syntax error compiling fx/privfn at (user:1:1).".to_owned(),
            "java.lang.IllegalStateException | var: #'fx/privfn is not public".to_owned(),
        ]
    );
    // Value position has no operator and no `#'` (probed).
    assert_eq!(
        compile_err(&mut eval, "fx/privfn"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.IllegalStateException | var: fx/privfn is not public".to_owned(),
        ]
    );
    // Private macros escape bare from every expansion path
    // (direct, nested, chained — probed).
    assert_eq!(
        uncaught(&mut eval, "(fx/privm 1)"),
        "java.lang.IllegalStateException | var: #'fx/privm is not public"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (fx/privm 1))"),
        "java.lang.IllegalStateException | var: #'fx/privm is not public"
    );
    // Value-position private macros check private first (probed).
    assert_eq!(
        compile_err(&mut eval, "fx/privm"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.IllegalStateException | var: fx/privm is not public".to_owned(),
        ]
    );
    // From `macroexpand-1` the `IllegalStateException` is bare;
    // non-macro privates expand to themselves.
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(fx/privm 1))"),
        "java.lang.IllegalStateException | var: #'fx/privm is not public"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 (list (var fx/privm) 1))"),
        "java.lang.IllegalStateException | var: #'fx/privm is not public"
    );
    assert_eq!(
        run_last(&mut eval, "(macroexpand-1 '(fx/privfn 1))"),
        "(fx/privfn 1)"
    );
    // The `var` form never checks; same-namespace uses are fine.
    assert_eq!(run_last(&mut eval, "(var fx/privfn)"), "#'fx/privfn");
    run_last(&mut eval, "(ns fx)");
    assert_eq!(run_last(&mut eval, "(privfn 1)"), "1");
    assert_eq!(run_last(&mut eval, "(privm 2)"), "2");
    run_last(&mut eval, "(ns user)");
}

#[test]
fn gensym_test() {
    let mut eval = boot();
    // `str` of the prefix plus the shared per-boot counter.
    assert_eq!(run_last(&mut eval, "(gensym)"), "G__1");
    assert_eq!(run_last(&mut eval, "(gensym \"p\")"), "p2");
    assert_eq!(run_last(&mut eval, "(gensym nil)"), "3");
    assert_eq!(run_last(&mut eval, "(gensym \"\")"), "4");
    assert_eq!(run_last(&mut eval, "(gensym 5)"), "55");
    assert_eq!(run_last(&mut eval, "(gensym 's)"), "s6");
    assert_eq!(run_last(&mut eval, "(resolve 'G__1)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(gensym 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/gensym"
    );
}

#[test]
fn declare_test() {
    let mut eval = boot();
    // Returns the last var; roots survive, `:declared` lands.
    assert_eq!(run_last(&mut eval, "(declare dc1 dc2)"), "#'user/dc2");
    assert_eq!(run_last(&mut eval, "(bound? #'dc1)"), "false");
    assert_eq!(run_last(&mut eval, "(:declared (meta #'dc1))"), "true");
    run_last(&mut eval, "(def dk 1)");
    assert_eq!(run_last(&mut eval, "(declare dk)"), "#'user/dk");
    assert_eq!(run_last(&mut eval, "dk"), "1");
    assert_eq!(run_last(&mut eval, "(:declared (meta #'dk))"), "true");
    assert_eq!(run_last(&mut eval, "(declare)"), "nil");
    // `^:dynamic` flows through.
    run_last(&mut eval, "(declare ^:dynamic dd)");
    assert_eq!(run_last(&mut eval, "(binding [dd 9] dd)"), "9");
    // Qualified names fail through `def`; non-symbols are the
    // bare `IObj` cast.
    assert_eq!(
        compile_err(&mut eval, "(declare q/nope)"),
        vec![
            "Syntax error compiling def at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Can't refer to qualified var that doesn't exist"
                .to_owned(),
        ]
    );
    assert_eq!(
        uncaught(&mut eval, "(declare 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IObj"
    );
}

#[test]
fn ns_basics() {
    let mut eval = boot();
    // Creates, switches, refers clojure, marks loaded, returns nil.
    assert_eq!(run_last(&mut eval, "(ns foo)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-name *ns*)"), "foo");
    assert_eq!(
        run_last(&mut eval, "(resolve 'list)"),
        "#'clojure.core/list"
    );
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*loaded-libs*))"),
        "#{foo}"
    );
    // Docstring and attrmap land as the ns meta.
    run_last(&mut eval, "(ns user)");
    run_last(&mut eval, "(ns foodoc \"d\" {:a 1})");
    assert_eq!(
        run_last(&mut eval, "(meta (the-ns 'foodoc))"),
        "{:a 1, :doc \"d\"}"
    );
    // The attrmap wins over the docstring for `:doc`.
    run_last(&mut eval, "(ns user)");
    run_last(&mut eval, "(ns foodoc2 \"d1\" {:doc \"d2\"})");
    assert_eq!(
        run_last(&mut eval, "(:doc (meta (the-ns 'foodoc2)))"),
        "\"d2\""
    );
    // Re-`ns` keeps interns (and clears meta without an attrmap).
    run_last(&mut eval, "(ns user)");
    run_last(&mut eval, "(ns foo2)");
    run_last(&mut eval, "(def keep 5)");
    run_last(&mut eval, "(ns foo2)");
    assert_eq!(run_last(&mut eval, "keep"), "5");
    run_last(&mut eval, "(ns user)");
}

#[test]
fn ns_errors() {
    let mut eval = boot();
    // Shape failures are the `ns` spec pair (the `*ns*` never
    // switches on failure).
    for src in [
        "(ns foo (:bogus 1))",
        "(ns 5)",
        "(ns \"s\")",
        "(ns foo/bar)",
        "(ns foo (:import String))",
        "(ns foo \"d1\" \"d2\")",
        "(ns)",
    ] {
        assert_eq!(
            compile_err(&mut eval, src),
            vec![
                "Syntax error macroexpanding clojure.core/ns at (user:1:1).".to_owned(),
                "clojure.lang.ExceptionInfo | Call to clojure.core/ns did not conform to spec."
                    .to_owned(),
            ],
            "{src}"
        );
    }
    assert_eq!(run_last(&mut eval, "(ns-name *ns*)"), "user");
    // Load failures surface raw (not as spec errors).
    assert_eq!(
        uncaught(&mut eval, "(ns baduser (:require [nonexistent-lib-xyz]))"),
        "java.io.FileNotFoundException | Could not locate nonexistent_lib_xyz__init.class, nonexistent_lib_xyz.clj or nonexistent_lib_xyz.cljc on classpath. Please check that namespaces with dashes use underscores in the Clojure file name."
    );
}

#[test]
fn ns_clauses() {
    let fx = Fixture::new("nsclauses");
    fx.write("fxlib.clj", "(ns fxlib)\n(def h 42)\n");
    let mut eval = boot();
    // `:require` with `:as` (flags flow through).
    run_last(
        &mut eval,
        &fx.within("(ns userns (:require [fxlib :as f] :reload))"),
    );
    assert_eq!(run_last(&mut eval, "(ns-name *ns*)"), "userns");
    assert_eq!(run_last(&mut eval, "f/h"), "42");
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*loaded-libs*))"),
        "#{fxlib userns}"
    );
    // `:use` refers publics.
    run_last(&mut eval, "(ns user)");
    run_last(&mut eval, &fx.within("(ns useuser (:use [fxlib]))"));
    assert_eq!(run_last(&mut eval, "h"), "42");
    // `:refer-clojure` filters the default refer.
    run_last(&mut eval, "(ns user)");
    run_last(&mut eval, "(ns excluser (:refer-clojure :exclude [list]))");
    assert_eq!(run_last(&mut eval, "(resolve 'list)"), "nil");
    assert_eq!(run_last(&mut eval, "(resolve 'seq)"), "#'clojure.core/seq");
    run_last(&mut eval, "(ns user)");
}

#[test]
fn builtin_macro_vars() {
    let mut eval = boot();
    // Every builtin resolves as a macro (divergent: the JVM has
    // real macros only for some, and `fn`/`let` expand further).
    for name in [
        "let",
        "loop",
        "fn",
        "binding",
        "letfn",
        "defmacro",
        "declare",
        "ns",
        "with-redefs",
        "with-bindings",
        "bound-fn",
    ] {
        assert_eq!(
            run_last(&mut eval, &format!("(resolve '{name})")),
            format!("#'clojure.core/{name}"),
            "{name}"
        );
        assert_eq!(
            run_last(&mut eval, &format!("(:macro (meta #'clojure.core/{name}))")),
            "true",
            "{name}"
        );
    }
    // Direct invocation fails loudly (the analyzer intercepts
    // every real form).
    assert_eq!(
        uncaught(&mut eval, "((var-get (var let)) 1)"),
        "java.lang.IllegalStateException | Can't invoke builtin macro: clojure.core/let"
    );
    // .. and the names have no value.
    assert_eq!(
        compile_err(&mut eval, "let"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.RuntimeException | Can't take value of a macro: #'clojure.core/let"
                .to_owned(),
        ]
    );
}

#[test]
fn ns_star_sync() {
    let mut eval = boot();
    run_last(&mut eval, "(create-ns 'syncfx)");
    run_last(&mut eval, "(ns syncfx)");
    run_last(&mut eval, "(def sh 7)");
    run_last(&mut eval, "(ns user)");
    // Analysis resolves against the bound `*ns*`.
    assert_eq!(
        run_last(&mut eval, "(binding [*ns* (the-ns 'syncfx)] (eval 'sh))"),
        "7"
    );
    // So does `def` installation.
    run_last(
        &mut eval,
        "(binding [*ns* (the-ns 'syncfx)] (eval '(def sh2 8)))",
    );
    run_last(&mut eval, "(ns syncfx)");
    assert_eq!(run_last(&mut eval, "sh2"), "8");
    run_last(&mut eval, "(ns user)");
}

#[test]
fn early_colls() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(list 1 2)"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(list)"), "()");
    assert_eq!(run_last(&mut eval, "(vector 1 2)"), "[1 2]");
    assert_eq!(run_last(&mut eval, "(vector)"), "[]");
    assert_eq!(run_last(&mut eval, "(hash-map :a 1 :b 2)"), "{:a 1, :b 2}");
    assert_eq!(run_last(&mut eval, "(hash-map)"), "{}");
    assert_eq!(
        uncaught(&mut eval, "(hash-map :a)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :a"
    );
    assert_eq!(run_last(&mut eval, "(hash-set 1 1 2)"), "#{1 2}");
    assert_eq!(run_last(&mut eval, "(hash-set)"), "#{}");
    assert_eq!(run_last(&mut eval, "(seq nil)"), "nil");
    assert_eq!(run_last(&mut eval, "(seq [])"), "nil");
    assert_eq!(run_last(&mut eval, "(seq [1])"), "(1)");
    assert_eq!(run_last(&mut eval, "(seq '(1 2))"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(seq \"ab\")"), "(\\a \\b)");
    assert_eq!(run_last(&mut eval, "(seq {:a 1})"), "([:a 1])");
    assert_eq!(run_last(&mut eval, "(seq #{1})"), "(1)");
    assert_eq!(
        uncaught(&mut eval, "(seq 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(concat)"), "()");
    assert_eq!(run_last(&mut eval, "(concat nil [1] nil [2])"), "(1 2)");
    assert_eq!(
        uncaught(&mut eval, "(concat 5)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(run_last(&mut eval, "(apply list [1 2])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(apply list 1 [2])"), "(1 2)");
    assert_eq!(run_last(&mut eval, "(apply list nil)"), "()");
    assert_eq!(
        uncaught(&mut eval, "(apply list 1 2)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Long"
    );
    assert_eq!(
        uncaught(&mut eval, "(apply list)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/apply"
    );
    assert_eq!(
        uncaught(&mut eval, "(apply)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/apply"
    );
}

#[test]
fn syntax_quote_macros() {
    let mut eval = boot();
    // Syntax-quote bodies exercise `seq`/`concat`/`list`/`apply`.
    run_last(&mut eval, "(defmacro msq [x] `(list ~x))");
    assert_eq!(run_last(&mut eval, "(msq 5)"), "(5)");
    // Auto-gensyms are hygienic (read-time fixed per form).
    run_last(&mut eval, "(defmacro mhyg [x] `(let [a# 0] (list a# ~x)))");
    assert_eq!(run_last(&mut eval, "(mhyg 5)"), "(0 5)");
    let expanded = run_last(&mut eval, "(macroexpand-1 '(mhyg 5))");
    assert!(
        expanded.starts_with("(clojure.core/let [a__") && expanded.contains("__auto__"),
        "{expanded}"
    );
    // Qualification is read-time (current ns at the `defmacro`).
    run_last(&mut eval, "(defmacro mqhundered [] `'x)");
    assert_eq!(run_last(&mut eval, "(mqhundered)"), "user/x");
    // Splicing, vector, and map templates.
    run_last(&mut eval, "(defmacro msp [x] `(list ~@x))");
    assert_eq!(run_last(&mut eval, "(msp [1 2])"), "(1 2)");
    run_last(&mut eval, "(defmacro mv [x] `[~x ~x])");
    assert_eq!(run_last(&mut eval, "(mv 1)"), "[1 1]");
    run_last(&mut eval, "(defmacro mm [x] `{:a ~x})");
    assert_eq!(run_last(&mut eval, "(mm 1)"), "{:a 1}");
    // Nested syntax-quote.
    run_last(&mut eval, "(defmacro mnest [x] `(list `(inner ~~x)))");
    assert_eq!(run_last(&mut eval, "(mnest 5)"), "((user/inner 5))");
}

#[test]
fn shadowable_new_forms() {
    let mut eval = boot();
    // Locals shadow the new builtins.
    assert_eq!(run_last(&mut eval, "(let [defmacro 1] defmacro)"), "1");
    assert_eq!(run_last(&mut eval, "(let [declare 2] declare)"), "2");
    assert_eq!(run_last(&mut eval, "(let [ns 3] ns)"), "3");
    // So do user globals (the builtin's own var does not).
    run_last(&mut eval, "(def declare 5)");
    assert_eq!(
        uncaught(&mut eval, "(declare)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}
