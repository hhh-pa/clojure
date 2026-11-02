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
// static methods (method fns per `v-method`) are still deferred
// and keep `No such namespace`; field reads (per `v-field`)
// resolve — see `static_fields_rows`.
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

// ---- value-position static fields ----

// Value-position static fields (oracle `v-field`): known fields
// read their constants through simple (default-imported) and
// fully-qualified heads alike; unknown members keep the
// deferred `No such namespace`.
#[test]
fn static_fields_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "Math/PI"), "3.141592653589793");
    assert_eq!(run(&mut eval, "Math/E"), "2.718281828459045");
    assert_eq!(run(&mut eval, "Long/MIN_VALUE"), "-9223372036854775808");
    assert_eq!(run(&mut eval, "Long/MAX_VALUE"), "9223372036854775807");
    assert_eq!(run(&mut eval, "Integer/MIN_VALUE"), "-2147483648");
    assert_eq!(run(&mut eval, "Integer/MAX_VALUE"), "2147483647");
    assert_eq!(run(&mut eval, "Double/MIN_VALUE"), "4.9E-324");
    assert_eq!(run(&mut eval, "Double/MAX_VALUE"), "1.7976931348623157E308");
    assert_eq!(run(&mut eval, "Double/POSITIVE_INFINITY"), "##Inf");
    assert_eq!(run(&mut eval, "Double/NEGATIVE_INFINITY"), "##-Inf");
    assert_eq!(run(&mut eval, "Double/NaN"), "##NaN");
    assert_eq!(run(&mut eval, "java.lang.Math/PI"), "3.141592653589793");
    assert_eq!(run(&mut eval, "(str java.util.Locale/US)"), "\"en_US\"");
    assert_eq!(
        run(&mut eval, "(= java.util.Locale/US java.util.Locale/US)"),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(instance? java.util.Locale java.util.Locale/US)"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(class java.util.Locale/US)"),
        "java.util.Locale"
    );
    assert_eq!(
        compile_err(&mut eval, "Math/NOPE"),
        "java.lang.RuntimeException | No such namespace: Math"
    );
    assert_eq!(
        compile_err(&mut eval, "java.util.Locale/UK"),
        "java.lang.RuntimeException | No such namespace: java.util.Locale"
    );
}

// ---- numeric/character/math statics ----

#[test]
fn static_parse_long_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Long/parseLong \"42\")"), "42");
    assert_eq!(run(&mut eval, "(Long/parseLong \"+42\")"), "42");
    assert_eq!(run(&mut eval, "(Long/parseLong \"-0\")"), "0");
    assert_eq!(
        run(&mut eval, "(Long/parseLong \"9223372036854775807\")"),
        "9223372036854775807"
    );
    assert_eq!(
        run(&mut eval, "(Long/parseLong \"-9223372036854775808\")"),
        "-9223372036854775808"
    );
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16)"), "255");
    assert_eq!(run(&mut eval, "(Long/parseLong \"-ff\" 16)"), "-255");
    assert_eq!(
        run(&mut eval, "(Long/parseLong \"\\u0661\\u0662\" 10)"),
        "12"
    );
    assert_eq!(
        run(&mut eval, "(Long/parseLong \"\\u0661\\u0662\" 16)"),
        "18"
    );
    // The radix narrows through the reflector int rule.
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16.0)"), "255");
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16.9)"), "255");
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16N)"), "255");
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16/1)"), "255");
    assert_eq!(run(&mut eval, "(Long/parseLong \"ff\" 16.0M)"), "255");
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"abc\")"),
        "java.lang.NumberFormatException | For input string: \"abc\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"\")"),
        "java.lang.NumberFormatException | For input string: \"\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"9223372036854775808\")"),
        "java.lang.NumberFormatException | For input string: \"9223372036854775808\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong nil)"),
        "java.lang.NumberFormatException | Cannot parse null string"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong 42)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"g\" 16)"),
        "java.lang.NumberFormatException | For input string: \"g\" under radix 16"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"1\" 1)"),
        "java.lang.NumberFormatException | radix 1 less than Character.MIN_RADIX"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"1\" 37)"),
        "java.lang.NumberFormatException | radix 37 greater than Character.MAX_RADIX"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"ff\" \"16\")"),
        cce_bootstrap("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"ff\" nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Character.charValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"ff\" \\a)"),
        cce_bootstrap("java.lang.Character", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"ff\" 1099511627776)"),
        "java.lang.ArithmeticException | integer overflow"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/parseLong \"ff\" 1180591620717411303424N)"),
        "java.lang.IllegalArgumentException | Value out of range for long: 1180591620717411303424"
    );
}

