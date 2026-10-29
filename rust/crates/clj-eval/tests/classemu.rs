// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #60 (S16) class-emulation tests: every feasible pin in
//! `docs/zero-oos-s16-classemu/probes/*.out` reproduced against the
//! port, plus the documented divergences. Test names follow the
//! spec's §1.15 plan (`ns_imports_*`, `instance_*`, `type_*`,
//! `class_*`, `bases_*`, `supers_*`, `import_*`, `dotdot_*`,
//! `identical_*`, `printwriter_*`, `bean_*`, `cast_*`, `classq_*`,
//! `memfn_*`); each assertion cites its oracle row, and every
//! JVM-only row is skip-commented with its cause.
//!
//! Oracle command (see the spec's §2.6):
//! `CP=.../clojure-1.13.0-alpha10.jar:... java -Duser.language=en
//! -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main
//! docs/zero-oos-s16-classemu/probes/<probe>.clj`.

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw (macroexpansion-time
/// throws surface as `Compile`, so both are accepted).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// A compile error's `Class | message` (panics unless the failure
/// phase is compile).
fn compile_err(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(vs) => panic!("expected a compile error, got values: {vs:?}"),
    }
}

/// Whether the failure is a bare compile error (no wrapper).
fn compile_is_bare(eval: &mut Evaluator, src: &str) -> bool {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => e.bare,
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(vs) => panic!("expected a compile error, got values: {vs:?}"),
    }
}

// Both sides bootstrap-loaded: one shared `java.base` clause (the
// port's `cce_message` matches the JVM here).
fn cce_bootstrap(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} and {to} are in module java.base of loader 'bootstrap')"
    )
}

// A host-side name against a bootstrap target: the cross-loader
// clause (host descriptors in class position).
fn cce_cross(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} is in unnamed module of loader 'app'; {to} is in module java.base of loader 'bootstrap')"
    )
}

// The oracle's `normctr`: scrub the eval/fn counters so closure
// displays compare (`user$eval103$fn__148` → `user$evalN$fn__N`).
fn normctr(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 'l' && out.ends_with("eva") && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            out.push('l');
            out.push('N');
            while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                chars.next();
            }
        } else if c == '_' && chars.peek() == Some(&'_') {
            let mut lookahead = chars.clone();
            lookahead.next();
            if lookahead.peek().is_some_and(|d| d.is_ascii_digit()) {
                out.push_str("__N");
                chars.next();
                while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                    chars.next();
                }
            } else {
                out.push(c);
            }
        } else {
            out.push(c);
        }
    }
    out
}

// ---- smoke ----

#[test]
fn smoke_import_and_class() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(import 'java.lang.String)"),
        "java.lang.String"
    );
    assert_eq!(run(&mut eval, "(class \"a\")"), "java.lang.String");
    assert_eq!(
        run(&mut eval, "(str (class \"a\"))"),
        "\"class java.lang.String\""
    );
    assert_eq!(run(&mut eval, "(instance? String \"a\")"), "true");
}

// ---- import ----

#[test]
fn import_expansions() {
    let mut eval = boot();
    // i-expand-quote/bare: the port fans out to the hidden `__import*`
    // (the JVM emits `clojure.core/import*`).
    assert_eq!(
        run(&mut eval, "(macroexpand-1 '(import 'java.util.Date))"),
        "(do (clojure.core/__import* \"java.util.Date\"))"
    );
    assert_eq!(
        run(&mut eval, "(macroexpand-1 '(import java.util.Date))"),
        "(do (clojure.core/__import* \"java.util.Date\"))"
    );
    // i-expand-list/list-bare: package lists fan out per class.
    assert_eq!(
        run(
            &mut eval,
            "(macroexpand-1 '(import '(java.util Date Calendar)))"
        ),
        "(do (clojure.core/__import* \"java.util.Date\") (clojure.core/__import* \"java.util.Calendar\"))"
    );
    assert_eq!(
        run(&mut eval, "(macroexpand-1 '(import (java.util Date)))"),
        "(do (clojure.core/__import* \"java.util.Date\"))"
    );
    // i-expand-multi: specs concatenate in order.
    assert_eq!(
        run(
            &mut eval,
            "(macroexpand-1 '(import 'java.lang.Long '(java.util Date Calendar)))"
        ),
        "(do (clojure.core/__import* \"java.lang.Long\") (clojure.core/__import* \"java.util.Date\") (clojure.core/__import* \"java.util.Calendar\"))"
    );
}

#[test]
fn import_pred_and_ret() {
    let mut eval = boot();
    // `import` is a macro (the port has no `macro?`, so the
    // `:macro` meta flag proves it).
    assert_eq!(
        run(&mut eval, "(:macro (meta #'clojure.core/import))"),
        "true"
    );
    // i-ret: `import` returns the last class.
    assert_eq!(run(&mut eval, "(import 'java.util.Date)"), "java.util.Date");
    // i-import-multi-ret: the last class of a multi-spec import.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impmulti) (clojure.core/refer-clojure) (import 'java.util.Date '(java.util Calendar)))"
        ),
        "java.util.Calendar"
    );
}

#[test]
fn import_validation() {
    let mut eval = boot();
    // Every malformed spec is the spec error at expansion (cause
    // level): i-import-nil/empty/slash/num/badlist/lonely/qual-pkg/
    // qual-cls/list-str.
    let msg = "Call to clojure.core/import did not conform to spec.";
    for src in [
        "(import nil)",
        "(import '())",
        "(import 'java.util/Date)",
        "(import 5)",
        "(import '((a) B))",
        "(import '(java.util))",
        "(import '(a/b C))",
        "(import '(p a/b))",
        "(import '(p \"C\"))",
        "(import :kw)",
    ] {
        let err = uncaught(&mut eval, src);
        assert!(
            err == format!("clojure.lang.ExceptionInfo | {msg}"),
            "{src}: {err}"
        );
    }
    // i-expand-str: the expansion is lazy (the `mapcat` tail), so
    // `macroexpand-1` alone answers the unforced `(do ...)`; forcing
    // it (the oracle's `pr-str`) throws the bare spec error — the
    // JVM behaves the same way (its `mapcat` is lazy too).
    assert_eq!(
        uncaught(&mut eval, "(pr-str (macroexpand-1 '(import \"x\")))"),
        format!("clojure.lang.ExceptionInfo | {msg}")
    );
    // i-import-exdata: the oracle source verbatim — the message
    // matches and the port carries nil data (the JVM's spec
    // internals are unportable by design).
    assert_eq!(
        run(
            &mut eval,
            "(in-ns 'impexd) (clojure.core/refer-clojure) (try (eval '(clojure.core/import 5)) (catch Throwable t (let [c (loop [x t] (let [n (ex-cause x)] (if n (recur n) x)))] [(ex-message c) (sort (map str (keys (ex-data c))))])))"
        ),
        format!("[\"{msg}\" ()]")
    );
}

#[test]
fn import_hidden_native() {
    let mut eval = boot();
    // i-import*-resolve/import*-resolve-q: the JVM's `import*` is
    // compiler magic and resolves to nil — the port has no such var
    // either.
    assert_eq!(run(&mut eval, "(resolve 'import*)"), "nil");
    assert_eq!(run(&mut eval, "(resolve 'clojure.core/import*)"), "nil");
    // The port's `__import*` stands like `__new*`: a real public
    // var (referred, callable cross-namespace so the `import` macro
    // analyzes, unlisted in the matrix).
    assert_eq!(
        run(
            &mut eval,
            "[(resolve '__import*) (resolve 'clojure.core/__import*) (resolve '__new*)]"
        ),
        "[#'clojure.core/__import* #'clojure.core/__import* #'clojure.core/__new*]"
    );
    // A non-string arg fails the `String` cast (the macro only ever
    // emits strings, so this is direct-call misuse).
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__import* 5)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

#[test]
fn import_unknown() {
    let mut eval = boot();
    // i-unknown/unknown-list/import-package: ClassNotFoundException
    // with the bare name (packages are not classes either).
    assert_eq!(
        uncaught(&mut eval, "(import 'foo.bar.Baz)"),
        "java.lang.ClassNotFoundException | foo.bar.Baz"
    );
    assert_eq!(
        uncaught(&mut eval, "(import '(foo.bar Baz))"),
        "java.lang.ClassNotFoundException | foo.bar.Baz"
    );
    assert_eq!(
        uncaught(&mut eval, "(import 'java.util)"),
        "java.lang.ClassNotFoundException | java.util"
    );
    // Dotless names never resolve (the JVM's `Class.forName`
    // likewise rejects them).
    assert_eq!(
        uncaught(&mut eval, "(import 'Point)"),
        "java.lang.ClassNotFoundException | Point"
    );
}

#[test]
fn import_dup_and_conflicts() {
    let mut eval = boot();
    // i-dup: the same class re-imports as a no-op returning it.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impdup) (clojure.core/refer-clojure) (import 'java.util.Date) (import 'java.util.Date))"
        ),
        "java.util.Date"
    );
    // i-conflict-class: a different class under the same simple name
    // is the `already refers to` state error.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (in-ns 'impcc) (clojure.core/refer-clojure) (import 'java.util.Date) (import 'java.sql.Date))"
        ),
        "java.lang.IllegalStateException | Date already refers to: class java.util.Date in namespace: impcc"
    );
    // i-conflict-def: over a var mapping is the module-shape
    // Var → Class cast failure (byte-equal to the oracle).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (in-ns 'impcdef) (clojure.core/refer-clojure) (def Date 1) (import 'java.util.Date))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class java.lang.Class (clojure.lang.Var is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
    // i-import-over-refer: over a referred var is the same failure.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (in-ns 'impa) (clojure.core/refer-clojure) (def Date 1) (in-ns 'impovr) (clojure.core/refer-clojure) (clojure.core/refer 'impa :only '[Date]) (import 'java.util.Date))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class java.lang.Class (clojure.lang.Var is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
    // Over a var holding an instance (not the same-name descriptor):
    // the same `Var` → `Class` cast failure.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (in-ns 'impinst) (clojure.core/refer-clojure) (deftype IDate [x]) (def Date (IDate. 1)) (import 'java.util.Date))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class java.lang.Class (clojure.lang.Var is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
}

#[test]
fn import_same_def_leniency() {
    let mut eval = boot();
    // A var holding the `HostVal::Def` of the same qualified name
    // imports as a no-op success (importing a type in its defining
    // namespace must not conflict when the JVM no-ops).
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype LenPoint [x]) (import 'user.LenPoint))"
        ),
        "user.LenPoint"
    );
    // ... but no import is recorded (the var still resolves).
    assert_eq!(run(&mut eval, "(class? (resolve 'LenPoint))"), "false");
    // A var holding any other descriptor still conflicts.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (deftype LenA [x]) (deftype LenB [x]) (def LenB LenA) (import 'user.LenB))"
        ),
        "java.lang.ClassCastException | class clojure.lang.Var cannot be cast to class java.lang.Class (clojure.lang.Var is in unnamed module of loader 'app'; java.lang.Class is in module java.base of loader 'bootstrap')"
    );
    // A var rebound to an instance leaves no `HostDef` of the
    // name, so the import is unknown (like the JVM's failed load).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (deftype LenC [x]) (def LenC (LenC. 1)) (import 'user.LenC))"
        ),
        "java.lang.ClassNotFoundException | user.LenC"
    );
    // A var holding a plain value is unknown the same way.
    assert_eq!(
        uncaught(&mut eval, "(do (def Namb 1) (import 'user.Namb))"),
        "java.lang.ClassNotFoundException | user.Namb"
    );
}

#[test]
fn import_def_over_import() {
    let mut eval = boot();
    // i-def-over-import: `def` over an imported name is a compile
    // error. Two phases: the import commits, then the `def` fails to
    // analyze (one form cannot do both — analysis precedes runtime).
    run(
        &mut eval,
        "(do (in-ns 'impdef) (clojure.core/refer-clojure) (import 'java.util.Date))",
    );
    assert_eq!(
        compile_err(&mut eval, "(def Date 1)"),
        "java.lang.RuntimeException | Expecting var, but Date is mapped to class java.util.Date"
    );
    // The oracle's `eval` shape: the same cause under the
    // `CompilerException` wrap (cause-walked like `throwcause`).
    assert_eq!(
        run(
            &mut eval,
            "(try (eval '(def Date 1)) (catch Throwable t (let [c (loop [x t] (let [n (ex-cause x)] (if n (recur n) x)))] [(str (class c)) (ex-message c)])))"
        ),
        "[\"class java.lang.RuntimeException\" \"Expecting var, but Date is mapped to class java.util.Date\"]"
    );
    // i-def-over-import-after: the failed `def` leaves no var (the
    // port has no `find-var`, so `ns-interns` proves the absence)
    // and the import resolves.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impdef2) (clojure.core/refer-clojure) (import 'java.util.Date) (try (eval '(def Date 1)) (catch Throwable t nil)) [(resolve 'Date) (contains? (ns-interns 'impdef2) 'Date)])"
        ),
        "[java.util.Date false]"
    );
}

#[test]
fn import_intern_refer_unmap() {
    let mut eval = boot();
    // i-intern-over-import: `intern` silently replaces the import
    // (the port stays silent where the JVM warns on `*err*` —
    // captured through a `PrintWriter-on`).
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impin) (clojure.core/refer-clojure) (import 'java.util.Date) (let [warns (atom []) pw (PrintWriter-on (fn [s] (swap! warns conj s)) nil)] (binding [*err* pw] (intern 'impin 'Date 5)) (.flush pw) [(resolve 'Date) (get (ns-imports 'impin) 'Date) @warns]))"
        ),
        "[#'impin/Date nil []]"
    );
    // i-refer-over-import / i-refer-removes-import: `refer` wins
    // resolution and removes the import.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impc) (clojure.core/refer-clojure) (def Date 1) (in-ns 'imprf) (clojure.core/refer-clojure) (import 'java.util.Date) (clojure.core/refer 'impc :only '[Date]) (resolve 'Date))"
        ),
        "#'impc/Date"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impg) (clojure.core/refer-clojure) (def Date 1) (in-ns 'imph) (clojure.core/refer-clojure) (import 'java.util.Date) (clojure.core/refer 'impg :only '[Date]) [(resolve 'Date) (get (ns-imports 'imph) 'Date)])"
        ),
        "[#'impg/Date nil]"
    );
    // i-refer-warn: the JVM warns; the port stays silent.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impe) (clojure.core/refer-clojure) (def Date 1) (in-ns 'impf) (clojure.core/refer-clojure) (import 'java.util.Date) (let [warns (atom []) pw (PrintWriter-on (fn [s] (swap! warns conj s)) nil)] (binding [*err* pw] (clojure.core/refer 'impe :only '[Date])) (.flush pw) @warns))"
        ),
        "[]"
    );
    // i-unmap: `ns-unmap` drops the import too.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impun) (clojure.core/refer-clojure) (import 'java.util.Date) (ns-unmap 'impun 'Date) [(resolve 'Date) (contains? (ns-imports 'impun) 'Date)])"
        ),
        "[nil false]"
    );
}

#[test]
fn import_deftype_roundtrip() {
    let mut eval = boot();
    // i-deftype-import/construct: host types import by qualified name
    // and construct through the import.
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype ImpPoint [x]) (in-ns 'imp8) (clojure.core/refer-clojure) (import 'user.ImpPoint) (resolve 'ImpPoint))"
        ),
        "user.ImpPoint"
    );
    assert_eq!(
        run(&mut eval, "(.getName ^Class (class (eval '(ImpPoint. 1))))"),
        "\"user.ImpPoint\""
    );
    // i-construct: `(Date. 0)` constructs through the import on the
    // JVM; the port has no `Date` values, so the `__new*` class arm
    // reports the missing `HostDef` (forced divergence).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (in-ns 'impdt) (clojure.core/refer-clojure) (import 'java.util.Date) (eval '(Date. 0)))"
        ),
        "java.lang.IllegalArgumentException | Not a type: java.util.Date"
    );
}

#[test]
fn construct_via_class_values() {
    // The `__new*` class arm (§1.8): a class value resolves through
    // its name to a `HostDef` and constructs; a JDK name with no
    // `HostDef` is `Not a type`; an interface def is the missing-ctor
    // error.
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype NCV [x]) (let [c (import 'user.NCV)] (class (clojure.core/__new* c 5))))"
        ),
        "user.NCV"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.String) (clojure.core/__new* String \"x\"))"
        ),
        "java.lang.IllegalArgumentException | Not a type: java.lang.String"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (definterface NCI (m [x])) (let [c (import 'user.NCI)] (clojure.core/__new* c)))"
        ),
        "java.lang.IllegalArgumentException | No matching ctor found for class user.NCI"
    );
}

#[test]
fn import_struct_name() {
    let mut eval = boot();
    // A struct name resolves (it is a `HostDef`) and carves to the
    // reported `$Def` class.
    assert_eq!(
        run(
            &mut eval,
            "(do (defstruct isq :a) (in-ns 'impst) (clojure.core/refer-clojure) (import 'user.isq) (str (resolve 'isq)))"
        ),
        "\"class clojure.lang.PersistentStructMap$Def\""
    );
}

// i-fqn-eval: the JVM evaluates bare `java.lang.String` to the
// class; the port keeps rejecting dotted bare names (no honorable
// `Class.forName`), pinned here and by the stage-1 suite.
#[test]
fn import_fqn_still_rejected() {
    let mut eval = boot();
    assert_eq!(
        compile_err(&mut eval, "java.lang.String"),
        "java.lang.RuntimeException | Unable to resolve symbol: java.lang.String in this context"
    );
    // The oracle's `eval` shape: the same rejection under the
    // `CompilerException` wrap (cause-walked like `throwcause`).
    assert_eq!(
        run(
            &mut eval,
            "(try (eval 'java.lang.String) (catch Throwable t (ex-message (loop [x t] (let [n (ex-cause x)] (if n (recur n) x))))))"
        ),
        "\"Unable to resolve symbol: java.lang.String in this context\""
    );
}

// i-ns-clause: the JVM's `(ns impns2 (:import ...))` imports; the
// port rejects the `:import` clause (existing `ns` pins).
#[test]
fn import_ns_clause_still_rejected() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(ns impns2 (:import java.util.Date))"),
        "clojure.lang.ExceptionInfo | Call to clojure.core/ns did not conform to spec."
    );
}

