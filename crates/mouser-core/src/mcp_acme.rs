//! Let's Encrypt (ACME, RFC 8555) certificates for the MCP server's HTTPS
//! listener (docs/mcp.md §Let's Encrypt).
//!
//! - Challenges: `tls-alpn-01` answered on the HTTPS port itself (the
//!   [`CertStore`] resolver hands the validator a challenge cert when it asks
//!   for ALPN `acme-tls/1`), `http-01` on an optional port-80 listener, and
//!   `dns-01` through a hook command or by hand (`mouser mcp cert renew`).
//! - Files: `$MOUSER_HOME/mcp/acme/` (account, cert, key, state), 0600.
//! - Renewal: [`RENEW_BEFORE`] before expiry; the new cert is swapped into the
//!   running listener without a restart. Failures keep the old cert and back
//!   off from one hour to a day ([`backoff`]), which also keeps well inside
//!   Let's Encrypt's rate limits (5 failed validations per name per hour, 5
//!   duplicate certificates per week).
//!
//! Pure Rust: instant-acme over hyper-rustls with ring; no OpenSSL.

use anyhow::{Context, Result, bail};
pub use instant_acme::RetryPolicy;
use instant_acme::{
    Account, AccountCredentials, AuthorizationStatus, ChallengeType, HttpClient, Identifier, LetsEncrypt, NewAccount, NewOrder, OrderStatus,
};
use rustls::server::{ClientHello, ResolvesServerCert};
use rustls::sign::CertifiedKey;
use std::collections::HashMap;
use std::io::{BufRead, BufReader, Read, Write};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

/// Renew this long before `notAfter`.
pub const RENEW_BEFORE: Duration = Duration::from_secs(30 * 86400);
/// ALPN protocol id of the tls-alpn-01 challenge (RFC 8737).
pub const ACME_TLS_ALPN: &[u8] = b"acme-tls/1";
/// Shortest and longest wait after a failed attempt.
pub const BACKOFF_MIN: Duration = Duration::from_secs(3600);
pub const BACKOFF_MAX: Duration = Duration::from_secs(86400);
/// A forced renewal is refused this soon after the last certificate.
pub const FORCE_MIN_AGE: Duration = Duration::from_secs(86400);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Challenge {
    TlsAlpn01,
    Http01,
    Dns01,
}

impl Challenge {
    pub const NAMES: [&'static str; 3] = ["tls-alpn-01", "http-01", "dns-01"];

    pub fn parse(s: &str) -> Result<Self> {
        Ok(match s {
            "tls-alpn-01" => Challenge::TlsAlpn01,
            "http-01" => Challenge::Http01,
            "dns-01" => Challenge::Dns01,
            o => bail!("mcp.server.tls.challenge: one of tls-alpn-01, http-01, dns-01 (not {o:?})"),
        })
    }

    pub fn as_str(self) -> &'static str {
        Challenge::NAMES[self as usize]
    }

    fn acme(self) -> ChallengeType {
        match self {
            Challenge::TlsAlpn01 => ChallengeType::TlsAlpn01,
            Challenge::Http01 => ChallengeType::Http01,
            Challenge::Dns01 => ChallengeType::Dns01,
        }
    }
}

/// What `[mcp.server.tls]` with `source = "acme"` asks for, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeConfig {
    pub domains: Vec<String>,
    pub email: Option<String>,
    pub directory: String,
    pub challenge: Challenge,
    pub http_port: u16,
    pub dns_hook: Option<String>,
}

/// Whether `d` is a public DNS name we may ask a CA for.
fn valid_domain(d: &str, wildcard_ok: bool) -> bool {
    let d = if wildcard_ok { d.strip_prefix("*.").unwrap_or(d) } else { d };
    let labels: Vec<&str> = d.split('.').collect();
    d.len() <= 253
        && labels.len() >= 2
        && d != "localhost"
        && !d.ends_with(".localhost")
        && d.parse::<std::net::IpAddr>().is_err()
        && !labels.last().unwrap().bytes().all(|b| b.is_ascii_digit())
        && labels.iter().all(|l| {
            !l.is_empty() && l.len() <= 63 && !l.starts_with('-') && !l.ends_with('-') && l.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-')
        })
}

impl AcmeConfig {
    pub fn from_prefs(t: &crate::storage::McpTlsPrefs) -> Result<Self> {
        let challenge = Challenge::parse(t.challenge.as_deref().unwrap_or("tls-alpn-01"))?;
        let domains: Vec<String> = t
            .domains
            .as_deref()
            .unwrap_or("")
            .split([',', ' '])
            .map(|d| d.trim().trim_end_matches('.').to_ascii_lowercase())
            .filter(|d| !d.is_empty())
            .collect();
        if domains.is_empty() {
            bail!("mcp.server.tls.domains: set the public DNS name(s) that point at this computer");
        }
        if domains.len() > 100 {
            bail!("mcp.server.tls.domains: at most 100 names");
        }
        for d in &domains {
            if !valid_domain(d, challenge == Challenge::Dns01) {
                bail!(
                    "mcp.server.tls.domains: {d:?} is not a public DNS name{}",
                    if d.starts_with("*.") { " (wildcards need challenge = \"dns-01\")" } else { "" }
                );
            }
        }
        let email = t.email.as_deref().map(str::trim).filter(|e| !e.is_empty()).map(String::from);
        if let Some(e) = &email
            && !(e.contains('@') && !e.contains(char::is_whitespace) && !e.starts_with('@') && !e.ends_with('@'))
        {
            bail!("mcp.server.tls.email: {e:?} is not an email address");
        }
        let directory = match t.directory.as_deref().map(str::trim).filter(|d| !d.is_empty()) {
            Some(d) if d.starts_with("https://") => d.to_string(),
            Some(d) => bail!("mcp.server.tls.directory: an https:// URL (not {d:?})"),
            None if t.staging => LetsEncrypt::Staging.url().to_string(),
            None => LetsEncrypt::Production.url().to_string(),
        };
        Ok(AcmeConfig {
            domains,
            email,
            directory,
            challenge,
            http_port: t.http_port.unwrap_or(80),
            dns_hook: t.dns_hook.clone().filter(|h| !h.trim().is_empty()),
        })
    }

    /// `production`, `staging` or the custom directory URL.
    pub fn ca_name(&self) -> &str {
        if self.directory == LetsEncrypt::Production.url() {
            "Let's Encrypt"
        } else if self.directory == LetsEncrypt::Staging.url() {
            "Let's Encrypt staging"
        } else {
            &self.directory
        }
    }
}

