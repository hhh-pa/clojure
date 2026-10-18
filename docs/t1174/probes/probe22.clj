;; S4 oracle probes: throw/try/catch/finally mechanics + ex-info.
;; Exit 0 (all errors caught and printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t (str (.getName (class t)) " | " (.getMessage t)))))))
(tx "ex-info-roundtrip" #(try (throw (ex-info "m" {:a 1})) (catch clojure.lang.ExceptionInfo e [(ex-message e) (ex-data e)])))
(tx "catch-specificity" #(try (throw (IllegalArgumentException. "x")) (catch IllegalStateException e :ise) (catch IllegalArgumentException e :iae) (catch Exception e :ex)))
(tx "finally-order" #(let [log (atom [])] [(try (do (swap! log conj :body) 1) (catch Exception e :c) (finally (swap! log conj :fin))) @log]))
(tx "finally-on-throw" #(let [log (atom [])] [(try (try (do (swap! log conj :body) (throw (ex-info "e" {}))) (finally (swap! log conj :fin))) (catch Exception e :caught)) @log]))
(tx "(try 1)" #(try 1))
(tx "finally-discards" #(try 1 (finally :f)))
(tx "catch-binds-message" #(try (throw (RuntimeException. "boom")) (catch Exception e (.getMessage e))))
(tx "uncaught-class" #(try (throw (ArithmeticException. "d")) (catch IllegalArgumentException e :no) (catch Throwable t (.getName (class t)))))
(tx "throw-expr-value" #(try (throw (str "e" "rr")) (catch Throwable t (str "caught:" t))))
(tx "nested-try" #(try (try (throw (ex-info "in" {})) (catch IllegalArgumentException e :inner)) (catch Exception e :outer)))
(tx "do-in-try" #(try (do 1 2 3) (catch Exception e :c)))
(tx "try-in-fn" #((fn [] (try (throw (ex-info "f" {})) (catch Exception e :fn-caught)))))
