;; S5D differential corpus 13: format errors (flags/precision/width/index/missing).
;; Oracle: java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp clojure.jar clojure.main
;; Sources: s5d-pins.md (Format: flag/precision/check-order), s5d-gaps7f/7h/7h2
;; (error specs), FmtProbe/2 (Clojure mirrors), s5d-gap format-extra,
;; probe39 format-edges. %a/%t/%c errors live in 14.
(defn p [tag thunk]
  (print tag " => ")
  (prn (try (thunk)
            (catch Throwable t
              (str "THROW " (.getName (class t)) ": " (.getMessage t))))))

;; --- conversion mismatches (7f, 7h, FmtProbe, probe39) ---
(p ":fmt-d-dbl" #(format "%d" 5.5))
(p ":fmt-d-str" #(format "%d" "a"))
(p ":fmt-f-int" #(format "%f" 5))
(p ":c-d-bigint" #(format "%d" 5N))
(p ":c-d-ratio" #(format "%d" 2/3))
(p ":c-d-bigdec" #(format "%d" 5.5M))
(p ":c-d-char" #(format "%d" \a))
(p ":c-d-bool" #(format "%d" true))
(p ":c-f-ratio" #(format "%f" 2/3))
(p ":c-f-bigint" #(format "%f" 5N))
(p ":c-x-big" #(format "%x" 255N))
(p ":bool-d" #(format "%d" true))
(p ":dbl-d" #(format "%d" 1.5))
(p ":str-d" #(format "%d" "5"))
(p ":long-f" #(format "%f" 5))
(p ":str-f" #(format "%f" "1.5"))
(p ":fmt-d-x" #(format "%d" "x"))
(p ":f-intobj" #(format "%f" (int 5)))
(p ":x-char" #(format "%x" \A))

;; --- flag/conversion mismatches (7h2, FmtProbe/2) ---
(p ":plus-s" #(format "%+s" "ab"))
(p ":hash-s" #(format "%#s" "a"))
(p ":plus-x" #(format "%+x" 5))
(p ":comma-x" #(format "%,x" 5000))
(p ":space-x" #(format "% x" 5))
(p ":paren-xneg" #(format "%(x" -5))
(p ":zero-s" #(format "%05s" "ab"))
(p ":sharp-g" #(format "%#.0g" 5.0))
(p ":pct-spacex" #(format "% x" 1))
(p ":pct-hashd" #(format "%#d" 1))
(p ":pct-0s" #(format "%0s" "a"))
(p ":pct-commas" #(format "%,s" "a"))
(p ":pct-parens" #(format "%(s" "a"))

;; --- duplicate / illegal flags (FmtProbe) ---
(p ":dup-flag" #(format "%--s" "a"))
(p ":plus-space" #(format "%+ d" 1))
(p ":pct-plusx" #(format "%+ x" 1))

;; --- precision errors (7f, 7h2, FmtProbe, s5d-gap) ---
(p ":prec-d" #(format "%.3d" 5))
(p ":prec-d2" #(format "%.2d" 1))
(p ":pct-5dot3" #(format "%5.3" 1))

;; --- unknown conversions (7h, 7h2, FmtProbe/2, s5d-gap) ---
(p ":unknown-flag" #(format "%_s" "a"))
(p ":c-bad-conv" #(format "%q" 5))
(p ":c-bad-flag" #(format "%_s" "a"))
(p ":c-star" #(format "%*s" "ab"))
(p ":c-pct-end" #(format "100%"))
(p ":conv-Q" #(format "%Q" "a"))
(p ":fmt-q" #(format "%q" 1))
(p ":inc5" #(format "%5"))
(p ":inc-dash" #(format "%-"))
(p ":inc-dot" #(format "%."))
(p ":inc-dot2" #(format "%.2"))
(p ":inc-lt" #(format "%<"))
(p ":inc-idx" #(format "%2$"))
(p ":inc-2" #(format "%2"))
(p ":inc-pct" #(format "%"))
(p ":pct-dot" #(format "%." "a"))
(p ":pct-5dot" #(format "%5." "a"))
(p ":pct-2" #(format "%2" "a"))
(p ":pct-space" #(format "% " "a"))
(p ":incomplete" #(format "%5" 1))
(p ":incomplete-dot" #(format "%.2" 1))
(p ":fmt-5" #(format "%5" 1))
(p ":neg-prec" #(format "%.-2f" 1.5))
(p ":c-neg-width" #(format "%-0s" "ab"))

;; --- argument indexing (7h, 7h2, FmtProbe/2) ---
(p ":c-idx0" #(format "%0$s" "a"))
(p ":idx0" #(format "%0$s" "a"))
(p ":pct-0idx-d" #(format "%0$d" 1))
(p ":pct-hugeidx" #(format "%99999999999999999999$s" "a"))
(p ":lt-noprev" #(format "%<s" "a"))
(p ":lt-noref" #(format "%<s" "a"))

;; --- missing / nil format (7f, 7h2, probe39) ---
(p ":fmt-missing" #(format "%s %s" 1))
(p ":missing" #(format "%s %s" "a"))
(p ":fmt-d-noargs" #(format "%d"))
(p ":fmt-0" #(format))
(p ":fmt-nil" #(format nil "a"))
(p ":fmt-nil-0" #(format nil))

;; --- %n misuse (7h2, FmtProbe) ---
(p ":n-width" #(format "%5n"))
(p ":n-flag" #(format "%-n"))

;; --- check-order probes (pins; NEW) ---
(p ":co-prec-plus-s" #(format "%.2+s" "ab"))
(p ":co-dash-5n" #(format "%-5n"))
(p ":co-space-plus-d" #(format "% +d" 5))
(p ":co-plus-hash-s" #(format "%+#s" "ab"))
(p ":co-maxindex-s" #(format "%2147483647$s" "a" "b"))
(p ":co-8th-s" #(format "%8$s" "a"))
(p ":co-dash-1st-s" #(format "%-1$s" "a"))
(p ":co-1st-dash-s" #(format "%1$-s" "a"))
(println "S5D-13 DONE")
