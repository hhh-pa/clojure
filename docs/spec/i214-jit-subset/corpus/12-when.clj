;; when/when-not: branch taken/skipped, multi-body, nil results.
(print (when true :w))
(print (when false :w))
(print (when-not false :n))
(print (when-not true :n))
(print (when true 1 2 3))
