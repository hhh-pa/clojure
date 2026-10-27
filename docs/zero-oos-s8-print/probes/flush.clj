;; Zero-oos S8 (issue #51) oracle probes: *flush-on-newline* root + flush timing (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/flush.clj > docs/zero-oos-s8-print/probes/flush.out
(require 'clojure.string)
;; Flush-counting *out*: PrintWriter-on fires flush-fn per .flush() with a
;; non-empty buffer. The body runs, the count is snapshotted, then one manual
;; flush drains the BufferedWriter so the content is observable.
(defn fprobe [label thunk]
  (let [flushes (atom 0) sw (java.io.StringWriter.)
        pw (PrintWriter-on (fn [s] (.write sw ^String s) (swap! flushes inc)) nil)]
    (binding [*out* pw] (thunk))
    (let [n @flushes]
      (.flush ^java.io.PrintWriter pw)
      (println (str label " [" n " " (pr-str (str sw)) "]")))))
(println (str "f-root " (pr-str *flush-on-newline*)))
(println (str "f-dynamic " (pr-str (.isDynamic #'*flush-on-newline*))))
(println (str "f-sep " (pr-str (System/getProperty "line.separator"))))
(fprobe "f-newline" #(newline))
(fprobe "f-newline-false" #(binding [*flush-on-newline* false] (newline)))
(fprobe "f-prn" #(prn 1))
(fprobe "f-prn-false" #(binding [*flush-on-newline* false] (prn 1)))
(fprobe "f-println" #(println 1))
(fprobe "f-println-false" #(binding [*flush-on-newline* false] (println 1)))
(fprobe "f-pr" #(pr 1))
(fprobe "f-print" #(print 1))
(fprobe "f-prn-empty" #(prn))
(fprobe "f-multi" #(prn 1 2))
(fprobe "f-newline-then-prn" #(do (newline) (prn 1)))
(println "S8-flush DONE")
