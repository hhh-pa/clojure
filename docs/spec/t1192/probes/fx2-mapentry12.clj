(def e (first {:a 1}))
(defn throwof [label form]
  (println label (try (eval form)
                      (catch Throwable t (str "THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(throwof "merge-with-entry-left:" '(merge-with + e {:b 2}))
(println "with-meta-nonmap" (pr-str (try (with-meta [1] 5) (catch Exception ex (str (class ex) "::" (.getMessage ex))))))
