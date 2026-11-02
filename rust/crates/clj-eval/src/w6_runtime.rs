// Copyright (c) Rich Hickey. All rights reserved.
// The use and distribution terms for this software are covered by the
// Eclipse Public License 1.0 (http://opensource.org/licenses/eclipse-1.0.php)
// which can be found in the file epl-v10.html at the root of this distribution.
// By using this software in any fashion, you are agreeing to be bound by
// the terms of this license.
// You must not remove this notice, or any other, from this software.

//! W6 runtime-ish substrate (stdlib-all #204): the socket-server and
//! XML-scan natives behind the `clojure.core.server` and `clojure.xml`
//! ports. Sockets are std `TcpListener`/`TcpStream` (accept polls so
//! `stop-server` always unblocks it); XML is a small hand-rolled
//! scanner (no new crates) that never resolves external entities, so
//! both `startparse` variants are XXE-immune.
//!
//! Wiring: the integrator adds `load_one` arms calling
//! [`install_server_natives`]/[`install_xml_natives`] (the gap-123
//! `install_async_ns` shape); until then tests call them directly,
//! which is why they are `pub`.

use std::io::{BufRead, BufReader, Write};
use std::net::{Shutdown, TcpListener, TcpStream};
use std::sync::{
    Arc, Mutex,
    atomic::{AtomicBool, Ordering},
};
use std::time::{Duration, Instant};

use clj_value::{ExtensionValue, ThrowClass, Value, Vector};

use crate::core::cce_err;
use crate::error::NativeError;
use crate::eval::Evaluator;
use crate::ns::{NativeFnPtr, check_arity};

/// The socket-server namespace (a ported source file plus these
/// hidden natives).
pub(crate) const SERVER_NS: &str = "clojure.core.server";

/// The XML namespace (a ported source file plus [`native_xml_scan`]).
pub(crate) const XML_NS: &str = "clojure.xml";

/// How long one accept poll sleeps on an idle listener (so
/// `stop-server` lands within milliseconds, like closing a
/// `ServerSocket` unblocks `accept`).
const ACCEPT_POLL: Duration = Duration::from_millis(20);

/// An IO failure as an `IOException` carrying the OS message (shared
/// shape — `SocketException`/`UnknownHostException` extend it, so
/// the class stays put while the message varies).
fn io_err(e: std::io::Error) -> NativeError {
    NativeError::new(ThrowClass::IOException, e.to_string())
}

/// A bound server socket (the `ServerSocket` the port's
/// `start-server` returns): a nonblocking listener plus the closed
/// latch. `close` drops the listener, so the OS port frees even
/// while Clojure values still reference the handle (like the JDK).
#[derive(Debug)]
pub(crate) struct ServerSock {
    id: u64,
    inner: Arc<ServerInner>,
}

/// The shared server state: the listener (taken on close) and the
/// cached local port (which answers after close, like
/// `getLocalPort`).
#[derive(Debug)]
struct ServerInner {
    listener: Mutex<Option<TcpListener>>,
    closed: AtomicBool,
    local_port: u16,
}

impl ExtensionValue for ServerSock {
    fn class_name(&self) -> String {
        "java.net.ServerSocket".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.net.ServerSocket 0x{:x} \"java.net.ServerSocket@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<ServerSock>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// A connected socket (accepted or dialed): one reader plus one
/// writer half over two clones. `close` shuts the socket down,
/// which unblocks a pending read with EOF (like the JDK).
#[derive(Debug)]
pub(crate) struct SocketConn {
    id: u64,
    state: Arc<ConnState>,
}

/// The shared connection state.
#[derive(Debug)]
struct ConnState {
    reader: Mutex<BufReader<TcpStream>>,
    writer: Mutex<TcpStream>,
    closed: AtomicBool,
}

impl ExtensionValue for SocketConn {
    fn class_name(&self) -> String {
        "java.net.Socket".to_owned()
    }

    fn print(&self) -> String {
        format!(
            "#object[java.net.Socket 0x{:x} \"java.net.Socket@{:x}\"]",
            self.id, self.id
        )
    }

    fn dyn_eq(&self, other: &dyn ExtensionValue) -> bool {
        other
            .as_any()
            .downcast_ref::<SocketConn>()
            .is_some_and(|o| o.id == self.id)
    }

    fn dyn_hash(&self, state: &mut dyn std::hash::Hasher) {
        state.write_u64(self.id);
    }

    fn as_any(&self) -> &dyn std::any::Any {
        self
    }
}

/// Downcast a value to a server socket, if it is one.
fn as_server(v: &Value) -> Option<&ServerSock> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<ServerSock>(),
        _ => None,
    }
}

/// Downcast a value to a connection, if it is one.
fn as_conn(v: &Value) -> Option<&SocketConn> {
    match v {
        Value::Extension(e) => e.as_any().downcast_ref::<SocketConn>(),
        _ => None,
    }
}

/// A server socket or the `ServerSocket` CCE.
fn need_server(v: &Value) -> Result<&ServerSock, NativeError> {
    as_server(v).ok_or_else(|| cce_err(v, "java.net.ServerSocket"))
}

/// A connection or the `Socket` CCE.
fn need_conn(v: &Value) -> Result<&SocketConn, NativeError> {
    as_conn(v).ok_or_else(|| cce_err(v, "java.net.Socket"))
}

