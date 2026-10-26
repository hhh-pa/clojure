;; T1192 FX2 MapEntry probe 20 (nested-let native-destructure rows).
;; The analyzer intercepts `let` before macros, so a *nested* `let`
;; destructures natively (eval.rs bind_map), while a top-level `let`
;; lowers through the destructure macro. Both spellings must agree.
;; Oracle: clojure 1.13.0-alpha9 + spec.alpha 0.6.249 + core.specs.alpha 0.6.133-alpha10.
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha9/clojure-1.13.0-alpha9.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.133-alpha10/core.specs.alpha-0.6.133-alpha10.jar clojure.main docs/t1192/probes/fx2-mapentry20.clj > docs/t1192/probes/fx2-mapentry20.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "nest-mapdest-0" '(do (let [{x 0} (first {:a 1})] x)))
(throwof "nest-mapdest-1" '(do (let [{x 1} (first {:a 1})] x)))
(throwof "nest-mapdest-5" '(do (let [{x 5} (first {:a 1})] x)))
(throwof "nest-mapdest-neg" '(do (let [{x -1} (first {:a 1})] x)))
(throwof "nest-mapdest-kw" '(do (let [{x :k} (first {:a 1})] x)))
(throwof "nest-vec-3rd" '(do (let [[a b c] (first {:x 1})] c)))
(throwof "nest-vec-rest" '(do (let [[a & r] (first {:a 1})] [a r])))
(throwof "nest-all-entry" '(do (let [{{:keys [a] :or {a 1}} :m, :all all} (first {:m {:a 5}})] all)))
(throwof "nest-all-vec" '(do (let [{w :w, :or {w 0}, :all a} [(first {:a 1})]] a)))
(throwof "invoke-kw-entry" '(:a (first {:a 1})))
(throwof "fnbody-mapdest-0" '((fn [e] (let [{x 0} e] x)) (first {:a 1})))
