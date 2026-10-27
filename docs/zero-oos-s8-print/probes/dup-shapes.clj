;; Zero-oos S8 (issue #51) oracle probes: per-kind *print-dup* shapes (oracle: alpha10 jars).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s8-print/probes/dup-shapes.clj > docs/zero-oos-s8-print/probes/dup-shapes.out
(require 'clojure.string)
(defn norm [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")
      (clojure.string/replace #"eval\d+" "evalN")
      (clojure.string/replace #"--\d+" "--N")
      (clojure.string/replace #"__\d+" "__N")))
(defn dupof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (norm (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (norm (str (.getMessage ^Throwable c)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- top-level fixtures (stable class names) ----
(def dupshape-x 1)
(defn dupshape-f [x] x)
(defrecord RP [x y])
(deftype DT [x])
;; ---- scalars (print-dup = print-method) ----
(dupof "d-nil" '(binding [*print-dup* true] (pr-str nil)))
(dupof "d-bool" '(binding [*print-dup* true] (pr-str true)))
(dupof "d-long" '(binding [*print-dup* true] (pr-str 5)))
(dupof "d-double" '(binding [*print-dup* true] (pr-str 1.5)))
(dupof "d-nan" '(binding [*print-dup* true] (pr-str ##NaN)))
(dupof "d-bigint" '(binding [*print-dup* true] (pr-str 5N)))
(dupof "d-bigdec" '(binding [*print-dup* true] (pr-str 5.0M)))
(dupof "d-ratio" '(binding [*print-dup* true] (pr-str 1/2)))
(dupof "d-string" '(binding [*print-dup* true] (pr-str "hi")))
(dupof "d-char" '(binding [*print-dup* true] (pr-str \a)))
(dupof "d-char-named" '(binding [*print-dup* true] (pr-str \newline)))
(dupof "d-keyword" '(binding [*print-dup* true] (pr-str :u/id)))
(dupof "d-symbol" '(binding [*print-dup* true] (pr-str 'a/b)))
(dupof "d-regex" '(binding [*print-dup* true] (pr-str #"a+")))
(dupof "d-tagged" '(binding [*print-dup* true] (pr-str (tagged-literal 'x 1))))
;; ---- Number ctor forms (non-Long/Double numbers) ----
(dupof "d-int" '(binding [*print-dup* true] (pr-str (int 5))))
(dupof "d-short" '(binding [*print-dup* true] (pr-str (short 3))))
(dupof "d-byte" '(binding [*print-dup* true] (pr-str (byte 2))))
(dupof "d-float" '(binding [*print-dup* true] (pr-str (float 1.5))))
(dupof "d-biginteger" '(binding [*print-dup* true] (pr-str (BigInteger. "5"))))
;; ---- seqs/collections (print-dup = print-method via ISeq / concrete class) ----
(dupof "d-list" '(binding [*print-dup* true] (pr-str (list 1 2))))
(dupof "d-vec" '(binding [*print-dup* true] (pr-str [1 2])))
(dupof "d-lazy" '(binding [*print-dup* true] (pr-str (map inc [1 2]))))
(dupof "d-range" '(binding [*print-dup* true] (pr-str (range 3))))
(dupof "d-repeat" '(binding [*print-dup* true] (pr-str (take 2 (repeat 7)))))
(dupof "d-cons" '(binding [*print-dup* true] (pr-str (cons 1 [2]))))
(dupof "d-stringseq" '(binding [*print-dup* true] (pr-str (seq "ab"))))
(dupof "d-keyseq" '(binding [*print-dup* true] (pr-str (keys {:a 1}))))
(dupof "d-valseq" '(binding [*print-dup* true] (pr-str (vals {:a 1}))))
(dupof "d-chunked" '(binding [*print-dup* true] (pr-str (seq [1 2]))))
(dupof "d-arrayseq" '(binding [*print-dup* true] (pr-str (seq (into-array [1 2])))))
(dupof "d-queue" '(binding [*print-dup* true] (pr-str (conj clojure.lang.PersistentQueue/EMPTY 1 2))))
(dupof "d-hashset" '(binding [*print-dup* true] (pr-str #{1 2})))
;; ---- collection ctor forms (classes WITHOUT a print-dup = print-method entry) ----
(dupof "d-arraymap" '(binding [*print-dup* true] (pr-str {:a 1})))
(dupof "d-sortedmap" '(binding [*print-dup* true] (pr-str (sorted-map :b 2 :a 1))))
(dupof "d-sortedset" '(binding [*print-dup* true] (pr-str (sorted-set 2 1))))
(dupof "d-mapentry" '(binding [*print-dup* true] (pr-str (first {:a 1}))))
(dupof "d-subvec" '(binding [*print-dup* true] (pr-str (subvec [1 2 3] 1))))
(dupof "d-eduction" '(binding [*print-dup* true] (pr-str (eduction (map inc) [1 2]))))
;; PersistentHashMap (large map): order-unstable, so pin shape facts, not literal text.
(println (str "d-hashmap-shape "
              (norm (pr-str (binding [*print-dup* true]
                              (let [m (into {} (map (fn [i] [(str "k" i) i]) (range 20)))
                                    s (pr-str m)]
                                [(.startsWith ^String s "{")
                                 (not (.startsWith ^String s "#=("))
                                 (every? #(.contains ^String s %) ["\"k0\" 0" "\"k19\" 19"])]))))))
;; ---- vars, namespaces, fns ----
(dupof "d-var-user" '(binding [*print-dup* true] (pr-str #'dupshape-x)))
(dupof "d-var-core" '(binding [*print-dup* true] (pr-str #'inc)))
(dupof "d-ns" '(binding [*print-dup* true] (pr-str (find-ns 'user))))
(dupof "d-anonfn" '(binding [*print-dup* true] (pr-str (fn [x] x))))
(dupof "d-namedfn" '(binding [*print-dup* true] (pr-str @#'dupshape-f)))
;; ---- records, classes ----
(dupof "d-record" '(binding [*print-dup* true] (pr-str (->RP 1 2))))
(dupof "d-record-str" '(binding [*print-dup* true] (pr-str (->RP 1 "a"))))
(dupof "d-record-ext" '(binding [*print-dup* true] (pr-str (assoc (->RP 1 2) :z 9))))
(dupof "d-record-ext2" '(binding [*print-dup* true] (pr-str (assoc (->RP 1 2) :z 8 :w 9))))
(dupof "d-class" '(binding [*print-dup* true] (pr-str String)))
(dupof "d-class-iface" '(binding [*print-dup* true] (pr-str java.util.Map)))
(dupof "d-class-prim" '(binding [*print-dup* true] (pr-str Integer/TYPE)))
(dupof "d-class-array" '(binding [*print-dup* true] (pr-str (class (make-array String 0)))))
;; ---- no print-dup method: THROW IllegalArgumentException (message shape load-bearing) ----
(dupof "d-object" '(binding [*print-dup* true] (pr-str (Object.))))
(dupof "d-deftype" '(binding [*print-dup* true] (pr-str (DT. 1))))
(dupof "d-reify" '(binding [*print-dup* true] (pr-str (reify Object (toString [this] "R")))))
(dupof "d-atom" '(binding [*print-dup* true] (pr-str (atom 1))))
(dupof "d-delay" '(binding [*print-dup* true] (pr-str (delay 1))))
(dupof "d-future" '(binding [*print-dup* true] (pr-str (future 1))))
(dupof "d-promise" '(binding [*print-dup* true] (pr-str (promise))))
(dupof "d-agent" '(binding [*print-dup* true] (pr-str (agent 1))))
(dupof "d-volatile" '(binding [*print-dup* true] (pr-str (volatile! 1))))
(dupof "d-reduced" '(binding [*print-dup* true] (pr-str (reduced 1))))
(dupof "d-transient" '(binding [*print-dup* true] (pr-str (transient [1]))))
(dupof "d-throwable" '(binding [*print-dup* true] (pr-str (ex-info "x" {}))))
(dupof "d-readercond" '(binding [*print-dup* true] (pr-str (read-string {:read-cond :preserve} "#?@(:x)"))))
(dupof "d-multifn" '(binding [*print-dup* true] (pr-str print-method)))
(dupof "d-writer" '(binding [*print-dup* true] (pr-str (java.io.StringWriter.))))
(dupof "d-reader" '(binding [*print-dup* true] (pr-str (java.io.PushbackReader. (java.io.StringReader. "x")))))
(println "S8-dup-shapes DONE")
