(print (try (throw (ex-info "x" {})) (catch Exception e :c)))
