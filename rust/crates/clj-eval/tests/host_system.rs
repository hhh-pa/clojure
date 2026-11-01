// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Issue #120 slice 3: the `System` static surface — `getProperty`
//! over the well-known table (unknown keys oust precisely),
//! `lineSeparator`, and `identityHashCode` — through static syntax,
//! instance position on class values, and direct `__static*`
//! calls. Oracle: upstream source text, read, never run — JDK 27
//! `java.base/java/lang/System.java` (`checkKey`,
//! `lineSeparator`, `identityHashCode`, `getProperty`) and
//! `jdk/internal/util/SystemProps.java` (the standard keys), plus
//! the upstream consumers in `clojure-1.12.0.jar` (`core.clj`
//! `system-newline`, `main.clj` two-arg defaults,
//! `core_print.clj` / `pprint/dispatch.clj` identity slots).
//! Deferred (pinned only as misses where noted): `setProperty` /
//! `clearProperty` / `getProperties`, `getenv`, `exit`,
//! `currentTimeMillis` / `nanoTime`, and `arraycopy`.

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

/// The pinned `os.name` per platform (JVM spellings; `Windows`
/// reads the family — a documented narrowing).
fn expected_os_name() -> &'static str {
    if cfg!(target_os = "linux") {
        "Linux"
    } else if cfg!(target_os = "macos") {
        "Mac OS X"
    } else if cfg!(target_os = "windows") {
        "Windows"
    } else {
        std::env::consts::OS
    }
}

/// The pinned `os.arch` per platform (`x86_64` takes the JVM
/// spelling `amd64`).
fn expected_os_arch() -> &'static str {
    if cfg!(target_arch = "x86_64") {
        "amd64"
    } else if cfg!(target_arch = "x86") {
        "x86"
    } else {
        std::env::consts::ARCH
    }
}

// ---- well-known table ----

// The six well-known keys answer their pinned constants — through
// simple (default-imported) and fully-qualified heads alike — and
// a hit ignores the default.
#[test]
fn getproperty_table() {
    let mut eval = boot();
    let nl = if cfg!(windows) { "\\r\\n" } else { "\\n" };
    assert_eq!(
        run(&mut eval, "(System/getProperty \"line.separator\")"),
        format!("\"{nl}\"")
    );
    let fs = if cfg!(windows) { "\"\\\\\"" } else { "\"/\"" };
    assert_eq!(
        run(&mut eval, "(System/getProperty \"file.separator\")"),
        fs
    );
    let ps = if cfg!(windows) { "\";\"" } else { "\":\"" };
    assert_eq!(
        run(&mut eval, "(System/getProperty \"path.separator\")"),
        ps
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"file.encoding\")"),
        "\"UTF-8\""
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"os.name\")"),
        format!("\"{}\"", expected_os_name())
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"os.arch\")"),
        format!("\"{}\"", expected_os_arch())
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"os.name\" \"dflt\")"),
        format!("\"{}\"", expected_os_name())
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"file.encoding\" nil)"),
        "\"UTF-8\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(java.lang.System/getProperty \"file.encoding\")"
        ),
        "\"UTF-8\""
    );
}

// Unknown keys oust precisely — nil (1-arg) or the default
// (2-arg), like the JVM's unset keys. Upstream's own `-D` keys
// arrive unset, feeding upstream's defaults (`main.clj`
// `clojure.compile.path` / `clojure.main.report`, `basis`
// `clojure.basis`); dynamic and JVM-only keys are documented
// narrowings.
#[test]
fn getproperty_ousts() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(System/getProperty \"no.such.prop\")"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"no.such.prop\" \"dflt\")"),
        "\"dflt\""
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"no.such.prop\" nil)"),
        "nil"
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"clojure.compile.path\")"),
        "nil"
    );
    assert_eq!(
        run(
            &mut eval,
            "(System/getProperty \"clojure.compile.path\" \"classes\")"
        ),
        "\"classes\""
    );
    assert_eq!(
        run(
            &mut eval,
            "(System/getProperty \"clojure.main.report\" \"file\")"
        ),
        "\"file\""
    );
    assert_eq!(
        run(&mut eval, "(System/getProperty \"clojure.basis\")"),
        "nil"
    );
    assert_eq!(run(&mut eval, "(System/getProperty \"user.dir\")"), "nil");
    assert_eq!(
        run(&mut eval, "(System/getProperty \"java.version\")"),
        "nil"
    );
    assert_eq!(run(&mut eval, "(System/getProperty \"os.version\")"), "nil");
}

