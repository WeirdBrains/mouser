//! Let's Encrypt (ACME, RFC 8555) certificates for the MCP server's HTTPS
//! listener (docs/mcp.md §Let's Encrypt), on [weirdnet-tls](https://docs.rs/weirdnet-tls).
//!
//! weirdnet-tls does the ACME work: the account, orders, the `tls-alpn-01`,
//! `http-01` and `dns-01` answers, the SNI [`Resolver`] the listener uses,
//! renewal from ACME Renewal Information (else two thirds into the lifetime),
//! failure back-off from one hour to a day, and the hot swap of a new
//! certificate. Mouser keeps what is Mouser's:
//!
//! - reading `[mcp.server.tls]` ([`AcmeConfig::from_prefs`]);
//! - the store in `$MOUSER_HOME/mcp/acme/` (a [`FileStore`], files 0600), one
//!   certificate set per CA so switching staging to production gets a new
//!   certificate, and where the files of Mouser 1.7 and earlier were
//!   ([`legacy`], imported with [`Acme::import`]);
//! - the `MOUSER_ACME_` prefix for `dns_hook` variables (next to weirdnet-tls's
//!   `WEIRDNET_TLS_*`);
//! - the `renew-now` file `mouser mcp cert renew` drops for the running app
//!   ([`trigger_path`]).

use anyhow::{Result, bail};
use std::net::TcpListener;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use weirdnet_tls::instant_acme::HttpClient;
use weirdnet_tls::renew::Persisted;
pub use weirdnet_tls::renew::Schedule;
use weirdnet_tls::store::{self, Legacy};
use weirdnet_tls::{Acme, CertInfo, Directory, FileStore, HookDns};
pub use weirdnet_tls::{ChallengeKind as Challenge, DnsChallenge, DnsRecord, ManualDns, Outcome, Resolver, Status};

/// The `mcp.server.tls.challenge` values.
pub const CHALLENGES: [&str; 3] = ["tls-alpn-01", "http-01", "dns-01"];
/// How often the running app looks for [`trigger_path`].
pub const TRIGGER_POLL: Duration = Duration::from_secs(60);

/// `tls-alpn-01`, `http-01` or `dns-01`.
pub fn parse_challenge(s: &str) -> Result<Challenge> {
    Ok(match s {
        "tls-alpn-01" => Challenge::TlsAlpn01,
        "http-01" => Challenge::Http01,
        "dns-01" => Challenge::Dns01,
        o => bail!("mcp.server.tls.challenge: one of tls-alpn-01, http-01, dns-01 (not {o:?})"),
    })
}

/// What `[mcp.server.tls]` with `source = "acme"` asks for, checked.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AcmeConfig {
    pub domains: Vec<String>,
    pub email: Option<String>,
    /// The directory URL.
    pub directory: String,
    pub challenge: Challenge,
    pub http_port: u16,
    pub dns_hook: Option<String>,
}

