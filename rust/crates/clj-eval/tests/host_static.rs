// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #120 slice 1: static `Class/forName` over the emulated
//! lattice, fully-qualified class literals, and the namespace-part
//! `ClassNotFoundException` rule. Every pin cites its JVM oracle
//! label; the probes ran under:
//! `java -Duser.language=en -Duser.country=US -Duser.timezone=UTC
//! -cp clojure-1.13.0-alpha9.jar:spec.alpha-0.6.249.jar:
//! core.specs.alpha-0.6.133-alpha10.jar clojure.main probeN.clj`.
//! Deferred to later slices (pinned only as current behavior where
//! noted): array descriptors in `forName`, further statics and
//! static fields, value-position static members, and the 3-arg `.`
//! special form.

use clj_eval::{Evaluator, SourceError, boot};
use clj_reader::{NoHooks, ReaderOpts, read_str};
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

/// Evaluate one form in a fresh `eval_source` call (reads see the
/// imports the previous calls committed — one call reads all its
/// forms up front).
fn run_step(eval: &mut Evaluator, src: &str) -> String {
    run(eval, src)
}

/// The `Class | message` of an uncaught throw (strict: compile
/// errors fail — phase pins must not silently pass).
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

/// A compile error's `Class | message` (strict: uncaught throws
/// fail — phase pins must not silently pass).
fn compile_err(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Compile(e)) => format!("{} | {}", e.class, e.message),
        Err(other) => panic!("expected a compile error, got {other:?}"),
        Ok(vs) => panic!("expected a compile error, got values: {vs:?}"),
    }
}

/// The full compile chain (wrapper + cause) for an analyzed form.
fn chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let form = read_str(src, &ReaderOpts::default(), &mut NoHooks).unwrap();
    match eval.analyze(&form) {
        Ok(_) => panic!("expected an error for {src}"),
        Err(e) => e.chain(),
    }
}

fn cce_bootstrap(from: &str, to: &str) -> String {
    format!(
        "java.lang.ClassCastException | class {from} cannot be cast to class {to} ({from} and {to} are in module java.base of loader 'bootstrap')"
    )
}

// ---- 1-arg forName ----

// forName-String/forName-eq/fq-forName/cls-getName/cls-eq-class:
// the lattice resolves through simple (default-imported) and
// fully-qualified heads alike.
#[test]
fn forname_one_lattice() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(Class/forName \"java.lang.String\")"),
        "java.lang.String"
    );
    assert_eq!(
        run(&mut eval, "(java.lang.Class/forName \"java.lang.Long\")"),
        "java.lang.Long"
    );
    assert_eq!(
        run(&mut eval, "(= String (Class/forName \"java.lang.String\"))"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(type (Class/forName \"java.lang.String\"))"),
        "java.lang.Class"
    );
    assert_eq!(
        run(&mut eval, "(.getName (Class/forName \"java.lang.String\"))"),
        "\"java.lang.String\""
    );
}

// forName-missing/forName-int/forName-nil/forName-num/o1-empty:
// unknown names CNFE, nil NPEs (port-pinned text — the JDK message
// comes from `Class.forName0` internals and varies by JDK),
// non-strings fail the bootstrap cast. `forName-arr*` (array
// descriptors) is slice 2: they CNFE like any unresolvable name.
#[test]
fn forname_one_misses() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"no.such.Klass\")"),
        "java.lang.ClassNotFoundException | no.such.Klass"
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"int\")"),
        "java.lang.ClassNotFoundException | int"
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"\")"),
        "java.lang.ClassNotFoundException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"[I\")"),
        "java.lang.ClassNotFoundException | [I"
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Class.forName(String)\" because \"name\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName 1)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

