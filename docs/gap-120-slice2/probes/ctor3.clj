;; #120 slice 2 oracle batch 3: Ratio/int matching, charset-name
;; validity + aliases + UTF-32, surrogate ground truth, misc ctor
;; edges. Same run header as ctor.clj.
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

;; --- Ratio / int matching ground truth ---
(show "r-class" #(class 1/1))
(show-chain "r-half" "(String. (char-array [\\a \\b \\c]) 1/2 1)")
(show-chain "r-whole" "(String. (char-array [\\a \\b \\c]) 4/2 1)")
(show "r-intcast" #(try (String. (char-array [\a \b \c]) 4/2 1)
                       (catch Throwable t (str (.getName (class t)) " | " (.getMessage t)))))

;; --- nil edges ---
(show-chain "n-3nil" "(String. nil nil nil)")
(show-chain "n-nil-char" "(String. nil \\a)")
(show-chain "n-strs" "(String. \"a\" \"b\" :k)")
(show-chain "n-chars-int" "(String. (char-array [\\a]) 0)")
(show-chain "n-intarr-1" "(String. (int-array [65]))")
(show-chain "n-longarr" "(String. (long-array [65]) 0 1)")

;; --- charset-name validity ---
(show-chain "v-empty" "(String. (byte-array [65]) \"\")")
(show-chain "v-badchars" "(String. (byte-array [65]) \"bad name!\")")
(show-chain "v-colon" "(String. (byte-array [65]) \"x:y\")")

;; --- charset aliases ---
(show "a-ascii" #(String. (byte-array [65]) "ASCII"))
(show "a-latin1" #(String. (byte-array [-1]) "Latin1"))
(show "a-iso8859" #(String. (byte-array [-1]) "ISO8859-1"))
(show "a-isoexact" #(String. (byte-array [-1]) "ISO-8859-1"))
(show "a-utf16" #(String. (byte-array [0 65]) "UTF16"))
(show "a-unicode" #(String. (byte-array [0 65]) "Unicode"))
(show "a-unicodebig" #(String. (byte-array [0 65]) "UnicodeBig"))
(show "a-unicodelittle" #(String. (byte-array [65 0]) "UnicodeLittle"))
(show "a-utf16low" #(String. (byte-array [0 65]) "utf-16"))
(show "a-utf16bel" #(String. (byte-array [0 65]) "utf-16be"))
(show "a-utf8up" #(String. (byte-array [65]) "Utf-8"))

;; --- UTF-32 + long tail ---
(show "u32-be" #(String. (byte-array [0 0 0 65]) "UTF-32"))
(show "u32-be-x" #(String. (byte-array [0 0 0 65]) "UTF-32BE"))
(show "u32-le" #(String. (byte-array [65 0 0 0]) "UTF-32LE"))
(show "u32-bom" #(String. (byte-array [0 0 -2 -1 0 0 0 65]) "UTF-32"))
(show "u32-trunc" #(String. (byte-array [65]) "UTF-32BE"))
(show "u32-badcp" #(String. (byte-array [-1 -1 -1 -1]) "UTF-32BE"))
(show "tail-cp1252" #(String. (byte-array [65]) "windows-1252"))

;; --- surrogate ground truth ---
(show "sg-hex" #(Integer/toHexString (int (.charAt (String. (int-array [55296]) 0 1) 0))))
(show "sg-len" #(.length (String. (int-array [55296 65]) 0 2)))
(show "sg-mixed" #(Integer/toHexString (int (.charAt (String. (int-array [55296 65]) 0 2) 1))))

;; --- hibyte bit semantics ---
(show "hb-2" #(String. (byte-array [65 66]) 2))
(show "hb-neg1" #(String. (byte-array [65]) -1))
(show "hb-negbyte" #(String. (byte-array [-1]) 0))

;; --- BOM-only ---
(show "bom-le" #(String. (byte-array [-1 -2]) "UTF-16"))
(show "bom-be" #(String. (byte-array [-2 -1]) "UTF-16"))

;; --- freshness ---
(show "fr-e" #(identical? (String. "é") "é"))
(show "fr-empty" #(identical? (String.) ""))
(show "fr-chararr" #(identical? (String. (char-array [])) ""))

;; --- misc heads ---
(show-chain "m-deftype-simple-new" "(do (deftype CNX [a]) (eval '(CNX/new 1)))")
(show-chain "m-inst-new" "(.. String (new \"x\"))")
(show-chain "m-obj-new-arity" "(Object/new 1)")
(show-chain "m-new-obj-nil" "(new Object nil)")
(show-chain "m-new-qual" "(new foo/bar 1)")
(show-chain "m-new-qual-str" "(new foo/String 1)")