// i-static-field, i-static-call: no statics surface exists on the
// port (`Integer/MAX_VALUE`, `Integer/parseInt` are follow-ups) —
// JVM-only rows, skip-commented with cause.

// ---- ns-imports ----

#[test]
fn ns_imports_table() {
    let mut eval = boot();
    // i-ns-imports: only explicit imports are listed (the JVM's ~100
    // defaults are JVM-only).
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'imp1) (clojure.core/refer-clojure) (import 'java.util.Date) (sort (map str (keys (ns-imports 'imp1)))))"
        ),
        "(\"Date\")"
    );
    // i-ns-imports-nsobj/val: `ns-imports` maps simple names to
    // classes (namespace objects coerce like symbols).
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impni) (clojure.core/refer-clojure) (import 'java.util.Date) (get (ns-imports (the-ns 'impni)) 'Date))"
        ),
        "java.util.Date"
    );
    // i-ns-map: `ns-map` layers imports under refers/interns.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impnm) (clojure.core/refer-clojure) (import 'java.util.Date) (get (ns-map 'impnm) 'Date))"
        ),
        "java.util.Date"
    );
    // i-ns-publics/interns: imports are not vars.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impnp) (clojure.core/refer-clojure) (import 'java.util.Date) [(contains? (ns-publics 'impnp) 'Date) (contains? (ns-interns 'impnp) 'Date)])"
        ),
        "[false false]"
    );
    // i-resolve/ns-resolve/value-pos: resolution answers the class.
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'imprs) (clojure.core/refer-clojure) (import 'java.lang.Number) [(resolve 'Number) (ns-resolve 'imprs 'Number) (eval 'Number)])"
        ),
        "[java.lang.Number java.lang.Number java.lang.Number]"
    );
    // i-defaults: a fresh namespace imports nothing (the JVM's ~100
    // default imports are JVM-only; the port installs none).
    assert_eq!(
        run(
            &mut eval,
            "(do (create-ns 'impfresh) (sort (map str (keys (ns-imports 'impfresh)))))"
        ),
        "()"
    );
    assert_eq!(run(&mut eval, "(ns-imports 'impfresh)"), "{}");
}

#[test]
fn ns_imports_coercions() {
    let mut eval = boot();
    // i-ns-imports-str: a string arg is the short-shape `String` →
    // `Symbol` cast failure (the oracle shows the module shape).
    assert_eq!(
        uncaught(&mut eval, "(ns-imports \"x\")"),
        "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Symbol"
    );
    // i-ns-imports-nil: nil is the missing-key NPE (byte-equal to
    // the oracle through the shared helper).
    assert_eq!(
        uncaught(&mut eval, "(ns-imports nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.hashCode()\" because \"key\" is null"
    );
    // i-ns-imports-missing: unknown namespaces error.
    assert_eq!(
        uncaught(&mut eval, "(ns-imports 'nosuchnsxyz)"),
        "java.lang.Exception | No namespace: nosuchnsxyz found"
    );
}

// ---- instance? ----
//
// The oracle sources use short names (`String`, `Object`, ...) via
// the JVM's default imports. The port has none (§1.7), so each test
// imports what its rows name; the pinned VALUES are byte-equal.

// c-inst-basic-t/basic-f, c-inst-object/object-nil/nil-x, c-inst-num,
// c-inst-class, c-inst-ho, c-inst-order,
// c-inst-iface/serial/comparable: membership through the lattice
// (FQNs import first — bare dotted names stay unresolvable, §1.8).
#[test]
fn instance_basics() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Long 'java.lang.Object 'java.lang.Number 'java.lang.Comparable 'java.lang.Class 'java.io.Serializable 'clojure.lang.IPersistentVector)",
    );
    assert_eq!(run(&mut eval, "(instance? String \"a\")"), "true");
    assert_eq!(run(&mut eval, "(instance? String 1)"), "false");
    assert_eq!(run(&mut eval, "(instance? Object \"a\")"), "true");
    assert_eq!(run(&mut eval, "(instance? Object nil)"), "false");
    assert_eq!(run(&mut eval, "(instance? String nil)"), "false");
    assert_eq!(run(&mut eval, "(instance? Number 1)"), "true");
    assert_eq!(run(&mut eval, "(instance? Class String)"), "true");
    assert_eq!(run(&mut eval, "(instance? IPersistentVector [1])"), "true");
    assert_eq!(run(&mut eval, "(instance? Serializable \"a\")"), "true");
    assert_eq!(run(&mut eval, "(instance? Comparable 1)"), "true");
    // c-inst-ho: higher-order through the var.
    assert_eq!(
        run(&mut eval, "(map instance? [String Long] [\"a\" 1])"),
        "(true true)"
    );
    // c-inst-order: the class position evaluates first.
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom [])] [(instance? (do (swap! a conj :c) String) (do (swap! a conj :x) \"a\")) @a])"
        ),
        "[true [:c :x]]"
    );
}

// c-inst-iseq-t/f: the `ISeq` shape agrees with the `seq?` rewrite.
#[test]
fn instance_seq_rewrite() {
    let mut eval = boot();
    run(&mut eval, "(import 'clojure.lang.ISeq)");
    assert_eq!(run(&mut eval, "(instance? ISeq '())"), "true");
    assert_eq!(run(&mut eval, "(instance? ISeq [])"), "false");
    // A direct qualified 2-arg call falls through to the var.
    assert_eq!(run(&mut eval, "(clojure.core/instance? ISeq '())"), "true");
    // c-inst-macroexpand/resolve: no macro; a real var.
    assert_eq!(
        run(&mut eval, "(macroexpand '(instance? String \"a\"))"),
        "(instance? String \"a\")"
    );
    assert_eq!(
        run(&mut eval, "(resolve 'instance?)"),
        "#'clojure.core/instance?"
    );
}

// c-inst-var-apply/var-apply-f/var-arity: first-class use through the var.
#[test]
fn instance_var_calls() {
    let mut eval = boot();
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(
        run(
            &mut eval,
            "(apply @#'clojure.core/instance? [String \"a\"])"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(apply @#'clojure.core/instance? [String 1])"),
        "false"
    );
    assert_eq!(
        uncaught(&mut eval, "(apply @#'clojure.core/instance? [String])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/instance?"
    );
    // c-inst-var-class: the root is a counter-free native (the JVM
    // reports its `instance_QMARK__` fn with a build counter).
    assert_eq!(
        normctr(&run(
            &mut eval,
            "(.getName ^Class (class @#'clojure.core/instance?))"
        )),
        "\"clojure.core$instance_QMARK_\""
    );
}

// c-inst-arity1/3: unqualified wrong arities fail at runtime (minus
// the JVM's `--N` defn suffix); direct qualified calls reject at
// compile time instead (same text — §2.4 phase divergence).
#[test]
fn instance_arities() {
    let mut eval = boot();
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(
        uncaught(&mut eval, "(instance? String)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/instance?"
    );
    assert_eq!(
        uncaught(&mut eval, "(instance? String \"a\" \"b\")"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/instance?"
    );
    assert_eq!(
        compile_err(&mut eval, "(clojure.core/instance? String)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/instance?"
    );
    assert!(!compile_is_bare(
        &mut eval,
        "(clojure.core/instance? String)"
    ));
    assert_eq!(
        compile_err(&mut eval, "(clojure.core/instance? String \"a\" \"b\")"),
        "clojure.lang.ArityException | Wrong number of args (3) passed to: clojure.core/instance?"
    );
    assert!(!compile_is_bare(
        &mut eval,
        "(clojure.core/instance? String \"a\" \"b\")"
    ));
}

// c-inst-nil-class/nonclass: nil is the JVM-exact NPE, a non-class
// the module-shape cast failure.
#[test]
fn instance_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(instance? nil \"a\")"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.isInstance(Object)\" because \"c\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(instance? 1 \"a\")"),
        cce_bootstrap("java.lang.Long", "java.lang.Class")
    );
}

// Class-position descriptors fold through the registry (records,
// deftypes, interfaces); instances, struct descriptors, and folded
// names with no entry are the cross-loader cast failure (§1.5).
#[test]
fn instance_descriptors() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord IQRec [x]) (def IQI (IQRec. 1)) [(instance? IQRec IQI) (instance? IQRec {:x 1})])"
        ),
        "[true false]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype IQDet [x]) (def IQD (IQDet. 1)) (instance? IQDet IQD))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface IQIfc (m [x])) (instance? IQIfc (reify IQIfc (m [t x] x))))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defrecord IQRec2 [x]) (instance? (IQRec2. 1) {:x 1}))"
        ),
        cce_cross("user.IQRec2", "java.lang.Class")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (defstruct iqs :a) (instance? iqs (struct iqs 1)))"
        ),
        cce_cross("clojure.lang.PersistentStructMap$Def", "java.lang.Class")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (deftype IQGone [x]) (def IQG IQGone) (ns-unmap *ns* 'IQGone) (instance? IQG 1))"
        ),
        cce_cross("user.IQGone", "java.lang.Class")
    );
}

// ---- type ----

// c-type-plain/nil/nometa/class/meta/meta-str/meta-nil/meta-false/
// sym-meta + c-type-arity0/2: `(or (:type (meta x)) (class x))`.
#[test]
fn type_queries() {
    let mut eval = boot();
    run(&mut eval, "(import 'java.lang.String)");
    assert_eq!(run(&mut eval, "(type \"a\")"), "java.lang.String");
    assert_eq!(run(&mut eval, "(type nil)"), "nil");
    assert_eq!(run(&mut eval, "(type 1)"), "java.lang.Long");
    assert_eq!(run(&mut eval, "(type String)"), "java.lang.Class");
    assert_eq!(
        run(&mut eval, "(type (with-meta [1] {:type :foo}))"),
        ":foo"
    );
    assert_eq!(
        run(&mut eval, "(type (with-meta [1] {:type \"bar\"}))"),
        "\"bar\""
    );
    assert_eq!(
        run(&mut eval, "(type (with-meta [1] {:type nil}))"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(
        run(&mut eval, "(type (with-meta [1] {:type false}))"),
        "clojure.lang.PersistentVector"
    );
    assert_eq!(run(&mut eval, "(type (with-meta 's {:type :t}))"), ":t");
    assert_eq!(
        uncaught(&mut eval, "(type)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/type"
    );
    assert_eq!(
        uncaught(&mut eval, "(type 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/type"
    );
}

// ---- class ----

// c-class-str/nil/int/bool/char/kw/sym/vec/map/set/list/var/regex/
// ns/bigint/bigdec/ratio/double/lazy/cons-nil/cons/range/subvec/entry/
// delay/atom/agent/volatile/exinfo/out/err/class/multifn/sortedmap/
// sortedset/emptylist + c-class-arity0/2: one class per kind.
#[test]
fn class_queries() {
    let mut eval = boot();
    run(&mut eval, "(import 'java.lang.String)");
    let cases = [
        ("(class \"a\")", "java.lang.String"),
        ("(class nil)", "nil"),
        ("(class 1)", "java.lang.Long"),
        ("(class true)", "java.lang.Boolean"),
        ("(class \\a)", "java.lang.Character"),
        ("(class :a)", "clojure.lang.Keyword"),
        ("(class 'a)", "clojure.lang.Symbol"),
        ("(class [1])", "clojure.lang.PersistentVector"),
        ("(class {:a 1})", "clojure.lang.PersistentArrayMap"),
        ("(class #{1})", "clojure.lang.PersistentHashSet"),
        ("(class '(1))", "clojure.lang.PersistentList"),
        ("(class #'map)", "clojure.lang.Var"),
        ("(class #\"x\")", "java.util.regex.Pattern"),
        ("(class *ns*)", "clojure.lang.Namespace"),
        ("(class (bigint 5))", "clojure.lang.BigInt"),
        ("(class 1.5M)", "java.math.BigDecimal"),
        ("(class 1/3)", "clojure.lang.Ratio"),
        ("(class 1.5)", "java.lang.Double"),
        ("(class (map inc [1]))", "clojure.lang.LazySeq"),
        ("(class (cons 1 nil))", "clojure.lang.PersistentList"),
        ("(class (cons 1 '(2)))", "clojure.lang.Cons"),
        ("(class (range 3))", "clojure.lang.LongRange"),
        (
            "(class (subvec [1 2] 0 1))",
            "clojure.lang.APersistentVector$SubVector",
        ),
        ("(class (first {:a 1}))", "clojure.lang.MapEntry"),
        ("(class (delay 1))", "clojure.lang.Delay"),
        ("(class (atom 1))", "clojure.lang.Atom"),
        ("(class (agent 1))", "clojure.lang.Agent"),
        ("(class (volatile! 1))", "clojure.lang.Volatile"),
        ("(class (ex-info \"x\" {}))", "clojure.lang.ExceptionInfo"),
        ("(class *out*)", "java.io.OutputStreamWriter"),
        ("(class *err*)", "java.io.PrintWriter"),
        ("(class String)", "java.lang.Class"),
        ("(do (defmulti cm :k) (class cm))", "clojure.lang.MultiFn"),
        (
            "(class (sorted-map :a 1))",
            "clojure.lang.PersistentTreeMap",
        ),
        ("(class (sorted-set 1))", "clojure.lang.PersistentTreeSet"),
        ("(class '())", "clojure.lang.PersistentList$EmptyList"),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // c-class-fn: the closure display, counters scrubbed (byte-equal
    // to the oracle after `normctr`).
    assert_eq!(
        normctr(&run(&mut eval, "(.getName ^Class (class (fn [x] x)))")),
        "\"user$evalN$fn__N\""
    );
    // c-class-arith: the oracle constructs `(ArithmeticException.
    // "x")`; the port has no JDK construction (§1.8), so the same
    // class arrives via a caught divide-by-zero.
    assert_eq!(
        run(
            &mut eval,
            "(try (/ 1 0) (catch ArithmeticException t (class t)))"
        ),
        "java.lang.ArithmeticException"
    );
    // c-class-object: `(Object.)` is the §1.8 miss (a JDK name with
    // no `HostDef`); the oracle VALUE is JVM-only — no expression in
    // the port evaluates to a bare `Object`.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.lang.Object) (class (Object.)))"
        ),
        "java.lang.IllegalArgumentException | Not a type: java.lang.Object"
    );
    // c-class-tagged: `tagged-literal` has no port constructor and no
    // bare tagged value is surface-reachable — JVM-only row.
    // c-class-readercond: the constructor is out of scope; the same
    // class arrives through the preserving reader.
    assert_eq!(
        run(
            &mut eval,
            "(class (read-string {:read-cond :preserve} \"#?(1)\"))"
        ),
        "clojure.lang.ReaderConditional"
    );
    // c-class-arity0/2.
    assert_eq!(
        uncaught(&mut eval, "(class)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/class"
    );
    assert_eq!(
        uncaught(&mut eval, "(class 1 2)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/class"
    );
}

// c-name-eduction/reduced/future/promise/iteration/reader/chunk/
// chunkbuf/matcher/struct/thash/tvec/tarr/tset/seq-smallmap/seq-set/
// seq-sortedset/keys/keys-big/vals/rseq/subvec-seq/sort/str-seq/
// vec-seq/native/native-fixed: port-reported names verified against
// the oracle (counters scrubbed where the JVM counts).
#[test]
fn class_names() {
    let mut eval = boot();
    let cases = [
        (
            "(.getName ^Class (class (eduction (map inc) [1])))",
            "\"clojure.core.Eduction\"",
        ),
        (
            "(.getName ^Class (class (reduced 1)))",
            "\"clojure.lang.Reduced\"",
        ),
        (
            "(.getName ^Class (class *in*))",
            "\"clojure.lang.LineNumberingPushbackReader\"",
        ),
        (
            "(let [b (chunk-buffer 2)] (chunk-append b 1) (.getName ^Class (class (chunk b))))",
            "\"clojure.lang.ArrayChunk\"",
        ),
        (
            "(.getName ^Class (class (chunk-buffer 2)))",
            "\"clojure.lang.ChunkBuffer\"",
        ),
        (
            "(.getName ^Class (class (re-matcher #\"x\" \"x\")))",
            "\"java.util.regex.Matcher\"",
        ),
        (
            "(do (defstruct cs :a) [(.getName ^Class (class (struct cs 1))) (.getName ^Class (class cs)) (.getName ^Class (class (accessor cs :a)))])",
            "[\"clojure.lang.PersistentStructMap\" \"clojure.lang.PersistentStructMap$Def\" \"clojure.lang.PersistentStructMap$1\"]",
        ),
        (
            "(.getName ^Class (class (transient (into {} (map (fn [i] [i i]) (range 20))))))",
            "\"clojure.lang.PersistentHashMap$TransientHashMap\"",
        ),
        (
            "(.getName ^Class (class (transient [1])))",
            "\"clojure.lang.PersistentVector$TransientVector\"",
        ),
        (
            "(.getName ^Class (class (transient {:a 1})))",
            "\"clojure.lang.PersistentArrayMap$TransientArrayMap\"",
        ),
        (
            "(.getName ^Class (class (transient #{1})))",
            "\"clojure.lang.PersistentHashSet$TransientHashSet\"",
        ),
        (
            "(.getName ^Class (class (seq {:a 1})))",
            "\"clojure.lang.PersistentArrayMap$Seq\"",
        ),
        (
            "(.getName ^Class (class (seq #{1 2})))",
            "\"clojure.lang.APersistentMap$KeySeq\"",
        ),
        (
            "(.getName ^Class (class (seq (sorted-set 1 2))))",
            "\"clojure.lang.APersistentMap$KeySeq\"",
        ),
        (
            "(.getName ^Class (class (keys {:a 1})))",
            "\"clojure.lang.APersistentMap$KeySeq\"",
        ),
        (
            "(.getName ^Class (class (keys (into {} (map (fn [i] [i i]) (range 30))))))",
            "\"clojure.lang.APersistentMap$KeySeq\"",
        ),
        (
            "(.getName ^Class (class (vals {:a 1})))",
            "\"clojure.lang.APersistentMap$ValSeq\"",
        ),
        (
            "(.getName ^Class (class (rseq [1 2])))",
            "\"clojure.lang.APersistentVector$RSeq\"",
        ),
        (
            "(.getName ^Class (class (seq (subvec [1 2 3] 0 2))))",
            "\"clojure.lang.APersistentVector$Seq\"",
        ),
        (
            "(.getName ^Class (class (sort [2 1])))",
            "\"clojure.lang.ArraySeq\"",
        ),
        (
            "(.getName ^Class (class (seq \"ab\")))",
            "\"clojure.lang.StringSeq\"",
        ),
        (
            "(.getName ^Class (class (seq [1 2])))",
            "\"clojure.lang.PersistentVector$ChunkedSeq\"",
        ),
        ("(.getName ^Class (class map))", "\"clojure.core$map\""),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // c-name-future/promise/iteration: the port reports the stable
    // counter-free names (the JVM mints per-call-site reify classes).
    assert_eq!(
        run(&mut eval, "(.getName ^Class (class (future 1)))"),
        "\"clojure.core$future_call$reify\""
    );
    assert_eq!(
        run(&mut eval, "(.getName ^Class (class (promise)))"),
        "\"clojure.core$promise$reify\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.getName ^Class (class (iteration (fn [x] (when (< x 3) [:a (inc x)])) :initk 0)))"
        ),
        "\"clojure.core$iteration\""
    );
    // c-name-native-fixed: the port's natives are counter-free (the
    // JVM's `:static` natives carry a build counter).
    assert_eq!(
        run(&mut eval, "(.getName ^Class (class first))"),
        "\"clojure.core$first\""
    );
    // c-name-seq-bigmap-oo/seq-sortedmap-oo: the two divergent seq
    // simplifications (§1.4) — the port names rule here.
    assert_eq!(
        run(
            &mut eval,
            "(.getName ^Class (class (seq (into {} (map (fn [i] [i i]) (range 30))))))"
        ),
        "\"clojure.lang.APersistentMap$KeySeq\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.getName ^Class (class (seq (sorted-map :a 1 :b 2))))"
        ),
        "\"clojure.lang.PersistentArrayMap$Seq\""
    );
}

// ---- class? ----

// c-class?-t/f-str/f-nil/f-iface + c-class?-arity0/2: `(instance?
// Class x)` — interfaces are classes too.
#[test]
fn classq_queries() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Long 'java.lang.Comparable)",
    );
    assert_eq!(run(&mut eval, "(class? String)"), "true");
    assert_eq!(run(&mut eval, "(class? \"a\")"), "false");
    assert_eq!(run(&mut eval, "(class? nil)"), "false");
    assert_eq!(run(&mut eval, "(class? Comparable)"), "true");
    assert_eq!(
        uncaught(&mut eval, "(class?)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/class?"
    );
    assert_eq!(
        uncaught(&mut eval, "(class? String Long)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/class?"
    );
}

// ---- cast ----

// c-cast-ok/nil-x/miss/nil-c/nonclass-c/iface/class/arity1/order.
#[test]
fn cast_queries() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Long 'java.lang.Number 'java.lang.Class)",
    );
    assert_eq!(run(&mut eval, "(cast String \"a\")"), "\"a\"");
    assert_eq!(run(&mut eval, "(cast String nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(cast String 1)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(cast nil \"a\")"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.cast(Object)\" because \"c\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(cast \"x\" \"a\")"),
        cce_bootstrap("java.lang.String", "java.lang.Class")
    );
    assert_eq!(run(&mut eval, "(cast Number 1)"), "1");
    assert_eq!(run(&mut eval, "(cast Class String)"), "java.lang.String");
    assert_eq!(
        uncaught(&mut eval, "(cast String)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/cast"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom [])] [(cast (do (swap! a conj :c) String) (do (swap! a conj :x) \"a\")) @a])"
        ),
        "[\"a\" [:c :x]]"
    );
}

// ---- class value semantics ----

// c-pr-str/print-str/str-class/tostring/print-iface/str-iface/
// tostring-iface/with-out-str/eq-tt/eq-tf/meta/with-meta/seq-err/
// ifn?/fn?: classes print bare, `str`/`toString` debug-prefix,
// equality is by name, and classes take no meta/seq.
#[test]
fn class_value_semantics() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Long 'java.lang.Comparable)",
    );
    assert_eq!(run(&mut eval, "(pr-str String)"), "\"java.lang.String\"");
    assert_eq!(run(&mut eval, "(print-str String)"), "\"java.lang.String\"");
    assert_eq!(run(&mut eval, "(str String)"), "\"class java.lang.String\"");
    assert_eq!(
        run(&mut eval, "(.toString ^Class String)"),
        "\"class java.lang.String\""
    );
    assert_eq!(
        run(&mut eval, "(pr-str Comparable)"),
        "\"java.lang.Comparable\""
    );
    assert_eq!(
        run(&mut eval, "(str Comparable)"),
        "\"interface java.lang.Comparable\""
    );
    assert_eq!(
        run(&mut eval, "(.toString ^Class Comparable)"),
        "\"interface java.lang.Comparable\""
    );
    assert_eq!(
        run(&mut eval, "(with-out-str (print String))"),
        "\"java.lang.String\""
    );
    assert_eq!(run(&mut eval, "(= String String)"), "true");
    assert_eq!(run(&mut eval, "(= String Long)"), "false");
    assert_eq!(run(&mut eval, "(meta String)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(with-meta String {:a 1})"),
        "java.lang.ClassCastException | class java.lang.Class cannot be cast to class clojure.lang.IObj (java.lang.Class is in module java.base of loader 'bootstrap'; clojure.lang.IObj is in unnamed module of loader 'app')"
    );
    assert_eq!(
        uncaught(&mut eval, "(seq String)"),
        "java.lang.IllegalArgumentException | Don't know how to create ISeq from: java.lang.Class"
    );
    assert_eq!(run(&mut eval, "(ifn? String)"), "false");
    assert_eq!(run(&mut eval, "(fn? String)"), "false");
    // The port's `compare`/`sort` cast through `Comparable`
    // (the value-level `Class`/`Class` mismatch lives in
    // `clj-value`'s unit tests).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'java.util.Date) (compare String Date))"
        ),
        "java.lang.ClassCastException | class java.lang.Class cannot be cast to class java.lang.Comparable (java.lang.Class and java.lang.Comparable are in module java.base of loader 'bootstrap')"
    );
    // `print-dup` writes the `#=` name shape.
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (print-dup String pw) (.flush ^java.io.Writer pw) @acc)"
        ),
        "[\"#=java.lang.String\"]"
    );
    // Classes hash by name (deterministic stand-in for identity).
    assert_eq!(
        run(
            &mut eval,
            "(= (.. (class \"a\") (hashCode)) (.. \"java.lang.String\" (hashCode)))"
        ),
        "true"
    );
    // `*print-dup*` printing takes the same `#=` shape.
    assert_eq!(
        run(&mut eval, "(binding [*print-dup* true] (pr-str String))"),
        "\"#=java.lang.String\""
    );
    // `hash` is by name too (`=`-equal classes share buckets;
    // the literal is `hash_int` over the Java string hash —
    // deterministic, like `hash-code`'s spots).
    assert_eq!(
        run(&mut eval, "(= (hash String) (hash (class \"a\")))"),
        "true"
    );
    assert_eq!(run(&mut eval, "(hash String)"), "-1890889484");
}

