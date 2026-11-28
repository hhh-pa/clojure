// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Vector equality probes recorded on JVM Clojure 1.13.0-alpha10.

use clj_eval::boot;
use clj_value::Value;

#[test]
fn vector_windows_match_jvm() {
    let mut eval = boot();
    let source = include_str!("../../../../docs/spec/i277-recipes/probes/vector_equiv.clj");
    let (definitions, rows) = source.split_once("(t ").expect("first probe");
    eval.eval_source(
        "(binding [*warn-on-reflection* *warn-on-reflection*] (require 'clojure.string))",
    )
    .unwrap();
    eval.eval_source(definitions).expect("probe definitions");
    let program = format!("(with-out-str (t {rows}\n)");
    let values = eval.eval_source(&program).expect("vector equality probes");
    let Some(Value::Str(output)) = values.last() else {
        panic!("expected captured probe output");
    };
    assert_eq!(
        &**output,
        include_str!("../../../../docs/spec/i277-recipes/probes/vector_equiv.out")
    );
}
