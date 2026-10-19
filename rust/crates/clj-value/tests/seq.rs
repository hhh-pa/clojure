// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Seq tests (S3 TDD, C4): constructors, first/next/more, nil-punning,
//! chunked realization counts (probe18), lazy memoization.

use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use clj_value::{List, Map, Seq, Set, Value, Vector, seq_vec};

fn int(i: i64) -> Value {
    Value::Int(i)
}

fn ints(n: i64) -> Vec<Value> {
    (0..n).map(int).collect()
}

/// Chunk-aware lazy map (mirrors core.clj's chunked path).
fn map_seq(f: Arc<dyn Fn(&Value) -> Value + Send + Sync>, s: Seq) -> Seq {
    Seq::lazy(move || {
        if s.is_empty() {
            return None;
        }
        if let Some((chunk, rest)) = s.chunk_next() {
            let out: Vec<Value> = chunk.iter().map(|v| f(v)).collect();
            let tail = map_seq(Arc::clone(&f), rest);
            Some(Seq::chunk_cons(out, tail))
        } else {
            let head = f(s.first().unwrap());
            let tail = s.next().map(|t| map_seq(Arc::clone(&f), t));
            Some(Seq::cons(head, tail))
        }
    })
}

/// Chunk-aware lazy filter (mirrors core.clj: chunk in, chunk out).
fn filter_seq(p: Arc<dyn Fn(&Value) -> bool + Send + Sync>, s: Seq) -> Seq {
    Seq::lazy(move || {
        let mut cur = s.clone();
        loop {
            if cur.is_empty() {
                return None;
            }
            if let Some((chunk, rest)) = cur.chunk_next() {
                let kept: Vec<Value> = chunk.iter().filter(|v| p(v)).cloned().collect();
                if kept.is_empty() {
                    cur = rest;
                    continue;
                }
                let tail = filter_seq(Arc::clone(&p), rest);
                return Some(Seq::chunk_cons(kept, tail));
            }
            let h = cur.first()?;
            let tail = cur.next();
            if p(h) {
                let rest = tail.map(|t| filter_seq(Arc::clone(&p), t));
                return Some(Seq::cons(h.clone(), rest));
            }
            match tail {
                Some(t) => cur = t,
                None => return None,
            }
        }
    })
}

/// Chunk-aware take: pulls whole chunks like the JVM.
fn take_seq(n: usize, s: &Seq) -> Vec<Value> {
    let mut out = Vec::new();
    let mut cur = s.clone();
    let mut rem = n;
    while rem > 0 {
        if cur.is_empty() {
            break;
        }
        if let Some((chunk, rest)) = cur.chunk_next() {
            let k = rem.min(chunk.len());
            out.extend(chunk[..k].iter().cloned());
            rem -= k;
            cur = rest;
        } else {
            let Some(h) = cur.first() else {
                break;
            };
            out.push(h.clone());
            rem -= 1;
            match cur.next() {
                Some(t) => cur = t,
                None => break,
            }
        }
    }
    out
}

/// Realization counter for side-effect-order tests.
#[derive(Clone)]
struct Counter {
    hits: Arc<AtomicUsize>,
}

impl Counter {
    fn new() -> Self {
        Self {
            hits: Arc::new(AtomicUsize::new(0)),
        }
    }

    fn rec(&self, v: &Value) -> Value {
        self.hits.fetch_add(1, Ordering::SeqCst);
        v.clone()
    }

    fn count(&self) -> usize {
        self.hits.load(Ordering::SeqCst)
    }
}

#[test]
fn seq_constructors_and_nil_punning() {
    // Empty sources seq to None (`(seq [])` is nil).
    assert!(Seq::from_vector(&Vector::from_vec(vec![])).is_none());
    assert!(Seq::from_list(&List::from_vec(vec![])).is_none());
    let m = Map::from_kvs_checked(vec![]).unwrap();
    assert!(Seq::from_map(&m).is_none());
    // Non-empty sources.
    let vseq = Seq::from_vector(&Vector::from_vec(ints(3))).unwrap();
    assert!(vseq.chunk_next().is_some());
    let lseq = Seq::from_list(&List::from_vec(ints(3))).unwrap();
    assert!(lseq.chunk_next().is_none());
    assert_eq!(vseq.first(), Some(&int(0)));
    assert_eq!(lseq.first(), Some(&int(0)));
    // Map seqs yield [k v] vectors in seq order.
    let m = Map::from_kvs_checked(vec![(int(1), int(2)), (int(3), int(4))]).unwrap();
    let got = take_seq(10, &Seq::from_map(&m).unwrap());
    assert_eq!(got.len(), 2);
    assert!(matches!(&got[0], Value::Vector(_)));
    // Set seqs yield members in insertion order.
    let s = Set::from_items_checked(vec![int(5), int(6)]).unwrap();
    let got = take_seq(10, &Seq::from_set(&s).unwrap());
    assert_eq!(got, vec![int(5), int(6)]);
}

#[test]
fn first_next_more_empty_rules() {
    let s = Seq::from_vector(&Vector::from_vec(ints(2))).unwrap();
    assert_eq!(s.first(), Some(&int(0)));
    let n = s.next().unwrap();
    assert_eq!(n.first(), Some(&int(1)));
    assert!(n.next().is_none());
    // more of the last item is the empty seq (truthy mechanics are S5).
    assert!(n.more().is_empty());
    let e = Seq::empty();
    assert!(e.first().is_none());
    assert!(e.next().is_none());
    assert!(e.is_empty());
    // cons onto nil and onto seqs.
    let c = Seq::cons(int(9), None);
    assert_eq!(c.first(), Some(&int(9)));
    assert!(c.next().is_none());
    let c2 = Seq::cons(int(8), Some(c));
    assert_eq!(take_seq(5, &c2), vec![int(8), int(9)]);
}

