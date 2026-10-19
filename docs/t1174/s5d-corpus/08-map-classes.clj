;; S5D differential corpus 08: map classes (array-map vs hash-map rules).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Map classes), s5d-gaps7g12/7g13/7g14(fixed)/7g14b/7g14c.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn cls [tag thunk]
  (print tag " => ")
  (prn (try (let [v (thunk)] [(class v) (seq? v) (counted? v) (list? v) (sequential? v) (coll? v)])
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))
(defn mclass [m] (str (class m)))

;; --- builders: into / apply hash-map / array-map / zipmap (7g12) ---
(p ":map-into-kw40" #(mclass (into {} (map (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(p ":map-apply-kw40" #(mclass (apply hash-map (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(p ":map-array-kw40" #(mclass (apply array-map (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(p ":map-zipmap-kw40" #(mclass (zipmap (map (fn [i] (keyword (str "k" i))) (range 40)) (range 40))))
(p ":map-into-int40" #(mclass (into {} (map (fn [i] [i i]) (range 40)))))
(p ":map-apply-int40" #(mclass (apply hash-map (mapcat (fn [i] [i i]) (range 40)))))
(p ":map-into-kw8" #(mclass (into {} (map (fn [i] [(keyword (str "k" i)) i]) (range 8)))))
(p ":map-into-kw9" #(mclass (into {} (map (fn [i] [(keyword (str "k" i)) i]) (range 9)))))
(p ":map-apply-kw9" #(mclass (apply hash-map (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 9)))))
(p ":map-apply-int9" #(mclass (apply hash-map (mapcat (fn [i] [i i]) (range 9)))))
(p ":into-int9" #(mclass (into {} (map (fn [i] [i i]) (range 9)))))
(p ":into-kw9" #(mclass (into {} (map (fn [i] [(keyword (str "k" i)) i]) (range 9)))))
(p ":hashmap-kw40" #(mclass (apply hash-map (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(p ":into-empty" #(mclass (into {} [])))
(p ":merge-ee" #(mclass (merge {} {})))
(p ":merge-0" #(merge))

;; --- hash-map / array-map always-rules (7g13, 7g14b) ---
(p ":hashmap-1c" #(mclass (hash-map :a 1)))
(p ":hashmap-0c" #(mclass (hash-map)))
(p ":hashmap-fn-kw9" #(mclass (hash-map :k0 0 :k1 1 :k2 2 :k3 3 :k4 4 :k5 5 :k6 6 :k7 7 :k8 8)))
(p ":arraymap-int40" #(mclass (apply array-map (mapcat (fn [i] [i i]) (range 40)))))
(p ":arraymap-kw65" #(mclass (apply array-map (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 65)))))

;; --- literals: canBePAM (7g13) ---
(p ":lit-kw9" #(mclass {:k0 0 :k1 1 :k2 2 :k3 3 :k4 4 :k5 5 :k6 6 :k7 7 :k8 8}))
(p ":lit-int9" #(mclass {0 0 1 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8}))
(p ":lit-empty" #(mclass {}))

;; --- persistent assoc promotion (7g12, 7g13, 7g14) ---
(p ":map-dissoc-shrink" #(mclass (apply dissoc (into {} (map (fn [i] [i i]) (range 40))) (range 38))))
(p ":map-assoc-grow" #(mclass (apply assoc {:a 0} (mapcat (fn [i] [i i]) (range 40)))))
(p ":assoc-kw40" #(mclass (apply assoc {} (mapcat (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(p ":assoc-int9" #(mclass (apply assoc {} (mapcat (fn [i] [i i]) (range 9)))))
(p ":conj-pairs-kw40" #(mclass (reduce conj {} (map (fn [i] [(keyword (str "k" i)) i]) (range 40)))))
(def m39 (apply assoc {} (mapcat identity (map (fn [i] [(keyword (str "k" i)) i]) (range 39)))))
(p ":assoc-int-last" #(mclass (assoc m39 99 99)))
(p ":assoc-int-first" #(mclass (assoc (assoc {} 99 99) :k0 0)))

;; --- transient assoc! capacity gating (7g13, 7g14) ---
(p ":tbang-kw40" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(keyword (str "k" i)) i]) (range 40))))))
(p ":tbang-int40" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [i i]) (range 40))))))
(p ":tbang-mixed" #(mclass (persistent! (reduce conj! (transient {}) (conj (map (fn [i] [(keyword (str "k" i)) i]) (range 39)) [99 99])))))
(p ":tbang-str40" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(str "k" i) i]) (range 40))))))
(p ":tbang-kw65" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(keyword (str "k" i)) i]) (range 65))))))
(def kw39 (map (fn [i] [(keyword (str "k" i)) i]) (range 39)))
(def int9pairs (map (fn [i] [i i]) (range 9)))
(def kw31 (map (fn [i] [(keyword (str "q" i)) i]) (range 31)))
(p ":int-last" #(mclass (persistent! (reduce conj! (transient {}) (conj (vec kw39) [99 99])))))
(p ":int-first" #(mclass (persistent! (reduce conj! (transient {}) (into [[99 99]] kw39)))))
(p ":kw64" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(keyword (str "k" i)) i]) (range 64))))))
(p ":kw65" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(keyword (str "k" i)) i]) (range 65))))))
(p ":int9-then-kw31" #(mclass (persistent! (reduce conj! (transient {}) (concat int9pairs kw31)))))
(p ":kw31-then-int9" #(mclass (persistent! (reduce conj! (transient {}) (concat kw31 (map (fn [i] [(+ 100 i) i]) (range 9)))))))
(p ":str9" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(str "k" i) i]) (range 9))))))
(p ":sym40" #(mclass (persistent! (reduce conj! (transient {}) (map (fn [i] [(symbol (str "k" i)) i]) (range 40))))))
(p ":transient-big-class" #(str (class (transient (into {} (map (fn [i] [i i]) (range 40)))))))
(p ":transient-small-class" #(str (class (transient {:a 1}))))

;; --- merge / select-keys / dissoc! classes (7g12, 7g13) ---
(p ":merge-kw40" #(mclass (merge {:z -1} (zipmap (map (fn [i] (keyword (str "k" i))) (range 40)) (range 40)))))
(p ":selkeys-big" #(mclass (select-keys (into {} (map (fn [i] [i i]) (range 40))) [1 2])))
(p ":dissoc-bang-stays" #(mclass (persistent! (apply dissoc! (transient (into {} (map (fn [i] [i i]) (range 40)))) (range 38)))))

;; --- dissoc order vs dissoc! swap-last (7g14b) ---
(p ":dissoc-order" #(seq (dissoc {:a 1 :b 2 :c 3} :a)))
(p ":dissoc-bang-order" #(seq (persistent! (dissoc! (transient {:a 1 :b 2 :c 3}) :a))))

;; --- empty kind preservation (7g12, 7g13) ---
(p ":empty-bigmap" #(mclass (empty (into {} (map (fn [i] [i i]) (range 40))))))
(p ":empty-smallmap" #(mclass (empty {:a 1})))
(p ":empty-vec" #(mclass (empty [1])))
(p ":empty-list" #(mclass (empty '(1))))
(p ":empty-set" #(mclass (empty #{1})))
(p ":empty-str" #(empty "ab"))
(p ":empty-nil" #(empty nil))
(p ":empty-range" #(mclass (empty (range 5))))
(p ":empty-lazy" #(mclass (empty (map inc [1]))))
(p ":empty-tvec" #(empty (transient [1])))
(p ":empty-tmap" #(empty (transient {:a 1})))

;; --- map seqs: array-map counted, hash-map not (pins) ---
(p ":seq-smallmap-counted" #(counted? (seq {:a 1 :b 2})))
(p ":seq-bigmap-counted" #(counted? (seq (into {} (map (fn [i] [i i]) (range 40))))))
(println "S5D-08 DONE")
