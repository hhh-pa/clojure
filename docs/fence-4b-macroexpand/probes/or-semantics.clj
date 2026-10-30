;; Fence 4b oracle probes: map-destructure `:or` semantics.
;; Oracle: stock Clojure 1.13.0-alpha9. Run:
;;   java -cp <1.13.0-alpha9 jars> clojure.main or-semantics.clj
;; Output: or-semantics.out (committed). Justifies the F1/F2 reverts
;; (review B1/B2) and records the layer split: `let` spec-gates
;; `:or` (map-only), while the `destructure` fn is lenient
;; (nil/seqables modeled) — the port implements the fn layer
;; (pre-existing; the `let`-spec gap is out of scope).

(defn root-msg [t]
  (loop [t t]
    (if (.getCause ^Throwable t)
      (recur (.getCause ^Throwable t))
      (str (pr-str (type t)) " " (pr-str (.getMessage ^Throwable t))))))

(defn probe [label thunk]
  (println label)
  (prn (try (thunk)
                (catch Throwable t (str "THROW " (root-msg t))))))

(probe "instance-emission:"
  #(macroexpand-1 '(let [{a :a} {}] a)))

(probe "or-hoist:"
  #(macroexpand-1 '(let [{:keys [a] :or {a 1}} {}] a)))

(probe "or-multiple:"
  #(macroexpand-1 '(let [{a :x :or {a 1 :x 2}} {:x 3}] a)))

(probe "or-keyside:"
  #(let [{x :x :or {:x 7}} {}] x))

(probe "or-double-eval:"
  #(let [c (atom 0) {:keys [a] :or {a (swap! c inc)} :select s} {}] [@c s]))

(probe "or-appear-only:"
  #(macroexpand-1 '(let [{a :x :select s :or {:y 1}} {:x 5}] s)))

(println "--- let-spec layer (JVM `let` rejects; port has no let-spec: pre-existing gap) ---")

(probe "let-or-nil:"
  #(eval '(let [{a :a :or nil} {}] a)))

(probe "let-or-empty-vec:"
  #(eval '(let [{:keys [a] :or []} {}] a)))

(println "--- destructure-fn layer (lenient; the port matches this) ---")

(probe "fn-or-nil:"
  #(destructure '[{a :a :or nil} {}]))

(probe "fn-or-empty-vec:"
  #(destructure '[{:keys [a] :or []} {}]))

(probe "fn-or-5:"
  #(destructure '[{:keys [a] :or 5} {}]))

(probe "fn-or-vec1:"
  #(destructure '[{:keys [a] :or [1]} {}]))
