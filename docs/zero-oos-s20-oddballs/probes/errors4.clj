;; S20 oddballs oracle probes, round 4: normalization site + trace identity + nil-msg plain throw (oracle: alpha10 jars, issue #55).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s20-oddballs/probes/errors4.clj > docs/zero-oos-s20-oddballs/probes/errors4.out
(println "l-raw-data" (pr-str (.getData (ex-info nil nil))))
(println "l-exdata-plain" (pr-str (ex-data (Exception. "x"))))
(def l-root (ex-info "r" {:b 2}))
(def l-j (Throwable->map (ex-info "o" {:a 1} (Exception. "mid" l-root))))
(println "l-trace-is-root-stack" (pr-str (= (mapv StackTraceElement->vec (.getStackTrace l-root)) (:trace l-j))))
(def l-n (Throwable->map (Exception.)))
(println "l-keys-n" (pr-str (sort (map str (keys l-n)))))
(println "l-via0-keys-n" (pr-str (sort (map str (keys (first (:via l-n)))))))
