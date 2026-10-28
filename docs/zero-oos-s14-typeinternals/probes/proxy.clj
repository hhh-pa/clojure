;; S14 type-internals oracle probes: proxy helpers (oracle: alpha10 jars, issue #58).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s14-typeinternals/probes/proxy.clj > docs/zero-oos-s14-typeinternals/probes/proxy.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
(defn show [label form]
  (println (str label " " (normhex (pr-str (eval form))))))
;; ---- A: proxy-name ----
(show "a-name-obj" '(proxy-name Object []))
(show "a-name-runnable" '(proxy-name Object [Runnable]))
(show "a-name-two" '(proxy-name Object [Runnable java.io.Serializable]))
(show "a-name-str" '(proxy-name String []))
(show "a-name-vec" '(proxy-name Object (list Runnable)))
(throwcause "a-name-nil" '(proxy-name nil []))
(throwcause "a-name-noclass" '(proxy-name 5 []))
(throwcause "a-name-arity" '(proxy-name Object))
;; ---- B: get-proxy-class ----
(show "b-class" '(class (get-proxy-class Object)))
(show "b-name" '(.getName ^Class (get-proxy-class Object)))
(show "b-name-iface" '(.getName ^Class (get-proxy-class Object Runnable)))
(show "b-cached" '(identical? (get-proxy-class Object) (get-proxy-class Object)))
(show "b-default-super" '(.getName ^Class (get-proxy-class Runnable)))
(throwcause "b-none" '(get-proxy-class))
(throwcause "b-final" '(get-proxy-class String))
(throwcause "b-nonclass" '(get-proxy-class 5))
;; ---- C: construct-proxy ----
(show "c-class" '(class (construct-proxy (get-proxy-class Object))))
(show "c-mappings-fresh" '(proxy-mappings (construct-proxy (get-proxy-class Object))))
(throwcause "c-ctor-args" '(construct-proxy (get-proxy-class Object) 1 2))
(throwcause "c-nonclass" '(construct-proxy "foo"))
(throwcause "c-arity" '((var construct-proxy)))
;; ---- D: init/update/mappings ----
(show "d-init" '(let [p (construct-proxy (get-proxy-class Object))
                      m {"toString" (fn [this] "hi")}
                      q (init-proxy p m)]
                  [(identical? p q) (= (proxy-mappings p) m)]))
(show "d-call" '(let [p (construct-proxy (get-proxy-class Object))]
                  (init-proxy p {"toString" (fn [this] "hi")})
                  (str p)))
(show "d-update" '(let [p (construct-proxy (get-proxy-class Object))]
                  (init-proxy p {"toString" (fn [this] "hi") "hashCode" (fn [this] 42)})
                  (update-proxy p {"toString" (fn [this] "yo")})
                  [(str p) (sort (keys (proxy-mappings p)))]))
(show "d-nil" '(let [p (construct-proxy (get-proxy-class Object))]
                 (init-proxy p {"toString" (fn [this] "hi")})
                 (update-proxy p {"toString" nil})
                 [(contains? (proxy-mappings p) "toString") (get (proxy-mappings p) "toString")]))
(show "d-macro-map" '(let [p (proxy [Object] [] (toString [] "mx"))]
                       [(sort (keys (proxy-mappings p))) (str p)]))
(throwcause "d-init-nonproxy" '(init-proxy 5 {}))
(throwcause "d-init-nonmap" '(let [p (construct-proxy (get-proxy-class Object))] (init-proxy p 5)))
(throwcause "d-update-nonproxy" '(update-proxy 5 {}))
(throwcause "d-update-nonmap" '(let [p (construct-proxy (get-proxy-class Object))] (update-proxy p 5)))
(throwcause "d-mappings-nonproxy" '(proxy-mappings 5))
;; ---- E: method-sig ----
(defn gm [c n ps] (. ^Class c getMethod n (into-array Class ps)))
(show "e-tostring" '(method-sig (gm Object "toString" [])))
(show "e-valueof" '(method-sig (gm String "valueOf" [Object])))
(show "e-shape" '(let [[n ps r] (method-sig (gm Object "toString" []))]
                   [(string? n) (nil? ps) (class? r)]))
(throwcause "e-arity" '((var method-sig)))
(throwcause "e-nonmethod" '(method-sig "toString"))
;; ---- F: proxy-call-with-super / proxy-super ----
(show "f-super" '(let [p (proxy [Object] []
                             (toString []
                               (str "sub:" (proxy-call-with-super (fn [] (. this toString)) this "toString"))))]
                   (str p)))
(show "f-macro" '(macroexpand-1 '(proxy-super toString)))
(show "f-supermacro" '(let [p (proxy [Object] [] (toString [] (str "S:" (proxy-super toString))))]
                        (str p)))
(show "f-restored" '(let [p (proxy [Object] [] (toString [] "mx"))]
                      (proxy-call-with-super (fn [] :called) p "toString")
                      [(sort (keys (proxy-mappings p))) (str p)]))
(throwcause "f-arity" '((var proxy-call-with-super) (fn [] 1)))
