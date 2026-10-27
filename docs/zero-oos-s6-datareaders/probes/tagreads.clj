;; S6 data readers oracle probes: tag-table read path (oracle: alpha10 jars, issue #49).
;; Run from repo root:
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s6-datareaders/probes/tagreads.clj > docs/zero-oos-s6-datareaders/probes/tagreads.out
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
;; throwdata: pin the ex-data too (read errors carry line/column).
(defn throwdata [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)) "|" (pr-str (ex-data t)))))))
(defn rd [s] (java.io.PushbackReader. (java.io.StringReader. s)))
;; ---- E: table reads ----
(throwof "e-basic" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string "#foo 1")))
(throwof "e-ns" '(binding [*data-readers* {'foo/bar (fn [x] [:ok x])}] (read-string "#foo/bar 1")))
(throwof "e-override-inst" '(binding [*data-readers* {'inst (fn [x] :mine)}] (read-string "#inst \"2024-01-01\"")))
(throwof "e-override-novalid" '(binding [*data-readers* {'inst (fn [x] :mine)}] (read-string "#inst \"bogus\"")))
(throwof "e-nested" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string "[#foo 1]")))
(throwof "e-nested-tag" '(binding [*data-readers* {'foo (fn [x] [:f x]) 'bar (fn [x] [:b x])}] (read-string "#foo #bar 1")))
(throwof "e-quote" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read-string "'#foo 1")))
(throwof "e-no-redescent" '(binding [*data-readers* {'foo (fn [x] (tagged-literal 'bar x)) 'bar (fn [x] [:B x])}] (read-string "#foo 1")))
(throwof "e-nonmap" '(binding [*data-readers* 42] (read-string "#foo 1")))
(throwof "e-nonmap-nil" '(binding [*data-readers* nil] (read-string "#foo 1")))
(throwof "e-nonfn" '(binding [*data-readers* {'foo 42}] (read-string "#foo 1")))
(throwof "e-strkey" '(binding [*data-readers* {"foo" (fn [x] :s)}] (read-string "#foo 1")))
(throwof "e-fn-throws" '(binding [*data-readers* {'foo (fn [x] (throw (ex-info "boom" {:k 1})))}] (read-string "#foo 1")))
(throwof "e-fn-arity" '(binding [*data-readers* {'foo (fn [a b] :x)}] (read-string "#foo 1")))
(throwof "e-unknown" '(read-string "#foo 1"))
(throwdata "e-unknown-data" '(read-string "#foo 1"))
(throwof "e-unknown-nested" '(read-string "[1 #foo 2]"))
(throwdata "e-unknown-nested-data" '(read-string "[1 #foo 2]"))
(throwof "e-unknown-nstag" '(read-string "#foo/bar 1"))
(throwdata "e-stream-unknown" '(read (rd "#foo 1")))
(throwdata "e-stream-unknown-opts" '(read {} (rd "#foo 1")))
(throwdata "e-stream-recursive" '(read (rd "#foo 1") true :eofval true))
;; ---- F: default fn ----
(throwof "f-basic" '(binding [*default-data-reader-fn* (fn [tag x] [:d tag x])] (read-string "#foo 1")))
(throwof "f-tag-shape" '(binding [*default-data-reader-fn* (fn [tag x] [(str (type tag)) (str tag) x])] (read-string "#foo/bar 1")))
(throwof "f-vs-defaults" '(binding [*default-data-reader-fn* (fn [tag x] :dflt)] (read-string "#inst \"2024-01-01\"")))
(throwof "f-vs-table" '(binding [*data-readers* {'foo (fn [x] :t)} *default-data-reader-fn* (fn [tag x] :d)] (read-string "#foo 1")))
(throwof "f-table-miss" '(binding [*data-readers* {'other (fn [x] :t)} *default-data-reader-fn* (fn [tag x] :d)] (read-string "#foo 1")))
(throwof "f-nonfn" '(binding [*default-data-reader-fn* 42] (read-string "#foo 1")))
(throwof "f-arity1" '(binding [*default-data-reader-fn* (fn [tag] :x)] (read-string "#foo 1")))
(throwof "f-throws" '(binding [*default-data-reader-fn* (fn [tag x] (throw (ex-info "bf" {:k 2})))] (read-string "#foo 1")))
;; ---- G: read opts + cond interplay ----
(throwof "g-opts-readers" '(read {:readers {'foo (fn [x] :R)}} (rd "#foo 1")))
(throwof "g-opts-default" '(read {:default (fn [tag x] :D)} (rd "#foo 1")))
(throwof "g-opts-vs-bound" '(binding [*data-readers* {'foo (fn [x] :T)}] (read {:readers {'foo (fn [x] :R)}} (rd "#foo 1"))))
(throwof "g-preserve" '(read {:read-cond :preserve} (rd "#?(:a #inst \"2024-01-01\")")))
(throwof "g-preserve-top" '(read {:read-cond :preserve} (rd "#inst \"2024-01-01\"")))
(throwof "g-preserve-usertag" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read {:read-cond :preserve} (rd "#?(:a #foo 1)"))))
(throwof "g-allow-match" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (read {:read-cond :allow} (rd "#?(:clj #foo 1 :cljs 2)"))))
(throwof "g-allow-nomatch" '(read {:read-cond :allow} (rd "#?(:cljs #bogus 99) :kept")))
(throwof "g-discard" '(read-string "#_ #bogus 1 :kept"))
(throwof "g-discard-inst-bad" '(read-string "#_ #inst \"bogus\" :kept"))
(throwof "e-nilentry" '(binding [*data-readers* {'foo nil}] (read-string "#foo 1")))
(throwof "e-vectable" '(binding [*data-readers* [(fn [x] :v)]] (read-string "#foo 1")))
(throwof "e-niltable-inst" '(binding [*data-readers* nil] (read-string "#inst \"2024-01-01\"")))
(throwof "e-discard-fires" '(do (def drec-calls (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! drec-calls conj x) :v)}] [(read-string "#_ #foo 1 :kept") @drec-calls])))
(throwof "e-discard-order" '(do (def dord-calls (atom [])) (binding [*data-readers* {'a (fn [x] (swap! dord-calls conj [:a x]) :va) 'b (fn [x] (swap! dord-calls conj [:b x]) :vb)}] [(read-string "#_ #a 1 #b 2") @dord-calls])))
(throwof "e-multiform-order" '(do (def mc-calls (atom [])) (binding [*data-readers* {'a (fn [x] (swap! mc-calls conj :a) :va) 'b (fn [x] (swap! mc-calls conj :b) :vb) 'c (fn [x] (swap! mc-calls conj :c) :vc)}] (let [r (rd "#a 1 #_ #b 2 #c 3")] [(read r) (read r) @mc-calls]))))
(throwof "g-preserve-instbad" '(read {:read-cond :preserve} (rd "#?(:a #inst \"bogus\")")))
(throwof "g-allow-nomatch-instbad" '(read {:read-cond :allow} (rd "#?(:cljs #inst \"bogus\") :kept")))
(throwof "e-nonmap-inst" '(binding [*data-readers* 42] (read-string "#inst \"2024-01-01\"")))
(throwof "e-discard-eof" '(do (def eof-calls (atom [])) (binding [*data-readers* {'foo (fn [x] (swap! eof-calls conj x) :v)}] (try [(read-string "#_ #foo 1")] (catch Throwable t [(str (class t)) (str (.getMessage t)) @eof-calls])))))
;; ---- H: read wrap behavior (ReaderException iff line-numbering?) ----
(defn lnpr [s] (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s)))
;; throwchain: pin the outer shape plus the cause shape (named fns only: no eval counters).
(defn throwchain [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (let [c (.getCause ^Throwable t)]
        (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)) "|" (pr-str (ex-data t))
                      (when c (str " CAUSE " (.getName (class c)) "|" (pr-str (.getMessage c)) "|" (pr-str (ex-data c))))))))))
(defn hboom [x] (throw (ex-info "hboom" {:hk 7})))
(defn harity [a b] :h)
(throwdata "h-lnpr-wrap" '(read (lnpr "#foo 1")))
(throwdata "h-lnpr-multiline" '(read (lnpr "#foo\n1")))
(throwdata "h-lnpr-recursive" '(read (lnpr "#foo 1") true :eofval true))
(throwdata "h-lnpr-opts" '(read {} (lnpr "#foo 1")))
(throwdata "h-wis" '(with-in-str "#foo \"x\"" (read {:read-cond :preserve} *in*)))
(throwdata "h-lnpr-instbad" '(read (lnpr "#inst \"bogus\"")))
(throwchain "h-lnpr-fnthrows" '(binding [*data-readers* {'foo hboom}] (read (lnpr "#foo 1"))))
(throwchain "h-lnpr-fnarity" '(binding [*data-readers* {'foo harity}] (read (lnpr "#foo 1"))))
(throwchain "h-str-fnthrows" '(binding [*data-readers* {'foo hboom}] (read-string "#foo 1")))
(throwdata "h-lnpr-discard-throw" '(read (lnpr "#_ #bogus 1 :kept")))
(throwof "e-meta-vec" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (let [v (read-string "^:k #foo 1")] [(meta v) v])))
(throwof "e-meta-scalar" '(binding [*data-readers* {'foo (fn [x] 42)}] (let [v (read-string "^:k #foo 1")] [(meta v) v])))
(throwof "e-meta-inner" '(binding [*data-readers* {'foo (fn [x] [:ok x])}] (meta (read-string "^{:t #foo 1} []"))))
