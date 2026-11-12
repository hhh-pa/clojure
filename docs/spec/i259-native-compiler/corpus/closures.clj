;; Closures capturing locals across every extent form (capture analysis).
(def ^:dynamic *v* 0)
(def lock (Object.))
(def r (ref 0))
(defn make [a b]
  {:map (fn [] {a b})
   :set (fn [] #{a b})
   :vec (fn [] [a b])
   :bind (fn [] (with-bindings {#'*v* a} (+ *v* b)))
   :binding (fn [] (binding [*v* b] (+ a *v*)))
   :out (fn [] (with-out-str (print a b)))
   :in (fn [] (with-in-str (str a) (read-line)))
   :lock (fn [] (locking lock (+ a b)))
   :sync (fn [] (dosync (ref-set r (+ a b))))
   :io (fn [] (io! (* a b)))
   :delay (fn [] @(delay (- a b)))
   :lazy (fn [] (doall (lazy-seq [a b])))
   :redefs (fn [] (with-redefs [*v* a] (+ *v* b)))
   :try (fn [] (try (if (pos? a) (throw (ex-info "t" {:b b})) a) (catch Exception e (:b (ex-data e))) (finally a)))
   :case (fn [] (case a 1 b 2 (+ a b) :none))
   :letfn (fn [] (letfn [(g [] (+ a b))] (g)))
   :loop (fn [] (loop [i a acc 0] (if (< i b) (recur (inc i) (+ acc i)) acc)))
   :destr (fn [] (let [[x y] [a b] {:keys [z] :or {z a}} {}] [x y z]))
   :nested (fn [] ((fn [] ((fn [] (* a b))))))
   :bound (fn [] ((bound-fn [] (+ a b))))
   :assert (fn [] (assert (= a a) (str b)) :ok)
   :set! (fn [] (binding [*v* 0] (set! *v* (+ a b)) *v*))
   :time (fn [] (count (with-out-str (time (+ a b)))))
   :throw (fn [] (try (throw (RuntimeException. (str a))) (catch RuntimeException e (.getMessage e))))
   :meta (fn [] (meta ^{:x a} [b]))})
(let [m (make 1 4)]
  (doseq [k (sort (keys m))]
    (let [v ((get m k))]
      (println k (if (= k :time) (pos? v) v)))))
(defn letfn-capture [n]
  (letfn [(down [i] (if (pos? i) (cons i (down (dec i))) (list n)))]
    (down n)))
(println (letfn-capture 3))
(defn counter-cells []
  (letfn [(a [i] (if (pos? i) (b (dec i)) :a-done))
          (b [i] (if (pos? i) (a (dec i)) :b-done))]
    [(a 4) (a 5) (fn [] (a 2))]))
(let [[x y f] (counter-cells)] (println x y (f)))
