(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(def PRESERVE {:read-cond :preserve})
(defn t [s]
  (print (str ">>> " (pr-str s) " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr PRESERVE)]
        (prn [:ok (pr-str o1)])))
    (catch Throwable e
      (let [r (root-cause e)]
        (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r))])))))
(doseq [s ["#?(:a #{#inst [0.0] #inst [-0.0]})" "#?(:a #{#?(:b 1) #?(:b 1)})" "#?(:a #{#?([0.0] x) #?([-0.0] x)})"]]
  (t s))
(println ">>> PROBE15-DONE")
