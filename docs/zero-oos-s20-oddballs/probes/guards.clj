;; S20 oddballs oracle probes: guard-row verdicts + *repl* root (oracle: alpha10 jars, issue #55).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s20-oddballs/probes/guards.clj > docs/zero-oos-s20-oddballs/probes/guards.out
;; ---- guard verdicts (absence => inventory repair by removal; presence => implement) ----
(println "g-clone" (pr-str (find-var 'clojure.core/clone-thread-bindings)))
(println "g-pprint-public" (pr-str (contains? (ns-publics 'clojure.core) 'pprint)))
(println "g-pprint-var" (pr-str (find-var 'clojure.core/pprint)))
;; ---- *repl* (expected: present, false root) ----
(println "r-resolves" (pr-str (some? (find-var 'clojure.core/*repl*))))
(println "r-root" (pr-str @#'clojure.core/*repl*))
(println "r-dynamic" (pr-str (-> #'clojure.core/*repl* meta :dynamic)))
(println "r-bound" (pr-str (bound? #'clojure.core/*repl*)))
(println "r-macro" (pr-str (-> #'clojure.core/*repl* meta :macro)))
