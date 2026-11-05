;; #120 slice 2 oracle batch 2: overload disambiguation edges, int
;; coercion matrix, charset-success rows, `Class/new` syntax, `new`
;; vs locals.
;; Same run header as ctor.clj.
(println (str "file.encoding=" (System/getProperty "file.encoding")))
(defn chain [t]
  (loop [t t, out []]
    (if (nil? t)
      out
      (recur (.getCause t) (conj out (str (.getName (class t)) " | " (.getMessage t)))))))

(defn show [label thunk]
  (try
    (let [v (thunk)]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " THROWS " (.getName (class t)) " | " (.getMessage t))))))

(defn show-chain [label src]
  (try
    (let [v (eval (read-string src))]
      (println (str label " => " (pr-str v))))
    (catch Throwable t
      (println (str label " CHAIN:"))
      (doseq [line (chain t)]
        (println (str "   " line))))))

;; --- nil disambiguation ---
(show-chain "d-nil-utf8" "(String. nil \"UTF-8\")")
(show-chain "d-bytes-nil" "(String. (byte-array [65]) nil)")
(show-chain "d-nil-0" "(String. nil 0)")
(show-chain "d-nil-0-0-nil" "(String. nil 0 0 nil)")
(show-chain "d-chars-nil-1" "(String. (char-array [\\a]) nil 1)")
(show-chain "d-nil4" "(String. nil nil nil nil)")
(show-chain "d-nil5" "(String. nil nil nil nil nil)")
(show-chain "d-str-nil" "(String. \"a\" nil)")

;; --- int coercion matrix (offset/count params) ---
(show "c-bigint" #(String. (char-array [\a \b \c]) (bigint 1) (bigint 2)))
(show-chain "c-bigint-big" "(String. (char-array [\\a \\b \\c]) (bigint 1) (bigint 3000000000))")
(show-chain "c-bigdec" "(String. (char-array [\\a \\b \\c]) (bigdec 1) 2)")
(show-chain "c-ratio" "(String. (char-array [\\a \\b \\c]) 1/1 2)")
(show-chain "c-char" "(String. (char-array [\\a \\b \\c]) \\a 1)")
(show-chain "c-bool" "(String. (char-array [\\a \\b \\c]) true 1)")
(show-chain "c-hibyte-big" "(String. (byte-array [65 66]) 3000000000)")
(show "c-hibyte-long" #(String. (byte-array [65 66]) (long 1)))

;; --- charset success rows ---
(show "cs-4arg" #(String. (byte-array [65 66 67]) 0 2 "UTF-8"))
(show "cs-utf16le" #(String. (byte-array [65 0]) "UTF-16LE"))
(show "cs-utf16le-bom" #(String. (byte-array [-1 -2 65 0]) "UTF-16"))
(show "cs-ascii-high" #(String. (byte-array [-1]) "US-ASCII"))
(show "cs-utf16-odd" #(String. (byte-array [65]) "UTF-16BE"))
(show "cs-empty" #(String. (byte-array []) "UTF-8"))
(show "cs-alias" #(String. (byte-array [65]) "utf8"))
(show-chain "cs-nil-name" "(String. (byte-array [65]) nil)")
(show-chain "cs-nil-bytes" "(String. nil \"UTF-8\")")
(show-chain "cs-off-oob" "(String. (byte-array [65]) 5 1 \"UTF-8\")")

;; --- code-point edges ---
(show-chain "cp-surrogate" "(String. (int-array [55296]) 0 1)")
(show-chain "cp-toobig" "(String. (int-array [1114112]) 0 1)")
(show "cp-max" #(String. (int-array [1114111]) 0 1))
(show "cp-astral" #(String. (int-array [128512]) 0 1))

;; --- `Class/new` syntax ---
(show "cn-str" #(eval '(String/new "x")))
(show "cn-str0" #(eval '(String/new)))
(show "cn-obj" #(eval '(class (Object/new))))
(show "cn-int" #(eval '(Integer/new 5)))
(show "cn-fq" #(eval '(java.lang.String/new "x")))
(show-chain "cn-badtype" "(String/new 1)")
(show-chain "cn-arity5" "(String/new 1 2 3 4 5)")
(show-chain "cn-nil1" "(String/new nil)")
(show-chain "cn-missing" "(NopeKlass/new 1)")
(show-chain "cn-missing-fq" "(java.no.Klass/new 1)")
(show-chain "cn-iface" "(java.util.List/new)")
(show-chain "cn-val" "String/new")
(show "cn-resolve" #(resolve 'String/new))
(show "cn-shadow-new" #(eval '(let [new 5] (String/new "x"))))
(show "cn-deftype" #(eval '(do (deftype CNW [a]) (user.CNW/new 1))))

;; --- `new` vs locals / misc ---
(show "nl-new-shadow-str" #(eval '(let [String 5] (new String "x"))))
(show "nl-ctor-shadow-new" #(eval '(let [new 5] (String. "x"))))
(show "nl-new-var-str" #(eval '(do (def String 5) (new String "x"))))
(show-chain "nl-new-numclass" "(new 1 2 3)")
(show "nl-import-face" #(eval '(do (import 'java.util.HashMap) (class (HashMap.)))))
