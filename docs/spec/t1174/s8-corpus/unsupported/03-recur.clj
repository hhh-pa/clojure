(defn g [n] (if (= n 0) 0 (recur (+ n -1))))
(print (g 3))
