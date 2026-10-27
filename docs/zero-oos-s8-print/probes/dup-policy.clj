;; Zero-oos S8 (issue #51) oracle probes: *print-dup* policy (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/dup-policy.clj > docs/zero-oos-s8-print/probes/dup-policy.out
(require 'clojure.string)
(defn norm [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")
      (clojure.string/replace #"eval\d+" "evalN")
      (clojure.string/replace #"--\d+" "--N")
      (clojure.string/replace #"__\d+" "__N")))
(defn dupof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (norm (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (norm (str (.getMessage ^Throwable c)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; A print-dup method on a keyword dispatch value: print-dup dispatches on
;; class only, so this method is NEVER consulted (row p-type-ignored).
(defmethod print-dup ::pdup-mine [o w] (.write ^java.io.Writer w "SHOULD-NOT-APPEAR"))
(defrecord PP [x y])
(def pdup-x 1)
;; ---- root ----
(dupof "p-root" '(pr-str *print-dup*))
;; ---- *print-length* / *print-level* are ignored under dup ----
(dupof "p-length" '(binding [*print-dup* true *print-length* 1] (pr-str [1 2 3])))
(dupof "p-level" '(binding [*print-dup* true *print-level* 1] (pr-str [[1] [2]])))
(dupof "p-length-ctor" '(binding [*print-dup* true *print-length* 0] (pr-str {:a 1 :b 2})))
;; ---- meta ALWAYS prints under dup (no *print-meta* needed) ----
(dupof "p-meta" '(binding [*print-dup* true] (pr-str (with-meta [1] {:a 1}))))
(dupof "p-meta-tag" '(binding [*print-dup* true] (pr-str (with-meta [1] {:tag String}))))
(dupof "p-record-meta" '(binding [*print-dup* true] (pr-str (with-meta (->PP 1 2) {:a 1}))))
;; print-dup Var prints NO meta (its method skips print-meta).
(dupof "p-var-meta" '(binding [*print-dup* true] (pr-str #'pdup-x)))
;; ---- nesting: ctor args print with print-dup recursively ----
(dupof "p-nested" '(binding [*print-dup* true] (pr-str [{:a 1}])))
(dupof "p-nested-map" '(binding [*print-dup* true] (pr-str {:a [1]})))
(dupof "p-nested-vec-in-set" '(binding [*print-dup* true] (pr-str (sorted-set [1]))))
;; ---- dup beats *print-readably* false ----
(dupof "p-str-unreadable" '(binding [*print-dup* true *print-readably* false] (pr-str "hi")))
(dupof "p-char-unreadable" '(binding [*print-dup* true *print-readably* false] (pr-str \a)))
(dupof "p-print-str" '(binding [*print-dup* true] (print-str [1 2])))
(dupof "p-print-str-meta" '(binding [*print-dup* true] (print-str (with-meta [1] {:a 1}))))
;; ---- print-dup ignores meta :type (class dispatch only) ----
(dupof "p-type-ignored" '(binding [*print-dup* true] (pr-str (with-meta [1] {:type ::pdup-mine}))))
;; ---- controls (dup off) ----
(dupof "p-false-map" '(binding [*print-dup* false] (pr-str {:a 1})))
(dupof "p-false-meta" '(binding [*print-dup* false] (pr-str (with-meta [1] {:a 1}))))
(println "S8-dup-policy DONE")
