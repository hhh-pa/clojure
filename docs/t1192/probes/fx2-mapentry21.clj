;; T1192 FX2 MapEntry probe 21 (conj-into-map-of-seq rows).
;; Oracle: clojure 1.13.0-alpha9 + spec.alpha 0.6.249 + core.specs.alpha 0.6.133-alpha10.
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha9/clojure-1.13.0-alpha9.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar clojure.main docs/t1192/probes/fx2-mapentry21.clj > docs/t1192/probes/fx2-mapentry21.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "seq-elem-all" '(let [e (first {:m (list 1 2)})] (let [{{:keys [a] :or {a 1}} :m, :all all} e] all)))
(throwof "seq-elem-all-flat" '(let [{{:keys [a] :or {a 1}} :m, :all all} (first {:m (list 1 2)})] all))
(throwof "seq-in-vec-all" '(let [e [(list (first {:a 1}))]] (let [{w :w, :or {w 0}, :all a} e] a)))
