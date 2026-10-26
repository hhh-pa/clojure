;; T1192 FX4 shadow probe (PR #22 review round 1, H1).
;; Operator position never defers to locals for these heads:
;; `new`/`letfn*` are real specials and the `monitor-*` macros beat
;; shadowing bindings too.
;; Oracle: clojure 1.13.0-alpha9 + spec.alpha 0.6.249 + core.specs.alpha 0.6.133-alpha10.
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha9/clojure-1.13.0-alpha9.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar clojure.main docs/t1192/probes/fx4-shadow.clj > docs/t1192/probes/fx4-shadow.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "shadow-new-object" '(let [new (fn [x] x)] (pr-str (new Object))))
(throwof "plain-new-object" '(pr-str (new Object)))
(throwof "shadow-monitor" '(let [monitor-enter (fn [x] x)] (locking 5 1)))
(throwof "plain-monitor" '(locking 5 1))
(throwof "shadow-letfn" '(let [letfn* (fn [x] x)] (letfn* 5)))