/// Lock a mutex, tolerating poisoning (like `Writer`'s buffer).
fn lock<T>(m: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

/// A port argument: an int in range (the Clojure side validates
/// first, so this only fires on direct native calls).
fn need_port(v: &Value) -> Result<u16, NativeError> {
    match v {
        Value::Int(p) if (0..=65535).contains(p) => Ok(*p as u16),
        Value::Int(p) => Err(NativeError::new(
            ThrowClass::IllegalArgumentException,
            format!("Invalid socket server port: {p}"),
        )),
        _ => Err(cce_err(v, "java.lang.Long")),
    }
}

/// Mint a connection value over an accepted or dialed stream
/// (accepted sockets inherit the listener's nonblocking flag on
/// BSDs, so block explicitly — REPL reads are blocking).
fn conn_value(eval: &mut Evaluator, stream: TcpStream) -> Result<Value, NativeError> {
    stream.set_nonblocking(false).map_err(io_err)?;
    let writer = stream.try_clone().map_err(io_err)?;
    Ok(Value::Extension(Arc::new(SocketConn {
        id: eval.next_id(),
        state: Arc::new(ConnState {
            reader: Mutex::new(BufReader::new(stream)),
            writer: Mutex::new(writer),
            closed: AtomicBool::new(false),
        }),
    })))
}

/// `__server-socket*`: bind a server socket (nil address is
/// loopback, like `InetAddress/getByName` on nil; port 0 is
/// ephemeral). Host names resolve through the OS resolver.
fn native_server_socket(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__server-socket*", args, &[2])?;
    let host = match &args[0] {
        Value::Nil => "127.0.0.1".to_owned(),
        Value::Str(s) => s.to_string(),
        v => return Err(cce_err(v, "java.lang.String")),
    };
    let port = need_port(&args[1])?;
    let listener = TcpListener::bind(format!("{host}:{port}")).map_err(io_err)?;
    listener.set_nonblocking(true).map_err(io_err)?;
    let local_port = listener.local_addr().map_err(io_err)?.port();
    Ok(Value::Extension(Arc::new(ServerSock {
        id: eval.next_id(),
        inner: Arc::new(ServerInner {
            listener: Mutex::new(Some(listener)),
            closed: AtomicBool::new(false),
            local_port,
        }),
    })))
}

/// `__server-port*`: the bound local port (cached, so it answers
/// after close, like `getLocalPort`).
fn native_server_port(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__server-port*", args, &[1])?;
    let sock = need_server(&args[0])?;
    Ok(Value::Int(i64::from(sock.inner.local_port)))
}

/// `__server-accept*`: block until a client connects (nil once the
/// server closes). Accept errors retry after the poll sleep: Linux
/// spuriously aborts accepts, upstream's loop swallows them too (its
/// `catch SocketException` recurs), and a close always rescues the
/// loop, so no failure can wedge it.
fn native_server_accept(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__server-accept*", args, &[1])?;
    let sock = need_server(&args[0])?.inner.clone();
    loop {
        let accepted = match lock(&sock.listener).as_ref() {
            Some(l) => l.accept(),
            None => return Ok(Value::Nil),
        };
        match accepted {
            Ok((stream, _)) => return conn_value(eval, stream),
            Err(_) => std::thread::sleep(ACCEPT_POLL),
        }
    }
}

/// `__server-close*`: latch closed and drop the listener (idempotent;
/// the OS port frees at once, like the JDK).
fn native_server_close(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__server-close*", args, &[1])?;
    let sock = need_server(&args[0])?;
    sock.inner.closed.store(true, Ordering::SeqCst);
    lock(&sock.inner.listener).take();
    Ok(Value::Nil)
}

/// `__server-closed?*`: whether the server latched closed.
fn native_server_closed(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__server-closed?*", args, &[1])?;
    let sock = need_server(&args[0])?;
    Ok(Value::Bool(sock.inner.closed.load(Ordering::SeqCst)))
}

/// `__socket-connect*`: dial a host and port (like `remote-prepl`'s
/// `Socket.` ctor; refused/unresolvable is an `IOException`).
fn native_socket_connect(eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-connect*", args, &[2])?;
    let Value::Str(host) = &args[0] else {
        return Err(cce_err(&args[0], "java.lang.String"));
    };
    let port = need_port(&args[1])?;
    let stream = TcpStream::connect(format!("{host}:{port}")).map_err(io_err)?;
    conn_value(eval, stream)
}

/// `__socket-conn?*`: whether a value is a socket connection (lets
/// `prepl`/`remote-prepl` aim reads at sockets or stream readers).
fn native_socket_conn_pred(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-conn?*", args, &[1])?;
    Ok(Value::Bool(as_conn(&args[0]).is_some()))
}

/// `__socket-read-line*`: one line off a connection (nil at EOF; the
/// trailing newline stays behind, like `read-line`).
fn native_socket_read_line(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-read-line*", args, &[1])?;
    let conn = need_conn(&args[0])?;
    if conn.state.closed.load(Ordering::SeqCst) {
        return Ok(Value::Nil);
    }
    let mut buf = String::new();
    match lock(&conn.state.reader).read_line(&mut buf) {
        Ok(0) => Ok(Value::Nil),
        Ok(_) => {
            let line = buf.strip_suffix('\n').unwrap_or(&buf);
            let line = line.strip_suffix('\r').unwrap_or(line);
            Ok(Value::Str(Arc::from(line)))
        }
        Err(e) => Err(io_err(e)),
    }
}

/// `__socket-write*`: write a string to a connection, flushed at
/// once (socket REPLs print line by line, so there is nothing
/// useful to buffer).
fn native_socket_write(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-write*", args, &[2])?;
    let conn = need_conn(&args[0])?;
    if conn.state.closed.load(Ordering::SeqCst) {
        return Err(NativeError::new(
            ThrowClass::IOException,
            "Socket is closed".to_owned(),
        ));
    }
    let Value::Str(s) = &args[1] else {
        return Err(cce_err(&args[1], "java.lang.String"));
    };
    let mut w = lock(&conn.state.writer);
    w.write_all(s.as_bytes()).map_err(io_err)?;
    w.flush().map_err(io_err)?;
    Ok(Value::Nil)
}

/// `__socket-close*`: shut a connection down (idempotent; unblocks a
/// pending read with EOF, like the JDK).
fn native_socket_close(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-close*", args, &[1])?;
    let conn = need_conn(&args[0])?;
    conn.state.closed.store(true, Ordering::SeqCst);
    let _ = lock(&conn.state.writer).shutdown(Shutdown::Both);
    Ok(Value::Nil)
}

/// `__socket-shutdown-write*`: half-close a connection's write side
/// (idempotent; the peer reads EOF while this side keeps reading —
/// `remote-prepl`'s writer epilogue. A full close here would strand
/// in-flight replies: on BSDs `SHUT_RD` discards pending input,
/// unlike the JDK's graceful socket close).
fn native_socket_shutdown_write(
    _eval: &mut Evaluator,
    args: &[Value],
) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__socket-shutdown-write*", args, &[1])?;
    let conn = need_conn(&args[0])?;
    let _ = lock(&conn.state.writer).shutdown(Shutdown::Write);
    Ok(Value::Nil)
}

/// The monotonic origin for [`native_nano_time`] (first use starts
/// the clock; only differences ever surface).
static NANO_ORIGIN: std::sync::LazyLock<Instant> = std::sync::LazyLock::new(Instant::now);

/// `__nano-time*`: monotonic nanoseconds (the `System/nanoTime` the
/// port's `System` lacks, for `prepl`'s `:ms` timings).
fn native_nano_time(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.core.server/__nano-time*", args, &[0])?;
    Ok(Value::Int(NANO_ORIGIN.elapsed().as_nanos() as i64))
}

/// The socket-server natives (installed lazily by
/// [`install_server_natives`] — never referred into `user`).
pub(crate) const SERVER_NATIVES: &[(&str, NativeFnPtr)] = &[
    ("__server-socket*", native_server_socket),
    ("__server-port*", native_server_port),
    ("__server-accept*", native_server_accept),
    ("__server-close*", native_server_close),
    ("__server-closed?*", native_server_closed),
    ("__socket-connect*", native_socket_connect),
    ("__socket-conn?*", native_socket_conn_pred),
    ("__socket-read-line*", native_socket_read_line),
    ("__socket-write*", native_socket_write),
    ("__socket-close*", native_socket_close),
    ("__socket-shutdown-write*", native_socket_shutdown_write),
    ("__nano-time*", native_nano_time),
];

