;; An uncaught assertion failure.
(defn check [n] (assert (pos? n) "n must be positive") n)
(println (check 1))
(check -1)