// deftype-forName/deftype-name: `HostDef` rows resolve, and struct
// names carve exactly like `import`.
#[test]
fn forname_hostdefs() {
    let mut eval = boot();
    run(&mut eval, "(deftype FQ1 [a])");
    assert_eq!(run(&mut eval, "(Class/forName \"user.FQ1\")"), "user.FQ1");
    assert_eq!(
        run(&mut eval, "(.getName (Class/forName \"user.FQ1\"))"),
        "\"user.FQ1\""
    );
    run(&mut eval, "(defstruct fqst :a)");
    assert_eq!(
        run(&mut eval, "(.getName (Class/forName \"user.fqst\"))"),
        "\"clojure.lang.PersistentStructMap$Def\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(do (in-ns 'impfq) (clojure.core/refer-clojure) (= (import 'user.fqst) (Class/forName \"user.fqst\")))"
        ),
        "true"
    );
}

// ---- 3-arg forName ----

// forName-3arg/forName-3arg-init/forName-init-str/
// forName-loader-str/forName-nil-init/o3-*: the flag is accepted
// (true and false), casts run in order and nil-permissive, the
// unbox NPEs before the loader cast and the deref
// (port-pinned text — the JDK's `because "null" is null` is
// version-specific), and only nil loaders pass.
#[test]
fn forname_three() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(Class/forName \"java.lang.String\" false nil)"),
        "java.lang.String"
    );
    assert_eq!(
        run(&mut eval, "(Class/forName \"java.lang.String\" true nil)"),
        "java.lang.String"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(Class/forName \"java.lang.String\" \"yes\" nil)"
        ),
        cce_bootstrap("java.lang.String", "java.lang.Boolean")
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"java.lang.String\" nil nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Boolean.booleanValue()\" because \"initialize\" is null"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(Class/forName \"java.lang.String\" false \"loader\")"
        ),
        cce_bootstrap("java.lang.String", "java.lang.ClassLoader")
    );
    // Cast order: name, then init, then loader — all before the
    // resolve (so a bad init beats a missing class).
    assert_eq!(
        uncaught(&mut eval, "(Class/forName 1 \"y\" \"z\")"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(Class/forName \"java.lang.String\" \"y\" \"z\")"
        ),
        cce_bootstrap("java.lang.String", "java.lang.Boolean")
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"missing.K\" \"bad\" nil)"),
        cce_bootstrap("java.lang.String", "java.lang.Boolean")
    );
    // The unbox beats the name deref.
    assert_eq!(
        uncaught(&mut eval, "(Class/forName nil nil nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Boolean.booleanValue()\" because \"initialize\" is null"
    );
    // The unbox beats a bad loader (oracle o3-init-unbox-first).
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"java.lang.String\" nil \"x\")"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Boolean.booleanValue()\" because \"initialize\" is null"
    );
}

// ---- 2-arg forName ----

// forName-2arg/forName-2arg-nilmod/forName-2arg-nil-bad/
// forName-nn/forName-order: the module casts (nil passes), the name
// casts (nil passes), then the null module NPEs empty — no
// `Module` values exist port-natively, so this always throws,
// exactly as the JVM does for every port-representable input.
#[test]
fn forname_two() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(Class/forName nil \"java.lang.String\")"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"a\" \"b\")"),
        cce_bootstrap("java.lang.String", "java.lang.Module")
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName nil 5)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName nil nil)"),
        "java.lang.NullPointerException | "
    );
    assert_eq!(
        uncaught(&mut eval, "(Class/forName \"m\" 5)"),
        cce_bootstrap("java.lang.String", "java.lang.Module")
    );
}

// ---- static-call analysis ----

