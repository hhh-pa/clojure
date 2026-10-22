;; T1192 FX1 time-digits probe (oracle: stock Clojure 1.13.0-alpha9).
;; The `time` macro prints (str (/ (double NANOS) 1000000.0));
;; fixed NANOS rows make the digits deterministic.
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.13.0-alpha9 jars> clojure.main fx1-time.clj
(doseq [n [0 1 10 100 999 1000 1234 2341699 123456789 999999500000000 9007199254740993 123456789012345678]]
  (println n "=>" (pr-str (str (/ (double n) 1000000.0)))))
(println "live:" (pr-str (with-out-str (time (+ 1 2)))))