// JDK `checkKey` verbatim, both arities; casts run left-to-right
// (the slice-2 `forName` order), so a bad default CCEs before a
// nil key NPEs.
#[test]
fn getproperty_key_errors() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty nil)"),
        "java.lang.NullPointerException | key can't be null"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty \"\")"),
        "java.lang.IllegalArgumentException | key can't be empty"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty nil \"d\")"),
        "java.lang.NullPointerException | key can't be null"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty \"\" \"d\")"),
        "java.lang.IllegalArgumentException | key can't be empty"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty 5)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty 5 nil)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty \"os.name\" 5)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getProperty nil 5)"),
        cce_bootstrap("java.lang.Long", "java.lang.String")
    );
}

// ---- lineSeparator ----

// `lineSeparator` answers the `line.separator` property (JDK:
// `return lineSeparator` — the two agree by construction).
#[test]
fn lineseparator() {
    let mut eval = boot();
    let nl = if cfg!(windows) { "\\r\\n" } else { "\\n" };
    assert_eq!(
        run(&mut eval, "(System/lineSeparator)"),
        format!("\"{nl}\"")
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (System/lineSeparator) (System/getProperty \"line.separator\"))"
        ),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(java.lang.System/lineSeparator)"),
        format!("\"{nl}\"")
    );
}

// ---- static-call analysis ----

// Known members arity-check at compile time, attributed to the
// head as written (mirroring slice 1's `static_arity_compile`).
#[test]
fn system_arity_compile() {
    let mut eval = boot();
    for (src, head, method, argc) in [
        (
            "(System/getProperty)",
            "System/getProperty",
            "getProperty",
            0,
        ),
        (
            "(System/getProperty \"a\" \"b\" \"c\")",
            "System/getProperty",
            "getProperty",
            3,
        ),
        (
            "(System/lineSeparator \"x\")",
            "System/lineSeparator",
            "lineSeparator",
            1,
        ),
        (
            "(System/identityHashCode)",
            "System/identityHashCode",
            "identityHashCode",
            0,
        ),
        (
            "(System/identityHashCode 1 2)",
            "System/identityHashCode",
            "identityHashCode",
            2,
        ),
        (
            "(java.lang.System/getProperty)",
            "java.lang.System/getProperty",
            "getProperty",
            0,
        ),
    ] {
        assert_eq!(
            chain(&mut eval, src),
            vec![
                format!("Syntax error compiling {head} at (user:1:1)."),
                format!(
                    "java.lang.IllegalArgumentException | No matching method {method} found taking {argc} args for class java.lang.System"
                ),
            ],
            "{src}"
        );
    }
}

// ---- identityHashCode ----

// nil is 0 (JDK javadoc); small ints mix to themselves; cached
// kinds hash content. Stable per allocation, distinct per
// allocation.
#[test]
fn identity_hashcode_pins() {
    let mut eval = boot();
    assert_eq!(run(&mut eval, "(System/identityHashCode nil)"), "0");
    assert_eq!(run(&mut eval, "(System/identityHashCode 5)"), "5");
    assert_eq!(run(&mut eval, "(System/identityHashCode true)"), "1231");
    assert_eq!(run(&mut eval, "(System/identityHashCode \\a)"), "97");
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom 1)] (= (System/identityHashCode a) (System/identityHashCode a)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [s \"lit\"] (= (System/identityHashCode s) (System/identityHashCode s)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(= (System/identityHashCode :a) (System/identityHashCode :a))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (atom 1) b (atom 1)] (not= (System/identityHashCode a) (System/identityHashCode b)))"
        ),
        "true"
    );
    assert_eq!(
        run(
            &mut eval,
            "(let [a (Object.) b (Object.)] (not= (System/identityHashCode a) (System/identityHashCode b)))"
        ),
        "true"
    );
}

// One row per `Value` variant (`Uri` has no surface constructor —
// covered by the `identity_hash_unreachable_uri` unit test):
// every kind answers an integer.
#[test]
fn identity_hashcode_every_kind() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (deftype IHCT1 [x]) (defmulti ihc_m1 :k) (def ihc_v1 1) (def ihc_u1) nil)",
    );
    for src in [
        "nil",
        "true",
        "5",
        "1.5",
        "\\a",
        "\"s\"",
        "'s",
        ":k",
        "'(1)",
        "[1]",
        "{:a 1}",
        "#{1}",
        "(find {:a 1} :a)",
        "#\"a\"",
        "(parse-uuid \"123e4567-e89b-12d3-a456-426614174000\")",
        "#inst \"2024-01-01\"",
        "(tagged-literal 'x 1)",
        "(read-string {:read-cond :preserve} \"#?(1)\")",
        "#'ihc_v1",
        "ihc_u1",
        "(ex-info \"x\" {})",
        "(transient [1])",
        "(atom 1)",
        "(delay 1)",
        "(future 1)",
        "(promise)",
        "(agent 1)",
        "(volatile! 1)",
        "(ref 1)",
        "(Object.)",
        "(IHCT1. 1)",
        "ihc_m1",
        "String",
        "(int-array [1])",
        "(vector-of :int 1)",
        "(executor)",
        "5N",
        "(biginteger 5)",
        "5M",
        "(/ 1 2)",
    ] {
        assert_eq!(
            run(
                &mut eval,
                &format!("(integer? (System/identityHashCode {src}))")
            ),
            "true",
            "{src}"
        );
    }
}

