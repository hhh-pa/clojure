// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5 coverage matrix (TDD): every `planned`/`implemented`/
//! `divergent` row in `rust/core-coverage.md` must resolve in the
//! booted core; every `out-of-scope` row must not. Fails until each
//! var lands (the failure lists the missing names).

/// Parse the normative table: (`var`, `status`) rows.
fn coverage_rows() -> Vec<(String, String)> {
    const COVERAGE: &str = include_str!("../../../core-coverage.md");
    let mut rows = Vec::new();
    for line in COVERAGE.lines() {
        let cells: Vec<&str> = line.split('|').map(str::trim).collect();
        let [_, var, status, _, _] = cells.as_slice() else {
            continue;
        };
        let Some(name) = var.strip_prefix('`').and_then(|v| v.strip_suffix('`')) else {
            continue;
        };
        if name == "var" {
            continue;
        }
        rows.push((name.to_owned(), (*status).to_owned()));
    }
    rows
}

/// Does `name` resolve to a var in the booted core?
fn resolves(eval: &mut clj_eval::Evaluator, name: &str) -> bool {
    match eval.eval_source(&format!("(resolve '{name})")) {
        Ok(values) => !matches!(values.as_slice(), [clj_value::Value::Nil]),
        Err(_) => false,
    }
}

#[test]
fn coverage_matrix() {
    let rows = coverage_rows();
    assert!(!rows.is_empty(), "coverage file parsed to zero rows");
    let mut eval = clj_core::boot();
    let mut missing = Vec::new();
    let mut leaked = Vec::new();
    for (name, status) in &rows {
        match status.as_str() {
            "planned" | "implemented" | "divergent" => {
                if !resolves(&mut eval, name) {
                    missing.push(name.clone());
                }
            }
            "out-of-scope" => {
                if resolves(&mut eval, name) {
                    leaked.push(name.clone());
                }
            }
            other => panic!("unknown coverage status for {name}: {other}"),
        }
    }
    assert!(
        missing.is_empty() && leaked.is_empty(),
        "missing: {missing:?}; leaked: {leaked:?}"
    );
}
