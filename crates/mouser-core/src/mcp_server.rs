//! Mouser's MCP server (P2, docs/mcp.md): JSON-RPC 2.0 MCP over Streamable HTTP
//! on loopback, the same over TLS (user cert or self-signed), and a stdio bridge
//! (`mouser mcp serve --stdio`) to the running app's loopback endpoint. The tools
//! themselves come from the app through [`Tools`].
//!
//! Security: loopback only; `Authorization: Bearer <token>` (constant-time
//! compare); Host and Origin checked against DNS rebinding; bodies capped;
//! responses are plain JSON (no SSE streams).

use anyhow::{Context, Result, bail};
use serde_json::{Value, json};
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// Versions served (newest first); the client's choice wins when listed.
pub const VERSIONS: [&str; 3] = ["2025-11-25", "2025-06-18", "2025-03-26"];
pub const MAX_BODY: usize = 4 << 20;
const MAX_CONNS: usize = 32;

/// The app's tools.
pub trait Tools: Send + Sync {
    /// MCP tool descriptors (`name`, `description`, `inputSchema`, `annotations`).
    fn list(&self) -> Vec<Value>;
    /// Run one; `client` is the session's `clientInfo.name`. `Err` becomes an
    /// `isError` result the model can read.
    fn call(&self, client: &str, name: &str, args: &Value) -> std::result::Result<Value, String>;
}

pub struct Server {
    tools: Arc<dyn Tools>,
    token: String,
    version: String,
    /// Session id -> client name.
    sessions: Mutex<HashMap<String, String>>,
}

fn err(id: &Value, code: i64, msg: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": msg}})
}

impl Server {
    /// `version`: what `serverInfo.version` reports (Mouser's version).
    pub fn new(tools: Arc<dyn Tools>, token: String, version: &str) -> Arc<Self> {
        Arc::new(Server { tools, token, version: version.into(), sessions: Mutex::new(HashMap::new()) })
    }

    /// One JSON-RPC message; `None` for notifications and responses.
    pub fn rpc(&self, client: &str, m: &Value) -> Option<Value> {
        let id = m.get("id")?.clone();
        let method = m.get("method").and_then(Value::as_str)?;
        let p = m.get("params").cloned().unwrap_or(json!({}));
        let result = match method {
            "initialize" => {
                let want = p["protocolVersion"].as_str().unwrap_or("");
                let v = VERSIONS.iter().find(|v| **v == want).copied().unwrap_or(VERSIONS[0]);
                json!({
                    "protocolVersion": v,
                    "capabilities": {"tools": {"listChanged": false}},
                    "serverInfo": {"name": "mouser", "title": "Mouser terminal", "version": self.version},
                    "instructions": "Mouser's tabs, screens, scrollback, shell and files. Typing, running and file changes ask the user in Mouser first."
                })
            }
            "ping" => json!({}),
            "tools/list" => json!({"tools": self.tools.list()}),
            "tools/call" => {
                let Some(name) = p["name"].as_str() else { return Some(err(&id, -32602, "params.name is required")) };
                if !self.tools.list().iter().any(|t| t["name"] == name) {
                    return Some(err(&id, -32602, &format!("unknown tool {name}")));
                }
                let args = p.get("arguments").cloned().unwrap_or(json!({}));
                match self.tools.call(client, name, &args) {
                    Ok(v) => {
                        let text = if let Value::String(s) = &v { s.clone() } else { serde_json::to_string_pretty(&v).unwrap_or_default() };
                        let mut r = json!({"content": [{"type": "text", "text": text}], "isError": false});
                        if v.is_object() {
                            r["structuredContent"] = v;
                        }
                        r
                    }
                    Err(e) => json!({"content": [{"type": "text", "text": e}], "isError": true}),
                }
            }
            _ => return Some(err(&id, -32601, &format!("method not found: {method}"))),
        };
        Some(json!({"jsonrpc": "2.0", "id": id, "result": result}))
    }

    /// A body: one message or a batch.
    fn body(&self, client: &str, v: &Value) -> Option<Value> {
        match v {
            Value::Array(a) => {
                let r: Vec<Value> = a.iter().filter_map(|m| self.rpc(client, m)).collect();
                (!r.is_empty()).then_some(Value::Array(r))
            }
            m => self.rpc(client, m),
        }
    }
}

