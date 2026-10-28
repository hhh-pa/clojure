;; S16 oracle probes: PrintWriter-on (oracle: alpha10 jars, issue #60).
;; Run from repo root (/home/hhu/workspace/clojure-s16):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s16-classemu/probes/writers.clj > docs/zero-oos-s16-classemu/probes/writers.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"__\d+" "__N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(defn showhex [label form]
  (println (str label " " (normhex (pr-str (eval form))))))
;; ---- A: shape ----
(throwof "w-class" '(class (PrintWriter-on prn nil)))
(showhex "w-print" '(PrintWriter-on prn nil))
(throwof "w-arity3-class" '(class (PrintWriter-on prn nil true)))
(throwcause "w-arity1" '(PrintWriter-on prn))
(throwcause "w-arity4" '(PrintWriter-on prn nil true 4))
;; ---- B: behavior ----
(throwof "w-write-flush" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw] (print "hi") (.flush ^java.io.Writer pw)) @acc))
(throwof "w-flush-empty" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (.flush ^java.io.Writer pw) @acc))
(throwof "w-close" '(let [acc (atom []) closed (atom false) pw (PrintWriter-on (fn [s] (swap! acc conj s)) (fn [] (reset! closed true)))] (binding [*out* pw] (print "x")) (.close ^java.io.Writer pw) [@acc @closed]))
(throwof "w-close-nil" '(let [pw (PrintWriter-on prn nil)] (.close ^java.io.Writer pw) :closed))
(throwof "w-println" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (println "a")) (.flush ^java.io.Writer pw) @acc))
(throwof "w-flush-default" '(boolean *flush-on-newline*))
(throwof "w-noflush-noauto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (println "a")) @acc))
(throwof "w-noflush-auto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (binding [*out* pw *flush-on-newline* false] (println "a")) @acc))
(throwof "w-flush-noauto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* true] (println "a")) @acc))
(throwof "w-print-newline-auto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (binding [*out* pw *flush-on-newline* false] (print "a\n")) @acc))
(throwof "w-flush-call" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw] (print "z") (flush)) @acc))
(throwof "w-prn-to" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (binding [*out* pw *flush-on-newline* false] (prn {:a 1})) (.flush ^java.io.Writer pw) @acc))
(throwof "w-nonfn-construct" '(let [pw (PrintWriter-on 1 nil)] (class pw)))
(throwcause "w-nonfn-flush" '(let [pw (PrintWriter-on 1 nil)] (binding [*out* pw] (print "x")) (.flush ^java.io.Writer pw)))
(throwof "w-close-twice" '(let [n (atom 0) pw (PrintWriter-on prn (fn [] (swap! n inc)))] (.close ^java.io.Writer pw) (.close ^java.io.Writer pw) @n))
(throwof "w-write-after-close" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (.close ^java.io.Writer pw) (binding [*out* pw] (print "q")) (.flush ^java.io.Writer pw) @acc))
(throwof "w-direct-println-auto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (.println ^java.io.PrintWriter pw "a") @acc))
(throwof "w-direct-println-noauto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil)] (.println ^java.io.PrintWriter pw "a") @acc))
(throwof "w-direct-print-auto" '(let [acc (atom []) pw (PrintWriter-on (fn [s] (swap! acc conj s)) nil true)] (.print ^java.io.PrintWriter pw "a") @acc))
(throwof "w-flush-ret" '(let [pw (PrintWriter-on prn nil)] (.flush ^java.io.Writer pw)))
(throwof "w-close-ret" '(let [pw (PrintWriter-on prn nil)] (.close ^java.io.Writer pw)))
