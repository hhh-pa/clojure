;; T1200 D1 transducer probes (oracle: alpha10 jars).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp /Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar clojure.main docs/t1200/probes/xform.clj > docs/t1200/probes/xform.out
;; NOTE on paren depth: `(map inc)` takes `rf` first, so init is
;; `((map inc) +)` called with zero args = `(((map inc) +))`;
;; `(completing +)` takes `f` directly, so init is `((completing +))`.
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; 1-arg xform builders: init / complete / step protocol
(throwof "map-init" '(((map inc) +)))
(throwof "map-complete" '(((map inc) +) 0))
(throwof "map-step" '(((map inc) +) 0 5))
(throwof "filter-step-t" '(((filter odd?) +) 0 5))
(throwof "filter-step-f" '(((filter odd?) +) 0 4))
(throwof "remove-step" '(((remove odd?) +) 0 5))
(throwof "keep-step-nil" '(((keep #(when (odd? %) %)) +) 0 4))
(throwof "keep-step-v" '(((keep #(when (odd? %) %)) +) 0 5))
(throwof "keep-indexed-step" '(((keep-indexed #(when (odd? %2) %2)) +) 0 5))
(throwof "distinct-step" '(let [d ((distinct) +)] [(d 0 5) (d 0 5) (d 0 7)]))
(throwof "dedupe-step" '(let [d ((dedupe) +)] [(d 0 5) (d 0 5) (d 0 7)]))
;; audit: more 1-arg transducer overloads (take/drop families etc.)
(throwof "take-step" '(let [t ((take 2) +)] [(t 0 5) (t 0 6) (t 0 7)]))
(throwof "drop-step" '(let [d ((drop 2) +)] [(d 0 5) (d 0 6) (d 0 7)]))
(throwof "take-while-step" '(let [t ((take-while odd?) +)] [(t 0 5) (t 0 4)]))
(throwof "drop-while-step" '(let [d ((drop-while odd?) +)] [(d 0 5) (d 0 4) (d 0 7)]))
(throwof "take-nth-step" '(let [t ((take-nth 2) +)] [(t 0 5) (t 0 6) (t 0 7) (t 0 8)]))
(throwof "map-indexed-step" '(((map-indexed vector) conj) [] 5))
(throwof "interpose-step" '(let [i ((interpose \,) conj)] [(i [] 5) (i [] 6)]))
(throwof "mapcat-step" '(((mapcat range) conj) [] 3))
(throwof "cat-init" '((cat +)))
(throwof "cat-complete" '((cat +) 0))
(throwof "cat-step" '((cat +) 0 [1 2]))
;; completing (takes f directly: 2 paren levels)
(throwof "completing-init" '((completing +)))
(throwof "completing-complete" '((completing +) 5))
(throwof "completing-step" '((completing +) 5 6))
(throwof "completing-cf" '((completing + str) 5))
;; transduce arities + early termination
(throwof "sum" '(transduce (comp (map inc) (filter odd?)) + (range 10)))
(throwof "sum-init" '(transduce (map inc) + 100 [1 2 3]))
(throwof "sum-2arg" '(transduce (map inc) + [1 2 3]))
(throwof "halt" '(transduce (halt-when #(> % 5)) conj [] (range)))
(throwof "halt-retf" '(transduce (halt-when #(> % 5) (fn [r i] (conj r :halted))) conj [] (range)))
(throwof "halt-plus" '(transduce (halt-when #(> % 5)) + 0 (range)))
(throwof "halt-step" '(let [h ((halt-when odd?) +)] [(h 0 4) (h 0 5)]))
(throwof "halt-complete-plain" '(let [h ((halt-when odd?) +)] (h 7)))
(throwof "halt-complete-usermap" '(let [h ((halt-when odd?) +)] (h {:clojure.core/halt 9, :other 1})))
;; reduced helpers; preserving-reduced is private (not referred)
(throwof "ensure" '(ensure-reduced 5))
(throwof "ensure-r" '(ensure-reduced (reduced 5)))
(throwof "unreduced" '(unreduced (reduced 5)))
(throwof "resolve-preserving" '(resolve 'preserving-reduced))
(throwof "resolve-preserving-q" '(resolve 'clojure.core/preserving-reduced))
(throwof "preserving-unqualified" '(preserving-reduced +))
(throwof "preserving-qualified" '((clojure.core/preserving-reduced +) 5 6))
;; sequence / eduction / into
(throwof "into-xf" '(into [] (map inc) [1 2 3]))
(throwof "eduction-reduce3" '(reduce + 0 (eduction (map inc) [1 2 3])))
(throwof "eduction-reduce2" '(reduce + (eduction (map inc) [1 2 3])))
(throwof "eduction-seq" '(seq (eduction (map inc) [1 2 3])))
(throwof "sequence-take" '(take 3 (sequence (map inc) (range))))
;; missing rows audit
(throwof "resolve-cat" '(resolve 'cat))
(throwof "resolve-random-sample" '(resolve 'random-sample))
;; error shapes
(throwof "err-bad-xform" '(transduce 5 + [1 2 3]))
(throwof "err-noncoll" '(transduce (map inc) + 5))
(throwof "err-seq-nonseq" '(sequence (map inc) 5))
(throwof "err-into-xf-nonseq" '(into [] (map inc) 5))
(throwof "err-completing-badf" '(transduce (completing 5) + [1]))
