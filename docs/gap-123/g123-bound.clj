;; Oracle g123-bound: MAX-QUEUE-SIZE (1024) pending-op bound messages.
(require '[clojure.core.async :as a])

(defn thrown1 [label thunk]
  (try (thunk)
       (println (str label " => NO-THROW (unexpected!)"))
       (catch Throwable t
         (println (str label " => THROW " (.getName (class t)) " | " (pr-str (.getMessage t)))))))

;; 1024 pending takes queued; the 1025th take must fail.
(let [c (a/chan)
      fs (doall (for [_ (range 1024)] (future (a/<!! c))))]
  (Thread/sleep 3000)
  (thrown1 "e-take-bound" #(a/<!! c))
  (a/close! c)
  (doseq [f fs] @f)
  (println "q-takes-released => true"))

;; 1024 pending puts queued; the 1025th put must fail.
(let [c (a/chan)
      fs (doall (for [i (range 1024)] (future (a/>!! c i))))]
  (Thread/sleep 3000)
  (thrown1 "e-put-bound" #(a/>!! c :extra))
  (a/close! c)
  (let [vals (sort (for [_ (range 1024)] (a/<!! c)))]
    (println (str "q-puts-drained => " (pr-str [(first vals) (last vals) (count vals)]))))
  (doseq [f fs] @f)
  (println "q-puts-released => true"))
