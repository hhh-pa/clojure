;; S5D differential corpus 11: str/print, print vars, with-out-str, subs, name/namespace.
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Print/str, subs), s5d-gaps7f (str/subs), s5d-gaps7g4
;; (subs frac, arities), s5d-gaps7g5 (subs/name/nil edges), s5d-gaps7d/7d2/7g3
;; (name/namespace/symbol/keyword), gaps3/gaps4/gaps6 (print/out edges),
;; s5d-gap/gap2 (str-nested, subs, out), probe39 (str/print/subs/name/out).
;; KNOWN DIVERGENCE: print interleave order (:print-interleave-*) -- JVM prints
;; "(" then bounded-realizes (side-effect prints land inside the brackets);
;; the port interleaves differently.
;; KNOWN DIVERGENCE: fractional/negative *print-length* terminates on the JVM
;; (unlimited) but hangs the port (:plen-neg, :plen-frac, :plen-nan).
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- str (7f, 7g4, 7g5, probe39, s5d-gap) ---
(p ":str-nil" #(str nil))
(p ":str-nilnil" #(str nil nil))
(p ":str-mix" #(str :a/b "x" nil 5))
(p ":str-0" #(str))
(p ":str-mix2" #(str 1 :a "b" [1] {:k 1}))
(p ":str-char" #(str \a))
(p ":str-nested" #(vector (str ["a"]) (str ["a" 1] {:k "v"}) (str 'sym \c)))
(p ":a-str" #(eval (list 'str)))

;; --- pr-str / print-str (7f, 7g4, 7g5, probe39) ---
(p ":prstr-mix" #(pr-str :a "b" nil [1]))
(p ":prstr-0" #(pr-str))
(p ":prstr-exact" #(pr-str "a" :b 1 nil [1]))
(p ":prstr-nil" #(pr-str nil))
(p ":printstr-mix" #(print-str :a "b" nil [1]))
(p ":printstr-exact" #(print-str "a" :b 1 nil [1]))
(p ":printstr-nil" #(print-str nil))
(p ":a-printstr" #(eval (list 'print-str)))

;; --- print fns (probe39, 7g4) ---
(p ":print-basic" #(with-out-str (print "a" 1)))
(p ":println-basic" #(with-out-str (println "a" 1)))
(p ":pr-basic" #(with-out-str (pr "a" 1)))
(p ":prn-basic" #(with-out-str (prn "a" 1)))
(p ":print-newline" #(with-out-str (print "a") (newline) (print "b")))
(p ":prn-0" #(with-out-str (prn)))
(p ":print-nil" #(with-out-str (print nil)))
(p ":pr-nil" #(with-out-str (pr nil)))
(p ":println-nil" #(with-out-str (println nil)))
(p ":prn-nil" #(with-out-str (prn nil)))
(p ":a-pr" #(eval (list 'pr)))
(p ":a-print" #(eval (list 'print)))
(p ":a-prn" #(eval (list 'prn)))
(p ":a-println" #(eval (list 'println)))
(p ":a-newline" #(eval (list 'newline)))
(p ":a-flush" #(eval (list 'flush)))

;; --- with-out-str / *out* / *err* (probe39, gaps3, gaps6, 7g5) ---
(p ":wos-nested" #(with-out-str (print "a") (with-out-str (print "INNER")) (print "c")))
(p ":wos-ret" #(with-out-str 42))
(p ":wos-0" #(eval '(with-out-str)))
(p ":out-class" #(str (class *out*)))
(p ":err-class" #(str (class *err*)))
(p ":out-capture" #(let [sw (java.io.StringWriter.)] (binding [*out* sw] (print "o39") (flush)) (str sw)))
(p ":err-capture" #(let [sw (java.io.StringWriter.)] (binding [*err* sw] (binding [*out* *err*] (print "e39"))) (str sw)))
(p ":flush-ret" #(flush))
(p ":out-nil" #(binding [*out* nil] (print "x")))
(p ":out-num" #(binding [*out* 5] (print "x")))

;; --- *print-length* (probe39, pins, NEW negative/fractional/NaN/integral-double) ---
(p ":plen-2" #(binding [*print-length* 2] (pr-str [1 2 3 4])))
(p ":plen-0" #(binding [*print-length* 0] (pr-str [1])))
(p ":plen-2-printstr" #(binding [*print-length* 2] (print-str [1 2 3])))
(p ":plen-neg" #(binding [*print-length* -1] (pr-str [1 2 3])))
(p ":plen-frac" #(binding [*print-length* 2.5] (pr-str [1 2 3 4])))
(p ":plen-nan" #(binding [*print-length* ##NaN] (pr-str [1 2 3 4])))
(p ":plen-intdbl" #(binding [*print-length* 2.0] (pr-str [1 2 3 4])))
(p ":plen-range" #(binding [*print-length* 3] (pr-str (range))))
(p ":print-interleave-chunked" #(with-out-str (print (binding [*print-length* 1] (pr-str (map (fn [x] (print "E") x) [1 2 3]))))))
(p ":print-interleave-list" #(with-out-str (print (binding [*print-length* 1] (pr-str (map (fn [x] (print "E") x) '(1 2 3)))))))

;; --- *print-level* (probe39, pins, NEW negative/fractional/NaN) ---
(p ":plevel-1-map" #(binding [*print-level* 1] (pr-str {:a {:b 1}})))
(p ":plevel-1-vec" #(binding [*print-level* 1] (pr-str [[1]])))
(p ":plevel-2-vec" #(binding [*print-level* 2] (pr-str [[1]])))
(p ":plevel-3-vec" #(binding [*print-level* 3] (pr-str [[[1]]])))
(p ":plevel-0" #(binding [*print-level* 0] (pr-str [1])))
(p ":plevel-neg" #(binding [*print-level* -1] (pr-str [[1]])))
(p ":plevel-negfrac" #(binding [*print-level* -0.5] (pr-str [[1]])))
(p ":plevel-frac" #(binding [*print-level* 2.5] (pr-str [[[1]]])))
(p ":plevel-nan" #(binding [*print-level* ##NaN] (pr-str [[[1]]])))
(p ":plevel-intdbl" #(binding [*print-level* 2.0] (pr-str [[[1]]])))

;; --- *print-dup* / *print-meta* / *print-readably* / *print-namespace-maps* ---
;; NOTE: s5d-pins.md claims *print-namespace-maps* root FALSE; the oracle
;; (1.13.0-master-SNAPSHOT) shows root TRUE with common-ns lifting. Re-verified
;; 2026-10-18; the entries below pin actual behavior (see MANIFEST).
(p ":pdup-vec" #(binding [*print-dup* true] (pr-str [1 2])))
(p ":pmeta-plain" #(binding [*print-meta* true] (pr-str [1])))
(p ":pmeta-meta" #(binding [*print-meta* true] (pr-str (with-meta [1] {:a 1}))))
(p ":pmeta-nested" #(binding [*print-meta* true] (pr-str [(with-meta [1] {:a 1})])))
(p ":preadably-false" #(binding [*print-readably* false] (pr-str "a\nb")))
(p ":preadably-root" #(vector *print-readably* (binding [*print-readably* true] (print-str "a")) (binding [*print-readably* true] (with-out-str (print "a")))))
(p ":pnsmaps-root" #(deref #'*print-namespace-maps*))
(p ":pnsmaps-same2" #(pr-str {:a/x 1 :a/y 2}))
(p ":pnsmaps-mixed" #(pr-str {:a/x 1 :a/y 2 :b/z 3}))
(p ":pnsmaps-single" #(pr-str {:a/x 1}))
(p ":pnsmaps-mixed-str" #(pr-str {:a/x 1 "k" 2}))
(p ":pnsmaps-bound-false" #(binding [*print-namespace-maps* false] (pr-str {:a/x 1 :a/y 2})))
(p ":pnsmaps-bound-true" #(binding [*print-namespace-maps* true] (pr-str {:a/x 1 :a/y 2})))
(p ":pnsmaps-nested" #(pr-str [{:a/x 1 :a/y 2}]))
(p ":pmeta-root" #(deref #'*print-meta*))
(p ":pdup-root" #(deref #'*print-dup*))
(p ":plen-root" #(deref #'*print-length*))
(p ":plevel-root" #(deref #'*print-level*))
(p ":print-vars-dynamic" #(vec (map (fn [v] (boolean (:dynamic (meta v)))) [#'*print-meta* #'*print-readably* #'*print-dup* #'*print-length* #'*print-level* #'*print-namespace-maps*])))

;; --- subs (7f, 7g4, 7g5, probe39, s5d-gap, gaps6) ---
(p ":subs-basic" #(subs "hello" 1 3))
(p ":subs-start" #(subs "hello" 1))
(p ":subs-end" #(subs "hello" 5))
(p ":subs-oob" #(subs "hello" 6))
(p ":subs-inv" #(subs "hello" 3 1))
(p ":subs-neg" #(subs "hello" -1 2))
(p ":subs-neg1" #(subs "abc" -1))
(p ":subs-nil" #(subs nil 0))
(p ":subs-2nil" #(subs nil 0 1))
(p ":subs-frac" #(subs "ab" 1.5))
(p ":subs-frac2" #(subs "abc" 0 1.5))
(p ":subs-empty" #(subs "" 0))
(p ":subs-nilidx" #(subs "ab" nil))
(p ":subs-stridx" #(subs "ab" "a"))
(p ":subs-abc-1" #(subs "abc" 1))
(p ":subs-abc-12" #(subs "abc" 1 2))
(p ":subs-abc-3" #(subs "abc" 3))
(p ":subs-abc-00" #(subs "abc" 0 0))
(p ":subs-abc-5" #(subs "abc" 5))
(p ":subs-abc-05" #(subs "abc" 0 5))
(p ":subs-abc-15" #(subs "abc" 1 5))
(p ":subs-abc-21" #(subs "abc" 2 1))
(p ":a-subs" #(eval (list 'subs)))

;; --- name / namespace (7d, 7d2, 7g3, 7g4, 7g5, probe39) ---
(p ":name-nil" #(name nil))
(p ":namespace-nil" #(namespace nil))
(p ":namespace-str" #(namespace "s"))
(p ":name-kw" #(name :a/b))
(p ":name-sym" #(name 'c/d))
(p ":namespace-sym" #(namespace 'a/b))
(p ":namespace-kw" #(namespace :a/b))
(p ":namespace-kw-plain" #(namespace :a))
(p ":name-str" #(name "s"))
(p ":name-e" #(name "e"))
(p ":name-empty" #(name ""))
(p ":name-num" #(name 5))
(p ":name-true" #(name true))
(p ":a-name" #(eval (list 'name)))
(p ":a-namespace" #(eval (list 'namespace)))

;; --- symbol / keyword (7d, 7d2, 7g3, 7g4, probe39) ---
(p ":symbol-nil" #(symbol nil))
(p ":symbol-str" #(symbol "a"))
(p ":symbol-slash" #(symbol "a/b"))
(p ":symbol-2" #(symbol "a" "b"))
(p ":symbol-ns-nm" #(symbol "ns" "nm"))
(p ":symbol-kw" #(symbol :a/b))
(p ":symbol-num" #(symbol 5))
(p ":symbol-true" #(symbol true))
(p ":a-symbol" #(eval (list 'symbol)))
(p ":keyword-nil" #(keyword nil))
(p ":keyword-kw" #(keyword :a))
(p ":keyword-sym" #(keyword 'a/b))
(p ":keyword-str" #(keyword "a"))
(p ":keyword-slash" #(keyword "a/b"))
(p ":keyword-2" #(keyword "a" "b"))
(p ":keyword-ns-nm" #(keyword "ns" "nm"))
(p ":keyword-num" #(keyword 5))
(p ":keyword-true" #(keyword true))
(p ":keyword-3" #(keyword "a" "b" "c"))
(p ":a-keyword" #(eval (list 'keyword)))
(println "S5D-11 DONE")
