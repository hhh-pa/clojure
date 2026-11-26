(require '[clojure.string :as str])
(defn neighbors [rows r c]
  (for [[dr dc] [[-1 0] [1 0] [0 -1] [0 1]]
        :let [r2 (+ r dr) c2 (+ c dc)]
        :when (and (< -1 r2 (count rows)) (< -1 c2 (count (first rows))))]
    (nth (nth rows r2) c2)))
(let [[h _] (map #(Integer/parseInt %) (str/split (read-line) #" "))
      rows (vec (repeatedly h read-line))]
  (println (count (for [r (range h) c (range (count (first rows)))
                        :when (and (= \# (nth (nth rows r) c))
                                   (some #{\.} (neighbors rows r c)))]
                    [r c]))))
