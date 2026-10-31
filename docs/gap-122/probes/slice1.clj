;; gap-122 slice-1 upstream pins.
;; CP=clojure-1.12.6.jar:spec.alpha-0.5.238.jar:core.specs.alpha-0.4.74.jar
;; java -cp $CP clojure.main docs/gap-122/probes/slice1.clj > docs/gap-122/probes/slice1.out
(require '[clojure.spec.alpha :as s])

;; 1: predicate spec via def, valid?
(s/def ::even even?)
(println [:valid-even-4 (s/valid? ::even 4)])
(println [:valid-even-5 (s/valid? ::even 5)])

;; 2: bare predicate (no def)
(println [:valid-bare-even (s/valid? even? 4)])
(println [:valid-bare-odd (s/valid? even? 5)])

;; 3: conform / unform identity for predicates
(println [:conform-even-4 (s/conform ::even 4)])
(println [:conform-even-5 (s/conform ::even 5)])
(println [:unform-even-4 (s/unform ::even 4)])
(println [:invalid?-conformed-5 (s/invalid? (s/conform ::even 5))])

;; 4: set as predicate
(s/def ::small #{1 2 3})
(println [:valid-set-2 (s/valid? ::small 2)])
(println [:valid-set-9 (s/valid? ::small 9)])

;; 5: anonymous predicate
(s/def ::big #(> % 100))
(println [:valid-big-101 (s/valid? ::big 101)])
(println [:valid-big-99 (s/valid? ::big 99)])

;; 6: s/and
(s/def ::even-small (s/and even? #(< % 10)))
(println [:valid-and-4 (s/valid? ::even-small 4)])
(println [:valid-and-5 (s/valid? ::even-small 5)])
(println [:valid-and-42 (s/valid? ::even-small 42)])
(println [:conform-and-4 (s/conform ::even-small 4)])
(println [:conform-and-5 (s/conform ::even-small 5)])
(println [:unform-and-4 (s/unform ::even-small 4)])

;; 7: explain-str outputs (exact format)
(println :explain-even-5-begin)
(print (s/explain-str ::even 5))
(println :explain-even-5-end)
(println :explain-and-5-begin)
(print (s/explain-str ::even-small 5))
(println :explain-and-5-end)
(println :explain-and-42-begin)
(print (s/explain-str ::even-small 42))
(println :explain-and-42-end)
(println :explain-bare-begin)
(print (s/explain-str even? 5))
(println :explain-bare-end)
(println :explain-success-begin)
(print (s/explain-str ::even 4))
(println :explain-success-end)

;; 8: explain-data shape
(println [:explain-data (s/explain-data ::even 5)])
(println [:explain-data-ok (s/explain-data ::even 4)])

;; 9: registry / get-spec / spec?
(println [:registry-has-even (contains? (s/registry) ::even)])
(println [:get-spec-even (some? (s/get-spec ::even))])
(println [:get-spec-missing (s/get-spec ::nope)])
(println [:spec?-of-spec (some? (s/spec? (s/get-spec ::even)))])
(println [:spec?-of-fn (s/spec? even?)])
(println [:spec?-of-kw (s/spec? ::even)])

;; 10: def returns k; nil removes
(println [:def-returns (s/def ::tmp string?)])
(println [:def-then-valid (s/valid? ::tmp "a")])
(s/def ::tmp nil)
(println [:def-nil-removed (s/get-spec ::tmp)])

;; 11: unresolvable spec errors
(println [:valid-missing
          (try (s/valid? ::definitely-missing 1)
               (catch Exception e (str "THREW:" (.getMessage e))))])
(println [:conform-missing
          (try (s/conform ::definitely-missing 1)
               (catch Exception e (str "THREW:" (.getMessage e))))])

;; 12: def with non-namespaced key fails
(println [:def-bare-kw
          (try (s/def :bare string?)
               (catch Throwable e (str "THREW:" (.getMessage e))))])

;; 13: form / describe
(println [:form-even (s/form ::even)])
(println [:describe-even (s/describe ::even)])
(println [:form-and (s/form ::even-small)])
(println [:describe-and (s/describe ::even-small)])

;; 14: explain *explain-out* binding: explain prints, returns nil
(println [:explain-returns (s/explain ::even 5)])
