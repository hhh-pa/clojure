;; S13 oracle probes: hierarchy-aware dispatch (oracle: alpha10 jars, issue #57).
;; Run from repo root (/Users/hhu/workspace/clojure):
;; CP=/Users/hhu/.m2/repository/org/clojure/clojure/1.13.0-alpha10/clojure-1.13.0-alpha10.jar:/Users/hhu/.m2/repository/org/clojure/spec.alpha/0.6.249/spec.alpha-0.6.249.jar:/Users/hhu/.m2/repository/org/clojure/core.specs.alpha/0.6.148-alpha12/core.specs.alpha-0.6.148-alpha12.jar; java -Duser.language=en -Duser.country=US -Duser.timezone=UTC -cp $CP clojure.main docs/zero-oos-s13-hierarchy/probes/dispatch.clj > docs/zero-oos-s13-hierarchy/probes/dispatch.out
;; NOTE: single JVM, top to bottom; global derives accumulate (unique tags per label).
(defn throwof [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (pr-str v))))
    (catch Throwable t
      (println (str label " THROW " (.getName (class t)) "|" (pr-str (.getMessage t)))))))
(defn normhex [s]
  (-> s
      (clojure.string/replace #"0x[0-9a-fA-F]+" "0xH")
      (clojure.string/replace #"@[0-9a-fA-F]+" "@L")))
;; throwcause: pin the root cause (stable; skips file paths / eval counters).
(defn throwcause [label form]
  (try
    (let [v (eval form)]
      (println (str label " " (normhex (pr-str v)))))
    (catch Throwable t
      (let [c (loop [x t] (let [n (.getCause ^Throwable x)] (if n (recur n) x)))
            m (-> (.getMessage ^Throwable c)
                  (clojure.string/replace #"eval\d+" "evalN")
                  (clojure.string/replace #"--\d+" "--N")
                  (clojure.string/replace #"@[0-9a-fA-F]+" "@L"))]
        (println (str label " THROW " (.getName (class c)) "|" (pr-str m)))))))
;; ---- A: best-method walk ----
(throwof "x-basic" '(do (defmulti hdbas :k) (defmethod hdbas ::xshape [m] :shape) (derive ::xsq ::xshape) (hdbas {:k ::xsq})))
(throwof "x-specific" '(do (defmulti hdspc :k) (defmethod hdspc ::xshp [m] :shape) (defmethod hdspc ::xsqr [m] :square) (derive ::xsqr ::xshp) (hdspc {:k ::xsqr})))
(throwcause "x-amb" '(do (defmulti hdamb :k) (defmethod hdamb ::xfly [m] :fly) (defmethod hdamb ::xswim [m] :swim) (derive ::xduck ::xfly) (derive ::xduck ::xswim) (hdamb {:k ::xduck})))
(throwof "x-trans" '(do (defmulti hdtrn :k) (defmethod hdtrn ::xtop [m] :top) (derive ::xmid ::xtop) (derive ::xleaf ::xmid) (hdtrn {:k ::xleaf})))
(throwof "x-def-hier" '(do (defmulti hddef :k) (defmethod hddef ::xdshp [m] :shape) (defmethod hddef :default [m] :dflt) (derive ::xdsq ::xdshp) (hddef {:k ::xdsq})))
(throwof "x-vec" '(do (defmulti hdvec (fn [x y] [(:a x) (:b y)])) (defmethod hdvec [::xvsh ::xvot] [x y] :hit) (derive ::xvsq ::xvsh) (hdvec {:a ::xvsq} {:b ::xvot})))
;; ---- B: prefers at dispatch ----
(throwof "x-prefer" '(do (defmulti hdprf :k) (defmethod hdprf ::xpfl [m] :fly) (defmethod hdprf ::xpsw [m] :swim) (derive ::xpdk ::xpfl) (derive ::xpdk ::xpsw) (prefer-method hdprf ::xpsw ::xpfl) (hdprf {:k ::xpdk})))
(throwof "x-prefer-flip" '(do (defmulti hdprg :k) (defmethod hdprg ::xqfl [m] :fly) (defmethod hdprg ::xqsw [m] :swim) (derive ::xqdk ::xqfl) (derive ::xqdk ::xqsw) (prefer-method hdprg ::xqfl ::xqsw) (hdprg {:k ::xqdk})))
(throwof "x-prefers-view" '(do (defmulti hdpv :k) (defmethod hdpv :a [m] :a) (prefer-method hdpv :a :b) (prefers hdpv)))
(throwcause "x-prefer-conflict" '(do (defmulti hdpc :k) (defmethod hdpc :a [m] :a) (defmethod hdpc :b [m] :b) (prefer-method hdpc :a :b) (prefer-method hdpc :b :a)))
(throwcause "x-prefer-self" '(do (defmulti hdps :k) (defmethod hdps :a [m] :a) (prefer-method hdps :a :a)))
(throwof "x-prefer-bare" '(do (defmulti hdpb :k) (prefer-method hdpb :a :b) (prefers hdpb)))
;; ---- C: get-method ----
(throwof "x-get-hier" '(do (defmulti hdget :k) (defmethod hdget ::xgsh [m] :shape) (derive ::xgsq ::xgsh) ((get-method hdget ::xgsq) {:k 1})))
(throwcause "x-get-amb" '(do (defmulti hdga :k) (defmethod hdga ::xhfl [m] :fly) (defmethod hdga ::xhsw [m] :swim) (derive ::xhdk ::xhfl) (derive ::xhdk ::xhsw) (get-method hdga ::xhdk)))
;; ---- D: class dispatch through the lattice ----
(throwof "x-class" '(do (defmulti hdcls class) (defmethod hdcls Object [x] :obj) (hdcls "hi")))
(throwof "x-class-spec" '(do (defmulti hdcs2 class) (defmethod hdcs2 Object [x] :obj) (defmethod hdcs2 String [x] :str) (list (hdcs2 "hi") (hdcs2 3))))
;; ---- E: cache invalidation + remove interplay ----
(throwof "x-cache-inv" '(do (defmulti hdcin :k :default :xnone) (defmethod hdcin :xnone [m] :dflt) (defmethod hdcin ::xcinv [m] :hit) (derive ::xcinvc ::xleft) (list (hdcin {:k ::xcinvc}) (do (derive ::xcinvc ::xcinv) (underive ::xcinvc ::xleft) (hdcin {:k ::xcinvc})))))
(throwof "x-remove-hier" '(do (defmulti hdrmh :k) (defmethod hdrmh ::xrmp [m] :shape) (defmethod hdrmh :default [m] :dflt) (derive ::xrmc ::xrmp) (remove-method hdrmh ::xrmp) (hdrmh {:k ::xrmc})))
;; ---- F: remove-all-methods ----
(throwof "x-rmall" '(do (defmulti hdrma :k) (defmethod hdrma :a [m] :a) (prefer-method hdrma :a :b) (remove-all-methods hdrma) (list (methods hdrma) (prefers hdrma))))
(throwcause "x-rmall-call" '(do (defmulti hdrmb :k) (defmethod hdrmb :a [m] :a) (remove-all-methods hdrmb) (hdrmb {:k :a})))
(throwcause "x-rmall-bad" '(remove-all-methods 5))
(throwof "x-rmall-reuse" '(do (defmulti hdrmc :k) (defmethod hdrmc :a [m] :a) (remove-all-methods hdrmc) (defmethod hdrmc :b [m] :b) (hdrmc {:k :b})))
;; ---- G: :hierarchy option ----
(def hatm (atom (make-hierarchy)))
(defmulti hamlive :k :hierarchy hatm)
(defmethod hamlive ::xshape [m] :xshape)
(throwcause "x-atom-miss" '(hamlive {:k ::xx}))
(throwof "x-atom-live" '(do (swap! hatm derive ::xx ::xshape) (hamlive {:k ::xx})))
(throwof "x-hier-var" '(do (defmulti hdvg :k :hierarchy #'clojure.core/global-hierarchy) (defmethod hdvg ::xxvg [m] :hit) (derive ::xxvgc ::xxvg) (hdvg {:k ::xxvgc})))
(throwof "x-hier-iso" '(do (def hh (atom (make-hierarchy))) (defmulti hdiso :k :hierarchy hh) (defmethod hdiso ::xiso [m] :hit) (derive ::xisoc ::xiso) (try (hdiso {:k ::xisoc}) (catch IllegalArgumentException e :no-match))))
(throwcause "x-hier-map" '(do (defmulti hdmm :k :hierarchy {}) (hdmm {:k :a})))
(throwcause "x-hier-5" '(do (defmulti hdm5 :k :hierarchy 5) (hdm5 {:k :a})))
(throwcause "x-hier-nil" '(do (defmulti hdmn :k :hierarchy nil) (hdmn {:k :a})))
(throwof "x-hier-badval" '(do (defmulti hdmbv :k :hierarchy (atom 5)) (defmethod hdmbv :a [m] :a) (hdmbv {:k :a})))
