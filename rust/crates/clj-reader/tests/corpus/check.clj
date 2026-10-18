;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Cross-check: the driver's raw JVM outcomes must reproduce the
;; original docs/t1174/probes/probe*.out `>>>` lines (same input →
;; same raw pr-strs / same error triple), modulo gensym-id and
;; identity-hash normalization.
;;
;; Usage: clojure.main check.clj <raw-dump.edn>
;; Exit 0 with "CROSS-CHECK OK (n/m)" or nonzero listing mismatches.

(require '[clojure.java.io :as io]
         '[clojure.edn :as edn]
         '[clojure.string :as str])

(def corpus-dir (.getParent (io/file *file*)))
(def probe-dir (io/file corpus-dir "../../../../../docs/t1174/probes"))
(def jar-probe-out
  ;; Fresh probe outputs are NOT regenerated here (probe.sh does that);
  ;; the check reads whichever probe*.out files exist under OUT.
  [])

(defn normalize [s]
  (-> s
      (str/replace #"__\d+#" "__N#")
      (str/replace #"__\d+__auto__" "__N__auto__")
      (str/replace #"@[0-9a-f]{4,}" "@HEX")))

;; Parse `>>> LABEL REST` lines from a probe output file. LABEL is a
;; pr-str literal (possibly `META `-prefixed); recover the input with
;; the reader. REST is the edn verdict.
(defn parse-probe-line [line]
  (when (str/starts-with? line ">>> ")
    (let [body (subs line 4)
          meta? (str/starts-with? body "META ")
          body (if meta? (subs body 5) body)
          ;; LABEL is one edn form (usually a string literal); REST follows.
          rdr (java.io.PushbackReader. (java.io.StringReader. body))
          label (edn/read {:eof nil} rdr)
          rest-str (str/trim (slurp rdr))]
      (cond
        (string? label)
        {:input label :meta? meta? :rest rest-str}
        ;; probe2's real whitespace verdicts: `>>> WS-XXXX [...]`.
        (and (symbol? label) (re-matches #"WS-[0-9A-F]{4}" (name label)))
        {:input (str "1" (char (Integer/parseInt (subs (name label) 3) 16)) "2")
         :meta? false :rest rest-str}
        :else nil))))

(defn probe-expectations []
  (let [files ["probe.out" "probe-all.out" "probe3.out" "probe4.out" "probe5.out"
               "probe6.out" "probe7.out" "probe8.out" "probe9.out" "probe10.out" "probe11.out"]
        ;; probe.sh outputs (run docs/t1174/probes/probe.sh first).
        out (or (System/getenv "PROBE_OUT") "/tmp/t1174-repro")]
    (->> files
         (map #(io/file out %))
         (filter #(.exists ^java.io.File %))
         (mapcat #(str/split-lines (slurp %)))
         (map parse-probe-line)
         (remove nil?)
         (group-by :input))))

;; The driver dump, grouped by input.
(def dump (edn/read-string (slurp (first *command-line-args*))))
(def dump-by-input (group-by :input dump))

;; Probe verdicts print FULL class names; normalize both sides to
;; [kind payload...] (ex-data included, compared exactly).
(defn probe-verdict [{verdict-str :rest meta? :meta?}]
  (let [v (edn/read-string verdict-str)]
    (if (= :ok (first v))
      ;; META lines carry [:ok pr :meta mpr]; compare the value part.
      (if meta? [:ok (second v)] (into [:ok] (rest v)))
      (let [[_ class _ msg _ xd] v
            simple (last (str/split class #"\."))]
        [:err simple msg (if (= xd "nil") nil xd)]))))

(defn dump-verdict [entry]
  (if-let [ok (get-in entry [:jvm :ok])]
    (into [:ok] (map :raw ok))
    (let [{:keys [class message line col]} (get-in entry [:jvm :err])]
      [:err class message
       (if line (str "#:clojure.error{:line " line ", :column " col "}") nil)])))

;; Probe verdicts with no dump counterpart (eval-only, by design).
(def known-probe-only
  #{["#=(+ 1 2)" [:ok "3"]]
    ["#=String" [:ok "java.lang.String"]]})

(def probe-by-input (probe-expectations))
(println "probe inputs:" (count probe-by-input) "| dump entries:" (count dump))

(def checked (atom 0))
(def skipped (atom []))
(def mismatches (atom []))
(def norm (comp normalize pr-str))

;; Every dump entry's verdict must appear among the probe's verdicts
;; for that input (set comparison — multi-opt inputs have several).
(doseq [[input entries] dump-by-input
        :let [probe-vs (get probe-by-input input)]]
  (cond
    (nil? probe-vs)
    (swap! skipped conj input)

    (some #(= :unrepresentable (:raw %)) (mapcat #(get-in % [:jvm :ok] []) entries))
    (swap! skipped conj input)

    :else
    (let [want (set (map (comp norm probe-verdict) probe-vs))]
      (doseq [entry entries]
        (swap! checked inc)
        (let [got (norm (dump-verdict entry))]
          (when (not (contains? want got))
            (swap! mismatches conj {:input input :want want :got got})))))))

;; And every probe verdict must appear in the dump (minus known
;; eval-only and unrepresentable outputs).
(defn has-unrepresentable? [input]
  (some #(= :unrepresentable (:raw %))
        (mapcat #(get-in % [:jvm :ok] []) (get dump-by-input input []))))

(doseq [[input verdicts] probe-by-input
        :let [dump-vs (set (map (comp norm dump-verdict) (get dump-by-input input [])))]]
  (doseq [pv verdicts
          :let [want (norm (probe-verdict pv))]
          :when (and (not (contains? dump-vs want))
                     (not (contains? known-probe-only [input (probe-verdict pv)]))
                     (get dump-by-input input)
                     (not (has-unrepresentable? input)))]
    (swap! mismatches conj {:input input :want want :got :missing-from-dump})))

(println "checked:" @checked "| dump inputs without probe verdicts:" (count @skipped))
(when (seq @skipped) (println "unmatched dump inputs:" (pr-str (take 12 @skipped)) "..."))
(if (seq @mismatches)
  (do (println "CROSS-CHECK FAILURES:" (count @mismatches))
      (doseq [m (take 20 @mismatches)] (prn m))
      (System/exit 1))
  (println "CROSS-CHECK OK"))