impl AcmeConfig {
    pub fn from_prefs(t: &crate::storage::McpTlsPrefs) -> Result<Self> {
        let challenge = parse_challenge(t.challenge.as_deref().unwrap_or("tls-alpn-01"))?;
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
            if !weirdnet_tls::cert::valid_name(d, challenge == Challenge::Dns01) {
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
            None if t.staging => weirdnet_tls::acme::LETS_ENCRYPT_STAGING.to_string(),
            None => weirdnet_tls::acme::LETS_ENCRYPT.to_string(),
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

    /// `Let's Encrypt`, `Let's Encrypt staging` or the custom directory URL.
    pub fn ca_name(&self) -> &str {
        match self.directory.as_str() {
            weirdnet_tls::acme::LETS_ENCRYPT => "Let's Encrypt",
            weirdnet_tls::acme::LETS_ENCRYPT_STAGING => "Let's Encrypt staging",
            d => d,
        }
    }

    fn acme_directory(&self) -> Directory {
        match self.directory.as_str() {
            weirdnet_tls::acme::LETS_ENCRYPT => Directory::LetsEncrypt,
            weirdnet_tls::acme::LETS_ENCRYPT_STAGING => Directory::LetsEncryptStaging,
            d => Directory::Custom(d.to_string()),
        }
    }

    /// The certificate set in the store: one per CA, so a new CA means a new
    /// certificate (`mcp`, `mcp-staging`, `mcp-<hash of the URL>`).
    pub fn set_id(&self) -> String {
        match self.directory.as_str() {
            weirdnet_tls::acme::LETS_ENCRYPT => "mcp".into(),
            weirdnet_tls::acme::LETS_ENCRYPT_STAGING => "mcp-staging".into(),
            d => format!("mcp-{}", short_hash(d, 4)),
        }
    }

    /// The weirdnet-tls configuration.
    pub fn to_weirdnet(&self) -> weirdnet_tls::AcmeConfig {
        let mut c = weirdnet_tls::AcmeConfig::new(self.set_id(), self.domains.clone());
        c.directory = self.acme_directory();
        c.contact = self.email.iter().cloned().collect();
        c.challenge = self.challenge;
        c
    }
}

fn short_hash(s: &str, bytes: usize) -> String {
    use sha2::Digest;
    sha2::Sha256::digest(s.as_bytes())[..bytes].iter().map(|b| format!("{b:02x}")).collect()
}

/// `$MOUSER_HOME/mcp/acme`.
pub fn default_dir() -> PathBuf {
    crate::mcp_server::dir().join("acme")
}

/// File the CLI drops to ask the running app to renew now.
pub fn trigger_path(dir: &Path) -> PathBuf {
    dir.join("renew-now")
}

/// Ask the running app to renew within [`TRIGGER_POLL`] (forced).
pub fn request_renewal(dir: &Path) -> Result<()> {
    store::write_private(&trigger_path(dir), b"")?;
    Ok(())
}

/// `state.json` of Mouser 1.7 and earlier.
#[derive(Debug, Default, serde::Deserialize)]
struct LegacyState {
    #[serde(default)]
    failures: u32,
    #[serde(default)]
    last_failure: Option<u64>,
    #[serde(default)]
    last_error: Option<String>,
    #[serde(default)]
    last_issued: Option<u64>,
    #[serde(default)]
    directory: Option<String>,
}

fn from_unix(s: u64) -> SystemTime {
    UNIX_EPOCH + Duration::from_secs(s)
}

/// What Mouser 1.7 and earlier kept in `dir` for `cfg`'s CA:
/// `account-<id>.json`, and `cert.pem`, `key.pem` and `state.json` when the
/// certificate came from that CA. For [`Acme::import`], which never overwrites.
pub fn legacy(dir: &Path, cfg: &AcmeConfig) -> Result<Legacy> {
    let old: LegacyState = std::fs::read(dir.join("state.json")).ok().and_then(|b| serde_json::from_slice(&b).ok()).unwrap_or_default();
    let account = dir.join(format!("account-{}.json", short_hash(&cfg.directory, 6)));
    let (c, k) = (dir.join("cert.pem"), dir.join("key.pem"));
    let same_ca = old.directory.as_deref().is_none_or(|d| d == cfg.directory);
    let l = if same_ca { Legacy::from_files(Some(&account), Some(&c), Some(&k))? } else { Legacy::from_files(Some(&account), None, None)? };
    Ok(l.with_state(Persisted {
        failures: old.failures,
        last_failure: old.last_failure.map(from_unix),
        last_error: old.last_error,
        last_issued: old.last_issued.map(from_unix),
        ..Persisted::default()
    }))
}

/// Makes the client factory weirdnet-tls talks to the CA through (tests).
pub type ClientFactory = Box<dyn Fn() -> Box<dyn HttpClient> + Send + Sync>;

/// One `[mcp.server.tls]` ACME setup: the store, and the weirdnet-tls [`Acme`]
/// serving through its own [`Resolver`].
pub struct Certs {
    pub cfg: AcmeConfig,
    pub dir: PathBuf,
    pub store: Arc<FileStore>,
    pub acme: Arc<Acme>,
}

impl std::fmt::Debug for Certs {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Certs").field("cfg", &self.cfg).field("dir", &self.dir).finish_non_exhaustive()
    }
}

impl Certs {
    /// The app's setup: `dns-01` through `dns_hook`, or failing with a hint to
    /// run `mouser mcp cert renew` when there is none.
    pub fn open(cfg: &AcmeConfig, dir: &Path) -> Result<Self> {
        Self::open_with(cfg, dir, None, None)
    }

