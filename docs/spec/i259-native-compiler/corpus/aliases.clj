;; Direct defn calls pass plain locals as aliases and params read the
;; caller's argument array in place: values must survive every exit.
(defn pick [v i] (nth v i))
(defn boom [v] (throw (ex-info "boom" {:n (count v)})))
(defn keep-closure [v] (fn [] (count v)))
(defn keep-lazy [v] (lazy-seq (map inc v)))
(defn countdown [v n] (if (pos? n) (recur (conj v n) (dec n)) v))
(defn destr [[a b] {:keys [c]}] [a b c])
(defn shadow [v] (let [v (conj v :x) w v] [v w]))
(defn use-all [n]
  (let [v (vec (range 5))
        t (transient [1 2])
        m {:c 3}]
    (dotimes [_ n] (pick v 2) (destr v m))
    [(pick v 4)
     (try (boom v) (catch Exception e (ex-data e)))
     v
     ((keep-closure v))
     (doall (keep-lazy v))
     (countdown v 3)
     v
     (destr v m)
     (shadow v)
     (count (persistent! (conj! t (pick v 0))))]))
(println (use-all 1000))
(defn pick [v i] (str "redefined " (count v) i))
(println (let [v [1 2]] [(pick v 0) v]))
