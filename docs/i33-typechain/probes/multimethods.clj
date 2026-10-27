;; S-typechain oracle probes: stage 3 multimethods (oracle: alpha10 jars, issue #33).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i33-typechain/probes/multimethods.clj > docs/i33-typechain/probes/multimethods.out
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
;; ---- top-level multimethod definitions (certain) ----
(defmulti encounter (fn [x y] [(:species x) (:species y)]))
(defmethod encounter ["Bunny" "Lion"] [x y] :run-away)
(defmethod encounter ["Lion" "Bunny"] [x y] :eat)
(defmethod encounter :default [x y] :ignore)
(defmulti area :shape)
(defmethod area :square [m] (* (:s m) (:s m)))
(defmethod area :circle [m] (* 3 (:r m) (:r m)))
;; ---- A: dispatch ----
(throwof "a-exact" '(encounter {:species "Bunny"} {:species "Lion"}))
(throwof "a-flip" '(encounter {:species "Lion"} {:species "Bunny"}))
(throwof "a-default" '(encounter {:species "Bunny"} {:species "Bunny"}))
(throwof "a-kw" '(area {:shape :square :s 3}))
(throwof "a-circle" '(area {:shape :circle :r 2}))
;; ---- B: table views ----
(throwof "b-methods" '(sort (map str (keys (methods encounter)))))
(throwof "b-get" '((get-method encounter ["Bunny" "Lion"]) {:species "Bunny"} {:species "Lion"}))
(throwof "b-get-miss" '(get-method encounter ["Nope" "Nope"]))
(throwof "b-default-entry" '(contains? (methods encounter) :default))
;; ---- C: remove-method ----
(throwof "c-remove" '(do (defmulti tmpm :k) (defmethod tmpm 1 [m] :one) (remove-method tmpm 1) (tmpm {:k 1})))
(throwof "c-remove-miss" '(do (defmulti tmpn :k) (defmethod tmpn 1 [m] :one) (remove-method tmpn 2) (tmpn {:k 1})))
;; ---- D: prefer-method (no hierarchies in the port, so only
;; the call shape + error pins transfer) ----
(throwcause "d-prefer" '(do (defmulti tmpx :k) (defmethod tmpx :a [m] :a) (defmethod tmpx :b [m] :b) (prefer-method tmpx :a :b)))
(throwcause "e-prefer-nonmulti" '(prefer-method :notmulti :a :b))
;; ---- E: errors ----
(defmulti strictm (fn [x] (:k x)))
(defmethod strictm 1 [m] :one)
(throwcause "e-nomethod" '(strictm {:k 2}))
(throwof "e-nomulti" '(defmethod nosuchmulti 1 [m] 1))
(throwof "e-bad-dispatch" '(do (defmulti tmpd 5) (tmpd 1)))
;; ---- F: class dispatch ----
(defmulti foo class)
(defmethod foo String [s] :string)
(defmethod foo Long [n] :long)
(throwof "f-string" '(foo "hi"))
(throwof "f-long" '(foo 3))
(throwof "f-miss" '(foo :kw))
;; ---- G: multi-arity methods ----
(defmulti marity (fn ([x] (:k x)) ([x y] (:k x))))
(defmethod marity :a ([m] :zero) ([m y] [:one y]))
(throwof "g-zero" '(marity {:k :a}))
(throwof "g-one" '(marity {:k :a} 9))
;; ---- H: var shape ----
(defmulti diet "eats things" (fn [x] (:eats x)))
(throwof "h-map" '(map? encounter))
(throwof "h-doc" '(:doc (meta #'diet)))