/// Install the `clojure.core.server` natives (idempotent —
/// reinstalling over itself is a `:reload` no-op). `pub` so
/// integration tests can install before `require` until the loader
/// grows the matching arm.
pub fn install_server_natives(eval: &mut Evaluator) {
    eval.registry.ensure_ns(SERVER_NS);
    for (name, f) in SERVER_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(SERVER_NS, name, func);
    }
}

/// One scanned XML event: element boundaries carry the qualified tag
/// (upstream keys tags by q-name) and flat attributes; character
/// data accumulates across text, entity, and CDATA runs (like SAX
/// callers observe through the content handler).
enum XmlEvent {
    Start {
        tag: String,
        attrs: Vec<(String, String)>,
    },
    End {
        tag: String,
    },
    Chars {
        text: String,
    },
}

/// An open tag: the tag plus flat attributes plus whether it
/// self-closes.
type OpenTag = (String, Vec<(String, String)>, bool);

/// A scan failure: a SAX-style message plus the span where the
/// scanner stopped.
struct XmlError {
    message: String,
    line: usize,
    col: usize,
}

/// The byte scanner (`text` is `\r`-normalized up front, so `\n`
/// alone tracks lines; splits only land on ASCII bytes, so slicing
/// stays on char boundaries).
struct XmlScanner<'a> {
    text: &'a str,
    bytes: &'a [u8],
    pos: usize,
    line: usize,
    line_start: usize,
}

impl<'a> XmlScanner<'a> {
    fn new(text: &'a str) -> Self {
        Self {
            text,
            bytes: text.as_bytes(),
            pos: 0,
            line: 1,
            line_start: 0,
        }
    }

    /// The column of the current position (chars past the last
    /// newline, computed only for errors).
    fn col(&self) -> usize {
        self.text[self.line_start..self.pos].chars().count() + 1
    }

    fn fail(&self, message: String) -> XmlError {
        XmlError {
            message,
            line: self.line,
            col: self.col(),
        }
    }

    fn peek(&self) -> Option<u8> {
        self.bytes.get(self.pos).copied()
    }

    fn starts_with(&self, s: &str) -> bool {
        self.text[self.pos..].starts_with(s)
    }

    /// Advance one byte, tracking newlines.
    fn bump(&mut self) {
        if self.bytes[self.pos] == b'\n' {
            self.line += 1;
            self.line_start = self.pos + 1;
        }
        self.pos += 1;
    }

    fn skip_ws(&mut self) {
        while matches!(self.peek(), Some(b' ' | b'\t' | b'\n')) {
            self.bump();
        }
    }

    /// Whether a byte can start a name (ASCII letter, `_`, `:`, or
    /// any non-ASCII byte of the valid-UTF-8 input).
    fn is_name_start(b: u8) -> bool {
        b.is_ascii_alphabetic() || b == b'_' || b == b':' || b >= 0x80
    }

    /// Whether a byte can continue a name (start bytes plus digits,
    /// `.`, and `-`).
    fn is_name_char(b: u8) -> bool {
        Self::is_name_start(b) || b.is_ascii_digit() || b == b'.' || b == b'-'
    }

    /// Read a qualified name (`None` when the next byte cannot start
    /// one).
    fn read_name(&mut self) -> Option<String> {
        let start = self.pos;
        if !self.peek().is_some_and(Self::is_name_start) {
            return None;
        }
        while self.peek().is_some_and(Self::is_name_char) {
            self.bump();
        }
        Some(self.text[start..self.pos].to_owned())
    }

    /// Skip to (and past) a closing delimiter (`None` at EOF).
    fn skip_until(&mut self, delim: &str) -> bool {
        loop {
            if self.text[self.pos..].starts_with(delim) {
                self.pos += delim.len();
                return true;
            }
            if self.peek().is_none() {
                return false;
            }
            // Newlines inside skipped regions still count (columns
            // past a multiline comment stay honest).
            self.bump();
        }
    }

    /// Skip miscellany (whitespace, comments, processing
    /// instructions) between markup.
    fn skip_misc(&mut self) -> Result<(), XmlError> {
        loop {
            self.skip_ws();
            if self.starts_with("<!--") {
                self.skip_comment()?;
            } else if self.starts_with("<?") {
                self.skip_pi()?;
            } else {
                return Ok(());
            }
        }
    }

    /// Skip a comment, rejecting `--` inside (like SAX).
    fn skip_comment(&mut self) -> Result<(), XmlError> {
        self.pos += "<!--".len();
        let start = self.pos;
        if !self.skip_until("-->") {
            return Err(self.fail("unclosed comment".to_owned()));
        }
        if self.text[start..self.pos - "-->".len()].contains("--") {
            return Err(self.fail("comment must not contain --".to_owned()));
        }
        Ok(())
    }

    /// Skip a processing instruction (the prolog `<?xml ...?>` rides
    /// along — the input is already a string, so there is no
    /// encoding to honor).
    fn skip_pi(&mut self) -> Result<(), XmlError> {
        self.pos += "<?".len();
        if !self.skip_until("?>") {
            return Err(self.fail("unclosed processing instruction".to_owned()));
        }
        Ok(())
    }

    /// Skip a DOCTYPE (the internal subset may nest `[]` and quote
    /// strings; nothing is ever resolved, so external entities
    /// cannot leak in).
    fn skip_doctype(&mut self) -> Result<(), XmlError> {
        self.pos += "<!DOCTYPE".len();
        let mut depth = 0usize;
        loop {
            match self.peek() {
                None => return Err(self.fail("unclosed DOCTYPE".to_owned())),
                Some(b'[') => {
                    depth += 1;
                    self.bump();
                }
                Some(b']') => {
                    depth = depth.saturating_sub(1);
                    self.bump();
                }
                Some(b'"' | b'\'') => self.skip_quoted()?,
                Some(b'>') if depth == 0 => {
                    self.bump();
                    return Ok(());
                }
                _ => self.bump(),
            }
        }
    }

    /// Skip a quoted string inside a DOCTYPE (no entity expansion —
    /// the subset is inert).
    fn skip_quoted(&mut self) -> Result<(), XmlError> {
        let quote = self.peek().expect("the caller matched a quote");
        self.bump();
        loop {
            match self.peek() {
                None => return Err(self.fail("unclosed DOCTYPE".to_owned())),
                Some(b) if b == quote => {
                    self.bump();
                    return Ok(());
                }
                _ => self.bump(),
            }
        }
    }

