# #120 sub-slice 4: `clojure.reflect` facsimile

Owner direction (see [#120](https://github.com/hhh-pa/clojure/issues/120)):
port-native solutions only — no JDK/JVM at runtime, ever. So
`clojure.reflect` cannot shell out to `java.lang.reflect` or ASM.
This slice ships a facsimile: upstream `type-reflect`/`reflect` map
shapes over the emulated class lattice.

Oracle: upstream `clojure/reflect.clj` + `clojure/reflect/java.clj`
source text (read, never executed). Member *shapes* match upstream
exactly; member *coverage* is the port's own (below).

## What lands

- `rust/crates/clj-eval/src/reflect.clj` (new, embedded): the
  `clojure.reflect` namespace — `Reflector`/`TypeReference`
  protocols, `TypeReference` extensions for `Symbol`/`Class`,
  upstream-verbatim `flag-descriptors`/`parse-flags`, plain-map
  members (see below), the `EmulatedReflector` `deftype` + private
  `default-reflector`, upstream-verbatim `type-reflect` (incl. the
  `:ancestors` walk and `:reflector` option) and `reflect`.
- `rust/crates/clj-eval/src/reflect.rs` (new): the `__reflect*` and
  `__reflect_typename*` hidden natives. `__reflect*` resolves the
  typeref through the shared `resolve_class_name` (so `reflect`,
  `import`, and `Class/forName` agree on every name, including
  `HostDef` rows) and returns `:bases` (nil when empty, like
  upstream's `not-empty`), `:flags-raw`, and `:members` (plain maps;
  the `.clj` side swaps `:flags-raw` for the parsed `:flags` so the
  single upstream `parse-flags` does every flags conversion).
- The emulated member table (`CLASS_MEMBERS` in `reflect.rs`): one
  row per lattice class with emulated members. Coverage rule: every
  reported member is invokable/constructible through the port, and
  nothing else is reported.
- `rust/crates/clj-eval/tests/reflect.rs` (new, 20 tests): every
  member pinned, both entry points, the `:ancestors` walk, the
  `:reflector` option, `typename` shapes + misses, error rows, a
  `HostDef` row, `parse-flags`, and the hidden natives direct
  (every arm).
- Loader row + `REFLECT_HIDDEN` chain in `ns.rs`, `pub mod reflect`
  in `lib.rs`.

## Fidelity notes

- Members are exactly the emulated surface: the `..` members
  (`SURFACE_MEMBERS` in `classemu.rs`), the `Class/forName`
  overloads (`static_member_arities`), and the `String`/`Object`
  ctors (`ctor_arities`, with the port-constructible signatures from
  `ctor.rs`). Unemulated JVM members (`Object.wait`,
  `String.length`, …) are absent; there are no fields (the port
  exposes none); `:exception-types` is always empty. New emulated
  members (e.g. the parallel string-members / system-statics slices)
  must add a row here plus a pin in `tests/reflect.rs`.
- Modifiers are emulated: classes read `public` (+`abstract` and
  `interface` on interfaces, like `javac` output); members add
  `abstract` on interface methods and `static` on statics. Ctor
  names render as the binary name and parse in the `:method`
  context, both as upstream.
- `TypeReference` extends `Symbol` and `Class` only: upstream also
  extends the ASM `Type` (no port equivalent), and strings stay
  unextended, as upstream. Non-typerefs fail with the port's
  protocol-miss `IllegalArgumentException`, through `(typename
  typeref)` in `do-reflect` — mirroring upstream's
  `typeref->class`.
- Unknown names are `ClassNotFoundException` carrying the bare name
  (the `import`/`forName` shape). Array spellings (`byte<>`)
  normalize through `typename`, then miss like `forName`.
- `clojure.set`/`clojure.string` are not embedded, so the one set
  operation is a local `set-difference`, symbol `<>`/`[]` surgery
  lives in the `__reflect_typename*` native, and there is no
  `typesym` (the native builds the symbols). No `datafy` metadata
  (no `clojure.datafy` in the port). No `JavaReflector`/
  `AsmReflector` (names that would lie about a JVM); the
  `EmulatedReflector`'s `classloader` field is always nil.
- Members are plain maps, not the upstream `Constructor`/`Method`/
  `Field` records: port records do not `=` maps, so records would
  break map-shape equality (and the port cannot `instance?` record
  classes anyway). `default-reflector` stays private, as upstream.

## Gates (local, this session)

- `cargo fmt --all -- --check`: clean.
- `cargo clippy --locked -p clj-eval --all-targets -- -D warnings`: exit 0.
- New suite `reflect`: 20/20.
- Complexity: no new violations (table + small helpers).
- Coverage: every new Rust line exercised (each helper arm has a test above).
