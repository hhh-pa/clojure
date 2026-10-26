;; T1192 FX4 macroexpand-1 probe (oracle: Clojure 1.13.0-alpha9).
;; Gensym ids are unstable across JVM boots; pin shape, not the number.
;; Run:
;;   java -Duser.language=en -Duser.country=US -Duser.timezone=UTC \
;;     -cp <clojure + spec.alpha + core.specs.alpha> \
;;     clojure.main docs/t1192/probes/fx4-macroexpand.clj
(require '[clojure.string :as str])

(defn causes [t]
  (loop [t t out []]
    (if t
      (recur (.getCause t)
             (conj out (str (.getName (class t)) " | " (.getMessage t))))
      out)))

(defn show [label form]
  (try
    (println label "=>" (pr-str (macroexpand-1 form)))
    (catch Throwable t
      (println label "ERR" (str/join " ;; " (causes t))))))

(defn show-full [label form]
  (try
    (println label "=>" (pr-str (macroexpand form)))
    (catch Throwable t
      (println label "ERR" (.getName (class t)) "|" (.getMessage t)))))

(defn show-eval [label form]
  (try
    (println label "=>" (pr-str (eval form)))
    (catch Throwable t
      (println label "ERR" (.getName (class t)) "|" (.getMessage t)))))

(println "---- expand-1 ----")
(show "let" '(let [a 1] a))
(show "loop" '(loop [a 1] (if (pos? a) (recur (dec a)) a)))
(show "fn1" '(fn [a] a))
(show "fn-multi" '(fn ([a] a) ([a b] b)))
(show "fn-name" '(fn f ([a] a)))
(show "binding" '(binding [*out* *out*] 1))
(show "letfn" '(letfn [(f [a] a)] (f 1)))
(show "with-redefs" '(with-redefs [*out* *out*] 1))
(show "with-bindings" '(with-bindings {(var *out*) *out*} 1))
(show "bound-fn" '(bound-fn [x] x))
(show "locking" '(locking *out* 1))
(show "qlet" '(clojure.core/let [a 1] a))
(show "declare" '(declare a))
(show "ns" '(ns foo))
(show "defmacro" '(defmacro m [x] x))
(show "assert" '(assert true))
(show "time" '(time 1))
(show "wos" '(with-out-str 1))

(println "---- destructure let ----")
(binding [*print-meta* true]
  (show "let-vec" '(let [[a b] v] a))
  (show "let-amp" '(let [[a & r] v] a))
  (show "let-map" '(let [{:keys [a] :or {a 1} :as m} v] a))
  (show "let-keys!" '(let [{:keys! [a] :missing m} v] a))
  (show "fn-dest" '(fn [[a b]] a))
  (show "fn-pre" '(fn [x] {:pre [(pos? x)] :post [(number? %)]} x))
  (show "fn-meta" '(fn [a] a)))

(println "---- loop destructure ----")
(show "loop-vec" '(loop [[a b] v] a))
(show "loop-map" '(loop [{:keys [a]} v] a))

(println "---- arity / shape ----")
(show "let0" '(let))
(show "let-novec" '(let 1))
(show "let-odd" '(let [a] 1))
(show "let-bad" '(let [1 2] 1))
(show "loop0" '(loop))
(show "binding0" '(binding))
(show "letfn0" '(letfn))
(show "letfn-bad" '(letfn 5 1))
(show "wrd0" '(with-redefs))
(show "wrd-long" '(with-redefs 5 1))
(show "wb0" '(with-bindings))
(show "lock0" '(locking))
(show "fn0" '(fn))
(show "fn5" '(fn 5))
(show "fn-nil" '(fn ()))
(show "fn-sig" '(fn ([a] a) 5))
(show "amp-bad" '(let [[a & b c] [1]] a))
(show "dir-bad" '(let [{:foo [a]} {}] a))
(show "amp2" '(let [{:keys [a & b & c]} {}] a))
(show "amp-sym" '(let [{:keys [a & b]} {}] a))
(show "or-both" '(let [{a a :or {a 1}} {}] a))
(show "or-req" '(let [{:keys! [a] :or {a 1}} {}] a))
(show "or-sym" '(let [{:keys [a] :or {b 1} :select s} {}] s))
(show "or-only" '(let [{:keys [a] :or {a 1 :z 2} :select s} {}] s))
(show-eval "direct-let" '((var-get (var let)) 1))
(show-eval "direct-declare" '((var-get (var declare)) 1))

(println "---- req ----")
(show-eval "req" '(clojure.core/req! {} :a))

(println "---- eval smoke ----")
(show-eval "ev-let" '(let [a 1] a))
(show-eval "ev-vec" '(let [[a b] [1 2]] [a b]))
(show-eval "ev-keys" '(let [{:keys [a] :or {a 5}} {}] a))
(show-eval "ev-fn" '((fn [a] a) 3))
(show-eval "ev-pre" '((fn [x] {:pre [(pos? x)]} x) -1))
(show-eval "ev-loop" '(loop [a 2] (if (pos? a) (recur (dec a)) a)))
(show-eval "ev-letfn" '(letfn [(f [n] (if (pos? n) (f (dec n)) n))] (f 3)))
(show-eval "ev-lock" '(locking :x 1))

(println "---- coverage top-up (PR #22) ----")
(show "fn-multi-body" '(fn [x] x x))
(show-eval "ev-loop-mixed" '(loop [x 1 [a] [2]] [x a]))
(show-eval "ev-fn-post-multi" '((fn [x] {:post [(pos? x)]} (inc x) x) 2))
(show-eval "ev-fn-mixed" '((fn [x [a b]] [x a b]) 1 [2 3]))
(show "let-bad-vec" '(let [[1] [1]] 1))
(show-eval "ev-keys-kw" '(let [{:keys [:a]} {:a 1}] a))
(show-eval "ev-keys-nil" '(let [{:keys nil} {:a 1}] 42))
(show-eval "ev-keys-set" '(let [{:keys #{a}} {:a 1}] a))
(show-eval "ev-keys-dup" '(let [{:keys [a a]} {:a 1}] a))
(show-eval "ev-select-nested" '(let [{{:keys [x] :select t} :p :select s} {:p {:x 1}}] [x t s]))
(show-eval "ev-keys-bang" '(let [{:keys! [a]} {:a 1}] a))
(show-eval "ev-keys-bang-missing" '(let [{:keys! [a]} {}] a))
(show-eval "ev-nth-object" '(nth (new Object) 0))
(show-eval "ev-wrd-nested" '(do (def wrdt 11) (let [r ((fn [] (with-redefs [wrdt 2] wrdt)))] (ns-unmap *ns* 'wrdt) r)))
(show-eval "ev-wrd-list" '(do (def wrdt 11) (let [r ((fn [] (with-redefs (wrdt 2) wrdt)))] (ns-unmap *ns* 'wrdt) r)))
(show-eval "ev-lock-nil" '((fn [] (locking nil 1))))
(show-eval "ev-invoke-entry-kw" '(:a (first {:a 1})))
(show-eval "ev-seq-rest" '(let [[a & r] (first {:a 1})] [a r]))
