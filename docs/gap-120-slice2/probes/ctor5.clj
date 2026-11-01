;; #120 slice 2 oracle batch 5: error-order ground truth.
;; Same run header as ctor.clj.
(defn chain [t]
  (loop [t t, out []]
    (if (nil? t)
      out
      (recur (.getCause t) (conj out (str (.getName (class t)) " | " (.getMessage t)))))))

(defn show-chain [label src]
  (try
    (let [v (eval (read-string src))]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " CHAIN:"))
      (doseq [line (chain t)]
        (println (str "   " line))))))

(show-chain "e-badboth" "(String. (byte-array [65]) 5 1 \"NOPE-CS\")")
(show-chain "e-nil-badbounds" "(String. nil 5 1 \"UTF-8\")")
(show-chain "e-nil-badcs" "(String. nil 0 1 \"NOPE-CS\")")
