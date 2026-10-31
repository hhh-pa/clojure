;; Oracle g123-block: blocking rendezvous, close/wake interplay, fan-in.
(require '[clojure.core.async :as a])

(defn show [label v]
  (println (str label " => " (pr-str v))))

;; --- unbuffered rendezvous: taker first ---
(let [c (a/chan)
      f (future (a/<!! c))]
  (Thread/sleep 200)
  (show "r-put-ret" (a/>!! c 42))
  (show "r-take-val" @f))

;; --- unbuffered rendezvous: putter first ---
(let [c (a/chan)
      f (future (a/>!! c :v))]
  (Thread/sleep 200)
  (show "p-take-val" (a/<!! c))
  (show "p-put-ret" @f))

;; --- pending take woken by close with nil ---
(let [c (a/chan)
      f (future (a/<!! c))]
  (Thread/sleep 200)
  (a/close! c)
  (show "w-take-val" @f))

;; --- pending put SURVIVES close; a later taker releases it ---
(let [c (a/chan)
      f (future (a/>!! c 7))]
  (Thread/sleep 200)
  (a/close! c)
  (show "k-take-val" (a/<!! c))
  (show "k-put-ret" @f)
  (show "k-take-empty" (a/<!! c)))

;; --- pending put on full fixed buffer survives close; drain refills ---
(let [c (a/chan 1)
      _ (a/>!! c :first)
      f (future (a/>!! c :second))]
  (Thread/sleep 200)
  (a/close! c)
  (show "b-drain" [(a/<!! c) (a/<!! c) (a/<!! c)])
  (show "b-put-ret" @f))

;; --- close hands each pending taker one buffered value, rest nil ---
(let [c (a/chan 1)
      _ (a/>!! c :only)
      f1 (future (a/<!! c))
      f2 (future (a/<!! c))]
  (Thread/sleep 300)
  (a/close! c)
  (show "m-takes" (sort-by str [@f1 @f2])))

;; --- many-to-many fan-in (sorted for determinism) ---
(let [c (a/chan 4)
      puts (doall (for [i (range 8)] (future (a/>!! c i))))
      takes (doall (for [_ (range 8)] (future (a/<!! c))))]
  (show "n-puts" (sort (map deref puts)))
  (show "n-takes" (sort (map deref takes))))

;; --- unbuffered fan-in through rendezvous ---
(let [c (a/chan)
      takes (doall (for [_ (range 4)] (future (a/<!! c))))]
  (Thread/sleep 300)
  (doseq [i (range 4)] (a/>!! c i))
  (show "u-takes" (sort (map deref takes))))
