// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! Threaded sharing test (S3 TDD): persistent collections shared
//! across threads stay consistent under concurrent reads, clones,
//! and derived writes.

use std::sync::Arc;
use std::thread;

use clj_value::{List, Map, Set, Value, Vector};

fn int(i: i64) -> Value {
    Value::Int(i)
}

#[test]
fn persistent_collections_share_across_threads() {
    let map =
        Arc::new(Map::from_kvs_checked((0..100).map(|i| (int(i), int(i * 2))).collect()).unwrap());
    let vec = Arc::new(Vector::from_vec((0..100).map(int).collect()));
    let set = Arc::new(Set::from_items_checked((0..100).map(int).collect()).unwrap());
    let list = Arc::new(List::from_vec((0..100).map(int).collect()));

    let mut handles = Vec::new();
    for t in 0..8 {
        let (map, vec, set, list) = (
            Arc::clone(&map),
            Arc::clone(&vec),
            Arc::clone(&set),
            Arc::clone(&list),
        );
        handles.push(thread::spawn(move || {
            for i in 0..100 {
                let k = int((i + t) % 100);
                // Reads.
                assert_eq!(map.get(&k), Some(&int(((i + t) % 100) * 2)));
                assert!(map.contains_key(&k));
                assert_eq!(map.size(), 100);
                assert!(set.contains(&k));
                assert_eq!(set.size(), 100);
                assert_eq!(vec.iter().nth(i as usize), Some(&int(i)));
                assert_eq!(vec.len(), 100);
                assert_eq!(list.iter().nth(i as usize), Some(&int(i)));
                assert_eq!(list.len(), 100);
                // Clones + derived persistent writes (never mutate shared).
                let m2 = (*map).clone().insert(int(1000 + t), int(t));
                assert_eq!(m2.size(), 101);
                assert_eq!(map.size(), 100);
                let v2 = (*vec).clone().push_back(int(t));
                assert_eq!(v2.len(), 101);
                let s2 = (*set).clone().insert(int(1000 + t));
                assert_eq!(s2.size(), 101);
                let l2 = (*list).clone().push_front(int(t));
                assert_eq!(l2.len(), 101);
            }
        }));
    }
    for h in handles {
        h.join().unwrap();
    }
    // The shared originals are untouched.
    assert_eq!(map.size(), 100);
    assert_eq!(vec.len(), 100);
    assert_eq!(set.size(), 100);
    assert_eq!(list.len(), 100);
}
