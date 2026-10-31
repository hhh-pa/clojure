;; Oracle g123-xform: chan transducers + ex-handlers (slice 2).
;; Run: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC
;;   -cp <jars> clojure.main docs/gap-123/g123-xform.clj
;; (same <jars> as probes.md; one fresh JVM).
(require '[clojure.core.async :as a]
         '[clojure.core.async.impl.protocols :as impl])

(def closed? impl/closed?)

(defn show [label v]
  (println (str label " => " (pr-str v))))

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW " (pr-str :done)))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

(defn boom [v]
  (throw (ex-info "boom" {:n v})))

;; --- X: basic transforms ---
(let [c (a/chan 2 (map inc))]
  (show "m-puts" [(a/>!! c 1) (a/>!! c 2)])
  (show "m-takes" [(a/<!! c) (a/<!! c)]))
(let [c (a/chan 2 (filter odd?))]
  (show "f-puts" [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3)])
  (show "f-takes" [(a/<!! c) (a/<!! c)]))
(let [c (a/chan 4 (mapcat identity))]
  (show "c-put" (a/>!! c [1 2]))
  (show "c-takes" [(a/<!! c) (a/<!! c)]))
(let [c (a/chan 4 (comp (map inc) (filter odd?)))]
  (show "k-puts" [(a/>!! c 1) (a/>!! c 2) (a/>!! c 3)])
  (show "k-take" (a/<!! c)))
(let [c (a/chan (a/dropping-buffer 1) (mapcat identity))]
  (show "d-put" (a/>!! c [1 2]))
  (show "d-takes" [(a/<!! c) (a/alts!! [c] :default :empty)]))
(let [c (a/chan (a/sliding-buffer 1) (map inc))]
  (show "s-puts" [(a/>!! c 1) (a/>!! c 2)])
  (show "s-take" (a/<!! c)))
(let [c (a/chan 4 (dedupe))]
  (show "u-puts" [(a/>!! c 1) (a/>!! c 1) (a/>!! c 2)])
  (show "u-takes" [(a/<!! c) (a/<!! c)]))
(let [c (a/chan 2 (partition-all 2))]
  (a/>!! c 1) (a/>!! c 2)
  (show "p-take1" (a/<!! c))
  (a/>!! c 3) (a/close! c)
  (show "p-take2" (a/<!! c))
  (show "p-take3" (a/<!! c)))

;; --- R: reduced closes the channel ---
(let [c (a/chan 2 (take 2))]
  (show "t-puts" [(a/>!! c 1) (a/>!! c 2)])
  (show "t-closed" (closed? c))
  (show "t-takes" [(a/<!! c) (a/<!! c) (a/<!! c)])
  (show "t-put-after" (a/>!! c 9)))
(let [c (a/chan 1 (take 2))]
  (a/>!! c 1)
  (let [f (future (a/>!! c 2))]
    (Thread/sleep 800)
    (show "t-park-takes" [(a/<!! c) (a/<!! c) (a/<!! c)])
    (show "t-park-f" (deref f 8000 :timeout))
    (show "t-park-closed" (closed? c))))
(let [c (a/chan 1 (take 2))]
  (a/>!! c 1)
  (let [f (future (a/>!! c 2))]
    (Thread/sleep 800)
    (let [g (future (a/>!! c 3))]
      (Thread/sleep 800)
      (show "t-g-takes" [(a/<!! c) (a/<!! c) (a/<!! c)])
      (show "t-g-f" (deref f 8000 :timeout))
      (show "t-g-g" (deref g 8000 :timeout)))))
(let [c (a/chan 1 (take 1))]
  (let [t1 (future (a/<!! c)) t2 (future (a/<!! c))]
    (Thread/sleep 800)
    (show "t-w-put" (a/>!! c 1))
    (show "t-w-t1" (deref t1 8000 :timeout))
    (show "t-w-t2" (deref t2 8000 :timeout))))
(let [c (a/chan 1 (comp (mapcat identity) (take 2)))]
  (a/>!! c [:a])
  (let [f (future (a/>!! c [:b :c]))]
    (Thread/sleep 800)
    (show "t-m-takes" [(a/<!! c) (a/<!! c) (a/<!! c) (a/<!! c)])
    (show "t-m-f" (deref f 8000 :timeout))))

