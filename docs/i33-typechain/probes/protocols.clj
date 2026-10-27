;; S-typechain oracle probes: stage 2 protocols + with-open (oracle: alpha10 jars, issue #33).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i33-typechain/probes/protocols.clj > docs/i33-typechain/probes/protocols.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(defn prof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; ---- top-level protocol definitions (certain) ----
(defprotocol Greet "greets things" (greet [this]) (greet2 [this x]))
(defprotocol Adder2 (add2 [this x y]))
(deftype PD [a])
(defrecord PR [x])
;; ---- A: defprotocol shape ----
(throwof "a-map" '(map? Greet))
(throwof "a-sigs" '(sort (map str (keys (:sigs Greet)))))
(throwof "a-on" '(str (:on Greet)))
(throwof "a-doc" '(:doc (meta #'greet)))
(throwof "a-proto-doc" '(:doc (meta #'Greet)))
(throwof "a-fnclass" '(str (class greet)))
;; ---- B: extend-type over stage-1 types ----
(throwof "b-dt" '(do (extend-type PD Greet (greet [this] (str "pd:" (.-a this))) (greet2 [this x] [x])) [(greet (PD. 1)) (greet2 (PD. 1) 2)]))
(throwof "b-rec" '(do (extend-type PR Greet (greet [this] (str "pr:" (:x this)))) (greet (PR. 5))))
(throwof "b-sat-dt" '(satisfies? Greet (PD. 1)))
(throwof "b-sat-rec" '(satisfies? Greet (PR. 1)))
(throwof "b-sat-no" '(satisfies? Greet 5))
(throwof "b-sat-nil" '(satisfies? Greet nil))
(throwof "b-ext-t" '(extends? Greet PD))
(throwof "b-ext-no" '(extends? Greet String))
;; ---- C: extend-protocol over JVM types ----
(throwof "c-str" '(do (extend-protocol Greet String (greet [this] (str "s:" this)) (greet2 [this x] x)) [(greet "hi") (greet2 "hi" 9)]))
(throwof "c-nil" '(do (extend-protocol Greet nil (greet [this] "nil!")) (greet nil)))
(throwof "c-long" '(do (extend-protocol Adder2 Long (add2 [this x y] (+ this x y))) (add2 1 2 3)))
(throwof "c-map" '(do (extend-protocol Adder2 clojure.lang.IPersistentMap (add2 [this x y] [(:x this) x y])) (add2 {:x 1} 2 3)))
;; ---- D: extend map form ----
(throwof "d-map" '(do (extend Double Greet {:greet (fn [this] "dbl") :greet2 (fn [this x] x)}) (greet (double 1.5))))
(throwof "d-sat" '(satisfies? Greet (double 1.5)))
;; ---- E: dispatch errors ----
(throwcause "e-noimpl" '(greet :kw))
(throwcause "e-arity" '(greet (PD. 1) 2 3))
(throwof "e-ext-nonproto" '(extend-type PD :notaproto (m [this] 1)))
(throwof "e-extend-nonproto" '(extend String :notaproto {:m (fn [this] 1)}))
;; ---- F: inline protocol impls ----
(throwof "f-dt-inline" '(do (deftype Q [a] Greet (greet [this] (str "q:" (.-a this)))) (greet (Q. 7))))
(throwof "f-rec-inline" '(do (defrecord QR [x] Greet (greet [this] (str "qr:" (:x this)))) (greet (QR. 8))))
(throwof "f-reify-inline" '(greet (reify Greet (greet [this] "reified"))))
(throwof "f-sat-inline" '(satisfies? Greet (reify Greet (greet [this] "r"))))
;; ---- G: with-open ----
(throwof "g-close" '(let [seen (atom [])] (with-open [c (reify java.io.Closeable (close [this] (swap! seen conj :closed)))] :body) @seen))
(throwof "g-value" '(with-open [c (reify java.io.Closeable (close [this] nil))] :ret))
(throwof "g-exc" '(let [seen (atom [])] (try (with-open [c (reify java.io.Closeable (close [this] (swap! seen conj :closed)))] (throw (ex-info "boom" {}))) (catch clojure.lang.ExceptionInfo e @seen))))
(throwof "g-multi" '(let [seen (atom [])] (with-open [a (reify java.io.Closeable (close [this] (swap! seen conj :a))) b (reify java.io.Closeable (close [this] (swap! seen conj :b)))] :go) @seen))
(throwof "g-nonclose" '(with-open [c 5] c))
