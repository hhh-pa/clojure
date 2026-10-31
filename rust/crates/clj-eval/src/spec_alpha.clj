;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-122 slice 1: a faithful subset port of clojure.spec.alpha
;; (upstream spec.alpha-0.5.238, clojure/spec/alpha.clj) covering the
;; spec registry, `def`, predicate specs, `s/and`, `valid?`,
;; `conform`/`unform`, and basic `explain`/`explain-str`.
;;
;; Port adaptations (see docs/gap-122/slice-1.md for the full list):
;; - No syntax-quote and no `::auto-resolve` anywhere in this file:
;;   the port reads a whole load batch before evaluating it, so both
;;   would qualify against the *requiring* namespace instead of this
;;   one. Macros build expansions with `list` (fully-qualified
;;   heads), and spec keywords are written out in full.
;; - Spec objects are `defrecord`s (PredSpec/AndSpec) with the Spec
;;   protocol extended to them: the port's `reify` instances do not
;;   support metadata, while the JVM's do (`with-name` needs it).
;; - `specize` is a plain `cond` (not the Specize protocol): the
;;   port has no `Object`-default protocol dispatch, and set dispatch
;;   would miss sorted sets (exact-class keys). Behavior matches.
;; - `fn-sym` always yields nil (the port exposes no fn-name
;;   primitive), so bare-fn specs carry the `::unknown` form.
;; - `Exception.` construction is `ex-info` (same message, caught by
;;   `catch Exception` all the same).
;; - `->sym` reads the var's `:ns`/`:name` meta (no `(symbol var)`).
;; - `ns-qualify` uses `ns-name` (no `.name` interop, no `str` of
;;   namespaces).
;; - `gen*`/`with-gen*` are declared on the protocol but left
;;   unextended (generators are a later slice).
(ns clojure.spec.alpha
  "The spec library specifies the structure of data or functions and provides
  operations to validate, conform, explain, describe, and generate data based on
  the specs.

  Rationale: https://clojure.org/about/spec
  Guide: https://clojure.org/guides/spec"
  (:refer-clojure :exclude [and def]))

(alias 'c 'clojure.core)

;; Upstream alpha.clj 44-50. `gen*`/`with-gen*` are declared so later
;; slices extend them; slice 1 leaves them unimplemented.
(defprotocol Spec
  (conform* [spec x])
  (unform* [spec y])
  (explain* [spec path via in x])
  (gen* [spec overrides path rmap])
  (with-gen* [spec gfn])
  (describe* [spec]))

;; Spec object records. Upstream uses `reify` (alpha.clj 913,
;; 1151); the port's reify instances carry no metadata while its
;; records do, and `with-name`/`spec-name` store the registered name
;; in metadata. AndSpec's `specs` field holds a delay (as upstream),
;; so forward references resolve at first conform, not at build.
(defrecord PredSpec [form pred])
(defrecord AndSpec [forms preds specs])

;; Forward declarations: the `extend-type` blocks below call fns
;; defined later in this file (the port resolves globals at analyze
;; time), as do `specize` and `def-impl` (-> `spec-impl`).
(declare spec-impl dt invalid? unform explain-pred-list)

(extend-type PredSpec
  Spec
  (conform* [this x]
    (let [ret ((:pred this) x)]
      (if ret x :clojure.spec.alpha/invalid)))
  (unform* [this x] x)
  (explain* [this path via in x]
    (when (invalid? (dt (:pred this) x (:form this)))
      [{:path path :pred (:form this) :val x :via via :in in}]))
  (describe* [this] (:form this)))

