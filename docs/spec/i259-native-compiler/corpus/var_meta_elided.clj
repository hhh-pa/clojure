;; #271 Slice 3d: a program that references no var-metadata reader.
;; Native builds compile no `defn` `:arglists` writes (nor `merge`s of
;; inert attr maps), def names drop inert keys, and vars carry no
;; metadata — none of which this program can observe: values, private
;; and dynamic vars, nested defns, and failed preconditions behave
;; exactly as under run.
(defn f [s] (inc s))
(defn g "doc" [tok] (str tok "!"))
(defn h ([a] a) ([a b] (+ a b)))
(defn k [x] {:pre [(pos? x)]} (* 3 x))
(defn- p [x] (* 2 x))
(def ^:private q 5)
(def ^{:doc "documented"} r 7)
(def ^{:arglists '([x])} t identity)
(def ^{:doc (str "computed" " doc")} u 9)
(defn attr-doc {:doc "merged"} [x] (- x))
(def ^:dynamic *depth* 0)

(defn outer [n]
  ;; A nested defn: its metadata write sits in the fn body's `do`.
  (defn inner [m] (* m n))
  (inner 10))

(f 41)
(vector #'f inc)
((fn [] (println :anonymous-call)))
(println (f 1) (g 2) (h 1 2) (k 3) (p 4) q r (t 8) u (attr-doc 6))
(println (outer 4) (inner 5))
(println (binding [*depth* 2] *depth*) *depth*)
(println (try (k -1) (catch AssertionError e (.getMessage e))))
