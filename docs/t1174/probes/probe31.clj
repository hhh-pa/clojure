;; S4 oracle probes, round 1b: letfn qualified names (H5),
;; seq :as timing (H6), lone pre/post maps (H7), macro shadowing
;; and list-only spans (H8). Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "letfn-qual-name" #(eval '(letfn [(u311/f311 [x311] x311)] 1)))
(tx "loop-as-nonsym" #(eval '(loop [[a311 :as 5] [1]] a311)))
(tx "loop-as-dangle" #(eval '(loop [[a311 :as] [1]] a311)))
(tx "loop-as-str" #(eval '(loop [[a311 :as "s311"] [1]] a311)))
(tx "loop-as-vec" #(eval '(loop [[a311 :as [b311]] [1 2]] a311)))
(tx "fn-lone-premap" #(eval '((fn [x311] {:pre [false]}) -5)))
(tx "fn-lone-postmap" #(eval '((fn [x311] {:post [false]}) -5)))
(tx "fn-lone-multi" #(eval '((fn ([x311] {:pre [false]}) ([x311 y311] x311)) -5)))
(tx "shadow-loop" #(eval '(let [loop311 5] (loop311 [x311 1] x311))))
(tx "shadow-loop-real" #(eval '(let [loop 5] (loop [x311 1] x311))))
(tx "shadow-if" #(eval '(let [if 5] (if 1 2))))
(tx "shadow-fn" #(eval '(let [fn 5] (fn [x311] x311))))
(tx "shadow-let" #(eval '(let [let 5] (let [x311 1] x311))))
(tx "shadow-binding" #(eval '(let [binding 5] (binding [x311 1] x311))))
(tx "shadow-letfn" #(eval '(let [letfn 5] (letfn [(f311 [x311] x311)] (f311 1)))))
(tx "shadow-global" #(eval '(do (def loop 5) (loop [x311 1] x311))))
(tx "span-vec" #(eval '(do [nope312] 1)))
(tx "span-map" #(eval '(do {:a nope312} 1)))
(tx "span-nested-list" #(eval '(do (t-plus nope312 1) 2)))
