;; Core vars rebound through a resolved var: native code must see the new roots.
(defn m [] (max 1 2))
(defn c [x] (count x))
(println (m) (c (map inc [1 2])))
((resolve 'clojure.core/alter-var-root) #'clojure.core/max (fn [_] (fn [& xs] :patched)))
((resolve 'clojure.core/alter-var-root) #'clojure.core/count (fn [_] (fn [x] :counted)))
(println (m) (c (map inc [1 2])) (c [1]))
