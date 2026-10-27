;; S-instream oracle probes (oracle: alpha10 jars, issue #36).
;; Run from repo root (/home/hhu/workspace/clojure):
;; CP=/home/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/home/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/home/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/i36-instream/probes/instream.clj > docs/i36-instream/probes/instream.out
(require 'clojure.string)
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@H")
                  (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; normalize object hex for identity-free print pins.
(defn norm [s]
  (-> s
      (clojure.string/replace #"@[0-9a-fA-F]+" "@H")
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")))
;; ---- A: *in* root ----
(throwof "in-class" '(class *in*))
(throwof "in-pr" '(norm (pr-str *in*)))
(throwof "wis-class" '(with-in-str "x" (class *in*)))
;; ---- B: read-line ----
(throwof "rl-basic" '(with-in-str "a\nb" [(read-line) (read-line) (read-line)]))
(throwof "rl-crlf" '(with-in-str "a\r\nb" [(read-line) (read-line) (read-line)]))
(throwof "rl-cr" '(with-in-str "a\rb" [(read-line) (read-line) (read-line)]))
(throwof "rl-mixed" '(with-in-str "a\r\n\rb\nc\rd" [(read-line) (read-line) (read-line) (read-line) (read-line) (read-line)]))
(throwof "rl-empty" '(with-in-str "" (read-line)))
(throwof "rl-trailing-nl" '(with-in-str "a\n" [(read-line) (read-line)]))
(throwof "rl-empty-lines" '(with-in-str "a\n\nb" [(read-line) (read-line) (read-line) (read-line)]))
(throwcause "rl-nil-in" '(binding [*in* nil] (read-line)))
(throwcause "rl-str-in" '(binding [*in* "x"] (read-line)))
(throwcause "rl-num-in" '(binding [*in* 1] (read-line)))
;; ---- C: read ----
(throwof "r-basic" '(with-in-str "1 2" [(read) (read)]))
(throwof "r-shapes" '(with-in-str "1 1.5 \"s\" :k sym \\c" [(read) (read) (read) (read) (read) (read)]))
(throwof "r-comment" '(with-in-str "; c\n42" (read)))
(throwof "r-discard" '(with-in-str "#_ 1 2" (read)))
(throwof "r-meta" '(with-in-str "^:k [1]" (let [v (read)] [(pr-str v) (meta v)])))
(throwof "r-eof-val" '(with-in-str "" (read *in* false :eof)))
(throwof "r-eof-nil" '(with-in-str "" (read *in* false nil)))
(throwof "r-eof-false" '(with-in-str "" (read *in* false false)))
(throwof "r-eof-ws" '(with-in-str "  " (read *in* false :eof)))
(throwcause "r-eof-throw" '(with-in-str "" (read)))
(throwcause "r-eof-3" '(with-in-str "" (read *in* true nil)))
(throwcause "r-trunc" '(with-in-str "(a" (read *in* false :eof)))
(throwcause "r-trunc-str" '(with-in-str "\"abc" (read *in* false :eof)))
(throwcause "r-trunc-char" '(with-in-str "\\" (read *in* false :eof)))
(throwcause "r-trunc-regex" '(with-in-str "#\"ab" (read *in* false :eof)))
(throwof "r-opts-eof" '(with-in-str "" (read {:eof :x} *in*)))
(throwof "r-opts-eofnil" '(with-in-str "" (read {:eof nil} *in*)))
(throwcause "r-opts-noeof" '(with-in-str "" (read {} *in*)))
(throwof "r-opts-unknown" '(with-in-str "1" (read {:bogus 1} *in*)))
(throwof "r-opts-nonmap" '(with-in-str "1" (read :foo *in*)))
(throwof "r-opts-nil" '(with-in-str "1" (read nil *in*)))
(throwcause "r-eoferr-truthy" '(with-in-str "" (read *in* 0 :eof)))
(throwof "r-eoferr-nil" '(with-in-str "" (read *in* nil :eof)))
(throwof "r-readcond-allow" '(with-in-str "#?(:clj 1 :default 2)" (read {:read-cond :allow} *in*)))
(throwof "r-readcond-preserve" '(with-in-str "#?(:clj 1 :default 2)" (read {:read-cond :preserve} *in*)))
(throwof "r-features" '(with-in-str "#?(:foo 1 :default 2)" (read {:read-cond :allow :features #{:foo}} *in*)))
(throwof "r-recursive" '(with-in-str "1" (read *in* true nil true)))
(throwof "r-recursive-discard" '(with-in-str "#_ 1 2" (read *in* true nil true)))
(throwof "r-kw-colons" '(with-in-str ":a:b" (read)))
(throwof "r-baddelim" '(with-in-str ")" (read)))
(throwof "r-baddelim-exdata" '(try (with-in-str ")" (read)) :no-throw (catch Throwable t (ex-data t))))
(throwof "r-cond-plain" '(with-in-str "#?(:clj 1 :default 2)" (read)))
(throwof "r-splice-opts" '(with-in-str "#?@(:clj [1 2])" (read {:read-cond :allow} *in*)))
(throwof "r-splice-nested" '(with-in-str "(1 #?@(:clj [2 3]))" (read {:read-cond :allow} *in*)))
(throwof "r-cond-opts-absent" '(with-in-str "#?(:clj 1 :default 2)" (read {:eof :x} *in*)))
(throwof "r-splice-top" '(with-in-str "#?@(:clj [1 2])" (read)))
(throwof "r-splice-then-ok" '(with-in-str "#?@(:clj [1]) 2" [(try (read) :no-throw (catch Throwable t :threw)) (read)]))
(throwof "r-line-meta" '(with-in-str "(a)\n(b)" (do (read) (meta (read)))))
(throwcause "r-nil-stream" '(read nil))
(throwcause "r-str-stream" '(read "1"))
;; ---- C2: read error wrappers (outer exception, not root cause) ----
(throwof "r-eof-outer" '(with-in-str "" (read)))
(throwof "r-trunc-outer" '(with-in-str "(a" (read)))
(throwof "r-recursive-outer" '(with-in-str "" (read *in* true nil true)))
(throwof "r-exdata" '(try (with-in-str "(a" (read)) :no-throw (catch Throwable t (ex-data t))))
(throwof "r-exdata-eof" '(try (with-in-str "" (read)) :no-throw (catch Throwable t (ex-data t))))
(throwof "r-excause" '(try (with-in-str "(a" (read)) :none (catch Throwable t (.getName (class (.getCause ^Throwable t))))))
;; ---- D: interleave ----
(throwof "mix-1" '(with-in-str "a\n(b)" [(read-line) (read)]))
(throwof "mix-2" '(with-in-str "1 2" [(read) (read-line)]))
(throwof "mix-3" '(with-in-str "(a) tail" [(read) (read-line)]))
;; ---- E: line-seq ----
;; NOTE: ls-basic/ls-class/ls-lazy/ls-empty pin the JVM *rejection* of
;; LineNumberingPushbackReader (not a BufferedReader). The port has one
;; readable kind, so its line-seq consumes it by design (see spec).
(throwof "ls-basic" '(with-in-str "x\ny" (doall (line-seq *in*))))
(throwof "ls-class" '(with-in-str "x" (class (line-seq *in*))))
(throwof "ls-lazy" '(with-in-str "x\ny" (let [s (line-seq *in*)] [(realized? s) (first s) (realized? s)])))
(throwof "ls-empty" '(with-in-str "" (line-seq *in*)))
(throwof "ls-basic2" '(doall (line-seq (java.io.BufferedReader. (java.io.StringReader. "x\ny")))))
(throwof "ls-class2" '(class (line-seq (java.io.BufferedReader. (java.io.StringReader. "x")))))
(throwof "ls-lazy2" '(let [s (line-seq (java.io.BufferedReader. (java.io.StringReader. "x\ny")))] [(realized? (rest s)) (first (rest s)) (realized? (rest s))]))
(throwof "ls-empty2" '(line-seq (java.io.BufferedReader. (java.io.StringReader. ""))))
(throwof "ls-eager" '(try (line-seq nil) :no-throw (catch Throwable t :threw-at-call)))
(throwof "ls-str-eager" '(try (line-seq "x") :no-throw (catch Throwable t :threw-at-call)))
(def ls-closed (atom false))
(throwof "ls-noclose" '(do (doall (line-seq (proxy [java.io.BufferedReader] [(java.io.StringReader. "a")] (close [] (reset! ls-closed true))))) @ls-closed))
(throwcause "ls-nil" '(doall (line-seq nil)))
(throwcause "ls-str" '(doall (line-seq "x")))
;; ---- F: with-in-str ----
(throwof "wis-val" '(with-in-str "1" :body))
(throwof "wis-restore" '(let [out *in*] (with-in-str "x" :y) (identical? out *in*)))
(throwof "wis-nested" '(with-in-str "a" [(read-line) (with-in-str "b" (read-line)) (read-line)]))
(throwof "wis-read" '(with-in-str "1 2" [(read) (read)]))
(throwcause "wis-num" '(with-in-str 1 :x))
;; ---- G: drive-by oracle questions on implemented vars ----
(throwcause "rs-empty" '(read-string ""))
(throwcause "rs-trunc" '(read-string "(a"))
(throwof "rs-cond" '(read-string "#?(:clj 1 :default 2)"))
;; ---- H: arity/opts edge pins (ad-hoc queries, now committed) ----
(throwcause "r-2arity-boolstream" '(with-in-str "1" (read *in* false)))
(throwof "r-optsbad-nilstream" '(read :foo nil))
(throwcause "r-eofopts-nilstream" '(read {:eof :x} nil))
(throwof "r-feat-vec" '(with-in-str "1" (read {:read-cond :allow :features [:foo]} *in*)))
(throwof "r-feat-nil" '(with-in-str "#?(:foo 1 :default 2)" (read {:read-cond :allow :features nil} *in*)))
(throwof "r-feat-set-nonkw" '(with-in-str "1" (read {:read-cond :allow :features #{1}} *in*)))
(throwof "r-rc-bogus" '(with-in-str "#?(:clj 1 :default 2)" (read {:read-cond :bogus} *in*)))
(throwof "r-rc-nil" '(with-in-str "#?(:clj 1 :default 2)" (read {:read-cond nil} *in*)))
(throwof "r-arity5" '(with-in-str "1" (read 1 2 3 4 5)))
(throwof "r-opts-mapentry" '(with-in-str "1" (read (find {:a 1} :a) *in*)))
(throwof "rl-arity1" '(with-in-str "x" (read-line 1)))
(throwof "wis-arity0" '(with-in-str))
(throwof "r-preserve-fail-then-tag" '(with-in-str "#?(a b] #foo \"x\"" [(try (read {:read-cond :preserve} *in*) :no-throw (catch Throwable t :caught)) (read {:read-cond :preserve} *in*)]))