// ---- bases/supers ----

// c-bases-nil/nonclass/shape/object/iface/arity0/2,
// c-supers-nil/nonclass/shape/object/iface/arity0: nil and empty
// ancestry answer nil, else a seq (bases) or set (supers) in
// lattice order; non-classes are the module-shape CCE.
#[test]
fn bases_supers_queries() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Object 'java.lang.Comparable)",
    );
    assert_eq!(run(&mut eval, "(bases nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(bases \"a\")"),
        cce_bootstrap("java.lang.String", "java.lang.Class")
    );
    assert_eq!(run(&mut eval, "(supers nil)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(supers 1)"),
        cce_bootstrap("java.lang.Long", "java.lang.Class")
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [b (bases String)] [(seq? b) (count b) (map #(.getName ^Class %) b)])"
        ),
        "[true 6 (\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.CharSequence\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\")]"
    );
    assert_eq!(
        run(&mut eval, "(let [s (supers String)] [(set? s) (count s)])"),
        "[true 6]"
    );
    assert_eq!(run(&mut eval, "(bases Object)"), "nil");
    assert_eq!(run(&mut eval, "(supers Object)"), "nil");
    assert_eq!(run(&mut eval, "(bases Comparable)"), "nil");
    assert_eq!(run(&mut eval, "(supers Comparable)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(bases)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/bases"
    );
    assert_eq!(
        uncaught(&mut eval, "(bases String Object)"),
        "clojure.lang.ArityException | Wrong number of args (2) passed to: clojure.core/bases"
    );
    assert_eq!(
        uncaught(&mut eval, "(supers)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/supers"
    );
}

// Descriptors fold at the `bases`/`supers` boundary like in class
// position (§1.3); struct descriptors do not.
#[test]
fn bases_supers_descriptors() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(do (deftype QB [x]) (map str (bases QB)))"),
        "(\"class java.lang.Object\" \"interface clojure.lang.IType\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype QS [x]) (sort (map str (supers QS))))"
        ),
        "(\"class java.lang.Object\" \"interface clojure.lang.IType\")"
    );
    assert_eq!(
        uncaught(&mut eval, "(do (defstruct qbs :a) (bases qbs))"),
        cce_cross("clojure.lang.PersistentStructMap$Def", "java.lang.Class")
    );
}

// c-rec-bases/supers/claimed, c-det-bases/claimed: record/deftype
// implicit interfaces and claimed-first order. The oracle claims
// the JDK `ISeq` in `c-det-claimed`; the port's interface position
// only resolves port interfaces (§1.8 — a JDK claim is the
// `Not an interface` miss pinned below), so the claimed-first
// order transcribes with a port interface.
#[test]
fn hostdef_bases() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord QXR [x]) (vec (map #(.getName ^Class %) (bases QXR))))"
        ),
        "[\"java.lang.Object\" \"clojure.lang.IRecord\" \"clojure.lang.IHashEq\" \"clojure.lang.IObj\" \"clojure.lang.ILookup\" \"clojure.lang.IKeywordLookup\" \"clojure.lang.IPersistentMap\" \"java.util.Map\" \"java.io.Serializable\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord QXS [x]) (vec (sort (map #(.getName ^Class %) (supers QXS)))))"
        ),
        "[\"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IKeywordLookup\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.IRecord\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Map\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord QXW [x] java.io.Closeable (close [t] nil)) (vec (map #(.getName ^Class %) (bases QXW))))"
        ),
        "[\"java.lang.Object\" \"java.io.Closeable\" \"clojure.lang.IRecord\" \"clojure.lang.IHashEq\" \"clojure.lang.IObj\" \"clojure.lang.ILookup\" \"clojure.lang.IKeywordLookup\" \"clojure.lang.IPersistentMap\" \"java.util.Map\" \"java.io.Serializable\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype QXD [x]) (vec (map #(.getName ^Class %) (bases QXD))))"
        ),
        "[\"java.lang.Object\" \"clojure.lang.IType\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface QIfc (m [x])) (deftype QXY [x] QIfc (m [t x] x)) (vec (map #(.getName ^Class %) (bases QXY))))"
        ),
        "[\"java.lang.Object\" \"user.QIfc\" \"clojure.lang.IType\"]"
    );
    // A claimed JDK interface misses (the oracle's `c-det-claimed`
    // source shape, with the import the port needs to name it).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (import 'clojure.lang.ISeq) (deftype QXY2 [x] ISeq (first [t] 1) (more [t] nil) (next [t] nil) (seq [t] t)))"
        ),
        "java.lang.IllegalArgumentException | Not an interface: ISeq"
    );
    // Redefinition leniency: a claimed name redefined as a struct
    // resolves through the struct carve-out (the walk answers for
    // the live definition, like `import` does).
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface RI1564 (m [x])) (deftype RT1564 [x] RI1564 (m [t x] x)) (defstruct RI1564 :a) (contains? (supers (class (RT1564. 1))) (class RI1564)))"
        ),
        "true"
    );
}

// ---- lattice ----

