# #102 oracle probes: `compare` CCE text (upstream source)

No JDK runs here — the oracle is upstream source text
(`clojure/clojure` master + the JDK `compareTo` internals it
calls into), read 2026-10-30. Each result is pinned by a test
in `rust/crates/clj-value/src/compare.rs` (unit), its
`tests/compare.rs`, or `rust/crates/clj-eval/tests/arith.rs`
(the `compare` native, which routes through `jvm_compare` and
already threw these texts — #102 extends them to
`clj_value::compare`).

## P1 — `Util.compare` (the dispatch)

`src/jvm/clojure/lang/Util.java` @ master:

```java
static public int compare(Object k1, Object k2){
	if(k1 == k2)
		return 0;
	if(k1 != null)
		{
		if(k2 == null)
			return 1;
		if(k1 instanceof Number)
			return Numbers.compare((Number) k1, (Number) k2);
		return ((Comparable) k1).compareTo(k2);
		}
	return -1;
}
```

So a numeric `k1` against a non-numeric `k2` checkcasts `k2`
to `Number` (CC `String → Number`), and a non-`Comparable`
`k1` fails itself against `Comparable` — no per-class
`compareTo` runs in either case.

Pins: `numeric_k1_fails_k2_to_number` (all six tower kinds),
`cells_fail_against_comparable`, `lists_fail_against_comparable`,
`plain_extensions_fail_against_comparable`,
`classes_are_never_comparable`, `reader_forms_fail_against_comparable`,
`multifn_fails_against_comparable`,
`host_values_fail_against_comparable`,
`core_r4_compare_nils` (`(compare 1 "s")` → `String → Number`).

## P2 — `Symbol.compareTo` / `Keyword.compareTo`

`Symbol.java` @ master:

```java
public int compareTo(Object o){
	Symbol s = (Symbol) o;
	...
}
```

`Keyword.java` @ master:

```java
public int compareTo(Object o){
	return sym.compareTo(((Keyword) o).sym);
}
```

Alien `k2` fails to `clojure.lang.Symbol` / `clojure.lang.Keyword`
respectively.

Pins: `comparable_nameds_fail_k2_to_own_class`,
`cross_type_compares_fail_with_cce_text` (`:a` vs `'a`),
`core_r4_compare_casts` (`(compare :a 1)`, `(compare 'a 1)`).

## P3 — `APersistentVector.compareTo`

`APersistentVector.java` @ master:

```java
public int compareTo(Object o){
	IPersistentVector v = (IPersistentVector) o;
	...
}
```

Vectors (and map entries, which compare as their 2-vector)
fail an alien `k2` to `clojure.lang.IPersistentVector`.

Pins: `vector_family_fails_k2_to_ipersistentvector`,
`lists_fail_against_comparable` (vector-vs-list leg),
`map_entry_cross_type_compares_fail_without_panicking`,
`core_r4_compare_casts` (`(compare [1] 1)`).

## P4 — JDK `compareTo` internals

`String.compareTo(String)`, `Character.compareTo(Character)`,
`Boolean.compareTo(Boolean)`, `UUID.compareTo(UUID)`,
`Date.compareTo(Date)`, `URI.compareTo(URI)` each take their
own class, so the bridge checkcast fails alien `k2` to
`k1`'s class.

Pins: `comparable_scalars_fail_k2_to_own_class`,
`cross_type_compares_fail_with_cce_text` (`(1, true)`,
`(true, 1)`, `("a", :a)` legs), `core_r4_compare_casts`
(`(compare "a" 1)`, `(compare \a 1)`, `(compare true 1)`).

## P5 — module verbiage

`java.*` (and array descriptors) live in
`module java.base of loader 'bootstrap'`; everything else is
in `unnamed module of loader 'app'`. Same-module pairs
combine (`A and B are in M`); cross-module pairs split
(`A is in M1; B is in M2`). Rendered by the shared
`cce_message` (byte-pinned by probe39); every pin above
asserts the full text including the clause.
