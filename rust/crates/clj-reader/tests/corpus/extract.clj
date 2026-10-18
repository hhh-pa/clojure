;   Copyright (c) Rich Hickey. All rights reserved.
;   The use and distribution terms for this software are covered by the
;   Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;   which can be found in the file epl-v10.html at the root of this distribution.
;   By using this software in any fashion, you are agreeing to be bound by
;   the terms of this license.
;   You must not remove this notice, or any other, from this software.

;; Extractor: mechanical translation of docs/t1174/probes/probe*.clj
;; reader inputs into corpus case records.
;;
;; Reads each probe file as DATA (no eval), walks the top-level forms,
;; and prints {:id :input :opts} records (pr-str, so escaping is
;; exact). Computed inputs (whitespace codepoints, str-built forms)
;; are evaluated with stub readers. Stream-level POS probes, ISWS
;; lines, and eval-only sections (NSMAP-PRINT/QBRANCH bodies,
;; SURR-COUNT) are skipped by design; print-meta section inputs are
;; harvested as ordinary cases (every case reports print-meta).
;;
;; Usage: java -cp <jar> clojure.main extract.clj
;; Output: EDN records on stdout (one per line).

(require '[clojure.java.io :as io])

(def probe-dir
  ;; <repo>/rust/crates/clj-reader/tests/corpus → <repo>/docs/t1174/probes
  (io/file (.getParent (io/file *file*)) "../../../../../docs/t1174/probes"))
(def allow {:read-cond :allow})
(def feat-ab {:read-cond :allow :features [:a :b]})
(def deny {:read-cond :deny})
(def preserve {:read-cond :preserve})

(defn read-forms [f]
  (let [rdr (java.io.PushbackReader. (io/reader f))]
    (loop [forms []]
      (let [form (try (read {:eof ::eof} rdr) (catch Throwable _ ::eof))]
        (if (= form ::eof) forms (recur (conj forms form)))))))

(def recorded (atom []))
(defn emit [id input opts]
  (swap! recorded conj {:id id :input input :opts opts}))

;; Stub evaluators for computed inputs.
(defn eval-ws-doseq [form]
  ;; (doseq [cp [0x.. ..]] (try-read (str "WS-" (format ..)) (str "1" (char cp) "2")))
  (let [[_ [sym cps] & _] form]
    (doseq [cp (eval cps)]
      (emit (str "WS-" (format "%04X" cp)) (str "1" (char cp) "2") allow))))

(defn eval-str-call [form]
  ;; (t (str ...) ...) with literal opts symbols.
  (let [[_ input & args] form
        input (eval input)
        opts (cond (empty? args) allow
                   (= 'FEAT-AB (first args)) feat-ab
                   (= 'DENY (first args)) deny
                   (= 'PRESERVE (first args)) preserve
                   :else allow)]
    (emit (pr-str input) input opts)))

(defn lit-opts [args]
  (let [[o r] args]
    (cond (nil? o) [allow nil]
          (= 'ALLOW o) [allow r]
          (= 'FEAT-AB o) [feat-ab r]
          (= 'DENY o) [deny r]
          (= 'PRESERVE o) [preserve r]
          :else [allow r])))

(defn walk-doseq [form]
  (let [[_ binding body] form]
    (cond
      ;; (doseq [s [".." ..]] (t s)) / (tm s)
      (and (vector? binding) (= 2 (count binding))
           (vector? (second binding)) (every? string? (second binding))
           (seq? body) (#{'t 'tm} (first body)))
      (doseq [s (second binding)] (emit (pr-str s) s allow))
      ;; (doseq [cp [..]] (try-read ...)) — whitespace batteries.
      (and (vector? binding) (= 2 (count binding))
           (vector? (second binding)) (seq? body) (= 'try-read (first body)))
      (eval-ws-doseq form)
      :else nil)))

(defn walk-call [form]
  (let [[head & args] form]
    (cond
      (= head 't)
      (let [[input & rest] args]
        (cond (string? input)
              (let [[opts re] (lit-opts rest)]
                (if (= re 'true)
                  (println ";; SKIP readeval=true:" (pr-str input))
                  (emit (pr-str input) input opts)))
              (seq? input) (eval-str-call form)
              :else (println ";; SKIP non-literal t call:" (pr-str form))))
      (= head 'try-read)
      (let [[label input] args]
        (if (and (string? label) (string? input))
          (emit label input allow)
          (println ";; SKIP non-literal try-read:" (pr-str form))))
      (= head 'tm)
      (let [[input] args]
        (if (string? input)
          (emit (pr-str input) input allow)
          (println ";; SKIP non-literal tm:" (pr-str form))))
      ;; print-meta/print sections: harvest literal input vectors
      ;; nested anywhere under the prn.
      (= head 'prn)
      (doseq [node (tree-seq #(or (seq? %) (vector? %)) seq args)]
        (when (and (vector? node) (seq node) (every? string? node))
          (doseq [s node] (emit (pr-str s) s allow))))
      :else nil)))

(doseq [probe ["probe.clj" "probe2.clj" "probe3.clj" "probe4.clj" "probe5.clj"
               "probe6.clj" "probe7.clj" "probe8.clj" "probe9.clj" "probe10.clj" "probe11.clj"]]
  (println ";; ====" probe)
  (doseq [form (read-forms (io/file probe-dir probe))]
    (when (seq? form)
      (cond (= 'doseq (first form)) (walk-doseq form)
            :else (walk-call form)))))

;; Dedupe identical (input, opts); suffixed ids on label collision.
(let [by-key (group-by (juxt :input :opts) @recorded)
      seen (atom #{})]
  (doseq [[_ recs] (sort-by (comp :id first) by-key)
          :let [r (first recs)
                base (:id r)
                id (if (contains? @seen base)
                     (str base "@" (pr-str (:opts r)))
                     base)]]
    (swap! seen conj id)
    (prn (assoc r :id id))))
(println ";; total:" (count @recorded) "records," (count (group-by (juxt :input :opts) @recorded)) "unique")
