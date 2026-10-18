;; S3 oracle probes: chunked-seq realization order + nil-punning (C4).
;; Exit 0. Each block prints how many items the transducer fn computed.
(do
  (def realized (atom []))
  (defn rec [x] (swap! realized conj x) x)
  (reset! realized [])
  (doall (take 2 (map rec (range 100))))
  (println "take2-map-range computed =>" (pr-str (count @realized))))
(do
  (def xs (apply list (range 100)))
  (reset! realized [])
  (doall (take 2 (map rec xs)))
  (println "take2-map-list computed =>" (pr-str (count @realized))))
(do
  (reset! realized [])
  (doall (take 2 (map rec (vec (range 100)))))
  (println "take2-map-vector computed =>" (pr-str (count @realized))))
(do
  (reset! realized [])
  (doall (take 35 (map rec (range 100))))
  (println "take35-map-range computed =>" (pr-str (count @realized))))
(do
  (reset! realized [])
  (doall (take 2 (filter odd? (map rec (range 100)))))
  (println "take2-filter-map-range computed =>" (pr-str (count @realized))))
(do
  (reset! realized [])
  (doall (take 2 (map rec (filter odd? (range 100)))))
  (println "take2-map-filter-range computed =>" (pr-str (count @realized))))
(println "(seq nil) =>" (pr-str (seq nil)))
(println "(next nil) =>" (pr-str (next nil)))
(println "(first nil) =>" (pr-str (first nil)))
(println "(rest nil) =>" (pr-str (rest nil)))
(println "(seq []) =>" (pr-str (seq [])))
(println "(next [1]) =>" (pr-str (next [1])))
(println "(rest [1]) =>" (pr-str (rest [1])))
(println "(seq [1]) =>" (pr-str (seq [1])))
