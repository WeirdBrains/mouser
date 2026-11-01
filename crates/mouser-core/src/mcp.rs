//! MCP client (P1 skeleton, P3 client): stdio servers (a child process,
//! newline-delimited JSON-RPC) and Streamable HTTP(S) servers (POST per
//! message; JSON or SSE replies; `Mcp-Session-Id`). `initialize`,
//! `notifications/initialized`, `ping`, `tools/list`, `tools/call`.

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, channel};
use std::time::{Duration, Instant};

/// The protocol version Mouser offers (servers may answer an older one).
pub const PROTOCOL: &str = "2025-06-18";
pub const TIMEOUT: Duration = Duration::from_secs(10);
/// Tool calls may take long (a server asking its user, a slow job).
pub const CALL_TIMEOUT: Duration = Duration::from_secs(600);
/// After a cancelled call, how long a server gets to stop on its own (after
/// `notifications/cancelled`) before a stdio server is killed.
pub const CANCEL_GRACE: Duration = Duration::from_secs(1);
/// The error text of a call cancelled by the user (`Manager` reconnects when the
/// server had to be stopped).
pub const CANCELLED: &str = "cancelled by the user";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Tool {
    pub name: String,
    pub description: String,
}

/// An HTTP(S) endpoint.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Target {
    pub https: bool,
    pub host: String,
    pub port: u16,
    pub path: String,
}

impl Target {
    pub fn parse(url: &str) -> Result<Self> {
        let (https, rest) = if let Some(r) = url.strip_prefix("https://") {
            (true, r)
        } else if let Some(r) = url.strip_prefix("http://") {
            (false, r)
        } else {
            bail!("url must start with http:// or https://");
        };
        let (hp, path) = match rest.find('/') {
            Some(i) => (&rest[..i], rest[i..].to_string()),
            None => (rest, "/".to_string()),
        };
        let (host, port) = if let Some(h) = hp.strip_prefix('[') {
            let (h, p) = h.split_once(']').context("bad [ipv6] host")?;
            (h.to_string(), p.strip_prefix(':').map(str::parse).transpose()?)
        } else {
            match hp.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), Some(p.parse().context("bad port")?)),
                None => (hp.to_string(), None),
            }
        };
        if host.is_empty() {
            bail!("no host in {url}");
        }
        let t = Target { https, port: port.unwrap_or(if https { 443 } else { 80 }), host, path };
        if !t.https && !t.loopback() {
            bail!("plain http is only allowed to this computer (127.0.0.1/localhost); use https for {}", t.host);
        }
        Ok(t)
    }

    pub fn loopback(&self) -> bool {
        matches!(self.host.as_str(), "127.0.0.1" | "localhost" | "::1")
    }

    pub fn host_header(&self) -> String {
        if self.host.contains(':') { format!("[{}]:{}", self.host, self.port) } else { format!("{}:{}", self.host, self.port) }
    }
}

/// TLS for a client: trust `ca` (PEM) if given, else the Mozilla roots.
pub fn tls_for(ca: Option<&std::path::Path>) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    match ca {
        Some(p) => {
            for c in rustls_pemfile::certs(&mut BufReader::new(std::fs::File::open(p).with_context(|| format!("{}", p.display()))?)) {
                roots.add(c?)?;
            }
        }
        None => roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned()),
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// An HTTP reply.
pub struct Reply {
    pub status: u16,
    pub session: Option<String>,
    pub content_type: String,
    pub body: String,
}

