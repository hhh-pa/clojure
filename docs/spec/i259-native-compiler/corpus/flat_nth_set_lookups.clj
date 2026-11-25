;; #271 Slice 5w: `nth` through the flat-vector front (longs, doubles,
;; generic items, tries, strings, transients, subvectors, defaults,
;; errors) and `contains?`/`get` on small sets by scalar keys (a long
;; key against a big-integer member, sorted and hashed sets).
(defn at [v i] (nth v i))
(defn at3 [v i d] (nth v i d))
(def big (vec (range 2000)))
(defn has [s k] (contains? s k))
(defn getk [s k] (get s k))
(println (at [1 2 3] 1) (at [1.5 2.5] 1) (at [[1] :k "s"] 0) (at [nil true] 1)
         (at big 1500) (at "abc" 2) (at (transient [4 5]) 1) (at3 [1 2] 5 :d)
         (at3 [1 2] -1 :d) (at3 "ab" 9 :d) (at3 big 1999 :d) (at (subvec [1 2 3] 1) 0))
(println (has #{1 :a \c true nil} 1) (has #{1 :a} 2) (has #{1N} 1) (has #{:a} :a)
         (has #{\c} \c) (has #{nil} nil) (has #{false} false) (has #{"s"} "s")
         (has (sorted-set 3 1) 1) (has (set (range 20)) 7) (has #{1.0} 1))
(println (getk #{1 2} 2) (getk #{1N} 1) (getk #{:a} :b) (getk (set (range 20)) 19)
         (getk #{[1 2]} [1 2]) ((fn [s] (s 2)) #{2}))
(println (try (at [1 2] 5) (catch Exception _ :oob)) (try (at [1 2] -1) (catch Exception _ :neg))
         (loop [i 0 acc 0] (if (< i 4) (recur (inc i) (+ acc (at [10 20 30 40] i))) acc)))
