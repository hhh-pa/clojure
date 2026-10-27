;; S20 oddballs oracle probes, round 3: ex-data nil rule + 3-chain data/trace attribution (oracle: alpha10 jars, issue #55).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s20-oddballs/probes/errors3.clj > docs/zero-oos-s20-oddballs/probes/errors3.out
;; ---- I: nil-data ex-info ----
(println "i-exdata-nilnil" (pr-str (ex-data (ex-info nil nil))))
(println "i-exdata-mnil" (pr-str (ex-data (ex-info "m" nil))))
(println "i-via-data-mnil" (pr-str (:data (first (:via (Throwable->map (ex-info "m" nil)))))))
(println "i-top-data-mnil" (pr-str (:data (Throwable->map (ex-info "m" nil)))))
(println "i-top-keys-mnil" (pr-str (sort (map str (keys (Throwable->map (ex-info "m" nil)))))))
;; ---- J: 3-chain, ExceptionInfo root ----
(def j (Throwable->map (ex-info "o" {:a 1} (Exception. "mid" (ex-info "r" {:b 2})))))
(println "j-cause" (pr-str (:cause j)))
(println "j-top-data" (pr-str (:data j)))
(println "j-via-msgs" (pr-str (mapv :message (:via j))))
(println "j-via-data-keys" (pr-str (mapv #(contains? % :data) (:via j))))
(println "j-via-data" (pr-str (mapv :data (:via j))))
(println "j-trace0-which" (pr-str (mapv #(= % (first (:trace j))) (mapv :at (:via j)))))
;; ---- K: 3-chain, plain root ----
(def k (Throwable->map (ex-info "o" {:a 1} (Exception. "mid" (RuntimeException. "rt")))))
(println "k-cause" (pr-str (:cause k)))
(println "k-top-keys" (pr-str (sort (map str (keys k)))))
(println "k-top-data" (pr-str (:data k)))
(println "k-via-data-keys" (pr-str (mapv #(contains? % :data) (:via k))))
(println "k-trace0-which" (pr-str (mapv #(= % (first (:trace k))) (mapv :at (:via k)))))