// ---- instance position ----

// Statics resolve through instance position on `System` class
// values (`..`, `.`-sugar, `memfn` — mirroring slice 1's
// `dotdot-static`). Other classes miss against `java.lang.Class`
// (the runtime miss convention), and other arities miss at
// runtime (the reflector is runtime).
#[test]
fn instance_system_statics() {
    let mut eval = boot();
    assert_eq!(
        run(&mut eval, "(.. System (getProperty \"file.encoding\"))"),
        "\"UTF-8\""
    );
    assert_eq!(
        run(&mut eval, "(.getProperty System \"file.encoding\")"),
        "\"UTF-8\""
    );
    assert_eq!(
        run(
            &mut eval,
            "((memfn getProperty k) System \"file.encoding\")"
        ),
        "\"UTF-8\""
    );
    assert_eq!(
        run(&mut eval, "(.. System (getProperty \"nope\" \"d\"))"),
        "\"d\""
    );
    let nl = if cfg!(windows) { "\\r\\n" } else { "\\n" };
    assert_eq!(
        run(&mut eval, "(.. System (lineSeparator))"),
        format!("\"{nl}\"")
    );
    assert_eq!(run(&mut eval, "(.. System (identityHashCode nil))"), "0");
    assert_eq!(
        uncaught(&mut eval, "(.. Class (getProperty \"os.name\"))"),
        "java.lang.IllegalArgumentException | No matching method getProperty found taking 1 args for class java.lang.Class"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. System (getProperty))"),
        "java.lang.IllegalArgumentException | No matching method getProperty found taking 0 args for class java.lang.Class"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. System (lineSeparator 1))"),
        "java.lang.IllegalArgumentException | No matching method lineSeparator found taking 1 args for class java.lang.Class"
    );
    assert_eq!(
        uncaught(&mut eval, "(.. nil (getProperty \"os.name\"))"),
        "java.lang.NullPointerException | Cannot invoke \"Object.getClass()\" because \"target\" is null"
    );
}

// Static syntax arity-checks at compile time, so the runtime miss
// fires only through direct `__static*` calls (mirroring slice
// 1's `forname_runtime_arity`).
#[test]
fn system_runtime_arity() {
    let mut eval = boot();
    assert_eq!(
        uncaught(
            &mut eval,
            "(__static* \"java.lang.System\" \"getProperty\")"
        ),
        "java.lang.IllegalArgumentException | No matching method getProperty found taking 0 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(__static* \"java.lang.System\" \"getProperty\" \"a\" \"b\" \"c\")"
        ),
        "java.lang.IllegalArgumentException | No matching method getProperty found taking 3 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(__static* \"java.lang.System\" \"lineSeparator\" 1)"
        ),
        "java.lang.IllegalArgumentException | No matching method lineSeparator found taking 1 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(__static* \"java.lang.System\" \"identityHashCode\")"
        ),
        "java.lang.IllegalArgumentException | No matching method identityHashCode found taking 0 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(
            &mut eval,
            "(__static* \"java.lang.System\" \"identityHashCode\" 1 2)"
        ),
        "java.lang.IllegalArgumentException | No matching method identityHashCode found taking 2 args for class java.lang.System"
    );
}

// Real-but-unimplemented `System` members keep the precise miss
// (later slices; `exit` must never exit).
#[test]
fn system_unimplemented_miss() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(System/exit 0)"),
        "java.lang.IllegalArgumentException | No matching method exit found taking 1 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/getenv \"HOME\")"),
        "java.lang.IllegalArgumentException | No matching method getenv found taking 1 args for class java.lang.System"
    );
    assert_eq!(
        uncaught(&mut eval, "(System/currentTimeMillis)"),
        "java.lang.IllegalArgumentException | No matching method currentTimeMillis found taking 0 args for class java.lang.System"
    );
}
