// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! S5D io smoke: `println`/`pr`/`newline`/`flush` captured via
//! `with-out-str`, a `spit`/`slurp` roundtrip (dirs owned by the
//! Rust side), and `read-string` basics.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = vs.last().unwrap();
    let r = eval.realize_deep(v).unwrap();
    pr(&r)
}

#[test]
fn io_smoke_rows() {
    let mut eval = boot();
    // Print fns captured through `with-out-str` (exact strings).
    assert_eq!(
        run_last(&mut eval, "(with-out-str (println 1))"),
        "\"1\\n\""
    );
    assert_eq!(
        run_last(&mut eval, "(with-out-str (pr 1 \"a\"))"),
        "\"1 \\\"a\\\"\""
    );
    assert_eq!(run_last(&mut eval, "(with-out-str (newline))"), "\"\\n\"");
    assert_eq!(run_last(&mut eval, "(with-out-str (flush))"), "\"\"");
    // `spit`/`slurp` roundtrip: Rust owns the dirs, the path goes
    // into the form as a string literal.
    let dir = "./target/tmp-s5d-io";
    let file = "./target/tmp-s5d-io/round.txt";
    let target_preexisted = std::path::Path::new("./target").exists();
    std::fs::create_dir_all(dir).unwrap();
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(do (spit {file:?} \"hello\") (slurp {file:?}))")
        ),
        "\"hello\""
    );
    assert_eq!(
        run_last(
            &mut eval,
            &format!("(do (spit {file:?} \"!!\" :append true) (slurp {file:?}))")
        ),
        "\"hello!!\""
    );
    std::fs::remove_dir_all(dir).unwrap();
    if !target_preexisted {
        // Only remove what we created; ignore a non-empty dir.
        let _ = std::fs::remove_dir("./target");
    }
    // `read-string` reads one form.
    assert_eq!(run_last(&mut eval, "(read-string \"1 2\")"), "1");
    assert_eq!(run_last(&mut eval, "(read-string \"[1 :a]\")"), "[1 :a]");
}
