# `clojure.core` coverage (normative for T1174)

Every var the port implements is listed here with its conformance
state. Anything absent is out of scope, not silently missing
(spec A5). Seed state (slice S1): nothing implemented yet.

Status values: `unimplemented` | `partial` | `done` (differential
corpus pins behavior).

## Arithmetic (slice S5)

`+ - * / quot rem mod inc dec` — unimplemented.

## Comparison and predicates (slice S5)

`< <= > >= = == not not= nil? some? true? false? boolean? number?
integer? string? keyword? symbol? fn? ifn? coll? sequential? map?
vector? set? list?` — unimplemented.

## Collections (slices S3/S5)

`count empty? seq first second rest next cons conj assoc dissoc get
contains? keys vals select-keys merge merge-with zipmap vec vector
list list* hash-map hash-set sorted-map sorted-set nth last butlast
peek pop subvec update update-in assoc-in get-in` — unimplemented.

## Sequences and transducers (slice S5)

`map filter remove mapv filterv reduce reductions take drop
take-while drop-while range repeat repeatedly iterate cycle
interleave interpose concat apply comp partial juxt constantly
identity complement every? some every-pred some-fn keep keep-indexed
map-indexed distinct dedupe partition partition-all partition-by
group-by frequencies sort sort-by max min max-key min-key` —
unimplemented.

## I/O and strings (slices S4/S5)

`str print println pr prn newline read-string slurp spit format
subs keyword symbol name namespace` — unimplemented.

## Namespaces, vars, and loading (slice S5)

`gensym macroexpand macroexpand-1 eval load-file require use in-ns
ns create-ns the-ns ns-name find-ns all-ns refer binding
push-thread-bindings pop-thread-bindings with-redefs declare intern
ns-interns ns-publics resolve var-get var-set alter-var-root bound?
thread-bound?` — unimplemented.

## Control and macros (slices S4/S5)

`defn defmacro defonce comment assert doseq dotimes dorun doall for
when when-not when-let if-let if-some when-some cond condp case and
or lazy-seq` — unimplemented.

## Concurrency (slice S6)

`delay force future deref realized? add-watch remove-watch atom
swap! reset! compare-and-set! send send-off await shutdown-agents` —
unimplemented.

## REPL and environment (slice S7)

`pprint clojure-version *command-line-args* *ns* *out* *err* *in*
*print-length* *print-level* *1 *2 *3 *e` — unimplemented.
