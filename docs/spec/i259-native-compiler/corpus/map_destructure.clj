;; Map destructuring bound by direct lookups (#271): every source shape
;; the interpreter's binding reads, :or defaults, :as, nested patterns.
(defrecord P [x y])
(defn kv [{:keys [a b] :or {b :dflt} :as m}] [a b m])
(defn ent [{x :x [p q] :pair {:keys [deep]} :inner :or {x 7}}] [x p q deep])
(defn strs-syms [{:strs [s] :syms [y] :keys [ns/k] :or {s "none"}}] [s y k])
(defn opts [& {:keys [a b] :as o}] [a b o])
(println (kv {:a 1 :b 2}) (kv {:a 1}) (kv {:b nil}) (kv nil) (kv {}))
(println (ent {:x 1 :pair [2 3] :inner {:deep 4}}) (ent {}))
(println (strs-syms {"s" 1 'y 2 :ns/k 3}) (strs-syms {}))
;; list sources convert: pairs, a single map, an odd trailing map, none
(println (opts :a 1 :b 2) (opts {:a 3}) (opts :a 4 {:b 5}) (opts))
;; non-map sources: records, int-keyed vectors and strings, sets, numbers
(let [{:keys [x y]} (->P 1 2)] (println x y))
(let [{a 0 b 1 c 5} [:p :q]] (println a b c))
(let [{c 1} "xyz"] (println c))
(let [{a :a z :z} #{:a}] (println a z))
(let [{:keys [a] :or {a 5}} 42] (println a))
;; a present nil keeps nil; an absent key takes its default
(let [{:keys [a b] :or {a 5 b 6}} {:a nil}] (println a b))
;; :or keyed by the key form, for a nested local
(let [{[p q] :pair :or {:pair [8 9]}} {}] (println p q))
;; defaults see the enclosing locals, and all evaluate eagerly, in order
(let [z 3
      log (atom [])
      {:keys [a b c] :or {a z
                          b (do (swap! log conj :b) 2)
                          c (do (swap! log conj :c) 4)}} {:b 0}]
  (println a b c @log))
;; loops and repeated binds
(println (reduce (fn [s {:keys [a b]}] (+ s a b)) 0 (for [i (range 5)] {:a i :b (* 2 i)})))
(loop [[{:keys [n]} & more] [{:n 1} {:n 2} {:n 3}] acc 0]
  (if n (recur more (+ acc n)) (println acc)))
;; required keys keep the generic destructure, nested shapes and all
(defn req-all [{:keys! [a] :keys [b] :strs [s] [p & ps :as pv] :pair
                {:keys [d]} :inner :or {b 2} :as m}]
  [a b s p ps pv d (count m)])
(println (req-all {:a 1 "s" 3 :pair [4 5 6] :inner {:d 7}}))
(println (try (req-all {}) (catch Exception e (.getMessage e))))
(println (try (let [{:keys! [a]} {}] a) (catch Exception e (.getMessage e))))
(let [{:keys! [a]} {:a 1}] (println a))
