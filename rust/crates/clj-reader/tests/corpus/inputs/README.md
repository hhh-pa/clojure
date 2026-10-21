# Vendored regen inputs (T1174 Java removal)

The S2 differential corpus regenerates by running the JVM oracle
over upstream's own `.clj` sources (`driver.clj` `file-rels`). When
the JVM tree (`src/`, `test/`) was removed from this repo, the 138
enumerated files were vendored here — bit-identical to their HEAD
blobs at removal time — so `./regen.sh` keeps working and
`files.edn` `:path` rels (`src/clj/...`, `test/...`) stay
byte-stable.

- Layout mirrors the removed tree (`inputs/src/clj/...`,
  `inputs/test/...`): the driver strips the `inputs/` prefix, so
  rels are unchanged.
- Only `.clj`/`.cljc` files the driver enumerates are vendored
  (exact regen closure; e.g. `test/java/*.java` never was read).
- Do not edit: refresh from upstream sources (Maven Central
  `org.clojure/clojure` sources jar, or this repo's git history)
  if the corpus inputs ever need updating.

Byte-pinned LF via `.gitattributes` (fixture stability).