// The 214 static lattice rows, verbatim from `lattice.out`
// (`[interface? ordered-bases-or-nil sorted-supers-or-nil]`
// per binary name — generated, do not hand-edit).
const LATTICE_STATIC: &[(&str, &str)] = &[
    (
        "clojure.core.Eduction",
        "[false [\"java.lang.Object\" \"java.lang.Iterable\" \"clojure.lang.IReduceInit\" \"clojure.lang.Sequential\" \"clojure.lang.IType\"] [\"clojure.lang.IReduceInit\" \"clojure.lang.IType\" \"clojure.lang.Sequential\" \"java.lang.Iterable\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.AFn",
        "[false [\"java.lang.Object\" \"clojure.lang.IFn\"] [\"clojure.lang.IFn\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.AFunction",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.IObj\" \"java.util.Comparator\" \"clojure.lang.Fn\" \"java.io.Serializable\"] [\"clojure.lang.AFn\" \"clojure.lang.Fn\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Comparator\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.AMapEntry",
        "[false [\"clojure.lang.APersistentVector\" \"clojure.lang.IMapEntry\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentVector\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IMapEntry\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IPersistentVector\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.List\" \"java.util.Map$Entry\" \"java.util.RandomAccess\" \"java.util.SequencedCollection\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.APersistentMap",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.IPersistentMap\" \"java.util.Map\" \"java.lang.Iterable\" \"java.io.Serializable\" \"clojure.lang.MapEquivalence\" \"clojure.lang.IHashEq\"] [\"clojure.lang.AFn\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.MapEquivalence\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Map\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.APersistentMap$KeySeq",
        "[false [\"clojure.lang.ASeq\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.APersistentMap$ValSeq",
        "[false [\"clojure.lang.ASeq\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.APersistentSet",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.IPersistentSet\" \"java.util.Collection\" \"java.util.Set\" \"java.io.Serializable\" \"clojure.lang.IHashEq\"] [\"clojure.lang.AFn\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentSet\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.Set\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.APersistentVector",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.IPersistentVector\" \"java.lang.Iterable\" \"java.util.List\" \"java.util.RandomAccess\" \"java.lang.Comparable\" \"java.io.Serializable\" \"clojure.lang.IHashEq\"] [\"clojure.lang.AFn\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IPersistentVector\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.List\" \"java.util.RandomAccess\" \"java.util.SequencedCollection\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.APersistentVector$RSeq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.Counted\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.APersistentVector$Seq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.IReduce\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.APersistentVector$SubVector",
        "[false [\"clojure.lang.APersistentVector\" \"clojure.lang.IObj\" \"clojure.lang.IKVReduce\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentVector\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IKVReduce\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IPersistentVector\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.List\" \"java.util.RandomAccess\" \"java.util.SequencedCollection\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.ARef",
        "[false [\"clojure.lang.AReference\" \"clojure.lang.IRef\"] [\"clojure.lang.AReference\" \"clojure.lang.IDeref\" \"clojure.lang.IMeta\" \"clojure.lang.IRef\" \"clojure.lang.IReference\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.AReference",
        "[false [\"java.lang.Object\" \"clojure.lang.IReference\"] [\"clojure.lang.IMeta\" \"clojure.lang.IReference\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.ASeq",
        "[false [\"clojure.lang.Obj\" \"clojure.lang.ISeq\" \"clojure.lang.Sequential\" \"java.util.List\" \"java.io.Serializable\" \"clojure.lang.IHashEq\"] [\"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.ATransientMap",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.ITransientMap\" \"clojure.lang.ITransientAssociative2\"] [\"clojure.lang.AFn\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientAssociative2\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientMap\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.ATransientSet",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.ITransientSet\"] [\"clojure.lang.AFn\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientSet\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.Agent",
        "[false [\"clojure.lang.ARef\"] [\"clojure.lang.ARef\" \"clojure.lang.AReference\" \"clojure.lang.IDeref\" \"clojure.lang.IMeta\" \"clojure.lang.IRef\" \"clojure.lang.IReference\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.ArityException",
        "[false [\"java.lang.IllegalArgumentException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "clojure.lang.ArrayChunk",
        "[false [\"java.lang.Object\" \"clojure.lang.IChunk\" \"java.io.Serializable\"] [\"clojure.lang.Counted\" \"clojure.lang.IChunk\" \"clojure.lang.Indexed\" \"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.ArraySeq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.IReduce\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Associative",
        "[true [\"clojure.lang.IPersistentCollection\" \"clojure.lang.ILookup\"] [\"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.Seqable\"]]",
    ),
    (
        "clojure.lang.Atom",
        "[false [\"clojure.lang.ARef\" \"clojure.lang.IAtom2\"] [\"clojure.lang.ARef\" \"clojure.lang.AReference\" \"clojure.lang.IAtom\" \"clojure.lang.IAtom2\" \"clojure.lang.IDeref\" \"clojure.lang.IMeta\" \"clojure.lang.IRef\" \"clojure.lang.IReference\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.BigInt",
        "[false [\"java.lang.Number\" \"clojure.lang.IHashEq\"] [\"clojure.lang.IHashEq\" \"java.io.Serializable\" \"java.lang.Number\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.ChunkBuffer",
        "[false [\"java.lang.Object\" \"clojure.lang.Counted\"] [\"clojure.lang.Counted\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.ChunkedCons",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\"] [\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Compiler$CompilerException",
        "[false [\"java.lang.RuntimeException\" \"clojure.lang.IExceptionInfo\"] [\"clojure.lang.IExceptionInfo\" \"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "clojure.lang.Cons",
        "[false [\"clojure.lang.ASeq\" \"java.io.Serializable\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    ("clojure.lang.Counted", "[true nil nil]"),
    (
        "clojure.lang.Cycle",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IReduce\" \"clojure.lang.IPending\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPending\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Delay",
        "[false [\"java.lang.Object\" \"clojure.lang.IDeref\" \"clojure.lang.IPending\"] [\"clojure.lang.IDeref\" \"clojure.lang.IPending\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.ExceptionInfo",
        "[false [\"java.lang.RuntimeException\" \"clojure.lang.IExceptionInfo\"] [\"clojure.lang.IExceptionInfo\" \"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    ("clojure.lang.Fn", "[true nil nil]"),
    ("clojure.lang.IAtom", "[true nil nil]"),
    (
        "clojure.lang.IAtom2",
        "[true [\"clojure.lang.IAtom\"] [\"clojure.lang.IAtom\"]]",
    ),
    ("clojure.lang.IBlockingDeref", "[true nil nil]"),
    (
        "clojure.lang.IChunk",
        "[true [\"clojure.lang.Indexed\"] [\"clojure.lang.Counted\" \"clojure.lang.Indexed\"]]",
    ),
    (
        "clojure.lang.IChunkedSeq",
        "[true [\"clojure.lang.ISeq\" \"clojure.lang.Sequential\"] [\"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\"]]",
    ),
    (
        "clojure.lang.IDeref",
        "[true [\"java.util.function.Supplier\" \"java.util.function.BooleanSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.DoubleSupplier\"] [\"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    ("clojure.lang.IDrop", "[true nil nil]"),
    ("clojure.lang.IEditableCollection", "[true nil nil]"),
    ("clojure.lang.IExceptionInfo", "[true nil nil]"),
    (
        "clojure.lang.IFn",
        "[true [\"java.util.concurrent.Callable\" \"java.lang.Runnable\"] [\"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    ("clojure.lang.IHashEq", "[true nil nil]"),
    ("clojure.lang.IKVReduce", "[true nil nil]"),
    ("clojure.lang.IKeywordLookup", "[true nil nil]"),
    ("clojure.lang.ILookup", "[true nil nil]"),
    (
        "clojure.lang.IMapEntry",
        "[true [\"java.util.Map$Entry\"] [\"java.util.Map$Entry\"]]",
    ),
    ("clojure.lang.IMapIterable", "[true nil nil]"),
    ("clojure.lang.IMeta", "[true nil nil]"),
    (
        "clojure.lang.IObj",
        "[true [\"clojure.lang.IMeta\"] [\"clojure.lang.IMeta\"]]",
    ),
    ("clojure.lang.IPending", "[true nil nil]"),
    (
        "clojure.lang.IPersistentCollection",
        "[true [\"clojure.lang.Seqable\"] [\"clojure.lang.Seqable\"]]",
    ),
    (
        "clojure.lang.IPersistentList",
        "[true [\"clojure.lang.Sequential\" \"clojure.lang.IPersistentStack\"] [\"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\"]]",
    ),
    (
        "clojure.lang.IPersistentMap",
        "[true [\"java.lang.Iterable\" \"clojure.lang.Associative\" \"clojure.lang.Counted\"] [\"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.Seqable\" \"java.lang.Iterable\"]]",
    ),
    (
        "clojure.lang.IPersistentSet",
        "[true [\"clojure.lang.IPersistentCollection\" \"clojure.lang.Counted\" \"clojure.lang.ILookup\"] [\"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.Seqable\"]]",
    ),
    (
        "clojure.lang.IPersistentStack",
        "[true [\"clojure.lang.IPersistentCollection\"] [\"clojure.lang.IPersistentCollection\" \"clojure.lang.Seqable\"]]",
    ),
    (
        "clojure.lang.IPersistentVector",
        "[true [\"clojure.lang.Associative\" \"clojure.lang.Sequential\" \"clojure.lang.IPersistentStack\" \"clojure.lang.Reversible\" \"clojure.lang.Indexed\"] [\"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\"]]",
    ),
    ("clojure.lang.IRecord", "[true nil nil]"),
    (
        "clojure.lang.IReduce",
        "[true [\"clojure.lang.IReduceInit\"] [\"clojure.lang.IReduceInit\"]]",
    ),
    ("clojure.lang.IReduceInit", "[true nil nil]"),
    (
        "clojure.lang.IRef",
        "[true [\"clojure.lang.IDeref\"] [\"clojure.lang.IDeref\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.IReference",
        "[true [\"clojure.lang.IMeta\"] [\"clojure.lang.IMeta\"]]",
    ),
    (
        "clojure.lang.ISeq",
        "[true [\"clojure.lang.IPersistentCollection\"] [\"clojure.lang.IPersistentCollection\" \"clojure.lang.Seqable\"]]",
    ),
    (
        "clojure.lang.ITransientAssociative",
        "[true [\"clojure.lang.ITransientCollection\" \"clojure.lang.ILookup\"] [\"clojure.lang.ILookup\" \"clojure.lang.ITransientCollection\"]]",
    ),
    ("clojure.lang.ITransientAssociative2", "[true nil nil]"),
    ("clojure.lang.ITransientCollection", "[true nil nil]"),
    (
        "clojure.lang.ITransientMap",
        "[true [\"clojure.lang.ITransientAssociative\" \"clojure.lang.Counted\"] [\"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientCollection\"]]",
    ),
    (
        "clojure.lang.ITransientSet",
        "[true [\"clojure.lang.ITransientCollection\" \"clojure.lang.Counted\" \"clojure.lang.ILookup\"] [\"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientCollection\"]]",
    ),
    (
        "clojure.lang.ITransientVector",
        "[true [\"clojure.lang.ITransientAssociative\" \"clojure.lang.Indexed\"] [\"clojure.lang.Counted\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientCollection\" \"clojure.lang.Indexed\"]]",
    ),
    ("clojure.lang.IType", "[true nil nil]"),
    (
        "clojure.lang.Indexed",
        "[true [\"clojure.lang.Counted\"] [\"clojure.lang.Counted\"]]",
    ),
    (
        "clojure.lang.IndexedSeq",
        "[true [\"clojure.lang.ISeq\" \"clojure.lang.Sequential\" \"clojure.lang.Counted\"] [\"clojure.lang.Counted\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\"]]",
    ),
    (
        "clojure.lang.Iterate",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IReduce\" \"clojure.lang.IPending\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPending\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Keyword",
        "[false [\"java.lang.Object\" \"clojure.lang.IFn\" \"java.lang.Comparable\" \"clojure.lang.Named\" \"java.io.Serializable\" \"clojure.lang.IHashEq\"] [\"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.Named\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.LazySeq",
        "[false [\"clojure.lang.Obj\" \"clojure.lang.ISeq\" \"clojure.lang.Sequential\" \"java.util.List\" \"clojure.lang.IPending\" \"clojure.lang.IHashEq\"] [\"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPending\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.LineNumberingPushbackReader",
        "[false [\"java.io.PushbackReader\"] [\"java.io.Closeable\" \"java.io.FilterReader\" \"java.io.PushbackReader\" \"java.io.Reader\" \"java.lang.AutoCloseable\" \"java.lang.Object\" \"java.lang.Readable\"]]",
    ),
    (
        "clojure.lang.LispReader$ReaderException",
        "[false [\"java.lang.RuntimeException\" \"clojure.lang.IExceptionInfo\"] [\"clojure.lang.IExceptionInfo\" \"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "clojure.lang.LongRange",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IDrop\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.MapEntry",
        "[false [\"clojure.lang.AMapEntry\"] [\"clojure.lang.AFn\" \"clojure.lang.AMapEntry\" \"clojure.lang.APersistentVector\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IMapEntry\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IPersistentVector\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.List\" \"java.util.Map$Entry\" \"java.util.RandomAccess\" \"java.util.SequencedCollection\" \"java.util.concurrent.Callable\"]]",
    ),
    ("clojure.lang.MapEquivalence", "[true nil nil]"),
    (
        "clojure.lang.MultiFn",
        "[false [\"clojure.lang.AFn\"] [\"clojure.lang.AFn\" \"clojure.lang.IFn\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    ("clojure.lang.Named", "[true nil nil]"),
    (
        "clojure.lang.Namespace",
        "[false [\"clojure.lang.AReference\" \"java.io.Serializable\"] [\"clojure.lang.AReference\" \"clojure.lang.IMeta\" \"clojure.lang.IReference\" \"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.Obj",
        "[false [\"java.lang.Object\" \"clojure.lang.IObj\" \"java.io.Serializable\"] [\"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.PersistentArrayMap",
        "[false [\"clojure.lang.APersistentMap\" \"clojure.lang.IObj\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IMapIterable\" \"clojure.lang.IKVReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentMap\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IDrop\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IKVReduce\" \"clojure.lang.ILookup\" \"clojure.lang.IMapIterable\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.MapEquivalence\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Map\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentArrayMap$Seq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IDrop\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentArrayMap$TransientArrayMap",
        "[false [\"clojure.lang.ATransientMap\"] [\"clojure.lang.AFn\" \"clojure.lang.ATransientMap\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientAssociative2\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientMap\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentHashMap",
        "[false [\"clojure.lang.APersistentMap\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IObj\" \"clojure.lang.IMapIterable\" \"clojure.lang.IKVReduce\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentMap\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IKVReduce\" \"clojure.lang.ILookup\" \"clojure.lang.IMapIterable\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.MapEquivalence\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Map\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentHashMap$ArrayNode$Seq",
        "[false [\"clojure.lang.ASeq\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentHashMap$TransientHashMap",
        "[false [\"clojure.lang.ATransientMap\"] [\"clojure.lang.AFn\" \"clojure.lang.ATransientMap\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientAssociative2\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientMap\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentHashSet",
        "[false [\"clojure.lang.APersistentSet\" \"clojure.lang.IObj\" \"clojure.lang.IEditableCollection\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentSet\" \"clojure.lang.Counted\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentSet\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.Set\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentHashSet$TransientHashSet",
        "[false [\"clojure.lang.ATransientSet\"] [\"clojure.lang.AFn\" \"clojure.lang.ATransientSet\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientSet\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentList",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IPersistentList\" \"clojure.lang.IReduce\" \"java.util.List\" \"clojure.lang.Counted\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentList\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentList$EmptyList",
        "[false [\"clojure.lang.Obj\" \"clojure.lang.IPersistentList\" \"java.util.List\" \"clojure.lang.ISeq\" \"clojure.lang.Counted\" \"clojure.lang.IHashEq\"] [\"clojure.lang.Counted\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentList\" \"clojure.lang.IPersistentStack\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentStructMap",
        "[false [\"clojure.lang.APersistentMap\" \"clojure.lang.IObj\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentMap\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.MapEquivalence\" \"clojure.lang.Seqable\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Map\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentStructMap$1",
        "[false [\"clojure.lang.AFn\"] [\"clojure.lang.AFn\" \"clojure.lang.IFn\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentStructMap$Def",
        "[false [\"java.lang.Object\" \"java.io.Serializable\"] [\"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.PersistentTreeMap",
        "[false [\"clojure.lang.APersistentMap\" \"clojure.lang.IObj\" \"clojure.lang.Reversible\" \"clojure.lang.Sorted\" \"clojure.lang.IKVReduce\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentMap\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IKVReduce\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentMap\" \"clojure.lang.MapEquivalence\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sorted\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Map\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentTreeMap$Seq",
        "[false [\"clojure.lang.ASeq\"] [\"clojure.lang.ASeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentTreeSet",
        "[false [\"clojure.lang.APersistentSet\" \"clojure.lang.IObj\" \"clojure.lang.Reversible\" \"clojure.lang.Sorted\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentSet\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentSet\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sorted\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.Set\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentVector",
        "[false [\"clojure.lang.APersistentVector\" \"clojure.lang.IObj\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IKVReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.AFn\" \"clojure.lang.APersistentVector\" \"clojure.lang.Associative\" \"clojure.lang.Counted\" \"clojure.lang.IDrop\" \"clojure.lang.IEditableCollection\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IKVReduce\" \"clojure.lang.ILookup\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IPersistentStack\" \"clojure.lang.IPersistentVector\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.Indexed\" \"clojure.lang.Reversible\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Collection\" \"java.util.List\" \"java.util.RandomAccess\" \"java.util.SequencedCollection\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.PersistentVector$ChunkedSeq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.Counted\" \"clojure.lang.IReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IDrop\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.PersistentVector$TransientVector",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.ITransientVector\" \"clojure.lang.ITransientAssociative2\" \"clojure.lang.Counted\"] [\"clojure.lang.AFn\" \"clojure.lang.Counted\" \"clojure.lang.IFn\" \"clojure.lang.ILookup\" \"clojure.lang.ITransientAssociative\" \"clojure.lang.ITransientAssociative2\" \"clojure.lang.ITransientCollection\" \"clojure.lang.ITransientVector\" \"clojure.lang.Indexed\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.Range",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IReduce\"] [\"clojure.lang.ASeq\" \"clojure.lang.IChunkedSeq\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Ratio",
        "[false [\"java.lang.Number\" \"java.lang.Comparable\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Number\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.ReaderConditional",
        "[false [\"java.lang.Object\" \"clojure.lang.ILookup\"] [\"clojure.lang.ILookup\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.Reduced",
        "[false [\"java.lang.Object\" \"clojure.lang.IDeref\"] [\"clojure.lang.IDeref\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.Repeat",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IReduce\" \"clojure.lang.IDrop\"] [\"clojure.lang.ASeq\" \"clojure.lang.IDrop\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduce\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.RestFn",
        "[false [\"clojure.lang.AFunction\"] [\"clojure.lang.AFn\" \"clojure.lang.AFunction\" \"clojure.lang.Fn\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Comparator\" \"java.util.concurrent.Callable\"]]",
    ),
    ("clojure.lang.Reversible", "[true nil nil]"),
    ("clojure.lang.Seqable", "[true nil nil]"),
    ("clojure.lang.Sequential", "[true nil nil]"),
    ("clojure.lang.Settable", "[true nil nil]"),
    ("clojure.lang.Sorted", "[true nil nil]"),
    (
        "clojure.lang.StringSeq",
        "[false [\"clojure.lang.ASeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.IDrop\" \"clojure.lang.IReduceInit\"] [\"clojure.lang.ASeq\" \"clojure.lang.Counted\" \"clojure.lang.IDrop\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPersistentCollection\" \"clojure.lang.IReduceInit\" \"clojure.lang.ISeq\" \"clojure.lang.IndexedSeq\" \"clojure.lang.Obj\" \"clojure.lang.Seqable\" \"clojure.lang.Sequential\" \"java.io.Serializable\" \"java.lang.Iterable\" \"java.lang.Object\" \"java.util.Collection\" \"java.util.List\" \"java.util.SequencedCollection\"]]",
    ),
    (
        "clojure.lang.Symbol",
        "[false [\"clojure.lang.AFn\" \"clojure.lang.IObj\" \"java.lang.Comparable\" \"clojure.lang.Named\" \"java.io.Serializable\" \"clojure.lang.IHashEq\"] [\"clojure.lang.AFn\" \"clojure.lang.IFn\" \"clojure.lang.IHashEq\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.Named\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.TaggedLiteral",
        "[false [\"java.lang.Object\" \"clojure.lang.ILookup\"] [\"clojure.lang.ILookup\" \"java.lang.Object\"]]",
    ),
    (
        "clojure.lang.Var",
        "[false [\"clojure.lang.ARef\" \"clojure.lang.IFn\" \"clojure.lang.IRef\" \"clojure.lang.Settable\" \"java.io.Serializable\"] [\"clojure.lang.ARef\" \"clojure.lang.AReference\" \"clojure.lang.IDeref\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IRef\" \"clojure.lang.IReference\" \"clojure.lang.Settable\" \"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "clojure.lang.Var$Unbound",
        "[false [\"clojure.lang.AFn\"] [\"clojure.lang.AFn\" \"clojure.lang.IFn\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\"]]",
    ),
    (
        "clojure.lang.Volatile",
        "[false [\"java.lang.Object\" \"clojure.lang.IDeref\"] [\"clojure.lang.IDeref\" \"java.lang.Object\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]",
    ),
    (
        "java.io.Closeable",
        "[true [\"java.lang.AutoCloseable\"] [\"java.lang.AutoCloseable\"]]",
    ),
    (
        "java.io.FileNotFoundException",
        "[false [\"java.io.IOException\"] [\"java.io.IOException\" \"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.io.FilterReader",
        "[false [\"java.io.Reader\"] [\"java.io.Closeable\" \"java.io.Reader\" \"java.lang.AutoCloseable\" \"java.lang.Object\" \"java.lang.Readable\"]]",
    ),
    ("java.io.Flushable", "[true nil nil]"),
    (
        "java.io.IOException",
        "[false [\"java.lang.Exception\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.io.OutputStreamWriter",
        "[false [\"java.io.Writer\"] [\"java.io.Closeable\" \"java.io.Flushable\" \"java.io.Writer\" \"java.lang.Appendable\" \"java.lang.AutoCloseable\" \"java.lang.Object\"]]",
    ),
    (
        "java.io.PrintWriter",
        "[false [\"java.io.Writer\"] [\"java.io.Closeable\" \"java.io.Flushable\" \"java.io.Writer\" \"java.lang.Appendable\" \"java.lang.AutoCloseable\" \"java.lang.Object\"]]",
    ),
    (
        "java.io.PushbackReader",
        "[false [\"java.io.FilterReader\"] [\"java.io.Closeable\" \"java.io.FilterReader\" \"java.io.Reader\" \"java.lang.AutoCloseable\" \"java.lang.Object\" \"java.lang.Readable\"]]",
    ),
    (
        "java.io.Reader",
        "[false [\"java.lang.Object\" \"java.lang.Readable\" \"java.io.Closeable\"] [\"java.io.Closeable\" \"java.lang.AutoCloseable\" \"java.lang.Object\" \"java.lang.Readable\"]]",
    ),
    ("java.io.Serializable", "[true nil nil]"),
    (
        "java.io.StringWriter",
        "[false [\"java.io.Writer\"] [\"java.io.Closeable\" \"java.io.Flushable\" \"java.io.Writer\" \"java.lang.Appendable\" \"java.lang.AutoCloseable\" \"java.lang.Object\"]]",
    ),
    (
        "java.io.Writer",
        "[false [\"java.lang.Object\" \"java.lang.Appendable\" \"java.io.Closeable\" \"java.io.Flushable\"] [\"java.io.Closeable\" \"java.io.Flushable\" \"java.lang.Appendable\" \"java.lang.AutoCloseable\" \"java.lang.Object\"]]",
    ),
    ("java.lang.Appendable", "[true nil nil]"),
    (
        "java.lang.ArithmeticException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.ArrayIndexOutOfBoundsException",
        "[false [\"java.lang.IndexOutOfBoundsException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IndexOutOfBoundsException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.AssertionError",
        "[false [\"java.lang.Error\"] [\"java.io.Serializable\" \"java.lang.Error\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.AutoCloseable", "[true nil nil]"),
    (
        "java.lang.Boolean",
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.constant.Constable\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\"]]",
    ),
    ("java.lang.CharSequence", "[true nil nil]"),
    (
        "java.lang.Character",
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.constant.Constable\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\"]]",
    ),
    (
        "java.lang.Class",
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.reflect.GenericDeclaration\" \"java.lang.reflect.Type\" \"java.lang.reflect.AnnotatedElement\" \"java.lang.invoke.TypeDescriptor$OfField\" \"java.lang.constant.Constable\"] [\"java.io.Serializable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.invoke.TypeDescriptor\" \"java.lang.invoke.TypeDescriptor$OfField\" \"java.lang.reflect.AnnotatedElement\" \"java.lang.reflect.GenericDeclaration\" \"java.lang.reflect.Type\"]]",
    ),
    (
        "java.lang.ClassCastException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.ClassNotFoundException",
        "[false [\"java.lang.ReflectiveOperationException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.ReflectiveOperationException\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.Cloneable", "[true nil nil]"),
    ("java.lang.Comparable", "[true nil nil]"),
    (
        "java.lang.Double",
        "[false [\"java.lang.Number\" \"java.lang.Comparable\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Number\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"]]",
    ),
    (
        "java.lang.Error",
        "[false [\"java.lang.Throwable\"] [\"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.Exception",
        "[false [\"java.lang.Throwable\"] [\"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.IllegalAccessError",
        "[false [\"java.lang.IncompatibleClassChangeError\"] [\"java.io.Serializable\" \"java.lang.Error\" \"java.lang.IncompatibleClassChangeError\" \"java.lang.LinkageError\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.IllegalArgumentException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.IllegalStateException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.IncompatibleClassChangeError",
        "[false [\"java.lang.LinkageError\"] [\"java.io.Serializable\" \"java.lang.Error\" \"java.lang.LinkageError\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.IndexOutOfBoundsException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.Iterable", "[true nil nil]"),
    (
        "java.lang.LinkageError",
        "[false [\"java.lang.Error\"] [\"java.io.Serializable\" \"java.lang.Error\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.Long",
        "[false [\"java.lang.Number\" \"java.lang.Comparable\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Number\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"]]",
    ),
    (
        "java.lang.NegativeArraySizeException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.NullPointerException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.Number",
        "[false [\"java.lang.Object\" \"java.io.Serializable\"] [\"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "java.lang.NumberFormatException",
        "[false [\"java.lang.IllegalArgumentException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.Object", "[false nil nil]"),
    ("java.lang.Readable", "[true nil nil]"),
    (
        "java.lang.ReflectiveOperationException",
        "[false [\"java.lang.Exception\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.Runnable", "[true nil nil]"),
    (
        "java.lang.RuntimeException",
        "[false [\"java.lang.Exception\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.String",
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.CharSequence\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"] [\"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"]]",
    ),
    (
        "java.lang.StringIndexOutOfBoundsException",
        "[false [\"java.lang.IndexOutOfBoundsException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IndexOutOfBoundsException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.lang.Throwable",
        "[false [\"java.lang.Object\" \"java.io.Serializable\"] [\"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "java.lang.UnsupportedOperationException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    ("java.lang.constant.Constable", "[true nil nil]"),
    ("java.lang.constant.ConstantDesc", "[true nil nil]"),
    ("java.lang.invoke.TypeDescriptor", "[true nil nil]"),
    (
        "java.lang.invoke.TypeDescriptor$OfField",
        "[true [\"java.lang.invoke.TypeDescriptor\"] [\"java.lang.invoke.TypeDescriptor\"]]",
    ),
    ("java.lang.reflect.AnnotatedElement", "[true nil nil]"),
    (
        "java.lang.reflect.GenericDeclaration",
        "[true [\"java.lang.reflect.AnnotatedElement\"] [\"java.lang.reflect.AnnotatedElement\"]]",
    ),
    ("java.lang.reflect.Type", "[true nil nil]"),
    (
        "java.math.BigDecimal",
        "[false [\"java.lang.Number\" \"java.lang.Comparable\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Number\" \"java.lang.Object\"]]",
    ),
    (
        "java.math.BigInteger",
        "[false [\"java.lang.Number\" \"java.lang.Comparable\"] [\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.Number\" \"java.lang.Object\"]]",
    ),
    (
        "java.util.Collection",
        "[true [\"java.lang.Iterable\"] [\"java.lang.Iterable\"]]",
    ),
    ("java.util.Comparator", "[true nil nil]"),
    (
        "java.util.Date",
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Cloneable\" \"java.lang.Comparable\"] [\"java.io.Serializable\" \"java.lang.Cloneable\" \"java.lang.Comparable\" \"java.lang.Object\"]]",
    ),
    (
        "java.util.DuplicateFormatFlagsException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.FormatFlagsConversionMismatchException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatArgumentIndexException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatCodePointException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatConversionException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatException",
        "[false [\"java.lang.IllegalArgumentException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.util.IllegalFormatFlagsException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatPrecisionException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.IllegalFormatWidthException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.List",
        "[true [\"java.util.SequencedCollection\"] [\"java.lang.Iterable\" \"java.util.Collection\" \"java.util.SequencedCollection\"]]",
    ),
    ("java.util.Map", "[true nil nil]"),
    ("java.util.Map$Entry", "[true nil nil]"),
    (
        "java.util.MissingFormatArgumentException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    (
        "java.util.MissingFormatWidthException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    ("java.util.RandomAccess", "[true nil nil]"),
    (
        "java.util.SequencedCollection",
        "[true [\"java.util.Collection\"] [\"java.lang.Iterable\" \"java.util.Collection\"]]",
    ),
    (
        "java.util.Set",
        "[true [\"java.util.Collection\"] [\"java.lang.Iterable\" \"java.util.Collection\"]]",
    ),
    (
        "java.util.UnknownFormatConversionException",
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]",
    ),
    ("java.util.concurrent.Callable", "[true nil nil]"),
    (
        "java.util.concurrent.CancellationException",
        "[false [\"java.lang.IllegalStateException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalStateException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    (
        "java.util.concurrent.ExecutionException",
        "[false [\"java.lang.Exception\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.Throwable\"]]",
    ),
    ("java.util.concurrent.Future", "[true nil nil]"),
    (
        "java.util.concurrent.RejectedExecutionException",
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
    ("java.util.function.BooleanSupplier", "[true nil nil]"),
    ("java.util.function.DoubleSupplier", "[true nil nil]"),
    ("java.util.function.IntSupplier", "[true nil nil]"),
    ("java.util.function.LongSupplier", "[true nil nil]"),
    ("java.util.function.Supplier", "[true nil nil]"),
    ("java.util.regex.MatchResult", "[true nil nil]"),
    (
        "java.util.regex.Matcher",
        "[false [\"java.lang.Object\" \"java.util.regex.MatchResult\"] [\"java.lang.Object\" \"java.util.regex.MatchResult\"]]",
    ),
    (
        "java.util.regex.Pattern",
        "[false [\"java.lang.Object\" \"java.io.Serializable\"] [\"java.io.Serializable\" \"java.lang.Object\"]]",
    ),
    (
        "java.util.regex.PatternSyntaxException",
        "[false [\"java.lang.IllegalArgumentException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]",
    ),
];

// All 214 static rows verbatim from `lattice.out`: each binary name
// resolves through `__import*` (no simple-name collisions across
// the set, so one namespace suffices) and answers
// `[isInterface ordered-bases sorted-supers]`, exactly like the
// probe's `lat`.
#[test]
fn lattice_static_rows() {
    let mut eval = boot();
    assert_eq!(LATTICE_STATIC.len(), 214);
    for (fqn, want) in LATTICE_STATIC {
        let src = format!(
            "(let [c (clojure.core/__import* \"{fqn}\")] [(.isInterface c) (when-let [b (seq (bases c))] (vec (map #(.getName ^Class %) b))) (when-let [s (seq (supers c))] (vec (sort (map #(.getName ^Class %) s))))])"
        );
        assert_eq!(run(&mut eval, &src), *want, "{fqn}");
    }
}

// The four tabled-but-unprobed static rows (§1.3): the two
// import-only rows (`Calendar`, `sql.Date` — ancestries verified
// live on the JVM) plus the S16 `NoSuchElementException` variant
// (live-JVM ancestry) and the synthesized nonexistent
// `MissingFormatConversionException` (throw-chain shape, matching
// `throw_parent`). Each answers the same `[isInterface
// ordered-bases sorted-supers]` shape.
#[test]
fn lattice_extra_rows() {
    let mut eval = boot();
    let lat = "(fn [c] [(.isInterface c) (when-let [b (seq (bases c))] (vec (map #(.getName ^Class %) b))) (when-let [s (seq (supers c))] (vec (sort (map #(.getName ^Class %) s))))])";
    run(&mut eval, &format!("(def lat {lat})"));
    assert_eq!(
        run(
            &mut eval,
            "(lat (clojure.core/__import* \"java.util.Calendar\"))"
        ),
        "[false [\"java.lang.Object\" \"java.io.Serializable\" \"java.lang.Cloneable\" \"java.lang.Comparable\"] [\"java.io.Serializable\" \"java.lang.Cloneable\" \"java.lang.Comparable\" \"java.lang.Object\"]]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(lat (clojure.core/__import* \"java.sql.Date\"))"
        ),
        "[false [\"java.util.Date\"] [\"java.io.Serializable\" \"java.lang.Cloneable\" \"java.lang.Comparable\" \"java.lang.Object\" \"java.util.Date\"]]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(lat (clojure.core/__import* \"java.util.NoSuchElementException\"))"
        ),
        "[false [\"java.lang.RuntimeException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\"]]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(lat (clojure.core/__import* \"java.util.MissingFormatConversionException\"))"
        ),
        "[false [\"java.util.IllegalFormatException\"] [\"java.io.Serializable\" \"java.lang.Exception\" \"java.lang.IllegalArgumentException\" \"java.lang.Object\" \"java.lang.RuntimeException\" \"java.lang.Throwable\" \"java.util.IllegalFormatException\"]]"
    );
}

