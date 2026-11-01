;; Oracle g123-goalt: alts! in go, alt!/alt!!, go-loop.
;; Run: java ... clojure.main docs/gap-123/g123-goalt.clj (fresh JVM; stderr interleaved)
(require '[clojure.core.async :as a]
         '[clojure.core.async.impl.protocols :as impl]
         '[clojure.string :as str])

(def closed? impl/closed?)

(defn show [label v]
  (println (str label " => " (pr-str v))))

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW (unexpected!)"))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

;; Take a go channel with a timeout guard (returns the value or :TIMEOUT).
(defn goo [label g]
  (let [[v p] (a/alts!! [g (a/timeout 10000)])
        v (if (= p g) v :TIMEOUT)]
    (println (str label " => " (pr-str v)))
    v))

;; --- alts! inside go (compare ports by identity) ---
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :ready)
  (goo "s-take" (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c1)]))))
(let [c1 (a/chan) c2 (a/chan 1)]
  (goo "s-put" (a/go (let [[v p] (a/alts! [[c2 :v]])] [v (= p c2) (a/<! c2)]))))
(let [c1 (a/chan) c2 (a/chan)]
  (a/close! c1)
  (goo "s-closed" (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c1)]))))
(let [c1 (a/chan) c2 (a/chan)]
  (a/close! c2)
  (goo "s-put-closed" (a/go (let [[v p] (a/alts! [[c2 :v]])] [v (= p c2)]))))
(let [c1 (a/chan) c2 (a/chan)]
  (goo "s-default" (a/go (a/alts! [c1 c2] :default :d))))
(let [c1 (a/chan) c2 (a/chan)]
  (goo "s-default-nil" (a/go (a/alts! [c1 c2] :default nil))))
(let [c1 (a/chan 1) c2 (a/chan)]
  (a/>!! c1 :ready)
  (goo "s-ready-beats-default" (a/go (let [[v p] (a/alts! [c1 c2] :default :d)] [v (= p c1)]))))
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :first) (a/>!! c2 :second)
  (goo "s-priority" (a/go (let [[v p] (a/alts! [c1 c2] :priority true)] [v (= p c1)]))))
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :first) (a/>!! c2 :second)
  (goo "s-priority-rev" (a/go (let [[v p] (a/alts! [c2 c1] :priority true)] [v (= p c2)]))))
(let [c1 (a/chan) c2 (a/chan) g (a/go (let [[v p] (a/alts! [c1 c2])] [v (= p c2)]))]
  (future (Thread/sleep 300) (a/>!! c2 :late))
  (goo "s-woken" g))
(let [c1 (a/chan) g (a/go (a/alts! [[c1 :pv]]))]
  (Thread/sleep 300)
  (goo "s-put-woken" (a/go (let [took (a/<! c1) [v p] (a/<! g)] [took v (= p c1)]))))
