(require '[clojure.string :as str])
(loop [acc [] line (read-line)]
  (if (nil? line)
    (let [v (sort acc)]
      (println (count v) (first v) (last v) (format "%.3f" (double (/ (reduce + v) (max 1 (count v))))))
      (println (str/join " " (map #(Math/abs (long %)) (take 10 v)))))
    (recur (if (str/blank? line) acc (conj acc (Integer/parseInt (str/trim line)))) (read-line))))
