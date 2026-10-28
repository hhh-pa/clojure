;; S12 oracle probes: structmaps (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/structmaps.clj > docs/zero-oos-s12-collmisc/probes/structmaps.out
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
(defstruct person :name :age)
(def other (create-struct :name :age))
;; ---- basis ----
(throwof "sm-basis-class" '(str (class (create-struct :a :b))))
(throwcause "sm-basis-print" '(pr-str (create-struct :a :b)))
(throwof "sm-defstruct-expand" '(pr-str (macroexpand-1 '(defstruct person2 :x :y))))
(throwof "sm-defstruct-class" '(str (class person)))
(throwof "sm-basis-eq" '(= (create-struct :a :b) (create-struct :a :b)))
;; ---- map-ness ----
(throwof "sm-make-print" '(pr-str (struct-map person :name "Ada" :age 36)))
(throwof "sm-make-class" '(str (class (struct-map person :name "Ada"))))
(throwof "sm-map?" '(map? (struct-map person)))
(throwof "sm-eq-map" '(= (struct-map person :name "Ada" :age 36) {:name "Ada" :age 36}))
(throwof "sm-hash-eq" '(= (hash (struct-map person :name "Ada" :age 36)) (hash {:name "Ada" :age 36})))
(throwof "sm-keys" '(pr-str (keys (struct-map person :name "Ada"))))
(throwof "sm-vals" '(pr-str (vals (struct-map person :name "Ada"))))
(throwof "sm-seq" '(pr-str (seq (struct-map person :name "Ada"))))
(throwof "sm-count" '(count (struct-map person :name "Ada")))
(throwof "sm-get-basis" '(get (struct-map person :name "Ada") :age))
(throwof "sm-contains" '(contains? (struct-map person) :age))
(throwof "sm-extra" '(pr-str (struct-map person :name "Ada" :zzz 9)))
(throwof "sm-empty" '(pr-str (struct-map person)))
(throwof "sm-cross-basis" '(= (struct-map person :name "Ada") (struct-map other :name "Ada")))
;; ---- struct ----
(throwof "sm-struct-print" '(pr-str (struct person "Ada" 36)))
(throwof "sm-struct-short" '(pr-str (struct person "Ada")))
(throwcause "sm-struct-extra" '(struct person "Ada" 36 "EXTRA"))
;; ---- assoc/dissoc ----
(throwof "sm-assoc-print" '(pr-str (assoc (struct-map person :name "Ada") :age 37)))
(throwof "sm-assoc-class" '(str (class (assoc (struct-map person :name "Ada") :age 37))))
(throwof "sm-assoc-extra" '(pr-str (assoc (struct-map person :name "Ada") :zzz 1)))
(throwof "sm-dissoc-basis" '(pr-str (dissoc (struct-map person :name "Ada" :age 1) :age)))
(throwof "sm-dissoc-extra" '(pr-str (dissoc (struct-map person :name "Ada" :zzz 1) :zzz)))
;; ---- accessor ----
(throwof "sm-accessor" '(let [a (accessor person :name)] (a (struct-map person :name "Ada"))))
(throwcause "sm-accessor-miss" '(accessor person :nope))
(throwcause "sm-accessor-plain" '((accessor person :name) {:name "Bob"}))
(throwof "sm-accessor-fn?" '(fn? (accessor person :name)))
(throwof "sm-accessor-class" '(str (class (accessor person :name))))
(throwcause "sm-accessor-print" '(pr-str (accessor person :name)))
(throwcause "sm-accessor-arity0" '((accessor person :name)))
(throwcause "sm-accessor-arity2" '((accessor person :name) (struct-map person :name "Ada") (struct-map person :name "Bo")))
(throwcause "sm-accessor-nil" '((accessor person :name) nil))
(throwcause "sm-accessor-mismatch" '((accessor person :name) (struct-map other :name "X" :age 1)))
(throwof "sm-assoc-extra-class" '(str (class (assoc (struct-map person :name "Ada") :zzz 1))))
(throwof "sm-dissoc-extra-class" '(str (class (dissoc (struct-map person :name "Ada" :zzz 1) :zzz))))
(throwof "sm-empty-basis" '(pr-str (struct-map (create-struct))))
(throwcause "sm-map-nonbasis" '(struct-map {:a 0} :a 1))
(throwcause "sm-struct-nonbasis" '(struct "x" 1))
(throwcause "sm-accessor-nonbasis" '(accessor {:a 0} :a))
;; ---- struct IFn (R2: structs invoke as maps; records stay non-IFn) ----
(throwof "sm-ifn" '(ifn? (struct-map person :name "Ada" :age 36 :zzz 9)))
(throwof "sm-call" '((struct-map person :name "Ada" :age 36 :zzz 9) :age))
(throwof "sm-call-nf" '((struct-map person :name "Ada" :age 36 :zzz 9) :missing :nf))
(throwcause "sm-call-arity0" '((struct-map person :name "Ada" :age 36 :zzz 9)))
(throwcause "sm-call-arity3" '((struct-map person :name "Ada" :age 36 :zzz 9) :a :b :c))
(throwof "sm-apply" '(apply (struct-map person :name "Ada" :age 36 :zzz 9) [:age]))
(throwof "sm-mapfn" '(pr-str (map (struct-map person :name "Ada" :age 36 :zzz 9) [:age :name :zzz :nope :nf-x])))
