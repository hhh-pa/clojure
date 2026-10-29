;; S20 oddballs oracle probes, round 2: top-level :data rule + chained :at (oracle: alpha10 jars, issue #55).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s20-oddballs/probes/errors2.clj > docs/zero-oos-s20-oddballs/probes/errors2.out
(defn normframe [f]
  (mapv (fn [x] (cond (symbol? x) (str "sym:" x)
                      (string? x) "str"
                      (integer? x) "int"
                      (nil? x) "nil" :else "?"))
        f))
(def m-a (Throwable->map (ex-info "boom" {:a 1})))
(def m-b (Throwable->map (ex-info "outer" {:a 1} (ex-info "inner" {:b 2}))))
(def m-c (Throwable->map (ex-info nil nil)))
(def m-d (Throwable->map (IllegalArgumentException. "bad")))
;; ---- G: top-level :data rule ----
(println "g-keys-b" (pr-str (sort (map str (keys m-b)))))
(println "g-keys-c" (pr-str (sort (map str (keys m-c)))))
(println "g-keys-d" (pr-str (sort (map str (keys m-d)))))
(println "g-data-a" (pr-str (:data m-a)))
(println "g-data-b" (pr-str (:data m-b)))
(println "g-data-c" (pr-str (:data m-c)))
(println "g-data-d" (pr-str (:data m-d)))
(println "g-cause-key-c" (pr-str (contains? m-c :cause)))
(println "g-via0-data-c" (pr-str (:data (first (:via m-c)))))
;; ---- H: chained :at vs :trace ----
(println "h-via0-at" (pr-str (normframe (:at (first (:via m-b))))))
(println "h-via1-at" (pr-str (normframe (:at (second (:via m-b))))))
(println "h-trace0" (pr-str (normframe (first (:trace m-b)))))
(println "h-trace-count" (pr-str (count (:trace m-b))))
(println "h-via1-at-is-trace0" (pr-str (= (:at (second (:via m-b))) (first (:trace m-b)))))
(println "h-via-count-a" (pr-str (count (:via m-a))))
