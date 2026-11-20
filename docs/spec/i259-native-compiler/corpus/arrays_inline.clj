;; aget/aset inline ops (#271): in-bounds primitive slots taking their own
;; value type store and read in place; every other shape keeps the native.
(defn msg [f] (try (f) (catch Exception e (str (.getSimpleName (class e)) ": " (.getMessage e)))))
(let [l (long-array 3) i (int-array 3) d (double-array 3) b (boolean-array 2) c (char-array 2)]
  (dotimes [k 3] (aset l k (* k 10)) (aset i k (- k)) (aset d k (/ k 2.0)))
  (aset b 1 true)
  (aset c 0 \z)
  (println (vec l) (vec i) (vec d) (vec b) (vec c))
  (println (aget l 2) (aget i 1) (aget d 1) (aget b 1) (aget c 0) (aset l 0 7) (aget l 0))
  (println (msg #(aset i 0 3000000000)) (msg #(aset l -1 1)) (msg #(aset l 3 1)) (msg #(aget d 3))
           (msg #(aget d -1)) (msg #(aset d 0 1)) (msg #(aset l 0 1.5)) (msg #(aset b 0 1)))
  (println (msg #(aget nil 0)) (msg #(aset nil 0 1)) (msg #(aget [1 2] 0)) (aget l (int 1)) (aget d 2.0)))
(let [o (object-array 2) s (into-array String ["a" "b"]) g (to-array-2d [[0 0] [0 0]])]
  (aset o 0 :k)
  (aset g 1 0 5)
  (println (aget o 0) (aget s 1) (msg #(aset s 0 1)) (aget g 1 0) (vec (aget g 1)))
  (println (reduce + (map #(aget g % 0) [0 1])) (alength g)))
(def sieve (boolean-array 50 true))
(doseq [p (range 2 8) :when (aget sieve p) m (range (* p p) 50 p)] (aset sieve m false))
(println (filter #(aget sieve %) (range 2 50)))
