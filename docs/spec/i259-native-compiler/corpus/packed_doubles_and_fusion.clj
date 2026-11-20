;; Corpus check for Slice 5k (#271): packed f64 vectors/transients, dvec/dtvec
;; scalar lowering, zero-alloc vector compare/eq/hash, SubVector/SplitVec HOF
;; iteration, and drop/take-while/drop-while/collect_all fusion.
(require '[clojure.string :as str])

(defn scan-max [q base na]
  (loop [a 1 bv (nth q base)]
    (if (>= a na)
      bv
      (let [v (nth q (+ base a))]
        (recur (inc a) (if (> v bv) v bv))))))

(defn update-dtvec! [q n]
  (loop [i 0 acc 0.0]
    (if (< i n)
      (let [v (+ (nth q i) acc)]
        (assoc! q i v)
        (recur (inc i) (+ acc 0.5)))
      (persistent! q))))

(defn grow-dtvec! [q n x]
  (loop [i 0 acc (double x)]
    (if (< i n)
      (do
        (conj! q acc)
        (assoc! q 0 acc)
        (recur (inc i) (+ acc 1.0)))
      (persistent! q))))

;; 1. Packed Double vectors, repeat, mapv parseDouble, streaming print-join
(let [rep (vec (repeat 5 0.25))
      parsed (mapv #(Double/parseDouble %) (str/split "1.5 2.5 -0.5 4.0" #" "))
      mapped (mapv (fn [x] (* x 2.0)) parsed)]
  (println (str/join "," rep))
  (println (str/join " " parsed))
  (println (str/join "," mapped))
  (println (= rep [0.25 0.25 0.25 0.25 0.25])
           (= (hash rep) (hash '(0.25 0.25 0.25 0.25 0.25)))
           (compare [1.0 2.0] [1.0 3.0])
           (compare [1.0 2.0] [1 2])))

;; 2. Scalar loops over immutable dvec, locked dtvec, growing dtvec, and mixed widening
(let [dv [1.5 4.25 2.0 3.75]
      tv (transient (vec (repeat 4 1.0)))
      _ (println (scan-max dv 0 4) (scan-max tv 0 4))
      u (update-dtvec! tv 4)
      g (grow-dtvec! (transient [0.0]) 3 2)]
  (println u g)
  ;; Widening a packed Double transient to Generic preserves semantics
  (let [w (transient [1.0 2.0])]
    (assoc! w 1 :k)
    (println (persistent! w))))

;; 3. Zero-alloc vector compare/sort & vector hash/eq in sets/maps
(let [pairs [[2 [1 3 2]] [1 [2 1 0]] [1 [1 2 3]] [2 [1 2 9]] [1 [1 2 3]]]
      sorted (vec (sort pairs))
      s (set (map second pairs))
      big1 (vec (range 40))
      big2 (vec (range 40))]
  (println sorted (count s) (contains? s [1 2 3]) (= big1 big2) (= (hash big1) (hash big2))))

;; 4. Dot products across packed Doubles, packed Ints, and mixed Generic
(let [dv1 [1.5 2.5 3.5]
      dv2 [2.0 4.0 6.0]
      iv [2 4 6]
      mv [1.5 4 3.5]]
  (println (reduce + 0.0 (map * dv1 dv2))
           (reduce + 0.0 (map * dv1 iv))
           (reduce + 0.0 (map * iv dv2))
           (reduce + 0.0 (map * mv iv))))

;; 5. SubVector and SplitVec inside compiled HOF loops
(let [sv (subvec [10 20 30 40 50] 1 4)
      sp (str/split "10 20 30 40" #" ")]
  (println (mapv inc sv)
           (filterv #(> % 20) sv)
           (reduce + 0 (map #(* % 2) sv))
           (every? pos? sv)
           (some #(when (= % 30) :ok) sv)
           (mapv #(Long/parseLong %) sp)
           (filterv #(not= % "20") sp)))

;; 6. Drop / take-while / drop-while / collect_all fusion
(println (vec (drop 2 [1 2 3 4 5]))
         (vec (take-while #(< % 4) [1 2 3 4 5]))
         (vec (drop-while #(< % 3) [1 2 3 4 2]))
         (set (map inc (drop 1 [10 20 30 20])))
         (vec (sort (drop 1 [5 3 1 4 2])))
         (vec (reverse (take-while #(< % 4) [1 2 3 4 5])))
         (frequencies (drop-while zero? [0 0 1 2 1])))
