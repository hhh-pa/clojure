(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
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
    (catch Throwable e
      (let [r (root-cause e)]
        (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r))])))))
(doseq [s ["'#?@(:clj (1 2)) X" "'#?@(:clj (1 2))" "#(1 #?@(:clj (2 3)))"]]
  (t s))
(println ">>> PROBE16-DONE")
