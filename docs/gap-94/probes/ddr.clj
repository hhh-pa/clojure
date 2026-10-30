;; Gap #94 oracle probes: `default-data-readers` root consult in readTagged
;; (oracle: 1.13.0-alpha9 jars in /tmp/clj113).
;; Run from repo root:
;; CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:/tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/gap-94/probes/ddr.clj > docs/gap-94/probes/ddr.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwdata: pin the ex-data too (read errors carry line/column).
(defn throwdata [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)) "|" (pr-str (ex-data t)))))))
;; Run (thunk) with the default-data-readers root swapped, restored after.
(defn with-ddr [root thunk]
  (let [saved default-data-readers]
    (try
      (alter-var-root #'default-data-readers (constantly root))
      (thunk)
      (finally (alter-var-root #'default-data-readers (constantly saved))))))
(defn h-reader [x] [:v x])
;; ---- H: wiped root ----
(throwof "h-wipe-nil" '(with-ddr nil (fn [] (read-string "#inst \"2024-01-01\""))))
(throwdata "h-wipe-nil-data" '(with-ddr nil (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-wipe-empty" '(with-ddr {} (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-wipe-uuid" '(with-ddr nil (fn [] (read-string "#uuid \"550e8400-e29b-41d4-a716-446655440000\""))))
(throwof "h-wipe-badpayload" '(with-ddr nil (fn [] (read-string "#inst \"bogus\""))))
(throwof "h-wipe-defaultfn" '(with-ddr nil (fn [] (binding [*default-data-reader-fn* (fn [tag x] [:d tag x])] (read-string "#inst \"2024-01-01\"")))))
(throwof "h-wipe-table" '(with-ddr nil (fn [] (binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string "#foo 1")))))
;; ---- H: borked root ----
(throwof "h-bork-long" '(with-ddr 42 (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-bork-long-foo" '(with-ddr 42 (fn [] (read-string "#foo 1"))))
(throwdata "h-bork-long-data" '(with-ddr 42 (fn [] (read-string "#foo 1"))))
(throwof "h-bork-str" '(with-ddr "x" (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-bork-vec" '(with-ddr [(fn [x] :v)] (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-bork-tablehit" '(with-ddr 42 (fn [] (binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string "#foo 1")))))
(throwof "h-bork-defaultfn" '(with-ddr 42 (fn [] (binding [*default-data-reader-fn* (fn [tag x] :d)] (read-string "#foo 1")))))
;; ---- H: extra / replaced entries ----
(throwof "h-extra" '(with-ddr (assoc default-data-readers 'foo (fn [x] [:ok x])) (fn [] (read-string "#foo 1"))))
(throwof "h-extra-ns" '(with-ddr (assoc default-data-readers 'foo/bar (fn [x] [:ok x])) (fn [] (read-string "#foo/bar 1"))))
(throwof "h-replace-inst" '(with-ddr (assoc default-data-readers 'inst (fn [x] [:mine x])) (fn [] (read-string "#inst \"2024-01-01\""))))
(throwof "h-replace-inst-bad" '(with-ddr (assoc default-data-readers 'inst (fn [x] [:mine x])) (fn [] (read-string "#inst \"bogus\""))))
(throwof "h-replace-inst-nonstr" '(with-ddr (assoc default-data-readers 'inst (fn [x] [:mine x])) (fn [] (read-string "#inst 42"))))
(throwof "h-replace-uuid" '(with-ddr (assoc default-data-readers 'uuid (fn [x] [:mine x])) (fn [] (read-string "#uuid \"550e8400-e29b-41d4-a716-446655440000\""))))
(throwof "h-nil-entry" '(with-ddr (assoc default-data-readers 'foo nil) (fn [] (read-string "#foo 1"))))
(throwof "h-nonfn" '(with-ddr (assoc default-data-readers 'foo 42) (fn [] (read-string "#foo 1"))))
(throwof "h-var-value" '(with-ddr (assoc default-data-readers 'foo #'h-reader) (fn [] (read-string "#foo 1"))))
(throwof "h-vs-defaultfn" '(with-ddr (assoc default-data-readers 'foo (fn [x] [:ok x])) (fn [] (binding [*default-data-reader-fn* (fn [tag x] :d)] (read-string "#foo 1")))))
(throwof "h-strkey" '(with-ddr {"foo" (fn [x] :s)} (fn [] (read-string "#foo 1"))))
;; ---- H: root restored after the swaps ----
(throwof "h-restored" '(read-string "#inst \"2024-01-01\""))
