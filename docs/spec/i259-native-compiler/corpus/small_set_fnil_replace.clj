;; Slice 5q: SmallSet (<= 8 inline unsorted & sorted, promotion > 8, disj,
;; equality, hashing, meta), owned fnil preserving update uniqueness, and
;; fast clojure.string/replace native.
(ns small-set-fnil-replace
  (:require [clojure.string :as str]))

;; SmallSet unsorted: literal, conj, dup conj, disj, empty, promote > 8, compact
(let [s0 #{}
      s3 #{1 2 3}
      s4 (conj s3 4)
      s4d (conj s4 2)
      s2 (disj s4 2 99)
      s9 (reduce conj #{} (range 10))
      s9d (disj s9 0 1 2 3 4 5 6 7)]
  (println (count s0) (empty? s0) (contains? s3 2) (get s3 2) (get s3 9)
           (= s4 s4d) (count s2) (contains? s2 2) (contains? s2 4)
           (count s9) (contains? s9 9) (get s9 5) (count s9d) (contains? s9d 8)
           (= #{1 2 3} (conj #{3 1} 2))
           (= (hash #{1 2 3}) (hash (conj #{3 1} 2)))))

;; SmallSet sorted: <= 8 and promotion > 8, disj interior & head
(let [ss (apply sorted-set [5 1 3 2 4])
      ss2 (conj ss 0 6 7 8 9)
      ss3 (disj ss2 0 5 99)]
  (println (vec ss) (vec (disj ss 3)) (vec ss2) (vec ss3)))

;; Owned fnil with update (preserves unique SmallSet & Vector ownership)
(let [m (reduce (fn [acc [k v]] (update acc k (fnil conj #{}) v))
                {}
                [[:a 1] [:a 2] [:a 3] [:b 10] [:a 2] [:b 20]])
      mv (reduce (fn [acc [k v]] (update acc k (fnil conj []) v))
                 {}
                 [[:x 1] [:x 2] [:y 3]])
      f2 (fnil + 10 20)]
  (println (count (:a m)) (contains? (:a m) 2) (count (:b m))
           (:x mv) (:y mv)
           (f2 nil nil) (f2 1 nil) (f2 nil 2) (f2 3 4)))

;; Fast clojure.string/replace (anchored ^c+, c+$, general regex, char, str, fn, errors)
(println
 (str/replace "000123" #"^0+" "")
 (str/replace "123" #"^0+" "")
 (str/replace "000" #"^0+" "")
 (str/replace "000123" #"^0+" "X")
 (str/replace "123000" #"0+$" "")
 (str/replace "123" #"0+$" "")
 (str/replace "123000" #"0+$" "Y")
 (str/replace "a1b2c" #"\d" "N")
 (str/replace "abc" #"\d" "N")
 (str/replace "banana" \a \o)
 (str/replace "xyz" \a \o)
 (str/replace "foo-bar-foo" "foo" "baz")
 (str/replace "xyz" "foo" "baz")
 (str/replace "a1b2" #"\d" #(str "[" % "]"))
 (str/replace "abc" #"\d" #(str "[" % "]")))
(println (try (str/replace "abc" 42 "x") (catch IllegalArgumentException _ :bad-match)))
(println (try (str/replace nil "a" "b") (catch NullPointerException _ :nil-str)))
