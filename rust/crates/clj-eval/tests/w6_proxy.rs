// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W6: `clojure.core-proxy` smoke tests — the `core_proxy.clj`
//! API (proxy macro, proxy helpers, bean) over the port's
//! deftype/reify machinery.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, pr_to_string};

fn pr(v: &clj_value::Value) -> String {
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

fn boot_proxy() -> Evaluator {
    let mut eval = boot();
    run(&mut eval, "(require 'clojure.core-proxy)");
    eval
}

#[test]
fn core_proxy_ns_loads() {
    let mut eval = boot_proxy();
    assert_eq!(
        run(
            &mut eval,
            "[(ifn? proxy-mappings) (ifn? get-proxy-class) (ifn? construct-proxy) (ifn? init-proxy) (ifn? update-proxy) (ifn? proxy-call-with-super) (ifn? proxy-name) (ifn? method-sig) (ifn? bean)]"
        ),
        "[true true true true true true true true true]"
    );
}

#[test]
fn proxy_macro_maps_methods() {
    let mut eval = boot_proxy();
    // The proxy fn map answers the mapped method (port proxy fns
    // exclude `this`, per the prelude).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [p (proxy [Object] [] (toString [] "w6!"))] ((get (proxy-mappings p) "toString")))"#
        ),
        r#""w6!""#
    );
}

#[test]
fn proxy_class_lifecycle() {
    let mut eval = boot_proxy();
    assert_eq!(
        run(
            &mut eval,
            r#"(let [c (get-proxy-class Object)
                      p (construct-proxy c)
                      _ (init-proxy p {"toString" (fn [] "hi")})]
                  [((get (proxy-mappings p) "toString"))
                   (do (update-proxy p {"toString" (fn [] "yo")})
                       ((get (proxy-mappings p) "toString")))
                   (do (update-proxy p {"toString" nil})
                       [(contains? (proxy-mappings p) "toString")
                        (get (proxy-mappings p) "toString")])])"#
        ),
        r#"["hi" "yo" [true nil]]"#
    );
}

#[test]
fn proxy_super_calls_up() {
    let mut eval = boot_proxy();
    // The atom carries the instance into its own method body (the
    // S14 pattern — `this` is caller-bound); `str` dispatches the
    // mapped toString, which prefixes the super call.
    let v = run(
        &mut eval,
        r#"(let [pc (atom nil) p (proxy [Object] [] (toString [] (let [this @pc] (str "S:" (proxy-super toString)))))] (reset! pc p) (str p))"#,
    );
    assert!(v.starts_with("\"S:"), "{v}");
}

#[test]
fn bean_and_names() {
    let mut eval = boot_proxy();
    assert_eq!(run(&mut eval, "(map? (bean (Object.)))"), "true");
    assert_eq!(
        run(
            &mut eval,
            "[(string? (proxy-name Object [])) (boolean (re-find #\"proxy\" (proxy-name Object [])))]"
        ),
        "[true true]"
    );
}
