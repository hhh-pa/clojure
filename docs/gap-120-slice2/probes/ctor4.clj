;; #120 slice 2 oracle batch 4: analysis-ordering ground truth
;; (same-form define+use), hostile-var heads, stragglers.
;; Same run header as ctor.clj.
(println (str "file.encoding=" (System/getProperty "file.encoding")))
(defn chain [t]
  (loop [t t, out []]
    (if (nil? t)
      out
      (recur (.getCause t) (conj out (str (.getName (class t)) " | " (.getMessage t)))))))

(defn show [label thunk]
  (try
    (let [v (thunk)]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " THROWS " (.getName (class t)) " | " (.getMessage t))))))

(defn show-chain [label src]
  (try
    (let [v (eval (read-string src))]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " CHAIN:"))
      (doseq [line (chain t)]
        (println (str "   " line))))))

;; --- analysis ordering: same-form define+use ---
(show-chain "o-do-deftype-dot" "(do (deftype DLM [a]) (user.DLM. 1))")
(show-chain "o-do-deftype-new" "(do (deftype DLM2 [a]) (new user.DLM2 1))")
(show-chain "o-do-import-use" "(do (import 'java.util.Date) (Date. 0))")
(show "o-do-import-eval" #(do (import 'java.util.Date) (eval '(Date. 0))))

;; --- hostile-var heads ---
(show-chain "h-var-new" "(do (def CNX5 5) (eval '(CNX5/new 1)))")
(show-chain "h-var-dot" "(do (def CNX6 5) (eval '(CNX6. 1)))")
(show "h-local-static" #(eval '(let [String 5] (String/new "x"))))
(show-chain "h-local-new" "(let [String 5] (new String \"x\" :bogus :bogus2 :bogus3))")

;; --- stragglers ---
(show "s-u32lebom" #(String. (byte-array [-1 -2 0 0 65 0 0 0]) "UTF-32"))
(show-chain "s-spcs" "(String. (byte-array [65]) \" UTF-8\")")
(show "s-4arg-be" #(String. (byte-array [65 66]) 1 1 "UTF-16BE"))
(show-chain "s-new-nil" "(new String nil)")
(show-chain "s-new-arity5" "(new String \"a\" \"b\" \"c\" \"d\" \"e\")")
(show-chain "s-new-str-bad" "(new String \"a\" \"b\")")
