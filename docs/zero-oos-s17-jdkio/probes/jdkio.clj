;; S17 JDK/IO oracle probes: enumeration-seq / iterator-seq / stream-* /
;; resultset-seq / file-seq / xml-seq (oracle: alpha10 jars, issue #61).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s17-jdkio/probes/jdkio.clj > docs/zero-oos-s17-jdkio/probes/jdkio.out
(require 'clojure.string)
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
;; ---- fixtures ----
(def enum3 (java.util.Collections/enumeration [1 2 3]))
(def enum0 (java.util.Collections/enumeration []))
(def iter3 (.iterator ^java.util.List [1 2 3]))
(def iter0 (.iterator ^java.util.List []))
(defn fresh-stream [xs] (.stream ^java.util.List (vec xs)))
;; A fake ResultSet over proxy (labels + rows close over atoms).
(defn fake-rs [labels rows]
  (let [pos (atom -1)
        meta (proxy [java.sql.ResultSetMetaData] []
               (getColumnCount [] (count labels))
               (getColumnLabel [^Integer i] (nth labels (dec i))))]
    (proxy [java.sql.ResultSet] []
      (getMetaData [] meta)
      (^Boolean next [] (swap! pos inc) (< @pos (count rows)))
      (getObject [^Integer i] (nth (nth rows @pos) (dec i))))))
(def rs2 (fake-rs ["NAME" "AGE"] [["a" 1] ["b" 2]]))
(def rs-empty (fake-rs ["A"] []))
(def rs-dup (fake-rs ["A" "a"] [[1 2]]))
;; A fixture tree: root/{mid/deep.txt, top.txt}.
(def tmp-root
  (let [base (java.io.File/createTempFile "s17probe" "root")]
    (.delete base) (.mkdir base)
    (let [mid (java.io.File. base "mid")]
      (.mkdir mid)
      (spit (java.io.File. mid "deep.txt") "deep")
      (spit (java.io.File. base "top.txt") "top"))
    (.getPath base)))
(def xml-tree {:tag :a :content [{:tag :b :content ["hi" {:tag :c :content []}]} "tail"]})
;; ---- A: seq adapters ----
(show "a-enum-type" '(str (type (enumeration-seq enum3))))
(show "a-enum-vals" '(vec (enumeration-seq (java.util.Collections/enumeration [1 2 3]))))
(show "a-enum-empty-vec" '(vec (enumeration-seq enum0)))
(show "a-enum-empty-nil" '(nil? (enumeration-seq enum0)))
(show "a-enum-chunked" '(chunked-seq? (enumeration-seq (java.util.Collections/enumeration [1]))))
(throwcause "a-enum-lazy" '(realized? (enumeration-seq (java.util.Collections/enumeration [1]))))
(show "a-iter-type" '(str (type (iterator-seq (.iterator ^java.util.List [1 2 3])))))
(show "a-iter-vals" '(vec (iterator-seq (.iterator ^java.util.List [1 2 3]))))
(show "a-iter-empty-vec" '(vec (iterator-seq iter0)))
(show "a-iter-empty-nil" '(nil? (iterator-seq iter0)))
(show "a-iter-chunked" '(chunked-seq? (iterator-seq (.iterator ^java.util.List [1]))))
(throwcause "a-iter-lazy" '(realized? (iterator-seq (.iterator ^java.util.List [1]))))
(show "a-enum-nil" '(try (enumeration-seq nil) (catch Throwable t (str "THROW " (.getName (class t))))))
(show "a-iter-nil" '(try (iterator-seq nil) (catch Throwable t (str "THROW " (.getName (class t))))))
;; ---- B: streams ----
(show "b-seq-type" '(str (type (stream-seq! (fresh-stream [1 2 3])))))
(show "b-seq-vals" '(vec (stream-seq! (fresh-stream [1 2 3]))))
(show "b-seq-empty-vec" '(vec (stream-seq! (fresh-stream []))))
(show "b-seq-empty-nil" '(nil? (stream-seq! (fresh-stream []))))
(show "b-seq-chunked" '(chunked-seq? (stream-seq! (fresh-stream [1]))))
(throwcause "b-seq-lazy" '(realized? (stream-seq! (fresh-stream [1]))))
(show "b-stream-class" '(str (type (fresh-stream [1]))))
(show "b-reduce" '(stream-reduce! + (fresh-stream [1 2 3])))
(show "b-reduce-init" '(stream-reduce! + 10 (fresh-stream [1 2 3])))
(show "b-reduce-empty" '(stream-reduce! + (fresh-stream [])))
(show "b-reduce-empty-init" '(stream-reduce! + 10 (fresh-stream [])))
(show "b-reduce-reduced" '(stream-reduce! (fn [a x] (if (= x 2) (reduced :stop) (+ a x))) 0 (fresh-stream [1 2 3])))
(show "b-transduce" '(stream-transduce! (map inc) + 0 (fresh-stream [1 2 3])))
(show "b-transduce-init" '(stream-transduce! (map inc) + (fresh-stream [1 2 3])))
(show "b-transduce-complete" '(stream-transduce! (map inc) (completing + str) 0 (fresh-stream [1 2 3])))
(show "b-into-vec" '(stream-into! [] (fresh-stream [1 2 3])))
(show "b-into-set" '(stream-into! #{} (fresh-stream [1 2 3])))
(show "b-into-map" '(stream-into! {} (fresh-stream [[:a 1] [:b 2]])))
(show "b-into-list" '(stream-into! () (fresh-stream [1 2 3])))
(show "b-into-xf" '(stream-into! [] (map inc) (fresh-stream [1 2 3])))
(show "b-into-xf-set" '(stream-into! #{} (map inc) (fresh-stream [1 2 3])))
(throwcause "b-reuse" '(let [s (fresh-stream [1])] (stream-reduce! + s) (stream-seq! s)))
(throwcause "b-reuse2" '(let [s (fresh-stream [1])] (stream-seq! s) (stream-reduce! + s)))
(show "b-seq-nil" '(try (stream-seq! nil) (catch Throwable t (str "THROW " (.getName (class t))))))
;; ---- C: resultset-seq ----
(show "c-rows" '(vec (resultset-seq (fake-rs ["NAME" "AGE"] [["a" 1] ["b" 2]]))))
(show "c-row-type" '(str (type (first (resultset-seq rs2)))))
(show "c-row-keys" '(vec (keys (first (resultset-seq (fake-rs ["NAME" "AGE"] [["a" 1]]))))))
(show "c-empty-vec" '(vec (resultset-seq rs-empty)))
(show "c-empty-nil" '(nil? (resultset-seq (fake-rs ["A"] []))))
(show "c-chunked" '(chunked-seq? (resultset-seq (fake-rs ["A"] [[1]]))))
(throwcause "c-lazy" '(realized? (resultset-seq (fake-rs ["A"] [[1]]))))
(throwcause "c-dupkeys" '(resultset-seq rs-dup))
(show "c-row-second" '(vec (map (juxt :name :age) (resultset-seq (fake-rs ["NAME" "AGE"] [["a" 1] ["b" 2]])))))
(show "c-nil" '(try (resultset-seq nil) (catch Throwable t (str "THROW " (.getName (class t))))))
;; ---- D: file-seq ----
(show "d-root" '(.getPath ^java.io.File (first (file-seq (java.io.File. tmp-root)))))
(show "d-names" '(sort (map #(.getName ^java.io.File %) (file-seq (java.io.File. tmp-root)))))
(show "d-count" '(count (file-seq (java.io.File. tmp-root))))
(show "d-preorder" '(let [paths (vec (map #(.getPath ^java.io.File %) (file-seq (java.io.File. tmp-root))))
                         idx (zipmap paths (range))]
                     (every? true? (for [p paths
                                          :let [f (java.io.File. p)]
                                          :when (not= p tmp-root)]
                                      (< (idx (.getPath (.getParentFile f))) (idx p))))))
(show "d-file-arg" '(vec (map #(.getName ^java.io.File %) (file-seq (java.io.File. tmp-root "top.txt")))))
(show "d-missing" '(vec (map #(.getName ^java.io.File %) (file-seq (java.io.File. "/nonexistent-s17-xyz")))))
(show "d-chunked" '(chunked-seq? (file-seq (java.io.File. tmp-root))))
(throwcause "d-lazy" '(realized? (file-seq (java.io.File. tmp-root))))
(show "d-nil" '(try (doall (file-seq nil)) (catch Throwable t (str "THROW " (.getName (class t))))))
(show "d-file-class" '(str (type (java.io.File. "x"))))
(show "d-file-str" '(str (java.io.File. "some/path.txt")))
(show "d-file-print" '(pr-str (java.io.File. "some/path.txt")))
;; ---- E: xml-seq ----
(show "e-walk" '(vec (map :tag (xml-seq xml-tree))))
(show "e-count" '(count (xml-seq xml-tree)))
(show "e-ident" '(let [xs (xml-seq xml-tree)] [(identical? (first xs) xml-tree) (identical? (second xs) {:tag :b :content ["hi" {:tag :c :content []}]})]))
(show "e-string-root" '(vec (xml-seq "just a string")))
(show "e-no-content" '(vec (xml-seq {:tag :x})))
(show "e-empty-content" '(vec (xml-seq {:tag :x :content []})))
(show "e-nil" '(vec (xml-seq nil)))
(show "e-chunked" '(chunked-seq? (xml-seq xml-tree)))
(throwcause "e-lazy" '(realized? (xml-seq xml-tree)))
(show "e-deep" '(vec (map :tag (xml-seq {:tag :r :content [{:tag :a :content [{:tag :b :content ["x"]}]} {:tag :c :content nil}]}))))
;; ---- F: handle print shapes ----
(show "f-enum-print" '(pr-str (java.util.Collections/enumeration [1])))
(show "f-iter-print" '(pr-str (.iterator ^java.util.List [1])))
(show "f-stream-print" '(pr-str (fresh-stream [1])))
(show "f-rs-print" '(pr-str (fake-rs ["A"] [[1]])))
;; ---- G: error shapes ----
(throwcause "g-enum-argerr" '(enumeration-seq 1))
(throwcause "g-iter-argerr" '(iterator-seq 1))
(throwcause "g-stream-argerr" '(stream-seq! [1 2]))
(throwcause "g-reduce-argerr" '(stream-reduce! + [1 2]))
(throwcause "g-rs-argerr" '(resultset-seq 1))
(throwcause "g-file-argerr" '(doall (file-seq "foo")))
(throwcause "g-enum-arity0" '(enumeration-seq))
(throwcause "g-enum-arity2" '(enumeration-seq enum3 enum3))
(throwcause "g-iter-arity0" '(iterator-seq))
(throwcause "g-reduce-arity1" '(stream-reduce! +))
(throwcause "g-seq-arity0" '(stream-seq!))
(throwcause "g-transduce-arity2" '(stream-transduce! (map inc) +))
(throwcause "g-into-arity1" '(stream-into! []))
(throwcause "g-rs-arity0" '(resultset-seq))
(throwcause "g-file-arity0" '(file-seq))
(throwcause "g-xml-arity0" '(xml-seq))
(throwcause "g-xml-arity2" '(xml-seq xml-tree xml-tree))
;; ---- N: nil messages, lazy file errors, call timing ----
(throwcause "n-enum-nil" '(enumeration-seq nil))
(throwcause "n-iter-nil" '(iterator-seq nil))
(throwcause "n-seq-nil" '(stream-seq! nil))
(throwcause "n-rs-nil" '(resultset-seq nil))
(throwcause "n-file-nil" '(doall (file-seq nil)))
(throwcause "n-reduce-nil" '(stream-reduce! + nil))
(throwcause "n-into-nil" '(stream-into! [] nil))
(throwcause "n-transduce-nil" '(stream-transduce! (map inc) + 0 nil))
(show "n-file-unforced" '(realized? (file-seq "foo")))
(show "n-file-str-unforced" '(do (file-seq "foo") :ok))
(show "n-file-nil-unforced" '(realized? (file-seq nil)))
(show "n-file-vec-force" '(try (vec (file-seq "foo")) (catch ClassCastException t (str "THROW " (.getMessage t)))))
(show "f-enum-str" '(str (java.util.Collections/enumeration [1])))
(show "f-iter-str" '(str (.iterator ^java.util.List [1])))
(show "f-stream-str" '(str (fresh-stream [1])))
(show "n-reduce-nonfn-single" '(stream-reduce! 1 (fresh-stream [7])))
(throwcause "n-reduce-nonfn-empty" '(stream-reduce! 1 (fresh-stream [])))
(throwcause "n-transduce-xnonfn" '(stream-transduce! 5 + 0 (fresh-stream [1])))
(throwcause "n-transduce-fnonfn" '(stream-transduce! 5 6 (fresh-stream [1])))
(throwcause "n-into-xnonfn" '(stream-into! [] 5 (fresh-stream [1])))
(throwcause "o-transduce-init-first" '(stream-transduce! "x" 6 (fresh-stream [1])))
(throwcause "o-into-xf-first" '(stream-into! [] 5 nil))
(throwcause "o-reduce-stream-first" '(stream-reduce! 1 nil))
;; ---- cleanup ----
(doseq [f (reverse (file-seq (java.io.File. tmp-root)))] (.delete ^java.io.File f))
(println "done true")
