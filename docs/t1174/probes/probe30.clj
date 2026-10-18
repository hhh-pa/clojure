;; S4 oracle probes, round 1b: fn/loop try-fence resets (B3/B4).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "fence-fn-try" #(eval '(try ((fn [n301] (if n301 1 (recur 2))) 0) (catch Exception e 2))))
(tx "fence-fn-prepost" #(eval '(try ((fn [n301] {:pre [(number? n301)]} (if (zero? n301) :d (recur (dec n301)))) 2) (catch Exception e :c))))
(tx "fence-loop-try" #(eval '(try (loop [n301 5] (if n301 7 (recur 1))) (catch Exception e 2))))
(tx "fence-loop-init" #(eval '(loop [x301 1] (loop [n301 (try (recur 5) (catch Exception e 2))] n301))))
(tx "fence-catch-targetless" #(eval '(try (throw (ex-info "x" {})) (catch Exception e (recur 1)))))
(tx "fence-finally-targetless" #(eval '(try 1 (finally (recur 1)))))
(tx "fence-catch-outerfence" #(eval '(try (loop [a302 1] (try 1 (catch Exception e (recur 1)))) (catch Exception f 2))))
(tx "fence-nested-try-in-catch" #(eval '(loop [a302 1] (try 1 (catch Exception e (try (recur 1) (catch Exception f 2)))))))
(tx "fence-fn-in-catch" #(eval '(try 1 (catch Exception e ((fn [x302] (recur x302)) 1)))))
(tx "fence-loop-in-catch" #(eval '(try 1 (catch Exception e (loop [a302 1] (recur a302))))))
(tx "fence-loop-nontail" #(eval '(do (loop [a303 3] (if (zero? a303) :d (recur (dec a303)))) 2)))
(tx "recur-args-first" #(eval '(loop [a301 1] (recur nope-xyz q301))))
(tx "recur-targetless-badarg" #(eval '(do (recur nope-xyz304) 2)))
(tx "recur-nontail-targetless" #(eval '(do (recur 1) 2)))
(tx "recur-nontail-badargs" #(eval '(loop [a304 1] (do (recur nope-xyz304 q304) 3))))