/// POST `body` to `t` (one request per connection).
pub fn post_to(
    t: &Target,
    token: Option<&str>,
    session: Option<&str>,
    body: &str,
    tls: Option<Arc<rustls::ClientConfig>>,
    timeout: Duration,
) -> Result<Reply> {
    let mut last = None;
    let mut conn = None;
    for addr in (t.host.as_str(), t.port).to_socket_addrs().with_context(|| format!("resolving {}", t.host))? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(5)) {
            Ok(s) => {
                conn = Some(s);
                break;
            }
            Err(e) => last = Some(e),
        }
    }
    let s = match (conn, last) {
        (Some(s), _) => s,
        (None, Some(e)) => return Err(anyhow::Error::from(e).context(format!("connecting to {}", t.host_header()))),
        (None, None) => bail!("no address for {}", t.host),
    };
    s.set_read_timeout(Some(timeout))?;
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nMCP-Protocol-Version: {PROTOCOL}\r\nContent-Length: {}\r\nConnection: close\r\n",
        t.path,
        t.host_header(),
        body.len()
    );
    if let Some(tok) = token {
        req.push_str(&format!("Authorization: Bearer {tok}\r\n"));
    }
    if let Some(id) = session {
        req.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    let mut raw = Vec::new();
    if t.https {
        let cfg = match tls {
            Some(c) => c,
            None => tls_for(None)?,
        };
        let name = rustls::pki_types::ServerName::try_from(t.host.clone())?;
        let mut st = rustls::StreamOwned::new(rustls::ClientConnection::new(cfg, name)?, s);
        st.write_all(req.as_bytes())?;
        if let Err(e) = st.read_to_end(&mut raw)
            && raw.is_empty()
        {
            return Err(e.into());
        }
    } else {
        let mut s = s;
        s.write_all(req.as_bytes())?;
        s.read_to_end(&mut raw)?;
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").context("bad HTTP reply")?;
    let header =
        |k: &str| head.lines().find_map(|l| l.split_once(':').filter(|(a, _)| a.trim().eq_ignore_ascii_case(k)).map(|(_, v)| v.trim().to_string()));
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let mut body = body.to_string();
    if header("transfer-encoding").is_some_and(|v| v.eq_ignore_ascii_case("chunked")) {
        body = dechunk(&body);
    }
    Ok(Reply { status, session: header("mcp-session-id"), content_type: header("content-type").unwrap_or_default(), body })
}

fn dechunk(s: &str) -> String {
    let mut out = String::new();
    let mut rest = s;
    while let Some((len, r)) = rest.split_once("\r\n") {
        let Ok(n) = usize::from_str_radix(len.trim(), 16) else { break };
        if n == 0 || r.len() < n {
            break;
        }
        out.push_str(&r[..n]);
        rest = r[n..].strip_prefix("\r\n").unwrap_or(&r[n..]);
    }
    out
}

/// JSON-RPC messages in a reply body (plain JSON or SSE `data:` events).
pub fn messages(content_type: &str, body: &str) -> Vec<Value> {
    if content_type.starts_with("text/event-stream") || body.starts_with("event:") || body.starts_with("data:") {
        let mut out = Vec::new();
        for ev in body.split("\n\n") {
            let data: Vec<&str> = ev.lines().filter_map(|l| l.strip_prefix("data:")).map(str::trim_start).collect();
            if let Ok(v) = serde_json::from_str::<Value>(&data.join("\n")) {
                out.push(v);
            }
        }
        out
    } else {
        match serde_json::from_str::<Value>(body.trim()) {
            Ok(Value::Array(a)) => a,
            Ok(v) => vec![v],
            Err(_) => vec![],
        }
    }
}

enum Link {
    Stdio { child: Child, stdin: ChildStdin, lines: Receiver<String> },
    Http { target: Target, token: Option<String>, tls: Option<Arc<rustls::ClientConfig>>, session: Option<String> },
}

pub struct Client {
    link: Link,
    next: u64,
    dead: bool,
    /// `serverInfo.name` / `version` and the negotiated protocol.
    pub server: (String, String, String),
}

/// A scrubbed environment for child servers (no tokens or keys leak to them).
fn scrubbed(c: &mut Command, plugin: &str) {
    c.env_clear();
    for k in [
        "PATH",
        "HOME",
        "LANG",
        "SystemRoot",
        "TEMP",
        "TMP",
        "USERPROFILE",
        "MOUSER_HOME",
        "MOUSER_SECRET_STORE",
        "XDG_RUNTIME_DIR",
        "DBUS_SESSION_BUS_ADDRESS",
    ] {
        if let Some(v) = std::env::var_os(k) {
            c.env(k, v);
        }
    }
    c.env("MOUSER_PLUGIN_API", crate::plugins::API.to_string()).env("MOUSER_PLUGIN_NAME", plugin);
}

impl Client {
    /// Start `argv` in `dir` with a scrubbed environment and do the handshake.
    pub fn start(argv: &[String], dir: &std::path::Path, plugin: &str) -> Result<Self> {
        Self::start_env(argv, dir, plugin, &[])
    }

    /// Like [`start`](Self::start), with extra environment (`MOUSER_HOME` for Mouser's own bridge).
    pub fn start_env(argv: &[String], dir: &std::path::Path, plugin: &str, env: &[(String, String)]) -> Result<Self> {
        let (prog, args) = argv.split_first().context("empty command")?;
        let mut c = Command::new(prog);
        c.args(args).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        // Its own process group, so stopping it also stops what it started.
        #[cfg(unix)]
        std::os::unix::process::CommandExt::process_group(&mut c, 0);
        scrubbed(&mut c, plugin);
        for (k, v) in env {
            c.env(k, v);
        }
        let mut child = c.spawn().with_context(|| format!("starting {prog}"))?;
        let stdin = child.stdin.take().context("stdin")?;
        let out = child.stdout.take().context("stdout")?;
        let (tx, lines) = channel();
        std::thread::spawn(move || {
            for l in BufReader::new(out).lines() {
                let Ok(l) = l else { break };
                if tx.send(l).is_err() {
                    break;
                }
            }
        });
        Self::handshake(Client { link: Link::Stdio { child, stdin, lines }, next: 1, dead: false, server: Default::default() })
    }

    /// Connect to a Streamable HTTP(S) server.
    pub fn connect(url: &str, token: Option<String>, ca: Option<&std::path::Path>) -> Result<Self> {
        let target = Target::parse(url)?;
        let tls = if target.https { Some(tls_for(ca)?) } else { None };
        Self::handshake(Client { link: Link::Http { target, token, tls, session: None }, next: 1, dead: false, server: Default::default() })
    }

    fn handshake(mut me: Self) -> Result<Self> {
        let r = me.request(
            "initialize",
            json!({"protocolVersion": PROTOCOL, "capabilities": {}, "clientInfo": {"name": "mouser", "version": env!("CARGO_PKG_VERSION")}}),
        )?;
        let s = |v: &Value| v.as_str().unwrap_or("").to_string();
        me.server = (s(&r["serverInfo"]["name"]), s(&r["serverInfo"]["version"]), s(&r["protocolVersion"]));
        me.notify("notifications/initialized")?;
        Ok(me)
    }

    fn notify(&mut self, method: &str) -> Result<()> {
        let m = json!({"jsonrpc": "2.0", "method": method});
        match &mut self.link {
            Link::Stdio { stdin, .. } => {
                let mut b = serde_json::to_vec(&m)?;
                b.push(b'\n');
                stdin.write_all(&b)?;
                stdin.flush()?;
            }
            Link::Http { target, token, tls, session } => {
                post_to(target, token.as_deref(), session.as_deref(), &m.to_string(), tls.clone(), TIMEOUT)?;
            }
        }
        Ok(())
    }

    /// One request; skips notifications and server requests until our reply.
    pub fn request(&mut self, method: &str, params: Value) -> Result<Value> {
        self.request_for(method, params, TIMEOUT)
    }

    fn request_for(&mut self, method: &str, params: Value, timeout: Duration) -> Result<Value> {
        self.request_cancellable(method, params, timeout, None)
    }

    fn request_cancellable(&mut self, method: &str, params: Value, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<Value> {
        let v = self.exchange(method, params, timeout, cancel)?;
        if let Some(e) = v.get("error") {
            bail!("{method}: {}", e["message"].as_str().unwrap_or("error"));
        }
        Ok(v["result"].clone())
    }

    /// Send a request and return the whole reply message (result or error).
    ///
    /// With `cancel`: when it is set the server gets `notifications/cancelled`;
    /// a stdio server that hasn't stopped within [`CANCEL_GRACE`] is killed
    /// (with its process group), and the error says so.
    fn exchange(&mut self, method: &str, params: Value, timeout: Duration, cancel: Option<&AtomicBool>) -> Result<Value> {
        let id = self.next;
        self.next += 1;
        let m = json!({"jsonrpc": "2.0", "id": id, "method": method, "params": params});
        let pick = |v: &Value| v.get("id").and_then(Value::as_u64) == Some(id) && v.get("method").is_none();
        let v = match &mut self.link {
            Link::Stdio { stdin, lines, .. } => {
                let mut b = serde_json::to_vec(&m)?;
                b.push(b'\n');
                stdin.write_all(&b)?;
                stdin.flush()?;
                let deadline = Instant::now() + timeout;
                let mut cancelled_at: Option<Instant> = None;
                loop {
                    if cancelled_at.is_none() && cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                        let n = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": CANCELLED}});
                        let mut b = serde_json::to_vec(&n)?;
                        b.push(b'\n');
                        let _ = stdin.write_all(&b);
                        let _ = stdin.flush();
                        cancelled_at = Some(Instant::now());
                    }
                    if let Some(t) = cancelled_at
                        && t.elapsed() >= CANCEL_GRACE
                    {
                        break None;
                    }
                    let mut left = deadline.saturating_duration_since(Instant::now());
                    if cancel.is_some() {
                        left = left.min(Duration::from_millis(50));
                    }
                    let l = match lines.recv_timeout(left) {
                        Ok(l) => l,
                        Err(RecvTimeoutError::Timeout) if Instant::now() < deadline => continue,
                        Err(RecvTimeoutError::Timeout) => bail!("{method}: no reply in {}s", timeout.as_secs()),
                        Err(RecvTimeoutError::Disconnected) => bail!("{method}: the server exited"),
                    };
                    if cancelled_at.is_some()
                        && let Ok(v) = serde_json::from_str::<Value>(&l)
                        && pick(&v)
                    {
                        // It stopped on its own (a reply or an error after the cancel).
                        bail!("{method}: {CANCELLED}");
                    }
                    if let Ok(v) = serde_json::from_str::<Value>(&l)
                        && pick(&v)
                    {
                        break Some(v);
                    }
                }
            }
            Link::Http { target, token, tls, session } => {
                // On cancel, tell the server from another connection; it ends the request.
                let done = AtomicBool::new(false);
                let r = std::thread::scope(|sc| {
                    if let Some(c) = cancel {
                        let (t, k, tl, se, d) = (target.clone(), token.clone(), tls.clone(), session.clone(), &done);
                        sc.spawn(move || {
                            while !d.load(Ordering::SeqCst) {
                                if c.load(Ordering::SeqCst) {
                                    let n = json!({"jsonrpc": "2.0", "method": "notifications/cancelled", "params": {"requestId": id, "reason": CANCELLED}});
                                    let _ = post_to(&t, k.as_deref(), se.as_deref(), &n.to_string(), tl, TIMEOUT);
                                    return;
                                }
                                std::thread::sleep(Duration::from_millis(50));
                            }
                        });
                    }
                    let r = post_to(target, token.as_deref(), session.as_deref(), &m.to_string(), tls.clone(), timeout);
                    done.store(true, Ordering::SeqCst);
                    r
                });
                if cancel.is_some_and(|c| c.load(Ordering::SeqCst)) {
                    bail!("{method}: {CANCELLED}");
                }
                let r = r?;
                match r.status {
                    200 => {}
                    401 | 403 => bail!("{method}: refused ({}); check the token", r.status),
                    404 if session.is_some() => bail!("{method}: session expired"),
                    s => bail!("{method}: HTTP {s}"),
                }
                if r.session.is_some() {
                    *session = r.session;
                }
                Some(
                    messages(&r.content_type, &r.body)
                        .into_iter()
                        .find(|v| pick(v))
                        .with_context(|| format!("{method}: no reply in the response"))?,
                )
            }
        };
        let Some(v) = v else {
            // Cancelled and still running after the grace period: stop it.
            self.dead = true;
            if let Link::Stdio { child, .. } = &mut self.link {
                kill_tree(child);
            }
            bail!("{method}: {CANCELLED} (the server was stopped)");
        };
        Ok(v)
    }

    /// Liveness: any reply (even an error from a server without `ping`) counts.
    pub fn ping(&mut self) -> Result<()> {
        self.exchange("ping", json!({}), TIMEOUT, None).map(|_| ())
    }

    pub fn tools(&mut self) -> Result<Vec<Tool>> {
        Ok(self
            .tools_json()?
            .iter()
            .map(|t| Tool { name: t["name"].as_str().unwrap_or("").into(), description: t["description"].as_str().unwrap_or("").into() })
            .collect())
    }

    /// The full tool descriptors (with `inputSchema`).
    pub fn tools_json(&mut self) -> Result<Vec<Value>> {
        let r = self.request("tools/list", json!({}))?;
        Ok(r["tools"].as_array().cloned().unwrap_or_default())
    }

    /// Call a tool; returns the text parts of its content.
    pub fn call(&mut self, name: &str, args: Value) -> Result<String> {
        self.call_cancellable(name, args, None)
    }

    /// [`call`](Self::call) that stops when `cancel` is set: the server is told
    /// (`notifications/cancelled`) and a stdio server that keeps running is killed.
    pub fn call_cancellable(&mut self, name: &str, args: Value, cancel: Option<&AtomicBool>) -> Result<String> {
        let r = self.request_cancellable("tools/call", json!({"name": name, "arguments": args}), CALL_TIMEOUT, cancel)?;
        let text: Vec<&str> = r["content"].as_array().map(|a| a.iter().filter_map(|c| c["text"].as_str()).collect()).unwrap_or_default();
        if r["isError"].as_bool() == Some(true) {
            bail!("{name}: {}", text.join("\n"));
        }
        Ok(text.join("\n"))
    }
}

impl Client {
    /// The stdio server was stopped (after a cancel); reconnect before using it again.
    pub fn is_dead(&self) -> bool {
        self.dead
    }

    /// The stdio server's process id.
    pub fn pid(&self) -> Option<u32> {
        match &self.link {
            Link::Stdio { child, .. } => Some(child.id()),
            Link::Http { .. } => None,
        }
    }
}

/// Stop a server and everything in its process group.
fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    unsafe {
        libc::kill(-(child.id() as libc::pid_t), libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

impl Drop for Client {
    fn drop(&mut self) {
        if let Link::Stdio { child, .. } = &mut self.link {
            kill_tree(child);
        }
    }
}

#[cfg(test)]
mod url_tests {
    use super::*;

    #[test]
    fn targets_and_replies() {
        let t = Target::parse("https://example.com/mcp").unwrap();
        assert_eq!((t.https, t.host.as_str(), t.port, t.path.as_str()), (true, "example.com", 443, "/mcp"));
        let t = Target::parse("http://127.0.0.1:7801/mcp").unwrap();
        assert_eq!((t.port, t.loopback()), (7801, true));
        assert!(Target::parse("http://example.com/mcp").unwrap_err().to_string().contains("only allowed"));
        assert!(Target::parse("ftp://x").is_err());
        assert_eq!(Target::parse("https://[::1]:9/x").unwrap().host, "::1");
        assert_eq!(messages("text/event-stream", "event: message\ndata: {\"id\":1}\n\n").len(), 1);
        assert_eq!(messages("application/json", "[{\"id\":1},{\"id\":2}]").len(), 2);
        assert_eq!(dechunk("5\r\nhello\r\n0\r\n\r\n"), "hello");
    }
}

/// A tiny line-based stdio MCP server script (tests): tool `greet`.
#[cfg(all(test, unix))]
pub(crate) fn fake_server(dir: &std::path::Path) -> std::path::PathBuf {
    use std::os::unix::fs::PermissionsExt;
    let p = dir.join("srv");
    std::fs::write(&p, FAKE).unwrap();
    std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
    p
}

#[cfg(all(test, unix))]
const FAKE: &str = r#"#!/bin/sh
while read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","method":"notifications/message","params":{}}\n{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"9"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"greet","description":"say hi","inputSchema":{"type":"object"}}]}}\n' "$id";;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"hi from fake"}]}}\n' "$id";;
    *'"id"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"nope"}}\n' "$id";;
  esac
done
"#;

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;

    #[test]
    fn handshake_list_and_call_with_a_shell_mcp_server() {
        let t = tempfile::tempdir().unwrap();
        // A tiny line-based MCP server: replies by id, sends a notification first.
        let s = r#"#!/bin/sh
while read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) printf '{"jsonrpc":"2.0","method":"notifications/message","params":{}}\n{"jsonrpc":"2.0","id":%s,"result":{"protocolVersion":"2025-06-18","capabilities":{"tools":{}},"serverInfo":{"name":"fake","version":"9"}}}\n' "$id";;
    *'"tools/list"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"tools":[{"name":"greet","description":"say hi","inputSchema":{"type":"object"}}]}}\n' "$id";;
    *'"tools/call"'*) printf '{"jsonrpc":"2.0","id":%s,"result":{"content":[{"type":"text","text":"hi from fake"}]}}\n' "$id";;
    *'"id"'*) printf '{"jsonrpc":"2.0","id":%s,"error":{"code":-32601,"message":"nope"}}\n' "$id";;
  esac
done
"#;
        let p = t.path().join("srv");
        std::fs::write(&p, s).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut c = Client::start(&[p.display().to_string()], t.path(), "fake").unwrap();
        assert_eq!(c.server, ("fake".into(), "9".into(), "2025-06-18".into()));
        assert_eq!(c.tools().unwrap(), vec![Tool { name: "greet".into(), description: "say hi".into() }]);
        assert_eq!(c.call("greet", json!({})).unwrap(), "hi from fake");
        assert!(c.request("resources/list", json!({})).unwrap_err().to_string().contains("nope"));
        c.ping().unwrap();
    }

    /// Cancel reaches a running call: the server gets `notifications/cancelled`;
    /// one that ignores it is stopped (with what it started) within the grace period.
    #[test]
    fn cancel_stops_a_running_call() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let t = tempfile::tempdir().unwrap();
        let log = t.path().join("log");
        let s = format!(
            r#"#!/bin/sh
while read -r l; do
  id=$(printf '%s' "$l" | sed -n 's/.*"id":\([0-9]*\).*/\1/p')
  case "$l" in
    *'"initialize"'*) printf '{{"jsonrpc":"2.0","id":%s,"result":{{"protocolVersion":"2025-06-18","capabilities":{{}},"serverInfo":{{"name":"slow","version":"1"}}}}}}\n' "$id";;
    *'notifications/cancelled'*) echo cancelled >> {log};;
    *'"tools/call"'*) sleep 30; echo finished >> {log};;
  esac
