// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! Chunked-lazy string/vector seq pins (#221): `seq` over strings
//! and vectors realizes 32 items per step instead of materializing
//! everything eagerly. These pins guard the chunk-boundary
//! arithmetic (empty/1/31/32/33/64/65, multi-byte chars) and
//! round-trip equivalence with the strict paths.

use clj_eval::{Evaluator, boot};
use clj_value::{PrintOpts, Value, pr_to_string};

fn pr(v: &Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

fn run_last(eval: &mut Evaluator, src: &str) -> String {
    let vs = eval.eval_source(src).unwrap();
    let v = eval.realize_deep(vs.last().unwrap()).unwrap();
    pr(&v)
}

fn ev() -> Evaluator {
    boot()
}

/// String seqs at chunk edges (32-char chunks).
#[test]
fn string_seq_chunk_boundaries() {
    let mut e = ev();
    for n in [0, 1, 31, 32, 33, 63, 64, 65, 100] {
        let s: String = (0..n).map(|i| (b'a' + (i % 26) as u8) as char).collect();
        let src = format!("(count (seq {s:?}))");
        assert_eq!(run_last(&mut e, &src), n.to_string(), "count n={n}");
        let src = format!("(apply str (seq {s:?}))");
        assert_eq!(run_last(&mut e, &src), format!("{s:?}"), "roundtrip n={n}");
    }
    assert_eq!(run_last(&mut e, "(seq \"\")"), "nil");
    assert_eq!(run_last(&mut e, "(first (seq \"\"))"), "nil");
}

/// Multi-byte chars step by char, not by byte (byte positions must
/// stay on char boundaries across chunks).
#[test]
fn string_seq_unicode_chunks() {
    let mut e = ev();
    // 40 two-byte chars + 40 three-byte chars: chunk splits land
    // mid-string at non-obvious byte offsets.
    let s: String = std::iter::repeat_n('é', 40)
        .chain(std::iter::repeat_n('中', 40))
        .collect();
    assert_eq!(run_last(&mut e, &format!("(count (seq {s:?}))")), "80");
    assert_eq!(
        run_last(&mut e, &format!("(apply str (seq {s:?}))")),
        format!("{s:?}")
    );
    assert_eq!(run_last(&mut e, &format!("(nth (seq {s:?}) 39)")), "\\é");
    assert_eq!(run_last(&mut e, &format!("(nth (seq {s:?}) 40)")), "\\中");
    assert_eq!(run_last(&mut e, &format!("(nth (seq {s:?}) 79)")), "\\中");
}

/// Vector seqs at chunk edges, `first`/`next`/`rest`/`nth`/`last`.
#[test]
fn vector_seq_chunk_boundaries() {
    let mut e = ev();
    for n in [0, 1, 31, 32, 33, 63, 64, 65, 100] {
        let v: Vec<i64> = (0..n).collect();
        let lit = format!(
            "[{}]",
            v.iter()
                .map(|i| i.to_string())
                .collect::<Vec<_>>()
                .join(" ")
        );
        assert_eq!(
            run_last(&mut e, &format!("(count (seq {lit}))")),
            n.to_string(),
            "count n={n}"
        );
        assert_eq!(
            run_last(&mut e, &format!("(vec (seq {lit}))")),
            if n == 0 { "[]".to_owned() } else { lit.clone() },
            "roundtrip n={n}"
        );
    }
    assert_eq!(run_last(&mut e, "(seq [])"), "nil");
    assert_eq!(run_last(&mut e, "(nth (seq (vec (range 100))) 32)"), "32");
    assert_eq!(run_last(&mut e, "(nth (seq (vec (range 100))) 33)"), "33");
    assert_eq!(run_last(&mut e, "(nth (seq (vec (range 65))) 64)"), "64");
    assert_eq!(run_last(&mut e, "(nth (seq (vec (range 65))) 0)"), "0");
    assert_eq!(
        run_last(&mut e, "(count (drop 33 (seq (vec (range 100)))))"),
        "67"
    );
}

/// `rest`/`next` chains across chunk joints keep every item.
#[test]
fn seq_rest_chains_across_chunks() {
    let mut e = ev();
    assert_eq!(
        run_last(
            &mut e,
            "(count (take 70 (iterate next (seq (vec (range 100))))))"
        ),
        "70"
    );
    assert_eq!(
        run_last(
            &mut e,
            "(apply str (take 70 (seq (apply str (repeat 100 \\a)))))"
        ),
        format!("{:?}", "a".repeat(70))
    );
}
