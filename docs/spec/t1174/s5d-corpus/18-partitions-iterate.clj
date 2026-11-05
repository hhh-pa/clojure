;; S5D differential corpus 18: partition step strictness, iterate counting, cons/cycle/interleave edges.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (7i partition step, 7i iterate/cons/cycle/interleave),
;; s5d-gaps7i (i-iterate-*, i-cons-*), s5d-gaps7i5 (i5-part0, i5-interleave),
;; s5d-gaps7i6 (r-part-*, r-iterate-next recovery).
;; NOTE: partition values/windows live in 06, cycle/iterate/cons/interleave
;; basics in 04; this file pins only the 7i-new cells (step strictness,
;; f-call counters, nil-f iterate, degenerate n-0/pad).
;; Rule: every infinite partition/iterate is take-bounded or class-only.
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

;; --- partition big step: vector AE, list single-window (7i6) ---
(p ":pbig-max-vec" #(str (take 2 (partition 2 9223372036854775807 [1 2 3 4]))))
(p ":pbig-max-list" #(vec (take 2 (partition 2 9223372036854775807 '(1 2 3 4)))))
(p ":pbig-max-list-str" #(str (take 2 (partition 2 9223372036854775807 '(1 2 3 4)))))
(p ":pbig-all-max-vec" #(str (take 2 (partition-all 2 9223372036854775807 [1 2 3 4]))))

;; --- partition step strictness: lazy ctor, strict-nthrest first force (7i5, 7i6) ---
(p ":pstep-str-vec-take1" #(str (take 1 (partition 2 "a" [1 2 3 4]))))
(p ":pstep-str-vec-first" #(first (partition 2 "a" [1 2 3 4])))
(p ":pstep-str-list-first" #(first (partition 2 "a" '(1 2 3 4))))
(p ":pstep-str-list-take1" #(str (take 1 (partition 2 "a" '(1 2 3 4)))))
(p ":pstep-str-list-take2" #(str (take 2 (partition 2 "a" '(1 2 3 4)))))
(p ":pstep-nil-list-first" #(first (partition 2 nil '(1 2))))
(p ":pstep-all-str-list-first" #(first (partition-all 2 "a" '(1 2 3 4))))
(p ":pstep-all-str-list-take2" #(str (take 2 (partition-all 2 "a" '(1 2 3 4)))))
(p ":pstep-str-ctor" #(do (partition 2 "a" [1 2]) :ok))

;; --- partition n-0 and pad (7i5 i5-part0) ---
(p ":partall-0-take2" #(vec (take 2 (partition-all 0 [1 2]))))
(p ":part-0-take2" #(vec (take 2 (partition 0 [1 2]))))
(p ":part-pad22-9" #(vec (partition 2 2 [9] [1])))
(p ":part-pad33-9" #(vec (partition 3 3 [9] [1])))

;; --- iterate ctor/f errors (7i i-iterate-ctor) ---
(p ":iterate-badf" #(iterate 5 0))
(p ":iterate-nilf-first" #(first (iterate nil 0)))
(cls ":c-iterate-nilf-rest" #(rest (iterate nil 0)))
(p ":iterate-nilf-rest-first" #(first (rest (iterate nil 0))))

;; --- iterate f-call counters: rest/next force current val only (7i, 7i6) ---
(p ":itc-first" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(first it) @c]))
(p ":itc-rest" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(str (class (rest it))) @c]))
(p ":itc-rest-first" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(first (rest it)) @c]))
(p ":itc-rest-rest" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(str (class (rest (rest it)))) @c]))
(p ":itc-restrest-first" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(first (rest (rest it))) @c]))
(p ":itc-next" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(str (class (next it))) @c]))
(p ":itc-next-first" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(first (next it)) @c]))
(p ":itc-next-rest" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(str (class (next (rest it)))) @c]))
(p ":itc-nth2" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(nth it 2) @c]))
(p ":itc-drop2-first" #(let [c (atom 0) it (iterate (fn [x] (swap! c inc) (inc x)) 0)] [(first (drop 2 it)) @c]))

;; --- cons/cycle news (7i i-cons-*) ---
(cls ":c-cons-1nil" #(cons 1 nil))
(p ":cons-1nil-val" #(cons 1 nil))
(cls ":c-cons-rest" #(rest (cons 1 [2])))
(p ":cons-force" #(let [c (atom 0) lz (lazy-seq (swap! c inc) [9])] [(str (class (cons 1 lz))) @c]))
(cls ":c-cycle-nil" #(cycle nil))
(cls ":c-cycle-empty" #(cycle []))
(cls ":c-cycle-rest" #(rest (cycle [1])))

;; --- interleave/concat news (7i5 i5-interleave) ---
(p ":il-nilq" #(nil? (interleave)))
(cls ":c-il-1vec" #(interleave [1 2]))
(p ":il-1vec-vec" #(vec (interleave [1 2])))
(p ":concat-nilq" #(nil? (concat)))
(println "S5D-18 DONE")
