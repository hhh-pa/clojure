;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Oracle driver: runs cases.edn and the full-file corpus against the
;; JVM reader and writes expectations.edn + files.edn.
;;
;; Per case (fresh reader; *read-eval* false; :eof sentinel loop like
;; the probes): ok → per-form raw pr-str + canonical pr-strs (default
;; and print-meta configs); err → root simple-class + message +
;; ex-data line/col. Canonicalization (map/set order + lifting +
;; meta-prefix rules) mirrors the Rust harness canon exactly; see
;; tests/corpus.rs. Outputs containing lone surrogates (§24.6) are
;; marked :unrepresentable (un-EDN-encodable for our reader).
;;
;; Full files: per-file form count + FNV-1a hashes of the joined
;; per-form canons (both configs). Files that fail on the oracle are
;; reported and abort regen (investigate, then mark :exclude with a
;; reason by hand).
;;
;; Existing :ours snapshots (divergence cases) are carried forward
;; untouched — only :jvm is regenerated.
;;
;; Usage: ./regen.sh (sets CP/OUT); or directly with OUT=<dir>.

(require 'clojure.java.io 'clojure.edn '[clojure.string :as str])
;; NOTE: only clojure.string is aliased into *ns* (io/edn stay
;; fully qualified) so live resolution matches the probes exactly.

(def corpus-dir (.getParent (clojure.java.io/file *file*)))
(def out-dir (or (System/getenv "OUT") "/tmp/t1174-corpus"))
(.mkdirs (clojure.java.io/file out-dir))

(def EOFMARK (Object.))
(defn root-cause [e]
  (loop [e ^Throwable e] (if-let [c (.getCause e)] (recur c) e)))
(defn simple-name [^Class c] (.getSimpleName c))

;; ---------------------------------------------------------------- canonical printer

;; Codepoint-order string comparison (UTF-8 byte order — what Rust
;; sorts by — equals codepoint order; neither `compare` on strings
;; (UTF-16 order) nor `compare` on codepoint vectors (`compareTo`
;; counts first) matches it).
(defn cp-compare [^String a ^String b]
  (loop [i 0 j 0]
    (let [na (.length a) nb (.length b)]
      (cond (and (= i na) (= j nb)) 0
            (= i na) -1
            (= j nb) 1
            :else (let [ca (.codePointAt a i) cb (.codePointAt b j)]
                    (if (= ca cb)
                      (recur (+ i (Character/charCount ca)) (+ j (Character/charCount cb)))
                      (compare ca cb)))))))

(defn leaf-pr [v]
  (binding [*print-meta* false
            *print-namespace-maps* false
            *print-length* nil
            *print-level* nil
            *print-dup* false
            *print-readably* true]
    (pr-str v)))

(declare canon)

(defn canon-meta-prefix [v print-meta?]
  (when print-meta?
    (let [m (meta v)]
      (when (and m (seq m))
        (str "^"
             (if (and (= 1 (count m)) (:tag m))
               (canon (:tag m) print-meta?)
               (canon m print-meta?))
             " ")))))

(defn liftable [entries]
  ;; Mirror lift-ns: every key qualified-ident? with the same ns.
  (when (seq entries)
    (let [nss (map #(when (qualified-ident? (key %)) (namespace (key %))) entries)]
      (when (and (every? some? nss) (apply = nss))
        (first nss)))))

(defn strip [k]
  (cond (keyword? k) (keyword nil (name k))
        (symbol? k) (symbol nil (name k))
        :else k))

(defn canon-map [m print-meta?]
  (let [entries (vec m)
        lifted (liftable entries)
        show (if lifted
               (map (fn [[k v]] [(strip k) v]) entries)
               entries)
        rendered (map (fn [[k v]] [(canon k print-meta?) (canon v print-meta?)]) show)
        ordered (sort (fn [[k1 v1] [k2 v2]]
                        (let [c (cp-compare k1 k2)]
                          (if (zero? c) (cp-compare v1 v2) c)))
                      rendered)]
    (str (when lifted (str "#:" lifted "{"))
         (when-not lifted "{")
         (str/join ", " (map (fn [[k v]] (str k " " v)) ordered))
         "}")))

(defn canon [v print-meta?]
  (str (canon-meta-prefix v print-meta?)
       (cond
         (instance? clojure.lang.TaggedLiteral v)
         (str "#" (:tag v) " " (canon (:form v) print-meta?))
         (instance? clojure.lang.ReaderConditional v)
         (str "#?" (when (:splicing? v) "@") (canon (:form v) print-meta?))
         (map? v) (canon-map v print-meta?)
         (set? v)
         (str "#{" (str/join " " (sort cp-compare (map #(canon % print-meta?) v))) "}")
         (vector? v)
         (str "[" (str/join " " (map #(canon % print-meta?) v)) "]")
         (or (seq? v) (list? v))
         (str "(" (str/join " " (map #(canon % print-meta?) v)) ")")
         :else (leaf-pr v))))

(defn unrepresentable? [s]
  (and (string? s) (boolean (some #(let [c (int %)] (<= 0xD800 c 0xDFFF)) s))))

(defn safe [s] (if (unrepresentable? s) :unrepresentable s))

;; ---------------------------------------------------------------- reading

(defn case-opts [{:keys [read-cond features]}]
  ;; Mirror the probes: DENY passes {} (no :read-cond key).
  (cond (= read-cond :deny) {}
        (= read-cond :preserve) {:read-cond :preserve}
        :else (cond-> {:read-cond :allow}
                features (assoc :features (set (map #(keyword nil (name %)) features))))))

(defn read-all-forms [s opts]
  (binding [*read-eval* false]
    (let [rdr (clojure.lang.LineNumberingPushbackReader. (java.io.StringReader. s))
          o1 (clojure.lang.LispReader/read rdr opts)]
      (loop [acc [o1]]
        (let [nxt (clojure.lang.LispReader/read rdr (assoc opts :eof EOFMARK))]
          (if (identical? nxt EOFMARK) acc (recur (conj acc nxt))))))))

(defn run-case [{:keys [id input opts]}]
  (try
    (let [forms (read-all-forms input (case-opts opts))]
      {:ok (mapv (fn [f]
                   {:raw (safe (binding [*print-meta* false] (pr-str f)))
                    :canon (safe (canon f false))
                    :canon-meta (safe (canon f true))})
                 forms)})
    (catch Throwable e
      (let [r (root-cause e)
            xd (try (ex-data e) (catch Throwable _ nil))]
        {:err {:class (simple-name (class r))
               :message (safe (str (.getMessage ^Throwable r)))
               :line (:clojure.error/line xd)
               :col (:clojure.error/column xd)}}))))

;; ---------------------------------------------------------------- files

(def repo-root (.getCanonicalPath (clojure.java.io/file corpus-dir "../../../../../")))

(defn fnv1a [^String s]
  (let [bytes (.getBytes s "UTF-8")]
    (loop [i 0 h (unchecked-long 0xcbf29ce484222325)]
      (if (= i (alength bytes))
        (format "%016x" h)
        (recur (inc i)
               (unchecked-multiply (bit-xor h (bit-and (aget bytes i) 0xFF))
                                   (unchecked-long 0x100000001b3)))))))

(defn normalize-gensyms [s]
  (-> s
      (str/replace #"__\d+#" "__N#")
      (str/replace #"__\d+__auto__" "__N__auto__")))

(defn run-file [rel]
  (let [f (clojure.java.io/file repo-root rel)
        s (slurp f)]
    (try
      (let [forms (read-all-forms s {:read-cond :allow})]
        {:path rel
         :count (count forms)
         :fnv-default (fnv1a (normalize-gensyms (str/join "\n" (map #(canon % false) forms))))
         :fnv-printmeta (fnv1a (normalize-gensyms (str/join "\n" (map #(canon % true) forms))))})
      (catch Throwable e
        (let [r (root-cause e)]
          {:path rel
           :oracle-error (str (simple-name (class r)) ": " (.getMessage ^Throwable r))})))))

;; ---------------------------------------------------------------- main

(defn var-ns-name [v]
  ;; "#'clojure.core/map" → ["clojure.core" "map"].
  (let [[_ ns n] (re-matches #"#'([^/]+)/(.+)" (str v))]
    [ns n]))

;; Live *ns* resolution context (= the probes': user + str alias).
(def resolver
  {:current-ns (str (ns-name *ns*))
   :aliases (into {} (map (fn [[k v]] [(name k) (str (ns-name v))])) (ns-aliases *ns*))
   :refers (into {} (map (fn [[k v]] [(name k) (var-ns-name v)])) (ns-refers *ns*))
   :class-map (into {} (comp (filter (comp class? val))
                             (map (fn [[k v]] [(name k) (.getName ^Class v)])))
                (ns-map *ns*))})

(def cases (clojure.edn/read-string (slurp (clojure.java.io/file corpus-dir "cases.edn"))))
(println "cases:" (count cases))

(def old-exp
  (let [f (clojure.java.io/file corpus-dir "expectations.edn")]
    (when (.exists f) (clojure.edn/read-string (slurp f)))))

;; DUMP=1 also writes raw per-form dumps for the cross-check + debugging.
(def dump? (= "1" (System/getenv "DUMP")))
(def raw-dump (atom []))

(def jvm-results
  (into {}
        (map (fn [c]
               (let [res (run-case c)
                     ours (get-in old-exp [:cases (:id c) :ours])]
                 (when dump?
                   (swap! raw-dump conj {:id (:id c) :input (:input c) :jvm res}))
                 [(:id c) (cond-> {:jvm res
                                   :opts (:opts c)}
                            (:diverge c) (assoc :diverge (:diverge c))
                            ours (assoc :ours ours))]))
             cases)))

(with-open [w (clojure.java.io/writer (clojure.java.io/file corpus-dir "expectations.edn"))]
  (.write w ";; Generated by ./regen.sh — do not edit (:jvm is oracle output).\n")
  (.write w ";; :ours snapshots (divergence cases) are reviewed; refresh with\n")
  (.write w ";; `cargo test -p clj-reader --test snapshot` and review the diff.\n")
  (binding [*out* w] (prn {:resolver resolver :cases jvm-results})))
(println "wrote expectations.edn")

(when dump?
  (with-open [w (clojure.java.io/writer (clojure.java.io/file out-dir "raw-dump.edn"))]
    (binding [*out* w] (prn @raw-dump)))
  (println "wrote raw-dump.edn"))

;; Full files: every src/clj/**.clj plus test .clj/.cljc that the
;; oracle reads (failures abort — mark :exclude by hand).
(defn list-files [dir exts]
  (->> (file-seq (clojure.java.io/file repo-root dir))
       (filter #(.isFile ^java.io.File %))
       (map #(.getCanonicalPath ^java.io.File %))
       (map #(str/replace % (str repo-root "/") ""))
       (filter (fn [p] (some #(.endsWith p %) exts)))
       (sort)))

(def file-rels
  (concat (list-files "src/clj" [".clj"])
          (list-files "test" [".clj" ".cljc"])))
(println "files:" (count file-rels))

(def old-files
  (let [f (clojure.java.io/file corpus-dir "files.edn")]
    (when (.exists f)
      (into {} (map (juxt :path identity)) (clojure.edn/read-string (slurp f))))))

(defn excluded? [rel]
  (get-in old-files [rel :exclude]))

;; Excluded paths are skipped (reviewed marks); anything else that
;; fails on the oracle aborts regen for investigation.
(def file-results
  (mapv (fn [rel]
          (if-let [reason (excluded? rel)]
            {:path rel :exclude reason}
            (run-file rel)))
        file-rels))
(when-let [bad (seq (filter :oracle-error file-results))]
  (doseq [b bad] (println "ORACLE-ERROR" (:path b) (:oracle-error b)))
  (println "aborting: mark :exclude with a reason, or fix the driver")
  (System/exit 1))

(def merged-files file-results)

(with-open [w (clojure.java.io/writer (clojure.java.io/file corpus-dir "files.edn"))]
  (.write w ";; Generated by ./regen.sh — do not edit (oracle output).\n")
  (.write w ";; :exclude marks (with reasons) are reviewed by hand.\n")
  (binding [*out* w] (prn merged-files)))
(println "wrote files.edn")
