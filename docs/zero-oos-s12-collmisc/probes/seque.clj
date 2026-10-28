;; S12 oracle probes: seque (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/seque.clj > docs/zero-oos-s12-collmisc/probes/seque.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (str)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
(throwof "sq-basic" '(pr-str (doall (seque [1 2 3]))))
(throwof "sq-class" '(str (class (seque [1 2]))))
(throwof "sq-cq" '(chunked-seq? (seque [1 2])))
(throwof "sq-nil-elems" '(pr-str (doall (seque [1 nil 3]))))
(throwof "sq-empty" '(pr-str (doall (seque []))))
(throwof "sq-empty-seq" '(nil? (seq (seque []))))
(throwof "sq-nil-input" '(pr-str (doall (seque nil))))
(throwof "sq-cap" '(pr-str (doall (seque 2 (range 10)))))
(throwof "sq-queue" '(pr-str (doall (seque (java.util.concurrent.LinkedBlockingQueue. 2) (range 5)))))
(throwcause "sq-zero-cap" '(doall (seque 0 [1])))
(throwof "sq-take" '(pr-str (take 3 (seque (range 100)))))
(throwof "sq-count" '(count (seque (range 50))))
(throwof "sq-inf" '(pr-str (take 5 (seque (map inc (range))))))
(throwcause "sq-neg-cap" '(doall (seque -1 [1])))
(throwcause "sq-cap-str" '(doall (seque "x" [1])))
(throwof "sq-cap-float" '(pr-str (doall (seque 2.9 (range 5)))))
(shutdown-agents)
