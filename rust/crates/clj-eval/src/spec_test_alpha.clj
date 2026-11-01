;; Copyright (c) Rich Hickey. All rights reserved.
;; The use and distribution terms for this software are covered by the
;; Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
;; which can be found in the file epl-v10.html at the root of this distribution.
;; By using this software in any fashion, you are agreeing to be bound by
;; the terms of this license.
;; You must not remove this notice, or any other, from this software.

;; gap-122 slice FDEF (finale): a faithful port of
;; clojure.spec.test.alpha (upstream spec.alpha-0.6.249,
;; clojure/spec/test/alpha.clj, 579 lines) covering `instrument`/
;; `unstrument`, `check`/`check-fn`, and the reporting fns (see
;; docs/gap-122/slice-fdef.md).
;;
;; Port adaptations:
;; - No `clojure.pprint` / `clojure.string` requires (neither is
;;   embedded): `summarize-results` prints with `prn`, and the
;;   stack-trace helpers use `re-find` plus local `subs`/`count`
;;   loops (`stc-…`).
;; - Upstream `@#'s/…` private refs become `@(find-var '…)`:
;;   qualified refs to private vars are a compile error (probe
;;   P4/P16), while `find-var` + `deref` ignores privacy exactly
;;   like the JVM.
;; - Caller capture in `spec-checking-fn` is guarded
;;   (`(try … (catch Throwable _ nil))`): `Thread/currentThread`
;;   throws at eval time (probe P3), so `::stc/caller` is always
;;   absent (slice-fdef divergence 9).
;; - `.applyTo` becomes `(apply f args)`; `Exception` /
;;   `IllegalArgumentException` construction becomes `ex-info`
;;   (same messages).
;; - The `check` driver (`quick-check`) samples `gen/sample`
;;   instead of `gen/for-all*` + `gen/quick-check` (neither is
;;   ported): first-failure-wins, no `:shrunk` (divergence 1),
;;   `:num-tests` (default 1000, kept) + `:gen` honored, other
;;   test.check opts ignored (divergence 3).
;; - `stc/` keys are written `:clojure.spec.test.check/…`
;;   literally (no `clojure.spec.test.check` ns exists to alias).
;; - `with-instrument-disabled` and the kwargs codegen are
;;   list-built (no syntax-quote: the port reads a whole load
;;   batch before evaluating it, so syntax-quote would qualify
;;   against the requiring namespace instead of this one);
;;   `binding`/`fn`/`let` stay bare (specials are not vars)
;;   while spec/gen/test vars qualify.
(ns clojure.spec.test.alpha
  (:refer-clojure :exclude [test])
  (:require [clojure.spec.alpha :as s]
            [clojure.spec.gen.alpha :as gen]))

;; Upstream test/alpha.clj 20-23.
(defn- throwable?
  [x]
  (instance? Throwable x))

