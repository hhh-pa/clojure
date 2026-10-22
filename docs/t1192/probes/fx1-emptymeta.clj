;; T1192 FX1 emptymeta probes (oracle: stock Clojure 1.12.6).
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.12.6 jars> clojure.main fx1-emptymeta.clj
(println (meta (with-meta (seq [1 2]) {:a 1})))
(println (with-meta (seq [1 2]) {:a 1}))
(println (meta (with-meta (range 3) {:a 1})))
(println (meta (with-meta (with-meta (seq [1]) {:a 1}) nil)))
(println (meta (vary-meta (with-meta (seq [1]) {:a 1}) assoc :b 2)))
(println (empty (with-meta (seq [1 2]) {:a 1})))
(println (meta (empty (with-meta (seq [1 2]) {:a 1}))))
(println (meta (empty (with-meta [1] {:a 1}))))
