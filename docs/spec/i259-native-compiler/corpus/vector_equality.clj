;; `=` of two flat long vectors compares their longs inline (#271 Slice 6m).
(defn tally [tr] [(count (filter #(= % [0 0]) tr)) (count (filter #(= % [0 1]) tr)) (count (remove #(not= % [1 1]) tr))])
(println (tally [[0 0] [0 1] [1 1] [0 0] [1 1] [1 0]]) (= [1 2] [1 2]) (= [1 2] [1 2 3]) (= [1 2] '(1 2)) (= [1 [2]] [1 [2]])
         (= [1.0] [1]) (= [] []) (= (vec (range 2000)) (vec (range 2000))) (not= [3] [4]) (= (subvec [0 1 2] 1) [1 2]))
(println (= '(1 2) [1 2 3]) (= (map inc [0 1]) [1 2 3]) (= [1 2 3] '(1 2)))