// l-fn-fixed/fn-multi-fixed/fn-variadic/fn-name,
// l-native-fixed/native-rest, l-future/future-name,
// l-promise/promise-name, l-reify-shape,
// l-iteration/iteration-name: the dynamic rules (fn rule, stable
// reify names, port-interface reify shape).
#[test]
fn lattice_dynamic_rules() {
    let mut eval = boot();
    let fixed = "[false [\"clojure.lang.AFunction\"] [\"clojure.lang.AFn\" \"clojure.lang.AFunction\" \"clojure.lang.Fn\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Comparator\" \"java.util.concurrent.Callable\"]]";
    let lat = "(fn [c] [(.isInterface c) (when-let [b (seq (bases c))] (vec (map #(.getName ^Class %) b))) (when-let [s (seq (supers c))] (vec (sort (map #(.getName ^Class %) s))))])";
    run(&mut eval, &format!("(def lat {lat})"));
    // l-fn-fixed/multi-fixed: fixed arities (even many) → AFunction.
    assert_eq!(run(&mut eval, "(lat (class (fn [x] x)))"), fixed);
    assert_eq!(
        run(&mut eval, "(lat (class (fn ([x] x) ([x y] y))))"),
        fixed
    );
    // l-fn-variadic: any rest arity → RestFn.
    assert_eq!(
        run(&mut eval, "(lat (class (fn [& xs] xs)))"),
        "[false [\"clojure.lang.RestFn\"] [\"clojure.lang.AFn\" \"clojure.lang.AFunction\" \"clojure.lang.Fn\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.RestFn\" \"java.io.Serializable\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.Comparator\" \"java.util.concurrent.Callable\"]]"
    );
    // l-fn-name: the closure display, counters scrubbed.
    assert_eq!(
        normctr(&run(&mut eval, "(str (class (fn [x] x)))")),
        "\"class user$evalN$fn__N\""
    );
    // l-native-fixed: natives answer AFunction (here agreeing with
    // the JVM).
    assert_eq!(run(&mut eval, "(lat (class first))"), fixed);
    // l-native-rest: the JVM reports RestFn for `map`; the port
    // answers AFunction for all natives (§1.3 fence divergence).
    assert_eq!(run(&mut eval, "(lat (class map))"), fixed);
    // l-future/name, l-promise/name: the stable counter-free reify
    // rows (ancestry byte-equal to the oracle).
    assert_eq!(
        run(&mut eval, "(lat (class (future 1)))"),
        "[false [\"java.lang.Object\" \"clojure.lang.IPending\" \"clojure.lang.IBlockingDeref\" \"java.util.concurrent.Future\" \"clojure.lang.IDeref\" \"clojure.lang.IObj\"] [\"clojure.lang.IBlockingDeref\" \"clojure.lang.IDeref\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPending\" \"java.lang.Object\" \"java.util.concurrent.Future\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]"
    );
    assert_eq!(
        run(&mut eval, "(str (class (future 1)))"),
        "\"class clojure.core$future_call$reify\""
    );
    assert_eq!(
        run(&mut eval, "(lat (class (promise)))"),
        "[false [\"java.lang.Object\" \"clojure.lang.IPending\" \"clojure.lang.IBlockingDeref\" \"clojure.lang.IDeref\" \"clojure.lang.IFn\" \"clojure.lang.IObj\"] [\"clojure.lang.IBlockingDeref\" \"clojure.lang.IDeref\" \"clojure.lang.IFn\" \"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IPending\" \"java.lang.Object\" \"java.lang.Runnable\" \"java.util.concurrent.Callable\" \"java.util.function.BooleanSupplier\" \"java.util.function.DoubleSupplier\" \"java.util.function.IntSupplier\" \"java.util.function.LongSupplier\" \"java.util.function.Supplier\"]]"
    );
    assert_eq!(
        run(&mut eval, "(str (class (promise)))"),
        "\"class clojure.core$promise$reify\""
    );
    // l-reify-shape: the oracle claims the JDK `IDeref`; the port's
    // interface position only resolves port interfaces (§1.8), so
    // the `[Object, ifaces...]` shape transcribes with one (reify
    // defs are `Type`-kind, hence the `IType` implicit).
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface RLIfc (deref [])) (lat (class (reify RLIfc (deref [t] 1)))))"
        ),
        "[false [\"java.lang.Object\" \"user.RLIfc\" \"clojure.lang.IType\"] [\"clojure.lang.IType\" \"java.lang.Object\" \"user.RLIfc\"]]"
    );
    // l-iteration/name: the synthetic row copies the JVM reify
    // ancestry under the stable name.
    assert_eq!(
        run(
            &mut eval,
            "(lat (class (iteration (fn [x] (when (< x 3) [:a (inc x)])) :initk 0)))"
        ),
        "[false [\"java.lang.Object\" \"clojure.lang.IReduceInit\" \"clojure.lang.Seqable\" \"clojure.lang.IObj\"] [\"clojure.lang.IMeta\" \"clojure.lang.IObj\" \"clojure.lang.IReduceInit\" \"clojure.lang.Seqable\" \"java.lang.Object\"]]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(str (class (iteration (fn [x] (when (< x 3) [:a (inc x)])) :initk 0)))"
        ),
        "\"class clojure.core$iteration\""
    );
}

// Dynamic names are values, never registry entries: importing a
// fn/reify display name is `CNFE` (§1.3).
#[test]
fn lattice_dynamic_names_unimportable() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(clojure.core/__import* \"user$reify\")"),
        "java.lang.ClassNotFoundException | user$reify"
    );
    let display = run(&mut eval, "(.getName ^Class (class (fn [x] x)))");
    let display = display.trim_matches('"');
    assert_eq!(
        uncaught(
            &mut eval,
            &format!("(clojure.core/__import* \"{display}\")")
        ),
        format!("java.lang.ClassNotFoundException | {display}")
    );
}

