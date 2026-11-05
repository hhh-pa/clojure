;; T1192 FX1 remainder probes (oracle: stock Clojure 1.13.0-alpha9).
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.13.0-alpha9 jars> clojure.main fx1-rest.clj
(println "tsneg:" (pr-str (format "%ts" -1)) (pr-str (format "%ts" -1001)) (pr-str (format "%ts" -1000)))
(println "cce-update-transient:"
  (try (update (transient {:a 1}) :a inc)
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "cce-keys-vec:"
  (try (first (keys [1 2]))
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "cce-keys-next-first:"
  (try (first (next (keys [1 2])))
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "cce-keys-rest-first:"
  (try (first (rest (keys [1 2])))
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "cce-keys-str:"
  (try (first (keys ["ab"]))
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "cce-update-vec:"
  (try (update [1 2] 0 inc)
       (catch ClassCastException e (str "CCE|msg=" (pr-str (.getMessage e))))))
(println "nth-1.0:" (pr-str (nth [1 2 3] 1.0)))
(println "nth-huge-f64:"
  (try (nth [1] 9.007199254740992E15)
       (catch ArithmeticException e (str "AE|msg=" (pr-str (.getMessage e))))))
(println "plus-overflow:"
  (try (+ 9223372036854775807 1)
       (catch ArithmeticException e (str "AE|msg=" (pr-str (.getMessage e))))))
(println "hash-repeat:"
  (try (hash (repeat 1))
       (catch UnsupportedOperationException e (str "UOE|msg=" (pr-str (.getMessage e))))))
