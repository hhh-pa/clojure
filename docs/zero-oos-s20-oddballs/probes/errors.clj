;; S20 oddballs oracle probes: Throwable->map / StackTraceElement->vec shapes (oracle: alpha10 jars, issue #55).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s20-oddballs/probes/errors.clj > docs/zero-oos-s20-oddballs/probes/errors.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause class only (messages carry JDK/file-path volatility).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))]
        (println (str label " THROW " (.getName (class c))))))))
;; slot: a frame slot's class tag (frame contents are volatile; the shape is the pin).
(defn slot [x]
  (cond (symbol? x) "sym" (string? x) "str" (integer? x) "int" (nil? x) "nil" :else "?"))
(defn frame-shape [f]
  (pr-str [(vector? f) (count f) (mapv slot f)]))
;; ---- A: single ex-info ----
(def m-a (Throwable->map (ex-info "boom" {:a 1})))
(println "a-keys" (pr-str (sort (map str (keys m-a)))))
(println "a-cause" (pr-str (:cause m-a)))
(println "a-via-count" (pr-str (count (:via m-a))))
(println "a-via0-keys" (pr-str (sort (map str (keys (first (:via m-a)))))))
(println "a-via0-type" (pr-str (:type (first (:via m-a)))))
(println "a-via0-message" (pr-str (:message (first (:via m-a)))))
(println "a-via0-data" (pr-str (:data (first (:via m-a)))))
(println "a-via0-at" (frame-shape (:at (first (:via m-a)))))
(println "a-trace-vec" (pr-str (vector? (:trace m-a))))
(println "a-trace-shape" (frame-shape (first (:trace m-a))))
(println "a-at-is-trace0" (pr-str (= (:at (first (:via m-a))) (first (:trace m-a)))))
(println "a-trace-all-frames" (pr-str (every? #(and (vector? %) (= 4 (count %))) (:trace m-a))))
;; ---- B: chained cause (outer + inner) ----
(def m-b (Throwable->map (ex-info "outer" {:a 1} (ex-info "inner" {:b 2}))))
(println "b-cause" (pr-str (:cause m-b)))
(println "b-via-count" (pr-str (count (:via m-b))))
(println "b-via-msgs" (pr-str (mapv :message (:via m-b))))
(println "b-via-types" (pr-str (mapv :type (:via m-b))))
(println "b-via-data" (pr-str (mapv :data (:via m-b))))
(println "b-at-is-trace0" (pr-str (= (:at (first (:via m-b))) (first (:trace m-b)))))
;; ---- C: nil message, nil data ----
(def m-c (Throwable->map (ex-info nil nil)))
(println "c-cause" (pr-str (:cause m-c)))
(println "c-via0-keys" (pr-str (sort (map str (keys (first (:via m-c)))))))
(println "c-via0-message" (pr-str (:message (first (:via m-c)))))
;; ---- D: non-ex-info throwable (no data) ----
(def m-d (Throwable->map (IllegalArgumentException. "bad")))
(println "d-via0-keys" (pr-str (sort (map str (keys (first (:via m-d)))))))
(println "d-via0-type" (pr-str (:type (first (:via m-d)))))
(println "d-cause" (pr-str (:cause m-d)))
;; ---- E: StackTraceElement->vec ----
(println "e-ste" (frame-shape (StackTraceElement->vec (first (.getStackTrace (Exception. "x"))))))
;; ---- F: error behavior (classes only) ----
(throwcause "f-map-nonstr" '(Throwable->map 1))
(throwcause "f-map-nil" '(Throwable->map nil))
(throwcause "f-map-arity0" '(Throwable->map))
(throwcause "f-map-arity2" '(Throwable->map (ex-info "x" {}) 1))
(throwcause "f-ste-nonstr" '(StackTraceElement->vec "x"))
(throwcause "f-ste-nil" '(StackTraceElement->vec nil))
(throwcause "f-ste-arity0" '(StackTraceElement->vec))
;; ---- G: nil/cast messages (verbatim pins for the port NPE texts) ----
(defn throwmsg [label form]
  (try
    (eval form)
    (println (str label " NO-THROW"))
    (catch Throwable t
      (println (str label " " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwmsg "g-map-nil" '(Throwable->map nil))
(throwmsg "g-ste-nil" '(StackTraceElement->vec nil))
(throwmsg "g-map-cce" '(Throwable->map 1))
(throwmsg "g-ste-cce" '(StackTraceElement->vec "x"))
