# Core coverage (A5, normative)

Every `clojure.core` var the port implements, with its conformance
state. Anything absent is out of scope, not silently missing; the
explicit `out-of-scope` rows below record the significant exclusions
with their reasons.

Status: `planned` (lands in S5) → `implemented` (resolved + pinned) |
`divergent` (resolves, fenced behavior) | `out-of-scope` (must not
resolve). The S5 coverage-matrix test (`clj-core/tests/`) parses this
file: `planned`/`implemented`/`divergent` rows must resolve in the
booted core, `out-of-scope` rows must not. Table shape is
machine-read — keep `| var | status | notes |` rows exact.

## Namespaces (S5A)

| var | status | notes |
| `ns` | planned | macro over the loader |
| `in-ns` | planned | |
| `create-ns` | planned | |
| `remove-ns` | planned | |
| `find-ns` | planned | |
| `all-ns` | planned | |
| `the-ns` | planned | |
| `ns-name` | planned | |
| `ns-aliases` | planned | |
| `ns-resolve` | planned | |
| `resolve` | planned | |
| `alias` | planned | |
| `refer` | planned | |
| `use` | planned | |
| `require` | planned | embedded + `*load-path*` files |
| `refer-clojure` | planned | |
| `intern` | planned | |
| `declare` | planned | forward declaration |
| `ns-interns` | planned | |
| `ns-publics` | planned | |
| `ns-refers` | planned | |
| `load` | planned | file by path over `*load-path*` |
| `load-string` | planned | |
| `load-file` | planned | |
| `loaded-libs` | planned | |
| `*ns*` | planned | |
| `*file*` | planned | bound while loading |
| `*loaded-libs*` | planned | |
| `*load-path*` | planned | port extension: vector of dirs |

## Vars (S5B)

| var | status | notes |
| `alter-var-root` | planned | atomic root swap |
| `with-redefs` | planned | scoped root rebinding |
| `bound?` | planned | |
| `thread-bound?` | planned | |
| `var-get` | planned | |
| `var-set` | planned | |
| `push-thread-bindings` | planned | |
| `pop-thread-bindings` | planned | |
| `clone-thread-bindings` | planned | |
| `get-thread-bindings` | planned | |
| `with-bindings*` | planned | |
| `with-bindings` | planned | macro |
| `bound-fn` | planned | conveyance primitive (S6 builds on it) |
| `meta` | planned | |
| `with-meta` | planned | |
| `vary-meta` | planned | |
| `alter-meta!` | planned | |
| `reset-meta!` | planned | |

## Macros (S5C)

| var | status | notes |
| `defmacro` | planned | mechanism TBD in S5C |
| `macroexpand-1` | planned | |
| `macroexpand` | planned | |
| `gensym` | planned | Rust-backed counter |

## Arithmetic (S5D, checked per C1)

| var | status | notes |
| `+` | planned | variadic |
| `-` | planned | variadic |
| `*` | planned | variadic |
| `/` | planned | exact-if-divisible else double (C1) |
| `quot` | planned | |
| `rem` | planned | |
| `mod` | planned | |
| `inc` | planned | |
| `dec` | planned | |
| `max` | planned | variadic |
| `min` | planned | variadic |

## Comparison (S5D, per C2)

| var | status | notes |
| `=` | planned | same-category (C2) |
| `==` | planned | cross-category numeric |
| `not=` | planned | |
| `<` | planned | variadic |
| `>` | planned | variadic |
| `<=` | planned | variadic |
| `>=` | planned | variadic |
| `zero?` | planned | |
| `pos?` | planned | |
| `neg?` | planned | |
| `compare` | planned | JVM compare semantics |

## Predicates (S5D)

| var | status | notes |
| `nil?` | planned | |
| `not` | planned | |
| `some?` | planned | |
| `true?` | planned | |
| `false?` | planned | |
| `boolean?` | planned | |
| `integer?` | planned | |
| `int?` | planned | |
| `float?` | planned | |
| `double?` | planned | |
| `number?` | planned | |
| `string?` | planned | |
| `char?` | planned | |
| `keyword?` | planned | |
| `symbol?` | planned | |
| `ident?` | planned | |
| `qualified-ident?` | planned | |
| `simple-ident?` | planned | |
| `qualified-keyword?` | planned | |
| `simple-keyword?` | planned | |
| `qualified-symbol?` | planned | |
| `simple-symbol?` | planned | |
| `coll?` | planned | |
| `sequential?` | planned | |
| `associative?` | planned | |
| `counted?` | planned | |
| `list?` | planned | |
| `vector?` | planned | |
| `map?` | planned | |
| `set?` | planned | |
| `fn?` | planned | |
| `ifn?` | planned | |
| `var?` | planned | |
| `seq?` | planned | |
| `seqable?` | planned | |
| `record?` | planned | always false (no records exist) |

