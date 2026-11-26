;; Issue #271 Slice 5i: unboxed Double scalar loops, Math/* intrinsics/shims,
;; top-level scalar globals, generalized 2D (nth (nth m i) j), hoisted flat
;; generic Double vectors, and in-place Double transient assoc!.

(def G_ALPHA 0.25)
(def G_BETA 1.5)
(def ^:dynamic *dyn-shift* 0.125)

(defn garch-like [xs]
  (let [n (count xs)]
    (loop [i 0 s 0.0]
      (if (< i n)
        (let [x (nth xs i)
              v (+ (* G_ALPHA x x) (* G_BETA G_ALPHA) *dyn-shift*)]
          (recur (inc i) (+ s (Math/log (+ v 1.0)) (Math/sqrt (Math/abs v)))))
        s))))

(defn mat2d-sum [m rows cols]
  (loop [r 0 acc 0.0]
    (if (< r rows)
      (let [row-sum (loop [c 0 s 0.0]
                      (if (< c cols)
                        (recur (inc c) (+ s (nth (nth m r) c)))
                        s))]
        (recur (inc r) (+ acc row-sum)))
      acc)))

(defn mixed-edge-cases [xs]
  (let [n (count xs)]
    (loop [i 0
           mx (nth xs 0)
           mn (nth xs 0)
           s 0.0]
      (if (< i n)
        (let [v (nth xs i)
              ;; mixed Int/Double max/min preserve winning operand type
              m1 (max 5 2.0)
              m2 (max 2.0 5)
              m3 (max 1 1.0)
              m4 (min 1 2.0)
              m5 (max Double/NaN 1.0)
              m6 (max 1.0 Double/NaN)
              a (+ (inc v) (dec v) (- v) (/ v 2.0) (/ 4.0)
                   (Math/pow 2.0 3.0) (Math/exp 0.0) (abs -3) (abs -2.5))
              c (+ (long 3.9) (long -3.9) (int 2.7) (long Double/NaN) (int Double/NaN)
                   (double 7) (double 1.25))]
          (recur (inc i) (max mx v) (min mn v)
                 (+ s a c (if (and (= (type m1) (type 1))
                                   (= (type m2) (type 1))
                                   (= (type m3) (type 1.0))
                                   (= (type m4) (type 1))
                                   (Double/isNaN m5)
                                   (Double/isNaN m6))
                            1.0 0.0))))
        [mx mn s]))))

(defn eq-and-cmp []
  (let [xs [-0.0 0.0 Double/NaN 1.0]
        ys [0.0 -0.0 Double/NaN 1]]
    ;; Pure-int loop inits reading Double vector: must deopt cleanly for (= -0.0 0.0)!
    (let [eq-no-fp (loop [i 0 ok true]
                     (if (< i 2)
                       (recur (inc i) (and ok (= (nth xs i) (nth ys i))))
                       ok))
          eq-fp (loop [i 0 c 0.0]
                  (if (< i 4)
                    (let [a (nth xs i)
                          b (nth ys i)
                          d (+ (if (= a b) 1.0 0.0)
                               (if (== a b) 10.0 0.0)
                               (if (zero? a) 100.0 0.0)
                               (if (pos? a) 1000.0 0.0)
                               (if (neg? (- a 1.0)) 10000.0 0.0)
                               (if (<= a 1.0) 100000.0 0.0)
                               (if (>= a 0.0) 1000000.0 0.0))]
                      (recur (inc i) (+ c d)))
                    c))]
      [eq-no-fp eq-fp])))

(defn int-loop-with-abs [v]
  (let [n (count v)]
    (loop [i 0 s 0]
      (if (< i n)
        (recur (inc i) (+ s (Math/abs (- (nth v i) 5))))
        s))))

(defn double-transient-assoc []
  (let [t (transient [0.0 0.0 0.0 0.0])]
    (loop [i 0]
      (when (< i 4)
        (assoc! t i (* (+ i 1.0) 2.5))
        (recur (inc i))))
    (persistent! t)))

(defn deopt-checks []
  [;; integer division must deopt to Ratio
   (loop [i 0 r 0]
     (if (< i 2) (recur (inc i) (/ 1 2)) r))
   ;; out-of-range double->long and double->int throw identically to run
   (try
     (loop [i 0 r 0]
       (if (< i 1) (recur (inc i) (long 1e30)) r))
     (catch IllegalArgumentException _ :long-oor))
   (try
     (loop [i 0 r 0]
       (if (< i 1) (recur (inc i) (int 1e20)) r))
     (catch IllegalArgumentException _ :int-oor))
   (try
     (loop [i 0 r 0.0]
       (if (< i 1) (recur (inc i) (double \a)) r))
     (catch ClassCastException _ :dbl-char))
   ;; Math/abs on Long/MIN_VALUE wraps identically to JDK
   (loop [i 0 r 0]
     (if (< i 1) (recur (inc i) (Math/abs Long/MIN_VALUE)) r))
   ;; dynamic rebinding of *dyn-shift*
   (binding [*dyn-shift* 10.0] (garch-like [1.0 2.0]))])

(println (format "%.6f" (garch-like [1.0 -2.0 3.5 0.0])))
(println (format "%.6f" (mat2d-sum [[1.5 2.5] [3.5 4.5]] 2 2)))
(let [[mx mn s] (mixed-edge-cases [1.0 -3.5 4.25 0.0])]
  (println mx mn (format "%.6f" s)))
(println (eq-and-cmp))
(println (int-loop-with-abs [1 5 9 2]))
(println (double-transient-assoc))
(let [[rat loor ioor dch minv dynv] (deopt-checks)]
  (println rat loor ioor dch minv (format "%.6f" dynv)))
