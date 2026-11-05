(print (loop [i 0] (if (= i 3) i (recur (+ i 1)))))
