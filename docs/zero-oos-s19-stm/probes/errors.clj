;; S19 oracle probes: STM error texts and arities (oracle: alpha10 jars, issue #63).
;; Run from repo root (/home/hhu/workspace/clojure-s19):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s19-stm/probes/errors.clj > docs/zero-oos-s19-stm/probes/errors.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- outside-transaction enforcement ----
(throwof "e-alter-outside" '(alter (ref 0) inc))
(throwof "e-refset-outside" '(ref-set (ref 0) 9))
(throwof "e-commute-outside" '(commute (ref 0) + 1))
(throwof "e-ensure-outside" '(ensure (ref 7)))
;; ---- check order: transaction gate vs ref cast ----
(throwof "e-alter-nil-outside" '(alter nil inc))
(throwof "e-alter-nonref-outside" '(alter 1 inc))
(throwcause "e-alter-nil-in" '(dosync (alter nil inc)))
(throwcause "e-alter-nonref-in" '(dosync (alter 1 inc)))
(throwcause "e-refset-nil-in" '(dosync (ref-set nil 1)))
(throwcause "e-refset-nonref-in" '(dosync (ref-set 1 9)))
(throwcause "e-commute-nil-in" '(dosync (commute nil + 1)))
(throwcause "e-commute-nonref-in" '(dosync (commute 1 + 1)))
(throwcause "e-ensure-nil-in" '(dosync (ensure nil)))
(throwcause "e-ensure-nonref-in" '(dosync (ensure 1)))
;; ---- fn checks ----
(throwcause "e-alter-nonfn" '(dosync (alter (ref 0) 5)))
(throwcause "e-commute-nonfn" '(dosync (commute (ref 0) 5)))
(throwof "e-alter-nonfn-outside" '(alter (ref 0) 5))
(throwof "e-commute-nonfn-outside" '(commute (ref 0) 5))
(throwof "e-alter-nil-nonfn" '(alter nil 5))
;; ---- arities ----
(throwcause "e-alter-arity" '(dosync (alter (ref 0))))
(throwcause "e-commute-arity" '(dosync (commute (ref 0))))
(throwcause "e-refset-arity" '(dosync (ref-set (ref 0))))
(throwcause "e-ensure-arity0" '(dosync (ensure)))
(throwcause "e-ensure-arity2" '(dosync (ensure (ref 0) (ref 1))))
(throwcause "e-count-arity0" '(ref-history-count))
(throwcause "e-count-arity2" '(ref-history-count (ref 0) 1))
(throwcause "e-min-arity0" '(ref-min-history))
(throwcause "e-min-arity3" '(ref-min-history (ref 0) 1 2))
(throwcause "e-max-arity0" '(ref-max-history))
(throwcause "e-max-arity3" '(ref-max-history (ref 0) 1 2))
;; ---- accessor type checks ----
(throwcause "e-min-nonref" '(ref-min-history 5))
(throwcause "e-max-nonref" '(ref-max-history 5))
(throwcause "e-count-nonref" '(ref-history-count 5))
(throwcause "e-count-nil" '(ref-history-count nil))
(throwcause "e-min-get-nil" '(ref-min-history nil))
(throwcause "e-max-get-nil" '(ref-max-history nil))
(throwcause "e-min-nonnum" '(ref-min-history (ref 0) :x))
(throwcause "e-max-nonnum" '(ref-max-history (ref 0) :x))
(throwof "e-min-double" '(let [r (ref 0)] (ref-min-history r 3.9) (ref-min-history r)))
(throwof "e-max-double" '(let [r (ref 0)] (ref-max-history r 3.9) (ref-max-history r)))
(throwcause "e-min-nil" '(ref-min-history (ref 0) nil))
(throwcause "e-max-nil" '(ref-max-history (ref 0) nil))
(throwof "e-min-huge" '(let [r (ref 0)] (ref-min-history r 1e300) (ref-min-history r)))
(throwof "e-min-negbig" '(let [r (ref 0)] (ref-min-history r -1e300) (ref-min-history r)))
(throwof "e-min-nan" '(let [r (ref 0)] (ref-min-history r ##NaN) (ref-min-history r)))
(throwof "e-min-longbig" '(let [r (ref 0)] (ref-min-history r 3000000000) (ref-min-history r)))
(throwof "e-min-char" '(let [r (ref 0)] (ref-min-history r \a) (ref-min-history r)))
(throwof "e-min-bool" '(let [r (ref 0)] (ref-min-history r true) (ref-min-history r)))
(throwof "e-min-bigint" '(let [r (ref 0)] (ref-min-history r 5N) (ref-min-history r)))
(throwof "e-min-str" '(let [r (ref 0)] (ref-min-history r "3") (ref-min-history r)))
(throwof "e-min-ratio" '(let [r (ref 0)] (ref-min-history r 7/2) (ref-min-history r)))
(throwof "e-min-bigdec" '(let [r (ref 0)] (ref-min-history r 7.9M) (ref-min-history r)))
(throwof "e-min-negratio" '(let [r (ref 0)] (ref-min-history r -7/2) (ref-min-history r)))
(throwof "e-min-bigint-big" '(let [r (ref 0)] (ref-min-history r 9999999999999999999999N) (ref-min-history r)))
(throwof "e-min-negdouble" '(let [r (ref 0)] (ref-min-history r -3.9) (ref-min-history r)))
(throwof "e-min-doublebig" '(let [r (ref 0)] (ref-min-history r 3e9) (ref-min-history r)))
;; ---- validator / throw passthrough ----
(throwcause "e-validator-msg" '(let [r (ref 0 :validator even?)] (dosync (alter r (constantly 3)))))
(throwof "e-dosync-passthrough" '(try (dosync (throw (ex-info "boom" {:k 1}))) :no (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)])))
