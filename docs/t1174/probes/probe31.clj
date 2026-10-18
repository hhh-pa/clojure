;; S4 oracle probes, round 1b: letfn qualified names (H5).
;; Exit 0 (caught; chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "letfn-qual-name" #(eval '(letfn [(u311/f311 [x311] x311)] 1)))
