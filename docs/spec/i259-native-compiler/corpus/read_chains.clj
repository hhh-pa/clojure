;; Borrowing read chains (#271): an inner read feeding a read op yields
;; a pointer into its collection; every other shape reads the general way.
(def grid [[1 2 3] [4 5 6]])
(def g {:adj [[1] [0 2] [1]] :w {:a 1}})
(defn cell [m r c] (nth (nth m r) c))
(defn deg [g v] (count (nth (get g :adj) v)))
(defn deep [x i j k] (nth (nth (nth x i) j) k))
(println (cell grid 1 2) (deg g 1) (deep [[[1 2] [3 4]] [[5 6]]] 0 1 0))
(println (count (get g :adj)) (first (nth grid 1)) (peek (nth grid 0)) (contains? (get g :w) :a))
(println (get (get g :w) :a) (get (get g :w) :z :none) (nth (nth grid 0) 9 :oob) (get (get g :nope) :a))
(println (cell '[[7 8] [9 10]] 1 0) (nth (nth [[:x]] 0) 0) (get (nth [{:k 1}] 0) :k))
;; Misses: packed rows, a large map, nil, wrong types, out of range.
(def big (zipmap (range 40) (map vector (range 40))))
(println (nth (get big 7) 0) (count (get big 41)) (get (nth [nil] 0) :a))
(println (try (cell grid 5 0) (catch Exception e :oob)) (try (nth (nth grid 0) 5) (catch Exception e :oob2)))
(println (try (nth (nth {:a 1} 0) 0) (catch Exception e :uns)) (try (count (nth [1 2] 0)) (catch Exception e :cnt)))
;; The base local is moved by a later argument, or at its last use.
(defn later-move [v]
  (let [r (nth (nth v 0) 1)] [r (count (conj v [9]))]))
(defn last-use [v i] (nth (nth v i) 0))
(defn twice [v] [(count (nth v 0)) (count (nth v 1)) v])
(println (later-move [[1 2] [3]]) (last-use [[5] [6]] 1) (twice [[1 2] [3]]))
(defn loop-reads [m n]
  (loop [i 0 acc 0]
    (if (< i n) (recur (inc i) (+ acc (nth (nth m (rem i 2)) (rem i 3)))) acc)))
(println (loop-reads grid 10) (loop-reads [[1.5 2.5 3.5] [0.5 1.0 2.0]] 7))
(let [rows (vec (repeat 3 [1 2]))]
  (println (map (fn [i] (count (nth rows i))) (range 3)) rows))
;; A chain feeding `count`'s owned slow path (sorted sets, lazy seqs)
;; must leave the parent collection intact.
(defn sizes [doms n]
  (loop [i 0 acc []]
    (if (< i n) (recur (inc i) (conj acc (count (nth doms (rem i 2))))) [acc doms])))
(println (sizes [(sorted-set 1 2 3) (sorted-set 4)] 4))
(println (sizes [(map inc [1 2]) (range 3)] 3))