#[test]
fn static_parse_int_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Integer/parseInt \"42\")"), "42");
    assert_eq!(run(&mut eval, "(Integer/parseInt \"-42\")"), "-42");
    assert_eq!(run(&mut eval, "(Integer/parseInt \"ff\" 16)"), "255");
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt nil)"),
        "java.lang.NumberFormatException | Cannot parse null string"
    );
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt 4.0)"),
        cce_bootstrap("java.lang.Double", "java.lang.String")
    );
    // Out-of-`i32` is the same `For input string` NFE, never
    // `integer overflow`.
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt \"2147483648\")"),
        "java.lang.NumberFormatException | For input string: \"2147483648\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt \"1ffffffff\" 16)"),
        "java.lang.NumberFormatException | For input string: \"1ffffffff\" under radix 16"
    );
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt \"xx\")"),
        "java.lang.NumberFormatException | For input string: \"xx\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Integer/parseInt \"1\" 0)"),
        "java.lang.NumberFormatException | radix 0 less than Character.MIN_RADIX"
    );
}

#[test]
fn static_parse_double_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Double/parseDouble \"1.5\")"), "1.5");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"NaN\")"), "##NaN");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"Infinity\")"), "##Inf");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"  1.5  \")"), "1.5");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"0x1p3\")"), "8.0");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"1e3\")"), "1000.0");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"1e999\")"), "##Inf");
    assert_eq!(run(&mut eval, "(Double/parseDouble \"1.5f\")"), "1.5");
    assert_eq!(
        uncaught(&mut eval, "(Double/parseDouble \"abc\")"),
        "java.lang.NumberFormatException | For input string: \"abc\""
    );
    assert_eq!(
        uncaught(&mut eval, "(Double/parseDouble nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"in\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Double/parseDouble \"\")"),
        "java.lang.NumberFormatException | empty String"
    );
    assert_eq!(
        uncaught(&mut eval, "(Double/parseDouble 42)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

#[test]
fn static_remainder_unsigned_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Long/remainderUnsigned -1 3)"), "0");
    assert_eq!(run(&mut eval, "(Long/remainderUnsigned 7 -3)"), "7");
    assert_eq!(run(&mut eval, "(Long/remainderUnsigned 7.5 2)"), "1");
    assert_eq!(run(&mut eval, "(Long/remainderUnsigned 7/2 2)"), "1");
    assert_eq!(
        uncaught(&mut eval, "(Long/remainderUnsigned 1 0)"),
        "java.lang.ArithmeticException | / by zero"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/remainderUnsigned nil 2)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/remainderUnsigned \"7\" 2)"),
        cce_bootstrap("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(Long/remainderUnsigned \\a 2)"),
        cce_bootstrap("java.lang.Character", "java.lang.Number")
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(Long/remainderUnsigned 1180591620717411303424N 3)"
        ),
        "java.lang.IllegalArgumentException | Value out of range for long: 1180591620717411303424"
    );
}

#[test]
fn static_char_digit_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Character/digit \\a 36)"), "10");
    assert_eq!(run(&mut eval, "(Character/digit \\z 36)"), "35");
    assert_eq!(run(&mut eval, "(Character/digit \\A 36)"), "10");
    assert_eq!(run(&mut eval, "(Character/digit \\F 16)"), "15");
    assert_eq!(run(&mut eval, "(Character/digit \\0 10)"), "0");
    assert_eq!(run(&mut eval, "(Character/digit \\g 16)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit \\9 8)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit \\space 10)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit \\a 1)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit 97 10)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit 2000000 16)"), "-1");
    assert_eq!(run(&mut eval, "(Character/digit \\uFF11 10)"), "1");
    assert_eq!(run(&mut eval, "(Character/digit \\uFF21 16)"), "10");
    assert_eq!(run(&mut eval, "(Character/digit \\uFF41 16)"), "10");
    assert_eq!(run(&mut eval, "(Character/digit \\u0661 10)"), "1");
    assert_eq!(run(&mut eval, "(Character/digit \\u0661 16)"), "1");
    assert_eq!(
        uncaught(&mut eval, "(Character/digit \"a\" 36)"),
        "java.lang.IllegalArgumentException | No matching method digit found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/digit nil 10)"),
        "java.lang.IllegalArgumentException | No matching method digit found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/digit 1.5 10)"),
        "java.lang.IllegalArgumentException | No matching method digit found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/digit 97N 10)"),
        "java.lang.IllegalArgumentException | No matching method digit found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/digit \\a nil)"),
        "java.lang.IllegalArgumentException | No matching method digit found taking 2 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/digit 1099511627776 16)"),
        "java.lang.ArithmeticException | integer overflow"
    );
}

