(require '[clojure.string :as str])
(let [lines (str/split-lines (slurp *in*))
      words (mapcat #(str/split % #" ") lines)
      freq (frequencies (map str/lower-case words))
      top (take 5 (sort-by (juxt (comp - val) key) freq))]
  (doseq [[w c] top] (printf "%s %d%n" w c))
  (println (str/join "," (sort (distinct (map count words)))))
  (when (empty? lines) (binding [*out* *err*] (println "no input"))))
