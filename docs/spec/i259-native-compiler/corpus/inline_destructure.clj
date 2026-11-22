;; Slice 5p: zero-alloc vector-literal destructuring elision and small leaf
;; stable-defn inlining (both single-value and destructured call sites).

(defn leaf0 [] [10 20])
(defn leaf1 [x] (unchecked-add (unchecked-multiply x 3) 1))
(defn pair-fn [x y] (let [s (+ x y)] [s (- x y)]))
(defn vec-param [[a b] c] (+ a (* b c)))
(defn map-param [{:keys [x y]}] (+ x y))
(defn map-or-param [{:keys [x] :or {x 9}}] (+ x 1))
(defn set-map-fn [a b] {:s #{a b} :k a})

;; Multi-level destructuring inline (mirrors q_learning/approx_inference)
(defn step-pair [x]
  (let [n (leaf1 x)]
    [n (rem n 7)]))

(defn branch-pair [flag x fallback]
  (let [[a b] (step-pair x)]
    (if flag
      (pair-fn a b)
      (if (= x 99) fallback [b a]))))

;; Non-scalar loop inside destructuring callee (zero exit-vector alloc)
(defn loop-pair [xs]
  (let [n (count xs)]
    (loop [i 0 acc ""]
      (if (= i n)
        [i acc]
        (recur (inc i) (str acc (nth xs i)))))))

;; Scalar loop in let destructuring (keeps scalar loop intact)
(defn scalar-loop-destr [n]
  (let [[i sum] (loop [i 0 s 0]
                  (if (< i n)
                    (recur (inc i) (+ s i))
                    [i s]))]
    (+ i sum)))

;; Recursive and forward-declared callees must NOT inline
(declare mut-b)
(defn mut-a [n] (if (pos? n) (mut-b (dec n)) 0))
(defn mut-b [n] (+ n (mut-a n)))
(defn self-rec [n] (if (pos? n) (+ n (self-rec (dec n))) 0))
(defn recur-arity [n acc] (if (zero? n) acc (recur (dec n) (+ acc n))))
(defn with-fn [xs] (mapv (fn [x] (inc x)) xs))

;; Direct let destructuring shapes (nested, partial if, do, let, empty do, non-inline invoke)
(defn destr-shapes [flag v]
  (let [[a b] [1 2]
        [[c d] e] [[3 4] 5]
        [f g] (if flag [6 7] v)
        [h i] (if flag [8 9] (do))
        [j k] (do (leaf1 1) [10 11])
        [l m] (let [z 12] [z (inc z)])
        [p q] (leaf0)
        [r s] (if flag (step-pair 5) (with-fn [1 2]))
        [t u] (branch-pair flag 4 [99 98])
        [w x] (loop-pair ["a" "b" "c"])
        [y z] (if flag [a a] [b b])]
    [a b c d e f g h i j k l m p q r s t u w x y z]))

(println (destr-shapes true [100 200]))
(println (destr-shapes false [100 200]))
(println (scalar-loop-destr 10))
(println (leaf1 (leaf1 4)) (vec-param [2 3] 5) (map-param {:x 4 :y 6}) (map-or-param {}))
(println (set-map-fn 1 2) (mut-b 3) (self-rec 5) (recur-arity 5 0))

;; Borrowed vs moved last-use args & side-effect order
(defn take-both [u v] [(conj u 9) (count v)])
(let [v1 [1 2]
      v2 [3 4]
      [r1 r2] (take-both v1 v1)
      [r3 r4] (take-both v2 v2)]
  (println v1 r1 r2 r3 r4))

;; Evaluation order & exceptions in inlined calls
(let [log (atom [])]
  (println
   (try
     (pair-fn (do (swap! log conj :a) 1)
              (do (swap! log conj :b) (throw (ex-info "boom" {}))))
     (catch Exception _ @log))))
(println (try (pair-fn 1 "bad") (catch Exception _ :type-err)))
