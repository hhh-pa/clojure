(require '[clojure.string :as str])
(let [n (Long/parseLong (str/trim (read-line)))
      xs (mapv #(Long/parseLong %) (str/split (str/trim (read-line)) #"\s+"))]
  (println (reduce + 0 (take n xs)) (apply max xs) (count (filter even? xs))))
