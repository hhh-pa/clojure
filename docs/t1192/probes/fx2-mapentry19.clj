;; T1192 FX2 MapEntry probe 19 (PR #20 review round 1 follow-up rows).
;; Oracle: clojure 1.13.0-alpha9 + spec.alpha 0.6.249 + core.specs.alpha 0.6.133-alpha10.
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha9/clojure-1.13.0-alpha9.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar clojure.main docs/t1192/probes/fx2-mapentry19.clj > docs/t1192/probes/fx2-mapentry19.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "mapdest-5" '(let [{x 5} (first {:a 1})] x))
(throwof "mapdest-neg" '(let [{x -1} (first {:a 1})] x))
(throwof "mapdest-missing" '(let [{x :nope} (first {:a 1})] x))
(throwof "fn-empty" '(fn ()))
(throwof "get-entry-0" '(get (first {:a 1}) 0))
(throwof "get-entry-1" '(get (first {:a 1}) 1))
(throwof "get-entry-5" '(get (first {:a 1}) 5))
(throwof "conj-entry-coll" '(conj (first {:a 1}) :b))
(throwof "conj-entry-elem" '(conj {:x 0} (first {:a 1})))
(throwof "conj-entry-seq" '(conj {:x 0} (list (first {:a 1}) (first {:b 2}))))
(throwof "merge-entry-left" '(merge (first {:a 1}) {:b 2}))
(throwof "merge-entry-seq" '(merge (first {:a 1}) (quote (:b))))
(throwof "seq-entry" '(seq (first {:a 1})))
(throwof "into-entry" '(into [] (first {:a 1})))