(let [c (a/chan 1)]
  (a/>!! c :in-set)
  (goo "s-set" (a/go (let [[v p] (a/alts! #{c})] [v (= p c)]))))
(let [c (a/chan 1)]
  (a/>!! c :x)
  (goo "s-unknown-opt" (a/go (let [[v p] (a/alts! [c] :bogus 1 :default :d)] [v (= p c)]))))
(let [g (a/go (a/alts! [(a/chan)] :default (do :dflt)))]
  (goo "s-default-expr" g))

;; --- alt! inside go ---
(let [c1 (a/chan 1) c2 (a/chan)]
  (a/>!! c1 :ready)
  (goo "t-take" (a/go (a/alt! c1 ([v] [:got v]) c2 ([v] [:other v])))))
(let [c (a/chan 1)]
  (goo "t-put" (a/go (a/alt! [[c :v]] :wrote (a/timeout 5000) :timeout))))
(let [c1 (a/chan) c2 (a/chan 1)]
  (a/>!! c2 :second)
  (goo "t-multi" (a/go (a/alt! [c1 c2] ([v p] [:got v (= p c2)])))))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (goo "t-bind" (a/go (a/alt! c ([v p] [v (some? p)])))))
(let [c (a/chan)]
  (goo "t-default" (a/go (a/alt! c :x :default 42))))
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :first) (a/>!! c2 :second)
  (goo "t-priority" (a/go (a/alt! :priority true c1 :first c2 :second))))
(let [c1 (a/chan) c2 (a/chan 1)]
  (a/>!! c2 :v2)
  (let [g (a/go (a/alt! c1 (a/<! c2)))]
    (future (Thread/sleep 300) (a/>!! c1 :v1))
    (goo "t-nested-take" g)))
(let [c (a/chan 1) d (a/chan 1)]
  (a/>!! d :dv)
  (goo "t-putval-take" (a/go (a/alt! [[c (a/<! d)]] :x)))
  (show "t-putval-took" (a/<!! c)))
(let [c (a/chan)]
  (goo "t-default-take" (a/go (a/alt! (a/timeout 50) :t :default :d))))
(thrown1 "t-outside" #(a/alt! [(a/chan)] :x))
(thrown1 "t-unbalanced" #(eval '(a/alt! [(a/chan)])))

;; --- alt!! (blocking, outside go) ---
(let [c1 (a/chan 1) c2 (a/chan)]
  (a/>!! c1 :ready)
  (show "u-take" (a/alt!! c1 ([v] [:got v]) c2 ([v] [:other v]))))
(let [c (a/chan 1)]
  (show "u-put" [(a/alt!! [[c :v]] :wrote) (a/<!! c)]))
(let [c1 (a/chan) c2 (a/chan 1)]
  (a/>!! c2 :second)
  (show "u-multi" (a/alt!! [c1 c2] ([v p] [:got v (= p c2)]))))
(let [c (a/chan 1)]
  (a/>!! c :v)
  (show "u-bind" (a/alt!! c ([v p] [v (some? p)]))))
(let [c (a/chan)]
  (show "u-default" (a/alt!! c :x :default 42)))
(let [c1 (a/chan 1) c2 (a/chan 1)]
  (a/>!! c1 :first) (a/>!! c2 :second)
  (show "u-priority" (a/alt!! :priority true c1 :first c2 :second)))
(let [c (a/chan)]
  (show "u-dup-default" (a/alt!! c :x :default 1 :default 2)))
(let [c1 (a/chan 1) c2 (a/chan)]
  (a/>!! c1 :v)
  (show "u-mixed" (a/alt!! [[c2 :w] c1] ([v p] [:got v (= p c1)]))))
(thrown1 "u-badport" #(eval '(a/alt!! 42 :x)))
(thrown1 "u-unbalanced" #(eval '(a/alt!! [(a/chan)])))
(thrown1 "u-empty-default" #(eval '(a/alt!! :default :x)))

;; --- go-loop ---
(let [c (a/chan 3)]
  (doseq [x [:a :b :c]] (a/>!! c x))
  (a/close! c)
  (goo "l-drain" (a/go-loop [acc []] (let [v (a/<! c)] (if (nil? v) acc (recur (conj acc v)))))))
(goo "l-count" (a/go-loop [i 5 acc 0] (if (zero? i) acc (recur (dec i) (+ acc i)))))
(let [c (a/chan 1)]
  (a/>!! c :only)
  (goo "l-single" (a/go-loop [] (a/alt! c ([v] [:got v]) (a/timeout 5000) :timeout))))

;; --- alts! errors inside go (take, settle, then print) ---
(let [g (a/go (a/alts! []))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "s-empty" [v (closed? g)]))
(let [c (a/chan 1) g (a/go (a/alts! [[c nil]]))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "s-nil-put" [v (closed? g)]))
(let [g (a/go (a/alts! [42]))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "s-nonchan" [v (closed? g)]))
(let [c (a/chan) g (a/go (a/alts! [c] :default))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "s-odd-opts" [v (closed? g)]))
(let [g (a/go (a/alt! :default :d))
      v (a/<!! g)]
  (Thread/sleep 1000)
  (show "t-empty-default" [v (closed? g)]))
(show "done" :done)
