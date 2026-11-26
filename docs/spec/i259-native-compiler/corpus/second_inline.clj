;; `second` of a vector or nil reads inline; other colls take the native (#271 Slice 6n).
(defn val-of [node] (case (first node) :num (second node) :neg (- (val-of (second node))) :pair [(second node) (second (second node))]))
(println (val-of [:num 5]) (val-of [:neg [:num 3]]) (val-of [:pair [1 2]]) (second [1]) (second []) (second nil) (second [1 nil 3])
         (second '(1 2)) (second "ab") (second {:a 1 :b 2}) (second (range 3)) (second (subvec [1 2 3] 1)) (second #{7}))
