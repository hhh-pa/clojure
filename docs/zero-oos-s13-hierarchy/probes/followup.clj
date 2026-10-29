;; S13 oracle probes: follow-ups (oracle: alpha10 jars, issue #57).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s13-hierarchy/probes/followup.clj > docs/zero-oos-s13-hierarchy/probes/followup.out
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
(defn sup [c] (sort (map str (supers c))))
(defn bas [c] (sort (map str (bases c))))
;; ---- N: nil invocation + partial hierarchies ----
(throwcause "n-nilret-call" '((identity nil) :a))
(throwcause "n-ancnil-call" '((:ancestors {}) :a))
(throwof "n-isa-partial" '(isa? {:ancestors {::pa #{::pb}}} ::pa ::pb))
(throwcause "n-derive-partial" '(derive {:parents {}} ::da ::db))
(throwcause "n-derive-partial2" '(derive {:parents {} :ancestors {}} ::dx ::dy))
(throwof "n-derive-edgeonly" '(derive {:parents {::ea #{::eb}}} ::ea ::eb))
(throwcause "n-derive-badparents" '(derive {:parents 5} ::fa ::fb))
(throwcause "n-isa-badanc" '(isa? {:ancestors 5} ::ga ::gb))
(throwof "n-par-partial" '(parents {:parents {::qa #{::qb}}} ::qa))
(throwof "n-par-badh-class" '(sort (map str (parents 5 String))))
(throwof "n-anc-partial" '(ancestors {:ancestors {::ra #{::rb}}} ::ra))
(throwof "n-desc-partial" '(descendants {:descendants {::sb #{::sa}}} ::sb))
(throwof "n-under-partial" '(underive {:parents {::ua #{::ub}}} ::ua ::ub))
;; ---- O: isa? arm order with unusable h ----
(throwof "n-isa-eq-badh" '(isa? 5 ::ox ::ox))
(throwof "n-isa-class-badh" '(isa? 5 String Object))
(throwof "n-isa-vec-badh" '(isa? 5 [::ova] [::ova]))
(throwcause "n-isa-ne-badh" '(isa? 5 ::ox ::oy))
;; ---- P: interfaces + leftover lattice ----
(throwof "n-iface-obj" '(isa? clojure.lang.IFn Object))
(throwof "n-iface-iface" '(isa? clojure.lang.ISeq clojure.lang.Seqable))
(throwof "n-obj-iface" '(isa? Object clojure.lang.IFn))
(throwof "n-par-ifn" '(sort (map str (parents clojure.lang.IFn))))
(throwof "n-range" '(let [c (Class/forName "clojure.lang.Range")] (list (str c) (bas c) (sup c))))
(throwof "n-unbound" '(let [c (Class/forName "clojure.lang.Var$Unbound")] (list (str c) (bas c) (sup c))))
;; ---- Q: dispatch edges ----
(throwcause "n-badval-miss" '(do (defmulti hdbm2 :k :hierarchy (atom 5)) (defmethod hdbm2 :a [m] :a) (hdbm2 {:k :b})))
(throwcause "n-prefer-trans" '(do (defmulti hdpt :k) (defmethod hdpt :a [m] :a) (prefer-method hdpt :a :b) (prefer-method hdpt :b :c) (prefer-method hdpt :c :a)))
(throwcause "n-deref-nil" '(deref nil))
(throwof "n-amb-order" '(do (defmulti hdmo :k) (defmethod hdmo ::moa [m] :a) (defmethod hdmo ::mob [m] :b) (derive ::moc ::moa) (derive ::moc ::mob) [(map first (seq (methods hdmo))) (try (hdmo {:k ::moc}) (catch IllegalArgumentException e (.getMessage e)))]))
