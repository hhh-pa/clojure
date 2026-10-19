;; S5D differential corpus 04: cycle, iterate, cons, concat, interleave, interpose.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Repeat/cycle/iterate/cons), s5d-gaps7g9/7g10/7g11
;; (classes, seq-self), s5d-gaps7e (cycle/interleave/interpose), s5d-gaps7g4
;; (cons arity), s5d-gaps7g5 (empty/interleave preds), gaps/gaps2/gaps3
;; (interleave shapes), probe39 (concat-cycle, interleave-interpose).
;; Rule: every infinite cycle/iterate/interleave is take-bounded.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn cls [tag thunk]
  (print tag " => ")
  (prn (try (let [v (thunk)] [(class v) (seq? v) (counted? v) (list? v) (sequential? v) (coll? v)])
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- classes + seq-self (7g9, 7g10, 7g11) ---
(cls ":c-iterate" #(iterate inc 0))
(cls ":c-cons" #(cons 1 [2]))
(cls ":c-cycle" #(cycle [1 2]))
(cls ":c-interleave" #(interleave [1] [2]))
(cls ":c-restcycle" #(rest (cycle [1 2])))
(cls ":c-restiterate" #(rest (iterate inc 0)))
(p ":seqself-cycle" #(let [c (cycle [1])] (identical? c (seq c))))
(p ":seqself-iterate" #(let [i (iterate inc 0)] (identical? i (seq i))))
(p ":seqself-cons" #(let [c (cons 1 [2])] (identical? c (seq c))))
(p ":interleave-0-preds" #(let [s (interleave)] [(seq? s) (counted? s) (list? s) (coll? s)]))

;; --- cycle (7e, 7g5, 7g10, probe39, 7g4) ---
(p ":cycle-take" #(take 3 (cycle [1 2])))
(p ":cycle-basic" #(take 4 (cycle [1 2])))
(p ":cycle-nil" #(take 3 (cycle nil)))
(p ":cycle-nil-val" #(cycle nil))
(p ":cycle-empty-coll" #(cycle []))
(p ":cycle-empty-str" #(cycle ""))
(p ":cycle-single" #(take 3 (cycle [7])))
(p ":cycle-take0" #(take 0 (cycle [1])))
(p ":a-cycle" #(eval (list 'cycle)))

;; --- iterate, incl. strict-more pin (7e, 7g10, gaps3, probe39, pins; NEW counts) ---
(p ":iterate-take" #(take 3 (iterate inc 0)))
(p ":iterate-first" #(first (iterate inc 5)))
(p ":iterate-more-strict" #(let [n (atom 0) i (iterate (fn [x] (swap! n inc) (inc x)) 0)] (first i) @n))
(p ":iterate-second-count" #(let [n (atom 0) i (iterate (fn [x] (swap! n inc) (inc x)) 0)] (second i) @n))
(p ":iterate-take3-count" #(let [n (atom 0) i (iterate (fn [x] (swap! n inc) (inc x)) 0)] (doall (take 3 i)) @n))
(p ":a-iterate" #(eval (list 'iterate)))

;; --- cons (probe39, 7g4) ---
(p ":cons-nil" #(cons 0 nil))
(p ":cons-vec" #(cons 0 [1]))
(p ":cons-first" #(first (cons 1 [2])))
(p ":cons-rest" #(rest (cons 1 [2])))
(p ":cons-next" #(next (cons 1 [2])))
(p ":a-cons" #(eval (list 'cons)))

;; --- concat (probe39, 7g5) ---
(cls ":c-concat0" #(concat))
(p ":concat-0" #(concat))
(p ":concat-nil" #(concat nil))
(p ":concat-nils" #(concat nil [1] nil [2]))
(p ":concat-1" #(concat [1 2]))

;; --- interleave / interpose (7e, gaps2, gaps3, probe39) ---
(p ":interleave-short" #(interleave [1 2] [3]))
(p ":interleave-long" #(interleave [1] [2 3 4]))
(p ":interleave-0" #(interleave))
(p ":interleave-1" #(interleave [1 2]))
(p ":interleave-3" #(interleave [1 2 3] [:a :b] [7 8]))
(p ":interleave-single" #(interleave [1 2]))
(p ":interleave-empty" #(interleave))
(p ":interleave-nil" #(interleave [1] nil))
(p ":interleave-vec" #(interleave [1 2 3] [:a :b]))
(p ":interpose-empty" #(interpose :x []))
(p ":interpose-1" #(interpose :x [1]))
(p ":interpose-3" #(interpose :x [1 2 3]))
(p ":interpose-nil" #(interpose :x nil))
(println "S5D-04 DONE")
