;; #271 Slice 3d: a program that reads var metadata keeps every key
;; exactly as run builds it — `defn` arglists and docs, attr maps,
;; private and doc'd defs, and its own metadata writes.
(defn f "doc of f" [s] (inc s))
(defn- p [x] (* 2 x))
(def ^{:doc "documented"} r 7)
(defn attr {:added "1.0"} [x] x)
(def a (atom 1))

(alter-meta! #'f assoc :private false)
(alter-meta! #'r assoc :doc (str "re" "documented"))
(alter-meta! #'p dissoc :doc)
(alter-meta! a assoc :doc "atom doc")
(alter-meta! #'r merge {:note (str "com" "puted")})

(println (:arglists (meta #'f)) (:doc (meta #'f)) (:private (meta #'f)))
(println (:private (meta #'p)) (:arglists (meta #'p)) (:doc (meta #'r)))
(println (:added (meta #'attr)) (:arglists (meta #'attr)) (:doc (meta a)))
(println (sort (keys (meta #'f))) (sort (keys (meta #'r))) (:note (meta #'r)))
(println (f 1) (p 2) r (attr 3) @a)