// ---- identical? ----

// d-nil–d-lazy-same (28 rows): the per-kind identity table. Value
// kinds compare by value (the port has no boxes/caches/interning),
// identity kinds by allocation/cell/id (§1.9).
#[test]
fn identical_kinds() {
    let mut eval = boot();
    run(&mut eval, "(import 'java.lang.String)");
    // Immediates agree with the JVM.
    for src in [
        "(identical? nil nil)",
        "(identical? true true)",
        "(identical? 1 1)",
        "(identical? -5 -5)",
        "(identical? \\a \\a)",
        "(identical? \"a\" \"a\")",
        "(identical? :a :a)",
    ] {
        assert_eq!(run(&mut eval, src), "true", "{src}");
    }
    // Cache/interning-dependent rows DIVERGE (forced, §1.9): the JVM
    // answers false (distinct boxes), the port compares values.
    for src in [
        "(identical? 10000000000 10000000000)",
        "(identical? 1.5 1.5)",
        "(identical? \\€ \\€)",
        "(identical? (bigint 5) (bigint 5))",
        "(identical? 1.5M 1.5M)",
        "(identical? 1/3 1/3)",
        "(identical? (str \\a) (str \\a))",
        "(identical? 'a 'a)",
    ] {
        assert_eq!(run(&mut eval, src), "true", "{src}");
    }
    // Collections: same binding true, distinct-but-equal false.
    assert_eq!(run(&mut eval, "(let [v [1]] (identical? v v))"), "true");
    for src in [
        "(identical? [1] [1])",
        "(identical? {:a 1} {:a 1})",
        "(identical? '(1) '(1))",
        "(identical? #{1} #{1})",
    ] {
        assert_eq!(run(&mut eval, src), "false", "{src}");
    }
    // Functions, atoms, vars, classes, regexes, lazies.
    assert_eq!(
        run(&mut eval, "(let [f (fn [x] x)] (identical? f f))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(identical? (fn [x] x) (fn [x] x))"),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(let [a (atom 1)] (identical? a a))"),
        "true"
    );
    assert_eq!(run(&mut eval, "(identical? (atom 1) (atom 1))"), "false");
    assert_eq!(run(&mut eval, "(identical? #'map #'map)"), "true");
    assert_eq!(run(&mut eval, "(identical? String String)"), "true");
    assert_eq!(run(&mut eval, "(identical? #\"x\" #\"x\")"), "false");
    assert_eq!(
        run(&mut eval, "(let [s (map inc [1])] (identical? s s))"),
        "true"
    );
    // Host values and multifns compare allocations (§1.9): same
    // binding true, distinct-but-equal false.
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype IdPt [x]) (let [i (IdPt. 1)] (identical? i i)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype IdPt [x]) (identical? (IdPt. 1) (IdPt. 1)))"
        ),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(do (defmulti idm :k) (identical? idm idm))"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defmulti idm1 :k) (defmulti idm2 :k) (identical? idm1 idm2))"
        ),
        "false"
    );
}

// d-macroexpand/inline-arities/has-inline: no macroexpand; the port
// carries no `:inline` metadata (the rewrite lives in the analyzer,
// not on the var).
#[test]
fn identical_inline() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(macroexpand '(identical? a b))"),
        "(identical? a b)"
    );
    assert_eq!(
        run(&mut eval, "(:inline-arities (meta #'identical?))"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(some? (:inline (meta #'identical?)))"),
        "false"
    );
}

// d-arity0/1/3: unqualified wrong arities fail at runtime
// (byte-equal — neither side has a `--N` suffix); direct qualified
// calls reject at compile time (§2.4 phase divergence).
#[test]
fn identical_arities() {
    let mut eval = boot();
    for (src, n) in [
        ("(identical?)", 0),
        ("(identical? 1)", 1),
        ("(identical? 1 2 3)", 3),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!(
                "clojure.lang.ArityException | Wrong number of args ({n}) passed to: clojure.core/identical?"
            ),
            "{src}"
        );
    }
    for (src, n) in [
        ("(clojure.core/identical?)", 0),
        ("(clojure.core/identical? 1)", 1),
        ("(clojure.core/identical? 1 2 3)", 3),
    ] {
        assert_eq!(
            compile_err(&mut eval, src),
            format!(
                "clojure.lang.ArityException | Wrong number of args ({n}) passed to: clojure.core/identical?"
            ),
            "{src}"
        );
    }
}

// d-apply-t/f/arity, d-ho-str/vec, d-eval-order, d-eq-vs-ident-*:
// first-class use through the var.
#[test]
fn identical_first_class() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(apply identical? [1 1])"), "true");
    assert_eq!(run(&mut eval, "(apply identical? [[1] [1]])"), "false");
    assert_eq!(
        uncaught(&mut eval, "(apply identical? [1])"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/identical?"
    );
    assert_eq!(run(&mut eval, "(map identical? [\"a\"] [\"a\"])"), "(true)");
    assert_eq!(run(&mut eval, "(map identical? [[1]] [[1]])"), "(false)");
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom [])] [(identical? (do (swap! a conj :x) 1) (do (swap! a conj :y) 1)) @a])"
        ),
        "[true [:x :y]]"
    );
    assert_eq!(
        run(&mut eval, "[(= [1] [1]) (identical? [1] [1])]"),
        "[true false]"
    );
    // d-eq-vs-ident-big/str: `=` agrees; `identical?` diverges per
    // the value rule (above).
    assert_eq!(
        run(
            &mut eval,
            "[(= 10000000000 10000000000) (identical? 10000000000 10000000000)]"
        ),
        "[true true]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(= (str \\a) (str \\a)) (identical? (str \\a) (str \\a))]"
        ),
        "[true true]"
    );
}

// ---- PrintWriter-on ----

// Scrub hex identities like the oracle's `normhex`.
fn normhex(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 'x' && out.ends_with("0") && chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push('x');
            out.push('H');
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else if c == '@' && chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
            out.push('@');
            out.push('L');
            while chars.peek().is_some_and(|d| d.is_ascii_hexdigit()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

// w-class/arity3-class/print/arity1/arity4: the shape — a
// `PrintWriter` with the `#object` print, 2–3 args.
#[test]
fn printwriter_shape() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(class (PrintWriter-on prn nil))"),
        "java.io.PrintWriter"
    );
    assert_eq!(
        normhex(&run(&mut eval, "(pr-str (PrintWriter-on prn nil))")),
        "\"#object[java.io.PrintWriter 0xH \\\"java.io.PrintWriter@L\\\"]\""
    );
    assert_eq!(
        run(&mut eval, "(class (PrintWriter-on prn nil true))"),
        "java.io.PrintWriter"
    );
    assert_eq!(
        uncaught(&mut eval, "(PrintWriter-on prn)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/PrintWriter-on"
    );
    assert_eq!(
        uncaught(&mut eval, "(PrintWriter-on prn nil true 4)"),
        "clojure.lang.ArityException | Wrong number of args (4) passed to: clojure.core/PrintWriter-on"
    );
}

// w-write-flush/flush-empty/close/close-nil/println/flush-default/
// noflush-noauto/noflush-auto/flush-noauto/print-newline-auto/
// flush-call/prn-to/flush-ret/close-ret: writes buffer, flush
// delivers, close flushes-then-calls-once; `autoflush?` is stored
// but never consulted (like the JVM at Clojure level).
#[test]
fn printwriter_behavior() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw] (print \"hi\") (.flush ^java.io.Writer pw)) @acc)"
        ),
        "[\"hi\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (.flush ^java.io.Writer pw) @acc)"
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) closed (atom false) pw (PrintWriter-on (fn [s] (swap! acc conj s)) (fn [] (reset! closed true)))] (binding [*out* pw] (print \"x\")) (.close ^java.io.Writer pw) [@acc @closed])"
        ),
        "[[\"x\"] true]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [pw (PrintWriter-on prn nil)] (.close ^java.io.Writer pw) :closed)"
        ),
        ":closed"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (println \"a\")) (.flush ^java.io.Writer pw) @acc)"
        ),
        "[\"a\\n\"]"
    );
    assert_eq!(run(&mut eval, "(boolean *flush-on-newline*)"), "true");
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (println \"a\")) @acc)"
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (binding [*out* pw *flush-on-newline* false] (println \"a\")) @acc)"
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* true] (println \"a\")) @acc)"
        ),
        "[\"a\\n\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (binding [*out* pw *flush-on-newline* false] (print \"a\\n\")) @acc)"
        ),
        "[]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw] (print \"z\") (flush)) @acc)"
        ),
        "[\"z\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (prn {:a 1})) (.flush ^java.io.Writer pw) @acc)"
        ),
        "[\"{:a 1}\\n\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [pw (PrintWriter-on prn nil)] (.flush ^java.io.Writer pw))"
        ),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [pw (PrintWriter-on prn nil)] (.close ^java.io.Writer pw))"
        ),
        "nil"
    );
    // Other writers no-op (returning nil, closing nothing).
    assert_eq!(run(&mut eval, "(do (.flush *out*) :ok)"), ":ok");
    assert_eq!(run(&mut eval, "(do (.close *out*) :ok)"), ":ok");
}

// w-nonfn-construct/flush, w-close-twice, w-write-after-close: a
// non-fn flush-fn fails at flush time (short shape, §2.4);
// close-fn runs once; writes after close drop silently.
#[test]
fn printwriter_edges() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(let [pw (PrintWriter-on 1 nil)] (class pw))"),
        "java.io.PrintWriter"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [pw (PrintWriter-on 1 nil)] (binding [*out* pw] (print \"x\")) (.flush ^java.io.Writer pw))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [n (atom 0) pw (PrintWriter-on prn (fn [] (swap! n inc)))] (.close ^java.io.Writer pw) (.close ^java.io.Writer pw) @n)"
        ),
        "1"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (.close ^java.io.Writer pw) (binding [*out* pw] (print \"q\")) (.flush ^java.io.Writer pw) @acc)"
        ),
        "[]"
    );
    // Interop on other writers is a silent no-op (§1.10 —
    // unlike `(flush)`, which flushes stdout).
    assert_eq!(
        run(&mut eval, "[(.. *out* (flush)) (.. *out* (close))]"),
        "[nil nil]"
    );
    // Close with a throwing flush fn: the flush error
    // propagates, `close-fn` is skipped, and the writer stays
    // closed (a second close still no-ops, §1.14).
    assert_eq!(
        uncaught(
            &mut eval,
            "(let [pw (PrintWriter-on (fn [s] (throw (ex-info \"boom\" {}))) prn)] (binding [*out* pw] (print \"x\")) (.close ^java.io.Writer pw))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [ran (atom false) pw (PrintWriter-on (fn [s] (throw (ex-info \"boom\" {}))) (fn [] (reset! ran true)))] (binding [*out* pw] (print \"x\")) [(try (.close ^java.io.Writer pw) (catch Throwable t :caught)) @ran (.close ^java.io.Writer pw)])"
        ),
        "[:caught false nil]"
    );
}

// w-direct-println-auto/noauto, w-direct-print-auto: oracle-only —
// direct `.println`/`.print` interop is the one autoflush mechanism
// the port has no surface for (§1.14).

// ---- .. ----

// Scrub both counters and gensym numbers (`target12` → `targetN`).
fn normboth(s: &str) -> String {
    let s = normctr(s);
    let s = normhex(&s);
    let mut out = String::with_capacity(s.len());
    let mut chars = s.chars().peekable();
    while let Some(c) = chars.next() {
        if c == 't' && out.ends_with("targe") && chars.peek().is_some_and(|d| d.is_ascii_digit()) {
            out.push('t');
            out.push('N');
            while chars.peek().is_some_and(|d| d.is_ascii_digit()) {
                chars.next();
            }
        } else {
            out.push(c);
        }
    }
    out
}

// m-expand-1/2/full/field, m-expand-arity0/1, m-arity1: the port
// emits `__meth*`/`__field*` calls (never the JVM's `.` forms),
// and arity failures use the standard macro-arity shape naming
// `clojure.core/..` (counts are the port's actual args).
#[test]
fn dotdot_expansions() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(macroexpand-1 '(.. System (getProperties)))"),
        "(clojure.core/__meth* (quote getProperties) System)"
    );
    assert_eq!(
        run(
            &mut eval,
            "(macroexpand-1 '(.. System (getProperties) (get \"os.name\")))"
        ),
        "(clojure.core/.. (clojure.core/__meth* (quote getProperties) System) (get \"os.name\"))"
    );
    assert_eq!(
        run(&mut eval, "(macroexpand '(.. \"a\" (toString) (length)))"),
        "(clojure.core/__meth* (quote length) (clojure.core/__meth* (quote toString) \"a\"))"
    );
    assert_eq!(
        run(&mut eval, "(macroexpand-1 '(.. rect p))"),
        "(clojure.core/__field* (quote p) rect)"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(.. x))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/.."
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(..))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/.."
    );
    assert_eq!(
        uncaught(&mut eval, "(eval '(.. \"a\"))"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/.."
    );
}

// m-thread-1/2/field, m-nil-tostring/hash/eq/getclass,
// m-nil-miss0/miss1/field, m-miss-0/1/field: threading, the nil
// target rule (resolving members NPE; anything else keeps the miss
// shapes with class `null`), and 0-arg misses reading field-first.
#[test]
fn dotdot_threading() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(.. \"a\" (toString))"), "\"a\"");
    assert_eq!(
        run(&mut eval, "(.. \"abc\" (toString) (toString))"),
        "\"abc\""
    );
    assert_eq!(
        run(&mut eval, "(.. \"abc\" (getClass) (getName))"),
        "\"java.lang.String\""
    );
    for src in [
        "(.. nil (toString))",
        "(.. nil (hashCode))",
        "(.. nil (equals \"a\"))",
        "(.. nil (getClass))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"target\" is null",
            "{src}"
        );
    }
    assert_eq!(
        uncaught(&mut eval, "(.. nil (nosuch_xyz))"),
        "java.lang.IllegalArgumentException | No matching field found: nosuch_xyz for class null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. nil (nosuch_xyz 1))"),
        "java.lang.IllegalArgumentException | No matching method nosuch_xyz found taking 1 args for class null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. nil nosuch_xyz)"),
        "java.lang.IllegalArgumentException | No matching field found: nosuch_xyz for class null"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. \"a\" (nosuch_xyz))"),
        "java.lang.IllegalArgumentException | No matching field found: nosuch_xyz for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. \"a\" (nosuch_xyz 1 2))"),
        "java.lang.IllegalArgumentException | No matching method nosuch_xyz found taking 2 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. \"a\" nosuch_xyz)"),
        "java.lang.IllegalArgumentException | No matching field found: nosuch_xyz for class java.lang.String"
    );
}

// m-strlen-oo, m-substring-oo: oracle-only — real JDK members
// outside the emulated surface (follow-up, §2.2).