#[test]
fn static_is_whitespace_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\space)"), "true");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\tab)"), "true");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\newline)"), "true");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\a)"), "false");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\u001C)"), "true");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\u2028)"), "true");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\u00A0)"), "false");
    assert_eq!(run(&mut eval, "(Character/isWhitespace \\u2007)"), "false");
    assert_eq!(run(&mut eval, "(Character/isWhitespace 32)"), "true");
    assert_eq!(
        uncaught(&mut eval, "(Character/isWhitespace \" \")"),
        "java.lang.IllegalArgumentException | No matching method isWhitespace found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Character/isWhitespace nil)"),
        "java.lang.IllegalArgumentException | No matching method isWhitespace found taking 1 args"
    );
}

// static-char-lod/lc-*: the char rule over chars, the int
// code-point rule over longs (unrepresentable code points read
// false), anything else the no-overload miss (oracles
// `/tmp/charpred.clj`, `/tmp/charpred3.clj`, `/tmp/charpred4.clj`).
#[test]
fn static_char_pred_rows() {
    let mut eval = boot();
    for (src, want) in [
        ("(Character/isLetterOrDigit \\a)", "true"),
        ("(Character/isLetterOrDigit \\Z)", "true"),
        ("(Character/isLetterOrDigit \\5)", "true"),
        ("(Character/isLetterOrDigit \\é)", "true"),
        ("(Character/isLetterOrDigit \\α)", "true"),
        ("(Character/isLetterOrDigit \\中)", "true"),
        ("(Character/isLetterOrDigit \\_)", "false"),
        ("(Character/isLetterOrDigit \\space)", "false"),
        ("(Character/isLetterOrDigit \\ß)", "true"),
        ("(Character/isLetterOrDigit 97)", "true"),
        ("(Character/isLetterOrDigit 65)", "true"),
        ("(Character/isLetterOrDigit 120792)", "true"),
        ("(Character/isLetterOrDigit -1)", "false"),
        ("(Character/isLetterOrDigit 1114112)", "false"),
        ("(Character/isLowerCase \\a)", "true"),
        ("(Character/isLowerCase \\Z)", "false"),
        ("(Character/isLowerCase \\5)", "false"),
        ("(Character/isLowerCase \\é)", "true"),
        ("(Character/isLowerCase \\α)", "true"),
        ("(Character/isLowerCase \\中)", "false"),
        ("(Character/isLowerCase \\ß)", "true"),
        ("(Character/isLowerCase \\A)", "false"),
        ("(Character/isLowerCase 223)", "true"),
        ("(Character/isLowerCase 120792)", "false"),
        ("(Character/isLowerCase -65)", "false"),
        ("(Character/isLowerCase 0)", "false"),
    ] {
        assert_eq!(run(&mut eval, src), want, "{src}");
    }
    for (src, member) in [
        ("(Character/isLetterOrDigit nil)", "isLetterOrDigit"),
        ("(Character/isLetterOrDigit \"a\")", "isLetterOrDigit"),
        ("(Character/isLetterOrDigit 1.5)", "isLetterOrDigit"),
        ("(Character/isLowerCase 97N)", "isLowerCase"),
        ("(Character/isLowerCase nil)", "isLowerCase"),
    ] {
        assert_eq!(
            uncaught(&mut eval, src),
            format!(
                "java.lang.IllegalArgumentException | No matching method {member} found taking 1 args"
            ),
            "{src}"
        );
    }
}

#[test]
fn static_math_rows() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(Math/abs -5)"), "5");
    assert_eq!(
        run(&mut eval, "(Math/abs -9223372036854775808)"),
        "-9223372036854775808"
    );
    assert_eq!(run(&mut eval, "(Math/abs -5.5)"), "5.5");
    assert_eq!(run(&mut eval, "(Math/abs -0.0)"), "0.0");
    assert_eq!(run(&mut eval, "(Math/sqrt 4)"), "2.0");
    assert_eq!(run(&mut eval, "(Math/sqrt 4N)"), "2.0");
    assert_eq!(run(&mut eval, "(Math/sqrt 1/2)"), "0.7071067811865476");
    assert_eq!(run(&mut eval, "(Math/sqrt -1.0)"), "##NaN");
    assert_eq!(run(&mut eval, "(Math/pow 2 10)"), "1024.0");
    assert_eq!(run(&mut eval, "(Math/pow 0.0 0.0)"), "1.0");
    assert_eq!(run(&mut eval, "(Math/ceil 5)"), "5.0");
    assert_eq!(run(&mut eval, "(Math/ceil -1.5)"), "-1.0");
    assert_eq!(run(&mut eval, "(Math/log 1.0)"), "0.0");
    assert_eq!(run(&mut eval, "(Math/exp 1.0)"), "2.718281828459045");
    assert_eq!(run(&mut eval, "(Math/sin 0.0)"), "0.0");
    assert_eq!(run(&mut eval, "(Math/cos 0.0)"), "1.0");
    assert_eq!(run(&mut eval, "(Math/tanh 1.0)"), "0.7615941559557649");
    assert_eq!(run(&mut eval, "(Math/hypot 3 4)"), "5.0");
    assert_eq!(
        uncaught(&mut eval, "(Math/abs 5N)"),
        "java.lang.IllegalArgumentException | No matching method abs found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/abs -1/2)"),
        "java.lang.IllegalArgumentException | No matching method abs found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/abs nil)"),
        "java.lang.IllegalArgumentException | No matching method abs found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/abs \\a)"),
        "java.lang.IllegalArgumentException | No matching method abs found taking 1 args"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/sqrt nil)"),
        "java.lang.NullPointerException | Cannot invoke \"java.lang.Number.doubleValue()\" because \"x\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/sqrt \"4\")"),
        cce_bootstrap("java.lang.String", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/sqrt \\a)"),
        cce_bootstrap("java.lang.Character", "java.lang.Number")
    );
    assert_eq!(
        uncaught(&mut eval, "(Math/nope 1)"),
        "java.lang.IllegalArgumentException | No matching method nope found taking 1 args for class java.lang.Math"
    );
}