    /// [`Certs::open`] with another `dns-01` publisher and CA client.
    pub fn open_with(cfg: &AcmeConfig, dir: &Path, dns: Option<Arc<dyn DnsChallenge>>, client: Option<ClientFactory>) -> Result<Self> {
        let store = Arc::new(FileStore::new(dir));
        let dns = dns.map_or_else(
            || -> Result<Arc<dyn DnsChallenge>> {
                Ok(match &cfg.dns_hook {
                    Some(h) => Arc::new(HookDns::new(h.as_str()).with_prefix("MOUSER_ACME_")?),
                    None => Arc::new(ManualDns::new(|_| {
                        Err(weirdnet_tls::Error::Challenge(
                            "dns-01 needs mcp.server.tls.dns_hook, or run `mouser mcp cert renew` to add the TXT record by hand".into(),
                        ))
                    })),
                })
            },
            Ok,
        )?;
        let mut acme = Acme::new(cfg.to_weirdnet(), store.clone(), Arc::new(Resolver::new()))?.with_dns(dns).with_default_cert(true);
        if let Some(f) = client {
            acme = acme.with_http_client(f);
        }
        if let Err(e) = legacy(dir, cfg).and_then(|l| Ok(acme.import(&l)?)) {
            log::warn!("acme: copying the Mouser 1.7 certificate files: {e:#}");
        }
        Ok(Self { cfg: cfg.clone(), dir: dir.to_path_buf(), store, acme: Arc::new(acme) })
    }

    /// Server TLS config (`http/1.1`, and `acme-tls/1` for the CA) from the resolver.
    pub fn server_config(&self) -> Result<Arc<rustls::ServerConfig>> {
        Ok(self.acme.resolver().server_config(&[b"http/1.1"])?)
    }

    /// Serves the stored certificate, also to handshakes without a known name
    /// (an IP address, no SNI), as Mouser did before weirdnet-tls.
    pub fn load(&self) -> Result<Option<CertInfo>> {
        Ok(self.acme.load()?)
    }

    /// The next renewal from the store; no network.
    pub fn schedule(&self, now: SystemTime) -> Result<Schedule> {
        Ok(self.acme.schedule(now)?)
    }

    /// One renewal pass on this thread (`mouser mcp cert renew`): answers
    /// `tls-alpn-01` or `http-01` on `challenge` alone while it runs.
    pub fn renew_blocking(&self, force: bool, challenge: Option<TcpListener>) -> Result<Outcome> {
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build()?;
        rt.block_on(async {
            let srv = match challenge {
                Some(l) => {
                    l.set_nonblocking(true)?;
                    Some(self.acme.serve_challenge(tokio::net::TcpListener::from_std(l)?)?)
                }
                None => None,
            };
            let r = self.acme.renew_once(force).await;
            if let Some(h) = srv {
                h.abort();
            }
            match r {
                Ok(o) => Ok(o),
                Err(weirdnet_tls::Error::TooSoon(t)) => bail!(
                    "a certificate was issued less than a day ago; not asking again before {} (Let's Encrypt limits duplicate certificates)",
                    fmt_time(t)
                ),
                Err(e) => Err(e.into()),
            }
        })
    }

