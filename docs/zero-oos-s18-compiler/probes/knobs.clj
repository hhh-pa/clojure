;; S18 oracle probes: compiler knobs (oracle: alpha10 jars, issue #62).
;; Run from repo root (/home/hhu/workspace/clojure-s18):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s18-compiler/probes/knobs.clj > docs/zero-oos-s18-compiler/probes/knobs.out
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; kpin: value pins hex-normalized; throw pins carry the root cause
;; with eval counters and identity hex normalized (stable reruns).
(defn kpin [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (str (.getMessage ^Throwable c))
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- A: knob roots ----
(kpin "k-compile-path" '*compile-path*)
(kpin "k-compile-files" '*compile-files*)
(kpin "k-compiler-options" '*compiler-options*)
(kpin "k-source-path" '*source-path*)
(kpin "k-fn-loader" '(if (nil? *fn-loader*) "nil" (.getName (class *fn-loader*))))
(kpin "k-fn-loader-val" '*fn-loader*)
(kpin "k-use-context" '*use-context-classloader*)
(kpin "k-verbose" '*verbose-defrecords*)
;; ---- B: dynamic-ness + meta ----
(kpin "k-dyn-compile-path" '(:dynamic (meta #'*compile-path*)))
(kpin "k-dyn-compile-files" '(:dynamic (meta #'*compile-files*)))
(kpin "k-dyn-compiler-options" '(:dynamic (meta #'*compiler-options*)))
(kpin "k-dyn-source-path" '(:dynamic (meta #'*source-path*)))
(kpin "k-dyn-fn-loader" '(:dynamic (meta #'*fn-loader*)))
(kpin "k-dyn-use-context" '(:dynamic (meta #'*use-context-classloader*)))
(kpin "k-dyn-verbose" '(:dynamic (meta #'*verbose-defrecords*)))
(kpin "k-dyn-prims" '(:dynamic (meta #'primitives-classnames)))
(kpin "k-added-compile-path" '(:added (meta #'*compile-path*)))
(kpin "k-added-compile-files" '(:added (meta #'*compile-files*)))
(kpin "k-added-compiler-options" '(:added (meta #'*compiler-options*)))
(kpin "k-doc-compile-path" '(:doc (meta #'*compile-path*)))
(kpin "k-doc-compile-files" '(:doc (meta #'*compile-files*)))
(kpin "k-doc-compiler-options" '(:doc (meta #'*compiler-options*)))
(kpin "k-verbose-doc" '(:doc (meta #'*verbose-defrecords*)))
(kpin "k-addcp-meta" '[(:added (meta #'add-classpath)) (:deprecated (meta #'add-classpath))])
(kpin "k-addcp-doc" '(:doc (meta #'add-classpath)))
(kpin "k-bind-compile-path" '(binding [*compile-path* "/tmp/x"] *compile-path*))
(kpin "k-bind-use-context" '(binding [*use-context-classloader* false] *use-context-classloader*))
(kpin "k-bind-options" '(binding [*compiler-options* {:a 1}] *compiler-options*))
;; ---- C: *verbose-defrecords* role (print-dup over records) ----
(defrecord Rec [x])
(kpin "k-verbose-false" '(binding [*print-dup* true *verbose-defrecords* false] (pr-str (->Rec 1))))
(kpin "k-verbose-true" '(binding [*print-dup* true *verbose-defrecords* true] (pr-str (->Rec 1))))
(kpin "k-verbose-default" '(binding [*print-dup* true] (pr-str (->Rec 1))))
;; ---- D2: var roots (getRawRoot bypasses the script-load bindings,
;; so these pin the true roots even for rebound vars) ----
(kpin "k-root-compile-path" '(.getRawRoot (var clojure.core/*compile-path*)))
(kpin "k-root-compile-files" '(.getRawRoot (var clojure.core/*compile-files*)))
(kpin "k-root-compiler-options" '(.getRawRoot (var clojure.core/*compiler-options*)))
(kpin "k-root-source-path" '(.getRawRoot (var clojure.core/*source-path*)))
(kpin "k-root-fn-loader" '(.getRawRoot (var clojure.core/*fn-loader*)))
(kpin "k-root-use-context" '(.getRawRoot (var clojure.core/*use-context-classloader*)))
(kpin "k-root-verbose" '(.getRawRoot (var clojure.core/*verbose-defrecords*)))
(kpin "k-eval-source" '(eval '*source-path*))
(kpin "k-eval-fnloader" '(eval '*fn-loader*))
;; ---- D: primitives-classnames shape + role (print-dup over Class) ----
(kpin "k-prims-count" '(count primitives-classnames))
(kpin "k-prims-vals" '(sort (vals primitives-classnames)))
(kpin "k-prims-keys" '(sort (map (fn [^Class c] (.getName c)) (keys primitives-classnames))))
(kpin "k-prims-lookup" '(primitives-classnames Long/TYPE))
(kpin "k-prims-miss" '(primitives-classnames String))
(kpin "k-prims-fn?" '(fn? primitives-classnames))
(kpin "k-prims-dup" '(binding [*print-dup* true] (pr-str Long/TYPE)))
(kpin "k-prims-nodup" '(binding [*print-dup* false] (pr-str Long/TYPE)))
