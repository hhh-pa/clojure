;; Loop/recur: countdown, accumulation, rebinding order, nesting, defn.
(print (loop [i 0] (if (= i 3) i (recur (+ i 1)))))
(print (loop [i 1 acc 0] (if (> i 100) acc (recur (+ i 1) (+ acc i)))))
(print (loop [a 1 b 2] (if (= a 2) [a b] (recur b a))))
(print (loop [i 0] (if (= i 2) (loop [j 10] (if (= j 12) (+ i j) (recur (+ j 1)))) (recur (+ i 1)))))
(defn countdown [n] (loop [i n] (if (= i 0) :done (recur (+ i -1)))))
(print (countdown 5))
