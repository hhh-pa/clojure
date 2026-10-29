;; S18 oracle probes: compile end-to-end (oracle: alpha10 jars, issue #62).
;; Run from repo root (/home/hhu/workspace/clojure-s18):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s18-compiler/probes/compile.clj > docs/zero-oos-s18-compiler/probes/compile.out
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(def root (str (System/getProperty "java.io.tmpdir") "/s18compile"))
(def src (str root "/src"))
(def classes (str root "/classes"))
(defn normroot [s] (clojure.string/replace s root "<ROOT>"))
(defn normcount [s] (clojure.string/replace s #"__\d+" "__N"))
(defn rmrf [^java.io.File f]
  (when (.exists f)
    (when (.isDirectory f) (doseq [c (.listFiles f)] (rmrf c)))
    (.delete f)))
;; cpin: value pins path/hex/counter-normalized; throw pins carry the
;; root cause normalized the same way (stable reruns).
(defn cpin [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normroot (normhex (normcount (pr-str v)))))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (str (.getMessage ^Throwable c))
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L")
                  (clojure.string/replace #"__\d+" "__N"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str (normroot m))))))))
(defn probe-fn [] :hi)
(defn artifacts [dir]
  (let [base (java.io.File. ^String dir)]
    (sort (map (fn [^java.io.File f] (clojure.string/replace (.getPath f) (str (.getPath base) "/") ""))
               (filter (fn [^java.io.File f] (.isFile f)) (file-seq base))))))
(defn wipe-classes! [] (doseq [c (.listFiles (java.io.File. classes))] (rmrf c)))
(rmrf (java.io.File. root))
(doseq [d ["/src/my" "/classes"]] (.mkdirs (java.io.File. (str root d))))
(spit (str src "/my/ret.clj") "(ns my.ret)\n(defn answer [] 41)\n(def marker \"ret\")\n")
(spit (str src "/my/twice.clj") "(ns my.twice)\n(swap! user/probe-counter inc)\n(def marker \"twice\")\n")
(spit (str src "/my/plain.clj") "(ns my.plain)\n(defn f [x] (+ x 1))\n(def marker \"plain\")\n")
(spit (str src "/my/plain2.clj") "(ns my.plain2)\n(defn f [x] (+ x 1))\n(def marker \"plain2\")\n")
(spit (str src "/my/ctx.clj") "(ns my.ctx)\n(def captured {:compile-files *compile-files* :compile-path *compile-path* :source-path *source-path* :fn-loader (if (nil? *fn-loader*) \"nil\" (.getName (class *fn-loader*))) :use-context *use-context-classloader* :verbose *verbose-defrecords* :options *compiler-options* :file *file*})\n")
(spit (str src "/my/broken.clj") "(ns my.broken)\n(defn f [x] (no-such-fn-xyz x))\n")
(spit (str src "/my/mismatch.clj") "(ns other.thing)\n(def marker \"mismatch\")\n")
(spit (str src "/my/gc.clj") "(ns my.gc (:gen-class :name my.gc :methods [[greet [] String]]))\n(defn -greet [this] \"hi\")\n")
(spit (str src "/my/use_t.clj") "(ns my.use-t)\n(def marker \"t\")\n")
(spit (str src "/my/use_f.clj") "(ns my.use-f)\n(def marker \"f\")\n")
(spit (str src "/my/record.clj") "(ns my.record)\n(defrecord R [x])\n(def marker \"record\")\n")
;; Fixture scaffolding (NOT spec behavior): RT.load resolves through
;; the stable parent loader (L0), which add-classpath cannot target
;; under clojure.main (see classpath.clj section A), so the fixture
;; dir enters L0 via a direct .addURL. Silent.
(.addURL ^clojure.lang.DynamicClassLoader (.getParent (.getClassLoader (class probe-fn))) (.. (java.io.File. src) toURI toURL))
(def probe-counter (atom 0))
;; ---- A: return value + artifacts + load effects ----
(cpin "c-ret" '(binding [*compile-path* classes] (compile 'my.ret)))
(cpin "c-ret-class" '(binding [*compile-path* classes] (.getName (class (compile 'my.ret)))))
(wipe-classes!)
(cpin "c-files" '(do (binding [*compile-path* classes] (compile 'my.plain)) (artifacts classes)))
(cpin "c-ns-loaded" '(boolean (find-ns 'my.plain)))
(cpin "c-loaded-libs" '(contains? (loaded-libs) 'my.plain))
;; ---- B: *compile-files* gates emission, never evaluation ----
(wipe-classes!)
(cpin "c-false-files" '(do (binding [*compile-files* false *compile-path* classes] (load "/my/plain")) (artifacts classes)))
(cpin "c-false-eval" '(do (binding [*compile-files* false *compile-path* classes] (load "/my/plain")) @(resolve 'my.plain/marker)))
(wipe-classes!)
(cpin "c-true-load" '(do (binding [*compile-files* true *compile-path* classes] (load "/my/plain")) (artifacts classes)))
(cpin "c-flag-truthy" '(do (wipe-classes!) (binding [*compile-files* 1 *compile-path* classes] (load "/my/plain")) (artifacts classes)))
;; compile always reloads: no loaded-libs shortcut (counter hits 2).
(cpin "c-twice" '(do (binding [*compile-path* classes] (compile 'my.twice)) (binding [*compile-path* classes] (compile 'my.twice)) @user/probe-counter))
;; ---- C: what the knobs observe during a compile ----
(cpin "c-captured" '(do (binding [*compile-path* classes] (compile 'my.ctx)) (into (sorted-map) @(resolve 'my.ctx/captured))))
(cpin "c-use-true" '(binding [*use-context-classloader* true *compile-path* classes] (compile 'my.use-t)))
(cpin "c-use-false" '(binding [*use-context-classloader* false *compile-path* classes] (compile 'my.use-f)))
;; ---- D: failures ----
(cpin "c-missing" '(binding [*compile-path* classes] (compile 'no.such.lib)))
(cpin "c-arity0" '(compile))
(cpin "c-arity2" '(compile 'a 'b))
(cpin "c-str" '(binding [*compile-path* classes] (compile "my/plain")))
(cpin "c-num" '(binding [*compile-path* classes] (compile 42)))
(cpin "c-broken" '(binding [*compile-path* classes] (compile 'my.broken)))
(cpin "c-broken-unmarked" '(contains? (loaded-libs) 'my.broken))
(cpin "c-mismatch" '(binding [*compile-path* classes] (compile 'my.mismatch)))
(cpin "c-nil-path" '(binding [*compile-path* nil] (compile 'my.plain)))
(cpin "c-bad-path" '(binding [*compile-path* 42] (compile 'my.plain)))
(cpin "c-keyword" '(binding [*compile-path* classes] (compile :my.plain)))
(cpin "c-keyword-ns" '(binding [*compile-path* classes] (compile :my/plain)))
(cpin "c-mismatch-unmarked" '(contains? (loaded-libs) 'my.mismatch))
(cpin "c-mismatch-other" '(contains? (loaded-libs) 'other.thing))
(cpin "c-nil-lib" '(binding [*compile-path* classes] (compile nil)))
(cpin "c-no-mkdirs" '(binding [*compile-path* (str root "/nested/deep")] (compile 'my.plain2)))
(cpin "c-broken-files" '(do (wipe-classes!) [(try (binding [*compile-path* classes] (compile 'my.broken)) (catch Throwable _ :threw)) (artifacts classes)]))
(cpin "c-mismatch-files" '(do (wipe-classes!) [(try (binding [*compile-path* classes] (compile 'my.mismatch)) (catch Throwable _ :threw)) (artifacts classes)]))
(cpin "c-options-bound" '(binding [*compiler-options* {:elide-meta [:x] :direct-linking true} *compile-path* classes] (compile 'my.use-t)))
;; ---- E: *source-path*/*file*/*fn-loader* across load entry points ----
(cpin "c-load-captured" '(do (binding [*compile-files* false *compile-path* classes] (load "/my/ctx")) (into (sorted-map) @(resolve 'my.ctx/captured))))
(cpin "c-loadfile-source" '(do (load-file (str src "/my/ctx.clj")) (select-keys @(resolve 'my.ctx/captured) [:source-path :file :fn-loader])))
(cpin "c-loadstring-source" '(do (load-string "(ns my.ls) (def lscap {:source-path *source-path* :file *file* :fn-loader *fn-loader*})") @(resolve 'my.ls/lscap)))
;; ---- F: gen-class round-trip ----
(wipe-classes!)
(cpin "c-gc-files" '(do (binding [*compile-path* classes] (compile 'my.gc)) (artifacts classes)))
(cpin "c-gc-load" '(let [u (.. (java.io.File. classes) toURI toURL) cl (java.net.URLClassLoader. (into-array [u]))] (.getName (.loadClass cl "my.gc"))))
(cpin "c-gc-new" '(let [u (.. (java.io.File. classes) toURI toURL) cl (java.net.URLClassLoader. (into-array [u]))] (str (.newInstance (.loadClass cl "my.gc")))))
(cpin "c-gc-fn" '(my.gc/-greet nil))
;; ---- G: defrecord compiles (subset-gap witness: the port plans this
;; lib's shape as a diagnostic, never an artifact) ----
(wipe-classes!)
(cpin "c-record-ok" '(binding [*compile-path* classes] (compile 'my.record)))
(cpin "c-record-files" '(artifacts classes))
(rmrf (java.io.File. root))
