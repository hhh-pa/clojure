;; Zero-oos S8 (issue #51) oracle probes: clojure.core/str under print bindings
;; (oracle: alpha10 jars). These rows do NOT change native str (out of S8
;; scope); they define the (str o) half of print-simple, which honors the
;; ambient bindings.
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/str-semantics.clj > docs/zero-oos-s8-print/probes/str-semantics.out
(require 'clojure.string)
(defn norm [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(defn sof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (norm (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str (norm (str (.getMessage ^Throwable c))))))))))
(sof "s2-meta" '(binding [*print-meta* true] (str (with-meta [1] {:a 1}))))
(sof "s2-dup-map" '(binding [*print-dup* true] (str {:a 1})))
(sof "s2-dup-vec" '(binding [*print-dup* true] (str [1 2])))
(sof "s2-length" '(binding [*print-length* 1] (str [1 2])))
(sof "s2-level" '(binding [*print-level* 1] (str [[1] [2]])))
(sof "s2-str-raw" '(binding [*print-meta* true] (str "hi")))
(sof "s2-dup-str" '(binding [*print-dup* true] (str "hi")))
(sof "s2-atom" '(binding [*print-meta* true] (str (atom 1 :meta {:a 1}))))
(sof "s2-readably" '(binding [*print-readably* false] (str [1 "a"])))
(sof "s2-cons" '(str (cons 1 [2])))
(sof "s2-range" '(str (range 3)))
(sof "s2-lazyseq" '(str (map inc [1 2])))
;; print-simple bypasses user methods (its (str o) half is RT.printString).
(deftype ST [x])
(defmethod print-method ST [o w] (.write ^java.io.Writer w "SHOULD-NOT-APPEAR"))
(sof "s2-simple-usermethod" '(let [sw (java.io.StringWriter.)] (print-simple (ST. 1) sw) (str sw)))
;; Nested user methods fire through collection toString (RT.print is live).
(deftype DT [x])
(defmethod print-method DT [o w] (.write ^java.io.Writer w "DT!"))
(sof "s2-str-nested-usermethod" '(str [(DT. 1)]))
(sof "s2-simple-nested-usermethod" '(let [sw (java.io.StringWriter.)] (print-simple [(DT. 1)] sw) (str sw)))
(sof "s2-seqlength" '(binding [*print-length* 1] (str (cons 1 [2 3]))))
(sof "s2-mapentry-str" '(str (first {"a" 1})))
(println "S8-str-semantics DONE")
