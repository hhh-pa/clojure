;; T1192 FX1 alter probes (oracle: stock Clojure 1.13.0-alpha9).
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;        -cp <1.13.0-alpha9 jars> clojure.main fx1-alter.clj
(def avr-c 0)
;; A. same-thread nested alter of one var: the monitor re-enters,
;;    inner installs 100, then the outer install (1000) wins.
(println "nested:"
  (alter-var-root #'avr-c (fn [x] (alter-var-root #'avr-c (fn [_] 100)) (+ x 1000)))
  avr-c)
;; B. two threads serialize: f1 holds the monitor (sleeps inside f),
;;    f2 blocks in alter until f1 installs root 1, then sees 1 -> 2.
(def avr-r (atom []))
(def avr-f1 (future (alter-var-root #'avr-c
                      (fn [x] (swap! avr-r conj :f1-in) (Thread/sleep 1500)
                                  (swap! avr-r conj :f1-out) 1))))
(Thread/sleep 500)
(def avr-f2 (future (alter-var-root #'avr-c
                      (fn [x] (swap! avr-r conj :f2-in) (+ x 10)))))
(println "ser:" @avr-f1 @avr-f2 avr-c @avr-r)