/// `$MOUSER_HOME/mcp/acme`.
pub fn default_dir() -> PathBuf {
    crate::mcp_server::dir().join("acme")
}

/// Write `data` to `path` atomically, readable by the owner only.
pub fn write_private(path: &Path, data: &[u8]) -> Result<()> {
    let dir = path.parent().context("no parent dir")?;
    std::fs::create_dir_all(dir)?;
    let tmp = path.with_extension("tmp");
    let mut o = std::fs::OpenOptions::new();
    o.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        o.mode(0o600);
    }
    {
        let mut f = o.open(&tmp).with_context(|| format!("{}", tmp.display()))?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            f.set_permissions(std::fs::Permissions::from_mode(0o600))?;
        }
        f.write_all(data)?;
        f.sync_all()?;
    }
    std::fs::rename(&tmp, path).with_context(|| format!("{}", path.display()))?;
    Ok(())
}

/// Renewal bookkeeping kept in `state.json`.
#[derive(Debug, Clone, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct State {
    /// Failed attempts since the last success.
    #[serde(default)]
    pub failures: u32,
    /// Unix seconds of the last failed attempt.
    #[serde(default)]
    pub last_failure: Option<u64>,
    #[serde(default)]
    pub last_error: Option<String>,
    /// Unix seconds of the last certificate issued.
    #[serde(default)]
    pub last_issued: Option<u64>,
    /// Names on the stored certificate (a change means a new certificate).
    #[serde(default)]
    pub domains: Vec<String>,
    /// Directory it came from (switching staging -> production reissues).
    #[serde(default)]
    pub directory: Option<String>,
}

fn unix(t: SystemTime) -> u64 {
    t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

fn from_unix(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(s)
}

/// Wait after `failures` failed attempts in a row: 1 h, 2 h, 4 h, … up to a day.
pub fn backoff(failures: u32) -> Duration {
    if failures == 0 {
        return Duration::ZERO;
    }
    BACKOFF_MIN.saturating_mul(1u32 << (failures - 1).min(16)).min(BACKOFF_MAX)
}

/// When the next attempt should run. `not_after`: the stored certificate's
/// expiry (None: no usable certificate, so now). Failures push it out by
/// [`backoff`] from the last failure, even when the cert is due.
pub fn next_attempt(cfg: &AcmeConfig, st: &State, not_after: Option<SystemTime>, now: SystemTime) -> SystemTime {
    let stale = st.domains != cfg.domains || st.directory.as_deref() != Some(cfg.directory.as_str());
    let due = match not_after {
        Some(na) if !stale => na.checked_sub(RENEW_BEFORE).unwrap_or(UNIX_EPOCH),
        _ => now,
    };
    match st.last_failure {
        Some(f) if st.failures > 0 => due.max(from_unix(f) + backoff(st.failures)),
        _ => due,
    }
}

/// One DER TLV: (tag, contents, rest).
fn tlv(b: &[u8]) -> Option<(u8, &[u8], &[u8])> {
    let (&tag, b) = b.split_first()?;
    let (&l, mut b) = b.split_first()?;
    let len = if l < 0x80 {
        l as usize
    } else {
        let n = (l & 0x7f) as usize;
        if n == 0 || n > 4 || b.len() < n {
            return None;
        }
        let v = b[..n].iter().fold(0usize, |a, &x| (a << 8) | x as usize);
        b = &b[n..];
        v
    };
    (b.len() >= len).then(|| (tag, &b[..len], &b[len..]))
}

fn der_time(tag: u8, v: &[u8]) -> Option<SystemTime> {
    let s = std::str::from_utf8(v).ok()?.strip_suffix('Z')?;
    let (year, rest) = match tag {
        0x17 if s.len() == 12 => {
            let y: i32 = s[..2].parse().ok()?;
            (if y >= 50 { 1900 + y } else { 2000 + y }, &s[2..])
        }
        0x18 if s.len() == 14 => (s[..4].parse().ok()?, &s[4..]),
        _ => return None,
    };
    let n = |i: usize| rest.get(i..i + 2)?.parse::<u8>().ok();
    let month = time::Month::try_from(n(0)?).ok()?;
    let date = time::Date::from_calendar_date(year, month, n(2)?).ok()?;
    let t = date.with_hms(n(4)?, n(6)?, n(8)?).ok()?.assume_utc();
    Some(UNIX_EPOCH + Duration::from_secs(u64::try_from(t.unix_timestamp()).ok()?))
}

/// `notAfter` of a DER certificate.
pub fn not_after(der: &[u8]) -> Option<SystemTime> {
    let (_, cert, _) = tlv(der)?;
    let (_, tbs, _) = tlv(cert)?;
    // [0] version (optional), then serial.
    let (tag, _, rest) = tlv(tbs)?;
    let rest = if tag == 0xa0 { tlv(rest)?.2 } else { rest };
    let (_, _, rest) = tlv(rest)?; // signature algorithm
    let (_, _, rest) = tlv(rest)?; // issuer
    let (_, validity, _) = tlv(rest)?;
    let (_, _, v) = tlv(validity)?; // notBefore
    let (tag, na, _) = tlv(v)?;
    der_time(tag, na)
}

/// A chain + key in PEM as a rustls signing pair, and the leaf's expiry.
pub fn certified_key(chain_pem: &[u8], key_pem: &[u8]) -> Result<(CertifiedKey, SystemTime)> {
    let certs: Vec<_> = rustls_pemfile::certs(&mut BufReader::new(chain_pem)).collect::<std::result::Result<_, _>>()?;
    let leaf = certs.first().context("no certificate in the chain")?;
    let na = not_after(leaf).context("can't read the certificate's expiry")?;
    let key = rustls_pemfile::private_key(&mut BufReader::new(key_pem))?.context("no private key")?;
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|e| anyhow::anyhow!("key: {e}"))?;
    Ok((CertifiedKey::new(certs, signer), na))
}

/// The live certificate, plus answers for challenges in progress. Swapping
/// [`CertStore::set`] affects the next handshake; open connections keep theirs.
#[derive(Default)]
pub struct CertStore {
    current: RwLock<Option<(Arc<CertifiedKey>, SystemTime)>>,
    /// tls-alpn-01: domain -> challenge certificate.
    alpn: Mutex<HashMap<String, Arc<CertifiedKey>>>,
    /// http-01: token -> key authorization.
    http01: Mutex<HashMap<String, String>>,
}