;; --- E: ex-handler application ---
(let [seen (atom nil)
      c (a/chan 2 (map boom) (fn [t] (reset! seen t) nil))]
  (show "h-nil-put" (a/>!! c 1))
  (show "h-nil-class" (str (class @seen)))
  (show "h-nil-msg" (ex-message @seen))
  (show "h-nil-data" (ex-data @seen))
  (show "h-nil-take" (a/alts!! [c] :default :empty)))
(let [c (a/chan 2 (map boom) (fn [t] :fallback))]
  (show "h-rep-put" (a/>!! c 1))
  (show "h-rep-take" (a/<!! c)))
(let [c (a/chan 2 (map boom) (fn [t] false))]
  (show "h-false-put" (a/>!! c 1))
  (show "h-false-take" (a/alts!! [c] :default :empty)))
;; NOTE: default ex-handler prints a stack trace to stderr.
(let [c (a/chan 2 (map boom))]
  (show "h-dflt-put" (a/>!! c 1))
  (show "h-dflt-take" (a/alts!! [c] :default :empty)))
(let [c (a/chan 2 (map boom) false)]
  (show "h-false-exh-put" (a/>!! c 1))
  (show "h-false-exh-take" (a/alts!! [c] :default :empty)))
(thrown1 "h-exh-throws" #(let [c (a/chan 2 (map boom) (fn [t] (throw (ex-info "from-exh" {}))))]
                           (a/>!! c 1)))
(let [txf (fn [rf] (fn ([r] (throw (ex-info "c-done" {}))) ([r v] (rf r v))))]
  (thrown1 "h-cdone-nil" #(let [c (a/chan 1 txf (fn [t] nil))] (a/close! c)))
  (thrown1 "h-cdone-throws" #(let [c (a/chan 1 txf (fn [t] (throw (ex-info "from-exh" {}))))]
                               (a/close! c))))
;; NOTE: sliding-zero evict crash inside add! -> default handler swallows.
(let [c (a/chan (a/sliding-buffer 0) (map inc))]
  (show "h-evict-put" (a/>!! c 1))
  (show "h-evict-take" (a/alts!! [c] :default :empty)))
;; NOTE: the :r replacement re-triggers the zero-size evict crash,
;; which is outside handle's try, so the put itself throws.
(thrown1 "h-evict-rep-put" #(let [c (a/chan (a/sliding-buffer 0) (map inc) (fn [t] :r))]
                              (a/>!! c 1)))
(let [seen (atom nil)
      c (a/chan (a/sliding-buffer 1) (map boom) (fn [t] (reset! seen t) :r))]
  (show "h-rep1-put" (a/>!! c 1))
  (show "h-rep1-class" (str (class @seen)))
  (show "h-rep1-take" (a/<!! c)))

;; --- C: completion arity ---
(let [cnt (atom 0)
      txf (fn [rf] (fn ([r] (swap! cnt inc) (rf r)) ([r v] (rf r v))))
      c (a/chan 1 txf)]
  (let [t (future (a/<!! c))]
    (Thread/sleep 500)
    (show "c-parked" @cnt)
    (a/>!! c 1)
    (show "c-parked-t" (deref t 8000 :timeout)))
  (show "c-pre" @cnt)
  (a/close! c)
  (show "c-after-close" @cnt)
  (a/<!! c)
  (show "c-after-take1" @cnt)
  (a/<!! c)
  (show "c-after-take2" @cnt)
  (a/<!! c)
  (show "c-after-take3" @cnt))
(let [c (a/chan 1 (partition-all 2))]
  (a/>!! c 1) (a/>!! c 2)
  (let [f (future (a/>!! c 3))]
    (Thread/sleep 500)
    (show "c-f3" (deref f 8000 :timeout)))
  (let [g (future (a/>!! c 4))]
    (Thread/sleep 800)
    (let [h (future (a/>!! c 5))]
      (Thread/sleep 500)
      (show "c-h5" (deref h 8000 :timeout))
      (a/close! c)
      (show "c-flush" [(a/<!! c) (a/<!! c) (a/<!! c) (a/<!! c)])
      (show "c-g" (deref g 8000 :timeout)))))