    /// Resolve one entity reference at the cursor (the `&` is
    /// current): the five predeclared names plus decimal and hex
    /// character references (custom entities stay undefined — the
    /// subset is inert, so there is nothing to resolve them from).
    fn read_entity(&mut self) -> Result<char, XmlError> {
        self.bump();
        let start = self.pos;
        while matches!(self.peek(), Some(b) if b != b';' && !b.is_ascii_whitespace() && b != b'&' && b != b'<')
        {
            self.bump();
        }
        if self.peek() != Some(b';') {
            return Err(self.fail("malformed entity reference".to_owned()));
        }
        let body = self.text[start..self.pos].to_owned();
        self.bump();
        match body.as_str() {
            "amp" => Ok('&'),
            "lt" => Ok('<'),
            "gt" => Ok('>'),
            "quot" => Ok('"'),
            "apos" => Ok('\''),
            _ => self.char_ref(&body),
        }
    }

    /// Resolve a numeric character reference body (`#123`/`#x1F`).
    fn char_ref(&self, body: &str) -> Result<char, XmlError> {
        let digits = body.strip_prefix('#');
        let (digits, radix) = match digits {
            Some(rest) if rest.starts_with(['x', 'X']) => (&rest[1..], 16),
            Some(rest) => (rest, 10),
            None => {
                return Err(self.fail(format!("undefined entity &{body};")));
            }
        };
        let cp = u32::from_str_radix(digits, radix).unwrap_or(u32::MAX);
        match char::from_u32(cp) {
            Some(c) if is_xml_char(cp) => Ok(c),
            _ => Err(self.fail(format!("invalid character reference &{body};"))),
        }
    }

    /// Read one attribute value (the quote is current), expanding
    /// entities and rejecting `<` (like SAX).
    fn read_attr_value(&mut self) -> Result<String, XmlError> {
        let quote = self.peek().expect("the caller matched a quote");
        self.bump();
        let mut out = String::new();
        loop {
            match self.peek() {
                None => return Err(self.fail("unclosed attribute value".to_owned())),
                Some(b) if b == quote => {
                    self.bump();
                    return Ok(out);
                }
                Some(b'&') => out.push(self.read_entity()?),
                Some(b'<') => {
                    return Err(self.fail("invalid '<' in attribute value".to_owned()));
                }
                Some(_) => {
                    // A whole char at a time (byte slicing would
                    // split multibyte text).
                    let ch = self.text[self.pos..]
                        .chars()
                        .next()
                        .expect("a char is current");
                    out.push(ch);
                    for _ in 0..ch.len_utf8() {
                        self.bump();
                    }
                }
            }
        }
    }

    /// Read the attribute list of an open tag (names must be unique,
    /// like SAX reports).
    fn read_attrs(&mut self) -> Result<Vec<(String, String)>, XmlError> {
        let mut attrs = Vec::new();
        loop {
            self.skip_ws();
            match self.peek() {
                Some(b'/') | Some(b'>') | None => return Ok(attrs),
                _ => {}
            }
            let Some(name) = self.read_name() else {
                return Err(self.fail("invalid attribute name".to_owned()));
            };
            self.skip_ws();
            if self.peek() != Some(b'=') {
                return Err(self.fail(format!("attribute \"{name}\" must have a value")));
            }
            self.bump();
            self.skip_ws();
            if !matches!(self.peek(), Some(b'"' | b'\'')) {
                return Err(self.fail(format!("attribute \"{name}\" value must be quoted")));
            }
            if attrs.iter().any(|(n, _)| *n == name) {
                return Err(self.fail(format!("duplicate attribute \"{name}\"")));
            }
            let value = self.read_attr_value()?;
            attrs.push((name, value));
        }
    }

    /// Scan one open tag (the `<` is current), returning the tag and
    /// attributes plus whether it self-closes.
    fn read_open_tag(&mut self) -> Result<OpenTag, XmlError> {
        self.bump();
        let Some(tag) = self.read_name() else {
            return Err(self.fail("invalid element name".to_owned()));
        };
        let attrs = self.read_attrs()?;
        if self.starts_with("/>") {
            self.pos += "/>".len();
            return Ok((tag, attrs, true));
        }
        if self.peek() != Some(b'>') {
            return Err(self.fail(format!("expected '>' to close <{tag}>")));
        }
        self.bump();
        Ok((tag, attrs, false))
    }

    /// Scan one close tag (the `<` of `</` is current).
    fn read_close_tag(&mut self) -> Result<String, XmlError> {
        self.pos += "</".len();
        let Some(tag) = self.read_name() else {
            return Err(self.fail("invalid element name".to_owned()));
        };
        self.skip_ws();
        if self.peek() != Some(b'>') {
            return Err(self.fail(format!("expected '>' to close </{tag}>")));
        }
        self.bump();
        Ok(tag)
    }

    /// Slurp a CDATA section into the char buffer (literal — no
    /// entity expansion).
    fn read_cdata(&mut self, chars: &mut String) -> Result<(), XmlError> {
        self.pos += "<![CDATA[".len();
        let start = self.pos;
        if !self.skip_until("]]>") {
            return Err(self.fail("unclosed CDATA section".to_owned()));
        }
        chars.push_str(&self.text[start..self.pos - "]]>".len()]);
        Ok(())
    }

    /// Slurp one text run into the char buffer (stops before markup
    /// or entity references).
    fn read_text(&mut self, chars: &mut String) {
        let start = self.pos;
        while matches!(self.peek(), Some(b) if b != b'<' && b != b'&') {
            self.bump();
        }
        chars.push_str(&self.text[start..self.pos]);
    }
}

/// Whether a codepoint is legal XML character data (the C0 controls
/// except tab/LF/CR, surrogates, and noncharacters are out).
fn is_xml_char(cp: u32) -> bool {
    matches!(cp, 0x9 | 0xA | 0xD)
        || matches!(cp, 0x20..=0xD7FF)
        || matches!(cp, 0xE000..=0xFFFD)
        || matches!(cp, 0x10000..=0x10FFFF)
}

/// Flush a pending char buffer as one event (element boundaries
/// always flush, so text never leaks across elements).
fn flush_chars(events: &mut Vec<XmlEvent>, chars: &mut String) {
    if !chars.is_empty() {
        events.push(XmlEvent::Chars {
            text: std::mem::take(chars),
        });
    }
}

/// If a close tag is next, flush pending chars and consume it,
/// returning `Some(())` (the content scan is done); else `None`
/// (content continues). A mismatched close tag is an error.
fn scan_close(
    scan: &mut XmlScanner,
    events: &mut Vec<XmlEvent>,
    open: &str,
    chars: &mut String,
) -> Result<Option<()>, XmlError> {
    if !scan.starts_with("</") {
        return Ok(None);
    }
    flush_chars(events, chars);
    let close = scan.read_close_tag()?;
    if close != open {
        return Err(scan.fail(format!(
            "mismatched tag: expected </{open}> but found </{close}>"
        )));
    }
    events.push(XmlEvent::End { tag: close });
    Ok(Some(()))
}

