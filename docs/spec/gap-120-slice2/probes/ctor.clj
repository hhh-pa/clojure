;; #120 slice 2 oracle: emulated construction — `new` + `(Class. args)`
;; over the lattice (String/Object success shapes, overload selection,
;; miss texts/phases, `new` head shapes, shadowing).
;; Run: CP=/tmp/clj113/clojure-1.13.0-alpha9.jar:/tmp/clj113/spec.alpha-0.6.249.jar:
;; /tmp/clj113/core.specs.alpha-0.6.133-alpha10.jar
;; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -Dfile.encoding=UTF-8 \
;;   -cp $CP clojure.main ctor.clj > ctor.out
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

;; --- String success shapes ---
(show "ctor-str-0" #(String.))
(show "ctor-str-1" #(String. "x"))
(show "ctor-str-class" #(class (String. "x")))
(show "ctor-str-eq" #(= String (class (String. "x"))))
(show "ctor-str-ident" #(identical? (String. "x") "x"))
(show "ctor-str-chararr" #(String. (char-array [\a \b])))
(show "ctor-str-chararr-off" #(String. (char-array [\a \b \c]) 1 2))
(show "ctor-str-chararr-empty" #(String. (char-array 0)))
(show "ctor-str-bytearr" #(String. (byte-array [65 66])))
(show "ctor-str-bytearr-off" #(String. (byte-array [65 66 67]) 1 2))
(show "ctor-str-bytearr-bad" #(String. (byte-array [-1])))
(show "ctor-str-bytearr-utf8" #(String. (byte-array [-61 -87]) "UTF-8"))
(show "ctor-str-bytearr-ascii" #(String. (byte-array [65]) "US-ASCII"))
(show "ctor-str-bytearr-latin" #(String. (byte-array [-1]) "ISO-8859-1"))
(show "ctor-str-bytearr-utf16" #(String. (byte-array [-2 -1 65 0]) "UTF-16"))
(show "ctor-str-bytearr-utf16be" #(String. (byte-array [0 65]) "UTF-16BE"))
(show "ctor-str-hibyte" #(String. (byte-array [65 66]) 0))
(show "ctor-str-intarr" #(String. (int-array [65 66]) 0 2))
(show "ctor-str-sb" #(String. (StringBuilder. "xy")))
(show "ctor-str-sbuf" #(String. (StringBuffer. "xy")))
(show "ctor-str-fq" #(java.lang.String. "x"))
(show "new-str-1" #(eval '(new String "x")))
(show "new-str-0" #(eval '(new String)))
(show "new-str-fq" #(eval '(new java.lang.String "x")))
(show "new-str-chararr" #(eval '(new String (char-array [\a]))))
(show "ctor-str-coerce-long" #(String. (char-array [\a \b \c]) 1 (long 2)))
(show-chain "ctor-str-coerce-oobig" "(String. (char-array [\\a \\b \\c]) 1 3000000000)")
(show-chain "ctor-str-coerce-double" "(String. (char-array [\\a \\b \\c]) 1.0 2)")
(show-chain "ctor-str-coerce-str" "(String. (char-array [\\a \\b \\c]) \"1\" 2)")

;; --- String failure shapes ---
(show-chain "ctor-str-arity2" "(String. \"a\" \"b\")")
(show-chain "ctor-str-arity4" "(String. \"a\" \"b\" \"c\" \"d\")")
(show-chain "ctor-str-arity5" "(String. 1 2 3 4 5)")
(show-chain "ctor-str-badtype" "(String. 1)")
(show-chain "ctor-str-nil1" "(String. nil)")
(show-chain "ctor-str-nil2" "(String. nil nil)")
(show-chain "ctor-str-nil3" "(String. nil 0 0)")
(show-chain "ctor-str-off-oob" "(String. (char-array [\\a]) 5 1)")
(show-chain "ctor-str-off-neg" "(String. (char-array [\\a]) -1 1)")
(show-chain "ctor-str-cnt-oob" "(String. (char-array [\\a]) 0 2)")
(show-chain "ctor-str-badcs" "(String. (byte-array [65]) \"NOPE-CS\")")
(show-chain "ctor-str-badcs4" "(String. (byte-array [65]) 0 1 \"NOPE-CS\")")
(show-chain "ctor-str-badcp" "(String. (int-array [-1]) 0 1)")
(show-chain "ctor-str-bytearr-nil" "(String. nil 0 1)")
(show-chain "new-str-bad" "(new String 1)")
(show-chain "new-str-arity" "(new String \"a\" \"b\")")

;; --- Object shapes ---
(show "ctor-obj-class" #(class (Object.)))
(show "ctor-obj-eq" #(= Object (class (Object.))))
(show "ctor-obj-ident" #(identical? (Object.) (Object.)))
(show "new-obj-fq" #(eval '(new java.lang.Object)))
(show-chain "ctor-obj-arity" "(Object. 1)")
(show-chain "ctor-obj-nil" "(Object. nil)")
(show-chain "new-obj-arity" "(new Object 1)")

;; --- non-allow-listed / rejection shapes (JVM succeeds or differs; port: uniform miss) ---
(show "ctor-int" #(Integer. 5))
(show "ctor-int-str" #(Integer. "42"))
(show "ctor-long-str" #(Long. "42"))
(show "ctor-bool-str" #(Boolean. "true"))
(show "ctor-hashmap" #(java.util.HashMap.))
(show "ctor-hashmap-args" #(java.util.HashMap. 4 0.5))
(show "ctor-arraylist" #(java.util.ArrayList.))
(show-chain "ctor-iface" "(java.util.List.)")
(show-chain "ctor-abstract" "(java.text.Format.)")
(show-chain "ctor-private" "(System.)")
(show-chain "ctor-private-new" "(new System)")
(show-chain "ctor-missing-simple" "(NopeKlass. 1)")
(show-chain "ctor-missing-fq" "(java.no.Klass. 1)")
(show-chain "ctor-missing-fq0" "(java.no.Klass.)")
(show-chain "new-missing-simple" "(new NopeKlass)")
(show-chain "new-missing-fq" "(new java.no.Klass)")
(show-chain "val-traildot" "String.")
(show-chain "val-traildot-fq" "java.lang.String.")
(show-chain "static-new" "(String/new \"x\")")

;; --- `new` head shapes ---
(show-chain "new-empty" "(new)")
(show-chain "new-num-head" "(new 1 \"x\")")
(show-chain "new-str-head" "(new \"String\" \"x\")")
(show-chain "new-nil-head" "(new nil \"x\")")
(show-chain "new-list-head" "(new (Class/forName \"java.lang.String\") \"x\")")
(show-chain "new-kw-head" "(new :String \"x\")")
(show "new-shadow" #(eval '(let [new 5] (new String "x"))))
(show "ctor-dot-shadow" #(eval '(let [String 5] (String. "x"))))
(show "new-deftype" #(eval '(do (deftype NPT [a b]) (new user.NPT 1 2))))
(show "ctor-deftype" #(eval '(do (deftype CPT [a b]) (user.CPT. 1 2))))
(show "new-obj-shadow" #(eval '(let [new (fn [x] x)] (class (new Object)))))