#[test]
fn static_quote_replacement_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(java.util.regex.Matcher/quoteReplacement \"a$b\\\\c\")"
        ),
        "\"a\\\\$b\\\\\\\\c\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(java.util.regex.Matcher/quoteReplacement \"abc\")"
        ),
        "\"abc\""
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.regex.Matcher/quoteReplacement nil)"),
        "java.lang.NullPointerException | Cannot invoke \"String.indexOf(int)\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(java.util.regex.Matcher/quoteReplacement 42)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

#[test]
fn static_string_format_rows() {
    let mut eval = boot();
    assert_eq!(
        run(
            &mut eval,
            "(String/format \"%.6f\" (to-array [(/ 1.0 3.0)]))"
        ),
        "\"0.333333\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(String/format java.util.Locale/US \"%.6f\" (to-array [2.5]))"
        ),
        "\"2.500000\""
    );
    assert_eq!(
        run(&mut eval, "(String/format \"%s %d\" (to-array [\"a\" 1]))"),
        "\"a 1\""
    );
    assert_eq!(run(&mut eval, "(String/format \"%s\" nil)"), "\"null\"");
    assert_eq!(
        run(&mut eval, "(String/format nil \"%s\" (to-array [\"a\"]))"),
        "\"a\""
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format \"%s\" \"a\")"),
        cce_bootstrap("java.lang.String", "[Ljava.lang.Object;")
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format \"%s\" (int-array [1]))"),
        cce_bootstrap("[I", "[Ljava.lang.Object;")
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format 42 (to-array []))"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format nil (to-array []))"),
        "java.lang.NullPointerException | Cannot invoke \"String.length()\" because \"s\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format \"%s\" \"a\" \"b\")"),
        cce_bootstrap("java.lang.String", "java.util.Locale")
    );
    assert_eq!(
        uncaught(&mut eval, "(String/format \"%d\" (to-array [\"a\"]))"),
        "java.util.IllegalFormatConversionException | d != java.lang.String"
    );
}

// The runtime arity miss of every new static (direct `__static*`
// calls — static syntax arity-checks at compile time).
#[test]
fn static_host_runtime_arity_rows() {
    let mut eval = boot();
    for (class, member) in [
        ("java.lang.Long", "parseLong"),
        ("java.lang.Long", "remainderUnsigned"),
        ("java.lang.Integer", "parseInt"),
        ("java.lang.Double", "parseDouble"),
        ("java.lang.Character", "digit"),
        ("java.lang.Character", "isWhitespace"),
        ("java.lang.Math", "abs"),
        ("java.lang.Math", "sqrt"),
        ("java.lang.Math", "pow"),
        ("java.util.regex.Matcher", "quoteReplacement"),
        ("java.lang.String", "format"),
    ] {
        assert_eq!(
            uncaught(&mut eval, &format!("(__static* \"{class}\" \"{member}\")")),
            format!(
                "java.lang.IllegalArgumentException | No matching method {member} found taking 0 args for class {class}"
            ),
            "{class}/{member}"
        );
    }
}

// Compile-time arity checks for the new statics (one per shape).
#[test]
fn static_host_arity_compile_rows() {
    let mut eval = boot();
    assert_eq!(
        chain(&mut eval, "(Long/parseLong)"),
        vec![
            "Syntax error compiling Long/parseLong at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method parseLong found taking 0 args for class java.lang.Long"
                .to_owned(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(String/format \"%s\")"),
        vec![
            "Syntax error compiling String/format at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method format found taking 1 args for class java.lang.String"
                .to_owned(),
        ]
    );
    assert_eq!(
        chain(&mut eval, "(Math/pow 1.0)"),
        vec![
            "Syntax error compiling Math/pow at (user:1:1).".to_owned(),
            "java.lang.IllegalArgumentException | No matching method pow found taking 1 args for class java.lang.Math"
                .to_owned(),
        ]
    );
}
