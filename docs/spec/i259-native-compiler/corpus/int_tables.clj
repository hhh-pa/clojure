;; Closed integer tables a `def` computes from literal ranges fold at build
;; time into the longs the program builds them from (#271 Slice 6g); every
;; other shape, and any init the fold cannot finish, runs as written.
(defn box-index [r c] (+ (* (quot r 3) 3) (quot c 3)))
(def cell-row (mapv #(quot % 9) (range 81)))
(def cell-col (mapv #(rem % 9) (range 0 81 1)))
(def evens (mapv (fn [i] (* 2 i)) (range 10 0 -2)))
(def masks
  (mapv (fn [m]
          (loop [d 1 acc []]
            (if (> d 4)
              acc
              (if (zero? (bit-and m (bit-shift-left 1 d)))
                (recur (inc d) acc)
                (recur (inc d) (conj acc d))))))
        (range 32)))
(def counts (mapv count masks))
(def widths (mapv (fn [row] (let [n (count row)] (if (not (zero? n)) (dec n) -1))) masks))
(println cell-row cell-col evens)
(println masks counts widths)
;; Empty rows are the one `[]` they came from; equal rows stay distinct.
(println (identical? (nth masks 0) (nth masks 1)) (identical? (nth masks 2) (nth masks 3))
         (= (nth masks 2) (nth masks 3)) (identical? (nth masks 0) []) (meta (nth masks 2))
         (conj (nth masks 2) :k) (conj (nth masks 0) 1.5) (class (nth masks 4)))
(def mixed (mapv (fn [i] (let [v (if (odd? i) (conj [] i) [])] (do (count []) v))) (range 4)))
(println mixed (identical? (nth mixed 0) (nth mixed 2)))

;; Kept as written: a program fn, a string, a double, a bool, a pooled
;; literal, two `[]` sites, a non-literal bound, an enclosing local, an
;; empty range, rows over a table, an overflow, an effect, a long loop.
(def cell-box (mapv #(box-index (quot % 9) (rem % 9)) (range 9)))
(def strs (mapv (fn [i] (str i)) (range 3)))
(def halves (mapv (fn [i] (/ i 2.0)) (range 3)))
(def flags (mapv (fn [i] (odd? i)) (range 3)))
(def pairs (mapv (fn [i] [1 2]) (range 2)))
(def two-sites (mapv (fn [i] (if (odd? i) [] [])) (range 4)))
(def n 3)
(def by-var (mapv (fn [i] (* i i)) (range n)))
(def outer (let [k 5] (mapv (fn [i] (+ i k)) (range 3))))
(def none (mapv (fn [i] i) (range 0)))
(def same-rows (mapv identity masks))
(def big (try (mapv (fn [i] (* i 4611686018427387904)) (range 3)) (catch ArithmeticException e :overflow)))
(def big2 (try (mapv (fn [i] (* i 4611686018427387904)) (range 3)) (catch ArithmeticException e (ex-message e))))
(def noisy (mapv (fn [i] (println "init" i) i) (range 2)))
(def slow (mapv (fn [i] (loop [k 0] (if (< k 20000) (recur (inc k)) (+ k i)))) (range 300)))
(def huge (mapv (fn [i] (loop [k 0 acc []] (if (< k 300) (recur (inc k) (conj acc k)) acc))) (range 300)))
(def odd-conj (mapv (fn [i] (loop [d 0 acc []] (if (> d 1) acc (recur (inc d) (conj acc (str d)))))) (range 2)))
(def lets (mapv (fn [i] (let [[a] [i]] a)) (range 2)))
(def strs2 (mapv (fn [i] (clojure.string/blank? "x")) (range 2)))
(println cell-box strs halves flags pairs (identical? (nth pairs 0) (nth pairs 1)) two-sites
         (identical? (nth two-sites 0) (nth two-sites 1)) by-var outer none big big2 noisy
         (count slow) (peek slow) (count huge) (count (peek huge)) odd-conj lets strs2)
(println (identical? (nth same-rows 2) (nth masks 2)))
;; Over a flat table, a nil, a non-long operand, a literal collection.
(def doubled (mapv (fn [x] (* 2 x)) evens))
(def nils (mapv (fn [i] (let [x nil] (if x 0 i))) (range 3)))
(def bad-arg (try (mapv (fn [i] (+ (odd? i) 1)) (range 2)) (catch Exception e :cast)))
(def over-lit (mapv inc [1 2 3]))
(def eqs (mapv (fn [i] (if (= (odd? i) true) 1 0)) (range 3)))
(println doubled nils bad-arg over-lit eqs)