/// Scan one element's content (the open tag is consumed): children,
/// char data, and the matching close tag.
fn scan_content(
    scan: &mut XmlScanner,
    events: &mut Vec<XmlEvent>,
    open: &str,
    chars: &mut String,
) -> Result<(), XmlError> {
    loop {
        if scan_close(scan, events, open, chars)?.is_some() {
            return Ok(());
        }
        if scan.starts_with("<!--") {
            scan.skip_comment()?;
        } else if scan.starts_with("<![CDATA[") {
            scan.read_cdata(chars)?;
        } else if scan.starts_with("<?") {
            scan.skip_pi()?;
        } else if scan.peek() == Some(b'<') {
            flush_chars(events, chars);
            scan_element(scan, events)?;
        } else if scan.peek() == Some(b'&') {
            chars.push(scan.read_entity()?);
        } else if scan.peek().is_some() {
            scan.read_text(chars);
        } else {
            return Err(scan.fail("premature end of file".to_owned()));
        }
    }
}

/// Scan one element (the `<` is current at the document root or
/// after a flush in content).
fn scan_element(scan: &mut XmlScanner, events: &mut Vec<XmlEvent>) -> Result<(), XmlError> {
    // The tag reader reports bad names itself (a lone `<` reads
    // as an empty name); anything else is stray content.
    if scan.peek() != Some(b'<') {
        return Err(scan.fail("invalid markup after '<'".to_owned()));
    }
    let (tag, attrs, self_closed) = scan.read_open_tag()?;
    events.push(XmlEvent::Start {
        tag: tag.clone(),
        attrs,
    });
    if self_closed {
        events.push(XmlEvent::End { tag });
        return Ok(());
    }
    let mut chars = String::new();
    scan_content(scan, events, &tag, &mut chars)
}

/// Scan a whole document into events (prolog, exactly one root
/// element, trailing miscellany).
fn scan_document(text: &str) -> Result<Vec<XmlEvent>, XmlError> {
    let text = text.strip_prefix('\u{FEFF}').unwrap_or(text);
    let mut scan = XmlScanner::new(text);
    scan.skip_misc()?;
    if scan.starts_with("<!DOCTYPE") {
        scan.skip_doctype()?;
        scan.skip_misc()?;
    }
    if scan.peek().is_none() {
        return Err(scan.fail("premature end of file".to_owned()));
    }
    let mut events = Vec::new();
    scan_element(&mut scan, &mut events)?;
    scan.skip_misc()?;
    if scan.peek().is_some() {
        return Err(scan.fail("content after root element".to_owned()));
    }
    Ok(events)
}

/// One event as a Clojure vector (`["start" tag flat-attrs]`,
/// `["end" tag]`, `["chars" text]`).
fn event_value(event: &XmlEvent) -> Value {
    let str_of = |s: &str| Value::Str(Arc::from(s));
    match event {
        XmlEvent::Start { tag, attrs } => {
            let mut flat = Vec::with_capacity(attrs.len() * 2);
            for (k, v) in attrs {
                flat.push(str_of(k));
                flat.push(str_of(v));
            }
            Value::Vector(Vector::from_vec(vec![
                str_of("start"),
                str_of(tag),
                Value::Vector(Vector::from_vec(flat)),
            ]))
        }
        XmlEvent::End { tag } => Value::Vector(Vector::from_vec(vec![str_of("end"), str_of(tag)])),
        XmlEvent::Chars { text } => {
            Value::Vector(Vector::from_vec(vec![str_of("chars"), str_of(text)]))
        }
    }
}

/// A scan failure as a native error (a checked `Exception`, like the
/// `SAXParseException` the JDK reports).
fn xml_err(e: &XmlError) -> NativeError {
    NativeError::new(
        ThrowClass::Exception,
        format!("{} (line {}, column {})", e.message, e.line, e.col),
    )
}

/// `__xml-scan*`: scan an XML string into event vectors (line
/// endings normalize per the spec; external entities never
/// resolve).
fn native_xml_scan(_eval: &mut Evaluator, args: &[Value]) -> Result<Value, NativeError> {
    check_arity("clojure.xml/__xml-scan*", args, &[1])?;
    let Value::Str(s) = &args[0] else {
        return Err(cce_err(&args[0], "java.lang.String"));
    };
    let normalized = s.replace("\r\n", "\n").replace('\r', "\n");
    let events = scan_document(&normalized).map_err(|e| xml_err(&e))?;
    Ok(Value::Vector(Vector::from_vec(
        events.iter().map(event_value).collect(),
    )))
}

/// The XML natives (installed lazily by [`install_xml_natives`] —
/// never referred into `user`).
pub(crate) const XML_NATIVES: &[(&str, NativeFnPtr)] = &[("__xml-scan*", native_xml_scan)];