(extend-type AndSpec
  Spec
  (conform* [this x]
    (let [specs @(:specs this)]
      (loop [ret x i 0]
        (if (< i (count specs))
          (let [nret (conform* (specs i) ret)]
            (if (invalid? nret)
              :clojure.spec.alpha/invalid
              (recur nret (inc i))))
          ret))))
  (unform* [this x]
    (reduce #(unform %2 %1) x (reverse (:preds this))))
  (explain* [this path via in x]
    (explain-pred-list (:forms this) (:preds this) path via in x))
  (describe* [this]
    (apply list 'clojure.spec.alpha/and (:forms this))))

;; Upstream alpha.clj 52.
(defonce ^:private registry-ref (atom {}))

;; Upstream alpha.clj 54-58.
(defn- deep-resolve [reg k]
  (loop [spec k]
    (if (ident? spec)
      (recur (get reg spec))
      spec)))

;; Upstream alpha.clj 60-69.
(defn- reg-resolve
  "returns the spec/regex at end of alias chain starting with k, nil if not found, k if k not ident"
  [k]
  (if (ident? k)
    (let [reg @registry-ref
          spec (get reg k)]
      (if-not (ident? spec)
        spec
        (deep-resolve reg spec)))
    k))

;; Upstream alpha.clj 71-77 (throw via ex-info: same message, still
;; caught by `catch Exception`).
(defn- reg-resolve!
  "returns the spec/regex at end of alias chain starting with k, throws if not found, k if k not ident"
  [k]
  (if (ident? k)
    (c/or (reg-resolve k)
          (throw (ex-info (str "Unable to resolve spec: " k) {})))
    k))

;; Upstream alpha.clj 79-83 (`satisfies?`: the port has no Spec
;; interface class to `instance?` against).
(defn spec?
  "returns x if x is a spec object, else logical false"
  [x]
  (when (satisfies? Spec x)
    x))

;; Upstream alpha.clj 85-88.
(defn regex?
  "returns x if x is a (clojure.spec) regex op, else logical false"
  [x]
  (c/and (:clojure.spec.alpha/op x) x))

;; Upstream alpha.clj 90-96. The port's `with-meta` works on the
;; spec records; narrowed from `IObj` to `spec?` ( every non-ident,
;; non-regex value reaching here is a spec object).
(defn- with-name [spec name]
  (cond
   (ident? spec) spec
   (regex? spec) (assoc spec :clojure.spec.alpha/name name)
   (spec? spec) (with-meta spec (assoc (meta spec) :clojure.spec.alpha/name name))))

;; Upstream alpha.clj 98-108 (same narrowing as `with-name`).
(defn- spec-name [spec]
  (cond
   (ident? spec) spec
   (regex? spec) (:clojure.spec.alpha/name spec)
   (spec? spec) (:clojure.spec.alpha/name (meta spec))))

;; Upstream alpha.clj 110-119. Regex ops cannot be built in slice 1,
;; so a regex reaching here is user data: fail loudly instead of
;; wrapping it (no `regex-spec-impl` yet).
(defn- maybe-spec
  "spec-or-k must be a spec, regex or resolvable kw/sym, else returns nil."
  [spec-or-k]
  (let [s (c/or (c/and (ident? spec-or-k) (reg-resolve spec-or-k))
                (spec? spec-or-k)
                (regex? spec-or-k)
                nil)]
    (if (regex? s)
      (throw (ex-info "regex specs are out of scope in gap-122 slice 1" {}))
      s)))

;; Upstream alpha.clj 121-126 (throw via ex-info, as `reg-resolve!`).
(defn- the-spec
  "spec-or-k must be a spec, regex or kw/sym, else returns nil. Throws if unresolvable kw/sym"
  [spec-or-k]
  (c/or (maybe-spec spec-or-k)
        (when (ident? spec-or-k)
          (throw (ex-info (str "Unable to resolve spec: " spec-or-k) {})))))

;; Spec coercion. Upstream (alpha.clj 128-160) extends the private
;; Specize protocol to Keyword/Symbol/Set/Object; the port dispatches
;; protocols on exact classes only (no `Object` default, and a sorted
;; set would miss an `IPersistentSet` entry), so this is the same
;; dispatch as a `cond`. `fn-sym` is folded to nil (see the file
;; header), which makes the Object branch unconditional.
(defn- specize
  ([s] (c/or (spec? s)
             (cond (ident? s) (specize (reg-resolve! s))
                   (set? s) (spec-impl s s)
                   :else (spec-impl :clojure.spec.alpha/unknown s))))
  ([s form] (c/or (spec? s)
                  (cond (ident? s) (specize (reg-resolve! s))
                        (set? s) (spec-impl form s)
                        :else (spec-impl form s)))))

;; Upstream alpha.clj 162-165.
(defn invalid?
  "tests the validity of a conform return value"
  [ret]
  (identical? :clojure.spec.alpha/invalid ret))

;; Upstream alpha.clj 167-171.
(defn conform
  "Given a spec and a value, returns :clojure.spec.alpha/invalid
if value does not match spec, else the (possibly destructured) value."
  [spec x]
  (conform* (specize spec) x))

;; Upstream alpha.clj 173-178.
(defn unform
  "Given a spec and a value created by or compliant with a call to
  'conform' with the same spec, returns a value with all conform
  destructuring undone."
  [spec x]
  (unform* (specize spec) x))

;; Upstream alpha.clj 180-184.
(defn form
  "returns the spec as data"
  [spec]
  (describe* (specize spec)))

;; Local postwalk (upstream uses clojure.walk, not ported yet).
(defn- spec-postwalk [f form]
  (let [walked (cond (seq? form) (apply list (map #(spec-postwalk f %) form))
                     (vector? form) (mapv #(spec-postwalk f %) form)
                     (record? form) (into form (map #(spec-postwalk f %) form))
                     (map? form) (into {} (map (fn [[k v]]
                                                 [(spec-postwalk f k) (spec-postwalk f v)])
                                               form))
                     (set? form) (into #{} (map #(spec-postwalk f %) form))
                     :else form)]
    (f walked)))

;; Upstream alpha.clj 186-203.
(defn abbrev [form]
  (cond
   (seq? form)
   (spec-postwalk (fn [form]
                    (cond
                     (c/and (symbol? form) (namespace form))
                     (-> form name symbol)

                     (c/and (seq? form) (= 'fn (first form)) (= '[%] (second form)))
                     (last form)

                     :else form))
                  form)

   (c/and (symbol? form) (namespace form))
   (-> form name symbol)

   :else form))

;; Upstream alpha.clj 205-208.
(defn describe
  "returns an abbreviated description of the spec as data"
  [spec]
  (abbrev (form spec)))

;; Upstream alpha.clj 218-223.
(defn explain-data* [spec path via in x]
  (let [probs (explain* (specize spec) path via in x)]
    (when-not (empty? probs)
      {:clojure.spec.alpha/problems probs
       :clojure.spec.alpha/spec spec
       :clojure.spec.alpha/value x})))

;; Upstream alpha.clj 225-232.
(defn explain-data
  "Given a spec and a value x which ought to conform, returns nil if x
  conforms, else a map with at least the key ::problems whose value is
  a collection of problem-maps, where problem-map has at least :path :pred and :val
  keys describing the predicate and the value that failed at that
  path."
  [spec x]
  (explain-data* spec [] (if-let [name (spec-name spec)] [name] []) [] x))

;; Upstream alpha.clj 234-257.
(defn explain-printer
  "Default printer for explain-data. nil indicates a successful validation."
  [ed]
  (if ed
    (let [problems (->> (:clojure.spec.alpha/problems ed)
                        (sort-by #(- (count (:in %))))
                        (sort-by #(- (count (:path %)))))]
      (doseq [{:keys [path pred val reason via in] :as prob} problems]
        (pr val)
        (print " - failed: ")
        (if reason (print reason) (pr (abbrev pred)))
        (when-not (empty? in)
          (print (str " in: " (pr-str in))))
        (when-not (empty? path)
          (print (str " at: " (pr-str path))))
        (when-not (empty? via)
          (print (str " spec: " (pr-str (last via)))))
        (doseq [[k v] prob]
          (when-not (#{:path :pred :val :reason :via :in} k)
            (print "\n\t" (pr-str k) " ")
            (pr v)))
        (newline)))
    (println "Success!")))

;; Upstream alpha.clj 259.
(def ^:dynamic *explain-out* explain-printer)

;; Upstream alpha.clj 261-265.
(defn explain-out
  "Prints explanation data (per 'explain-data') to *out* using the printer in *explain-out*,
   by default explain-printer."
  [ed]
  (*explain-out* ed))

;; Upstream alpha.clj 267-270.
(defn explain
  "Given a spec and a value that fails to conform, prints an explanation to *out*."
  [spec x]
  (explain-out (explain-data spec x)))

;; Upstream alpha.clj 272-276.
(defn explain-str
  "Given a spec and a value that fails to conform, returns an explanation as a string."
  [spec x]
  (with-out-str (explain spec x)))

;; Upstream alpha.clj 305-310. The port's `symbol` takes no var, so
;; the qualified symbol is rebuilt from the var's `:ns`/`:name` meta.
(defn- ->sym
  "Returns a symbol from a symbol or var"
  [x]
  (if (var? x)
    (let [m (meta x)]
      (symbol (str (ns-name (:ns m))) (name (:name m))))
    x))

;; Upstream alpha.clj 312-318 (postwalk-replace is the local
;; `spec-postwalk` over a one-entry table).
(defn- unfn [expr]
  (if (c/and (seq? expr)
             (symbol? (first expr))
             (= "fn*" (name (first expr))))
    (let [[[s] & form] (rest expr)]
      ;; Explicit `fn`, not `#()`: the reader would rewrite the
      ;; quoted `'%` table value into the arg placeholder.
      (conj (spec-postwalk (fn [x] (get {s '%} x x)) form) '[%] 'fn))
    expr))

;; Upstream alpha.clj 320-325.
(defn- res [form]
  (cond
   (keyword? form) form
   (symbol? form) (c/or (-> form resolve ->sym) form)
   (sequential? form) (spec-postwalk #(if (symbol? %) (res %) %) (unfn form))
   :else form))

;; Upstream alpha.clj 327-337. `assert` is bare (not `c/assert`):
;; the port only treats the unqualified and `clojure.core`-qualified
;; spellings as the special form.
(defn ^:skip-wiki def-impl
  "Do not call this directly, use 'def'"
  [k form spec]
  (assert (c/and (ident? k) (namespace k)) "k must be namespaced keyword or resolvable symbol")
  (if (nil? spec)
    (swap! registry-ref dissoc k)
    (let [spec (if (c/or (spec? spec) (regex? spec) (get @registry-ref spec))
                 spec
                 (spec-impl form spec))]
      (swap! registry-ref assoc k (with-name spec k))))
  k)

;; Upstream alpha.clj 339-345. The port has no `.name` interop and
;; `str` of a namespace renders the object rep, so both name reads go
;; through `ns-name`.
(defn- ns-qualify
  "Qualify symbol s by resolving it or using the current *ns*."
  [s]
  (if-let [ns-sym (some-> s namespace symbol)]
    (c/or (some-> (get (ns-aliases *ns*) ns-sym) ns-name str (symbol (name s)))
          s)
    (symbol (str (ns-name *ns*)) (str s))))

;; Upstream alpha.clj 347-354. Built with `list` (no syntax-quote:
;; see the file header).
(defmacro def
  "Given a namespace-qualified keyword or resolvable symbol k, and a
  spec, spec-name, predicate or regex-op makes an entry in the
  registry mapping k to the spec. Use nil to remove an entry in
  the registry for k."
  [k spec-form]
  (let [k (if (symbol? k) (ns-qualify k) k)]
    (list 'clojure.spec.alpha/def-impl (list 'quote k) (list 'quote (res spec-form)) spec-form)))

;; Upstream alpha.clj 356-359.
(defn registry
  "returns the registry map, prefer 'get-spec' to lookup a spec by name"
  []
  @registry-ref)

;; Upstream alpha.clj 361-364.
(defn get-spec
  "Returns spec registered for keyword/symbol/var k, or nil."
  [k]
  (get (registry) (if (keyword? k) k (->sym k))))

;; Upstream alpha.clj 366-383, narrowed to `[form]`: the `:gen`
;; kwarg belongs to the generators slice.
(defmacro spec
  "Takes a single predicate form, e.g. can be the name of a predicate,
  like even?, or a fn literal like #(< % 42).

  Returns a spec."
  [form]
  (list 'clojure.spec.alpha/spec-impl (list 'quote (res form)) form))

;; Upstream alpha.clj 493-501 (built with `list`, as `def`).
(defmacro and
  "Takes predicate/spec-forms, e.g.

  (s/and even? #(< % 42))

  Returns a spec that returns the conformed value. Successive
  conformed values propagate through rest of predicates."
  [& pred-forms]
  (list 'clojure.spec.alpha/and-spec-impl
        (list 'quote (mapv res pred-forms))
        (vec pred-forms)))

;; Upstream alpha.clj 757-768, narrowed: `cpred?` is conformer
;; machinery (a later slice). Throw via ex-info, as `reg-resolve!`.
(defn- dt
  [pred x form]
  (if pred
    (if-let [spec (the-spec pred)]
      (conform spec x)
      (if (ifn? pred)
        (if (pred x) x :clojure.spec.alpha/invalid)
        (throw (ex-info (str (pr-str form) " is not a fn, expected predicate fn") {}))))
    x))

;; Upstream alpha.clj 770-777.
(defn valid?
  "Helper function that returns true when x is valid for spec."
  ([spec x]
   (let [spec (specize spec)]
     (not (invalid? (conform* spec x)))))
  ([spec x form]
   (let [spec (specize spec form)]
     (not (invalid? (conform* spec x))))))

;; Upstream alpha.clj 786-793.
(defn- explain-1 [form pred path via in v]
  (let [pred (maybe-spec pred)]
    (if (spec? pred)
      (explain* pred path (if-let [name (spec-name pred)] (conj via name) via) in v)
      [{:path path :pred form :val v :via via :in in}])))

;; Upstream alpha.clj 1140-1150.
(defn- explain-pred-list
  [forms preds path via in x]
  (loop [ret x
         [form & forms] forms
         [pred & preds] preds]
    (when pred
      (let [nret (dt pred ret form)]
        (if (invalid? nret)
          (explain-1 form pred path via in ret)
          (recur nret forms preds))))))

;; Upstream alpha.clj 913-944, narrowed to `[form pred]` (`gfn` and
;; `cpred?`/`unc` are generator/conformer machinery). Builds a
;; PredSpec record; a regex pred fails loudly (no `regex-spec-impl`
;; yet).
(defn ^:skip-wiki spec-impl
  "Do not call this directly, use 'spec'"
  [form pred]
  (cond
   (spec? pred) pred
   (regex? pred) (throw (ex-info "regex specs are out of scope in gap-122 slice 1" {}))
   (ident? pred) (the-spec pred)
   :else (->PredSpec form pred)))

;; Upstream alpha.clj 1151-1193, narrowed to `[forms preds]` (same
;; reason as `spec-impl`). Builds an AndSpec record; the per-count
;; fast paths are one loop (behavior-identical).
(defn ^:skip-wiki and-spec-impl
  "Do not call this directly, use 'and'"
  [forms preds]
  (->AndSpec forms preds (delay (mapv specize preds forms))))

