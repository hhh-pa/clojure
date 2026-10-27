;; Zero-oos S8 (issue #51) oracle probes: print-method/print-dup dispatch order (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/dispatch.clj > docs/zero-oos-s8-print/probes/dispatch.out
;; NOTE: no DONE marker — the last row redefines print-method String, after
;; which no string prints readably anymore.
(require 'clojure.string)
(defn norm [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")
      (clojure.string/replace #"eval\d+" "evalN")
      (clojure.string/replace #"--\d+" "--N")
      (clojure.string/replace #"__\d+" "__N")))
(defn mof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (norm (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (norm (str (.getMessage ^Throwable c)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- mult shape + default tables (FIRST: later rows install methods) ----
(mof "m-shape" '[(.getName ^Class (class print-method)) (.getName ^Class (class print-dup))])
(mof "m-has-default" '[(contains? (methods print-method) :default) (contains? (methods print-dup) :default)])
(mof "m-table-method" '(sort (map #(if (instance? Class %) (.getName ^Class %) (str %)) (keys (methods print-method)))))
(mof "m-table-dup" '(sort (map #(if (instance? Class %) (.getName ^Class %) (str %)) (keys (methods print-dup)))))
(mof "m-getmethod" '[(fn? (get-method print-method String)) (nil? (get-method print-dup ::unknown))])
;; ---- user fixtures (stable class names) ----
(deftype DT [x])
(defmethod print-method DT [o w] (.write ^java.io.Writer w (str "DT" (.x ^DT o))))
(deftype DW [x])
(defmethod print-method DW [o w] (.write ^java.io.Writer w (if (identical? w *out*) "SAME" "DIFF")))
(defmethod print-method ::ntype [o w] (.write ^java.io.Writer w "NTYPE"))
(defmethod print-method ::xatom [o w] (.write ^java.io.Writer w "X!"))
(deftype DD [x])
(defmethod print-dup DD [o w] (.write ^java.io.Writer w "DUP!"))
;; ---- user print-method beats the default, recursively, with separators ----
(mof "m-user" '(pr-str (DT. 1)))
(mof "m-user-nested" '(pr-str [(DT. 1) 2]))
(mof "m-user-spaces" '(with-out-str (pr (DT. 1) (DT. 2))))
(mof "m-user-newline" '(with-out-str (prn (DT. 1))))
(mof "m-writer-id" '(pr-str (DW. 1)))
;; ---- str/toString NEVER consults print-method ----
(mof "m-str-unaffected" '(str (DT. 1)))
;; ---- meta :type keyword dispatch (+ :default chaining, never a throw) ----
(mof "m-type-kw" '(pr-str (with-meta [1] {:type ::ntype})))
(mof "m-type-nonstr" '(pr-str (with-meta [1] {:type "nope"})))
(mof "m-type-unknown-kw" '(pr-str (with-meta [1] {:type ::unknown})))
(mof "m-type-atom-ctor" '(pr-str (atom 1 :meta {:type ::unknown})))
(mof "m-dup-delegates" '(binding [*print-dup* true] (pr-str (with-meta (into {} (map (fn [i] [i i]) (range 20))) {:type ::xatom}))))
(mof "m-dup-nodelegates" '(binding [*print-dup* true] (pr-str (with-meta {:a 1} {:type ::xatom}))))
;; ---- remove-method restores the default ----
(mof "m-remove" '(do (remove-method print-method DT) (pr-str (DT. 1))))
;; ---- user print-dup is gated by *print-dup*; removal restores the throw ----
(mof "m-dup-user" '[(binding [*print-dup* true] (pr-str (DD. 1))) (binding [*print-dup* false] (pr-str (DD. 1)))])
(mof "m-dup-remove" '(do (remove-method print-dup DD) (binding [*print-dup* true] (pr-str (DD. 1)))))
;; ---- the :default strip, made visible with *print-meta* ----
(mof "m-type-strip-visible" '(binding [*print-meta* true] (pr-str (with-meta [1] {:type ::unknown}))))
(mof "m-type-nonstr-meta" '(binding [*print-meta* true] (pr-str (with-meta [1] {:type "nope"}))))
;; ---- interface-level user methods (informational: needs hierarchies, so the
;; port — exact-plus-default dispatch per #37 — does NOT replicate this) ----
(mof "m-iface" '(let [orig (get-method print-method Number)] (defmethod print-method Number [o w] (.write ^java.io.Writer w "NUM")) (let [r (pr-str 5)] (. ^clojure.lang.MultiFn print-method addMethod Number orig) [r (pr-str 5)])))
;; ---- LAST: redefining a core default beats it; str still unaffected.
;; Reported with counts + equality only: after the override no string prints
;; readably anymore (and removing String's method would stack-overflow the JVM
;; itself — hierarchy falls back to Object whose rep is a String — so the
;; method stays installed and this row reports via .write, last in the file).
(do (defmethod print-method String [s w] (.write ^java.io.Writer w "STR!"))
    (let [r [(pr-str "hi") (str "hi")]]
      (.write ^java.io.Writer *out* (str "m-str-override " (pr-str [(mapv count r) (= r ["STR!" "hi"])]) "\n"))))