done
"#,
            log = log.display()
        );
        let p = t.path().join("srv");
        std::fs::write(&p, s).unwrap();
        std::fs::set_permissions(&p, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut c = Client::start(&[p.display().to_string()], t.path(), "slow").unwrap();
        let pid = c.pid().unwrap();
        let cancel = Arc::new(AtomicBool::new(false));
        let k = cancel.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            k.store(true, Ordering::SeqCst);
        });
        let t0 = Instant::now();
        let e = c.call_cancellable("x", json!({}), Some(&cancel)).unwrap_err().to_string();
        let took = t0.elapsed();
        assert!(e.contains(CANCELLED) && e.contains("stopped"), "{e}");
        assert!(took < Duration::from_secs(4), "{took:?}");
        assert!(c.is_dead());
        // The server and its `sleep` are gone (the whole process group).
        std::thread::sleep(Duration::from_millis(200));
        assert_ne!(unsafe { libc::kill(-(pid as libc::pid_t), 0) }, 0, "process group still alive");
        let _ = std::fs::read_to_string(&log).unwrap_or_default();
        // Without cancel, a quick call still works on a fresh client.
        let mut c = Client::start(&[fake_server(t.path()).display().to_string()], t.path(), "fake").unwrap();
        let never = AtomicBool::new(false);
        assert_eq!(c.call_cancellable("greet", json!({}), Some(&never)).unwrap(), "hi from fake");
    }
}
