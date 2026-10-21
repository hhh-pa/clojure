// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5A2 namespace vars: probe35 replay against the booted core.
//! Namespace ids are deterministic per boot (`clojure.core` is 1,
//! `user` is 2), so handle prints pin the full `#object` shape.

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

/// The `Class | message` of an uncaught throw (natives surface
/// through `SourceError::Uncaught`).
fn uncaught_chain(eval: &mut Evaluator, src: &str) -> Vec<String> {
    let Err(SourceError::Uncaught(t)) = eval.eval_source(src) else {
        panic!("expected an uncaught throw for {src}");
    };
    let mut out = Vec::new();
    let mut cur = Some(&t);
    while let Some(th) = cur {
        out.push(format!(
            "{} | {}",
            th.class.jvm_name(),
            th.message.as_deref().unwrap_or("")
        ));
        cur = th.cause.as_deref();
    }
    out
}

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

#[test]
fn boot_shape() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "*ns*"),
        "#object[clojure.lang.Namespace 0x2 \"user\"]"
    );
    assert_eq!(run_last(&mut eval, "*file*"), "\"NO_SOURCE_PATH\"");
    // `*loaded-libs*` is private: read it through `var`
    // (unchecked, like the JVM) plus `var-get`.
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*loaded-libs*))"),
        "#{}"
    );
    assert_eq!(run_last(&mut eval, "*load-path*"), "[]");
    assert_eq!(
        run_last(&mut eval, "(var-get (var clojure.core/*pending-paths*))"),
        "()"
    );
    assert_eq!(
        run_last(
            &mut eval,
            "(var-get (var clojure.core/*loading-verbosely*))"
        ),
        "false"
    );
    // Privates stay out of the user refers; publics resolve.
    assert_eq!(
        run_last(&mut eval, "(resolve 'create-ns)"),
        "#'clojure.core/create-ns"
    );
    assert_eq!(run_last(&mut eval, "(resolve '*loaded-libs*)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(resolve 'clojure.core/*loaded-libs*)"),
        "#'clojure.core/*loaded-libs*"
    );
    assert_eq!(
        run_last(&mut eval, "(resolve '*ns*)"),
        "#'clojure.core/*ns*"
    );
}

