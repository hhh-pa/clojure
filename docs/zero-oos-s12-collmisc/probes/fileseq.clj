;; S12 oracle probes: file-seq (oracle: alpha10 jars, issue #54).
;; Run from repo root (/home/hhu/workspace/clojure-s12):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s12-collmisc/probes/fileseq.clj > docs/zero-oos-s12-collmisc/probes/fileseq.out
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(def root (str (System/getProperty "java.io.tmpdir") "/s12-fileseq"))
(defn normroot [s] (clojure.string/replace s root "<ROOT>"))
(defn rmrf [^java.io.File f]
  (when (.exists f)
    (when (.isDirectory f) (doseq [c (.listFiles f)] (rmrf c)))
    (.delete f)))
;; fspin: value pins normalized for machine paths + identity hex;
;; throw pins carry the root cause with the fixture path normalized.
(defn fspin [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normroot (normhex (pr-str v))))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str (normroot (normhex (str (.getMessage ^Throwable c)))))))))))
(rmrf (java.io.File. root))
(doseq [d ["" "/sub" "/sub/deep" "/empty"]]
  (.mkdirs (java.io.File. (str root d))))
(spit (str root "/a.txt") "a")
(spit (str root "/sub/b.txt") "b")
(spit (str root "/sub/deep/c.txt") "c")
(fspin "fs-classes" '(pr-str (distinct (map (fn [f] (.getName (class f))) (file-seq (java.io.File. root))))))
(fspin "fs-one" '(pr-str (first (file-seq (java.io.File. root)))))
(fspin "fs-count" '(count (file-seq (java.io.File. root))))
(fspin "fs-names" '(pr-str (sort (map (fn [^java.io.File f] (.getName f)) (file-seq (java.io.File. root))))))
(fspin "fs-root-first" '(= (first (file-seq (java.io.File. root))) (java.io.File. root)))
(fspin "fs-cq" '(chunked-seq? (file-seq (java.io.File. root))))
(fspin "fs-str-arg" '(file-seq root))
(fspin "fs-missing" '(count (file-seq (java.io.File. (str root "-nope")))))
(fspin "fs-file-root" '(count (file-seq (java.io.File. (str root "/a.txt")))))
(fspin "fs-empty-dir" '(count (file-seq (java.io.File. (str root "/empty")))))
(fspin "fs-seq?" '(seq? (file-seq (java.io.File. root))))
(rmrf (java.io.File. root))
