;; S18 oracle probes: add-classpath (oracle: alpha10 jars, issue #62).
;; Run from repo root (/home/hhu/workspace/clojure-s18):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s18-compiler/probes/classpath.clj > docs/zero-oos-s18-compiler/probes/classpath.out
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
(def root (str (System/getProperty "java.io.tmpdir") "/s18cp"))
(defn normroot [s] (clojure.string/replace s root "<ROOT>"))
(defn rmrf [^java.io.File f]
  (when (.exists f)
    (when (.isDirectory f) (doseq [c (.listFiles f)] (rmrf c)))
    (.delete f)))
;; apin: value pins path/hex-normalized; throw pins carry the root
;; cause normalized the same way (stable reruns).
(defn apin [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normroot (normhex (pr-str v))))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (str (.getMessage ^Throwable c))
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str (normroot m))))))))
(defn probe-fn [] :hi)
(rmrf (java.io.File. root))
(doseq [d ["/d1/my" "/d2/my" "/d2/new" "/d2/use_t" "/d2/use_f" "/ctxonly"]]
  (.mkdirs (java.io.File. (str root d))))
(spit (str root "/d1/my/lib.clj") "(ns my.lib)\n(def marker \"d1\")\n")
(spit (str root "/d2/my/lib.clj") "(ns my.lib)\n(def marker \"d2\")\n")
(spit (str root "/d2/new/lib.clj") "(ns new.lib)\n(def marker \"new\")\n")
(spit (str root "/d2/use_t/lib.clj") "(ns use-t.lib)\n(def marker \"t\")\n")
(spit (str root "/d2/use_f/lib.clj") "(ns use-f.lib)\n(def marker \"f\")\n")
(spit (str root "/ctxonly/ctx-marker.txt") "ctx-marker\n")
;; ---- A: the fn as-is under clojure.main (context loader is the app
;; loader, never Dynamic): the WARNING prints, then the loader check
;; throws. First call unwrapped: pins the bare WARNING on stdout.
(try (add-classpath "file:/s18-nope/") (catch Throwable _ nil))
(apin "a-broken-url" '(add-classpath "file:/s18-nope/"))
(apin "a-broken-bare" '(add-classpath "/tmp/s18-nope"))
(apin "a-broken-file" '(add-classpath (java.io.File. "/tmp/s18-nope")))
(apin "a-broken-num" '(add-classpath 42))
(apin "a-arity0" '(add-classpath))
(apin "a-arity2" '(add-classpath "a" "b"))
(apin "a-warn-text" '(with-out-str (try (add-classpath "file:/s18-nope/") (catch Throwable _ :caught))))
;; ---- B: the working mechanism. RT.load resolves through the stable
;; parent loader (L0); pointing the context loader at L0 restores the
;; historical flow, so every pin below drives the real fn.
(.setContextClassLoader (Thread/currentThread) (.getParent (.getClassLoader (class probe-fn))))
(apin "a-ret" '(let [sw (java.io.StringWriter.)] (binding [*out* sw] (add-classpath (str "file:" root "/d1/")))))
(apin "a-add2" '(let [sw (java.io.StringWriter.)] (binding [*out* sw] (add-classpath (str "file:" root "/d2/")))))
(apin "a-urls" '(vec (map str (.getURLs ^java.net.URLClassLoader (.getContextClassLoader (Thread/currentThread))))))
;; d1 was added before d2: append order means the first dir wins.
(apin "a-order" '(do (require 'my.lib) @(resolve 'my.lib/marker)))
(apin "a-effect" '(do (require 'new.lib) @(resolve 'new.lib/marker)))
(apin "a-dup-ret" '(let [sw (java.io.StringWriter.)] (binding [*out* sw] (add-classpath (str "file:" root "/d1/")))))
(apin "a-dup-urls" '(vec (map str (.getURLs ^java.net.URLClassLoader (.getContextClassLoader (Thread/currentThread))))))
(apin "a-missing" '(let [sw (java.io.StringWriter.)] (binding [*out* sw] (add-classpath "file:/s18-nope/"))))
(apin "a-use-true" '(binding [*use-context-classloader* true] (require 'use-t.lib) @(resolve 'use-t.lib/marker)))
(apin "a-use-false" '(binding [*use-context-classloader* false] (require 'use-f.lib) @(resolve 'use-f.lib/marker)))
;; io/resource consults the context loader under either flag value
;; (oracle-only context for the flag's port role: data, unconsulted).
(.setContextClassLoader (Thread/currentThread) (java.net.URLClassLoader. (into-array [(.. (java.io.File. (str root "/ctxonly")) toURI toURL)])))
(apin "a-res-true" '(binding [*use-context-classloader* true] (str (clojure.java.io/resource "ctx-marker.txt"))))
(apin "a-res-false" '(binding [*use-context-classloader* false] (str (clojure.java.io/resource "ctx-marker.txt"))))
(rmrf (java.io.File. root))