;; Upstream test/alpha.clj 25-27.
(defn ->sym
  [x]
  (@(find-var 'clojure.spec.alpha/->sym) x))

;; Upstream test/alpha.clj 29-36 (`IllegalArgumentException.` →
;; `ex-info`, same message).
(defn- ->var
  [s-or-v]
  (if (var? s-or-v)
    s-or-v
    (let [v (and (symbol? s-or-v) (resolve s-or-v))]
      (if (var? v)
        v
        (throw (ex-info (str (pr-str s-or-v) " does not name a var") {}))))))

;; Upstream test/alpha.clj 38-42.
(defn- collectionize
  [x]
  (if (symbol? x)
    (list x)
    x))

;; Upstream test/alpha.clj 44-57.
(defn enumerate-namespace
  "Given a symbol naming an ns, or a collection of such symbols,
returns the set of all symbols naming vars in those nses."
  [ns-sym-or-syms]
  (into
   #{}
   (mapcat (fn [ns-sym]
             (map
              (fn [name-sym]
                (symbol (name ns-sym) (name name-sym)))
              (keys (ns-interns ns-sym)))))
   (collectionize ns-sym-or-syms)))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;; instrument ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;; Upstream test/alpha.clj 59-61.
(def ^:private ^:dynamic *instrument-enabled*
  "if false, instrumented fns call straight through"
  true)

;; Upstream test/alpha.clj 63-66.
(defn- fn-spec?
  "Fn-spec must include at least :args or :ret specs."
  [m]
  (or (:args m) (:ret m)))

;; Upstream test/alpha.clj 68-72, list-built (no syntax-quote).
(defmacro with-instrument-disabled
  "Disables instrument's checking of calls, within a scope."
  [& body]
  (list* 'binding ['clojure.spec.test.alpha/*instrument-enabled* nil] body))

;; The `clojure.string` subset `interpret-stack-trace-element`
;; needs, over `subs`/`count` (upstream requires
;; `clojure.string`; the port embeds no such ns). `stc-find`
;; locates the first occurrence of `needle` at/after `from`.
(defn- stc-find
  [s needle from]
  (let [n (count s) m (count needle)]
    (loop [i from]
      (cond (> (+ i m) n) nil
            (= needle (subs s i (+ i m))) i
            :else (recur (inc i))))))

;; Upstream `str/replace` for a fixed-string match, all occurrences.
(defn- stc-replace-all
  [s match replacement]
  (loop [out "" rest s]
    (if-let [i (stc-find rest match 0)]
      (recur (str out (subs rest 0 i) replacement)
             (subs rest (+ i (count match)) (count rest)))
      (str out rest))))

;; Upstream `(str/split s #"\$" 3)` (fixed-string split, limit 3).
(defn- stc-split-dollar
  [s]
  (loop [parts [] rest s]
    (if (= 2 (count parts))
      (conj parts rest)
      (if-let [i (stc-find rest "$" 0)]
        (recur (conj parts (subs rest 0 i))
               (subs rest (inc i) (count rest)))
        (conj parts rest)))))

;; Upstream `(str/replace s #"--.*" "")`.
(defn- stc-degensym
  [s]
  (if-let [i (stc-find s "--" 0)]
    (subs s 0 i)
    s))

;; Upstream `clojure.lang.Compiler/demunge` over the munge table
;; (the `_` → `-` restoration runs last, so `_…_` tokens match
;; first).
(defn- stc-demunge
  [s]
  (reduce (fn [acc [m r]] (stc-replace-all acc m r))
          s
          [["_PLUS_" "+"] ["_STAR_" "*"] ["_SLASH_" "/"]
           ["_EQ_" "="] ["_LT_" "<"] ["_GT_" ">"]
           ["_BANG_" "!"] ["_QMARK_" "?"] ["_" "-"]]))

;; Upstream test/alpha.clj 74-75 (`str/includes?` → `re-find`;
;; the match text carries no regex syntax).
(defn- thunk-frame? [s]
  (some? (re-find #"--KVS--EMULATION--THUNK--" s)))

;; Upstream test/alpha.clj 77-107, over the `stc-…` locals.
(defn- interpret-stack-trace-element
  "Given the vector-of-syms form of a stacktrace element produced
by e.g. Throwable->map, returns a map form that adds some keys
guessing the original Clojure names. Returns a map with

  :class         class name symbol from stack trace
  :method        method symbol from stack trace
  :file          filename from stack trace
  :line          line number from stack trace
  :var-scope     optional Clojure var symbol scoping fn def
  :local-fn      optional local Clojure symbol scoping fn def

For non-Clojure fns, :scope and :local-fn will be absent."
  [[cls method file line]]
  (let [clojure? (contains? #{'invoke 'invokeStatic} method)
        demunge stc-demunge
        degensym stc-degensym
        [ns-sym name-sym local] (when clojure?
                                  (map demunge (stc-split-dollar (str cls))))]
    (merge {:file file
            :line line
            :method method
            :class cls}
           (when (and ns-sym name-sym)
             {:var-scope (symbol ns-sym name-sym)})
           (when local
             {:local-fn (symbol (degensym local))
              :thunk? (thunk-frame? local)}))))

;; Upstream test/alpha.clj 109-122.
(defn- stacktrace-relevant-to-instrument
  "Takes a coll of stack trace elements (as returned by
StackTraceElement->vec) and returns a coll of maps as per
interpret-stack-trace-element that are relevant to a
failure in instrument."
  [elems]
  (let [plumbing? (fn [{:keys [var-scope thunk?]}]
                    (or thunk?
                        (contains? '#{clojure.spec.test.alpha/spec-checking-fn
                                      clojure.core/apply}
                                   var-scope)))]
    (sequence (comp (map StackTraceElement->vec)
                    (map interpret-stack-trace-element)
                    (filter :var-scope)
                    (drop-while plumbing?))
              elems)))

;; Upstream test/alpha.clj 124-151. Caller capture is guarded
;; (`Thread/currentThread` throws at eval time, probe P3);
;; `.applyTo` is `(apply f args)`.
(defn- spec-checking-fn
  "Takes a function name, a function f, and an fspec and returns a thunk that
  first conforms the arguments given then calls f with those arguments if
  the conform succeeds. Otherwise, an exception is thrown containing information
  about the conform failure."
  [fn-name f fn-spec]
  (let [fn-spec (@(find-var 'clojure.spec.alpha/maybe-spec) fn-spec)
        conform! (fn [fn-name role spec data args]
                   (let [conformed (s/conform spec data)]
                     (if (= :clojure.spec.alpha/invalid conformed)
                       (let [caller (try (->> (.getStackTrace (Thread/currentThread))
                                              stacktrace-relevant-to-instrument
                                              first)
                                         (catch Throwable _ nil))
                             ed (merge (assoc (s/explain-data* spec [] [] [] data)
                                              :clojure.spec.alpha/fn fn-name
                                              :clojure.spec.alpha/args args
                                              :clojure.spec.alpha/failure :instrument)
                                       (when caller
                                         {:clojure.spec.test.check/caller (dissoc caller :class :method)}))]
                         (throw (ex-info
                                 (str "Call to " fn-name " did not conform to spec.")
                                 ed)))
                       conformed)))]
    (fn
     [& args]
     (if *instrument-enabled*
       (with-instrument-disabled
         (when (:args fn-spec) (conform! fn-name :args (:args fn-spec) args args))
         (binding [*instrument-enabled* true]
           (apply f args)))
       (apply f args)))))

;; Upstream test/alpha.clj 153-156.
(defn- no-fspec
  [v spec]
  (ex-info (str "Fn at " v " is not spec'ed.")
           {:var v :spec spec :clojure.spec.alpha/failure :no-fspec}))

;; Upstream test/alpha.clj 158.
(defonce ^:private instrumented-vars (atom {}))

;; Upstream test/alpha.clj 160-167.
(defn- find-varargs-decl
  "Takes an arglist and returns the restargs binding form if found, else nil."
  [arglist]
  (let [[_ decl :as restargs] (->> arglist
                                   (split-with (complement #{'&}))
                                   second)]
    (and (= 2 (count restargs))
         decl)))

;; Upstream test/alpha.clj 169-170.
(defn- has-kwargs? [arglists]
  (->> arglists (some find-varargs-decl) map?))

;; Upstream test/alpha.clj 172-182.
(defn- kwargs->kvs
  "Takes the restargs of a kwargs function call and checks for a trailing element.
  If found, that element is flattened into a sequence of key->value pairs and
  concatenated onto the preceding arguments."
  [args]
  (if (even? (count args))
    args
    (concat (butlast args)
            (reduce-kv (fn [acc k v] (->> acc (cons v) (cons k)))
                       ()
                       (last args)))))

;; Upstream test/alpha.clj 184-189.
(defn- gen-fixed-args-syms
  "Takes an arglist and generates a vector of names corresponding to the fixed
  args found."
  [arglist]
  (->> arglist (take-while (complement #{'&})) (map (fn [_] (gensym))) vec))

;; Upstream test/alpha.clj 191-199, list-built. The eval'd code
;; reaches the private `kwargs->kvs` via `find-var` + `deref`
;; (qualified refs to private vars are a compile error, probe
;; P4/P16 — and `eval` compiles too).
(defn- build-kwargs-body
  "Takes a function name fn-name and arglist and returns code for a function body that
  handles kwargs by calling fn-name with any fixed followed by its restargs transformed
  from kwargs to kvs."
  [fn-name arglist]
  (let [alias (gensym "kwargs")
        head-args (gen-fixed-args-syms arglist)
        kvs-call (list (list 'clojure.core/deref
                             (list 'clojure.core/find-var
                                   (list 'quote 'clojure.spec.test.alpha/kwargs->kvs)))
                       alias)]
    (list (vec (conj head-args '& alias))
          (list* 'clojure.core/apply fn-name (concat (vec head-args) (list kvs-call))))))

;; Upstream test/alpha.clj 201-208, list-built.
(defn- build-varargs-body
  "Takes a function name fn-name and arglist and returns code for a function body that
  handles varargs by calling fn-name with any fixed args followed by its rest args."
  [fn-name arglist]
  (let [head-args (gen-fixed-args-syms arglist)
        alias (gensym "restargs")]
    (list (vec (conj head-args '& alias))
          (list* 'clojure.core/apply fn-name (concat (vec head-args) (list alias))))))

;; Upstream test/alpha.clj 210-216, list-built.
(defn- build-fixed-args-body
  "Takes a function name fn-name and arglist and returns code for a function body that
  handles fixed args by calling fn-name with its fixed args."
  [fn-name arglist]
  (let [arglist (gen-fixed-args-syms arglist)]
    (list arglist
          (list* fn-name arglist))))

;; Upstream test/alpha.clj 218-233, list-built (`fn` stays bare:
;; specials are not vars).
(defn- build-flattener-code
  "Takes argslists and generates code for a HOF that given a function, returns a forwarding thunk
  of analogous arglists that ensures that kwargs are passed as kvs to the original function."
  [arglists]
  (let [closed-over-name (gensym "inner")]
    (list 'fn (vector closed-over-name)
          (list* 'fn '--KVS--EMULATION--THUNK--
                 (map (fn [arglist]
                        (let [varargs-decl (find-varargs-decl arglist)]
                          (cond (map? varargs-decl) (build-kwargs-body closed-over-name arglist)
                                varargs-decl (build-varargs-body closed-over-name arglist)
                                :default (build-fixed-args-body closed-over-name arglist))))
                      (or arglists '([& args])))))))

;; Upstream test/alpha.clj 253-262.
(defn- maybe-wrap-kvs-emulation
  "Takes an argslist and function f and returns f except when arglists
  contains a kwargs binding, else wraps f with a forwarding thunk that
  flattens a trailing map into kvs if present in the kwargs call."
  [f arglists]
  (if (has-kwargs? arglists)
    (let [flattener-code (build-flattener-code arglists)
          kvs-emu (eval flattener-code)]
      (kvs-emu f))
    f))

;; Upstream test/alpha.clj 264-271.
(defn- instrument-choose-fn
  "Helper for instrument."
  [f spec sym {over :gen :keys [stub replace]}]
  (if (some #{sym} stub)
    (-> spec (s/gen over) gen/generate)
    (get replace sym f)))

;; Upstream test/alpha.clj 273-276.
(defn- instrument-choose-spec
  "Helper for instrument"
  [spec sym {overrides :spec}]
  (get overrides sym spec))

;; Upstream test/alpha.clj 278-296.
(defn- instrument-1
  [s opts]
  (when-let [v (resolve s)]
    (when-not (-> v meta :macro)
      (let [spec (s/get-spec v)
            {:keys [raw wrapped]} (get @instrumented-vars v)
            current @v
            to-wrap (if (= wrapped current) raw current)
            ospec (or (instrument-choose-spec spec s opts)
                      (throw (no-fspec v spec)))
            ofn (instrument-choose-fn to-wrap ospec s opts)
            checked (spec-checking-fn (->sym v) ofn ospec)
            arglists (->> v meta :arglists (sort-by count) seq)
            wrapped (maybe-wrap-kvs-emulation checked arglists)]
        (alter-var-root v (constantly wrapped))
        (swap! instrumented-vars assoc v {:raw to-wrap :wrapped wrapped})
        (->sym v)))))

;; Upstream test/alpha.clj 298-306.
(defn- unstrument-1
  [s]
  (when-let [v (resolve s)]
    (when-let [{:keys [raw wrapped]} (get @instrumented-vars v)]
      (swap! instrumented-vars dissoc v)
      (let [current @v]
        (when (= wrapped current)
          (alter-var-root v (constantly raw))
          (->sym v))))))

;; Upstream test/alpha.clj 308-311.
(defn- opt-syms
  "Returns set of symbols referenced by 'instrument' opts map"
  [opts]
  (reduce into #{} [(:stub opts) (keys (:replace opts)) (keys (:spec opts))]))

;; Upstream test/alpha.clj 313-316.
(defn- fn-spec-name?
  [s]
  (and (symbol? s)
       (not (some-> (resolve s) meta :macro))))

;; Upstream test/alpha.clj 318-328.
(defn instrumentable-syms
  "Given an opts map as per instrument, returns the set of syms
that can be instrumented."
  ([] (instrumentable-syms nil))
  ([opts]
   (assert (every? ident? (keys (:gen opts))) "instrument :gen expects ident keys")
   (reduce into #{} [(filter fn-spec-name? (keys (s/registry)))
                     (keys (:spec opts))
                     (:stub opts)
                     (keys (:replace opts))])))

;; Upstream test/alpha.clj 330-375.
(defn instrument
  "Instruments the vars named by sym-or-syms, a symbol or collection
of symbols, or all instrumentable vars if sym-or-syms is not
specified.

If a var has an :args fn-spec, sets the var's root binding to a
fn that checks arg conformance (throwing an exception on failure)
before delegating to the original fn.

The opts map can be used to override registered specs, and/or to
replace fn implementations entirely. Opts for symbols not included
in sym-or-syms are ignored. This facilitates sharing a common
options map across many different calls to instrument.

The opts map may have the following keys:

  :spec     a map from var-name symbols to override specs
  :stub     a set of var-name symbols to be replaced by stubs
  :gen      a map from spec names to generator overrides
  :replace  a map from var-name symbols to replacement fns

:spec overrides registered fn-specs with specs your provide. Use
:spec overrides to provide specs for libraries that do not have
them, or to constrain your own use of a fn to a subset of its
spec'ed contract.

:stub replaces a fn with a stub that checks :args, then uses the
:ret spec to generate a return value.

:gen overrides are used only for :stub generation.

:replace replaces a fn with a fn that checks args conformance, then
invokes the fn you provide, enabling arbitrary stubbing and mocking.

:spec can be used in combination with :stub or :replace.

Returns a collection of syms naming the vars instrumented."
  ([] (instrument (instrumentable-syms)))
  ([sym-or-syms] (instrument sym-or-syms nil))
  ([sym-or-syms opts]
   (locking instrumented-vars
     (into
      []
      (comp (filter (instrumentable-syms opts))
            (distinct)
            (map #(instrument-1 % opts))
            (remove nil?))
      (collectionize sym-or-syms)))))

;; Upstream test/alpha.clj 377-387.
(defn unstrument
  "Undoes instrument on the vars named by sym-or-syms, specified
as in instrument. With no args, unstruments all instrumented vars.
Returns a collection of syms naming the vars unstrumented."
  ([] (unstrument (map ->sym (keys @instrumented-vars))))
  ([sym-or-syms]
   (locking instrumented-vars
     (into
      []
      (comp (filter symbol?)
            (map unstrument-1)
            (remove nil?))
      (collectionize sym-or-syms)))))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;; testing  ;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;

;; Upstream test/alpha.clj 391-399.
(defn- explain-check
  [args spec v role]
  (ex-info
   "Specification-based check failed"
   (when-not (s/valid? spec v nil)
     (assoc (s/explain-data* spec [role] [] [] v)
            :clojure.spec.test.alpha/args args
            :clojure.spec.test.alpha/val v
            :clojure.spec.alpha/failure :check-failed))))

;; Upstream test/alpha.clj 401-419.
(defn- check-call
  "Returns true if call passes specs, otherwise *returns* an exception
with explain-data + ::s/failure."
  [f specs args]
  (let [cargs (when (:args specs) (s/conform (:args specs) args))]
    (if (= cargs :clojure.spec.alpha/invalid)
      (explain-check args (:args specs) args :args)
      (let [ret (apply f args)
            cret (when (:ret specs) (s/conform (:ret specs) ret))]
        (if (= cret :clojure.spec.alpha/invalid)
          (explain-check args (:ret specs) ret :ret)
          (if (and (:args specs) (:ret specs) (:fn specs))
            (if (s/valid? (:fn specs) {:args cargs :ret cret})
              true
              (explain-check args (:fn specs) {:args cargs :ret cret} :fn))
            true))))))

;; Upstream test/alpha.clj 421-428. Sample-based driver (see the
;; file header): honors `:num-tests` (default 1000, kept) and
;; `:gen`; catches `Throwable` per trial; returns `{:result …,
;; :num-tests n}` (+ `:fail [args]` on failure), never `:shrunk`.
(defn- quick-check
  [f specs {gen :gen opts :clojure.spec.test.check/opts}]
  (let [{:keys [num-tests] :or {num-tests 1000}} opts
        g (try (s/gen (:args specs) gen) (catch Throwable t t))]
    (if (throwable? g)
      {:result g}
      (loop [n 0 samples (gen/sample g num-tests)]
        (if (empty? samples)
          {:result true :num-tests n}
          (let [args (first samples)
                r (try (check-call f specs args) (catch Throwable t t))]
            (if (true? r)
              (recur (inc n) (rest samples))
              {:result r :num-tests (inc n) :fail [args]})))))))

;; Upstream test/alpha.clj 430-442.
(defn- make-check-result
  "Builds spec result map."
  [check-sym spec test-check-ret]
  (merge {:spec spec
          :clojure.spec.test.check/ret test-check-ret}
         (when check-sym
           {:sym check-sym})
         (when-let [result (-> test-check-ret :result)]
           (when-not (true? result) {:failure result}))
         (when-let [shrunk (-> test-check-ret :shrunk)]
           {:failure (:result shrunk)})))

;; Upstream test/alpha.clj 444-466.
(defn- check-1
  [{:keys [s f v spec]} opts]
  (let [re-inst? (and v (seq (unstrument s)) true)
        f (or f (when v @v))
        specd (s/spec spec)]
    (try
      (cond
        (or (nil? f) (some-> v meta :macro))
        {:failure (ex-info "No fn to spec" {:clojure.spec.alpha/failure :no-fn})
         :sym s :spec spec}

        (:args specd)
        (let [tcret (quick-check f specd opts)]
          (make-check-result s spec tcret))

        :default
        {:failure (ex-info "No :args spec" {:clojure.spec.alpha/failure :no-args-spec})
         :sym s :spec spec})
      (finally
        (when re-inst? (instrument s))))))

;; Upstream test/alpha.clj 468-473.
(defn- sym->check-map
  [s]
  (let [v (resolve s)]
    {:s s
     :v v
     :spec (when v (s/get-spec v))}))

;; Upstream test/alpha.clj 475-477.
(defn- validate-check-opts
  [opts]
  (assert (every? ident? (keys (:gen opts))) "check :gen expects ident keys"))

;; Upstream test/alpha.clj 479-486.
(defn check-fn
  "Runs generative tests for fn f using spec and opts. See
'check' for options and return."
  ([f spec] (check-fn f spec nil))
  ([f spec opts]
   (validate-check-opts opts)
   (check-1 {:f f :spec spec} opts)))

;; Upstream test/alpha.clj 488-497.
(defn checkable-syms
  "Given an opts map as per check, returns the set of syms that
can be checked."
  ([] (checkable-syms nil))
  ([opts]
   (validate-check-opts opts)
   (reduce into #{} [(filter fn-spec-name? (keys (s/registry)))
                     (keys (:spec opts))])))

;; Upstream test/alpha.clj 499-545.
(defn check
  "Run generative tests for spec conformance on vars named by
sym-or-syms, a symbol or collection of symbols. If sym-or-syms
is not specified, check all checkable vars.

The opts map includes the following optional keys, where stc
aliases clojure.spec.test.check:

::stc/opts  opts to flow through test.check/quick-check
:gen        map from spec names to generator overrides

The ::stc/opts include :num-tests in addition to the keys
documented by test.check. Generator overrides are passed to
spec/gen when generating function args.

Returns a lazy sequence of check result maps with the following
keys

:spec       the spec tested
:sym        optional symbol naming the var tested
:failure    optional test failure
::stc/ret   optional value returned by test.check/quick-check

The value for :failure can be any exception. Exceptions thrown by
spec itself will have an ::s/failure value in ex-data:

:check-failed   at least one checked return did not conform
:no-args-spec   no :args spec provided
:no-fn          no fn provided
:no-fspec       no fspec provided
:no-gen         unable to generate :args
:instrument     invalid args detected by instrument
"
  ([] (check (checkable-syms)))
  ([sym-or-syms] (check sym-or-syms nil))
  ([sym-or-syms opts]
   (->> (collectionize sym-or-syms)
        (filter (checkable-syms opts))
        (pmap
         #(check-1 (sym->check-map %) opts)))))

;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;;; check reporting  ;;;;;;;;;;;;;;;;;;;;;;;

;; Upstream test/alpha.clj 551-553.
(defn- failure-type
  [x]
  (:clojure.spec.alpha/failure (ex-data x)))

;; Upstream test/alpha.clj 555-559.
(defn- unwrap-failure
  [x]
  (if (failure-type x)
    (ex-data x)
    x))

;; Upstream test/alpha.clj 561-574.
(defn- result-type
  "Returns the type of the check result. This can be any of the
::s/failure keywords documented in 'check', or:

  :check-passed   all checked fn returns conformed
  :check-threw    checked fn threw an exception"
  [ret]
  (let [failure (:failure ret)]
    (cond
      (nil? failure) :check-passed
      (failure-type failure) (failure-type failure)
      :default :check-threw)))

;; Upstream test/alpha.clj 576-585.
(defn abbrev-result
  "Given a check result, returns an abbreviated version
suitable for summary use."
  [x]
  (if (:failure x)
    (-> (dissoc x :clojure.spec.test.check/ret)
        (update :spec s/describe)
        (update :failure unwrap-failure))
    (dissoc x :spec :clojure.spec.test.check/ret)))

;; Upstream test/alpha.clj 587-600 (`pp/pprint` → `prn`: the port
;; embeds no pprint — slice-fdef divergence 2).
(defn summarize-results
  "Given a collection of check-results, e.g. from 'check', pretty
prints the summary-result (default abbrev-result) of each.

Returns a map with :total, the total number of results, plus a
key with a count for each different :type of result."
  ([check-results] (summarize-results check-results abbrev-result))
  ([check-results summary-result]
   (reduce
    (fn [summary result]
      (prn (summary-result result))
      (-> summary
          (update :total inc)
          (update (result-type result) (fnil inc 0))))
    {:total 0}
    check-results)))
