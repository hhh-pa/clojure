(println "trailer-entry" (pr-str (let [{a :a, b :b} (list :a 1 (first {:b 2}))] [a b])))
(println "trailer-listentry" (pr-str (let [{a :a, b :b} (list :a 1 (list (first {:b 2})))] [a b])))
