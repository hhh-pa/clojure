(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn show-err [e]
  (let [r (root-cause e)
        xd (try (ex-data e) (catch Throwable _ :no-ex-data))]
    (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r)) "|" (pr-str xd)])))
(def ALLOW {:read-cond :allow})
(def PRESERVE {:read-cond :preserve})
(defn t ([s] (t s ALLOW)) ([s opts]
  (print (str ">>> " (pr-str s) " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr opts)
            forms (loop [acc [o1]]
                    (let [nxt (clojure.lang.LispReader/read rdr (assoc opts :eof EOFMARK))]
                      (if (identical? nxt EOFMARK) acc (recur (conj acc nxt)))))]
        (prn (into [:ok] (map pr-str) forms))))
    (catch Throwable e (show-err e)))))
(doseq [s ["#{[0.0] [-0.0]}" "#{[##NaN] [##NaN]}" "#{(quote (0.0)) (quote (-0.0))}"
           "{{:x 0.0} {:x -0.0}}" "#{#{0.0} #{-0.0}}" "#{[(0.0)] (quote ((-0.0)))}" "#{:a/b :a/b}"
           "{0.0 0 1 1 2 2 3 3 4 4 5 5 6 6 7 7 8 8 9 9 10 10 11 11 12 12 13 13 14 14 15 15 16 16 17 17 18 18 -0.0 99}"]]
  (t s))
(t "#?(:a {#inst \"2020-01-01\" 1 #inst \"2020-01-01\" 2})" PRESERVE)
(println ">>> PROBE8-DONE")
