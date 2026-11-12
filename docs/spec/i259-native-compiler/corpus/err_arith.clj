;; An inline-arithmetic overflow escaping to the top level.
(defn big [n] (* n n n))
(println (big 10))
(big 9223372036854775)
