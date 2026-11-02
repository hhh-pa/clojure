; Fixture: `run` preloads `clojure.string` (JVM `clojure.main`
; requires it), so qualified vars resolve with no `require`.
(println (clojure.string/join "," ["a" "b"]))