impl std::fmt::Debug for CertStore {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CertStore").field("not_after", &self.not_after()).finish_non_exhaustive()
    }
}

impl CertStore {
    pub fn set(&self, ck: CertifiedKey, not_after: SystemTime) {
        *self.current.write().unwrap() = Some((Arc::new(ck), not_after));
    }

    pub fn not_after(&self) -> Option<SystemTime> {
        self.current.read().unwrap().as_ref().map(|c| c.1)
    }

    /// Load `cert.pem` + `key.pem` from `dir` if they're there and valid.
    pub fn load(&self, dir: &Path) -> Result<bool> {
        let (c, k) = (dir.join("cert.pem"), dir.join("key.pem"));
        if !c.is_file() || !k.is_file() {
            return Ok(false);
        }
        let (ck, na) = certified_key(&std::fs::read(&c)?, &std::fs::read(&k)?)?;
        self.set(ck, na);
        Ok(true)
    }

    /// The http-01 answer for `token`.
    pub fn http01(&self, token: &str) -> Option<String> {
        self.http01.lock().unwrap().get(token).cloned()
    }
}

impl ResolvesServerCert for CertStore {
    fn resolve(&self, hello: ClientHello<'_>) -> Option<Arc<CertifiedKey>> {
        let acme = hello.alpn().is_some_and(|mut a| a.any(|p| p == ACME_TLS_ALPN));
        if acme {
            let name = hello.server_name()?.to_ascii_lowercase();
            return self.alpn.lock().unwrap().get(&name).cloned();
        }
        self.current.read().unwrap().as_ref().map(|c| c.0.clone())
    }
}

/// Server TLS config that takes its certificate from `store`.
pub fn server_config(store: Arc<CertStore>) -> Result<Arc<rustls::ServerConfig>> {
    let mut cfg = rustls::ServerConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
        .with_safe_default_protocol_versions()?
        .with_no_client_auth()
        .with_cert_resolver(store);
    cfg.alpn_protocols = vec![b"http/1.1".to_vec(), ACME_TLS_ALPN.to_vec()];
    Ok(Arc::new(cfg))
}

/// The self-signed cert carrying the acmeIdentifier extension (RFC 8737 §3).
fn alpn_cert(domain: &str, digest: &[u8]) -> Result<CertifiedKey> {
    let mut p = rcgen::CertificateParams::new(vec![domain.to_string()])?;
    p.custom_extensions = vec![rcgen::CustomExtension::new_acme_identifier(digest)];
    let kp = rcgen::KeyPair::generate()?;
    let cert = p.self_signed(&kp)?;
    let key = rustls::pki_types::PrivateKeyDer::Pkcs8(kp.serialize_der().into());
    let signer = rustls::crypto::ring::sign::any_supported_type(&key).map_err(|e| anyhow::anyhow!("{e}"))?;
    Ok(CertifiedKey::new(vec![cert.der().clone()], signer))
}