    /// Background renewal for the running app: serves the stored certificate,
    /// answers `http-01` on `http01`, renews when due (weirdnet-tls
    /// [`Acme::spawn`]), and renews now ([`Acme::trigger_renewal`]) when
    /// [`trigger_path`] appears.
    pub fn spawn_renewer(&self, http01: Option<TcpListener>) -> std::thread::JoinHandle<()> {
        self.spawn_renewer_polling(http01, TRIGGER_POLL)
    }

    fn spawn_renewer_polling(&self, http01: Option<TcpListener>, poll: Duration) -> std::thread::JoinHandle<()> {
        if let Err(e) = self.load() {
            log::warn!("acme: stored certificate unusable, getting a new one: {e:#}");
        }
        let (acme, dir, names, ca) = (self.acme.clone(), self.dir.clone(), self.cfg.domains.join(", "), self.cfg.ca_name().to_string());
        std::thread::Builder::new()
            .name("mcp-acme".into())
            .spawn(move || {
                let rt = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                    Ok(rt) => rt,
                    Err(e) => return log::warn!("acme: no runtime: {e}"),
                };
                rt.block_on(async move {
                    if let Some(l) = http01 {
                        match l.set_nonblocking(true).and_then(|_| tokio::net::TcpListener::from_std(l)) {
                            Ok(l) => drop(weirdnet_tls::challenge::serve_http01(l, acme.http01().clone())),
                            Err(e) => log::warn!("acme: http-01 listener: {e}"),
                        }
                    }
                    let mut rx = acme.watch();
                    tokio::spawn(async move {
                        let mut last = (rx.borrow().not_after, rx.borrow().failures);
                        while rx.changed().await.is_ok() {
                            let s = rx.borrow_and_update().clone();
                            if (s.not_after, s.failures) != last {
                                match (&s.last_error, s.not_after) {
                                    (Some(e), _) if s.failures > 0 => {
                                        log::warn!("acme: renewal failed ({} in a row), keeping the current certificate: {e}", s.failures)
                                    }
                                    (_, Some(na)) => log::info!("acme: certificate for {names} from {ca}, valid until {}", fmt_time(na)),
                                    _ => {}
                                }
                                last = (s.not_after, s.failures);
                            }
                        }
                    });
                    let _renewer = acme.clone().spawn();
                    loop {
                        tokio::time::sleep(poll).await;
                        if std::fs::remove_file(trigger_path(&dir)).is_ok() {
                            acme.trigger_renewal();
                        }
                    }
                })
            })
            .expect("spawn mcp-acme")
    }
}

/// `2026-12-01 14:00 UTC`.
pub fn fmt_time(t: SystemTime) -> String {
    let s = t.duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
    time::OffsetDateTime::from_unix_timestamp(i64::try_from(s).unwrap_or(0))
        .map(|d| format!("{:04}-{:02}-{:02} {:02}:{:02} UTC", d.year(), d.month() as u8, d.day(), d.hour(), d.minute()))
        .unwrap_or_default()
}

