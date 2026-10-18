;; S4 oracle probes, round 1b: fn clause-check order (M6).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "fn-dup-beats-or" #(eval '(fn ([x321] x321) ([y321] y321) ([{a321 :x321 :or {a321 1 :x321 2}}] a321))))
(tx "fn-dup-beats-body" #(eval '(fn ([x321] nope321) ([y321] y321) ([z321] z321))))
(tx "fn-dup-after-body" #(eval '(fn ([x322] x322) ([y322] nope322))))
(tx "fn-count-first" #(eval '(fn ([{a322 :x322 :or {a322 1 :x322 2}} q0 q1 q2 q3 q4 q5 q6 q7 q8 q9 q10 q11 q12 q13 q14 q15 q16 q17 q18 q19] x322))))
(tx "fn-fixedvar-last" #(eval '(fn ([x322 & r322] x322) ([a322 b322] a322) ([{a322 :x322 :or {a322 1 :x322 2}}] z322))))
(tx "fn-or-beats-body" #(eval '(fn [{a321 :x321 :or {a321 1 :x321 2}}] nope321)))
(tx "fn-body-span" #(eval '(fn [x323] (do nope323 2))))
(tx "fn-clause-span" #(eval '(fn ([x323] (do nope323 2)))))