## Strings (S5D)

| var | status | notes |
| `str` | planned | variadic |
| `keyword` | planned | |
| `symbol` | planned | |
| `name` | planned | |
| `namespace` | planned | |
| `subs` | planned | |
| `pr-str` | planned | |
| `print-str` | planned | |
| `format` | planned | Formatter subset, oracle-pinned |

## Collections (S5D)

| var | status | notes |
| `count` | planned | |
| `empty?` | planned | |
| `not-empty` | planned | |
| `seq` | planned | nil-punning |
| `first` | planned | |
| `second` | planned | |
| `last` | planned | |
| `rest` | planned | |
| `next` | planned | |
| `nnext` | planned | |
| `fnext` | planned | |
| `nfirst` | planned | |
| `cons` | planned | |
| `conj` | planned | |
| `assoc` | planned | |
| `dissoc` | planned | |
| `get` | planned | |
| `contains?` | planned | |
| `find` | planned | |
| `keys` | planned | |
| `vals` | planned | |
| `merge` | planned | variadic |
| `merge-with` | planned | |
| `select-keys` | planned | |
| `vec` | planned | |
| `vector` | planned | |
| `list` | planned | |
| `list*` | planned | |
| `nth` | planned | |
| `update` | planned | |
| `update-in` | planned | |
| `assoc-in` | planned | |
| `get-in` | planned | |
| `zipmap` | planned | |
| `array-map` | planned | |
| `hash-map` | planned | |
| `hash-set` | planned | |
| `transient` | planned | over S3 transients |
| `persistent!` | planned | |
| `conj!` | planned | |
| `assoc!` | planned | |
| `dissoc!` | planned | |
| `pop!` | planned | |
| `peek` | planned | |
| `pop` | planned | |
| `subvec` | planned | |
| `replace` | planned | |
| `into` | divergent | plain into only; xform arity out of scope |
| `distinct?` | planned | |

## Seqs (S5D, chunked-lazy per C4)

| var | status | notes |
| `map` | planned | seq arities; xform arity out of scope |
| `filter` | planned | seq arities; xform arity out of scope |
| `remove` | planned | seq arities; xform arity out of scope |
| `reduce` | planned | no xform path |
| `take` | planned | |
| `drop` | planned | |
| `take-while` | planned | |
| `drop-while` | planned | |
| `take-last` | planned | |
| `drop-last` | planned | |
| `split-at` | planned | |
| `split-with` | planned | |
| `concat` | planned | variadic, lazy |
| `cycle` | planned | |
| `interleave` | planned | |
| `interpose` | planned | |
| `repeat` | planned | |
| `repeatedly` | planned | |
| `iterate` | planned | |
| `range` | planned | |
| `reverse` | planned | |
| `sort` | planned | |
| `sort-by` | planned | |
| `mapcat` | planned | |
| `keep` | planned | seq arities; xform arity out of scope |
| `keep-indexed` | planned | seq arities; xform arity out of scope |
| `map-indexed` | planned | |
| `distinct` | planned | seq arities; xform arity out of scope |
| `dedupe` | planned | seq arities; xform arity out of scope |
| `partition` | planned | |
| `partition-all` | planned | |
| `partition-by` | planned | |
| `take-nth` | planned | |
| `butlast` | planned | |
| `dorun` | planned | |
| `doall` | planned | |
| `mapv` | planned | eager |
| `filterv` | planned | eager |
| `reductions` | planned | |
| `every-pred` | planned | |
| `some-fn` | planned | |
| `group-by` | planned | |
| `frequencies` | planned | |
| `max-key` | planned | |
| `min-key` | planned | |

## Higher-order (S5D)

| var | status | notes |
| `apply` | planned | |
| `comp` | planned | variadic; fn composition (not xform comp) |
| `partial` | planned | |
| `complement` | planned | |
| `juxt` | planned | |
| `constantly` | planned | |
| `identity` | planned | |
| `fnil` | planned | |
| `every?` | planned | |
| `some` | planned | |
| `not-any?` | planned | |
| `not-every?` | planned | |