/// A parsed HTTP request.
struct Req {
    method: String,
    path: String,
    headers: HashMap<String, String>,
    body: Vec<u8>,
}

fn read_req(r: &mut impl BufRead) -> Result<Req> {
    let mut line = String::new();
    if r.read_line(&mut line)? == 0 {
        bail!("closed");
    }
    let mut it = line.split_whitespace();
    let (method, path) = (it.next().unwrap_or("").to_string(), it.next().unwrap_or("").to_string());
    let mut headers = HashMap::new();
    for _ in 0..100 {
        let mut h = String::new();
        r.read_line(&mut h)?;
        let h = h.trim_end();
        if h.is_empty() {
            break;
        }
        if let Some((k, v)) = h.split_once(':') {
            headers.insert(k.trim().to_ascii_lowercase(), v.trim().to_string());
        }
    }
    let n: usize = headers.get("content-length").and_then(|v| v.parse().ok()).unwrap_or(0);
    if n > MAX_BODY {
        bail!("body too large");
    }
    let mut body = vec![0; n];
    r.read_exact(&mut body)?;
    Ok(Req { method, path, headers, body })
}

fn respond(w: &mut impl Write, status: &str, extra: &[(&str, String)], body: &[u8]) -> std::io::Result<()> {
    let mut h = format!("HTTP/1.1 {status}\r\nContent-Length: {}\r\nConnection: close\r\nCache-Control: no-store\r\n", body.len());
    if !body.is_empty() {
        h.push_str("Content-Type: application/json\r\n");
    }
    for (k, v) in extra {
        h.push_str(&format!("{k}: {v}\r\n"));
    }
    h.push_str("\r\n");
    w.write_all(h.as_bytes())?;
    w.write_all(body)?;
    w.flush()
}

fn loopback_host(h: &str, port: u16) -> bool {
    ["127.0.0.1", "localhost", "[::1]"].iter().any(|n| h == format!("{n}:{port}") || h == *n)
}

fn loopback_origin(o: &str) -> bool {
    let rest = o.strip_prefix("http://").or_else(|| o.strip_prefix("https://")).unwrap_or("");
    if rest.starts_with("[::1]") {
        return true;
    }
    matches!(rest.split(['/', ':']).next().unwrap_or(""), "127.0.0.1" | "localhost")
}