// forName-0arg/forName-4arg: known members arity-check at compile
// time, attributed to the head as written.
#[test]
fn static_arity_compile() {
    let mut eval = boot();
    assert_eq!(
        chain(&mut eval, "(Class/forName)"),
        vec![
            "Syntax error compiling Class/forName at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method forName found taking 0 args for class java.lang.Class"
                .to_owned(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(Class/forName \"a\" true nil :x)"),
        vec![
            "Syntax error compiling Class/forName at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method forName found taking 4 args for class java.lang.Class"
                .to_owned(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(java.lang.Class/forName)"),
        vec![
            "Syntax error compiling java.lang.Class/forName at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method forName found taking 0 args for class java.lang.Class"
                .to_owned(),
        ]
    );
}

// var-vs-class/alias-vs-static: namespaces and aliases always win
// over statics — even when the var itself is missing.
#[test]
fn static_var_wins() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'salias) (intern 'salias 'greet (fn [x] (str \"hi \" x))) (alias 'String 'salias))",
    );
    assert_eq!(run(&mut eval, "(String/greet \"bob\")"), "\"hi bob\"");
    assert_eq!(
        compile_err(&mut eval, "(String/valueOf 1)"),
        "java.lang.RuntimeException | No such var: String/valueOf"
    );
}

// static-miss-0/static-miss-args: unknown members on resolved
// classes miss with the oracle text — at runtime (a documented
// phase narrowing; the JVM rejects them at compile time).
#[test]
fn static_unknown_member_runtime() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(String/nope)"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 0 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(String/nope 1 2)"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 2 args for class java.lang.String"
    );
    // Table-gated: real-but-unimplemented members miss the same way
    // until their entries land (later slices).
    assert_eq!(
        uncaught(&mut eval, "(java.lang.String/valueOf 1)"),
        "java.lang.IllegalArgumentException | No matching method valueOf found taking 1 args for class java.lang.String"
    );
}

// The FX4 builtin head is exempt: `createAsIfByAssoc` shapes keep
// their compile-time fallthrough (pinned in `fx4_macroexpand`),
// never the runtime static miss.
#[test]
fn static_fx4_exempt() {
    let mut eval = boot();
    assert_eq!(
        compile_err(
            &mut eval,
            "(clojure.lang.PersistentArrayMap/createAsIfByAssoc)"
        ),
        "java.lang.RuntimeException | No such namespace: clojure.lang.PersistentArrayMap"
    );
}

// static-unknown-fq/removed-ns-call: unresolvable dotted namespace
// parts are `ClassNotFoundException` (unattributed wrapper);
// simple ones keep `No such namespace`.
#[test]
fn static_unknown_fq() {
    let mut eval = boot();
    assert_eq!(
        chain(&mut eval, "(java.no.Klass/foo)"),
        vec![
            "Syntax error compiling at (user:1:1).".to_owned(),
            "java.lang.ClassNotFoundException | java.no.Klass".to_owned(),
        ]
    );
    assert_eq!(
        compile_err(&mut eval, "(foo.bar/baz)"),
        "java.lang.ClassNotFoundException | foo.bar"
    );
    assert_eq!(
        compile_err(&mut eval, "(NopeKlass/foo)"),
        "java.lang.RuntimeException | No such namespace: NopeKlass"
    );
}

// ---- fully-qualified literals ----

// val-fq-String/bare-deftype: dotted bare names evaluate to
// classes; val-fq-missing/val-dotdot/v-traildot/v-dashdot: missing
// ones CNFE; v-dotdash-a/v-leaddot: leading-dot names keep the
// unresolvable error.
#[test]
fn fq_literals() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "java.lang.String"), "java.lang.String");
    assert_eq!(
        run(
            &mut eval,
            "(= java.lang.String (Class/forName \"java.lang.String\"))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(str java.lang.String)"),
        "\"class java.lang.String\""
    );
    assert_eq!(
        compile_err(&mut eval, "java.no.Klass"),
        "java.lang.ClassNotFoundException | java.no.Klass"
    );
    assert_eq!(
        compile_err(&mut eval, "foo.bar"),
        "java.lang.ClassNotFoundException | foo.bar"
    );
    assert_eq!(
        compile_err(&mut eval, "foo."),
        "java.lang.ClassNotFoundException | foo."
    );
    assert_eq!(
        compile_err(&mut eval, "-a.b"),
        "java.lang.ClassNotFoundException | -a.b"
    );
    assert_eq!(
        compile_err(&mut eval, ".-a"),
        "java.lang.RuntimeException | Unable to resolve symbol: .-a in this context"
    );
    assert_eq!(
        compile_err(&mut eval, ".foo"),
        "java.lang.RuntimeException | Unable to resolve symbol: .foo in this context"
    );
    assert_eq!(
        compile_err(&mut eval, "NopeKlass"),
        "java.lang.RuntimeException | Unable to resolve symbol: NopeKlass in this context"
    );
}