// m-str-string/int/double/bool/char/bigint/bigdec/ratio/kw/sym/vec/
// map/set/list/entry/subvec/sortedmap/sortedset/range/cons/struct/
// var/class/lazy/atom/delay/promise/future/agent/volatile/multifn/
// record/deftype: `toString` ≡ `str` — these agree with the JVM
// (hash shapes scrubbed).
#[test]
fn surface_tostring() {
    let mut eval = boot();
    let cases = [
        ("(.. \"a\" (toString))", "\"a\""),
        ("(.. 1 (toString))", "\"1\""),
        ("(.. 1.5 (toString))", "\"1.5\""),
        ("(.. true (toString))", "\"true\""),
        ("(.. \\a (toString))", "\"a\""),
        ("(.. (bigint 5) (toString))", "\"5\""),
        ("(.. 1.5M (toString))", "\"1.5\""),
        ("(.. 1/3 (toString))", "\"1/3\""),
        ("(.. :a (toString))", "\":a\""),
        ("(.. 'a (toString))", "\"a\""),
        ("(.. [1 2] (toString))", "\"[1 2]\""),
        ("(.. {:a 1} (toString))", "\"{:a 1}\""),
        ("(.. #{1} (toString))", "\"#{1}\""),
        ("(.. '(1 2) (toString))", "\"(1 2)\""),
        ("(.. (first {:a 1}) (toString))", "\"[:a 1]\""),
        ("(.. (subvec [1 2 3] 0 2) (toString))", "\"[1 2]\""),
        ("(.. (sorted-map :a 1) (toString))", "\"{:a 1}\""),
        ("(.. (sorted-set 1) (toString))", "\"#{1}\""),
        ("(.. (range 3) (toString))", "\"(0 1 2)\""),
        ("(.. (cons 1 '(2)) (toString))", "\"(1 2)\""),
        (
            "(do (defstruct ms :a) (.. (struct ms 1) (toString)))",
            "\"{:a 1}\"",
        ),
        ("(.. #'map (toString))", "\"#'clojure.core/map\""),
        (
            "(.. (class \"a\") (toString))",
            "\"class java.lang.String\"",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    // Hash-suffixed shapes (scrubbed like the oracle).
    let hexed = [
        (
            "(.. (map inc [1]) (toString))",
            "\"clojure.lang.LazySeq@L\"",
        ),
        ("(.. (atom 1) (toString))", "\"clojure.lang.Atom@L\""),
        ("(.. (delay 1) (toString))", "\"clojure.lang.Delay@L\""),
        (
            "(.. (promise) (toString))",
            "\"clojure.core$promise$reify@L\"",
        ),
        (
            "(.. (future 1) (toString))",
            "\"clojure.core$future_call$reify@L\"",
        ),
        ("(.. (agent 1) (toString))", "\"clojure.lang.Agent@L\""),
        (
            "(.. (volatile! 1) (toString))",
            "\"clojure.lang.Volatile@L\"",
        ),
        (
            "(do (defmulti mm :k) (.. mm (toString)))",
            "\"clojure.lang.MultiFn@L\"",
        ),
        (
            "(do (defrecord QS2 [x]) (.. (QS2. 1) (toString)))",
            "\"user.QS2@L\"",
        ),
        (
            "(do (deftype QT2 [x]) (.. (QT2. 1) (toString)))",
            "\"user.QT2@L\"",
        ),
    ];
    for (src, want) in hexed {
        assert_eq!(normhex(&run(&mut eval, src)), want, "{src}");
    }
}

// m-str-regex/fn/ns/exinfo/transient-vec/transient-map/reader/chunk/matcher/
// eduction/reduced/iteration/accessor/readercond: `toString` ≡
// `str` — these DIVERGE (the port's print forms, §1.10).
#[test]
fn surface_tostring_divergent() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(.. #\"x\" (toString))"), "\"#\\\"x\\\"\"");
    assert_eq!(
        normboth(&run(&mut eval, "(.. (fn [x] x) (toString))")),
        "\"#object[user$evalN$fn__N 0xH \\\"user$evalN$fn__N@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. *ns* (toString))")),
        "\"#object[clojure.lang.Namespace 0xH \\\"user\\\"]\""
    );
    assert_eq!(
        run(&mut eval, "(.. (ex-info \"m\" {}) (toString))"),
        "\"#error {:cause \\\"m\\\" :data {} :via [{:type clojure.lang.ExceptionInfo :message \\\"m\\\" :data {}}] :trace []}\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. (transient [1]) (toString))")),
        "\"#object[clojure.lang.PersistentVector$TransientVector 0xH \\\"clojure.lang.PersistentVector$TransientVector@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. (transient {:a 1}) (toString))")),
        "\"#object[clojure.lang.PersistentArrayMap$TransientArrayMap 0xH \\\"clojure.lang.PersistentArrayMap$TransientArrayMap@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. *in* (toString))")),
        "\"#object[clojure.lang.LineNumberingPushbackReader 0xH \\\"clojure.lang.LineNumberingPushbackReader@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(
            &mut eval,
            "(let [b (chunk-buffer 2)] (chunk-append b 1) (.. (chunk b) (toString)))"
        )),
        "\"#object[clojure.lang.ArrayChunk 0xH \\\"clojure.lang.ArrayChunk@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. (re-matcher #\"x\" \"x\") (toString))")),
        "\"#object[java.util.regex.Matcher 0xH \\\"java.util.regex.Matcher[pattern=x region=0,1 lastmatch=]\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. (eduction (map inc) [1]) (toString))")),
        "\"#object[clojure.core.Eduction 0xH \\\"clojure.core.Eduction@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(&mut eval, "(.. (reduced 1) (toString))")),
        "\"#object[clojure.lang.Reduced 0xH \\\"clojure.lang.Reduced@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(
            &mut eval,
            "(.. (iteration (fn [x] (when (< x 3) [:a (inc x)])) :initk 0) (toString))"
        )),
        "\"#object[clojure.core$iteration 0xH \\\"clojure.core$iteration@L\\\"]\""
    );
    assert_eq!(
        normhex(&run(
            &mut eval,
            "(do (defstruct qsa :a) (.. (accessor qsa :a) (toString)))"
        )),
        "\"#object[clojure.lang.PersistentStructMap$1 0xH \\\"clojure.lang.PersistentStructMap$1@L\\\"]\""
    );
    // m-str-readercond: the constructor is out of scope; the same
    // print arrives through the preserving reader.
    assert_eq!(
        run(
            &mut eval,
            "(.. (read-string {:read-cond :preserve} \"#?(1)\") (toString))"
        ),
        "\"#?(1)\""
    );
}

// m-str-tagged: `tagged-literal` has no port constructor and no bare
// tagged value is surface-reachable — JVM-only row.

// m-eq-tt/tf/cross/bigint-long/long-bigint/nil-arg/vec/vec-list,
// m-eq-arity0/2, m-hash-str/int/vec/long, m-getclass: `equals` (the
// three rules), `hashCode` (≡ `hash-code`), `getClass` (≡ `class`).
#[test]
fn surface_equals_hashcode() {
    let mut eval = boot();
    let cases = [
        ("(.. \"a\" (equals \"a\"))", "true"),
        ("(.. \"a\" (equals \"b\"))", "false"),
        ("(.. 1 (equals 1))", "true"),
        ("(.. 1 (equals 2))", "false"),
        ("(.. 1 (equals 1.0))", "false"),
        ("(.. (bigint 1) (equals 1))", "false"),
        ("(.. 1 (equals (bigint 1)))", "false"),
        ("(.. \"a\" (equals nil))", "false"),
        ("(.. [1] (equals [1]))", "true"),
        ("(.. [1] (equals '(1)))", "true"),
        ("(.. \"a\" (hashCode))", "97"),
        ("(.. 1 (hashCode))", "1"),
        ("(.. [1 2] (hashCode))", "994"),
        ("(.. 123456789012345 (hashCode))", "-2045923535"),
        ("(.. \"a\" (getClass))", "java.lang.String"),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    assert_eq!(
        uncaught(&mut eval, "(.. \"a\" (equals))"),
        "java.lang.IllegalArgumentException | No matching method equals found taking 0 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. \"a\" (equals \"x\" \"y\"))"),
        "java.lang.IllegalArgumentException | No matching method equals found taking 2 args for class java.lang.String"
    );
    // `hashCode` refuses infinite seqs like `hash` does (no oracle
    // rows — the `hash-combine` coverage-pin shape): unbounded
    // inputs fail before realizing, nested ones after.
    for src in [
        "(.. (repeat 1) (hashCode))",
        "(.. (map identity [(repeat 1)]) (hashCode))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.UnsupportedOperationException | ",
            "{src}"
        );
    }
}

// m-eq-entry-vec/record-map/lazy-list/range-vec/lazy-vec/cons-vec/
// set-sorted/map-sorted/record-record/record-diff/deftype-same/
// deftype-diff/nan/negzero/bigdec-scale/ratio/record-diff-type/
// map-record/record-struct: Java collection semantics — the tower
// rule, the record-content rule, and `=` otherwise.
#[test]
fn surface_equals_expansions() {
    let mut eval = boot();
    let cases = [
        ("(.. (first {:a 1}) (equals [:a 1]))", "true"),
        (
            "(do (defrecord QR [x]) (.. (QR. 1) (equals {:x 1})))",
            "true",
        ),
        ("(.. (map inc [1]) (equals '(2)))", "true"),
        ("(.. (range 2) (equals [0 1]))", "true"),
        ("(.. (map inc [1]) (equals [2]))", "true"),
        ("(.. (cons 1 '(2)) (equals [1 2]))", "true"),
        ("(.. #{1} (equals (sorted-set 1)))", "true"),
        ("(.. {:a 1} (equals (sorted-map :a 1)))", "true"),
        (
            "(do (defrecord QS [x]) (.. (QS. 1) (equals (QS. 1))))",
            "true",
        ),
        (
            "(do (defrecord QT [x]) (.. (QT. 1) (equals (QT. 2))))",
            "false",
        ),
        (
            "(do (deftype QD [x]) (let [i (QD. 1)] (.. i (equals i))))",
            "true",
        ),
        (
            "(do (deftype QE [x]) (.. (QE. 1) (equals (QE. 1))))",
            "false",
        ),
        ("(.. ##NaN (equals ##NaN))", "true"),
        ("(.. 0.0 (equals -0.0))", "false"),
        ("(.. 1.5M (equals 1.50M))", "false"),
        ("(.. 1/3 (equals 2/6))", "true"),
        ("(.. (bigint 1) (equals (bigint 1)))", "true"),
        ("(do (defrecord QE1 [x]) (.. (QE1. 1) (equals 1)))", "false"),
        // A record against a non-tower non-map is simply unequal
        // (rule 2's `else false` — the tower rule never sees it).
        (
            "(do (defrecord QE4 [x]) (.. (QE4. 1) (equals \"x\")))",
            "false",
        ),
        (
            "(do (defrecord QE2 [x]) (deftype QD3 [x]) (.. (QE2. 1) (equals (QD3. 1))))",
            "false",
        ),
        (
            "(do (defrecord QRa [x]) (defrecord QRb [x]) (.. (QRa. 1) (equals (QRb. 1))))",
            "true",
        ),
        (
            "(do (defrecord QRc [x]) (.. {:x 1} (equals (QRc. 1))))",
            "true",
        ),
        (
            "(do (defrecord QRd [x]) (defstruct qse :x) (.. (QRd. 1) (equals (struct qse 1))))",
            "true",
        ),
        // Universals shadow host members (§1.10): the custom
        // `equals` never runs — the surface answers identity
        // `=` (not `:custom`).
        (
            "(do (definterface IEq (equals [x])) (deftype CEq [x] IEq (equals [this o] :custom)) (let [i (CEq. 1)] [(.. i (equals i)) (.. i (equals (CEq. 1)))]))",
            "[true false]",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

// m-cls-getname/simple/simple-nested/simple-fn/super/super-iface/
// super-object/ifaces/ifaces-order/isiface-t/isiface-f/isarray/
// isinstance-t/isinstance-f/isinstance-nil/assign-t/assign-f/
// assign-iface/assign-nonclass/assign-nil/cast-ok/cast-miss/
// tostring/eq/eq-f/getclass/miss: the `Class` reflection members.
#[test]
fn surface_class_members() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Long 'java.lang.Object 'java.lang.Comparable)",
    );
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (getName))"),
        "\"java.lang.String\""
    );
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (getSimpleName))"),
        "\"String\""
    );
    assert_eq!(
        run(&mut eval, "(.. (class (first {:a 1})) (getSimpleName))"),
        "\"MapEntry\""
    );
    assert_eq!(
        normctr(&run(&mut eval, "(.. (class (fn [x] x)) (getSimpleName))")),
        "\"user$evalN$fn__N\""
    );
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (getSuperclass))"),
        "java.lang.Object"
    );
    assert_eq!(run(&mut eval, "(.. Comparable (getSuperclass))"), "nil");
    // m-cls-super-object: `(Object.)` cannot construct (§1.8), so
    // the `Object` class arrives via its import.
    assert_eq!(run(&mut eval, "(.. Object (getSuperclass))"), "nil");
    assert_eq!(
        run(
            &mut eval,
            "(vec (sort (map #(.getName ^Class %) (.. (class \"a\") (getInterfaces)))))"
        ),
        "[\"java.io.Serializable\" \"java.lang.CharSequence\" \"java.lang.Comparable\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"]"
    );
    assert_eq!(
        run(
            &mut eval,
            "(vec (map #(.getName ^Class %) (.. (class \"a\") (getInterfaces))))"
        ),
        "[\"java.io.Serializable\" \"java.lang.Comparable\" \"java.lang.CharSequence\" \"java.lang.constant.Constable\" \"java.lang.constant.ConstantDesc\"]"
    );
    assert_eq!(run(&mut eval, "(.. Comparable (isInterface))"), "true");
    assert_eq!(run(&mut eval, "(.. (class \"a\") (isInterface))"), "false");
    assert_eq!(run(&mut eval, "(.. (class \"a\") (isArray))"), "false");
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (isInstance \"a\"))"),
        "true"
    );
    assert_eq!(run(&mut eval, "(.. (class \"a\") (isInstance 1))"), "false");
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (isInstance nil))"),
        "false"
    );
    // m-cls-assign-t/f: same `(Object.)` adaptation as
    // `m-cls-super-object`.
    assert_eq!(
        run(&mut eval, "(.. Object (isAssignableFrom String))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (isAssignableFrom Object))"),
        "false"
    );
    assert_eq!(
        run(&mut eval, "(.. Comparable (isAssignableFrom String))"),
        "true"
    );
    // m-cls-assign-nonclass: the port uses the short shape (the
    // oracle shows the module shape, §2.4).
    assert_eq!(
        uncaught(&mut eval, "(.. (class \"a\") (isAssignableFrom 1))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.Class"
    );
    // m-cls-assign-nil: the JVM's message is nil (the oracle prints
    // `<nil>`); the port throws the empty message.
    assert_eq!(
        uncaught(&mut eval, "(.. (class \"a\") (isAssignableFrom nil))"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(run(&mut eval, "(.. (class \"a\") (cast \"a\"))"), "\"a\"");
    assert_eq!(
        uncaught(&mut eval, "(.. (class \"a\") (cast 1))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to java.lang.String"
    );
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (toString))"),
        "\"class java.lang.String\""
    );
    assert_eq!(run(&mut eval, "(.. (class \"a\") (equals String))"), "true");
    assert_eq!(run(&mut eval, "(.. (class \"a\") (equals Long))"), "false");
    assert_eq!(
        run(&mut eval, "(.. (class \"a\") (getClass))"),
        "java.lang.Class"
    );
    // Classes miss method-shape at every arity (unlike the
    // field-first 0-arg misses on other values).
    assert_eq!(
        uncaught(&mut eval, "(.. (class \"a\") (nosuch_xyz))"),
        "java.lang.IllegalArgumentException | No matching method nosuch_xyz found taking 0 args for class java.lang.Class"
    );
}

// m-cls-symbol-target: oracle-only — the JVM reads a class-name
// symbol target as a static call; the port has no statics (an
// imported `String` target takes the instance path, `m-cls-getname`
// above).

// m-memfn-expand/expand-args/expand-hint/hint-meta/call/call-args/
// arity0: `memfn` emits the `__meth*` call (never the JVM's `.`
// form); a `^String` hint lands as `:tag` on the target.
#[test]
fn memfn_tests() {
    let mut eval = boot();
    assert_eq!(
        normboth(&run(&mut eval, "(macroexpand-1 '(memfn toString))")),
        "(fn* [targetN] (clojure.core/__meth* (quote toString) targetN))"
    );
    assert_eq!(
        normboth(&run(
            &mut eval,
            "(macroexpand-1 '(memfn substring start end))"
        )),
        "(fn* [targetN start end] (clojure.core/__meth* (quote substring) targetN start end))"
    );
    assert_eq!(
        normboth(&run(
            &mut eval,
            "(macroexpand-1 '(memfn ^String substring start))"
        )),
        "(fn* [targetN start] (clojure.core/__meth* (quote substring) targetN start))"
    );
    assert_eq!(
        run(&mut eval, "(do (def mts (memfn toString)) (mts \"a\"))"),
        "\"a\""
    );
    // m-memfn-call-args: the oracle calls `substring` (off-surface
    // here — `m-substring-oo`), so the port misses there; the
    // multi-arg call shape transcribes through `equals`.
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (def mss (memfn substring s e)) (mss \"abc\" 1 2))"
        ),
        "java.lang.IllegalArgumentException | No matching method substring found taking 2 args for class java.lang.String"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (def meq (memfn equals other)) (meq \"a\" \"a\"))"
        ),
        "true"
    );
    assert_eq!(
        uncaught(&mut eval, "(macroexpand-1 '(memfn))"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/memfn"
    );
    assert_eq!(
        run(
            &mut eval,
            "(-> (macroexpand-1 '(memfn ^String substring start)) second first meta)"
        ),
        "{:tag String}"
    );
}

// ---- bean ----

// b-keys-str/empty-f/empty-t/class-val/keys-long/keys-kw/keys-vec/
// keys-map/keys-class/cls-vals/keys-iface/nil/record/lazy/missing/
// contains/count/assoc/meta, m-det-bean-keys, m-bean-cap: `bean` is
// an eager plain map (`:class` + one entry per 0-arg getter, minus
// `getClass`); the JVM re-invokes per access, the port calls once.
#[test]
fn bean_basics() {
    let mut eval = boot();
    run(
        &mut eval,
        "(import 'java.lang.String 'java.lang.Comparable)",
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean \"hi\"))))"),
        "(\":blank\" \":class\" \":empty\")"
    );
    assert_eq!(run(&mut eval, "(:empty (bean \"hi\"))"), "false");
    assert_eq!(run(&mut eval, "(:empty (bean \"\"))"), "true");
    // b-bytes: oracle-only — `:bytes` needs byte arrays (S15, §2.2).
    assert_eq!(run(&mut eval, "(:class (bean \"hi\"))"), "java.lang.String");
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean 1))))"),
        "(\":class\")"
    );
    // Writers answer `:class` alone (`close`/`flush` are void
    // members, never keys, §1.11).
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean *out*))))"),
        "(\":class\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean :a))))"),
        "(\":class\" \":name\" \":namespace\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean [1]))))"),
        "(\":class\" \":empty\" \":first\" \":last\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean {:a 1}))))"),
        "(\":class\" \":empty\")"
    );
    // b-keys-class/iface: the 7 computable reflection keys (the
    // other 41 need a real JVM — follow-up, §2.2).
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean String))))"),
        "(\":array\" \":class\" \":interface\" \":interfaces\" \":name\" \":simpleName\" \":superclass\")"
    );
    assert_eq!(
        run(&mut eval, "(sort (map str (keys (bean Comparable))))"),
        "(\":array\" \":class\" \":interface\" \":interfaces\" \":name\" \":simpleName\" \":superclass\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [b (bean String)] [(:name b) (:simpleName b) (:superclass b) (:interface b) (:array b)])"
        ),
        "[\"java.lang.String\" \"String\" java.lang.Object false false]"
    );
    assert_eq!(
        uncaught(&mut eval, "(bean nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"x\" is null"
    );
    // Eager `bean` of an empty `List` throws at `bean` time
    // (§1.11, §2.4 — the JVM throws ITE only when `:first`/
    // `:last` is touched).
    assert_eq!(
        uncaught(&mut eval, "(bean [])"),
        "java.util.NoSuchElementException | "
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (defrecord BeanRec [x]) (sort (map str (keys (bean (BeanRec. 1))))))"
        ),
        "(\":class\" \":empty\")"
    );
    // b-lazy: getters run once, eagerly — `[1 1]`, where the JVM's
    // per-access re-invocation answers `[1 2]`. (The port reads the
    // field explicitly; the oracle's bare `c` is JVM-only magic.)
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface ICounter (getCount [])) (deftype BCnt [c] ICounter (getCount [this] (swap! (.-c this) inc) @(.-c this))) (let [b (bean (BCnt. (atom 0)))] [(:count b) (:count b)]))"
        ),
        "[1 1]"
    );
    assert_eq!(
        run(
            &mut eval,
            "[(:nosuch (bean \"hi\")) (:nosuch (bean \"hi\") :dflt)]"
        ),
        "[nil :dflt]"
    );
    assert_eq!(run(&mut eval, "(contains? (bean \"hi\") :empty)"), "true");
    assert_eq!(run(&mut eval, "(count (bean \"hi\"))"), "3");
    assert_eq!(
        run(
            &mut eval,
            "(let [m (assoc (bean \"hi\") :x 1)] [(sort (map str (keys m))) (:x m) (:empty m)])"
        ),
        "[(\":blank\" \":class\" \":empty\" \":x\") 1 false]"
    );
    assert_eq!(run(&mut eval, "(meta (bean \"hi\"))"), "nil");
    // Throwing getters fail `bean` itself (eager, §1.11).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (definterface TG (getBoom [])) (deftype TB [] TG (getBoom [this] (throw (ex-info \"boom\" {})))) (bean (TB.)))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
    // m-det-bean-keys/m-bean-cap: HostDef 0-arg `get*` methods
    // become eagerly-called keys (JavaBeans decapitalize).
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface ICounter2 (getCount [])) (deftype BCn [c] ICounter2 (getCount [this] 42)) (sort (map str (keys (bean (BCn. nil))))))"
        ),
        "(\":class\" \":count\")"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (definterface IUrl (getURL [])) (deftype BU [] IUrl (getURL [this] \"u\")) (sort (map str (keys (bean (BU.))))))"
        ),
        "(\":URL\" \":class\")"
    );
}

