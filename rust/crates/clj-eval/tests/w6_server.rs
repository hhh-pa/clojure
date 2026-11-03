// Copyright (c) Hao Hu. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.

//! W6: `clojure.core.server` smoke tests — start/accept/eval/respond
//! round-trips on localhost plus prepl/io-prepl/remote-prepl.

use std::io::{BufRead, BufReader, Write};
use std::net::TcpStream;
use std::time::Duration;

use clj_eval::{Evaluator, SourceError, boot};
use clj_value::{PrintOpts, pr_to_string};

fn pr(v: &clj_value::Value) -> String {
    pr_to_string(v, &PrintOpts::pr())
}

/// Evaluate and return the pr-string of the LAST top-level form.
fn run(eval: &mut Evaluator, src: &str) -> String {
    eval.eval_source(src)
        .unwrap()
        .iter()
        .map(|v| match eval.realize_deep(v) {
            Ok(r) => pr(&r),
            Err(e) => panic!("realize failed: {e:?}"),
        })
        .last()
        .unwrap()
}

/// The `Class | message` of an uncaught throw.
fn uncaught(eval: &mut Evaluator, src: &str) -> String {
    match eval.eval_source(src) {
        Err(SourceError::Uncaught(t)) => format!(
            "{} | {}",
            t.class.jvm_name(),
            t.message.as_deref().unwrap_or("")
        ),
        Err(other) => panic!("expected an uncaught throw, got {other:?}"),
        Ok(vs) => panic!("expected a throw, got values: {vs:?}"),
    }
}

fn boot_server() -> Evaluator {
    // #217: no manual installer call — `require` installs the
    // natives through the `load_one` arm (the production path).
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.server :as server])");
    eval
}

/// The eval-one accept fn (reads a line, evals it, prints the
/// result), def'd into the server ns under a fresh name per test
/// (accept-connection requires the accept ns, which no-ops for the
/// already-loaded server ns — eval-defined namespaces have no file
/// to load, exactly like upstream).
fn install_eval_one(eval: &mut Evaluator, name: &str) {
    run(
        eval,
        &format!(
            "(do (in-ns 'clojure.core.server) (defn {name} [] (let [line (__socket-read-line* *in*)] (when line (println (pr-str (eval (read-string line))))))) (in-ns 'user))"
        ),
    );
}

/// The accept symbol for a fixture installed by `install_eval_one`.
fn accept_sym(name: &str) -> String {
    format!("clojure.core.server/{name}")
}

/// Start a server and return its bound port (also `def`s the socket
/// as `srv`, for stopping).
fn start_server(eval: &mut Evaluator, name: &str, accept: &str) -> u16 {
    run(
        eval,
        &format!("(def srv (server/start-server {{:name \"{name}\" :port 0 :accept '{accept}}}))"),
    );
    let port = run(eval, "(server/__server-port* srv)");
    port.parse().expect("a numeric port")
}

/// A client with a read deadline (tests fail instead of hanging).
fn connect(port: u16) -> (TcpStream, BufReader<TcpStream>) {
    let sock = TcpStream::connect(format!("127.0.0.1:{port}")).expect("connect");
    sock.set_read_timeout(Some(Duration::from_secs(10)))
        .expect("deadline");
    let reader = BufReader::new(sock.try_clone().expect("clone"));
    (sock, reader)
}

fn write_line(sock: &mut TcpStream, line: &str) {
    sock.write_all(line.as_bytes()).expect("write");
    sock.write_all(b"\n").expect("newline");
    sock.flush().expect("flush");
}

fn read_line(reader: &mut BufReader<TcpStream>) -> String {
    let mut line = String::new();
    reader.read_line(&mut line).expect("read line");
    line
}

#[test]
fn server_ns_loads_with_api() {
    let mut eval = boot_server();
    assert_eq!(
        run(
            &mut eval,
            "[(ifn? server/start-server) (ifn? server/stop-server) (ifn? server/stop-servers) (ifn? server/start-servers) (ifn? server/repl-init) (ifn? server/repl-read) (ifn? server/repl) (ifn? server/prepl) (ifn? server/io-prepl) (ifn? server/remote-prepl)]"
        ),
        "[true true true true true true true true true true]"
    );
}