// call-fq-noargs: a class in operator position through a
// fully-qualified head takes the ordinary invoke path (the port's
// short cast — the JVM's long form is the baseline invoke
// narrowing).
#[test]
fn fq_operator_invoke() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(java.lang.String)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Class to clojure.lang.IFn"
    );
}

// val-simplequal-missing/removed-ns-val: qualified misses blame the
// namespace part (simple) or CNFE it (dotted). Value-position
// static members (method fns per `v-method`, field reads per
// `v-field`) are a later slice and keep `No such namespace`.
#[test]
fn value_qualified() {
    let mut eval = boot();
    assert_eq!(
        compile_err(&mut eval, "NopeKlass/foo"),
        "java.lang.RuntimeException | No such namespace: NopeKlass"
    );
    assert_eq!(
        compile_err(&mut eval, "java.no.Klass/foo"),
        "java.lang.ClassNotFoundException | java.no.Klass"
    );
    assert_eq!(
        compile_err(&mut eval, "String/valueOf"),
        "java.lang.RuntimeException | No such namespace: String"
    );
    assert_eq!(
        compile_err(&mut eval, "Integer/MAX_VALUE"),
        "java.lang.RuntimeException | No such namespace: Integer"
    );
}

// ---- operator-position imports ----

// invoke-class/op-args: an imported class in operator position is
// the bare `Expecting var` (uncaught, like the JVM); locals shadow
// imports.
#[test]
fn expecting_var() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(String)"),
        "java.lang.RuntimeException | Expecting var, but String is mapped to class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(String 1 2)"),
        "java.lang.RuntimeException | Expecting var, but String is mapped to class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(let [String 1] (String))"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.IFn"
    );
}

// ---- instance position ----

// dotdot-static: statics resolve through instance position on
// `Class` values (`..`, `.`-sugar, `memfn`, every `forName`
// arity) — the runtime compensates for `..` having no
// compile-time static context. Other classes miss against their
// own (runtime) class, and other arities miss at runtime (the
// reflector is runtime).
#[test]
fn instance_forname() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.. Class (forName \"java.lang.String\"))"),
        "java.lang.String"
    );
    assert_eq!(
        run(&mut eval, "(.forName Class \"java.lang.String\")"),
        "java.lang.String"
    );
    assert_eq!(
        run(&mut eval, "((memfn forName nm) Class \"java.lang.String\")"),
        "java.lang.String"
    );
    assert_eq!(
        run(
            &mut eval,
            "(.. Class (forName \"java.lang.String\" true nil))"
        ),
        "java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. Class (forName))"),
        "java.lang.IllegalArgumentException | No matching method forName found taking 0 args for class java.lang.Class"
    );
    // A non-`Class` class value misses against `java.lang.Class`
    // (the target's class, per the runtime miss convention — the
    // JVM rejects this shape at compile time against the
    // represented class instead).
    assert_eq!(
        uncaught(&mut eval, "(.. String (forName \"java.lang.String\"))"),
        "java.lang.IllegalArgumentException | No matching method forName found taking 1 args for class java.lang.Class"
    );
}

// inst-forName-str/inst-forName-1on-str: statics on non-class
// targets miss field-shape at 0 args, method-shape above.
#[test]
fn instance_forname_miss_shapes() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(.. \"s\" (forName))"),
        "java.lang.IllegalArgumentException | No matching field found: forName for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. \"s\" (forName \"x\"))"),
        "java.lang.IllegalArgumentException | No matching method forName found taking 1 args for class java.lang.String"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. 1 (forName))"),
        "java.lang.IllegalArgumentException | No matching field found: forName for class java.lang.Long"
    );
}