impl Server {
    /// Serve one HTTP request on `s` (plain or TLS).
    pub fn http(&self, s: &mut (impl Read + Write), port: u16) -> Result<()> {
        let req = {
            let mut r = BufReader::new(&mut *s);
            read_req(&mut r)?
        };
        let host = req.headers.get("host").map(String::as_str).unwrap_or("");
        if !loopback_host(host, port) || req.headers.get("origin").is_some_and(|o| !loopback_origin(o)) {
            respond(s, "403 Forbidden", &[], br#"{"error":"loopback clients only"}"#)?;
            return Ok(());
        }
        if req.path.split('?').next() != Some("/mcp") {
            respond(s, "404 Not Found", &[], b"")?;
            return Ok(());
        }
        let auth = req.headers.get("authorization").and_then(|a| a.strip_prefix("Bearer ")).unwrap_or("");
        if !crate::secrets::ct_eq(auth.trim().as_bytes(), self.token.as_bytes()) {
            respond(s, "401 Unauthorized", &[("WWW-Authenticate", "Bearer realm=\"mouser\"".into())], br#"{"error":"bad or missing token"}"#)?;
            return Ok(());
        }
        let sid = req.headers.get("mcp-session-id").cloned();
        match req.method.as_str() {
            "POST" => {}
            "DELETE" => {
                if let Some(id) = sid {
                    self.sessions.lock().unwrap().remove(&id);
                }
                respond(s, "200 OK", &[], b"")?;
                return Ok(());
            }
            _ => {
                respond(s, "405 Method Not Allowed", &[("Allow", "POST, DELETE".into())], b"")?;
                return Ok(());
            }
        }
        let Ok(v) = serde_json::from_slice::<Value>(&req.body) else {
            let b = err(&Value::Null, -32700, "parse error").to_string();
            respond(s, "400 Bad Request", &[], b.as_bytes())?;
            return Ok(());
        };
        let mut extra = Vec::new();
        let client = if v["method"] == "initialize" {
            let name = v["params"]["clientInfo"]["name"].as_str().unwrap_or("mcp-client");
            let name: String = name.chars().filter(|c| c.is_ascii_alphanumeric() || "-_.".contains(*c)).take(40).collect();
            let id = crate::secrets::random_token()[..32].to_string();
            let mut ss = self.sessions.lock().unwrap();
            if ss.len() > 256 {
                ss.clear();
            }
            ss.insert(id.clone(), name.clone());
            extra.push(("Mcp-Session-Id", id));
            name
        } else {
            sid.and_then(|id| self.sessions.lock().unwrap().get(&id).cloned()).unwrap_or_else(|| "mcp-client".into())
        };
        match self.body(&client, &v) {
            Some(r) => respond(s, "200 OK", &extra, r.to_string().as_bytes())?,
            None => respond(s, "202 Accepted", &extra, b"")?,
        }
        Ok(())
    }
}

/// Accept on `l` until the process ends; `tls` makes it HTTPS.
pub fn serve(l: TcpListener, tls: Option<Arc<rustls::ServerConfig>>, server: Arc<Server>) {
    let port = l.local_addr().map(|a| a.port()).unwrap_or(0);
    let live = Arc::new(AtomicUsize::new(0));
    for s in l.incoming().flatten() {
        if live.load(Ordering::SeqCst) >= MAX_CONNS {
            continue;
        }
        let (server, tls, live) = (server.clone(), tls.clone(), live.clone());
        live.fetch_add(1, Ordering::SeqCst);
        std::thread::spawn(move || {
            let _ = s.set_read_timeout(Some(Duration::from_secs(30)));
            // Tool calls that wait for the user's approval can take minutes.
            let _ = s.set_write_timeout(Some(Duration::from_secs(30)));
            let _ = match tls {
                Some(cfg) => rustls::ServerConnection::new(cfg).map_err(anyhow::Error::from).and_then(|c| {
                    let mut t = rustls::StreamOwned::new(c, s);
                    let r = server.http(&mut t, port);
                    t.conn.send_close_notify();
                    let _ = t.flush();
                    r
                }),
                None => {
                    let mut s = s;
                    server.http(&mut s, port)
                }
            };
            live.fetch_sub(1, Ordering::SeqCst);
        });
    }
}

/// Bind loopback only.
pub fn bind(port: u16) -> Result<TcpListener> {
    TcpListener::bind(("127.0.0.1", port)).with_context(|| format!("binding 127.0.0.1:{port}"))
}

pub fn dir() -> PathBuf {
    crate::storage::resolve_home().join("mcp")
}

/// The TLS config: the user's PEM cert/key, or a self-signed pair for
/// localhost/127.0.0.1 kept in $MOUSER_HOME/mcp/. Returns the config, the
/// certificate path and its SHA256 fingerprint (for pinning in clients).
pub fn tls_config(cert: Option<&str>, key: Option<&str>) -> Result<(Arc<rustls::ServerConfig>, PathBuf, String)> {
    let (cp, kp) = match (cert, key) {
        (Some(c), Some(k)) => (PathBuf::from(c), PathBuf::from(k)),
        (None, None) => {
            let (c, k) = (dir().join("cert.pem"), dir().join("key.pem"));
            if !c.is_file() || !k.is_file() {
                self_signed(&c, &k)?;
            }
            (c, k)
        }
        _ => bail!("set both mcp.server.cert and mcp.server.key, or neither"),
    };
    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(std::fs::File::open(&cp).with_context(|| format!("{}", cp.display()))?))
        .collect::<std::result::Result<_, _>>()?;
    let first = certs.first().context("no certificate in the PEM file")?.clone();
    let key = rustls_pemfile::private_key(&mut BufReader::new(std::fs::File::open(&kp).with_context(|| format!("{}", kp.display()))?))?
        .context("no private key in the PEM file")?;
    let cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_single_cert(certs, key)?;
    Ok((Arc::new(cfg), cp, fingerprint(&first)))
}

pub fn fingerprint(der: &[u8]) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(der).iter().map(|b| format!("{b:02X}")).collect::<Vec<_>>().join(":")
}

