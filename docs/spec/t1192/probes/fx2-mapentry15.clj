(def e (first {:a 1}))
(defn show [label v] (println label (pr-str v)))
(defn throwof [label form]
  (println label (try (eval form)
                      (catch Throwable t (str "THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "push-vec-scalar:" '(push-thread-bindings [1]))
(show "case-miss:" (case e [:b 2] :hit :miss))
(show "get-in-nested:" (get-in [e] [0 1]))
(show "update-entry:" (update e 0 name))
