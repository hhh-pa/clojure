;; defprotocol / defrecord / extend-protocol in compiled code: the
;; protocol method fns are compiled (native) fns, registered through
;; __protocol_method* like run's interpreted closures (#271).
(defprotocol Shape (area [s]) (name-of [s]))
(defrecord Circle [r] Shape (area [_] (* 3 r r)) (name-of [_] "circle"))
(defrecord Rect [w h] Shape (area [_] (* w h)) (name-of [_] "rect"))
(extend-protocol Shape Long (area [x] x) (name-of [_] "long"))
(def shapes [(->Circle 1) (->Rect 2 3) (map->Rect {:w 1 :h 1}) 5])
(println (map name-of shapes) (reduce + (map area shapes)) (:w (second shapes)))
(println (try (area "s") (catch IllegalArgumentException e (.getMessage e))))
(println (satisfies? Shape (->Rect 1 1)) (satisfies? Shape "x") (instance? Circle (first shapes)))
(defmulti describe :kind)
(defmethod describe :a [m] (str "A" (:v m)))
(defmethod describe :default [m] "other")
(println (describe {:kind :a :v 1}) (describe {:kind :b}))
