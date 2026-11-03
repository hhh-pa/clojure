;; Copyright (c) Rich Hickey. All rights reserved.
;; Copyright (c) Hao Hu. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; W6 port of upstream clojure/xml.clj (oracle: upstream source text,
;; read-only; no JVM at runtime, ever). Public API unchanged:
;; element/tag/attrs/content, content-handler, sax-parser,
;; disable-external-entities, startparse-sax[-safe], parse,
;; emit-element, emit.
;;
;; Port adaptations (pinned by rust/crates/clj-eval/tests/w6_xml.rs):
;; - No SAX/JAXP: `__xml-scan*` (w6_runtime.rs) tokenizes the input
;;   into `[kind arg...]` events, and the upstream state machine
;;   (push-chars/push-content over *stack*/*current*/*state*/*sb*)
;;   runs as plain fns. `content-handler` is a map of those fns, so
;;   the default startparse fns drive it as `ch`; custom startparse
;;   fns written against SAX interop cannot run (documented).
;; - Both startparse variants are XXE-immune: the scanner never
;;   resolves external entities (the DOCTYPE subset is inert).
;;   -safe is kept for API parity.
;; - `sax-parser`/`disable-external-entities` return opaque parser
;;   tokens (maps) instead of SAXParser objects.
;; - `parse` also accepts inline XML text (a string whose first
;;   non-whitespace char is `<`); anything else slurps as a path or
;;   stream. Upstream treats every string as a URI; the port DWIMs
;;   (a convenience superset — sources still slurp).
;; - `*sb*` accumulates a plain string (no StringBuilder); the
;;   whitespace test keeps `Character/isWhitespace` verbatim.

(ns ^{:doc "XML reading/writing."
       :author "Rich Hickey"}
  clojure.xml)

(def ^:dynamic *stack*)
(def ^:dynamic *current*)
(def ^:dynamic *state*) ; :element :chars :between
(def ^:dynamic *sb*)

(defstruct element :tag :attrs :content)

(def tag (accessor element :tag))
(def attrs (accessor element :attrs))
(def content (accessor element :content))

;; W6: the upstream ContentHandler proxy methods, as plain fns over
;; the same dynamic state.
(defn- push-content [e c]
  (assoc e :content (conj (or (:content e) []) c)))

(defn- push-chars []
  (when (and (= *state* :chars)
             (some (complement #(Character/isWhitespace (char %))) (str *sb*)))
    (set! *current* (push-content *current* (str *sb*)))))

(defn- start-element [q-name flat-attrs]
  (let [e (struct element
                  (keyword q-name)
                  (when (seq flat-attrs)
                    (into {}
                          (map (fn [[k v]] [(keyword k) v]))
                          (partition 2 flat-attrs))))]
    (push-chars)
    (set! *stack* (conj *stack* *current*))
    (set! *current* e)
    (set! *state* :element)))

(defn- end-element [q-name]
  (push-chars)
  (set! *current* (push-content (peek *stack*) *current*))
  (set! *stack* (pop *stack*))
  (set! *state* :between))

(defn- characters [s]
  (when-not (= *state* :chars)
    (set! *sb* ""))
  (set! *sb* (str *sb* s))
  (set! *state* :chars))

;; W6: the event-fn map the default startparse fns drive (upstream: a
;; ContentHandler proxy over the same state).
(def content-handler
  {:start-element start-element
   :end-element end-element
   :characters characters})

(defn- run-events [events ch]
  (let [start (:start-element ch)
        end (:end-element ch)
        chars (:characters ch)]
    (doseq [[kind a b] events]
      (case kind
        "start" (start a b)
        "end" (end a)
        "chars" (chars a)))))

;; W6: inline XML text (no clojure.string require — embedded files
;; stay independent of it) or a slurpable path/stream.
(defn- slurp-source [s]
  (if (and (string? s)
           (= (first (drop-while #(Character/isWhitespace %) s)) \<))
    s
    (slurp s)))

(defn sax-parser
  "Create a new SAXParser"
  {:added "1.11"}
  []
  {:w6-parser :sax})

(defn disable-external-entities
  "Modifies a SAXParser to disable external entity resolution to prevent XXE attacks"
  {:added "1.11"}
  [parser]
  (assoc parser :external-entities false))

(defn startparse-sax
  "A startparse function suitable for use with clojure.xml/parse.
  Note that this function is open to XXE entity attacks, see startparse-sax-safe."
  {:added "1.0"}
  [s ch]
  ;; W6: the native scanner never resolves externals, so this is as
  ;; safe as -safe (kept separate for API parity).
  (run-events (__xml-scan* (slurp-source s)) ch))

(defn startparse-sax-safe
  "A startparse function suitable for use with clojure.xml/parse.
  External entity resolution is disabled to prevent XXE entity attacks."
  {:added "1.11"}
  [s ch]
  (run-events (__xml-scan* (slurp-source s)) ch))

(defn parse
  "Parses and loads the source s, which can be a File, InputStream or
  String naming a URI. Returns a tree of the xml/element struct-map,
  which has the keys :tag, :attrs, and :content. and accessor fns tag,
  attrs, and content. Other parsers can be supplied by passing
  startparse, a fn taking a source and a ContentHandler and returning
  a parser.

  Prior to 1.11, used startparse-sax by default. As of 1.11, uses
  startparse-sax-safe, which disables XXE (XML External Entity)
  processing. Pass startparse-sax to revert to prior behavior."
  {:added "1.0"}
  ([s] (parse s startparse-sax-safe))
  ([s startparse]
    (binding [*stack* nil
              *current* (struct element)
              *state* :between
              *sb* nil]
      (startparse s content-handler)
      ((:content *current*) 0))))

(defn emit-element [e]
  (if (instance? String e)
    (println e)
    (do
      (print (str "<" (name (:tag e))))
      (when (:attrs e)
	(doseq [attr (:attrs e)]
	  (print (str " " (name (key attr)) "='" (val attr)"'"))))
      (if (:content e)
	(do
	  (println ">")
	  (doseq [c (:content e)]
	    (emit-element c))
	  (println (str "</" (name (:tag e)) ">")))
	(println "/>")))))

(defn emit [x]
  (println "<?xml version='1.0' encoding='UTF-8'?>")
  (emit-element x))
