;; Program fns passed by name run traversals and comprehension stages inline
;; when their var is stable and they have the arity the traversal calls
;; (#271 Slice 6e); others keep the plain call.
(defn sq [x] (* x x))
(defn add-sq [acc x] (conj acc (* x x)))
(defn plus2 [a b] (+ a b))
(defn odd-sq? [x] (odd? (sq x)))
(defn label [x] (str "<" x ">"))
(defn noisy [x] (println "visit" x) (inc x))
(defn bad [x] (if (= x 3) (throw (ex-info "bad item" {:x x})) x))
(defn many ([x] (inc x)) ([x y] (* x y)))
(defn varargs [& xs] (count xs))
(defn- hidden [x] (- x))
(defn mask-bits [m]
  (loop [d 1 acc []]
    (if (> d 9)
      acc
      (if (zero? (bit-and m (bit-shift-left 1 d)))
        (recur (inc d) acc)
        (recur (inc d) (conj acc d))))))
(def masks (mapv mask-bits (range 16)))
(println masks)
(println (mapv sq (range 5)) (filterv odd-sq? (range 7)) (reduce add-sq [] (range 4))
         (reduce plus2 (range 5)) (reduce plus2 10 [1 2 3]) (every? odd-sq? [1 3]) (some odd-sq? [2 4 5]))
(println (mapv many [1 2]) (mapv many [1 2] [3 4]) (mapv plus2 [1 2 3] (range 10))
         (mapv varargs [1 2]) (mapv hidden (list 1 2)) (vec (map sq [3 4])) (into [] (map label [1 2])))
(println (apply str (map label (range 3))) (into #{} (map sq [-2 2 3])) (count (map noisy [7 8]))
         (sort (map hidden [3 1 2])) (set (map sq (range -2 3))) (apply max (map sq [3 -5 4])))
(println (try (mapv bad (range 5)) (catch Exception e (ex-message e)))
         (try (apply str (map bad [1 2 3])) (catch Exception e (ex-data e)))
         (try (reduce plus2 [1 :k]) (catch Exception e (class e))))
(println (reduce add-sq [] []) (mapv sq []) (apply str (map label nil))
         (try (reduce plus2 []) (catch Exception e (ex-message e))))

;; Inside a compiled fn, and a fn passed in by the caller.
(defn table [n] (mapv mask-bits (range n)))
(defn apply-all [g xs] (mapv g xs))
(println (table 6) (apply-all sq [1 2]) (apply-all label [:a]))

;; Core fns as stage fns, and a fn in a local, which keeps the plain call.
(defn join-with [g xs] (apply str (map g xs)))
(println (apply max (map count [[1] [1 2] []])) (into #{} (map inc [1 2 2])) (sort (map - [1 3 2]))
         (count (map str [:a :b])) (apply str (map name [:x :y])) (join-with label [1 2]) (join-with inc [1 2]))

;; A fn used before its def runs keeps the plain call.
(declare later)
(defn early [xs] (mapv later xs))
(defn later [x] (* 10 x))
(println (early [1 2]) (mapv later [3]))

;; A var the program redefines keeps the plain call, which reads its root once.
(defn shifting [x] (def shifting (fn [y] (* 100 y))) (inc x))
(println (mapv shifting [1 2 3]) (mapv shifting [1 2]))

;; A stable map, set or keyword passed by name calls as its lookup (Slice 6l).
(def digit-vals (zipmap "0123456789" (range 10)))
(def vowels #{\a \e \i \o \u})
(def kw-a :a)
(defn all-digits? [s] (every? digit-vals (vec s)))
(println (all-digits? "0451") (all-digits? "04x1") (mapv digit-vals [\3 \9 \x]) (filterv vowels (vec "education"))
         (mapv kw-a [{:a 1} {:b 2} {:a 3}]) (some vowels (vec "rhythm")) (some vowels (vec "sky blue"))
         (reduce digit-vals 0 [\5 \x]) (count (filter vowels "aeiou")) (apply str (map digit-vals "12")))
