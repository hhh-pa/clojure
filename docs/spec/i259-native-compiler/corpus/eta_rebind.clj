;; A program that can rebind core vars keeps traversals of core fns by
;; name on the general path, so a rebound root shows (#271 Slice 5x).
(defn incs [xs] (mapv inc xs))
(defn total [xs] (reduce + xs))
(println (incs [1 2]) (total [1 2 3]))
((resolve 'clojure.core/alter-var-root) #'clojure.core/inc (fn [_] (fn [x] :patched)))
((resolve 'clojure.core/alter-var-root) #'clojure.core/+ (fn [_] (fn ([] 0) ([a b] :summed))))
(println (incs [1 2]) (total [1 2 3]))