(let [txf (fn [rf] (fn ([r] (reduced r)) ([r v] (rf r v))))
      c (a/chan 1 txf)]
  (a/>!! c 1) (a/close! c)
  (show "c-red-takes" [(a/<!! c) (a/<!! c)]))

;; --- B: errors and edges ---
(thrown1 "e-chan-42" #(a/chan 1 42))
(thrown1 "e-nil-xf" #(a/chan nil (map inc)))
(let [c (a/chan 1 (fn [rf] (fn ([r] r) ([r v] 42))))]
  (show "e-degen-put" (a/>!! c 1))
  (show "e-degen-take" (a/alts!! [c] :default :empty)))
(let [c (a/chan 1 (filter odd?))]
  (a/>!! c 1)
  (show "b-full-drop" (deref (future (a/>!! c 2)) 600 :pending)))
(let [cnt (atom 0)
      c (a/chan 1 (map (fn [v] (swap! cnt inc) v)))]
  (a/>!! c 1) (a/close! c)
  (show "b-closed-put" (a/>!! c 2))
  (show "b-closed-steps" @cnt))

;; --- A: alts with xforms ---
(let [c (a/chan 1 (map inc))]
  (a/>!! c 0)
  (show "a-full-dflt" (a/alts!! [[c 41]] :default :d)))
(let [c (a/chan 1 (map inc))]
  (a/close! c)
  (show "a-closed" (a/alts!! [c])))
(let [c (a/chan 1 (mapcat identity))]
  (a/>!! c [1])
  (show "a-stale" (a/alts!! [[c [2 3]]] :default :d))
  (show "a-stale-takes" [(a/<!! c) (a/alts!! [c] :default :empty)]))
(let [c (a/chan 1 (map inc))]
  (a/>!! c 0)
  (let [f (future (a/alts!! [[c 41]]))]
    (Thread/sleep 800)
    (show "a-w-take" (a/<!! c))
    (show "a-w-f" (deref f 8000 :timeout))
    (show "a-w-take2" (a/<!! c))))

;; --- Q: stepping granularity (ports pins + divergence record) ---
(let [cnt (atom 0)
      txf (fn [rf] (fn ([r] (swap! cnt inc) (rf r)) ([r v] (rf r v))))
      c (a/chan 1 txf)]
  (a/>!! c 1)
  (a/close! c)
  (show "q-d-pre" @cnt)
  (show "q-d-t1" (a/<!! c))
  (show "q-d-mid" @cnt)
  (show "q-d-t2" (a/<!! c))
  (show "q-d-end" @cnt))
(let [c (a/chan 1 (mapcat identity))]
  (a/>!! c [1])
  (let [f (future (a/>!! c [2 3]))]
    (Thread/sleep 800)
    (show "q-m-takes" [(a/<!! c) (a/<!! c) (a/<!! c)])
    (show "q-m-f" (deref f 8000 :timeout))))
(let [c (a/chan 1 (filter odd?))]
  (a/>!! c 1)
  (let [f (future (a/>!! c 2))]
    (Thread/sleep 800)
    (show "q-f0-parked" (deref f 200 :pending))
    (show "q-f0-take" (a/<!! c))
    (show "q-f0-f" (deref f 8000 :timeout))))
(let [calls (atom [])
      txf (comp (map (fn [v] (swap! calls conj v) v)) (take 2))
      c (a/chan 1 txf)]
  (a/>!! c 0)
  (let [f (future (a/>!! c 1))]
    (Thread/sleep 800)
    (let [g (future (a/>!! c 2))]
      (Thread/sleep 800)
      (show "q-calls-parked" @calls)
      (show "q-take1" (a/<!! c))
      (show "q-calls-after" @calls)
      (show "q-f" (deref f 8000 :timeout))
      (show "q-g" (deref g 8000 :timeout))
      (show "q-takes" [(a/<!! c) (a/<!! c)]))))
