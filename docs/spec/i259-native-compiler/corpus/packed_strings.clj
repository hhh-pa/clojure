;; Compact PackedStrings transient/vector storage and fast clojure.string/join (#271).
(require '[clojure.string :as str])

(defn build-strs [n]
  (let [t (transient [])]
    (loop [i 0]
      (if (= i n)
        (persistent! t)
        (do (conj! t (str "row-" i))
            (recur (inc i)))))))

(let [v (build-strs 300)
      g (vec (for [i (range 300)] (str "row-" i)))]
  (println (count v) (first v) (peek v) (nth v 0) (nth v 255) (nth v 299) (nth v 500 :oob) (get v 10) (get v 500 :nf))
  (println (= v g) (= (hash v) (hash g)))
  (println (count (str/join "\n" v)) (= (str/join "," v) (str/join "," g)))
  (println (= (apply str v) (apply str g)))
  (println (take 3 (rseq v)) (subvec v 254 258))
  (let [v2 (conj v "tail")
        v3 (pop v2)
        v4 (assoc v 100 :kw)]
    (println (count v2) (peek v2) (= v3 v) (nth v4 100) (nth v4 99)))
  (let [t (transient v)]
    (conj! t "more")
    (pop! t)
    (println (= (persistent! t) v))))
(println (subs "hello" 0 5) (subs "éà" 0 2))