/// How dns-01 TXT records get published.
pub enum DnsPublisher<'a> {
    /// `dns_hook` from the config.
    Hook(&'a str),
    /// Ask a person (CLI): `(record name, value)`; returns once it's published.
    Manual(&'a (dyn Fn(&str, &str) -> Result<()> + Sync)),
}

fn run_hook(hook: &str, action: &str, domain: &str, txt: &str) -> Result<()> {
    let base = domain.strip_prefix("*.").unwrap_or(domain);
    #[cfg(windows)]
    let mut c = {
        let mut c = std::process::Command::new("cmd");
        c.arg("/C").arg(hook);
        c
    };
    #[cfg(not(windows))]
    let mut c = {
        let mut c = std::process::Command::new("sh");
        c.arg("-c").arg(hook);
        c
    };
    let st = c
        .env("MOUSER_ACME_ACTION", action)
        .env("MOUSER_ACME_DOMAIN", base)
        .env("MOUSER_ACME_NAME", format!("_acme-challenge.{base}"))
        .env("MOUSER_ACME_TXT", txt)
        .status()
        .with_context(|| format!("running dns_hook {hook:?}"))?;
    if !st.success() {
        bail!("dns_hook {action} for {domain} failed ({st})");
    }
    Ok(())
}

/// Load the account for `cfg.directory`, or register one.
async fn account(cfg: &AcmeConfig, dir: &Path, http: Option<Box<dyn HttpClient>>) -> Result<Account> {
    use sha2::Digest;
    let tag: String = sha2::Sha256::digest(cfg.directory.as_bytes())[..6].iter().map(|b| format!("{b:02x}")).collect();
    let path = dir.join(format!("account-{tag}.json"));
    let builder = |http: Option<Box<dyn HttpClient>>| -> Result<_> {
        Ok(match http {
            Some(h) => Account::builder_with_http(h),
            None => Account::builder()?,
        })
    };
    if let Ok(s) = std::fs::read(&path)
        && let Ok(creds) = serde_json::from_slice::<AccountCredentials>(&s)
    {
        return builder(http)?.from_credentials(creds).await.context("loading the ACME account");
    }
    let contact = cfg.email.as_ref().map(|e| format!("mailto:{e}"));
    let contacts: Vec<&str> = contact.iter().map(String::as_str).collect();
    let new = NewAccount { contact: &contacts, terms_of_service_agreed: true, only_return_existing: false };
    let (acct, creds) =
        builder(http)?.create(&new, cfg.directory.clone(), None).await.with_context(|| format!("registering with {}", cfg.ca_name()))?;
    write_private(&path, &serde_json::to_vec(&creds)?)?;
    Ok(acct)
}

/// Get a certificate for `cfg.domains`. Challenge answers are published in
/// `store` (tls-alpn-01, http-01) or via `dns`, and withdrawn afterwards.
/// Returns (chain PEM, key PEM).
pub async fn issue(
    cfg: &AcmeConfig,
    dir: &Path,
    store: &CertStore,
    http: Option<Box<dyn HttpClient>>,
    dns: Option<DnsPublisher<'_>>,
    retry: RetryPolicy,
) -> Result<(String, String)> {
    let acct = account(cfg, dir, http).await?;
    let ids: Vec<Identifier> = cfg.domains.iter().map(|d| Identifier::Dns(d.clone())).collect();
    let mut order = acct.new_order(&NewOrder::new(&ids)).await.context("new order")?;
    let mut published: Vec<(Challenge, String, String)> = Vec::new();
    let res = async {
        let mut auths = order.authorizations();
        while let Some(a) = auths.next().await {
            let mut a = a.context("authorization")?;
            match a.status {
                AuthorizationStatus::Pending => {}
                AuthorizationStatus::Valid => continue,
                s => bail!("authorization is {s:?}"),
            }
            let mut ch = a.challenge(cfg.challenge.acme()).with_context(|| format!("the CA doesn't offer {}", cfg.challenge.as_str()))?;
            let domain = ch.identifier().to_string();
            let ka = ch.key_authorization();
            match cfg.challenge {
                Challenge::TlsAlpn01 => {
                    let name = domain.trim_start_matches("*.").to_string();
                    store.alpn.lock().unwrap().insert(name.clone(), Arc::new(alpn_cert(&name, ka.digest().as_ref())?));
                    published.push((Challenge::TlsAlpn01, name, String::new()));
                }
                Challenge::Http01 => {
                    store.http01.lock().unwrap().insert(ch.token.clone(), ka.as_str().to_string());
                    published.push((Challenge::Http01, ch.token.clone(), String::new()));
                }
                Challenge::Dns01 => {
                    let txt = ka.dns_value();
                    let base = domain.trim_start_matches("*.").to_string();
                    match &dns {
                        Some(DnsPublisher::Hook(h)) => run_hook(h, "present", &domain, &txt)?,
                        Some(DnsPublisher::Manual(f)) => f(&format!("_acme-challenge.{base}"), &txt)?,
                        None => bail!("dns-01 needs mcp.server.tls.dns_hook, or run `mouser mcp cert renew` to add the TXT record by hand"),
                    }
                    published.push((Challenge::Dns01, domain.clone(), txt));
                }
            }
            ch.set_ready().await.context("challenge ready")?;
        }
        let st = order.poll_ready(&retry).await.context("waiting for validation")?;
        if st != OrderStatus::Ready {
            let why = order.state().error.as_ref().map(|p| p.to_string()).unwrap_or_default();
            bail!("validation failed: order is {st:?} {why}");
        }
        let key = rcgen::KeyPair::generate()?;
        let mut params = rcgen::CertificateParams::new(cfg.domains.clone())?;
        params.distinguished_name = rcgen::DistinguishedName::new();
        let csr = params.serialize_request(&key)?;
        order.finalize_csr(csr.der()).await.context("finalize")?;
        let chain = order.poll_certificate(&retry).await.context("downloading the certificate")?;
        Ok((chain, key.serialize_pem()))
    }
    .await;
    for (kind, k, txt) in published {
        match kind {
            Challenge::TlsAlpn01 => {
                store.alpn.lock().unwrap().remove(&k);
            }
            Challenge::Http01 => {
                store.http01.lock().unwrap().remove(&k);
            }
            Challenge::Dns01 => {
                if let Some(DnsPublisher::Hook(h)) = &dns
                    && let Err(e) = run_hook(h, "cleanup", &k, &txt)
                {
                    log::warn!("acme: {e:#}");
                }
            }
        }
    }
    res
}

pub fn load_state(dir: &Path) -> State {
    std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default()
}

fn save_state(dir: &Path, st: &State) -> Result<()> {
    write_private(&dir.join("state.json"), &serde_json::to_vec_pretty(st)?)
}

/// What happened on one [`renew_once`].
#[derive(Debug, PartialEq, Eq)]
pub enum Outcome {
    /// New certificate in place, valid until this time.
    Renewed(SystemTime),
    /// Not due before this time (and not forced).
    NotDue(SystemTime),
}

/// Renew if due (or `force`, which still waits out failure back-off and
/// refuses within [`FORCE_MIN_AGE`] of the last certificate). On success the
/// files are written and `store` swaps to the new cert; on failure the old
/// cert stays and the failure is recorded for back-off.
pub fn renew_once(
    cfg: &AcmeConfig,
    dir: &Path,
    store: &CertStore,
    force: bool,
    http: Option<Box<dyn HttpClient>>,
    dns: Option<DnsPublisher<'_>>,
    retry: RetryPolicy,
) -> Result<Outcome> {
    let now = SystemTime::now();
    let mut st = load_state(dir);
    let next = next_attempt(cfg, &st, store.not_after(), now);
    if next > now {
        let backing_off = st.failures > 0 && st.last_failure.is_some_and(|f| from_unix(f) + backoff(st.failures) > now);
        if !force || backing_off {
            if force {
                bail!(
                    "backing off after {} failed attempt(s); next try after {} (last error: {})",
                    st.failures,
                    fmt_time(next),
                    st.last_error.clone().unwrap_or_default()
                );
            }
            return Ok(Outcome::NotDue(next));
        }
        if st.last_issued.is_some_and(|t| from_unix(t) + FORCE_MIN_AGE > now) {
            bail!("a certificate was issued less than a day ago; not asking again (Let's Encrypt limits duplicate certificates)");
        }
    }
    let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
    let r = rt.block_on(issue(cfg, dir, store, http, dns, retry)).and_then(|(chain, key)| {
        let (ck, na) = certified_key(chain.as_bytes(), key.as_bytes())?;
        write_private(&dir.join("key.pem"), key.as_bytes())?;
        write_private(&dir.join("cert.pem"), chain.as_bytes())?;
        Ok((ck, na))
    });
    match r {
        Ok((ck, na)) => {
            store.set(ck, na);
            st = State { last_issued: Some(unix(now)), domains: cfg.domains.clone(), directory: Some(cfg.directory.clone()), ..State::default() };
            save_state(dir, &st)?;
            log::info!("acme: new certificate for {} from {}, valid until {}", cfg.domains.join(", "), cfg.ca_name(), fmt_time(na));
            Ok(Outcome::Renewed(na))
        }
        Err(e) => {
            st.failures = st.failures.saturating_add(1);
            st.last_failure = Some(unix(now));
            st.last_error = Some(format!("{e:#}"));
            save_state(dir, &st)?;
            log::warn!("acme: renewal failed ({} in a row), keeping the current certificate: {e:#}", st.failures);
            Err(e)
        }
    }
}

/// `2026-12-01 14:00 UTC`.
pub fn fmt_time(t: SystemTime) -> String {
    let s = i64::try_from(unix(t)).unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(s)
        .map(|d| format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", d.year(), d.month() as u8, d.day(), d.hour(), d.minute()))
        .unwrap_or_default()
}

/// File the CLI drops to ask the running app to renew now.
pub fn trigger_path(dir: &Path) -> PathBuf {
    dir.join("renew-now")
}

/// Background renewal for the running app: loads the stored cert, then
/// renews when due, checking every minute for [`trigger_path`].
pub fn spawn_renewer(cfg: AcmeConfig, dir: PathBuf, store: Arc<CertStore>) -> std::thread::JoinHandle<()> {
    std::thread::Builder::new()
        .name("mcp-acme".into())
        .spawn(move || {
            if let Err(e) = store.load(&dir) {
                log::warn!("acme: stored certificate unusable, getting a new one: {e:#}");
            }
            loop {
                let force = std::fs::remove_file(trigger_path(&dir)).is_ok();
                let dns = cfg.dns_hook.as_deref().map(DnsPublisher::Hook);
                let wait = match renew_once(&cfg, &dir, &store, force, None, dns, RetryPolicy::new().timeout(Duration::from_secs(120))) {
                    Ok(Outcome::NotDue(t)) => t.duration_since(SystemTime::now()).unwrap_or_default(),
                    Ok(Outcome::Renewed(_)) | Err(_) => Duration::ZERO,
                };
                // Sleep until due, waking each minute for a trigger.
                let until = SystemTime::now() + wait.max(Duration::from_secs(60));
                while SystemTime::now() < until && !trigger_path(&dir).exists() {
                    std::thread::sleep(
                        Duration::from_secs(60).min(until.duration_since(SystemTime::now()).unwrap_or_default()).max(Duration::from_millis(10)),
                    );
                }
            }
        })
        .expect("spawn mcp-acme")
}

/// Serve http-01 answers on `l` (`GET /.well-known/acme-challenge/<token>`);
/// everything else is 404. Never serves MCP.
pub fn serve_http01(l: TcpListener, store: Arc<CertStore>) {
    for s in l.incoming().flatten() {
        let store = store.clone();
        std::thread::spawn(move || {
            let mut s = s;
            let _ = s.set_read_timeout(Some(Duration::from_secs(10)));
            let mut line = String::new();
            let mut r = BufReader::new((&s).take(8192));
            if r.read_line(&mut line).is_err() {
                return;
            }
            let path = line.split(' ').nth(1).unwrap_or("");
            let ans = path.strip_prefix("/.well-known/acme-challenge/").filter(|_| line.starts_with("GET ")).and_then(|t| store.http01(t));
            let resp = match ans {
                Some(a) => format!(
                    "HTTP/1.1 200 OK\r\nContent-Type: application/octet-stream\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{a}",
                    a.len()
                ),
                None => "HTTP/1.1 404 Not Found\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_string(),
            };
            let _ = s.write_all(resp.as_bytes());
        });
    }
}

/// TLS handshakes only, for answering tls-alpn-01 while Mouser isn't running
/// (`mouser mcp cert renew`): no HTTP, no MCP.
pub fn serve_challenge_only(l: TcpListener, store: Arc<CertStore>) {
    let Ok(tls) = server_config(store) else { return };
    for c in l.incoming().flatten() {
        let _ = c.set_read_timeout(Some(Duration::from_secs(10)));
        if let Ok(conn) = rustls::ServerConnection::new(tls.clone()) {
            let mut t = rustls::StreamOwned::new(conn, c);
            let _ = t.conn.complete_io(&mut t.sock);
            t.conn.send_close_notify();
            let _ = t.conn.complete_io(&mut t.sock);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::McpTlsPrefs;
    use bytes::Bytes;
    use http_body_util::{BodyExt, Full};
    use instant_acme::{BodyWrapper, BytesResponse};
    use std::future::Future;
    use std::pin::Pin;

    const DOMAIN: &str = "mcp.example.test";

    fn prefs(domains: &str) -> McpTlsPrefs {
        McpTlsPrefs { source: Some("acme".into()), domains: Some(domains.into()), ..Default::default() }
    }

    fn cfg() -> AcmeConfig {
        AcmeConfig { directory: "https://ca.test/dir".into(), ..AcmeConfig::from_prefs(&prefs(DOMAIN)).unwrap() }
    }

    #[test]
    fn config_defaults_to_production_and_checks_names() {
        let c = AcmeConfig::from_prefs(&prefs("MCP.Example.com., b.example.com")).unwrap();
        assert_eq!(c.domains, ["mcp.example.com", "b.example.com"]);
        assert_eq!(c.directory, LetsEncrypt::Production.url());
        assert_eq!((c.challenge, c.http_port, c.email.clone()), (Challenge::TlsAlpn01, 80, None));
        let s = AcmeConfig::from_prefs(&McpTlsPrefs { staging: true, email: Some("me@example.com".into()), ..prefs("a.example.com") }).unwrap();
        assert_eq!((s.directory.as_str(), s.email.as_deref()), (LetsEncrypt::Staging.url(), Some("me@example.com")));
        assert_eq!(s.ca_name(), "Let's Encrypt staging");
        let d = AcmeConfig::from_prefs(&McpTlsPrefs { directory: Some("https://ca.example/dir".into()), staging: true, ..prefs("a.example.com") })
            .unwrap();
        assert_eq!(d.directory, "https://ca.example/dir");
        for bad in ["", "localhost", "127.0.0.1", "box.localhost", "nodots", "a..b.com", "-a.com", "*.example.com", "a.b.123"] {
            assert!(AcmeConfig::from_prefs(&prefs(bad)).is_err(), "{bad}");
        }
        let w = AcmeConfig::from_prefs(&McpTlsPrefs { challenge: Some("dns-01".into()), ..prefs("*.example.com") }).unwrap();
        assert_eq!(w.challenge, Challenge::Dns01);
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { email: Some("nope".into()), ..prefs("a.example.com") }).is_err());
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { directory: Some("http://ca".into()), ..prefs("a.example.com") }).is_err());
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { challenge: Some("dns".into()), ..prefs("a.example.com") }).is_err());
        for n in Challenge::NAMES {
            assert_eq!(Challenge::parse(n).unwrap().as_str(), n);
        }
    }

    #[test]
    fn renewal_schedule_and_backoff() {
        let h = Duration::from_secs(3600);
        assert_eq!([0, 1, 2, 3, 5, 6, 40].map(backoff), [Duration::ZERO, h, 2 * h, 4 * h, 16 * h, 24 * h, 24 * h]);
        let c = cfg();
        let now = UNIX_EPOCH + Duration::from_secs(2_000_000_000);
        let na = now + Duration::from_secs(60 * 86400);
        let ok = State { domains: c.domains.clone(), directory: Some(c.directory.clone()), ..State::default() };
        assert_eq!(next_attempt(&c, &ok, Some(na), now), na - RENEW_BEFORE);
        assert_eq!(next_attempt(&c, &ok, None, now), now, "no cert: now");
        // Names or CA changed: a new certificate now.
        assert_eq!(next_attempt(&c, &State { domains: vec!["x.example.com".into()], ..ok.clone() }, Some(na), now), now);
        assert_eq!(next_attempt(&c, &State { directory: Some("https://other/dir".into()), ..ok.clone() }, Some(na), now), now);
        // Failures wait out the back-off even when the cert is overdue.
        let failed = State { failures: 3, last_failure: Some(unix(now)), ..ok.clone() };
        assert_eq!(next_attempt(&c, &failed, Some(now), now), now + 4 * h);
        assert_eq!(next_attempt(&c, &failed, Some(na), now), na - RENEW_BEFORE, "later of the two");
    }

    fn cert_pem(name: &str, days: u64) -> (String, String, Vec<u8>) {
        let mut p = rcgen::CertificateParams::new(vec![name.to_string()]).unwrap();
        p.not_after = time::OffsetDateTime::now_utc() + time::Duration::days(days as i64);
        let kp = rcgen::KeyPair::generate().unwrap();
        let c = p.self_signed(&kp).unwrap();
        (c.pem(), kp.serialize_pem(), c.der().to_vec())
    }

    #[test]
    fn not_after_is_read_from_der() {
        let mut p = rcgen::CertificateParams::new(vec![DOMAIN.to_string()]).unwrap();
        p.not_after = time::macros::datetime!(2031-05-06 12:34:56 UTC);
        let c = p.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap();
        assert_eq!(not_after(c.der()), Some(UNIX_EPOCH + Duration::from_secs(1_935_837_296)));
        let mut p = rcgen::CertificateParams::new(vec![DOMAIN.to_string()]).unwrap();
        p.not_after = time::macros::datetime!(2051-01-02 00:00:00 UTC); // GeneralizedTime
        let c = p.self_signed(&rcgen::KeyPair::generate().unwrap()).unwrap();
        assert_eq!(fmt_time(not_after(c.der()).unwrap()), "2051-01-02 00:00 UTC");
        assert_eq!(not_after(b"\x30\x03\x02\x01"), None);
    }

    /// Handshake with `alpn` and SNI `DOMAIN`, trusting only `root`; the peer's leaf.
    fn handshake(port: u16, root: &[u8], alpn: &[u8]) -> std::result::Result<Vec<u8>, String> {
        let mut roots = rustls::RootCertStore::empty();
        roots.add(rustls::pki_types::CertificateDer::from(root.to_vec())).unwrap();
        let mut cc = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .with_root_certificates(roots)
            .with_no_client_auth();
        cc.alpn_protocols = vec![alpn.to_vec()];
        let name = rustls::pki_types::ServerName::try_from(DOMAIN).unwrap().to_owned();
        let mut conn = rustls::ClientConnection::new(Arc::new(cc), name).unwrap();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        while conn.is_handshaking() {
            conn.complete_io(&mut s).map_err(|e| e.to_string())?;
        }
        Ok(conn.peer_certificates().unwrap()[0].to_vec())
    }

    /// The validator's view: any certificate, we only inspect it.
    #[derive(Debug)]
    struct AnyCert;

    impl rustls::client::danger::ServerCertVerifier for AnyCert {
        fn verify_server_cert(
            &self,
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &[rustls::pki_types::CertificateDer<'_>],
            _: &rustls::pki_types::ServerName<'_>,
            _: &[u8],
            _: rustls::pki_types::UnixTime,
        ) -> std::result::Result<rustls::client::danger::ServerCertVerified, rustls::Error> {
            Ok(rustls::client::danger::ServerCertVerified::assertion())
        }
        // webpki refuses the critical acmeIdentifier extension; the real
        // validator only checks the extension, so this one does too.
        fn verify_tls12_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        // webpki refuses the critical acmeIdentifier extension; the real
        // validator only checks the extension, so this one does too.
        fn verify_tls13_signature(
            &self,
            _: &[u8],
            _: &rustls::pki_types::CertificateDer<'_>,
            _: &rustls::DigitallySignedStruct,
        ) -> std::result::Result<rustls::client::danger::HandshakeSignatureValid, rustls::Error> {
            Ok(rustls::client::danger::HandshakeSignatureValid::assertion())
        }
        fn supported_verify_schemes(&self) -> Vec<rustls::SignatureScheme> {
            rustls::crypto::ring::default_provider().signature_verification_algorithms.supported_schemes()
        }
    }

    /// Connect like the tls-alpn-01 validator; true if the cert for DOMAIN has the acmeIdentifier.
    fn alpn_answer_ok(port: u16) -> bool {
        let mut cc = rustls::ClientConfig::builder_with_provider(Arc::new(rustls::crypto::ring::default_provider()))
            .with_safe_default_protocol_versions()
            .unwrap()
            .dangerous()
            .with_custom_certificate_verifier(Arc::new(AnyCert))
            .with_no_client_auth();
        cc.alpn_protocols = vec![ACME_TLS_ALPN.to_vec()];
        let name = rustls::pki_types::ServerName::try_from(DOMAIN).unwrap().to_owned();
        let mut conn = rustls::ClientConnection::new(Arc::new(cc), name).unwrap();
        let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
        while conn.is_handshaking() {
            if conn.complete_io(&mut s).is_err() {
                return false;
            }
        }
        let oid = [0x06, 0x08, 0x2b, 0x06, 0x01, 0x05, 0x05, 0x07, 0x01, 0x1f];
        conn.alpn_protocol() == Some(ACME_TLS_ALPN) && conn.peer_certificates().is_some_and(|c| c[0].windows(oid.len()).any(|w| w == oid))
    }

    fn listen(store: Arc<CertStore>) -> u16 {
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        std::thread::spawn(move || serve_challenge_only(l, store));
        port
    }

    #[test]
    fn hot_swap_takes_effect_on_the_next_handshake() {
        let store = Arc::new(CertStore::default());
        let port = listen(store.clone());
        let (a, ak, ader) = cert_pem(DOMAIN, 90);
        let (b, bk, bder) = cert_pem(DOMAIN, 90);
        let (ck, na) = certified_key(a.as_bytes(), ak.as_bytes()).unwrap();
        store.set(ck, na);
        assert_eq!(handshake(port, &ader, b"http/1.1").unwrap(), ader);
        let (ck, na) = certified_key(b.as_bytes(), bk.as_bytes()).unwrap();
        store.set(ck, na);
        assert_eq!(store.not_after(), Some(na));
        assert!(handshake(port, &ader, b"http/1.1").is_err(), "old cert no longer served");
        assert_eq!(handshake(port, &bder, b"http/1.1").unwrap(), bder);
    }

    /// An in-memory ACME CA: directory, nonce, account, one order with one
    /// authorization, finalize and download. On the challenge it validates
    /// tls-alpn-01 by connecting to `tls_port` like Let's Encrypt does.
    #[derive(Default)]
    struct Ca {
        tls_port: u16,
        fail: bool,
        ready: bool,
        finalized: bool,
        saw_alpn: bool,
        csr_ok: bool,
        orders: u32,
        cert: String,
    }

    #[derive(Clone)]
    struct Mock(Arc<Mutex<Ca>>);

    fn b64url(s: &str) -> Vec<u8> {
        let mut t: String = s
            .chars()
            .map(|c| match c {
                '-' => '+',
                '_' => '/',
                c => c,
            })
            .collect();
        while !t.len().is_multiple_of(4) {
            t.push('=');
        }
        crate::graphics::base64_decode(t.as_bytes()).unwrap()
    }

    fn payload(body: &[u8]) -> serde_json::Value {
        let v: serde_json::Value = serde_json::from_slice(body).unwrap_or_default();
        let p = v["payload"].as_str().unwrap_or("");
        if p.is_empty() { serde_json::Value::Null } else { serde_json::from_slice(&b64url(p)).unwrap() }
    }

    impl HttpClient for Mock {
        fn request(
            &self,
            req: http::Request<BodyWrapper<Bytes>>,
        ) -> Pin<Box<dyn Future<Output = std::result::Result<BytesResponse, instant_acme::Error>> + Send>> {
            let ca = self.0.clone();
            Box::pin(async move {
                let path = req.uri().path().to_string();
                let body = req.into_body().collect().await.unwrap().to_bytes();
                let b = "https://ca.test";
                let order = |st: &str, ca: &Ca| {
                    serde_json::json!({"status": st, "authorizations": [format!("{b}/authz/1")], "finalize": format!("{b}/finalize/1"),
                        "certificate": if ca.finalized { Some(format!("{b}/cert/1")) } else { None },
                        "error": if st == "invalid" { Some(serde_json::json!({"type": "urn:ietf:params:acme:error:connection", "detail": "connection refused"})) } else { None }})
                };
                let (status, loc, ctype, out): (u16, Option<String>, &str, String) = {
                    let mut ca = ca.lock().unwrap();
                    let st = if ca.finalized {
                        "valid"
                    } else if ca.ready {
                        "ready"
                    } else {
                        "pending"
                    };
                    match path.as_str() {
                        "/dir" => (200, None, "application/json", serde_json::json!({"newNonce": format!("{b}/nonce"), "newAccount": format!("{b}/acct"), "newOrder": format!("{b}/order")}).to_string()),
                        "/nonce" => (200, None, "text/plain", String::new()),
                        "/acct" => (201, Some(format!("{b}/acct/1")), "application/json", r#"{"status":"valid"}"#.into()),
                        "/order" => {
                            ca.orders += 1;
                            (201, Some(format!("{b}/order/1")), "application/json", order("pending", &ca).to_string())
                        }
                        "/order/1" if ca.fail => (200, None, "application/json", order("invalid", &ca).to_string()),
                        "/order/1" => (200, None, "application/json", order(st, &ca).to_string()),
                        "/authz/1" => (
                            200,
                            None,
                            "application/json",
                            serde_json::json!({"identifier": {"type": "dns", "value": DOMAIN}, "status": if ca.ready { "valid" } else { "pending" },
                                "challenges": [
                                    {"type": "tls-alpn-01", "url": format!("{b}/chall/alpn"), "token": "tok-alpn", "status": "pending"},
                                    {"type": "http-01", "url": format!("{b}/chall/http"), "token": "tok-http", "status": "pending"},
                                    {"type": "dns-01", "url": format!("{b}/chall/dns"), "token": "tok-dns", "status": "pending"}]})
                            .to_string(),
                        ),
                        "/chall/alpn" => {
                            // Validate like the CA: ALPN acme-tls/1, SNI = the name, and the
                            // cert must carry id-pe-acmeIdentifier (1.3.6.1.5.5.7.1.31).
                            if !ca.fail {
                                let port = ca.tls_port;
                                let ok = std::thread::spawn(move || alpn_answer_ok(port)).join().unwrap();
                                ca.saw_alpn = ok;
                                ca.ready = ok;
                            }
                            (200, None, "application/json", serde_json::json!({"type": "tls-alpn-01", "url": format!("{b}/chall/alpn"), "token": "tok-alpn", "status": "processing"}).to_string())
                        }
                        "/chall/dns" => {
                            ca.ready = !ca.fail;
                            (200, None, "application/json", serde_json::json!({"type": "dns-01", "url": format!("{b}/chall/dns"), "token": "tok-dns", "status": "processing"}).to_string())
                        }
                        "/finalize/1" => {
                            let csr = b64url(payload(&body)["csr"].as_str().unwrap_or(""));
                            ca.csr_ok = csr.windows(DOMAIN.len()).any(|w| w == DOMAIN.as_bytes());
                            ca.finalized = true;
                            (200, None, "application/json", order("valid", &ca).to_string())
                        }
                        "/cert/1" => (200, None, "application/pem-certificate-chain", ca.cert.clone()),
                        p => (404, None, "application/problem+json", format!(r#"{{"type":"urn:ietf:params:acme:error:malformed","detail":"no {p}"}}"#)),
                    }
                };
                let mut r = http::Response::builder().status(status).header("Replay-Nonce", "nonce-1").header("Content-Type", ctype);
                if let Some(l) = loc {
                    r = r.header("Location", l);
                }
                Ok(BytesResponse::from(r.body(Full::new(Bytes::from(out))).unwrap()))
            })
        }
    }

    fn mock(tls_port: u16, fail: bool) -> (Mock, Box<dyn HttpClient>) {
        let (cert, _, _) = cert_pem(DOMAIN, 90);
        let m = Mock(Arc::new(Mutex::new(Ca { tls_port, fail, cert, ..Ca::default() })));
        (m.clone(), Box::new(m))
    }

    fn fast() -> RetryPolicy {
        RetryPolicy::new().initial_delay(Duration::from_millis(5)).timeout(Duration::from_secs(5))
    }

    #[test]
    fn issue_over_tls_alpn_saves_swaps_and_then_waits() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(CertStore::default());
        let port = listen(store.clone());
        let c = AcmeConfig { directory: "https://ca.test/dir".into(), ..cfg() };
        let (m, http) = mock(port, false);
        let out = renew_once(&c, dir.path(), &store, false, Some(http), None, fast()).unwrap();
        let ca = m.0.lock().unwrap();
        assert!(ca.saw_alpn && ca.csr_ok && ca.finalized, "validated over acme-tls/1 and the CSR names {DOMAIN}");
        let Outcome::Renewed(na) = out else { panic!("{out:?}") };
        assert!(na > SystemTime::now() + Duration::from_secs(89 * 86400));
        assert_eq!(store.not_after(), Some(na), "swapped in");
        assert!(store.alpn.lock().unwrap().is_empty(), "challenge cert withdrawn");
        for f in ["cert.pem", "key.pem", "state.json"] {
            let p = dir.path().join(f);
            assert!(p.is_file(), "{f}");
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                assert_eq!(std::fs::metadata(&p).unwrap().permissions().mode() & 0o777, 0o600, "{f}");
            }
        }
        assert!(std::fs::read_dir(dir.path()).unwrap().flatten().any(|e| e.file_name().to_string_lossy().starts_with("account-")));
        let st = load_state(dir.path());
        assert_eq!((st.failures, st.domains.clone()), (0, vec![DOMAIN.to_string()]));
        drop(ca);
        // A fresh process loads the stored cert and isn't due for 60 days.
        let again = CertStore::default();
        assert!(again.load(dir.path()).unwrap());
        let (_, http) = mock(port, false);
        assert_eq!(renew_once(&c, dir.path(), &again, false, Some(http), None, fast()).unwrap(), Outcome::NotDue(na - RENEW_BEFORE));
        // --force right after issuing is refused (duplicate-certificate limit).
        let (m2, http) = mock(port, false);
        assert!(renew_once(&c, dir.path(), &again, true, Some(http), None, fast()).is_err());
        assert_eq!(m2.0.lock().unwrap().orders, 0, "nothing asked of the CA");
    }

    #[test]
    fn failure_keeps_the_old_cert_and_backs_off() {
        let dir = tempfile::tempdir().unwrap();
        let store = Arc::new(CertStore::default());
        let (old, ok, _) = cert_pem(DOMAIN, 10); // due: under 30 days left
        let (ck, old_na) = certified_key(old.as_bytes(), ok.as_bytes()).unwrap();
        store.set(ck, old_na);
        let c = cfg();
        let (_, http) = mock(0, true);
        assert!(renew_once(&c, dir.path(), &store, false, Some(http), None, fast()).is_err());
        assert_eq!(store.not_after(), Some(old_na), "old cert still served");
        let st = load_state(dir.path());
        assert_eq!(st.failures, 1);
        assert!(st.last_error.as_deref().unwrap_or("").contains("connection refused"), "{:?}", st.last_error);
        // Within the back-off nothing is attempted, forced or not.
        let (m, http) = mock(0, false);
        let Outcome::NotDue(t) = renew_once(&c, dir.path(), &store, false, Some(http), None, fast()).unwrap() else { panic!() };
        assert!(t > SystemTime::now() + Duration::from_secs(3500));
        let (_, http) = mock(0, false);
        assert!(renew_once(&c, dir.path(), &store, true, Some(http), None, fast()).unwrap_err().to_string().contains("backing off"));
        assert_eq!(m.0.lock().unwrap().orders, 0);
    }

    #[test]
    fn http01_answers_only_its_tokens() {
        let store = Arc::new(CertStore::default());
        store.http01.lock().unwrap().insert("tok".into(), "tok.thumb".into());
        let l = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = l.local_addr().unwrap().port();
        let s2 = store.clone();
        std::thread::spawn(move || serve_http01(l, s2));
        let get = |path: &str| {
            let mut s = std::net::TcpStream::connect(("127.0.0.1", port)).unwrap();
            write!(s, "GET {path} HTTP/1.1\r\nHost: x\r\n\r\n").unwrap();
            let mut o = String::new();
            s.read_to_string(&mut o).unwrap();
            o
        };
        let r = get("/.well-known/acme-challenge/tok");
        assert!(r.starts_with("HTTP/1.1 200") && r.ends_with("\r\n\r\ntok.thumb"), "{r}");
        assert!(get("/.well-known/acme-challenge/other").starts_with("HTTP/1.1 404"));
        assert!(get("/mcp").starts_with("HTTP/1.1 404"));
    }

    #[test]
    fn dns01_publishes_the_txt_record() {
        let dir = tempfile::tempdir().unwrap();
        let store = CertStore::default();
        let c = AcmeConfig { challenge: Challenge::Dns01, ..cfg() };
        let (_, http) = mock(0, false);
        let e = renew_once(&c, dir.path(), &store, false, Some(http), None, fast()).unwrap_err();
        assert!(format!("{e:#}").contains("dns_hook"), "{e:#}");
        let seen = Mutex::new(Vec::new());
        let manual = |name: &str, txt: &str| -> Result<()> {
            seen.lock().unwrap().push((name.to_string(), txt.to_string()));
            Ok(())
        };
        let dir = tempfile::tempdir().unwrap();
        let (m, http) = mock(0, false);
        assert!(matches!(
            renew_once(&c, dir.path(), &store, false, Some(http), Some(DnsPublisher::Manual(&manual)), fast()).unwrap(),
            Outcome::Renewed(_)
        ));
        let seen = seen.into_inner().unwrap();
        assert_eq!(seen.len(), 1);
        assert_eq!(seen[0].0, format!("_acme-challenge.{DOMAIN}"));
        assert_eq!(seen[0].1.len(), 43, "base64url SHA-256");
        assert!(m.0.lock().unwrap().finalized);
    }

    #[cfg(unix)]
    #[test]
    fn dns_hook_gets_the_record_in_its_environment() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("hook.txt");
        let hook = format!("echo \"$MOUSER_ACME_ACTION $MOUSER_ACME_NAME $MOUSER_ACME_TXT\" >> {}", out.display());
        run_hook(&hook, "present", "*.example.com", "abc").unwrap();
        run_hook(&hook, "cleanup", "example.com", "abc").unwrap();
        assert_eq!(std::fs::read_to_string(&out).unwrap(), "present _acme-challenge.example.com abc\ncleanup _acme-challenge.example.com abc\n");
        assert!(run_hook("exit 3", "present", "example.com", "x").is_err());
    }
}
