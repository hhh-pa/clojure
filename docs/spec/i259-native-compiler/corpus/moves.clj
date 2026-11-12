;; Last-use moves in `recur` args: accumulators handed to core fns by move
;; (in place), next to the cases that must not move.
(defn build [n]
  (loop [i 0 acc []]
    (if (< i n) (recur (inc i) (conj acc i)) acc)))
(println (build 5))
(defn build-map [n]
  (loop [i 0 m {}]
    (if (< i n) (recur (inc i) (assoc m i (* i i))) m)))
(println (build-map 4))
(defn twice-used [n]
  ;; `acc` is used twice in the recur args: no move, same answer.
  (loop [i 0 acc []]
    (if (< i n) (recur (inc i) (conj acc (count acc))) acc)))
(println (twice-used 4))
(defn inner-let [n]
  ;; A local bound inside the loop body moves too.
  (loop [i 0 acc []]
    (if (< i n)
      (let [nxt (conj acc i)]
        (recur (inc i) (conj nxt (- i))))
      acc)))
(println (inner-let 3))
(defn many-args [n]
  (loop [i 0 acc []]
    (if (< i n) (recur (inc i) (conj acc i i i)) (count acc))))
(println (many-args 4))
(defn transient-move [n]
  (loop [i 0 t (transient [])]
    (if (< i n) (recur (inc i) (conj! t i)) (persistent! t))))
(println (transient-move 4))
(defn fn-recur [n acc]
  ;; A fn-level recur: params belong to the caller, never moved.
  (if (pos? n) (recur (dec n) (conj acc n)) acc))
(def shared [9])
(println (fn-recur 3 shared) shared)
(defn aliasing [v]
  ;; The loop's initial value is shared with the caller's `v`: a move
  ;; inside the loop must not disturb it.
  (loop [i 0 acc v]
    (if (< i 2) (recur (inc i) (conj acc i)) acc)))
(def base [:a])
(println (aliasing base) base)
(defn captured [n]
  ;; `acc` is captured by a closure in the recur args: no move.
  (loop [i 0 acc [] fs []]
    (if (< i n) (recur (inc i) (conj acc i) (conj fs (fn [] (count acc)))) (map #(%) fs))))
(println (captured 3))
(defn nested-loop [rows]
  ;; `row` and `n` appear once in the recur args, but inside an inner
  ;; loop that runs them every inner iteration: no move.
  (loop [k 0 acc []]
    (if (= k (count rows))
      acc
      (let [row (nth rows k)
            n (count row)]
        (recur (inc k)
               (loop [i 0 acc acc]
                 (if (= i n) acc (recur (inc i) (conj acc (nth row i))))))))))
(println (nested-loop [[1 2] [3 4 5]]))
(defn drop-two [s]
  ;; A one-argument core call taking the moved local.
  (loop [i 0 s s]
    (if (< i 2) (recur (inc i) (rest s)) s)))
(println (drop-two (list 1 2 3 4)))
(defn walk-pairs [[a b] n]
  ;; fn-level recur rebinding a destructured param (not a plain symbol).
  (if (pos? n) (recur [b (+ a b)] (dec n)) a))
(println (walk-pairs [0 1] 10))
