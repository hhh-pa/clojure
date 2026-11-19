;; Scalar loops whose exit reads only locals bound before the loop run that
;; exit boxed when the loop ends (#271): (persistent! acc) and friends.
(defn squares [n]
  (let [acc (transient [])]
    (loop [i 0]
      (if (= i n) (persistent! acc) (do (conj! acc (* i i)) (recur (inc i)))))))
(defn prefix [s]
  (let [m (count s) pi (transient (vec (repeat m 0)))]
    (loop [i 1 j 0]
      (if (>= i m)
        (persistent! pi)
        (let [ch (nth s i)
              j (if (= ch (nth s j)) (inc j) 0)]
          (assoc! pi i j)
          (recur (inc i) j))))))
(defn counted [n]
  (let [acc (transient [])]
    (loop [i 0]
      (cond (= i n) (count acc)
            (> i 50) {:stopped (count acc) :limit n}
            :else (do (conj! acc i) (recur (inc i)))))))
(defn uses-loop-var [n]
  (let [acc (transient [])]
    (loop [i 0]
      (if (= i n) [i (count acc)] (do (conj! acc i) (recur (inc i)))))))
(defn bad-exit [n]
  (let [acc (transient [])]
    (loop [i 0]
      (if (= i n) (persistent! (persistent! acc)) (do (conj! acc i) (recur (inc i)))))))
(println (squares 6) (prefix "aabaaab") (counted 5) (counted 80) (uses-loop-var 3))
(println (try (bad-exit 2) (catch Exception e (.getMessage e))))
(println (vec (take 5 (prefix (apply str (repeat 40 "ab"))))))
