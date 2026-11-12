;; An uncaught ex-info escaping a defn aborts with the call-site span.
(defn explode [x] (throw (ex-info "kaboom" {:x x})))
(println :before)
(explode 1)
(println :never)