## Control (S5D, embedded `.clj` composites)

| var | status | notes |
| `and` | planned | macro |
| `or` | planned | macro |
| `when` | planned | macro |
| `when-not` | planned | macro |
| `if-not` | planned | macro |
| `if-let` | planned | macro |
| `when-let` | planned | macro |
| `when-some` | planned | macro |
| `if-some` | planned | macro |
| `cond` | planned | macro |
| `condp` | planned | macro |
| `case` | planned | macro |
| `->` | planned | macro |
| `->>` | planned | macro |
| `as->` | planned | macro |
| `cond->` | planned | macro |
| `cond->>` | planned | macro |
| `doto` | planned | macro; fn calls only (methods are N1) |
| `comment` | planned | macro |
| `defn` | planned | macro |
| `defn-` | planned | macro |
| `defonce` | planned | macro |
| `dotimes` | planned | macro |
| `doseq` | planned | macro |
| `for` | planned | macro |
| `lazy-seq` | planned | macro |
| `lazy-cat` | planned | macro |
| `assert` | planned | macro; honors `*assert*` |
| `time` | planned | macro |
| `with-out-str` | planned | macro |

## Exceptions (S5D)

| var | status | notes |
| `ex-info` | planned | |
| `ex-data` | planned | |
| `ex-message` | planned | |
| `ex-cause` | planned | |

## IO (S5D)

| var | status | notes |
| `print` | planned | honors `*out*` |
| `println` | planned | honors `*out*` |
| `pr` | planned | honors `*out*` |
| `prn` | planned | honors `*out*` |
| `newline` | planned | honors `*out*` |
| `flush` | planned | |
| `*out*` | planned | |
| `*err*` | planned | |
| `slurp` | planned | |
| `spit` | planned | |
| `read-string` | planned | over the S2 reader |
| `*print-length*` | planned | honored per C3 |
| `*print-level*` | planned | honored per C3 |
| `*print-dup*` | divergent | declared; ignored by the printer |
| `*print-meta*` | divergent | declared; ignored by the printer |
| `*print-readably*` | divergent | declared; ignored by the printer |

## Misc (S5D)

| var | status | notes |
| `eval` | planned | eval one form in the current ns |
| `trampoline` | planned | |
| `*assert*` | planned | default true |
| `*unchecked-math*` | planned | fixed false (always checked, C1) |
| `*warn-on-reflection*` | planned | fixed false (no interop, N1) |
| `*allow-unresolved-vars*` | planned | fixed false (always strict) |

## Special-form macros (S4 builtins, S5C vars)

| var | status | notes |
| `let` | divergent | analyzer builtin; resolves as macro; `macroexpand-1` returns the form unchanged (documented) |
| `loop` | divergent | analyzer builtin; resolves as macro; `macroexpand-1` returns the form unchanged (documented) |
| `fn` | divergent | analyzer builtin; resolves as macro; `macroexpand-1` returns the form unchanged (documented) |
| `binding` | divergent | analyzer builtin; resolves as macro; `macroexpand-1` returns the form unchanged (documented) |
| `letfn` | divergent | analyzer builtin; resolves as macro; `macroexpand-1` returns the form unchanged (documented) |

## Out of scope (significant exclusions)

