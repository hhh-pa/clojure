;; Calling nil through a local.
(defn call-it [f] (f 1))
(println (call-it inc))
(call-it nil)