fn self_signed(c: &std::path::Path, k: &std::path::Path) -> Result<()> {
    let ck = rcgen::generate_simple_self_signed(vec!["localhost".into(), "127.0.0.1".into()])?;
    std::fs::create_dir_all(c.parent().context("dir")?)?;
    std::fs::write(c, ck.cert.pem())?;
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    o.open(k)?.write_all(ck.key_pair.serialize_pem().as_bytes())?;
    Ok(())
}

/// A tiny HTTP(S) client for the bridge and tests: POST `body` to /mcp.
/// Returns (status, `Mcp-Session-Id`, body).
pub fn post(
    port: u16,
    token: &str,
    session: Option<&str>,
    body: &str,
    tls: Option<Arc<rustls::ClientConfig>>,
) -> Result<(u16, Option<String>, String)> {
    let s = TcpStream::connect(("127.0.0.1", port))?;
    s.set_read_timeout(Some(Duration::from_secs(600)))?;
    let mut req = format!(
        "POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {token}\r\nContent-Type: application/json\r\nAccept: application/json, text/event-stream\r\nContent-Length: {}\r\n",
        body.len()
    );
    if let Some(id) = session {
        req.push_str(&format!("Mcp-Session-Id: {id}\r\n"));
    }
    req.push_str("\r\n");
    req.push_str(body);
    let mut raw = Vec::new();
    match tls {
        Some(cfg) => {
            let name = rustls::pki_types::ServerName::try_from("localhost")?.to_owned();
            let mut t = rustls::StreamOwned::new(rustls::ClientConnection::new(cfg, name)?, s);
            t.write_all(req.as_bytes())?;
            let _ = t.read_to_end(&mut raw);
        }
        None => {
            let mut s = s;
            s.write_all(req.as_bytes())?;
            s.read_to_end(&mut raw)?;
        }
    }
    let text = String::from_utf8_lossy(&raw).into_owned();
    let (head, body) = text.split_once("\r\n\r\n").context("bad HTTP reply")?;
    let status = head.split_whitespace().nth(1).and_then(|c| c.parse().ok()).unwrap_or(0);
    let sid =
        head.lines().find_map(|l| l.split_once(':').filter(|(k, _)| k.eq_ignore_ascii_case("mcp-session-id")).map(|(_, v)| v.trim().to_string()));
    Ok((status, sid, body.to_string()))
}

/// A client TLS config trusting exactly `cert_pem` (for the self-signed cert).
pub fn client_tls(cert_pem: &std::path::Path) -> Result<Arc<rustls::ClientConfig>> {
    let mut roots = rustls::RootCertStore::empty();
    for c in rustls_pemfile::certs(&mut BufReader::new(std::fs::File::open(cert_pem)?)) {
        roots.add(c?)?;
    }
    Ok(Arc::new(
        rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_root_certificates(roots)
            .with_no_client_auth(),
    ))
}