| var | status | notes |
| `sequence` | out-of-scope | transducers: no slice |
| `transduce` | out-of-scope | transducers: no slice |
| `eduction` | out-of-scope | transducers: no slice |
| `completing` | out-of-scope | transducers: no slice |
| `ensure-reduced` | out-of-scope | transducers: no slice |
| `unreduced` | out-of-scope | transducers: no slice |
| `preserving-reduced` | out-of-scope | transducers: no slice |
| `halt-when` | out-of-scope | transducers: no slice |
| `sorted-map` | out-of-scope | sorted collections: no slice |
| `sorted-set` | out-of-scope | sorted collections: no slice |
| `sorted-map-by` | out-of-scope | sorted collections: no slice |
| `sorted-set-by` | out-of-scope | sorted collections: no slice |
| `subseq` | out-of-scope | sorted collections: no slice |
| `rsubseq` | out-of-scope | sorted collections: no slice |
| `rand` | out-of-scope | RNG: no slice |
| `rand-int` | out-of-scope | RNG: no slice |
| `shuffle` | out-of-scope | RNG: no slice |
| `instance?` | out-of-scope | interop (N1) |
| `type` | out-of-scope | class-based; no slice |
| `class` | out-of-scope | interop (N1) |
| `bases` | out-of-scope | interop (N1) |
| `supers` | out-of-scope | interop (N1) |
| `defmulti` | out-of-scope | multimethods: no slice |
| `defmethod` | out-of-scope | multimethods: no slice |
| `prefer-method` | out-of-scope | multimethods: no slice |
| `get-method` | out-of-scope | multimethods: no slice |
| `methods` | out-of-scope | multimethods: no slice |
| `remove-method` | out-of-scope | multimethods: no slice |
| `with-in-str` | out-of-scope | needs `*in*` machinery: no slice |
| `read` | out-of-scope | needs `*in*` machinery: no slice |
| `read-line` | out-of-scope | needs `*in*` machinery: no slice |
| `line-seq` | out-of-scope | needs `*in*` machinery: no slice |
| `*in*` | out-of-scope | needs `*in*` machinery: no slice |
| `deftype` | out-of-scope | no type-system slice |
| `defrecord` | out-of-scope | no type-system slice |
| `reify` | out-of-scope | no type-system slice |
| `proxy` | out-of-scope | no type-system slice |
| `gen-class` | out-of-scope | no type-system slice |
| `gen-interface` | out-of-scope | no type-system slice |
| `definterface` | out-of-scope | no type-system slice |
| `import` | out-of-scope | interop (N1) |
| `..` | out-of-scope | interop (N1) |
| `with-open` | out-of-scope | needs Closeable/protocol: no slice |
| `extend` | out-of-scope | protocols: no slice |
| `extend-type` | out-of-scope | protocols: no slice |
| `extend-protocol` | out-of-scope | protocols: no slice |
| `satisfies?` | out-of-scope | protocols: no slice |
| `extends?` | out-of-scope | protocols: no slice |
| `atom` | out-of-scope | S6 concurrency |
| `swap!` | out-of-scope | S6 concurrency |
| `reset!` | out-of-scope | S6 concurrency |
| `compare-and-set!` | out-of-scope | S6 concurrency |
| `shutdown-agents` | out-of-scope | S6 concurrency |
| `delay` | out-of-scope | S6 concurrency |
| `force` | out-of-scope | S6 concurrency |
| `deref` | out-of-scope | S6 concurrency |
| `future` | out-of-scope | S6 concurrency |
| `future-call` | out-of-scope | S6 concurrency |
| `future?` | out-of-scope | S6 concurrency |
| `future-cancel` | out-of-scope | S6 concurrency |
| `future-done?` | out-of-scope | S6 concurrency |
| `promise` | out-of-scope | S6 concurrency |
| `deliver` | out-of-scope | S6 concurrency |
| `realized?` | out-of-scope | S6 concurrency |
| `send` | out-of-scope | S6 concurrency |
| `send-off` | out-of-scope | S6 concurrency |
| `send-via` | out-of-scope | S6 concurrency |
| `await` | out-of-scope | S6 concurrency |
| `await-for` | out-of-scope | S6 concurrency |
| `agent` | out-of-scope | S6 concurrency |
| `agent-error` | out-of-scope | S6 concurrency |
| `restart-agent` | out-of-scope | S6 concurrency |
| `set-error-handler!` | out-of-scope | S6 concurrency |
| `set-error-mode!` | out-of-scope | S6 concurrency |
| `add-watch` | out-of-scope | S6 concurrency |
| `remove-watch` | out-of-scope | S6 concurrency |
| `locking` | out-of-scope | S6 concurrency |
| `pvalues` | out-of-scope | S6 concurrency |
| `pcalls` | out-of-scope | S6 concurrency |
| `pmap` | out-of-scope | S6 concurrency |
| `memoize` | out-of-scope | needs atom: S6 |
| `ref` | out-of-scope | STM (N4): never |
| `dosync` | out-of-scope | STM (N4): never |
| `commute` | out-of-scope | STM (N4): never |
| `ensure` | out-of-scope | STM (N4): never |
| `alter` | out-of-scope | STM (N4): never |
| `ref-set` | out-of-scope | STM (N4): never |
| `*1` | out-of-scope | S7 REPL |
| `*2` | out-of-scope | S7 REPL |
| `*3` | out-of-scope | S7 REPL |
| `*e` | out-of-scope | S7 REPL |
| `*command-line-args*` | out-of-scope | S7 CLI |
| `clojure-version` | out-of-scope | S7 environment |
| `*clojure-version*` | out-of-scope | S7 environment |
