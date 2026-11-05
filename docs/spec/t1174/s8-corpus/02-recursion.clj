(defn fact [n] (if (= n 0) 1 (* n (fact (+ n -1)))))
(defn fib [n] (if (< n 2) n (+ (fib (+ n -1)) (fib (+ n -2)))))
(print "mark ")
(print [(fact 5) (fib 10)])
