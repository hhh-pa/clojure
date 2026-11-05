(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn show-err [e]
  (let [r (root-cause e)
        xd (try (ex-data e) (catch Throwable _ :no-ex-data))]
    (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r)) "|" (pr-str xd)])))
(def ALLOW {:read-cond :allow})
(defn t [s]
  (print (str ">>> " (pr-str s) " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr ALLOW)
            forms (loop [acc [o1]]
                    (let [nxt (clojure.lang.LispReader/read rdr (assoc ALLOW :eof EOFMARK))]
                      (if (identical? nxt EOFMARK) acc (recur (conj acc nxt)))))]
        (prn (into [:ok] (map pr-str) forms))))
    (catch Throwable e (show-err e))))
(t "{0.0 0 1 1 2 2 3 3 4 4 5 5 6 6 -0.0 7}")
(t "{0.0 0 1 1 2 2 3 3 4 4 5 5 6 6 7 7 -0.0 8}")
(t "{:k0 0 :k1 1 :k2 2 :k3 3 :k4 4 :k5 5 :k6 6 :k7 7 :k8 8 :k0 9}")
(t (str "{[1] :a " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 2 20))) " (1) :b}"))
(t (str "{(1) :a " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 2 20))) " [1] :b}"))
(t (str "{{:x 0.0} :a " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 2 20))) " {:x -0.0} :b}"))
(t "#{{:a 1} {:a 1}}")
(println ">>> PROBE10-DONE")
