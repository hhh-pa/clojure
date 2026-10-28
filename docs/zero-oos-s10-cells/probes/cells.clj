;; S10 oracle probes: volatile cells (oracle: alpha10 jars, issue #53).
;; Run from repo root (/home/hhu/workspace/clojure-s10):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s10-cells/probes/cells.clj > docs/zero-oos-s10-cells/probes/cells.out
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
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
(defn showhex [label form]
  (println (str label " " (normhex (pr-str (eval form))))))
;; ---- construction / printing / predicate ----
(showhex "c-new" '(volatile! 0))
(showhex "c-new-nil" '(volatile! nil))
(showhex "c-new-coll" '(volatile! {:a [1 2]}))
(throwof "c-pred-t" '(volatile? (volatile! 1)))
(throwof "c-pred-atom" '(volatile? (atom 1)))
(throwof "c-pred-int" '(volatile? 1))
(throwof "c-pred-nil" '(volatile? nil))
(throwof "c-pred-str" '(volatile? "x"))
;; ---- deref ----
(throwof "c-deref" '(deref (volatile! 5)))
(throwof "c-deref-reader" '(do @(volatile! 7)))
(throwcause "c-deref-timeout" '(deref (volatile! 1) 100 :t))
(throwcause "c-deref-nil" '(deref nil))
;; ---- vreset! ----
(throwof "c-vreset" '(let [v (volatile! 0)] [(vreset! v 9) @v]))
(throwof "c-vreset-nil" '(let [v (volatile! 0)] [(vreset! v nil) @v]))
(throwcause "c-vreset-nonvol" '(vreset! 1 2))
(throwcause "c-vreset-nil-vol" '(vreset! nil 2))
;; ---- vswap! ----
(throwof "c-vswap" '(let [v (volatile! 0)] [(vswap! v + 5) @v]))
(throwof "c-vswap-multi" '(let [v (volatile! 1)] [(vswap! v + 1 2 3) @v]))
(throwof "c-vswap-noargs" '(let [v (volatile! 1)] [(vswap! v inc) @v]))
(throwcause "c-vswap-nonfn" '(let [v (volatile! 1)] (vswap! v 5)))
(throwcause "c-vswap-nonvol" '(vswap! (atom 1) inc))
(throwof "c-shape" '(macroexpand-1 '(vswap! v + 5)))
(throwof "c-shape-multi" '(macroexpand-1 '(vswap! v f 1 2)))
(throwof "c-shape-noargs" '(macroexpand-1 '(vswap! v inc)))
(throwof "c-double-eval" '(let [c (atom 0) v (volatile! 1)] (vswap! (do (swap! c inc) v) inc) [@v @c]))
;; ---- identity semantics ----
(throwof "c-eq-same" '(let [v (volatile! 1)] (= v v)))
(throwof "c-eq-distinct" '(= (volatile! 1) (volatile! 1)))
(throwof "c-identical" '(let [v (volatile! 1)] (identical? v v)))
(throwof "c-meta" '(meta (volatile! 1)))
(throwof "c-ifn" '(ifn? (volatile! 1)))
(throwof "c-counted" '(counted? (volatile! 1)))