// b-keys-sym/var/ns/pattern/matcher/transient/reader/future/promise/
// delay/atom/agent/volatile/exinfo/multifn/eduction/reduced/chunk/
// iteration/accessor/lazy/range/cons/subvec/entry/sortedmap/
// sortedset/struct/deftype/structdef/iface, m-fn-bean: per-kind key
// tables (the computable subset, §1.11).
#[test]
fn bean_keys() {
    let mut eval = boot();
    let one_class = [
        "(bean #'map)",
        "(bean *ns*)",
        "(bean #\"x\")",
        "(bean (re-matcher #\"x\" \"x\"))",
        "(bean (transient [1]))",
        "(bean *in*)",
        "(bean (atom 1))",
        "(bean (agent 1))",
        "(bean (volatile! 1))",
        "(bean (eduction (map inc) [1]))",
        "(bean (reduced 1))",
        "(bean (fn [x] x))",
    ];
    for src in one_class {
        assert_eq!(
            run(&mut eval, &format!("(sort (map str (keys {src})))")),
            "(\":class\")",
            "{src}"
        );
    }
    // b-keys-var: the JVM's CCE is an accidental getter; the port
    // answers `:class` (covered above).
    let cases = [
        (
            "(sort (map str (keys (bean (future 1)))))",
            "(\":class\" \":realized\")",
        ),
        (
            "(let [p (promise)] (deliver p 1) (sort (map str (keys (bean p)))))",
            "(\":class\" \":realized\")",
        ),
        (
            "(sort (map str (keys (bean (delay 1)))))",
            "(\":class\" \":realized\")",
        ),
        (
            "(sort (map str (keys (bean (ex-info \"m\" {})))))",
            "(\":cause\" \":class\" \":data\" \":localizedMessage\" \":message\")",
        ),
        // A non-`ExceptionInfo` throwable has no `:data` (and the
        // `:stackTrace`/`:suppressed` follow-up keys stay out, §2.2).
        (
            "(try (/ 1 0) (catch ArithmeticException e (sort (map str (keys (bean e))))))",
            "(\":cause\" \":class\" \":localizedMessage\" \":message\")",
        ),
        // A `get`-prefixed lowercase method is not a getter (the
        // JVM's property reflection needs the uppercase letter).
        (
            "(do (definterface IQx (getx [])) (deftype QX [x] IQx (getx [this] 1)) (sort (map str (keys (bean (QX. 1))))))",
            "(\":class\")",
        ),
        (
            "(do (defmulti qm :k) (sort (map str (keys (bean qm)))))",
            "(\":class\")",
        ),
        (
            "(let [b (chunk-buffer 2)] (chunk-append b 1) (sort (map str (keys (bean (chunk b))))))",
            "(\":class\")",
        ),
        (
            "(sort (map str (keys (bean (iteration (fn [x] (when (< x 3) [:a (inc x)])) :initk 0)))))",
            "(\":class\")",
        ),
        (
            "(do (defstruct qs :a) (sort (map str (keys (bean (accessor qs :a))))))",
            "(\":class\")",
        ),
        (
            "(sort (map str (keys (bean (map inc [1])))))",
            "(\":class\" \":empty\" \":first\" \":last\" \":realized\")",
        ),
        (
            "(sort (map str (keys (bean (range 3)))))",
            "(\":class\" \":empty\" \":first\" \":last\")",
        ),
        (
            "(sort (map str (keys (bean (cons 1 '(2))))))",
            "(\":class\" \":empty\" \":first\" \":last\")",
        ),
        (
            "(sort (map str (keys (bean (subvec [1 2 3] 0 2)))))",
            "(\":class\" \":empty\" \":first\" \":last\")",
        ),
        (
            "(sort (map str (keys (bean (first {:a 1})))))",
            "(\":class\" \":empty\" \":first\" \":key\" \":last\" \":value\")",
        ),
        (
            "(sort (map str (keys (bean (sorted-map :a 1)))))",
            "(\":class\" \":empty\")",
        ),
        (
            "(sort (map str (keys (bean (sorted-set 1)))))",
            "(\":class\" \":empty\")",
        ),
        (
            "(do (defstruct qst :a) (sort (map str (keys (bean (struct qst 1))))))",
            "(\":class\" \":empty\")",
        ),
        (
            "(do (deftype QDt [x]) (sort (map str (keys (bean (QDt. 1))))))",
            "(\":class\")",
        ),
        (
            "(do (defstruct qsd :a) (sort (map str (keys (bean qsd)))))",
            "(\":class\")",
        ),
        (
            "(do (definterface QIf (m [x])) (sort (map str (keys (bean QIf)))))",
            "(\":class\")",
        ),
        (
            "(sort (map str (keys (bean 'a))))",
            "(\":class\" \":name\" \":namespace\")",
        ),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
}

// m-deftype-field/method, m-record-field: `..` over port-defined
// types rides the existing `__field*`/`__meth*` dispatch.
#[test]
fn surface_hostdef() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype MPoint [x y]) (.. (MPoint. 1 2) x))"
        ),
        "1"
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (deftype MM [x] Object (toString [this] \"MM!\")) (.. (MM. 1) (toString)))"
        ),
        "\"MM!\""
    );
    assert_eq!(
        run(&mut eval, "(do (defrecord MR [x]) (.. (MR. 5) x))"),
        "5"
    );
}

// m-docstring-oo: oracle-only — a static call (follow-up, §2.2).

// m-isempty-*, m-isblank-*, m-named-*, m-first-*, m-last-*,
// m-entry-*, m-realized-*: the gated members (`String`, `Named`,
// `List`, `Collection`/`Map`, `MapEntry`, `IPending`).
#[test]
fn surface_gates() {
    let mut eval = boot();
    let cases = [
        ("(.. \"\" (isEmpty))", "true"),
        ("(.. \"a\" (isEmpty))", "false"),
        ("(.. [] (isEmpty))", "true"),
        ("(.. [1] (isEmpty))", "false"),
        ("(.. {} (isEmpty))", "true"),
        ("(.. #{} (isEmpty))", "true"),
        ("(.. '(1) (isEmpty))", "false"),
        ("(.. \"  \" (isBlank))", "true"),
        ("(.. \"a\" (isBlank))", "false"),
        ("(.. \" \\t\\n\" (isBlank))", "true"),
        ("(.. (first {:a 1}) (isEmpty))", "false"),
        ("(.. (map inc [1]) (isEmpty))", "false"),
        ("(.. (cons 1 nil) (isEmpty))", "false"),
        ("(.. (sorted-map :a 1) (isEmpty))", "false"),
        ("(do (defrecord QR2 [x]) (.. (QR2. 1) (isEmpty)))", "false"),
        (
            "(do (defstruct qs2 :a) (.. (struct qs2 1) (isEmpty)))",
            "false",
        ),
        ("(.. :a/b (getNamespace))", "\"a\""),
        ("(.. :a/b (getName))", "\"b\""),
        ("(.. :a (getNamespace))", "nil"),
        ("(.. 'a/b (getName))", "\"b\""),
        ("(.. 'a (getNamespace))", "nil"),
        ("(.. [1 2] (getFirst))", "1"),
        ("(.. [1 2] (getLast))", "2"),
        ("(.. '(1 2) (getFirst))", "1"),
        ("(.. (range 3) (getFirst))", "0"),
        ("(.. (subvec [1 2 3] 0 2) (getFirst))", "1"),
        ("(.. (cons 1 '(2)) (getFirst))", "1"),
        ("(.. (map inc [1]) (getFirst))", "2"),
        ("(.. (first {:a 1}) (getFirst))", ":a"),
        ("(.. (first {:a 1}) (getLast))", "1"),
        ("(.. (first {:a 1}) (getKey))", ":a"),
        ("(.. (first {:a 1}) (getValue))", "1"),
        ("(.. (delay 1) (isRealized))", "false"),
        ("(let [d (delay 1)] (force d) (.. d (isRealized)))", "true"),
        ("(.. (promise) (isRealized))", "false"),
        (
            "(let [p (promise)] (deliver p 1) (.. p (isRealized)))",
            "true",
        ),
        ("(let [f (future 1)] @f (.. f (isRealized)))", "true"),
        ("(.. (map inc [1]) (isRealized))", "false"),
        (
            "(let [s (map inc [1])] (doall s) (.. s (isRealized)))",
            "true",
        ),
        // Cycles/iterates are `IPending` and report realized (the
        // JVM's `isRealized` is unconditionally true there —
        // JVM-verified live; no oracle rows).
        ("(.. (cycle [1 2]) (isRealized))", "true"),
        ("(.. (iterate inc 1) (isRealized))", "true"),
    ];
    for (src, want) in cases {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    let misses = [
        (
            "(.. 1 (isEmpty))",
            "java.lang.IllegalArgumentException | No matching method isEmpty found taking 0 args for class java.lang.Long",
        ),
        (
            "(do (deftype QD2 [x]) (.. (QD2. 1) (isEmpty)))",
            "java.lang.IllegalArgumentException | No matching method isEmpty found taking 0 args for class user.QD2",
        ),
        (
            "(.. (class \"a\") (isEmpty))",
            "java.lang.IllegalArgumentException | No matching method isEmpty found taking 0 args for class java.lang.Class",
        ),
        (
            "(.. #'map (getName))",
            "java.lang.IllegalArgumentException | No matching method getName found taking 0 args for class clojure.lang.Var",
        ),
        (
            "(.. #'map (getNamespace))",
            "java.lang.IllegalArgumentException | No matching method getNamespace found taking 0 args for class clojure.lang.Var",
        ),
        (
            "(.. \"ab\" (getFirst))",
            "java.lang.IllegalArgumentException | No matching method getFirst found taking 0 args for class java.lang.String",
        ),
        (
            "(.. {:a 1} (getFirst))",
            "java.lang.IllegalArgumentException | No matching method getFirst found taking 0 args for class clojure.lang.PersistentArrayMap",
        ),
        (
            "(.. (sorted-set 1) (getFirst))",
            "java.lang.IllegalArgumentException | No matching method getFirst found taking 0 args for class clojure.lang.PersistentTreeSet",
        ),
        (
            "(.. 1 (isRealized))",
            "java.lang.IllegalArgumentException | No matching method isRealized found taking 0 args for class java.lang.Long",
        ),
        (
            "(.. \"a\" (close))",
            "java.lang.IllegalArgumentException | No matching method close found taking 0 args for class java.lang.String",
        ),
        (
            "(.. \"a\" (flush))",
            "java.lang.IllegalArgumentException | No matching method flush found taking 0 args for class java.lang.String",
        ),
        (
            "(do (deftype QDd [x]) (.. QDd (isEmpty)))",
            "java.lang.IllegalArgumentException | No matching method isEmpty found taking 0 args for class user.QDd",
        ),
        (
            "(.. 1 (isBlank))",
            "java.lang.IllegalArgumentException | No matching method isBlank found taking 0 args for class java.lang.Long",
        ),
        (
            "(.. \"a\" (getKey))",
            "java.lang.IllegalArgumentException | No matching method getKey found taking 0 args for class java.lang.String",
        ),
        (
            "(.. \"a\" (getValue))",
            "java.lang.IllegalArgumentException | No matching method getValue found taking 0 args for class java.lang.String",
        ),
        (
            "(.. \"a\" (getMessage))",
            "java.lang.IllegalArgumentException | No matching method getMessage found taking 0 args for class java.lang.String",
        ),
        (
            "(.. \"a\" (getCause))",
            "java.lang.IllegalArgumentException | No matching method getCause found taking 0 args for class java.lang.String",
        ),
        (
            "(.. \"a\" (getData))",
            "java.lang.IllegalArgumentException | No matching method getData found taking 0 args for class java.lang.String",
        ),
    ];
    for (src, want) in misses {
        assert_eq!(uncaught(&mut eval, src), want, "{src}");
    }
    // Non-writer extensions miss the same way (counters scrubbed).
    assert_eq!(
        normctr(&uncaught(&mut eval, "(.. (fn [x] x) (close))")),
        "java.lang.IllegalArgumentException | No matching method close found taking 0 args for class user$evalN$fn__N"
    );
    assert_eq!(
        normctr(&uncaught(&mut eval, "(.. (fn [x] x) (flush))")),
        "java.lang.IllegalArgumentException | No matching method flush found taking 0 args for class user$evalN$fn__N"
    );
    // m-first-empty: the JVM's message is nil (the oracle prints
    // `<nil>`); the port throws the empty message.
    assert_eq!(
        uncaught(&mut eval, "(.. [] (getFirst))"),
        "java.util.NoSuchElementException | "
    );
    // `getLast` on empty throws the same (no oracle row — the
    // by-position rule covers both ends).
    assert_eq!(
        uncaught(&mut eval, "(.. [] (getLast))"),
        "java.util.NoSuchElementException | "
    );
}

// t-msg/msg-nil/msg-exc/loc/cause-nil/cause-msg/cause-class,
// m-exinfo-data: the `Throwable` getters (`getData` on
// `ExceptionInfo` only).
#[test]
fn surface_throwable() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.. (ex-info \"m\" {}) (getMessage))"),
        "\"m\""
    );
    // t-msg-nil: `(RuntimeException.)` cannot construct (§1.8); a
    // nil-message `ex-info` throws the same nil.
    assert_eq!(run(&mut eval, "(.. (ex-info nil {}) (getMessage))"), "nil");
    // t-msg-exc: same adaptation through a caught divide-by-zero.
    assert_eq!(
        run(
            &mut eval,
            "(.. (try (/ 1 0) (catch Throwable t t)) (getMessage))"
        ),
        "\"Divide by zero\""
    );
    assert_eq!(
        run(&mut eval, "(.. (ex-info \"m\" {}) (getLocalizedMessage))"),
        "\"m\""
    );
    assert_eq!(run(&mut eval, "(.. (ex-info \"m\" {}) (getCause))"), "nil");
    assert_eq!(
        run(
            &mut eval,
            "(.. (.. (ex-info \"m\" {} (ex-info \"inner\" {})) (getCause)) (getMessage))"
        ),
        "\"inner\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(.getName ^Class (class (.. (ex-info \"m\" {} (try (/ 1 0) (catch Throwable t t))) (getCause))))"
        ),
        "\"java.lang.ArithmeticException\""
    );
    assert_eq!(
        run(&mut eval, "(.. (ex-info \"m\" {:a 1}) (getData))"),
        "{:a 1}"
    );
}

// m-isdone-future-oo, m-isdone-promise-oo, m-reader-line-oo:
// oracle-only — excluded kind-specific getters (follow-up, §2.2).

// ---- arrays (S15 follow-up; all rows JVM-only) ----
//
// The port has no array values and `isArray` answers `false`
// unconditionally (`m-cls-isarray`); array-class emulation (lattice
// arms, `getComponentType`, `[`-names) is the S15 follow-up. Oracle
// values, for the record:
//
// a-class-obj     "[Ljava.lang.Long;"
// a-class-int     "[I"
// a-class-str     "[Ljava.lang.String;"
// a-class-2d      "[[J"
// a-bases-obj     ["java.lang.Object" "java.lang.Cloneable" "java.io.Serializable"]
// a-bases-int     ["java.lang.Object" "java.lang.Cloneable" "java.io.Serializable"]
// a-supers-int    ["java.io.Serializable" "java.lang.Cloneable" "java.lang.Object"]
// a-instance-t    true
// a-instance-f    false
// a-instance-obj  true
// a-instance-cloneable true
// a-cast-ok       [2]
// a-cast-miss     CCE "Cannot cast [J to [I" (short shape)
// a-class-array?  true
// a-str           "class [I"
// a-pr            "int/1"
// a-isarray       true
// a-component     "int"
