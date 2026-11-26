(println
  (loop [i 0 data (vec (repeat 2048 1))]
    (if (= i 3)
      [(count data) (first data)]
      (recur (inc i) (vec (repeat 2048 i))))))
(println
  (loop [i 0 data [:old]]
    (if (zero? i)
      (recur ((fn [] (count data))) [:new])
      [i data])))
(println
  (loop [i 0 data [:old] {:keys [item] :or {item data}} {}]
    (if (zero? i)
      (recur (inc i) [:new] {})
      [data item])))
(let [seen (atom [])]
  (println
    (try
      (loop [i 0 data [:old]]
        (recur (do (swap! seen conj :first)
                   (throw (ex-info "stop" {:at :first})))
               (do (swap! seen conj :second) [:new])))
      (catch Exception e (ex-data e)))
    @seen))
