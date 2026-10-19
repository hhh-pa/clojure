;; S5 oracle probes: namespace resolution (S5A1) and top-level
;; `do` splitting in `eval` (S5A2/S5C).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "ns-missing" #(eval '(do foo234/bar 1)))
(tx "ns-novar" #(eval '(do (create-ns 'q234) (eval '(do q234/nope 1)))))
(tx "ns-alias-novar" #(eval '(do (alias 'qq234 'clojure.string) (eval '(do qq234/nope 1)))))
(tx "ns-alias-removed" #(eval '(do (create-ns 't234) (alias 'tt234 't234) (remove-ns 't234) (eval '(do tt234/x 1)))))
(tx "ns-refer-ok" #(eval '(do (refer 'clojure.string :only '[join]) (join ", " [1 2]))))
(tx "ns-crossdef" #(eval '(do (declare foo235/x) (def foo235/x 1) foo235/x)))
(tx "ns-crossdef-missing" #(eval '(def foo236/y 1)))
(tx "ns-intern-creates" #(eval '(do (intern 'nns237 'a 5) nns237/a)))
(tx "ns-intern-def" #(eval '(do (create-ns 'g235) (intern 'g235 'x) (def g235/x 1) g235/x)))
(tx "ns-removed-direct" #(eval '(do (create-ns 't236) (remove-ns 't236) (eval '(do t236/x 1)))))
(tx "ns-declare-q" #(eval '(do (declare dq235/x) [(contains? (ns-interns *ns*) 'dq235/x) (find-ns 'dq235)])))
(tx "ns-intern-basic" #(eval '(do (create-ns 'h238) (intern 'h238 'a) (intern 'h238 'b 7) [(eval 'h238/b) (var? (eval '(var h238/a)))])))
(tx "ns-alias-shadow" #(eval '(do (create-ns 'pa234) (intern 'pa234 'v 1) (alias 'pa234 'clojure.string) [(eval 'pa234/v) (try (eval 'pa234/join) (catch Throwable t :no-join))])))
(tx "ns-var-refer" #(eval '(do (refer 'clojure.string :only '[join]) (var join))))
(tx "ns-var-cross" #(eval '(var clojure.string/join)))
(tx "ns-eval-split-missing" #(eval 'nns999/a))
(tx "ns-eval-split-intern" #(eval '(do (intern 'nns999 'a 5) nns999/a)))
(tx "ns-eval-split-plain" #(eval '(do 1 nns999/a)))
(tx "ns-do-macro" #(eval '(do (defmacro mmm234 [] 42) (mmm234))))
(tx "ns-do-nested" #(eval '((fn [] (do (defmacro nnn234 [] 43) (nnn234))))))
(tx "ns-do-require" #(eval '(do (require '[clojure.string :as s234]) (s234/join ", " [1]))))