#[test]
fn chunked_realization_counts_match_jvm() {
    let mk = || {
        let c = Counter::new();
        let c2 = c.clone();
        let f: Arc<dyn Fn(&Value) -> Value + Send + Sync> = Arc::new(move |v| c2.rec(v));
        (c, f)
    };
    // take2-map-range => 32 (chunked input realizes a full chunk).
    let (c, f) = mk();
    let r = Seq::chunked(ints(100)).unwrap();
    assert_eq!(take_seq(2, &map_seq(f, r)).len(), 2);
    assert_eq!(c.count(), 32);
    // take2-map-list => 2 (sequential input realizes one at a time).
    let (c, f) = mk();
    let l = Seq::from_list(&List::from_vec(ints(100))).unwrap();
    assert_eq!(take_seq(2, &map_seq(f, l)).len(), 2);
    assert_eq!(c.count(), 2);
    // take2-map-vector => 32.
    let (c, f) = mk();
    let v = Seq::from_vector(&Vector::from_vec(ints(100))).unwrap();
    assert_eq!(take_seq(2, &map_seq(f, v)).len(), 2);
    assert_eq!(c.count(), 32);
    // take35-map-range => 64 (two full chunks).
    let (c, f) = mk();
    let r = Seq::chunked(ints(100)).unwrap();
    assert_eq!(take_seq(35, &map_seq(f, r)).len(), 35);
    assert_eq!(c.count(), 64);
    // take2-filter-map-range => 32 (filter pulls a full chunk).
    let (c, f) = mk();
    let r = Seq::chunked(ints(100)).unwrap();
    let p: Arc<dyn Fn(&Value) -> bool + Send + Sync> =
        Arc::new(|v| matches!(v, Value::Int(i) if i % 2 == 1));
    assert_eq!(take_seq(2, &filter_seq(p, map_seq(f, r))).len(), 2);
    assert_eq!(c.count(), 32);
    // take2-map-filter-range => 16 (filter preserves chunking: a
    // 16-odd chunk reaches map, which realizes all of it).
    let (c, f) = mk();
    let r = Seq::chunked(ints(100)).unwrap();
    let p: Arc<dyn Fn(&Value) -> bool + Send + Sync> =
        Arc::new(|v| matches!(v, Value::Int(i) if i % 2 == 1));
    let got = take_seq(2, &map_seq(f, filter_seq(p, r)));
    assert_eq!(got, vec![int(1), int(3)]);
    assert_eq!(c.count(), 16);
    // take2-map-vec10 => 10 (small vectors are chunked too).
    let (c, f) = mk();
    let v = Seq::from_vector(&Vector::from_vec(ints(10))).unwrap();
    assert_eq!(take_seq(2, &map_seq(f, v)), vec![int(0), int(1)]);
    assert_eq!(c.count(), 10);
}

#[test]
fn chunk_next_serves_remainder_mid_chunk() {
    let s = Seq::from_vector(&Vector::from_vec(ints(100))).unwrap();
    let mid = s.next().unwrap();
    let (chunk, rest) = mid.chunk_next().unwrap();
    assert_eq!(chunk.len(), 31);
    assert_eq!(chunk[0], int(1));
    assert_eq!(rest.first(), Some(&int(32)));
    let (fresh, _) = s.chunk_next().unwrap();
    assert_eq!(fresh.len(), 32);
}

#[test]
fn lazy_memoizes_once() {
    let c = Counter::new();
    let c2 = c.clone();
    let s = Seq::lazy(move || {
        c2.rec(&int(0));
        Seq::eager(vec![int(7)])
    });
    assert_eq!(s.first(), Some(&int(7)));
    assert_eq!(s.first(), Some(&int(7)));
    assert!(s.next().is_none());
    assert_eq!(c.count(), 1);
}

#[test]
fn lazy_memoizes_once_across_threads() {
    use std::thread;
    let c = Counter::new();
    let c2 = c.clone();
    let s = Arc::new(Seq::lazy(move || {
        c2.rec(&int(0));
        Seq::eager(vec![int(7)])
    }));
    let mut handles = Vec::new();
    for _ in 0..8 {
        let s = Arc::clone(&s);
        handles.push(thread::spawn(move || {
            for _ in 0..50 {
                assert_eq!(s.first(), Some(&int(7)));
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    assert_eq!(c.count(), 1);
}

#[test]
fn seq_is_send_sync() {
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<Seq>();
}

#[test]
fn seq_vec_coerces_seqables() {
    assert_eq!(seq_vec(&Value::Nil), Some(Vec::new()));
    assert_eq!(
        seq_vec(&Value::Str(Arc::from("ab"))),
        Some(vec![Value::Char('a'), Value::Char('b')])
    );
    let list = Value::List(List::from_vec(vec![Value::Int(1)]));
    assert_eq!(seq_vec(&list), Some(vec![Value::Int(1)]));
    let vec = Value::Vector(Vector::from_vec(vec![Value::Int(2)]));
    assert_eq!(seq_vec(&vec), Some(vec![Value::Int(2)]));
    let map = Value::Map(Map::from_kvs_checked(vec![(Value::Int(1), Value::Int(2))]).unwrap());
    assert_eq!(
        seq_vec(&map),
        Some(vec![Value::Vector(Vector::from_vec(vec![
            Value::Int(1),
            Value::Int(2)
        ]))])
    );
    let set = Value::Set(Set::from_items_checked(vec![Value::Int(3)]).unwrap());
    assert_eq!(seq_vec(&set), Some(vec![Value::Int(3)]));
    assert_eq!(seq_vec(&Value::Int(1)), None);
    assert_eq!(seq_vec(&Value::Bool(true)), None);
}
