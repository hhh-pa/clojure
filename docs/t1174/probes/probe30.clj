;; S4 oracle probes, round 1b: fn try-fence reset (B3).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "fence-fn-try" #(eval '(try ((fn [n301] (if n301 1 (recur 2))) 0) (catch Exception e 2))))
(tx "fence-fn-prepost" #(eval '(try ((fn [n301] {:pre [(number? n301)]} (if (zero? n301) :d (recur (dec n301)))) 2) (catch Exception e :c))))