#[test]
fn start_server_validates_opts() {
    let mut eval = boot_server();
    for (opts, msg) in [
        (
            "{:port 0 :accept 'a/b}",
            "Missing required socket server property :name",
        ),
        (
            "{:name \"n\" :accept 'a/b}",
            "Missing required socket server property :port",
        ),
        (
            "{:name \"n\" :port 0}",
            "Missing required socket server property :accept",
        ),
        (
            "{:name \"n\" :port -1 :accept 'a/b}",
            "Invalid socket server port: -1",
        ),
        (
            "{:name \"n\" :port 70000 :accept 'a/b}",
            "Invalid socket server port: 70000",
        ),
    ] {
        let err = uncaught(&mut eval, &format!("(server/start-server {opts})"));
        assert!(
            err.starts_with("clojure.lang.ExceptionInfo | ") && err.contains(msg),
            "{err}"
        );
    }
}

#[test]
fn eval_respond_round_trip() {
    let mut eval = boot_server();
    install_eval_one(&mut eval, "w6-eval-one");
    let port = start_server(&mut eval, "w6-roundtrip", &accept_sym("w6-eval-one"));
    let (mut sock, mut reader) = connect(port);
    write_line(&mut sock, "(+ 1 2)");
    assert_eq!(read_line(&mut reader), "3\n");
    // A second client is served too (fresh session id).
    let (mut sock2, mut reader2) = connect(port);
    write_line(&mut sock2, "(str (ns-name *ns*))");
    assert_eq!(read_line(&mut reader2), "\"user\"\n");
    assert_eq!(
        run(&mut eval, "(server/stop-server \"w6-roundtrip\")"),
        "true"
    );
    assert_eq!(
        run(&mut eval, "(server/stop-server \"w6-roundtrip\")"),
        "nil"
    );
}

#[test]
fn sessions_tracked_per_client() {
    let mut eval = boot_server();
    install_eval_one(&mut eval, "w6-eval-two");
    let port = start_server(&mut eval, "w6-sessions", &accept_sym("w6-eval-two"));
    let mut sessions = || {
        run(
            &mut eval,
            "(get @(deref (resolve 'server/servers)) \"w6-sessions\")",
        )
    };
    let (_sock, _reader) = connect(port);
    // The accept future registers the session asynchronously.
    let mut seen = String::new();
    for _ in 0..500 {
        seen = sessions();
        if seen.contains(":sessions {\"1\" {}}") {
            break;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    assert!(seen.contains(":sessions {\"1\" {}}"), "{seen}");
    assert_eq!(
        run(&mut eval, "(server/stop-server \"w6-sessions\")"),
        "true"
    );
}

#[test]
fn stop_servers_and_session_stop() {
    let mut eval = boot_server();
    install_eval_one(&mut eval, "w6-eval-three");
    start_server(&mut eval, "w6-stop-a", &accept_sym("w6-eval-three"));
    start_server(&mut eval, "w6-stop-b", &accept_sym("w6-eval-three"));
    assert_eq!(run(&mut eval, "(server/stop-servers)"), "nil");
    // The zero-arity stop reads the server name from *session*.
    start_server(&mut eval, "w6-stop-c", &accept_sym("w6-eval-three"));
    assert_eq!(
        run(
            &mut eval,
            "(binding [server/*session* {:server \"w6-stop-c\"}] (server/stop-server))"
        ),
        "true"
    );
}

#[test]
fn start_servers_reads_props() {
    let mut eval = boot_server();
    install_eval_one(&mut eval, "w6-eval-four");
    // clojure.edn is W4's — the read-string fallback covers plain
    // server-config data until it lands.
    run(
        &mut eval,
        "(server/start-servers {\"clojure.server.w6props\" \"{:port 0 :accept clojure.core.server/w6-eval-four}\" \"other.key\" \"{:port 0}\"})",
    );
    let port: u16 = run(
        &mut eval,
        "(server/__server-port* (:socket (get @(deref (resolve 'server/servers)) \"w6props\")))",
    )
    .parse()
    .expect("a numeric port");
    let (mut sock, mut reader) = connect(port);
    write_line(&mut sock, "(* 6 7)");
    assert_eq!(read_line(&mut reader), "42\n");
    assert_eq!(run(&mut eval, "(server/stop-server \"w6props\")"), "true");
}

#[test]
fn prepl_evals_structured() {
    let mut eval = boot_server();
    // One :ret per form (ms varies, so it is projected out).
    assert_eq!(
        run(
            &mut eval,
            r#"(let [out (atom [])]
                 (with-in-str "(+ 1 2)\n:repl/quit\n"
                   (server/prepl *in* (fn [m] (swap! out conj m))))
                 (mapv (fn [m] [(:tag m) (:val m) (:ns m) (:form m)]) @out))"#
        ),
        r#"[[:ret 3 "user" "(+ 1 2)"]]"#
    );
    // *1 chains across evals inside the session.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [out (atom [])]
                 (with-in-str "5\n(* 2 *1)\n:repl/quit\n"
                   (server/prepl *in* (fn [m] (swap! out conj m))))
                 (mapv :val @out))"#
        ),
        "[5 10]"
    );
    // Multi-line forms accumulate lines.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [out (atom [])]
                 (with-in-str "(+ 1\n2)\n:repl/quit\n"
                   (server/prepl *in* (fn [m] (swap! out conj m))))
                 (mapv (fn [m] [(:val m) (:form m)]) @out))"#
        ),
        // :form keeps the raw newline; run pr-str's it, hence \\n.
        "[[3 \"(+ 1\\n2)\"]]"
    );
    // Eval output and errors tag along.
    assert_eq!(
        run(
            &mut eval,
            r#"(let [out (atom [])]
                 (with-in-str "(println :hi)\n(/ 1 0)\n)\n:repl/quit\n"
                   (server/prepl *in* (fn [m] (swap! out conj m))))
                 (mapv (fn [m] [(:tag m) (if (:exception m) (:phase (:val m)) (:val m))]) @out))"#
        ),
        "[[:out \":hi\\n\"] [:ret nil] [:ret :execution] [:ret :read-source]]",
    );
}