/// `mouser mcp serve --stdio`: newline-delimited JSON-RPC on stdin/stdout,
/// forwarded to the app's loopback endpoint with the token.
pub fn stdio_bridge(port: u16, token: &str, input: impl BufRead, mut out: impl Write) -> Result<()> {
    let mut session: Option<String> = None;
    for line in input.lines() {
        let line = line?;
        if line.trim().is_empty() {
            continue;
        }
        let id = serde_json::from_str::<Value>(&line).ok().and_then(|v| v.get("id").cloned());
        match post(port, token, session.as_deref(), &line, None) {
            Ok((_, sid, body)) => {
                if sid.is_some() {
                    session = sid;
                }
                if !body.trim().is_empty() {
                    writeln!(out, "{}", body.trim())?;
                }
            }
            Err(e) => {
                if let Some(id) = id {
                    writeln!(
                        out,
                        "{}",
                        err(&id, -32000, &format!("Mouser is not reachable on 127.0.0.1:{port} (is it running with [mcp.server] enabled?): {e}"))
                    )?;
                }
            }
        }
        out.flush()?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Fake;
    impl Tools for Fake {
        fn list(&self) -> Vec<Value> {
            vec![json!({"name": "echo", "description": "echo", "inputSchema": {"type": "object"}})]
        }
        fn call(&self, client: &str, _: &str, a: &Value) -> std::result::Result<Value, String> {
            if a["fail"] == true {
                return Err("nope".into());
            }
            Ok(json!({"client": client, "args": a}))
        }
    }

    fn start(tls: Option<Arc<rustls::ServerConfig>>) -> (u16, Arc<Server>) {
        let l = bind(0).unwrap();
        let port = l.local_addr().unwrap().port();
        let s = Server::new(Arc::new(Fake), "t0ken-t0ken-t0ken-t0ken-t0ken-t0".into(), "9.9");
        let s2 = s.clone();
        std::thread::spawn(move || serve(l, tls, s2));
        (port, s)
    }

    const TOK: &str = "t0ken-t0ken-t0ken-t0ken-t0ken-t0";

    fn init() -> String {
        json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"tester","version":"1"}}}).to_string()
    }

    fn session_flow(port: u16, tls: Option<Arc<rustls::ClientConfig>>) {
        let (st, sid, b) = post(port, TOK, None, &init(), tls.clone()).unwrap();
        assert_eq!(st, 200, "{b}");
        let v: Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["result"]["protocolVersion"], "2025-06-18");
        assert_eq!(v["result"]["serverInfo"]["name"], "mouser");
        let sid = sid.expect("session id");
        let (st, _, b) = post(port, TOK, Some(&sid), r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#, tls.clone()).unwrap();
        assert_eq!((st, b.as_str()), (202, ""));
        let (_, _, b) = post(port, TOK, Some(&sid), r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#, tls.clone()).unwrap();
        assert!(b.contains("\"echo\""), "{b}");
        let (_, _, b) = post(
            port,
            TOK,
            Some(&sid),
            r#"{"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"echo","arguments":{"x":1}}}"#,
            tls.clone(),
        )
        .unwrap();
        let v: Value = serde_json::from_str(&b).unwrap();
        assert_eq!(v["result"]["structuredContent"]["client"], "tester", "the session knows the client");
        let (_, _, b) = post(
            port,
            TOK,
            Some(&sid),
            r#"{"jsonrpc":"2.0","id":4,"method":"tools/call","params":{"name":"echo","arguments":{"fail":true}}}"#,
            tls.clone(),
        )
        .unwrap();
        assert!(b.contains("\"isError\":true"));
        let (_, _, b) = post(port, TOK, Some(&sid), r#"{"jsonrpc":"2.0","id":5,"method":"tools/call","params":{"name":"rm"}}"#, tls).unwrap();
        assert!(b.contains("-32602"));
    }

    #[test]
    fn http_auth_host_origin_and_session() {
        let (port, _) = start(None);
        session_flow(port, None);
        assert_eq!(post(port, "wrong", None, &init(), None).unwrap().0, 401);
        assert_eq!(post(port, "", None, &init(), None).unwrap().0, 401);
        // Raw requests: bad Host (DNS rebinding), foreign Origin, GET, bad JSON.
        let raw = |r: String| {
            let mut s = TcpStream::connect(("127.0.0.1", port)).unwrap();
            s.write_all(r.as_bytes()).unwrap();
            let mut o = String::new();
            s.read_to_string(&mut o).unwrap();
            o
        };
        let b = init();
        assert!(
            raw(format!("POST /mcp HTTP/1.1\r\nHost: evil.example:{port}\r\nAuthorization: Bearer {TOK}\r\nContent-Length: {}\r\n\r\n{b}", b.len()))
                .starts_with("HTTP/1.1 403")
        );
        assert!(raw(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: https://evil.example\r\nAuthorization: Bearer {TOK}\r\nContent-Length: {}\r\n\r\n{b}", b.len())).starts_with("HTTP/1.1 403"));
        assert!(raw(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://localhost:3000\r\nAuthorization: Bearer {TOK}\r\nContent-Length: {}\r\n\r\n{b}", b.len())).starts_with("HTTP/1.1 200"));
        assert!(raw(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nOrigin: http://localhost.evil.example\r\nAuthorization: Bearer {TOK}\r\nContent-Length: {}\r\n\r\n{b}", b.len())).starts_with("HTTP/1.1 403"));
        assert!(raw(format!("GET /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOK}\r\n\r\n")).starts_with("HTTP/1.1 405"));
        assert!(
            raw(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOK}\r\nContent-Length: 3\r\n\r\n{{x}}"))
                .starts_with("HTTP/1.1 400")
        );
        assert!(
            raw(format!("POST /other HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOK}\r\nContent-Length: 0\r\n\r\n"))
                .starts_with("HTTP/1.1 404")
        );
        assert!(
            raw(format!("POST /mcp HTTP/1.1\r\nHost: 127.0.0.1:{port}\r\nAuthorization: Bearer {TOK}\r\nContent-Length: 99999999\r\n\r\n"))
                .is_empty(),
            "oversized bodies are dropped"
        );
    }

    #[test]
    #[serial_test::serial]
    fn https_self_signed_and_pinned_client() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let (cfg, cert, fp) = tls_config(None, None).unwrap();
        assert_eq!(fp.len(), 32 * 3 - 1);
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(std::fs::metadata(dir().join("key.pem")).unwrap().permissions().mode() & 0o777, 0o600);
        }
        // Stable across restarts.
        assert_eq!(tls_config(None, None).unwrap().2, fp);
        // The user's own pair works the same.
        let (_, _, fp2) = tls_config(Some(cert.to_str().unwrap()), Some(dir().join("key.pem").to_str().unwrap())).unwrap();
        assert_eq!(fp2, fp);
        assert!(tls_config(Some("x"), None).is_err());
        let (port, _) = start(Some(cfg));
        session_flow(port, Some(client_tls(&cert).unwrap()));
        // A client that doesn't trust the cert can't talk to it.
        let other = tempfile::tempdir().unwrap();
        self_signed(&other.path().join("c.pem"), &other.path().join("k.pem")).unwrap();
        assert!(post(port, TOK, None, &init(), Some(client_tls(&other.path().join("c.pem")).unwrap())).is_err_or_empty());
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[test]
    #[serial_test::serial]
    fn the_mcp_client_over_http_and_https() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let (port, _) = start(None);
        let url = format!("http://127.0.0.1:{port}/mcp");
        let mut c = crate::mcp::Client::connect(&url, Some(TOK.into()), None).unwrap();
        assert_eq!(c.server.1, "9.9");
        assert_eq!(c.tools().unwrap()[0].name, "echo");
        let r = c.call("echo", json!({"x": 1})).unwrap();
        assert!(r.contains("\"x\"") && r.contains("mouser"), "{r}");
        assert!(c.call("echo", json!({"fail": true})).is_err());
        c.ping().unwrap();
        let e = crate::mcp::Client::connect(&url, Some("wrong".into()), None).err().unwrap().to_string();
        assert!(e.contains("refused"), "{e}");
        let (cfg, cert, _) = tls_config(None, None).unwrap();
        let (sport, _) = start(Some(cfg));
        for host in ["127.0.0.1", "localhost"] {
            let mut c = crate::mcp::Client::connect(&format!("https://{host}:{sport}/mcp"), Some(TOK.into()), Some(&cert)).unwrap();
            assert_eq!(c.tools().unwrap().len(), 1);
        }
        // Without the CA (public roots only) the self-signed cert is refused.
        assert!(crate::mcp::Client::connect(&format!("https://127.0.0.1:{sport}/mcp"), Some(TOK.into()), None).is_err());
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    trait ErrOrEmpty {
        fn is_err_or_empty(&self) -> bool;
    }
    impl ErrOrEmpty for Result<(u16, Option<String>, String)> {
        fn is_err_or_empty(&self) -> bool {
            self.as_ref().map_or(true, |r| r.0 == 0)
        }
    }

    #[test]
    fn stdio_bridge_with_the_mcp_client() {
        let (port, _) = start(None);
        let input = format!(
            "{}\n{}\n{}\n",
            init(),
            r#"{"jsonrpc":"2.0","method":"notifications/initialized"}"#,
            r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"echo","arguments":{"y":2}}}"#
        );
        let mut out = Vec::new();
        stdio_bridge(port, TOK, input.as_bytes(), &mut out).unwrap();
        let lines: Vec<Value> = String::from_utf8(out).unwrap().lines().map(|l| serde_json::from_str(l).unwrap()).collect();
        assert_eq!(lines.len(), 2, "no line for the notification");
        assert_eq!(lines[1]["result"]["structuredContent"]["client"], "tester", "the bridge keeps the session");
        // Not running: a JSON-RPC error, not a hang.
        let dead = bind(0).unwrap().local_addr().unwrap().port();
        let mut out = Vec::new();
        stdio_bridge(dead, TOK, format!("{}\n", init()).as_bytes(), &mut out).unwrap();
        assert!(String::from_utf8(out).unwrap().contains("not reachable"));
    }
}
