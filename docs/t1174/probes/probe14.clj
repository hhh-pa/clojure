(def EOFMARK (Object.))
(defn root-cause [e] (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(def ALLOW {:read-cond :allow})
(defn t [s]
  (print (str ">>> " (pr-str s) " "))
  (flush)
  (try
    (binding [*read-eval* false]
      (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
            o1 (clojure.lang.LispReader/read rdr ALLOW)]
        (prn [:ok (pr-str o1)])))
    (catch Throwable e
      (let [r (root-cause e)]
        (prn [:err (str (.getName ^Class (class r))) "|" (str (.getMessage ^Throwable r))])))))
(doseq [s ["{(0.0) :a [-0.0] :b}" "#{[(0.0)] ((-0.0))}" "{[-0.0] :a (0.0) :b}" "{(1) :a [1.0] :b}"
           "{(quote (1)) :a [1] :b}" "{[1] :a (quote (1)) :b}"]]
  (t s))
(println ">>> PROBE14-DONE")
