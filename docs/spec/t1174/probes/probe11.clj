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
(t "#{{:x 0.0} {:x -0.0}}")
(t (str "{1 0 " (clojure.string/join " " (mapcat (fn [n] [n n]) (range 2 20))) " 1.0 99}"))
(t (str "#{1 " (clojure.string/join " " (range 2 21)) " 1.0}"))
(println ">>> PROBE11-DONE")
