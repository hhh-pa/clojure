;; Scalar loops hoisting flat int vectors and locking unaliased assoc!-only
;; transient int vectors across iterations (#271).
(defn knapsack [weights values cap]
  (let [n (count weights)
        dp (transient (vec (repeat (inc cap) 0)))]
    (loop [i 0]
      (if (< i n)
        (let [wi (nth weights i)
              vi (nth values i)]
          (loop [w cap]
            (when (>= w wi)
              (let [cand (+ (nth dp (- w wi)) vi)
                    cur (nth dp w)]
                (when (> cand cur)
                  (assoc! dp w cand)))
              (recur (dec w))))
          (recur (inc i)))
        (nth (persistent! dp) cap)))))

(defn aliased-update [a b]
  (loop [i 0 acc 0]
    (if (< i (count a))
      (do
        (assoc! a i (+ (nth b i) 10))
        (recur (inc i) (+ acc (nth b i))))
      acc)))

(defn assoc-append-and-mixed [t]
  (loop [i 0]
    (if (< i 5)
      (do
        (assoc! t i (if (= i 3) \a (* i i)))
        (recur (inc i)))
      (persistent! t))))

(defn read-only-transient [t]
  (loop [i 0 acc 0]
    (if (< i (count t))
      (recur (inc i) (+ acc (nth t i)))
      acc)))

(defn oob-unlocks [t]
  (try
    (loop [i 0 acc 0]
      (if (< i 5)
        (do
          (assoc! t 0 i)
          (recur (inc i) (+ acc (nth t i))))
        acc))
    (catch Exception _
      (persistent! t))))

(defn deopt-unlocks [t n]
  (loop [i 0 acc 0]
    (if (< i n)
      (do
        (assoc! t i i)
        (recur (inc i) (+ acc (nth t i) 9223372036854775800)))
      acc)))

(println (knapsack [2 3 4 5] [3 4 5 6] 5))
(let [t (transient [1 2 3 4])]
  (println (aliased-update t t) (persistent! t)))
(println (assoc-append-and-mixed (transient [0 0])))
(let [t (transient [10 20 30 40])]
  (println (read-only-transient t) (persistent! t)))
(println (oob-unlocks (transient [1 2])))
(let [t (transient [0 0 0])]
  (println (try (deopt-unlocks t 3) (catch Exception _ :ovf)) (persistent! t)))
