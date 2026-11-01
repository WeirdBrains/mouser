//! Streaming HTTP(S) POST for model APIs: headers out, SSE lines in, chunked or not.

use crate::mcp::{Target, tls_for};
use anyhow::{Context, Result, bail};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpStream, ToSocketAddrs};
use std::time::Duration;

trait Io: Read + Write {}
impl<T: Read + Write> Io for T {}

fn open(t: &Target, ca: Option<&std::path::Path>) -> Result<Box<dyn Io>> {
    let mut last = None;
    for addr in (t.host.as_str(), t.port).to_socket_addrs().with_context(|| format!("resolving {}", t.host))? {
        match TcpStream::connect_timeout(&addr, Duration::from_secs(10)) {
            Ok(s) => {
                s.set_read_timeout(Some(Duration::from_secs(300)))?;
                if !t.https {
                    return Ok(Box::new(s));
                }
                let name = rustls::pki_types::ServerName::try_from(t.host.clone())?;
                let c = rustls::ClientConnection::new(tls_for(ca)?, name)?;
                return Ok(Box::new(rustls::StreamOwned::new(c, s)));
            }
            Err(e) => last = Some(e),
        }
    }
    match last {
        Some(e) => Err(anyhow::Error::from(e).context(format!("connecting to {}", t.host_header()))),
        None => bail!("no address for {}", t.host),
    }
}

/// An HTTP error reply (status and body, trimmed; never includes our headers).
#[derive(Debug)]
pub struct HttpError {
    pub status: u16,
    pub body: String,
}

impl std::fmt::Display for HttpError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let msg = serde_json::from_str::<serde_json::Value>(&self.body)
            .ok()
            .and_then(|v| v["error"]["message"].as_str().or(v["error"].as_str()).or(v["message"].as_str()).map(String::from))
            .unwrap_or_else(|| self.body.chars().take(300).collect());
        write!(f, "HTTP {}: {}", self.status, msg.trim())
    }
}
impl std::error::Error for HttpError {}

/// POST JSON to `url` and call `line` for every body line (SSE); `line`
/// returns false to stop (cancel). Non-2xx replies become [`HttpError`].
pub fn post_lines(url: &str, headers: &[(&str, String)], body: &str, ca: Option<&std::path::Path>, line: &mut dyn FnMut(&str) -> bool) -> Result<()> {
    let t = Target::parse(url)?;
    let mut s = open(&t, ca)?;
    let mut req = format!(
        "POST {} HTTP/1.1\r\nHost: {}\r\nContent-Type: application/json\r\nAccept: text/event-stream, application/json\r\nUser-Agent: mouser/{}\r\nContent-Length: {}\r\nConnection: close\r\n",
        t.path,
        t.host_header(),
        env!("CARGO_PKG_VERSION"),
        body.len()
    );
    for (k, v) in headers {
        req.push_str(&format!("{k}: {v}\r\n"));
    }
    req.push_str("\r\n");
    s.write_all(req.as_bytes())?;
    s.write_all(body.as_bytes())?;
    s.flush()?;
    let mut r = BufReader::new(s);
    let mut status_line = String::new();
    r.read_line(&mut status_line)?;
    let status: u16 = status_line.split_whitespace().nth(1).and_then(|c| c.parse().ok()).context("bad HTTP reply")?;
    let mut chunked = false;
    loop {
        let mut h = String::new();
        if r.read_line(&mut h)? == 0 || h == "\r\n" || h == "\n" {
            break;
        }
        if let Some((k, v)) = h.split_once(':')
            && k.trim().eq_ignore_ascii_case("transfer-encoding")
            && v.trim().eq_ignore_ascii_case("chunked")
        {
            chunked = true;
        }
    }
    let mut body_reader: Box<dyn BufRead> = if chunked { Box::new(BufReader::new(Chunked { r, left: 0, done: false })) } else { Box::new(r) };
    if !(200..300).contains(&status) {
        let mut b = String::new();
        let _ = body_reader.take(64 * 1024).read_to_string(&mut b);
        return Err(HttpError { status, body: b.trim().to_string() }.into());
    }
    let mut l = String::new();
    loop {
        l.clear();
        match body_reader.read_line(&mut l) {
            Ok(0) => break,
            Ok(_) => {
                if !line(l.trim_end_matches(['\r', '\n'])) {
                    break;
                }
            }
            Err(e) if l.is_empty() => return Err(e.into()),
            Err(_) => break,
        }
    }
    Ok(())
}

/// Chunked transfer decoding over a reader.
struct Chunked<R: BufRead> {
    r: R,
    left: usize,
    done: bool,
}

impl<R: BufRead> Read for Chunked<R> {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        if self.done {
            return Ok(0);
        }
        if self.left == 0 {
            let mut size = String::new();
            self.r.read_line(&mut size)?;
            if size.trim().is_empty() {
                size.clear();
                self.r.read_line(&mut size)?;
            }
            let n = usize::from_str_radix(size.trim().split(';').next().unwrap_or(""), 16).unwrap_or(0);
            if n == 0 {
                self.done = true;
                return Ok(0);
            }
            self.left = n;
        }
        let want = buf.len().min(self.left);
        let got = self.r.read(&mut buf[..want])?;
        self.left -= got;
        if self.left == 0 {
            let mut crlf = [0u8; 2];
            let _ = self.r.read_exact(&mut crlf);
        }
        Ok(got)
    }
}

/// SSE: collects `data:` lines and hands complete events (event name, data) to `f`.
pub struct Sse {
    event: String,
    data: Vec<String>,
}

impl Sse {
    pub fn new() -> Self {
        Sse { event: String::new(), data: vec![] }
    }
    /// Feed one line; returns a finished event on a blank line.
    pub fn line(&mut self, l: &str) -> Option<(String, String)> {
        if l.is_empty() {
            if self.data.is_empty() {
                self.event.clear();
                return None;
            }
            let ev = (std::mem::take(&mut self.event), self.data.join("\n"));
            self.data.clear();
            return Some(ev);
        }
        if let Some(v) = l.strip_prefix("data:") {
            self.data.push(v.strip_prefix(' ').unwrap_or(v).to_string());
        } else if let Some(v) = l.strip_prefix("event:") {
            self.event = v.trim().to_string();
        }
        None
    }
}

impl Default for Sse {
    fn default() -> Self {
        Self::new()
    }
}

/// Join a base URL and a path without doubling slashes.
pub fn join(base: &str, path: &str) -> String {
    format!("{}/{}", base.trim_end_matches('/'), path.trim_start_matches('/'))
}