/// The certificate and key files, for messages.
pub fn cert_file(dir: &Path, cfg: &AcmeConfig) -> PathBuf {
    dir.join(store::cert_key(&cfg.set_id()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::storage::McpTlsPrefs;
    use weirdnet_tls::testing::MockCa;
    use weirdnet_tls::{CertStore, Source};

    const DOMAIN: &str = "mcp.example.test";

    fn prefs(domains: &str) -> McpTlsPrefs {
        McpTlsPrefs { source: Some("acme".into()), domains: Some(domains.into()), ..Default::default() }
    }

    fn cfg(challenge: Challenge) -> AcmeConfig {
        AcmeConfig { directory: MockCa::DIRECTORY.into(), challenge, ..AcmeConfig::from_prefs(&prefs(DOMAIN)).unwrap() }
    }

    fn certs(ca: &MockCa, c: &AcmeConfig, dir: &Path) -> Certs {
        let ca = ca.clone();
        Certs::open_with(c, dir, None, Some(Box::new(move || ca.client()))).unwrap()
    }

    /// The leaf served to a handshake for `sni`, without checking it.
    fn served(resolver: &Resolver, sni: Option<&str>) -> Option<Vec<u8>> {
        resolver.lookup(sni).map(|(_, k)| k.cert[0].to_vec())
    }

    #[test]
    fn config_defaults_to_production_and_checks_names() {
        let c = AcmeConfig::from_prefs(&prefs("MCP.Example.com., b.example.com")).unwrap();
        assert_eq!(c.domains, ["mcp.example.com", "b.example.com"]);
        assert_eq!(c.directory, weirdnet_tls::acme::LETS_ENCRYPT);
        assert_eq!((c.challenge, c.http_port, c.email.clone()), (Challenge::TlsAlpn01, 80, None));
        assert_eq!((c.ca_name(), c.set_id().as_str()), ("Let's Encrypt", "mcp"));
        let s = AcmeConfig::from_prefs(&McpTlsPrefs { staging: true, email: Some("me@example.com".into()), ..prefs("a.example.com") }).unwrap();
        assert_eq!((s.directory.as_str(), s.email.as_deref()), (weirdnet_tls::acme::LETS_ENCRYPT_STAGING, Some("me@example.com")));
        assert_eq!((s.ca_name(), s.set_id().as_str()), ("Let's Encrypt staging", "mcp-staging"));
        let d = AcmeConfig::from_prefs(&McpTlsPrefs { directory: Some("https://ca.example/dir".into()), staging: true, ..prefs("a.example.com") })
            .unwrap();
        assert_eq!(d.directory, "https://ca.example/dir");
        assert!(d.set_id().starts_with("mcp-") && d.set_id().len() == 12, "{}", d.set_id());
        let w = d.to_weirdnet();
        assert_eq!((w.directory, w.contact.len()), (Directory::Custom("https://ca.example/dir".into()), 0));
        for bad in ["", "localhost", "127.0.0.1", "box.localhost", "nodots", "a..b.com", "-a.com", "*.example.com", "a.b.123"] {
            assert!(AcmeConfig::from_prefs(&prefs(bad)).is_err(), "{bad}");
        }
        let w = AcmeConfig::from_prefs(&McpTlsPrefs { challenge: Some("dns-01".into()), ..prefs("*.example.com") }).unwrap();
        assert_eq!(w.challenge, Challenge::Dns01);
        w.to_weirdnet().validate().unwrap();
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { email: Some("nope".into()), ..prefs("a.example.com") }).is_err());
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { directory: Some("http://ca".into()), ..prefs("a.example.com") }).is_err());
        assert!(AcmeConfig::from_prefs(&McpTlsPrefs { challenge: Some("dns".into()), ..prefs("a.example.com") }).is_err());
        for n in CHALLENGES {
            assert_eq!(parse_challenge(n).unwrap().as_str(), n);
        }
    }

    #[test]
    fn tls_alpn_issues_serves_and_schedules() {
        let dir = tempfile::tempdir().unwrap();
        let ca = MockCa::new();
        let c = cfg(Challenge::TlsAlpn01);
        let certs = certs(&ca, &c, dir.path());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        ca.validate_tls_alpn_at(l.local_addr().unwrap());
        assert!(certs.load().unwrap().is_none());
        assert!(served(certs.acme.resolver(), Some(DOMAIN)).is_none(), "no certificate, no TLS");
        assert!(certs.schedule(SystemTime::now()).unwrap().is_due(SystemTime::now()), "due now");

        // Answered on a challenge-only listener, as `mouser mcp cert renew` does.
        let Outcome::Renewed(info) = certs.renew_blocking(false, Some(l)).unwrap() else { panic!("not issued") };
        assert_eq!(info.names, [DOMAIN]);
        assert_eq!(ca.validations(), [(DOMAIN.to_string(), "tls-alpn-01".to_string())]);
        assert_eq!(served(certs.acme.resolver(), Some(DOMAIN)), Some(info.der.clone()));
        assert!(cert_file(dir.path(), &c).is_file());

        // A restart serves it to every handshake and schedules the renewal two
        // thirds in (or in the CA's window), without the network.
        let again = Certs::open_with(&c, dir.path(), None, None).unwrap();
        assert_eq!(again.load().unwrap().unwrap().der, info.der);
        assert_eq!(served(again.acme.resolver(), None), Some(info.der.clone()), "no SNI");
        assert_eq!(served(again.acme.resolver(), Some("192.0.2.1")), Some(info.der.clone()), "another name");
        let s = again.schedule(SystemTime::now()).unwrap();
        let life = info.lifetime();
        let next = s.next_attempt;
        assert!(next >= info.not_before + life * 2 / 3 && next <= info.not_before + life * 3 / 4, "{next:?}");
        assert!(matches!(certs.renew_blocking(false, None).unwrap(), Outcome::NotDue(_)));
        let e = certs.renew_blocking(true, None).unwrap_err();
        assert!(format!("{e:#}").contains("less than a day ago"), "{e:#}");
        assert_eq!(ca.issued(), 1);
    }

    #[test]
    fn http01_is_answered_on_its_own_listener() {
        let dir = tempfile::tempdir().unwrap();
        let ca = MockCa::new();
        let certs = certs(&ca, &cfg(Challenge::Http01), dir.path());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        ca.validate_http01_at(l.local_addr().unwrap());
        assert!(matches!(certs.renew_blocking(false, Some(l)).unwrap(), Outcome::Renewed(_)));
        assert_eq!(ca.validations(), [(DOMAIN.to_string(), "http-01".to_string())]);
        assert!(certs.acme.http01().is_empty(), "answers withdrawn");
    }

    #[test]
    fn dns01_without_a_hook_says_how() {
        let dir = tempfile::tempdir().unwrap();
        let ca = MockCa::new();
        let certs = certs(&ca, &cfg(Challenge::Dns01), dir.path());
        let e = certs.renew_blocking(false, None).unwrap_err();
        assert!(format!("{e:#}").contains("dns_hook"), "{e:#}");
        let s = certs.schedule(SystemTime::now()).unwrap();
        assert_eq!(s.failures, 1);
        assert!(s.next_attempt > SystemTime::now() + Duration::from_secs(3500), "backs off an hour");
    }

    #[test]
    fn a_trigger_renews_the_running_app_and_swaps_the_certificate() {
        let dir = tempfile::tempdir().unwrap();
        let ca = MockCa::new();
        let c = cfg(Challenge::Http01);
        let certs = certs(&ca, &c, dir.path());
        let l = TcpListener::bind("127.0.0.1:0").unwrap();
        ca.validate_http01_at(l.local_addr().unwrap());
        let _t = certs.spawn_renewer_polling(Some(l), Duration::from_millis(20));
        let wait = |n: usize| {
            let end = std::time::Instant::now() + Duration::from_secs(20);
            while ca.issued() < n {
                assert!(std::time::Instant::now() < end, "only {} issued", ca.issued());
                std::thread::sleep(Duration::from_millis(20));
            }
        };
        wait(1);
        let first = loop {
            if let Some(d) = served(certs.acme.resolver(), None) {
                break d;
            }
            std::thread::sleep(Duration::from_millis(20));
        };
        // Pretend the certificate is two days old, so a forced renewal may go.
        let key = store::state_key(&c.set_id());
        let mut st: Persisted = store::get_json(&*certs.store, &key).unwrap().unwrap();
        st.last_issued = Some(SystemTime::now() - Duration::from_secs(2 * 86400));
        store::put_json(&*certs.store, &key, &st).unwrap();
        request_renewal(dir.path()).unwrap();
        wait(2);
        let end = std::time::Instant::now() + Duration::from_secs(20);
        while served(certs.acme.resolver(), Some(DOMAIN)).as_ref() == Some(&first) {
            assert!(std::time::Instant::now() < end, "not swapped");
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(!trigger_path(dir.path()).exists());
        assert_eq!(certs.acme.resolver().lookup(Some(DOMAIN)).unwrap().0, Source::Acme);
    }

    #[test]
    fn mouser_1_7_files_are_imported() {
        // A real account from the CA, as 1.7 kept it.
        let ca = MockCa::new();
        let c = cfg(Challenge::TlsAlpn01);
        let scratch = tempfile::tempdir().unwrap();
        let reg = certs(&ca, &c, scratch.path());
        tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap().block_on(reg.acme.account()).unwrap();
        let creds = reg.store.get(&store::account_key(&c.directory)).unwrap().unwrap();

        let dir = tempfile::tempdir().unwrap();
        let pair = weirdnet_tls::cert::self_signed(&[DOMAIN.to_string()], 90).unwrap();
        std::fs::write(dir.path().join("cert.pem"), &pair.chain_pem).unwrap();
        std::fs::write(dir.path().join("key.pem"), &pair.key_pem).unwrap();
        std::fs::write(dir.path().join(format!("account-{}.json", short_hash(&c.directory, 6))), &creds).unwrap();
        let state = format!(
            r#"{{"failures":2,"last_failure":1900000000,"last_error":"boom","last_issued":1890000000,"domains":["{DOMAIN}"],"directory":"{}"}}"#,
            c.directory
        );
        std::fs::write(dir.path().join("state.json"), state).unwrap();

        let certs = certs(&ca, &c, dir.path());
        let s = &*certs.store;
        assert_eq!(store::load_cert(s, &c.set_id()).unwrap(), Some(pair.clone()));
        assert_eq!(s.get(&store::account_key(&c.directory)).unwrap().unwrap(), creds);
        let p: Persisted = store::get_json(s, &store::state_key(&c.set_id())).unwrap().unwrap();
        assert_eq!((p.failures, p.last_error.as_deref(), p.last_issued), (2, Some("boom"), Some(from_unix(1_890_000_000))));
        assert!(!certs.acme.import(&legacy(dir.path(), &c).unwrap()).unwrap().any(), "once");
        assert!(dir.path().join("cert.pem").is_file(), "old files stay");
        assert_eq!(certs.load().unwrap().unwrap().names, [DOMAIN]);

        // A certificate from another CA stays where it is.
        let other = tempfile::tempdir().unwrap();
        for f in ["cert.pem", "key.pem", "state.json"] {
            std::fs::copy(dir.path().join(f), other.path().join(f)).unwrap();
        }
        let prod = AcmeConfig { directory: weirdnet_tls::acme::LETS_ENCRYPT.into(), ..c.clone() };
        assert!(legacy(other.path(), &prod).unwrap().cert.is_none());
        let p = Certs::open_with(&prod, other.path(), None, None).unwrap();
        assert!(store::load_cert(&*p.store, &prod.set_id()).unwrap().is_none());
    }

    #[cfg(unix)]
    #[test]
    fn dns_hook_gets_both_sets_of_variables() {
        let dir = tempfile::tempdir().unwrap();
        let out = dir.path().join("hook.txt");
        let hook = HookDns::new(format!(
            "echo \"$MOUSER_ACME_ACTION $MOUSER_ACME_DOMAIN $MOUSER_ACME_NAME $MOUSER_ACME_TXT $WEIRDNET_TLS_TXT\" >> {}",
            out.display()
        ))
        .with_prefix("MOUSER_ACME_")
        .unwrap();
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        rt.block_on(async {
            hook.present(&DnsRecord::new("*.example.com", "abc")).await.unwrap();
            hook.cleanup(&DnsRecord::new("example.com", "abc")).await.unwrap();
        });
        assert_eq!(
            std::fs::read_to_string(&out).unwrap(),
            "present example.com _acme-challenge.example.com abc abc\ncleanup example.com _acme-challenge.example.com abc abc\n"
        );
        let fail = HookDns::new("exit 3");
        assert!(rt.block_on(fail.present(&DnsRecord::new("example.com", "x"))).is_err());
    }
}
