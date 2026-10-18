;; S4 oracle probes, round 1: recur frame aliasing.
;; Exit 0 (all errors caught; cause chains printed).
(defn tx [label f]
  (println label "=>" (pr-str (try (f) (catch Throwable t
    (loop [t t acc []] (if t (recur (.getCause t) (conj acc (str (.getName (class t)) " | " (.getMessage t)))) acc)))))))
(tx "recur-alias-loop" #(eval '(loop [x 2] (if (pos? (- x 1)) (do (def g271 (fn [] x)) (recur 0)) (g271)))))
(tx "recur-alias-fn" #(eval '((fn [x] (if (pos? (- x 1)) (do (def g272 (fn [] x)) (recur 0)) (g272))) 2)))
(tx "trailing-wins" #(eval '(let [{a :a} (quote (:a 1 {:a 9 :b 8}))] [a])))
(tx "trailing-order" #(eval '(let [{:keys [a b] :as m} (quote (:a 1 {:b 2}))] m)))