#[test]
fn io_prepl_serves_sockets() {
    let mut eval = boot_server();
    let port = start_server(&mut eval, "w6-io-prepl", "clojure.core.server/io-prepl");
    let (mut sock, mut reader) = connect(port);
    write_line(&mut sock, "(+ 1 2)");
    let resp = read_line(&mut reader);
    assert!(
        resp.contains(":tag :ret") && resp.contains(":val \"3\""),
        "{resp}"
    );
    // Quitting ends the session (EOF on the wire).
    write_line(&mut sock, ":repl/quit");
    assert_eq!(read_line(&mut reader), "");
    assert_eq!(
        run(&mut eval, "(server/stop-server \"w6-io-prepl\")"),
        "true"
    );
}

#[test]
fn remote_prepl_round_trip() {
    let mut eval = boot_server();
    let port = start_server(&mut eval, "w6-remote-prepl", "clojure.core.server/io-prepl");
    // The reader future lands responses asynchronously; poll for the
    // :ret on a generous budget ( sleeps via deref timeouts).
    let out = run(
        &mut eval,
        &format!(
            r#"(let [out (atom [])]
                 (with-in-str "(+ 1 2)\n:repl/quit\n"
                   (server/remote-prepl "127.0.0.1" "{port}" *in* (fn [m] (swap! out conj m))))
                 (loop [i 0]
                   (if (or (some #(= :ret (:tag %)) @out) (> i 200))
                     (mapv (fn [m] [(:tag m) (:val m)]) @out)
                     (do (deref (promise) 50 nil) (recur (inc i))))))"#
        ),
    );
    assert!(out.contains("[:ret 3]"), "{out}");
    assert_eq!(
        run(&mut eval, "(server/stop-server \"w6-remote-prepl\")"),
        "true"
    );
}

/// #217: the production path — a bare boot with no manual
/// installer call: `require` installs the natives, and one socket
/// op runs through them.
#[test]
fn require_installs_natives_without_manual_call() {
    let mut eval = boot();
    run(&mut eval, "(require '[clojure.core.server :as server])");
    assert_eq!(
        run(
            &mut eval,
            "(let [s (server/__server-socket* nil 0)] [(> (server/__server-port* s) 0) (do (server/__server-close* s) (server/__server-closed?* s))])"
        ),
        "[true true]"
    );
}