// nil-forName-0/nil-forName-1/nil-forName-3/nil-forName-5: known
// static names NPE on nil targets at every arity.
#[test]
fn instance_forname_nil() {
    let mut eval = boot();
    for src in [
        "(.. nil (forName))",
        "(.. nil (forName \"x\"))",
        "(.. nil (forName \"x\" true nil))",
        "(.. nil (forName 1 2 3 4))",
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"target\" is null",
            "{src}"
        );
    }
}

// ---- resolve ----

// resolve-fq/resolve-missing/resolve-static/resolve-simple-missing:
// `resolve`/`ns-resolve` answer classes for qualified names and
// nil for missing ones (never throwing here).
#[test]
fn resolve_classes() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(resolve 'String)"), "java.lang.String");
    assert_eq!(
        run(&mut eval, "(resolve 'java.lang.String)"),
        "java.lang.String"
    );
    assert_eq!(run(&mut eval, "(resolve 'java.no.Klass)"), "nil");
    assert_eq!(run(&mut eval, "(resolve 'String/valueOf)"), "nil");
    assert_eq!(run(&mut eval, "(resolve 'NopeKlass)"), "nil");
    assert_eq!(
        run(&mut eval, "(ns-resolve 'user 'java.lang.Long)"),
        "java.lang.Long"
    );
}

// ---- syntax-quote ----

// rsq-imported/rsq-qual-imported/rsq-bare/rsq-qual-bare: imports
// feed syntax-quote's class map (read-time, so each step reads
// after the previous committed).
#[test]
fn sq_imports() {
    let mut eval = boot();
    run_step(&mut eval, "(import 'java.util.Date)");
    assert_eq!(run_step(&mut eval, "`Date"), "java.util.Date");
    assert_eq!(run_step(&mut eval, "`Date/foo"), "java.util.Date/foo");
    // Default imports qualify too.
    assert_eq!(run_step(&mut eval, "`String"), "java.lang.String");
    let mut bare = boot();
    assert_eq!(run_step(&mut bare, "`TreeMap"), "user/TreeMap");
    assert_eq!(run_step(&mut bare, "`TreeMap/foo"), "TreeMap/foo");
}

// ---- direct __static* guards ----

// static-star-arity/static-star-fqn/static-star-member: the
// synthesized `(__static* fqn member args…)` head guards its own
// shape at runtime (direct calls bypass the compile-time
// arity check, so these fire only here).
#[test]
fn static_star_guards() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(__static*)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/__static*"
    );
    assert_eq!(
        uncaught(&mut eval, "(__static* 1 \"forName\")"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(__static* \"java.lang.Class\" 1)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

// forName-runtime-arity: static syntax arity-checks at compile
// time (oracles `forName-0arg`, `forName-4arg`), so the runtime
// miss fires only through direct `__static*` calls.
#[test]
fn forname_runtime_arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(__static* \"java.lang.Class\" \"forName\")"),
        "java.lang.IllegalArgumentException | No matching method forName found taking 0 args for class java.lang.Class"
    );
}

// static-arg-analyze: an unresolvable static-call argument fails
// analysis inside `static_call`, propagating out of the static
// rewrite (the head resolves; the tail does not).
#[test]
fn static_call_arg_error() {
    let mut eval = boot();
    assert_eq!(
        compile_err(&mut eval, "(Class/forName unbound-xyz)"),
        "java.lang.RuntimeException | Unable to resolve symbol: unbound-xyz in this context"
    );
}

// unmapped-core-macro: an unmapped `clojure.core` macro var is
// not intact, so the qualified head falls past the builtin
// intercept to the ordinary var miss (the namespace still
// exists, so the static rule does not fire).
#[test]
fn unmapped_core_macro_falls_through() {
    let mut eval = boot();
    run_step(&mut eval, "(ns-unmap 'clojure.core 'let)");
    assert_eq!(
        compile_err(&mut eval, "(clojure.core/let [x 1] x)"),
        "java.lang.RuntimeException | No such var: clojure.core/let"
    );
}