#[test]
fn create_and_find_ns() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(create-ns 'cns)"),
        "#object[clojure.lang.Namespace 0x3 \"cns\"]"
    );
    // Existing returns the identical handle (same id).
    assert_eq!(
        run_last(&mut eval, "(create-ns 'cns)"),
        "#object[clojure.lang.Namespace 0x3 \"cns\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(find-ns 'cns)"),
        "#object[clojure.lang.Namespace 0x3 \"cns\"]"
    );
    assert_eq!(run_last(&mut eval, "(find-ns 'missing)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(find-ns nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.hashCode()\" because \"key\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(create-ns nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.hashCode()\" because \"key\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(create-ns 5)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-ns :kw)"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Keyword to clojure.lang.Symbol"
    );
    assert_eq!(
        uncaught(&mut eval, "(find-ns (the-ns 'user))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Namespace to clojure.lang.Symbol"
    );
}

#[test]
fn in_ns_switches_current() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(in-ns 'inns)"),
        "#object[clojure.lang.Namespace 0x3 \"inns\"]"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/ns-name clojure.core/*ns*)"),
        "inns"
    );
    // Fresh namespaces refer nothing (probe35 create-vs-in-ns-refers).
    run(&mut eval, "(clojure.core/in-ns 'user)");
    assert_eq!(run_last(&mut eval, "(ns-refers 'inns)"), "{}");
    assert_eq!(
        uncaught(&mut eval, "(in-ns nil)"),
        "java.lang.NullPointerException | Cannot invoke \"Object.hashCode()\" because \"key\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(in-ns (the-ns 'user))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Namespace to clojure.lang.Symbol"
    );
}

#[test]
fn ns_name_and_the_ns() {
    let mut eval = boot();
    run(&mut eval, "(create-ns 'tn)");
    assert_eq!(run_last(&mut eval, "(ns-name (the-ns 'tn))"), "tn");
    assert_eq!(run_last(&mut eval, "(ns-name 'tn)"), "tn");
    assert_eq!(
        run_last(&mut eval, "(the-ns 'tn)"),
        "#object[clojure.lang.Namespace 0x3 \"tn\"]"
    );
    // Namespaces pass through identically (same id).
    assert_eq!(
        run_last(&mut eval, "(the-ns (the-ns 'tn))"),
        "#object[clojure.lang.Namespace 0x3 \"tn\"]"
    );
    assert_eq!(
        uncaught(&mut eval, "(the-ns 'missing)"),
        "java.lang.Exception | No namespace: missing found"
    );
    assert_eq!(
        uncaught(&mut eval, "(ns-name 'missing)"),
        "java.lang.Exception | No namespace: missing found"
    );
    assert_eq!(
        uncaught(&mut eval, "(the-ns \"tn\")"),
        "java.lang.ClassCastException | Cannot cast java.lang.String to clojure.lang.Symbol"
    );
}

#[test]
fn remove_ns_semantics() {
    let mut eval = boot();
    run(&mut eval, "(create-ns 'gone)");
    assert_eq!(
        run_last(&mut eval, "(remove-ns 'gone)"),
        "#object[clojure.lang.Namespace 0x3 \"gone\"]"
    );
    assert_eq!(run_last(&mut eval, "(find-ns 'gone)"), "nil");
    assert_eq!(run_last(&mut eval, "(remove-ns 'gone)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(remove-ns nil)"),
        "java.lang.NullPointerException | Cannot invoke \"clojure.lang.Symbol.equals(Object)\" because \"name\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(remove-ns 'clojure.core)"),
        "java.lang.IllegalArgumentException | Cannot remove clojure namespace"
    );
    // Removing the current namespace dangles `*ns*` (the frame keeps
    // the stale handle).
    let mut eval = boot();
    run(&mut eval, "(in-ns 'cur)");
    run(&mut eval, "(clojure.core/remove-ns 'cur)");
    assert_eq!(
        run_last(&mut eval, "(clojure.core/ns-name clojure.core/*ns*)"),
        "cur"
    );
    assert_eq!(run_last(&mut eval, "(clojure.core/find-ns 'cur)"), "nil");
}

#[test]
fn stale_handles_read_tombstones() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 'st) (intern 'st 'x 1))");
    run(&mut eval, "(def sth (the-ns 'st))");
    run(&mut eval, "(remove-ns 'st)");
    // `the-ns` returns the handle itself (JVM identity).
    assert_eq!(
        run_last(&mut eval, "(the-ns sth)"),
        "#object[clojure.lang.Namespace 0x3 \"st\"]"
    );
    assert_eq!(run_last(&mut eval, "(ns-name sth)"), "st");
    // The frozen mappings survive.
    assert_eq!(run_last(&mut eval, "(ns-map sth)"), "{x #'st/x}");
    assert_eq!(run_last(&mut eval, "(ns-interns sth)"), "{x #'st/x}");
    // And a captured var still derefs through the tombstone when it
    // holds a callable.
    let mut eval = boot();
    run(&mut eval, "(create-ns 'st2)");
    eval.install_native(
        "st2",
        "plus",
        std::sync::Arc::new(|_, args| match args {
            [Value::Int(a), Value::Int(b)] => Ok(Value::Int(a + b)),
            _ => Err(clj_eval::NativeError::arity("st2/plus", args.len())),
        }),
    );
    run(&mut eval, "(def vc (ns-resolve 'st2 'plus))");
    run(&mut eval, "(remove-ns 'st2)");
    assert_eq!(run_last(&mut eval, "(vc 10 20)"), "30");
}

#[test]
fn stale_handles_pin_tombstone_after_recreate() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 't) (intern 't 'x 1))");
    run(&mut eval, "(def th (the-ns 't))");
    run(&mut eval, "(remove-ns 't)");
    run(&mut eval, "(create-ns 't)");
    // Reads through the stale handle see the frozen mappings, not the new ns.
    assert_eq!(run_last(&mut eval, "(ns-interns th)"), "{x #'t/x}");
    assert_eq!(run_last(&mut eval, "(ns-interns (the-ns 't))"), "{}");
    // Interns through the stale handle land in the tombstone...
    run(&mut eval, "(intern th 'y 2)");
    assert_eq!(run_last(&mut eval, "(ns-resolve th 'y)"), "#'t/y");
    assert_eq!(run_last(&mut eval, "(ns-resolve (the-ns 't) 'y)"), "nil");
    // ...as do unmaps, meta reads and meta writes.
    run(&mut eval, "(ns-unmap th 'x)");
    assert_eq!(run_last(&mut eval, "(ns-interns th)"), "{y #'t/y}");
    run(&mut eval, "(alter-meta! th assoc :k :v)");
    assert_eq!(run_last(&mut eval, "(meta th)"), "{:k :v}");
    assert_eq!(run_last(&mut eval, "(meta (the-ns 't))"), "nil");
    // A second remove+recreate dangles the handle (only the latest
    // removal is kept): reads report the name as gone.
    run(&mut eval, "(remove-ns 't)");
    run(&mut eval, "(create-ns 't)");
    assert_eq!(
        uncaught(&mut eval, "(ns-interns th)"),
        "java.lang.Exception | No namespace: t found"
    );
}

#[test]
fn alias_rules() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 't1) (create-ns 't2))");
    assert_eq!(run_last(&mut eval, "(alias 'a1 't1)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(ns-aliases *ns*)"),
        "{a1 #object[clojure.lang.Namespace 0x3 \"t1\"]}"
    );
    // Same target re-aliases silently.
    assert_eq!(run_last(&mut eval, "(alias 'a1 't1)"), "nil");
    // A different target errors naming the old one.
    assert_eq!(
        uncaught(&mut eval, "(alias 'a1 't2)"),
        "java.lang.IllegalStateException | Alias a1 already exists in namespace user, aliasing t1"
    );
    assert_eq!(
        uncaught(&mut eval, "(alias 'a2 'missing)"),
        "java.lang.Exception | No namespace: missing found"
    );
    // Namespace objects are fair targets.
    assert_eq!(run_last(&mut eval, "(alias 'a3 (the-ns 't2))"), "nil");
    assert_eq!(
        run_last(&mut eval, "(ns-aliases *ns*)"),
        "{a1 #object[clojure.lang.Namespace 0x3 \"t1\"], a3 #object[clojure.lang.Namespace 0x4 \"t2\"]}"
    );
    assert_eq!(
        uncaught(&mut eval, "(alias nil 't1)"),
        "java.lang.NullPointerException | Expecting Symbol + Namespace"
    );
    assert_eq!(
        uncaught(&mut eval, "(alias 5 't1)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
    // Aliases shadow real namespaces for lookup but not for find-ns.
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'sh) (create-ns 'real) (intern 'real 'v 1))",
    );
    run(&mut eval, "(alias 'sh 'real)");
    assert_eq!(
        run_last(&mut eval, "(ns-aliases *ns*)"),
        "{sh #object[clojure.lang.Namespace 0x4 \"real\"]}"
    );
    assert_eq!(run_last(&mut eval, "(ns-name (find-ns 'sh))"), "sh");
    assert_eq!(run_last(&mut eval, "(resolve 'sh/v)"), "#'real/v");
}

#[test]
fn ns_map_family() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'fx) (intern 'fx 'pub 1) (intern 'fx 'priv 2))",
    );
    // Privates need one meta key the surface cannot set yet (no
    // `assoc` to build the map via `alter-meta!`), so the test sets
    // it directly.
    let priv_id = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("fx"), "priv"))
        .unwrap();
    let meta = eval
        .registry
        .state(priv_id)
        .unwrap()
        .meta
        .clone()
        .unwrap()
        .insert(
            Value::Keyword(clj_value::Keyword::new(None, "private")),
            Value::Bool(true),
        );
    eval.registry
        .with_state_mut(priv_id, |st| {
            st.meta = Some(meta);
        })
        .unwrap();
    run(
        &mut eval,
        "(do (create-ns 'usefx) (in-ns 'usefx) (clojure.core/refer 'fx :only '[pub]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(
        run_last(&mut eval, "(ns-map 'fx)"),
        "{priv #'fx/priv, pub #'fx/pub}"
    );
    assert_eq!(
        run_last(&mut eval, "(ns-interns 'fx)"),
        "{priv #'fx/priv, pub #'fx/pub}"
    );
    assert_eq!(run_last(&mut eval, "(ns-publics 'fx)"), "{pub #'fx/pub}");
    assert_eq!(run_last(&mut eval, "(ns-refers 'usefx)"), "{pub #'fx/pub}");
    assert_eq!(run_last(&mut eval, "(ns-imports 'fx)"), "{}");
    assert_eq!(
        uncaught(&mut eval, "(ns-map 'missing)"),
        "java.lang.Exception | No namespace: missing found"
    );
}

#[test]
fn s9b_clojure_edn_stays_absent() {
    // S9B D3: no `clojure.edn` namespace (that needs a
    // value-producing EDN reader entry, which doesn't exist; the
    // `--edn` CLI flag covers the interactive use).
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(find-ns 'clojure.edn)"), "nil");
}

#[test]
fn refer_and_use_filters() {
    // `binding [*ns* ...]` redirects installation too (the S5B
    // sync — see the `ns_star_binding_redirects_*` tests), but
    // these tests keep the `in-ns` round-trip style.
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'lib) (intern 'lib 'a 1) (intern 'lib 'b 2))",
    );
    run(&mut eval, "(create-ns 'w1)");
    run(
        &mut eval,
        "(do (in-ns 'w1) (clojure.core/refer 'lib :only '[a]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'w1)"), "{a #'lib/a}");
    // Idempotent.
    run(
        &mut eval,
        "(do (in-ns 'w1) (clojure.core/refer 'lib :only '[a]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'w1)"), "{a #'lib/a}");
    // Refers resolve and evaluate.
    run(&mut eval, "(in-ns 'w1)");
    assert_eq!(run_last(&mut eval, "(clojure.core/eval 'a)"), "1");
    run(&mut eval, "(clojure.core/in-ns 'user)");
    // Errors.
    assert_eq!(
        uncaught(&mut eval, "(refer 'missing)"),
        "java.lang.Exception | No namespace: missing"
    );
    assert_eq!(
        uncaught(&mut eval, "(refer 'lib :only '[nope])"),
        "java.lang.IllegalAccessError | nope does not exist"
    );
    assert_eq!(
        uncaught(&mut eval, "(refer 'lib :refer 'a)"),
        "java.lang.Exception | :only/:refer value must be a sequential collection of symbols"
    );
    assert_eq!(
        uncaught(&mut eval, "(refer 'lib :only)"),
        "java.lang.IllegalArgumentException | No value supplied for key: :only"
    );
    // A non-map rename invokes as a function; nil falls back to the
    // plain name (probe36 refer-rename-scalar).
    run(&mut eval, "(create-ns 'w2)");
    run(
        &mut eval,
        "(do (in-ns 'w2) (clojure.core/refer 'lib :rename 'a) (clojure.core/in-ns 'user))",
    );
    assert_eq!(
        run_last(&mut eval, "(ns-refers 'w2)"),
        "{a #'lib/a, b #'lib/b}"
    );
    // Map renames refer the target only.
    run(&mut eval, "(create-ns 'w3)");
    run(
        &mut eval,
        "(do (in-ns 'w3) (clojure.core/refer 'lib :only '[a] :rename '{a a2}) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'w3)"), "{a2 #'lib/a}");
    // Excludes drop.
    run(&mut eval, "(create-ns 'w4)");
    run(
        &mut eval,
        "(do (in-ns 'w4) (clojure.core/refer 'lib :exclude '[a]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'w4)"), "{b #'lib/b}");
    // `:refer :all` refers every public.
    run(&mut eval, "(create-ns 'w5)");
    run(
        &mut eval,
        "(do (in-ns 'w5) (clojure.core/refer 'lib :refer :all) (clojure.core/in-ns 'user))",
    );
    assert_eq!(
        run_last(&mut eval, "(ns-refers 'w5)"),
        "{a #'lib/a, b #'lib/b}"
    );
}

#[test]
fn ns_star_binding_redirects_refer() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'blib) (intern 'blib 'x 1) (create-ns 'btarget))",
    );
    // `refer` installs into the BOUND namespace (JVM-true), not
    // the registry current.
    run(
        &mut eval,
        "(binding [*ns* (the-ns 'btarget)] (refer 'blib :only '[x]))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'btarget)"), "{x #'blib/x}");
    assert_eq!(run_last(&mut eval, "(ns-resolve 'user 'x)"), "nil");
}

#[test]
fn ns_star_binding_redirects_alias() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'blib) (intern 'blib 'x 1) (create-ns 'btarget))",
    );
    // `alias` installs into the BOUND namespace too.
    run(
        &mut eval,
        "(binding [*ns* (the-ns 'btarget)] (alias 'bx 'blib))",
    );
    assert_eq!(
        run_last(&mut eval, "(ns-aliases 'btarget)"),
        "{bx #object[clojure.lang.Namespace 0x3 \"blib\"]}"
    );
    assert_eq!(run_last(&mut eval, "(ns-resolve 'user 'bx/x)"), "nil");
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'btarget 'bx/x)"),
        "#'blib/x"
    );
}

#[test]
fn ns_star_binding_redirects_resolve() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'btarget) (intern 'btarget 'by 2))",
    );
    // `resolve` reads the BOUND namespace too.
    assert_eq!(
        run_last(
            &mut eval,
            "(binding [*ns* (the-ns 'btarget)] (resolve 'by))"
        ),
        "#'btarget/by"
    );
    assert_eq!(run_last(&mut eval, "(resolve 'by)"), "nil");
}

#[test]
fn refer_private_and_shadow() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'pl) (intern 'pl 'pub 1) (intern 'pl 'priv 2))",
    );
    let priv_id = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("pl"), "priv"))
        .unwrap();
    let meta = eval
        .registry
        .state(priv_id)
        .unwrap()
        .meta
        .clone()
        .unwrap()
        .insert(
            Value::Keyword(clj_value::Keyword::new(None, "private")),
            Value::Bool(true),
        );
    eval.registry
        .with_state_mut(priv_id, |st| {
            st.meta = Some(meta);
        })
        .unwrap();
    assert_eq!(
        uncaught(&mut eval, "(refer 'pl :only '[priv])"),
        "java.lang.IllegalAccessError | priv is not public"
    );
    // Referring onto an intern keeps the intern (the JVM's REJECTED
    // warning waits for `*err*` in S5D).
    run(
        &mut eval,
        "(do (create-ns 'sh) (in-ns 'sh) (clojure.core/intern 'sh 'pub 99))",
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/refer 'pl :only '[pub])"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/ns-name clojure.core/*ns*)"),
        "sh"
    );
    assert_eq!(
        run_last(&mut eval, "(clojure.core/ns-map 'sh)"),
        "{pub #'sh/pub}"
    );
    // `def` over a refer replaces it silently.
    run(&mut eval, "(do (clojure.core/in-ns 'user) (create-ns 'dr))");
    run(
        &mut eval,
        "(do (in-ns 'dr) (clojure.core/refer 'pl :only '[pub]))",
    );
    run(&mut eval, "(clojure.core/eval '(def pub 7))");
    run(&mut eval, "(clojure.core/in-ns 'user)");
    assert_eq!(run_last(&mut eval, "(ns-refers 'dr)"), "{}");
    assert_eq!(run_last(&mut eval, "(ns-map 'dr)"), "{pub #'dr/pub}");
}

#[test]
fn refer_clojure() {
    let mut eval = boot();
    run(&mut eval, "(create-ns 'rc)");
    assert_eq!(run_last(&mut eval, "(ns-refers 'rc)"), "{}");
    run(
        &mut eval,
        "(do (in-ns 'rc) (clojure.core/refer-clojure :exclude '[create-ns]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'rc 'in-ns)"),
        "#'clojure.core/in-ns"
    );
    assert_eq!(run_last(&mut eval, "(ns-resolve 'rc 'create-ns)"), "nil");
    // Privates never land, even unexcluded.
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'rc '*loaded-libs*)"),
        "nil"
    );
}

#[test]
fn intern_arity() {
    let mut eval = boot();
    run(&mut eval, "(create-ns 'inl)");
    assert_eq!(run_last(&mut eval, "(intern 'inl 'two)"), "#'inl/two");
    assert_eq!(
        run_last(&mut eval, "(intern 'inl 'three 42)"),
        "#'inl/three"
    );
    // Overwrites keep the same var (registry ids agree).
    run(&mut eval, "(intern 'inl 'over 1)");
    let id_before = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("inl"), "over"))
        .unwrap();
    run(&mut eval, "(intern 'inl 'over 2)");
    let id_after = eval
        .registry
        .lookup(&clj_value::Symbol::new(Some("inl"), "over"))
        .unwrap();
    assert_eq!(id_before, id_after);
    // Namespace handles are fair game.
    assert_eq!(
        run_last(&mut eval, "(intern (the-ns 'inl) 'via 9)"),
        "#'inl/via"
    );
    assert_eq!(
        uncaught(&mut eval, "(intern 'missing 'x 1)"),
        "java.lang.Exception | No namespace: missing found"
    );
    assert_eq!(
        uncaught(&mut eval, "(intern 'inl 'a/b 1)"),
        "java.lang.IllegalArgumentException | Can't intern namespace-qualified symbol"
    );
    assert_eq!(
        uncaught(&mut eval, "(intern 'inl nil 1)"),
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"sym\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(intern 'inl)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/intern"
    );
}

#[test]
fn resolve_family() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 'rs) (intern 'rs 'v 1))");
    assert_eq!(run_last(&mut eval, "(resolve 'rs/v)"), "#'rs/v");
    assert_eq!(run_last(&mut eval, "(resolve 'rs/missing)"), "nil");
    run(&mut eval, "(intern *ns* 'u 1)");
    assert_eq!(run_last(&mut eval, "(resolve 'u)"), "#'user/u");
    assert_eq!(run_last(&mut eval, "(resolve 'nope)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-resolve 'rs 'v)"), "#'rs/v");
    assert_eq!(run_last(&mut eval, "(ns-resolve 'rs 'missing)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(ns-resolve 'missing 'x)"),
        "java.lang.Exception | No namespace: missing found"
    );
    assert_eq!(
        uncaught(&mut eval, "(resolve nil)"),
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"sym\" is null"
    );
    assert_eq!(
        uncaught(&mut eval, "(resolve (the-ns 'user))"),
        "java.lang.ClassCastException | Cannot cast clojure.lang.Namespace to clojure.lang.Symbol"
    );
    // The env argument shadows.
    assert_eq!(run_last(&mut eval, "(ns-resolve 'user {'u 1} 'u)"), "nil");
    assert_eq!(run_last(&mut eval, "(resolve {'u 1} 'u)"), "nil");
    // Aliases resolve through.
    run(&mut eval, "(alias 'rsl 'rs)");
    assert_eq!(run_last(&mut eval, "(ns-resolve *ns* 'rsl/v)"), "#'rs/v");
    assert_eq!(run_last(&mut eval, "(resolve 'rsl/v)"), "#'rs/v");
}

#[test]
fn unmap_and_unalias() {
    let mut eval = boot();
    run(&mut eval, "(intern *ns* 'unm 1)");
    assert_eq!(run_last(&mut eval, "(ns-unmap *ns* 'unm)"), "nil");
    assert_eq!(run_last(&mut eval, "(resolve 'unm)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-unmap *ns* 'missing)"), "nil");
    assert_eq!(
        uncaught(&mut eval, "(ns-unmap *ns* nil)"),
        "java.lang.NullPointerException | Cannot read field \"ns\" because \"sym\" is null"
    );
    run(&mut eval, "(do (create-ns 't) (alias 'ua 't))");
    assert_eq!(run_last(&mut eval, "(ns-unalias *ns* 'ua)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-unalias *ns* 'ua)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-unalias *ns* nil)"), "nil");
}

#[test]
fn all_ns_lists_live_sorted() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 'zz) (create-ns 'aa))");
    assert_eq!(
        run_last(&mut eval, "(all-ns)"),
        "(#object[clojure.lang.Namespace 0x4 \"aa\"] #object[clojure.lang.Namespace 0x1 \"clojure.core\"] #object[clojure.lang.Namespace 0x2 \"user\"] #object[clojure.lang.Namespace 0x3 \"zz\"])"
    );
    run(&mut eval, "(remove-ns 'zz)");
    assert_eq!(
        run_last(&mut eval, "(all-ns)"),
        "(#object[clojure.lang.Namespace 0x4 \"aa\"] #object[clojure.lang.Namespace 0x1 \"clojure.core\"] #object[clojure.lang.Namespace 0x2 \"user\"])"
    );
}

#[test]
fn eval_reenters_top_level() {
    let mut eval = boot();
    assert_eq!(run_last(&mut eval, "(eval 5)"), "5");
    // Top-level `do` splits: later elements analyze after earlier
    // ones evaluate.
    assert_eq!(run_last(&mut eval, "(eval '(do (def ev1 1) ev1))"), "1");
    assert_eq!(run_last(&mut eval, "ev1"), "1");
    assert_eq!(
        uncaught_chain(&mut eval, "(eval 'nope)"),
        vec![
            "clojure.lang.Compiler$CompilerException | Syntax error compiling at (user:1:1)."
                .to_owned(),
            "java.lang.RuntimeException | Unable to resolve symbol: nope in this context"
                .to_owned(),
        ]
    );
}

#[test]
fn arity_names_are_qualified() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(the-ns)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/the-ns"
    );
    assert_eq!(
        uncaught(&mut eval, "(ns-resolve 'user)"),
        "clojure.lang.ArityException | Wrong number of args (1) passed to: clojure.core/ns-resolve"
    );
}

#[test]
fn ns_unalias_via_symbol() {
    let mut eval = boot();
    run(&mut eval, "(alias 'axx 'clojure.core)");
    assert_eq!(run_last(&mut eval, "(ns-unalias 'user 'axx)"), "nil");
    assert_eq!(run_last(&mut eval, "(ns-aliases 'user)"), "{}");
}

#[test]
fn ns_unalias_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(ns-unalias 'user 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
}

#[test]
fn ns_unmap_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(ns-unmap 'user 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
}

#[test]
fn ns_unmap_qualified() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(ns-unmap 'user 'a/b)"),
        "java.lang.IllegalArgumentException | Can't unintern namespace-qualified symbol"
    );
}

#[test]
fn ns_resolve_env_rows() {
    let mut eval = boot();
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'user '#{map} 'map)"),
        "nil"
    );
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'user '#{zzz} 'map)"),
        "#'clojure.core/map"
    );
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'user nil 'map)"),
        "#'clojure.core/map"
    );
    assert_eq!(
        run_last(&mut eval, "(ns-resolve 'user '{y 1} 'map)"),
        "#'clojure.core/map"
    );
}

#[test]
fn intern_adopts_name_meta() {
    let mut eval = boot();
    run(&mut eval, "(create-ns 'px)");
    assert_eq!(
        run_last(
            &mut eval,
            "(get (meta (intern 'px (with-meta 'bar {:a 1}))) :a)"
        ),
        "1"
    );
    assert_eq!(run_last(&mut eval, "(ns-resolve 'px 'bar)"), "#'px/bar");
}

#[test]
fn intern_long_name() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(intern 'user 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
}

#[test]
fn refer_arity_zero() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(refer)"),
        "clojure.lang.ArityException | Wrong number of args (0) passed to: clojure.core/refer"
    );
}

#[test]
fn refer_rename_rows() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'rlib) (intern 'rlib 'a 1) (intern 'rlib 'b 2))",
    );
    // Map value non-symbol (586).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (create-ns 'wrn) (in-ns 'wrn) (clojure.core/refer 'rlib :only '[a] :rename '{a 42}))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
    run(&mut eval, "(clojure.core/in-ns 'user)");
    // Fn returning a symbol renames (590).
    run(&mut eval, "(create-ns 'wrs)");
    run(
        &mut eval,
        "(do (in-ns 'wrs) (clojure.core/refer 'rlib :only '[a] :rename (fn [s] 'zzz)) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'wrs)"), "{zzz #'rlib/a}");
    // Fn returning a non-symbol casts (591).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (create-ns 'wrx) (in-ns 'wrx) (clojure.core/refer 'rlib :only '[a] :rename (fn [s] 42)))"
        ),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
    run(&mut eval, "(clojure.core/in-ns 'user)");
    // A throwing rename fn propagates (592).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (create-ns 'wrt) (in-ns 'wrt) (clojure.core/refer 'rlib :only '[a] :rename (fn [s] (throw (clojure.core/ex-info \"boom\" {})))))"
        ),
        "clojure.lang.ExceptionInfo | boom"
    );
    run(&mut eval, "(clojure.core/in-ns 'user)");
    // Qualified rename target rejected (596-599).
    assert_eq!(
        uncaught(
            &mut eval,
            "(do (create-ns 'wrq) (in-ns 'wrq) (clojure.core/refer 'rlib :only '[a] :rename '{a b/c}))"
        ),
        "java.lang.IllegalArgumentException | Can't intern namespace-qualified symbol"
    );
    run(&mut eval, "(clojure.core/in-ns 'user)");
}

#[test]
fn refer_only_nonsymbol() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 'rlib) (intern 'rlib 'a 1))");
    assert_eq!(
        uncaught(&mut eval, "(refer 'rlib :only [42])"),
        "java.lang.IllegalAccessError | 42 does not exist"
    );
}

#[test]
fn refer_only_list() {
    let mut eval = boot();
    run(
        &mut eval,
        "(do (create-ns 'rlib) (intern 'rlib 'a 1) (intern 'rlib 'b 2))",
    );
    run(&mut eval, "(create-ns 'wl)");
    run(
        &mut eval,
        "(do (in-ns 'wl) (clojure.core/refer 'rlib :only '(a)) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-refers 'wl)"), "{a #'rlib/a}");
}

#[test]
fn ns_map_with_refers() {
    let mut eval = boot();
    run(&mut eval, "(do (create-ns 'fx) (intern 'fx 'pub 1))");
    run(
        &mut eval,
        "(do (create-ns 'usefx) (in-ns 'usefx) (clojure.core/refer 'fx :only '[pub]) (clojure.core/in-ns 'user))",
    );
    assert_eq!(run_last(&mut eval, "(ns-map 'usefx)"), "{pub #'fx/pub}");
}

#[test]
fn remove_ns_long() {
    let mut eval = boot();
    assert_eq!(
        uncaught(&mut eval, "(remove-ns 42)"),
        "java.lang.ClassCastException | Cannot cast java.lang.Long to clojure.lang.Symbol"
    );
}

#[test]
fn current_ns_falls_back_on_foreign_star_ns() {
    let mut eval = boot();
    // A non-namespace `*ns*` binding falls back to the
    // registry current; the def still lands in `user`.
    assert_eq!(
        run_last(&mut eval, "(binding [*ns* 1] (def qqq268 1))"),
        "#'user/qqq268"
    );
}
