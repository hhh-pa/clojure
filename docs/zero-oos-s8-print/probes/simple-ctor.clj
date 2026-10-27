;; Zero-oos S8 (issue #51) oracle probes: print-simple / print-ctor (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/simple-ctor.clj > docs/zero-oos-s8-print/probes/simple-ctor.out
(require 'clojure.string)
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (str (.getMessage ^Throwable c))
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- print-simple: shape ----
(throwof "s-arglists" '(:arglists (meta #'print-simple)))
(throwof "s-fn" '[(fn? print-simple) (var? #'print-simple)])
;; ---- print-simple: writes (str o) (+ print-meta prefix) to w ----
(throwof "s-str" '(let [sw (java.io.StringWriter.)] (print-simple "hi" sw) (str sw)))
(throwof "s-kw" '(let [sw (java.io.StringWriter.)] (print-simple :a sw) (str sw)))
(throwof "s-vec" '(let [sw (java.io.StringWriter.)] (print-simple [1 2] sw) (str sw)))
(throwof "s-onto-out" '(with-out-str (print-simple "hi" *out*)))
(throwof "s-meta" '(binding [*print-meta* true] (let [sw (java.io.StringWriter.)] (print-simple (with-meta [1] {:a 1}) sw) (str sw))))
(throwof "s-meta-off" '(let [sw (java.io.StringWriter.)] (print-simple (with-meta [1] {:a 1}) sw) (str sw)))
(throwof "s-meta-dup" '(binding [*print-dup* true] (let [sw (java.io.StringWriter.)] (print-simple (with-meta [1] {:a 1}) sw) (str sw))))
(throwof "s-meta-unreadable" '(binding [*print-meta* true *print-readably* false] (let [sw (java.io.StringWriter.)] (print-simple (with-meta [1] {:a 1}) sw) (str sw))))
(throwof "s-ret" '(let [sw (java.io.StringWriter.)] (print-simple 1 sw)))
;; ---- print-simple: errors ----
(throwof "s-arity0" '(print-simple))
(throwof "s-arity1" '(let [sw (java.io.StringWriter.)] (print-simple 1)))
(throwof "s-arity3" '(let [sw (java.io.StringWriter.)] (print-simple 1 sw :x)))
(throwof "s-cce" '(print-simple 1 "nope"))
(throwof "s-nilw" '(print-simple 1 nil))
;; ---- print-ctor: shape ----
(throwof "t-arglists" '(:arglists (meta #'print-ctor)))
(throwof "t-fn" '[(fn? print-ctor) (var? #'print-ctor)])
;; ---- print-ctor: #=(Class. <print-args o w>) ----
(throwof "t-basic" '(let [sw (java.io.StringWriter.)] (print-ctor 5 (fn [o w] (.write ^java.io.Writer w "ARGS")) sw) (str sw)))
(throwof "t-passes-o" '(let [sw (java.io.StringWriter.)] (print-ctor 5 (fn [o w] (.write ^java.io.Writer w (str o))) sw) (str sw)))
(throwof "t-writer-is-w" '(let [sw (java.io.StringWriter.)] (print-ctor 5 (fn [o w] (.write ^java.io.Writer w (if (identical? w sw) "SAME" "DIFF"))) sw) (str sw)))
(throwof "t-ret" '(let [sw (java.io.StringWriter.)] (print-ctor 5 (fn [o w]) sw)))
;; ---- print-ctor: errors ----
(throwof "t-arity" '(print-ctor 5))
(throwof "t-cce" '(print-ctor 5 (fn [o w]) "nope"))
(throwof "t-nilw" '(print-ctor 5 (fn [o w]) nil))
(throwof "t-nilo" '(let [sw (java.io.StringWriter.)] (print-ctor nil (fn [o w]) sw) (str sw)))
;; ---- the two multis are plain (non-dynamic) roots ----
(throwof "s-pm-nondynamic" '(binding [print-method 5] 1))
(throwof "s-pd-nondynamic" '(binding [print-dup 5] 1))
(println "S8-simple-ctor DONE")
