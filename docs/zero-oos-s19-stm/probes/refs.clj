;; S19 oracle probes: ref construction, options, accessors (oracle: alpha10 jars, issue #63).
;; Run from repo root (/home/hhu/workspace/clojure-s19):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s19-stm/probes/refs.clj > docs/zero-oos-s19-stm/probes/refs.out
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
(defn showof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
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
;; ---- construction ----
(showof "r-new" '(ref 0))
(showof "r-new-nil" '(ref nil))
(showof "r-new-coll" '(ref {:a [1 2]}))
(throwof "r-meta-opt" '(meta (ref 0 :meta {:k 1})))
(throwof "r-meta-none" '(meta (ref 0)))
(throwof "r-min-default" '(ref-min-history (ref 0)))
(throwof "r-max-default" '(ref-max-history (ref 0)))
(throwof "r-min-opt" '(ref-min-history (ref 0 :min-history 5)))
(throwof "r-max-opt" '(ref-max-history (ref 0 :max-history 7)))
(throwof "r-min-set" '(let [r (ref 0)] [(identical? (ref-min-history r 3) r) (ref-min-history r)]))
(throwof "r-max-set" '(let [r (ref 0)] [(identical? (ref-max-history r 7) r) (ref-max-history r)]))
(throwof "r-max-below-min" '(let [r (ref 0 :min-history 5)] (ref-max-history r 3) [(ref-min-history r) (ref-max-history r)]))
(throwof "r-min-above-max" '(let [r (ref 0 :max-history 7)] (ref-min-history r 9) [(ref-min-history r) (ref-max-history r)]))
(throwof "r-min-negative" '(let [r (ref 0)] (ref-min-history r -1) (ref-min-history r)))
(throwof "r-max-negative" '(let [r (ref 0)] (ref-max-history r -1) (ref-max-history r)))
(throwof "r-history-init" '(ref-history-count (ref 0)))
(throwof "r-history-after" '(let [r (ref 0)] (dosync (alter r inc)) (ref-history-count r)))
(throwof "r-history-after2" '(let [r (ref 0)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
(throwof "r-history-min-keeps" '(let [r (ref 0 :min-history 5)] (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
(throwof "r-history-max-trims" '(let [r (ref 0 :min-history 0 :max-history 1)] (dosync (alter r inc)) (dosync (alter r inc)) (dosync (alter r inc)) (ref-history-count r)))
(throwof "r-history-default-many" '(let [r (ref 0)] (dotimes [_ 15] (dosync (alter r inc))) (ref-history-count r)))
(throwof "r-history-min3-many" '(let [r (ref 0 :min-history 3)] (dotimes [_ 8] (dosync (alter r inc))) (ref-history-count r)))
;; ---- deref / predicates ----
(throwof "r-deref" '(deref (ref 5)))
(throwof "r-deref-reader" '@(ref 5))
(throwof "r-deref-timeout" '(deref (ref 5) 100 :t))
(throwof "r-realized" '(realized? (ref 0)))
(throwof "r-realized-atom" '(realized? (atom 0)))
(throwof "r-realized-num" '(realized? 5))
(throwof "r-eq-same" '(let [r (ref 0)] (= r r)))
(throwof "r-eq-distinct" '(= (ref 0) (ref 0)))
(throwof "r-identical" '(let [r (ref 0)] (identical? r r)))
(throwof "r-counted" '(counted? (ref 0)))
(throwof "r-ifn" '(ifn? (ref 0)))
(throwof "r-invoke0" '((ref 5)))
(throwcause "r-invoke1" '((ref 5) 1))
(throwof "r-invoke-fnval" '((ref (fn [a b] (+ a b))) 1 2))
(throwof "r-invoke-variadic" '(apply (ref +) (range 1 22)))
(throwof "r-atom-invoke" '((atom (fn [a b] (+ a b))) 1 2))
(throwof "r-atom-ifn" '(ifn? (atom 0)))
(showof "r-str" '(str (ref 0)))
(throwcause "r-with-meta" '(with-meta (ref 0) {:a 1}))
;; ---- options edge cases ----
(throwcause "r-odd-opts" '(ref 0 :validator))
(throwof "r-bad-opt-key" '(deref (ref 0 :bogus 1)))
(throwof "r-validator-nil-opt" '(let [r (ref 0 :validator nil)] (dosync (alter r (constantly :x)))))
(throwcause "r-validator-construct" '(ref 3 :validator even?))
(throwof "r-validator-reject" '(let [r (ref 0 :validator even?)] [(try (dosync (alter r (constantly 3))) :committed (catch IllegalStateException e [:thrown (.getMessage e)])) @r]))
