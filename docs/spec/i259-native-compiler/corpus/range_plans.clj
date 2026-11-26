;; Fused loops over (range …) classify by their long bounds without
;; building the range value (#271 Slice 5l): every fused HofLoop shape
;; over 1-3-arg ranges, the empty and repeat shapes, double bounds that
;; stay on the general path, and lazy-side-effect order.
(defn rsum [k] (reduce (fn [a j] (+ a j)) 0 (range k)))
(defn rsum2 [a b] (reduce (fn [a j] (+ a j)) 0 (range a b)))
(defn rsum3 [a b c] (reduce (fn [a j] (+ a j)) 0 (range a b c)))
(defn rmapv [k] (mapv (fn [j] (* j j)) (range k)))
(defn rsome [k] (some (fn [j] (when (= j 3) :hit)) (range k)))
(defn revery [k] (every? (fn [j] (< j 5)) (range k)))
(defn rfilter [k] (filterv (fn [j] (even? j)) (range k)))
(defn rcount [k] (count (filter (fn [j] (odd? j)) (range k))))
(defn rvec [k] (vec (map (fn [j] (inc j)) (range k))))
(defn rinto [k] (into [] (map (fn [j] (* 2 j)) (range k))))
(defn rdorun [k] (let [a (atom 0)] (dorun (map (fn [j] (swap! a + j)) (range k))) @a))
(defn rmap2 [a b] (mapv (fn [x y] (+ x y)) (range a) (range b)))
(defn rstep [k st] (reduce (fn [a j] (+ a j)) 0 (range 0 k st)))

(println (rsum 5) (rsum 0) (rsum2 2 7) (rsum3 0 10 3) (rsum3 9 0 -3) (rsum3 10 1 4))
(println (rmapv 4) (rmapv 0) (rsome 5) (rsome 3) (revery 4) (revery 9))
(println (rfilter 9) (rcount 10) (rvec 3) (rinto 3) (rdorun 6))
(println (rmap2 3 5) (rmap2 5 3) (rstep 9 2) (rstep 0 0) (rstep 5 -2))
;; zero step repeats the start forever: stays general, take bounds it
(println (vec (take 4 (map (fn [j] (* j 10)) (range 7 9 0)))))
;; double bounds stay general and walk the double range
(println (mapv (fn [j] j) (range 0.5 2.5 0.5)) (reduce + 0 (range 2.0)))
;; huge counts overflow i32 and stay general (lazy, not a LongRange plan)
(println (vec (take 3 (map (fn [j] (- j)) (range 9223372036854775800)))))
;; empty shapes answer like the empty list the value path builds
(println (rsum2 5 0) (rfilter 0) (rsome 0) (revery 0) (rcount 0))
;; errors match: a bad bound throws the same way either path
(println (try (reduce (fn [a j] a) 0 (range :bad)) (catch Exception e :bad-range)))
(println (try (mapv inc (range 0 :bad)) (catch Exception e :bad-bound)))
