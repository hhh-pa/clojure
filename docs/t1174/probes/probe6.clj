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
(doseq [s ["#{{} {}}" "{{} 1 {} 2}" "#{() ()}" "#{[] []}" "{(1) :a [1] :b}" "{[1.0] :a [1] :b}"
           "#{1 2 3 4 5 6 7 8 9 10 11 12 13 14 15 16 17 18 19 1.0}" "#{1 :a \"s\" :a}"
           "{:a 1 :b 2 :a 3}"
           "{1 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10 11 11 12 12 13 13 14 14 15 15 16 16 17 17 1 1}"]]
  (t s))
(println ">>> PROBE6-DONE")
