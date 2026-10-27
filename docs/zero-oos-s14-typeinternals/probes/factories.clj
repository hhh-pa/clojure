;; S14 type-internals oracle probes: factories (oracle: alpha10 jars, issue #58).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s14-typeinternals/probes/factories.clj > docs/zero-oos-s14-typeinternals/probes/factories.out
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
(defn show [label form]
  (println (str label " " (normhex (pr-str (eval form))))))
;; ---- A: classes ----
(show "a-empty-class" '(class EMPTY-NODE))
(show "a-vecnode-class" '(class (->VecNode nil (object-array 32))))
(show "a-vec-class" '(class (->Vec nil 0 5 EMPTY-NODE (object-array 0) nil)))
(show "a-vecseq-class" '(class (->VecSeq nil nil nil 0 0 nil)))
(show "a-chunk-class" '(class (->ArrayChunk nil (object-array 3) 0 3)))
(show "a-eduction-class" '(class (->Eduction (map inc) [1 2])))
;; ---- B: arglists ----
(show "b-vecnode-args" '(:arglists (meta #'->VecNode)))
(show "b-vec-args" '(:arglists (meta #'->Vec)))
(show "b-vecseq-args" '(:arglists (meta #'->VecSeq)))
(show "b-chunk-args" '(:arglists (meta #'->ArrayChunk)))
(show "b-eduction-args" '(:arglists (meta #'->Eduction)))
(show "b-empty-meta" '(meta #'EMPTY-NODE))
;; ---- C: arity errors ----
(throwcause "c-vecnode-1" '((var ->VecNode) nil))
(throwcause "c-vecnode-0" '((var ->VecNode)))
(throwcause "c-vec-short" '((var ->Vec) nil 0 5 EMPTY-NODE))
(throwcause "c-eduction-1" '((var ->Eduction) (map inc)))
(throwcause "c-chunk-3" '((var ->ArrayChunk) nil nil 0))
(throwcause "c-vecseq-5" '((var ->VecSeq) nil nil nil 0 0))
;; ---- D: print shapes ----
(show "d-empty" 'EMPTY-NODE)
(show "d-vecnode" '(->VecNode nil (object-array 32)))
(show "d-eduction" '(->Eduction (map inc) [1 2]))
;; ---- E: equality ----
(show "e-self" '(= EMPTY-NODE EMPTY-NODE))
(show "e-fresh" '(= EMPTY-NODE (->VecNode nil (object-array 32))))
(show "e-ident" '(identical? EMPTY-NODE EMPTY-NODE))
(show "e-eduction" '(= (->Eduction (map inc) [1 2]) (->Eduction (map inc) [1 2])))
;; ---- F: fields ----
(show "f-edit" '(.-edit EMPTY-NODE))
(show "f-arr-count" '(count (.-arr EMPTY-NODE)))
(show "f-vec-cnt" '(.-cnt (->Vec nil 7 5 EMPTY-NODE (object-array 0) nil)))
(show "f-chunk-off" '(.-off (->ArrayChunk nil (object-array 3) 1 3)))
(show "f-chunk-end" '(.-end (->ArrayChunk nil (object-array 3) 1 3)))
(show "f-vecseq-i" '(.-i (->VecSeq nil nil nil 4 0 nil)))
(show "f-eduction-coll" '(.-coll (->Eduction (map inc) [1 2])))
;; ---- G: predicates ----
(show "g-map" '(map? EMPTY-NODE))
(show "g-vec" '(vector? (->Vec nil 0 5 EMPTY-NODE (object-array 0) nil)))
(show "g-seq-chunk" '(seq? (->ArrayChunk nil (object-array 3) 0 3)))
(show "g-sequential-ed" '(sequential? (->Eduction (map inc) [1 2])))
(show "g-counted-ed" '(counted? (->Eduction (map inc) [1 2])))
;; ---- H: behavior ----
(show "h-ed-seq" '(seq (->Eduction (map inc) [1 2 3])))
(show "h-ed-first" '(first (->Eduction (map inc) [1 2 3])))
(show "h-chunk-count" '(count (->ArrayChunk nil (object-array 3) 0 3)))
(throwcause "h-chunk-nth" '(nth (->ArrayChunk nil (into-array [10 20 30]) 0 3) 1))
(show "h-vec-count" '(count (->Vec nil 0 5 EMPTY-NODE (object-array 0) nil)))
(throwcause "h-vecseq-seq" '(seq (->VecSeq nil nil nil 0 0 nil)))