/// Install the `clojure.xml` natives (idempotent — reinstalling over
/// itself is a `:reload` no-op). `pub` so integration tests can
/// install before `require` until the loader grows the matching arm.
pub fn install_xml_natives(eval: &mut Evaluator) {
    eval.registry.ensure_ns(XML_NS);
    for (name, f) in XML_NATIVES {
        let func: Arc<crate::ext::NativeImpl> = Arc::new(*f);
        eval.install_native(XML_NS, name, func);
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ns::boot;

    /// Call a server native by table name (panics on `Err`).
    fn call(eval: &mut Evaluator, name: &str, args: &[Value]) -> Value {
        let (_, f) = SERVER_NATIVES
            .iter()
            .chain(XML_NATIVES.iter())
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no such native: {name}"));
        f(eval, args).unwrap_or_else(|e| panic!("{name} failed: {e:?}"))
    }

    /// Call a native, expecting an error (returns `Class | message`).
    fn call_err(eval: &mut Evaluator, name: &str, args: &[Value]) -> String {
        call_err_opt(eval, name, args).expect("call_err expects an error")
    }

    /// Call a native, returning the `Class | message` rendering, or
    /// `None` when the call succeeds (for "fails eventually" loops
    /// where early writes may still land in buffers).
    fn call_err_opt(eval: &mut Evaluator, name: &str, args: &[Value]) -> Option<String> {
        let (_, f) = SERVER_NATIVES
            .iter()
            .chain(XML_NATIVES.iter())
            .find(|(n, _)| *n == name)
            .unwrap_or_else(|| panic!("no such native: {name}"));
        f(eval, args)
            .err()
            .map(|e| format!("{} | {}", e.class.jvm_name(), e.message))
    }

    fn str_val(s: &str) -> Value {
        Value::Str(Arc::from(s))
    }

    /// Bind an ephemeral server (nil address is loopback).
    fn bind_server(eval: &mut Evaluator) -> Value {
        call(eval, "__server-socket*", &[Value::Nil, Value::Int(0)])
    }

    fn expect_int(v: &Value) -> i64 {
        match v {
            Value::Int(n) => *n,
            _ => panic!("int expected"),
        }
    }

    #[test]
    #[should_panic(expected = "int expected")]
    fn expect_int_rejects_non_int() {
        expect_int(&Value::Nil);
    }

    fn expect_extension(v: &Value) -> std::sync::Arc<dyn ExtensionValue> {
        match v {
            Value::Extension(e) => e.clone(),
            _ => panic!("extension expected"),
        }
    }

    #[test]
    #[should_panic(expected = "extension expected")]
    fn expect_extension_rejects_non_extension() {
        expect_extension(&Value::Nil);
    }

    fn server_port(eval: &mut Evaluator, sock: &Value) -> u16 {
        expect_int(&call(eval, "__server-port*", std::slice::from_ref(sock))) as u16
    }

    /// Dial a server through the native (returns the client conn).
    fn dial(eval: &mut Evaluator, port: u16) -> Value {
        call(
            eval,
            "__socket-connect*",
            &[str_val("127.0.0.1"), Value::Int(i64::from(port))],
        )
    }

    #[test]
    fn server_binds_loopback_ephemeral() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        assert!(as_server(&sock).is_some());
        assert_ne!(server_port(&mut eval, &sock), 0);
        assert_eq!(
            call(&mut eval, "__server-closed?*", std::slice::from_ref(&sock)),
            Value::Bool(false)
        );
        assert!(as_conn(&sock).is_none());
    }

    #[test]
    fn server_binds_named_host() {
        let mut eval = boot();
        let sock = call(
            &mut eval,
            "__server-socket*",
            &[str_val("localhost"), Value::Int(0)],
        );
        assert_ne!(server_port(&mut eval, &sock), 0);
    }

    #[test]
    fn server_rejects_bad_args() {
        let mut eval = boot();
        assert!(
            call_err(
                &mut eval,
                "__server-socket*",
                &[Value::Int(1), Value::Int(0)]
            )
            .contains("java.lang.String")
        );
        assert!(
            call_err(&mut eval, "__server-socket*", &[Value::Nil, str_val("x")])
                .contains("java.lang.Long")
        );
        assert!(
            call_err(&mut eval, "__server-socket*", &[Value::Nil, Value::Int(-1)])
                .contains("Invalid socket server port: -1")
        );
        assert!(
            call_err(
                &mut eval,
                "__server-socket*",
                &[Value::Nil, Value::Int(70000)]
            )
            .contains("Invalid socket server port: 70000")
        );
        assert!(
            call_err(
                &mut eval,
                "__server-socket*",
                &[str_val("!!!"), Value::Int(0)]
            )
            .starts_with("java.io.IOException |")
        );
        assert!(call_err(&mut eval, "__server-socket*", &[Value::Nil]).contains("Arity"));
    }

    #[test]
    fn server_close_is_idempotent_and_answers_port() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        call(&mut eval, "__server-close*", std::slice::from_ref(&sock));
        call(&mut eval, "__server-close*", std::slice::from_ref(&sock));
        assert_eq!(
            call(&mut eval, "__server-closed?*", std::slice::from_ref(&sock)),
            Value::Bool(true)
        );
        assert_eq!(server_port(&mut eval, &sock), port);
        // Accept on a closed server answers nil at once.
        assert_eq!(
            call(&mut eval, "__server-accept*", std::slice::from_ref(&sock)),
            Value::Nil
        );
    }

    #[test]
    fn server_natives_reject_non_servers() {
        let mut eval = boot();
        for name in [
            "__server-port*",
            "__server-accept*",
            "__server-close*",
            "__server-closed?*",
        ] {
            assert!(call_err(&mut eval, name, &[Value::Int(1)]).contains("java.net.ServerSocket"));
        }
    }

    #[test]
    fn accept_delivers_a_connection() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let client = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            TcpStream::connect(format!("127.0.0.1:{port}")).unwrap()
        });
        let conn = call(&mut eval, "__server-accept*", std::slice::from_ref(&sock));
        assert!(as_conn(&conn).is_some());
        assert_eq!(
            call(&mut eval, "__socket-conn?*", std::slice::from_ref(&conn)),
            Value::Bool(true)
        );
        assert_eq!(
            call(&mut eval, "__socket-conn?*", &[Value::Nil]),
            Value::Bool(false)
        );
        assert_eq!(
            call(&mut eval, "__socket-conn?*", &[str_val("x")]),
            Value::Bool(false)
        );
        client.join().unwrap();
    }

    #[test]
    fn close_unblocks_accept_with_nil() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        // Close from another thread mid-accept (the shared listener
        // cell is what the poll loop watches).
        let inner = as_server(&sock).expect("a server").inner.clone();
        let closer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(60));
            inner.closed.store(true, Ordering::SeqCst);
            lock(&inner.listener).take();
        });
        assert_eq!(
            call(&mut eval, "__server-accept*", std::slice::from_ref(&sock)),
            Value::Nil
        );
        closer.join().unwrap();
    }

    #[test]
    fn socket_round_trip_lines() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let client = std::thread::spawn(move || {
            use std::io::{BufRead, BufReader, Write};
            let mut s = TcpStream::connect(format!("127.0.0.1:{port}")).unwrap();
            s.write_all(b"ping\r\nsecond\n").unwrap();
            let mut r = BufReader::new(s.try_clone().unwrap());
            let mut line = String::new();
            r.read_line(&mut line).unwrap();
            assert_eq!(line, "pong\n");
        });
        let conn = call(&mut eval, "__server-accept*", std::slice::from_ref(&sock));
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            ),
            str_val("ping")
        );
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            ),
            str_val("second")
        );
        call(
            &mut eval,
            "__socket-write*",
            &[conn.clone(), str_val("pong\n")],
        );
        client.join().unwrap();
        // The client dropped: EOF reads nil.
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            ),
            Value::Nil
        );
    }

    #[test]
    fn socket_connect_round_trip() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        let peer = call(&mut eval, "__server-accept*", std::slice::from_ref(&sock));
        call(
            &mut eval,
            "__socket-write*",
            &[conn.clone(), str_val("hello\n")],
        );
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&peer)
            ),
            str_val("hello")
        );
    }

    #[test]
    fn socket_connect_rejects_bad_args() {
        let mut eval = boot();
        assert!(
            call_err(
                &mut eval,
                "__socket-connect*",
                &[Value::Int(1), Value::Int(1)]
            )
            .contains("java.lang.String")
        );
        assert!(
            call_err(
                &mut eval,
                "__socket-connect*",
                &[str_val("127.0.0.1"), str_val("x")]
            )
            .contains("java.lang.Long")
        );
        assert!(
            call_err(
                &mut eval,
                "__socket-connect*",
                &[str_val("127.0.0.1"), Value::Int(99999)]
            )
            .contains("Invalid socket server port: 99999")
        );
        // Nothing listens on this just-freed port.
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        call(&mut eval, "__server-close*", std::slice::from_ref(&sock));
        assert!(
            call_err(
                &mut eval,
                "__socket-connect*",
                &[str_val("127.0.0.1"), Value::Int(i64::from(port))]
            )
            .starts_with("java.io.IOException |")
        );
        assert!(
            call_err(
                &mut eval,
                "__socket-connect*",
                &[str_val("!!!"), Value::Int(1)]
            )
            .starts_with("java.io.IOException |")
        );
    }

    #[test]
    fn socket_close_is_idempotent_and_reads_nil() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        call(&mut eval, "__socket-close*", std::slice::from_ref(&conn));
        call(&mut eval, "__socket-close*", std::slice::from_ref(&conn));
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            ),
            Value::Nil
        );
        assert!(
            call_err(&mut eval, "__socket-write*", &[conn, str_val("x")])
                .contains("Socket is closed")
        );
    }

    #[test]
    fn socket_shutdown_write_half_closes() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        let peer = call(&mut eval, "__server-accept*", std::slice::from_ref(&sock));
        // Half-closing the client write side EOFs the peer ...
        call(
            &mut eval,
            "__socket-shutdown-write*",
            std::slice::from_ref(&conn),
        );
        call(
            &mut eval,
            "__socket-shutdown-write*",
            std::slice::from_ref(&conn),
        );
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&peer)
            ),
            Value::Nil
        );
        // ... while the client still reads the peer's reply.
        call(
            &mut eval,
            "__socket-write*",
            &[peer.clone(), str_val("reply\n")],
        );
        assert_eq!(
            call(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            ),
            str_val("reply")
        );
    }

    #[test]
    fn socket_natives_reject_non_conns() {
        let mut eval = boot();
        for (name, args) in [
            ("__socket-read-line*", vec![Value::Int(1)]),
            ("__socket-write*", vec![Value::Int(1), str_val("x")]),
            ("__socket-close*", vec![Value::Int(1)]),
            ("__socket-shutdown-write*", vec![Value::Int(1)]),
        ] {
            assert!(call_err(&mut eval, name, &args).contains("java.net.Socket"));
        }
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        assert!(
            call_err(&mut eval, "__socket-write*", &[conn, Value::Int(1)])
                .contains("java.lang.String")
        );
    }

    #[test]
    fn socket_read_timeout_surfaces_io_error() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        // No data ever arrives: a tiny read deadline turns the
        // blocking read into an inducible IO failure.
        let raw = as_conn(&conn).expect("a conn");
        lock(&raw.state.reader)
            .get_ref()
            .set_read_timeout(Some(Duration::from_millis(30)))
            .unwrap();
        assert!(
            call_err(
                &mut eval,
                "__socket-read-line*",
                std::slice::from_ref(&conn)
            )
            .starts_with("java.io.IOException |")
        );
    }

    #[test]
    fn socket_write_to_dead_peer_errors() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        let peer = call(&mut eval, "__server-accept*", std::slice::from_ref(&sock));
        // Drop the peer outright, then write until the dead socket
        // reports (the first write may land in buffers).
        call(&mut eval, "__socket-close*", std::slice::from_ref(&peer));
        drop(peer);
        let big = "x".repeat(1 << 20);
        let errored = (0..50).any(|_| {
            call_err_opt(&mut eval, "__socket-write*", &[conn.clone(), str_val(&big)])
                .is_some_and(|s| s.starts_with("java.io.IOException |"))
        });
        assert!(errored, "writes to a dead peer must eventually fail");
    }

    #[test]
    fn nano_time_advances() {
        let mut eval = boot();
        let t1 = call(&mut eval, "__nano-time*", &[]);
        std::thread::sleep(Duration::from_millis(2));
        let t2 = call(&mut eval, "__nano-time*", &[]);
        let a = expect_int(&t1);
        let b = expect_int(&t2);
        assert!(b >= a, "monotonic: {a} -> {b}");
    }

    #[test]
    fn handles_print_and_compare_by_id() {
        let mut eval = boot();
        let sock = bind_server(&mut eval);
        let port = server_port(&mut eval, &sock);
        let conn = dial(&mut eval, port);
        let s = expect_extension(&sock);
        assert_eq!(s.class_name(), "java.net.ServerSocket");
        assert!(s.print().starts_with("#object[java.net.ServerSocket 0x"));
        assert!(s.dyn_eq(s.as_ref()));
        assert!(!s.dyn_eq(expect_extension(&conn).as_ref()));
        let c = expect_extension(&conn);
        assert_eq!(c.class_name(), "java.net.Socket");
        assert!(c.print().starts_with("#object[java.net.Socket 0x"));
        assert!(c.dyn_eq(c.as_ref()));
        assert!(!c.dyn_eq(s.as_ref()));
        // Hashes run (equality's companion; values differ per id).
        use std::collections::hash_map::DefaultHasher;
        use std::hash::Hasher;
        let mut h1 = DefaultHasher::new();
        s.dyn_hash(&mut h1);
        let mut h2 = DefaultHasher::new();
        c.dyn_hash(&mut h2);
        assert_ne!(h1.finish(), h2.finish());
    }

    #[test]
    fn installers_are_idempotent() {
        let mut eval = boot();
        install_server_natives(&mut eval);
        install_server_natives(&mut eval);
        install_xml_natives(&mut eval);
        install_xml_natives(&mut eval);
        assert_eq!(SERVER_NATIVES.len(), 12);
        assert_eq!(XML_NATIVES.len(), 1);
    }

    /// Scan XML through the native (panics on `Err`).
    fn scan(eval: &mut Evaluator, doc: &str) -> Value {
        call(eval, "__xml-scan*", &[str_val(doc)])
    }

    /// Scan XML, expecting an error (returns `Class | message`).
    fn scan_err(eval: &mut Evaluator, doc: &str) -> String {
        call_err(eval, "__xml-scan*", &[str_val(doc)])
    }

    /// Render scanned events into a compact string for assertions.
    fn render(events: &Value) -> String {
        use clj_value::{PrintOpts, pr_to_string};
        pr_to_string(events, &PrintOpts::pr())
    }

    #[test]
    fn xml_scans_nested_elements() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(&mut eval, "<a><b>x</b><c/></a>")),
            "[[\"start\" \"a\" []] [\"start\" \"b\" []] [\"chars\" \"x\"] [\"end\" \"b\"] [\"start\" \"c\" []] [\"end\" \"c\"] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_scans_attributes() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(&mut eval, "<a x=\"1\" y = '2' z= \"&amp;\"/>")),
            "[[\"start\" \"a\" [\"x\" \"1\" \"y\" \"2\" \"z\" \"&\"]] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_scans_entities_and_refs() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(
                &mut eval,
                "<a>&lt;&gt;&quot;&apos;&#65;&#x42;&#X43;</a>"
            )),
            "[[\"start\" \"a\" []] [\"chars\" \"<>\\\"'ABC\"] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_scans_cdata_comments_pis() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(
                &mut eval,
                "<?xml version=\"1.0\"?><!--head--><a>t<![CDATA[<raw>&]]><?tail x?></a><!--fin-->"
            )),
            "[[\"start\" \"a\" []] [\"chars\" \"t<raw>&\"] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_scans_doctype_and_bom() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(
                &mut eval,
                "\u{FEFF}<!DOCTYPE greeting [ <!ELEMENT greeting (#PCDATA)> <!ENTITY x \"a>b\"> ]><greeting>x</greeting>"
            )),
            "[[\"start\" \"greeting\" []] [\"chars\" \"x\"] [\"end\" \"greeting\"]]"
        );
        assert_eq!(
            render(&scan(&mut eval, "<!DOCTYPE a><a/>")),
            "[[\"start\" \"a\" []] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_keeps_gt_and_multibyte_text() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(&mut eval, "<élan clé=\"→\">a>b✓</élan>")),
            "[[\"start\" \"élan\" [\"clé\" \"→\"]] [\"chars\" \"a>b✓\"] [\"end\" \"élan\"]]"
        );
        assert_eq!(
            render(&scan(&mut eval, "<a-1.x:y_z/>")),
            "[[\"start\" \"a-1.x:y_z\" []] [\"end\" \"a-1.x:y_z\"]]"
        );
    }

    #[test]
    fn xml_normalizes_line_endings() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(&mut eval, "<a>one\r\ntwo\rthree</a>")),
            "[[\"start\" \"a\" []] [\"chars\" \"one\\ntwo\\nthree\"] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_rejects_bad_args() {
        let mut eval = boot();
        assert!(call_err(&mut eval, "__xml-scan*", &[Value::Int(1)]).contains("java.lang.String"));
    }

    #[test]
    fn xml_rejects_mismatched_and_unclosed() {
        let mut eval = boot();
        let err = scan_err(&mut eval, "<a></b>");
        assert!(
            err.starts_with("java.lang.Exception | mismatched tag: expected </a> but found </b>"),
            "{err}"
        );
        assert!(err.contains("(line 1, column 8)"), "{err}");
        assert!(scan_err(&mut eval, "<a>oops").contains("premature end of file"));
        assert!(scan_err(&mut eval, "").contains("premature end of file"));
        assert!(scan_err(&mut eval, "   ").contains("premature end of file"));
        assert!(scan_err(&mut eval, "<!--only-->").contains("premature end of file"));
        let err2 = scan_err(&mut eval, "<a>\n</b>");
        assert!(err2.contains("(line 2, column 5)"), "{err2}");
    }

    #[test]
    fn xml_rejects_stray_content() {
        let mut eval = boot();
        assert!(scan_err(&mut eval, "hello").contains("invalid markup after '<'"));
        assert!(scan_err(&mut eval, "<").contains("invalid element name"));
        assert!(scan_err(&mut eval, "<1/>").contains("invalid element name"));
        assert!(scan_err(&mut eval, "<>").contains("invalid element name"));
        assert!(scan_err(&mut eval, "<a/><b/>").contains("content after root element"));
        assert!(scan_err(&mut eval, "<a/>junk").contains("content after root element"));
        assert!(scan_err(&mut eval, "<a><!WHAT></a>").contains("invalid element name"));
        assert!(scan_err(&mut eval, "<a / >").contains("expected '>' to close <a>"));
        assert!(scan_err(&mut eval, "<a></a x>").contains("expected '>' to close </a>"));
        assert!(scan_err(&mut eval, "<a></>").contains("invalid element name"));
    }

    #[test]
    fn xml_rejects_bad_attributes() {
        let mut eval = boot();
        assert!(scan_err(&mut eval, "<a b>").contains("attribute \"b\" must have a value"));
        assert!(scan_err(&mut eval, "<a b=c>").contains("attribute \"b\" value must be quoted"));
        assert!(scan_err(&mut eval, "<a b=\"1\" b=\"2\"/>").contains("duplicate attribute \"b\""));
        assert!(scan_err(&mut eval, "<a =\"1\"/>").contains("invalid attribute name"));
        assert!(scan_err(&mut eval, "<a b=\"xyz").contains("unclosed attribute value"));
        assert!(scan_err(&mut eval, "<a b=\"x<y\"/>").contains("invalid '<' in attribute value"));
    }

    #[test]
    fn xml_rejects_bad_entities() {
        let mut eval = boot();
        assert!(scan_err(&mut eval, "<a>&foo;</a>").contains("undefined entity &foo;"));
        assert!(scan_err(&mut eval, "<a>&;</a>").contains("undefined entity &;"));
        assert!(scan_err(&mut eval, "<a>&amp</a>").contains("malformed entity reference"));
        assert!(scan_err(&mut eval, "<a>& </a>").contains("malformed entity reference"));
        assert!(scan_err(&mut eval, "<a>&").contains("malformed entity reference"));
        assert!(scan_err(&mut eval, "<a>&#0;</a>").contains("invalid character reference &#0;"));
        assert!(
            scan_err(&mut eval, "<a>&#xD800;</a>").contains("invalid character reference &#xD800;")
        );
        assert!(scan_err(&mut eval, "<a>&#x110000;</a>").contains("invalid character reference"));
        assert!(scan_err(&mut eval, "<a>&#xFFFE;</a>").contains("invalid character reference"));
        assert!(scan_err(&mut eval, "<a>&#zz;</a>").contains("invalid character reference"));
        assert!(scan_err(&mut eval, "<a>&#x;</a>").contains("invalid character reference"));
    }

    #[test]
    fn xml_accepts_edge_refs() {
        let mut eval = boot();
        assert_eq!(
            render(&scan(&mut eval, "<a>&#9;&#xA;&#x20AC;&#x1F600;</a>")),
            "[[\"start\" \"a\" []] [\"chars\" \"\\t\\n€😀\"] [\"end\" \"a\"]]"
        );
    }

    #[test]
    fn xml_rejects_unclosed_markup() {
        let mut eval = boot();
        assert!(scan_err(&mut eval, "<a><!--x</a>").contains("unclosed comment"));
        assert!(scan_err(&mut eval, "<a><!--x--y--></a>").contains("comment must not contain --"));
        assert!(scan_err(&mut eval, "<?oops").contains("unclosed processing instruction"));
        assert!(scan_err(&mut eval, "<a><?oops</a>").contains("unclosed processing instruction"));
        assert!(scan_err(&mut eval, "<a><![CDATA[x</a>").contains("unclosed CDATA section"));
        assert!(scan_err(&mut eval, "<!DOCTYPE a").contains("unclosed DOCTYPE"));
        assert!(scan_err(&mut eval, "<!DOCTYPE a [ \"xyz").contains("unclosed DOCTYPE"));
        assert_eq!(
            render(&scan(&mut eval, "<!DOCTYPE a [ 'x' ]><a/>")),
            "[[\"start\" \"a\" []] [\"end\" \"a\"]]"
        );
    }
}
