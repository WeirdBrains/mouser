//! Installing plugins and engines from a name, `gh:Owner/Repo[@tag]`, an
//! `https://` URL, a local path or `crate:NAME` (docs/downloads.md).
//!
//! Every source goes through the same steps, and any failure aborts that
//! install only: HTTPS only, a size limit, sha256, a minisign signature
//! (official items against the key pinned in Mouser; third-party ones against
//! their author's key, remembered on first use, or unsigned with a warning),
//! unpacking with path checks, a probe (engines in the pixel host), the
//! API/ABI range, then the platform's own signature check. The result goes
//! into its own version directory, `<name>/<version>/`, and `current` switches
//! to it only after that; the previous version is kept for `rollback`.
//! Downloads run on the download manager's threads (`crate::downloads`).

use crate::models::http;
use anyhow::{Context, Result, anyhow, bail};
use serde::{Deserialize, Serialize};
use sha2::Digest;
use std::collections::BTreeMap;
use std::fs;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::sync::OnceLock;

/// The official WeirdBrains signing key (minisign public key 61BA1720747F9146,
/// base64): every `plugins.lock` item must verify against it.
/// `MOUSER_TEST_OFFICIAL_KEY` replaces it in tests.
pub const OFFICIAL_KEY: Option<&str> = Some("RWRGkX90IBe6YaOypKF9r1wz4OqpCp3Ggb1Ag2omAzxTRaqoHzsNQSdP");

/// Largest download accepted (bytes).
pub const MAX_BYTES: u64 = 200 * 1024 * 1024;

/// How many versions of each item are kept (the current one and one to roll back to).
pub const KEEP_VERSIONS: usize = 2;

/// Plugins or engines (the two install roots; one implementation for both).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    Plugin,
    Engine,
}

impl Kind {
    pub fn name(self) -> &'static str {
        match self {
            Kind::Plugin => "plugin",
            Kind::Engine => "engine",
        }
    }
    /// Where installs of this kind live.
    pub fn root(self) -> PathBuf {
        match self {
            Kind::Plugin => crate::plugins::dir(),
            Kind::Engine => crate::engines::dir(),
        }
    }
}

/// A `<SRC>` as typed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Src {
    /// An official name from `plugins.lock`.
    Official(String),
    /// A GitHub release (latest without a tag).
    Github { owner: String, repo: String, tag: Option<String> },
    /// An asset, or a release manifest (`.json`/`.toml`).
    Url(String),
    /// A local file or folder.
    Path(PathBuf),
    /// `cargo binstall NAME` (mode plugins only).
    Crate(String),
}

impl Src {
    pub fn parse(s: &str) -> Result<Src> {
        let s = s.trim();
        if s.is_empty() {
            bail!("nothing to install: give a name, gh:Owner/Repo[@tag], an https:// URL, a path or crate:NAME");
        }
        if let Some(r) = s.strip_prefix("gh:") {
            let (path, tag) = match r.split_once('@') {
                Some((p, t)) if !t.is_empty() => (p, Some(t.to_string())),
                Some(_) => bail!("gh:Owner/Repo@TAG needs a tag after @"),
                None => (r, None),
            };
            let Some((owner, repo)) = path.split_once('/') else { bail!("use gh:Owner/Repo[@tag]") };
            let ok = |x: &str| !x.is_empty() && x.chars().all(|c| c.is_ascii_alphanumeric() || "-_.".contains(c)) && x != "." && x != "..";
            if !ok(owner) || !ok(repo) {
                bail!("use gh:Owner/Repo[@tag] (letters, digits, - _ .)");
            }
            return Ok(Src::Github { owner: owner.into(), repo: repo.into(), tag });
        }
        if let Some(n) = s.strip_prefix("crate:") {
            if !crate::engines::valid_name(n) {
                bail!("crate:NAME needs a crate name");
            }
            return Ok(Src::Crate(n.into()));
        }
        if s.starts_with("https://") {
            return Ok(Src::Url(s.into()));
        }
        if s.starts_with("http://") {
            bail!("only https:// downloads are allowed");
        }
        let p = Path::new(s);
        if p.exists() || s.contains(['/', '\\']) || s.starts_with('.') || s.starts_with('~') {
            return Ok(Src::Path(p.to_path_buf()));
        }
        if crate::engines::valid_name(s) {
            return Ok(Src::Official(s.into()));
        }
        bail!("{s:?} is not a name, gh:Owner/Repo, https:// URL, existing path or crate:NAME")
    }
}

impl std::fmt::Display for Src {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Src::Official(n) => write!(f, "{n}"),
            Src::Github { owner, repo, tag: Some(t) } => write!(f, "gh:{owner}/{repo}@{t}"),
            Src::Github { owner, repo, tag: None } => write!(f, "gh:{owner}/{repo}"),
            Src::Url(u) => write!(f, "{u}"),
            Src::Path(p) => write!(f, "{}", p.display()),
            Src::Crate(c) => write!(f, "crate:{c}"),
        }
    }
}

// ---------------------------------------------------------------- the lock

/// `plugins.lock`: the official items and versions this Mouser release pins.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Lock {
    #[serde(default)]
    pub item: Vec<LockItem>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockItem {
    pub name: String,
    pub kind: Kind,
    pub version: String,
    /// By Rust target triple.
    #[serde(default)]
    pub assets: BTreeMap<String, LockAsset>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockAsset {
    pub url: String,
    pub sha256: String,
    pub size: u64,
    /// The `.minisig` file's text.
    pub minisig: String,
}

/// The compiled-in lock (`MOUSER_TEST_LOCK` names a file to use instead, for tests).
pub fn lock() -> Lock {
    if let Some(p) = std::env::var_os("MOUSER_TEST_LOCK") {
        return fs::read_to_string(&p).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default();
    }
    toml::from_str(include_str!("plugins.lock")).expect("plugins.lock")
}

impl Lock {
    pub fn find(&self, name: &str) -> Option<&LockItem> {
        self.item.iter().find(|i| i.name == name)
    }
}

/// Target triples this build can run, best first (`-musl` after `-gnu` on
/// Linux, the universal macOS build first).
pub fn targets() -> Vec<String> {
    let arch = std::env::consts::ARCH;
    match std::env::consts::OS {
        "linux" if cfg!(target_env = "musl") => vec![format!("{arch}-unknown-linux-musl")],
        "linux" => vec![format!("{arch}-unknown-linux-gnu"), format!("{arch}-unknown-linux-musl")],
        "macos" => vec!["universal-apple-darwin".into(), format!("{arch}-apple-darwin")],
        "windows" => vec![format!("{arch}-pc-windows-msvc")],
        os => vec![format!("{arch}-unknown-{os}")],
    }
}

// ---------------------------------------------------------------- resolving

/// What to download and how to check it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Asset {
    /// File name (decides how it is unpacked).
    pub file: String,
    pub url: String,
    pub version: Option<String>,
    pub sha256: Option<String>,
    pub size: Option<u64>,
    pub minisig: Option<String>,
    /// From `plugins.lock`: must verify against [`OFFICIAL_KEY`].
    pub official: bool,
    /// The author's public key, when the source publishes one (`minisign.pub`).
    pub author_key: Option<String>,
    /// Who vouches for the key (TOFU record): `gh:owner/repo` or the URL's directory.
    pub origin: String,
}

/// (name, version, target, ext) of a release asset.
pub type AssetName = (String, String, String, String);

/// `NAME-VERSION-TARGET.EXT` → (name, version, target, ext).
pub fn split_asset_name(file: &str) -> Option<AssetName> {
    let exts = [".tar.gz", ".tgz", ".zip", ".so", ".dylib", ".dll", ".exe", ""];
    let (stem, ext) = exts.iter().find_map(|e| file.strip_suffix(e).map(|s| (s, e.trim_start_matches('.'))))?;
    // NAME-VERSION-TARGET: VERSION is the first part that starts with a digit
    // (or `v` and a digit) and has a dot; TARGET is the rest (2+ parts).
    let parts: Vec<&str> = stem.split('-').collect();
    for k in 1..parts.len() {
        let v = parts[k].trim_start_matches('v');
        if v.starts_with(|c: char| c.is_ascii_digit()) && v.contains('.') && parts.len() > k + 2 {
            return Some((parts[..k].join("-"), v.into(), parts[k + 1..].join("-"), ext.into()));
        }
    }
    None
}

/// The asset for this machine among `files` (names), or an error listing what there is.
pub fn pick_asset<'a>(files: &[&'a str], want_name: Option<&str>) -> Result<&'a str> {
    let parsed: Vec<(&str, AssetName)> = files
        .iter()
        .filter(|f| !f.ends_with(".sha256") && !f.ends_with(".minisig"))
        .filter_map(|f| split_asset_name(f).map(|p| (*f, p)))
        .filter(|(_, p)| want_name.is_none_or(|n| p.0 == n))
        .collect();
    for t in targets() {
        if let Some((f, _)) = parsed.iter().find(|(_, p)| p.2 == t) {
            return Ok(f);
        }
    }
    let have: Vec<String> = parsed.iter().map(|(_, p)| p.2.clone()).collect();
    bail!(
        "no asset for {} (this machine); the release has: {}",
        targets().join(" or "),
        if have.is_empty() { "no NAME-VERSION-TARGET assets".to_string() } else { have.join(", ") }
    )
}

/// A release manifest (`mouser-plugin.json`, or a `.toml`/`.json` URL).
#[derive(Debug, Clone, Deserialize)]
struct ReleaseManifest {
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    version: Option<String>,
    #[serde(default)]
    key: Option<String>,
    assets: Vec<ManifestAsset>,
}

#[derive(Debug, Clone, Deserialize)]
struct ManifestAsset {
    target: String,
    url: String,
    #[serde(default)]
    sha256: Option<String>,
    #[serde(default)]
    size: Option<u64>,
    #[serde(default)]
    minisig: Option<String>,
}

fn from_release_manifest(m: ReleaseManifest, base: &str, origin: String) -> Result<Asset> {
    let ts = targets();
    let a = ts.iter().find_map(|t| m.assets.iter().find(|a| &a.target == t)).ok_or_else(|| {
        anyhow!("no asset for {}; the manifest has: {}", ts.join(" or "), m.assets.iter().map(|a| a.target.as_str()).collect::<Vec<_>>().join(", "))
    })?;
    let url = if a.url.starts_with("https://") { a.url.clone() } else { http::join(base, &a.url) };
    let _ = m.name;
    Ok(Asset {
        file: url.rsplit('/').next().unwrap_or("asset").to_string(),
        url,
        version: m.version,
        sha256: a.sha256.clone(),
        size: a.size,
        minisig: a.minisig.clone(),
        official: false,
        author_key: m.key,
        origin,
    })
}

fn ca() -> Option<PathBuf> {
    std::env::var_os("MOUSER_TEST_CA").map(PathBuf::from)
}

/// GET with HTTPS enforced on every hop (redirects followed).
pub fn get(url: &str) -> Result<http::Response> {
    let mut url = url.to_string();
    for _ in 0..6 {
        if !url.starts_with("https://") {
            bail!("refusing {url}: only https:// downloads are allowed");
        }
        let r = http::request("GET", &url, &[("Accept", "*/*".to_string())], None, ca().as_deref())?;
        if matches!(r.status, 301 | 302 | 303 | 307 | 308) {
            let loc = r.header("location").context("redirect without Location")?.to_string();
            url = if loc.contains("://") { loc } else { http::join(&url, &loc) };
            continue;
        }
        return Ok(r);
    }
    bail!("too many redirects")
}

fn get_text(url: &str) -> Result<Option<String>> {
    let mut r = get(url)?;
    if r.status == 404 {
        return Ok(None);
    }
    if r.status != 200 {
        bail!("{url}: HTTP {}", r.status);
    }
    let mut s = String::new();
    (&mut r.body).take(1 << 20).read_to_string(&mut s)?;
    Ok(Some(s))
}

fn github_api() -> String {
    std::env::var("MOUSER_TEST_GITHUB_API").unwrap_or_else(|_| "https://api.github.com".into())
}

/// Work out the download for `src` (network for `gh:` and URLs).
pub fn resolve(src: &Src, kind: Kind, version: Option<&str>) -> Result<Asset> {
    match src {
        Src::Official(n) => {
            let l = lock();
            let it = l.find(n).ok_or_else(|| {
                let names: Vec<&str> = l.item.iter().map(|i| i.name.as_str()).collect();
                anyhow!(
                    "{n} is not an official {} (official: {}); use gh:Owner/Repo, an https:// URL or a path",
                    kind.name(),
                    if names.is_empty() { "none in this build yet".to_string() } else { names.join(", ") }
                )
            })?;
            if it.kind != kind {
                bail!("{n} is an official {}, not a {} (`mouser {} install {n}`)", it.kind.name(), kind.name(), it.kind.name());
            }
            if let Some(v) = version
                && v != it.version
            {
                bail!("this Mouser pins {n} {}; --version {v} needs gh:Owner/Repo@TAG or a URL", it.version);
            }
            let ts = targets();
            let (_, a) = ts
                .iter()
                .find_map(|t| it.assets.get(t).map(|a| (t, a)))
                .ok_or_else(|| anyhow!("{n} {} has no asset for {}", it.version, ts.join(" or ")))?;
            Ok(Asset {
                file: a.url.rsplit('/').next().unwrap_or(n).to_string(),
                url: a.url.clone(),
                version: Some(it.version.clone()),
                sha256: Some(a.sha256.clone()),
                size: Some(a.size),
                minisig: Some(a.minisig.clone()),
                official: true,
                author_key: None,
                origin: format!("official:{n}"),
            })
        }
        Src::Github { owner, repo, tag } => {
            let tag = tag.clone().or_else(|| version.map(|v| format!("v{}", v.trim_start_matches('v'))));
            let api = github_api();
            let url = match &tag {
                Some(t) => format!("{api}/repos/{owner}/{repo}/releases/tags/{t}"),
                None => format!("{api}/repos/{owner}/{repo}/releases/latest"),
            };
            let body = get_text(&url)?.ok_or_else(|| anyhow!("gh:{owner}/{repo}: no such release ({url})"))?;
            let v: serde_json::Value = serde_json::from_str(&body).context("GitHub release JSON")?;
            let assets: Vec<(String, String)> = v["assets"]
                .as_array()
                .map(|a| a.iter().filter_map(|x| Some((x["name"].as_str()?.to_string(), x["browser_download_url"].as_str()?.to_string()))).collect())
                .unwrap_or_default();
            let url_of = |n: &str| assets.iter().find(|(a, _)| a == n).map(|(_, u)| u.clone());
            let origin = format!("gh:{owner}/{repo}");
            let key = match url_of("minisign.pub") {
                Some(u) => get_text(&u)?.and_then(|t| key_line(&t)),
                None => None,
            };
            if let Some(u) = url_of("mouser-plugin.json") {
                let t = get_text(&u)?.context("mouser-plugin.json")?;
                let m: ReleaseManifest = serde_json::from_str(&t).context("mouser-plugin.json")?;
                let mut a = from_release_manifest(m, &u, origin)?;
                a.author_key = a.author_key.or(key);
                return Ok(a);
            }
            let names: Vec<&str> = assets.iter().map(|(n, _)| n.as_str()).collect();
            let file = pick_asset(&names, Some(repo.as_str())).or_else(|_| pick_asset(&names, None))?.to_string();
            let sha = match url_of(&format!("{file}.sha256")) {
                Some(u) => get_text(&u)?.and_then(|t| t.split_whitespace().next().map(str::to_lowercase)),
                None => None,
            };
            let sig = match url_of(&format!("{file}.minisig")) {
                Some(u) => get_text(&u)?,
                None => None,
            };
            Ok(Asset {
                url: url_of(&file).unwrap_or_default(),
                version: split_asset_name(&file).map(|p| p.1).or_else(|| v["tag_name"].as_str().map(|t| t.trim_start_matches('v').to_string())),
                file,
                sha256: sha,
                size: None,
                minisig: sig,
                official: false,
                author_key: key,
                origin,
            })
        }
        Src::Url(u) => {
            let dir = u.rsplit_once('/').map(|(d, _)| d.to_string()).unwrap_or_else(|| u.clone());
            let file = u.rsplit('/').next().unwrap_or("asset").split(['?', '#']).next().unwrap_or("asset").to_string();
            if file.ends_with(".json") || file.ends_with(".toml") {
                let t = get_text(u)?.ok_or_else(|| anyhow!("{u}: not found"))?;
                let m: ReleaseManifest = if file.ends_with(".json") { serde_json::from_str(&t)? } else { toml::from_str(&t)? };
                let mut a = from_release_manifest(m, u, dir.clone())?;
                if a.author_key.is_none() {
                    a.author_key = get_text(&format!("{dir}/minisign.pub")).ok().flatten().and_then(|t| key_line(&t));
                }
                return Ok(a);
            }
            let sha = get_text(&format!("{u}.sha256")).ok().flatten().and_then(|t| t.split_whitespace().next().map(str::to_lowercase));
            let sig = get_text(&format!("{u}.minisig")).ok().flatten();
            let key = get_text(&format!("{dir}/minisign.pub")).ok().flatten().and_then(|t| key_line(&t));
            Ok(Asset {
                version: split_asset_name(&file).map(|p| p.1),
                file,
                url: u.clone(),
                sha256: sha,
                size: None,
                minisig: sig,
                official: false,
                author_key: key,
                origin: dir,
            })
        }
        Src::Path(_) | Src::Crate(_) => bail!("{src} is installed without a download"),
    }
}

/// The key line of a `minisign.pub` (the last non-comment line).
fn key_line(t: &str) -> Option<String> {
    t.lines().map(str::trim).rfind(|l| !l.is_empty() && !l.starts_with("untrusted comment")).map(String::from)
}

// ---------------------------------------------------------------- download

/// Progress: (bytes so far, total if known). Return false to cancel.
pub type Progress<'a> = &'a mut dyn FnMut(u64, Option<u64>) -> bool;

/// Download `a` into `dir`, checking the size limit and the sha256 as it
/// goes. Returns the file.
pub fn download(a: &Asset, dir: &Path, progress: Progress<'_>) -> Result<PathBuf> {
    let mut r = get(&a.url)?;
    if r.status != 200 {
        let mut b = String::new();
        let _ = (&mut r.body).take(2048).read_to_string(&mut b);
        bail!("{}: HTTP {} {}", a.url, r.status, b.trim().chars().take(200).collect::<String>());
    }
    let limit = max_bytes();
    let total = r.header("content-length").and_then(|v| v.parse::<u64>().ok()).or(a.size);
    if total.is_some_and(|t| t > limit) {
        bail!("{} is {} bytes, over the {} MB limit", a.file, total.unwrap_or(0), limit / (1024 * 1024));
    }
    fs::create_dir_all(dir)?;
    let path = dir.join(safe_file_name(&a.file));
    let mut f = fs::File::create(&path)?;
    let mut h = sha2::Sha256::new();
    let mut buf = vec![0u8; 64 * 1024];
    let mut done = 0u64;
    loop {
        if !progress(done, total) {
            bail!("cancelled");
        }
        let n = r.body.read(&mut buf)?;
        if n == 0 {
            break;
        }
        done += n as u64;
        if done > limit {
            bail!("{} is over the {} MB limit", a.file, limit / (1024 * 1024));
        }
        h.update(&buf[..n]);
        f.write_all(&buf[..n])?;
    }
    f.flush()?;
    progress(done, total);
    if let Some(t) = a.size
        && t != done
    {
        bail!("{}: got {done} bytes, expected {t}", a.file);
    }
    let got = hex::encode(h.finalize());
    match &a.sha256 {
        Some(want) if !want.eq_ignore_ascii_case(&got) => bail!("{}: sha256 mismatch (got {got}, expected {want})", a.file),
        None if a.official => bail!("{}: an official item without a sha256", a.file),
        _ => {}
    }
    Ok(path)
}

fn max_bytes() -> u64 {
    std::env::var("MOUSER_TEST_MAX_BYTES").ok().and_then(|v| v.parse().ok()).unwrap_or(MAX_BYTES)
}

fn safe_file_name(f: &str) -> String {
    let s: String = f.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.".contains(c) { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with('.') { format!("asset{s}") } else { s }
}

// ---------------------------------------------------------------- trust

/// Who signed what was installed.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Trust {
    /// The pinned WeirdBrains key.
    Official,
    /// The author's key; `new` on first use (remembered from now on).
    Author { key: String, new: bool },
    /// No signature: installed only after a warning.
    Unsigned,
    /// A local path or crate: nothing to verify against.
    Local,
}

impl Trust {
    pub fn text(&self) -> String {
        match self {
            Trust::Official => "signed by WeirdBrains (official)".into(),
            Trust::Author { key, new: true } => format!("signed by its author, key {key} (new: remembered from now on)"),
            Trust::Author { key, .. } => format!("signed by its author, key {key} (as before)"),
            Trust::Unsigned => "UNSIGNED: nobody vouches for this file".into(),
            Trust::Local => "from this computer".into(),
        }
    }
}

fn official_key() -> Option<String> {
    std::env::var("MOUSER_TEST_OFFICIAL_KEY").ok().filter(|k| !k.is_empty()).or(OFFICIAL_KEY.map(String::from))
}

fn verify_sig(key_b64: &str, sig: &str, file: &Path) -> Result<()> {
    let pk = minisign_verify::PublicKey::from_base64(key_b64.trim()).map_err(|e| anyhow!("bad public key: {e}"))?;
    let s = minisign_verify::Signature::decode(sig.trim()).map_err(|e| anyhow!("bad signature: {e}"))?;
    let data = fs::read(file)?;
    pk.verify(&data, &s, false).map_err(|e| anyhow!("signature check failed: {e}"))
}

/// Remembered author keys (`$MOUSER_HOME/plugins/keys.toml`), by origin.
fn keys_path() -> PathBuf {
    crate::plugins::dir().join("keys.toml")
}

fn known_keys() -> BTreeMap<String, String> {
    fs::read_to_string(keys_path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
}

fn remember_key(origin: &str, key: &str) -> Result<()> {
    let mut k = known_keys();
    k.insert(origin.into(), key.into());
    fs::create_dir_all(crate::plugins::dir())?;
    fs::write(keys_path(), toml::to_string(&k)?)?;
    Ok(())
}

/// Check the downloaded file's signature.
pub fn verify(a: &Asset, file: &Path) -> Result<Trust> {
    if a.official {
        let key = official_key()
            .ok_or_else(|| anyhow!("this build has no official signing key yet, so official items can't be verified; nothing was installed"))?;
        let sig = a.minisig.as_deref().ok_or_else(|| anyhow!("{}: official items must be signed", a.file))?;
        verify_sig(&key, sig, file).with_context(|| format!("{} (official key)", a.file))?;
        return Ok(Trust::Official);
    }
    let remembered = known_keys().get(&a.origin).cloned();
    match (&a.minisig, &a.author_key, &remembered) {
        (Some(_), Some(k), Some(r)) if k.trim() != r.trim() => {
            bail!(
                "{}: the signing key changed (was {r}, now {k}); refusing. Remove {} from {} if you trust the new key",
                a.file,
                a.origin,
                keys_path().display()
            )
        }
        (Some(sig), Some(k), _) | (Some(sig), None, Some(k)) => {
            verify_sig(k, sig, file).with_context(|| a.file.clone())?;
            Ok(Trust::Author { key: k.trim().to_string(), new: remembered.is_none() })
        }
        (Some(_), None, None) => bail!("{}: signed, but the source publishes no minisign.pub to check it with", a.file),
        (None, _, Some(r)) => bail!("{}: unsigned, but {} signed with {r} before; refusing", a.file, a.origin),
        (None, _, None) => Ok(Trust::Unsigned),
    }
}

// ---------------------------------------------------------------- unpack

/// Unpack an archive (`.tar.gz`, `.tgz`, `.zip`) into `out`, refusing absolute
/// paths, `..`, links and devices; anything else is copied as the one file.
/// Returns the folder that holds the manifest (a single top folder is entered).
pub fn unpack(file: &Path, out: &Path) -> Result<PathBuf> {
    fs::create_dir_all(out)?;
    let name = file.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
    let mut count = 0usize;
    let check = |p: &Path| -> Result<PathBuf> {
        let mut clean = PathBuf::new();
        for c in p.components() {
            match c {
                std::path::Component::Normal(x) => clean.push(x),
                std::path::Component::CurDir => {}
                _ => bail!("archive entry {} escapes the folder; refusing the archive", p.display()),
            }
        }
        // `./` (as `tar -C dir -czf x.tar.gz .` writes): the folder itself.
        if clean.as_os_str().is_empty() {
            return Ok(out.to_path_buf());
        }
        Ok(out.join(clean))
    };
    if name.ends_with(".tar.gz") || name.ends_with(".tgz") {
        let mut ar = tar::Archive::new(flate2::read::GzDecoder::new(fs::File::open(file)?));
        for e in ar.entries()? {
            let mut e = e?;
            let path = e.path()?.into_owned();
            let dest = check(&path)?;
            let t = e.header().entry_type();
            if t.is_dir() {
                fs::create_dir_all(&dest)?;
            } else if t.is_file() {
                if let Some(p) = dest.parent() {
                    fs::create_dir_all(p)?;
                }
                let mut f = fs::File::create(&dest)?;
                std::io::copy(&mut e, &mut f)?;
                set_mode(&dest, e.header().mode().unwrap_or(0o644));
                count += 1;
            } else if t == tar::EntryType::XGlobalHeader || t == tar::EntryType::XHeader {
            } else {
                bail!("archive entry {} is a link or device; refusing the archive", path.display());
            }
        }
    } else if name.ends_with(".zip") {
        let mut z = zip::ZipArchive::new(fs::File::open(file)?)?;
        for i in 0..z.len() {
            let mut e = z.by_index(i)?;
            let raw = PathBuf::from(e.name());
            let dest = check(&raw)?;
            if e.enclosed_name().is_none() {
                bail!("archive entry {} escapes the folder; refusing the archive", raw.display());
            }
            if e.is_symlink() {
                bail!("archive entry {} is a link; refusing the archive", raw.display());
            }
            if e.is_dir() {
                fs::create_dir_all(&dest)?;
            } else {
                if let Some(p) = dest.parent() {
                    fs::create_dir_all(p)?;
                }
                let mut f = fs::File::create(&dest)?;
                std::io::copy(&mut e, &mut f)?;
                set_mode(&dest, e.unix_mode().unwrap_or(0o644));
                count += 1;
            }
        }
    } else {
        fs::copy(file, out.join(file.file_name().unwrap_or_default()))?;
        set_mode(&out.join(file.file_name().unwrap_or_default()), 0o755);
        count = 1;
    }
    if count == 0 {
        bail!("{} is empty", file.display());
    }
    // One top folder: that's the item.
    let entries: Vec<PathBuf> = fs::read_dir(out)?.flatten().map(|e| e.path()).collect();
    if entries.len() == 1 && entries[0].is_dir() {
        return Ok(entries[0].clone());
    }
    Ok(out.to_path_buf())
}

#[cfg(unix)]
fn set_mode(p: &Path, mode: u32) {
    use std::os::unix::fs::PermissionsExt;
    let _ = fs::set_permissions(p, fs::Permissions::from_mode(mode & 0o755 | 0o600));
}
#[cfg(not(unix))]
fn set_mode(_: &Path, _: u32) {}

// ---------------------------------------------------------------- probe

/// Probes an engine library and returns the name it reports. Set by the app
/// (it loads the library in the pixel host process, never in-process).
pub type EngineProbe = fn(&Path) -> Result<String>;
static ENGINE_PROBE: OnceLock<EngineProbe> = OnceLock::new();

pub fn set_engine_probe(p: EngineProbe) {
    let _ = ENGINE_PROBE.set(p);
}

/// What an unpacked folder holds, checked: (name, version, kind, permissions).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Item {
    pub name: String,
    pub version: String,
    pub kind: Kind,
    pub permissions: Vec<String>,
    pub description: String,
}

/// Read and check the manifest in `dir` (writing an `engine.toml` for a bare
/// engine library), probe it, and check it is a `want`.
pub fn inspect(dir: &Path, want: Kind, asset_version: Option<&str>) -> Result<Item> {
    if !crate::engines::is_engine_dir(dir) && !dir.join("plugin.toml").is_file() {
        // A bare engine library: describe it from the probe.
        let lib = fs::read_dir(dir)?
            .flatten()
            .map(|e| e.path())
            .find(|p| p.extension().is_some_and(|e| e == std::env::consts::DLL_EXTENSION))
            .ok_or_else(|| anyhow!("no plugin.toml or engine.toml (and no {} library) in the download", std::env::consts::DLL_EXTENSION))?;
        let probe = ENGINE_PROBE.get().ok_or_else(|| anyhow!("engine probe not available"))?;
        let name = probe(&lib)?;
        let m = crate::engines::Manifest {
            name,
            version: asset_version.unwrap_or("0.0.0").to_string(),
            abi: crate::engines::ABI_MAX,
            library: lib.file_name().unwrap_or_default().to_string_lossy().into_owned(),
            description: String::new(),
        };
        fs::write(dir.join("engine.toml"), toml::to_string_pretty(&m)?)?;
    }
    let item = if crate::engines::is_engine_dir(dir) {
        let (m, lib) = crate::engines::source(dir)?;
        m.check()?;
        let probe = ENGINE_PROBE.get().ok_or_else(|| anyhow!("engine probe not available"))?;
        let reported = probe(&lib).with_context(|| format!("probing {}", lib.display()))?;
        if reported != m.name {
            bail!("library reports engine name {reported:?} but the manifest says {:?}", m.name);
        }
        Item { name: m.name, version: m.version, kind: Kind::Engine, permissions: vec![], description: m.description }
    } else {
        let m = crate::plugins::Manifest::parse(&fs::read_to_string(dir.join("plugin.toml"))?)?;
        Item {
            name: m.name.clone(),
            version: m.version.clone(),
            kind: Kind::Plugin,
            permissions: m.permissions.clone(),
            description: m.description.clone(),
        }
    };
    if item.kind != want {
        bail!("this is {} {}, not {} {} (`mouser {} install …`)", article(item.kind), item.kind.name(), article(want), want.name(), item.kind.name());
    }
    Ok(item)
}

fn article(k: Kind) -> &'static str {
    match k {
        Kind::Plugin => "a",
        Kind::Engine => "an",
    }
}

/// macOS: the code signature (warning when missing), then the quarantine
/// flag is cleared (only after everything else passed). Windows:
/// Authenticode (warning when unsigned).
fn platform_checks(dir: &Path) -> Vec<String> {
    let mut warn = Vec::new();
    if cfg!(target_os = "macos") {
        let ok = std::process::Command::new("codesign").arg("--verify").arg("--deep").arg(dir).output().is_ok_and(|o| o.status.success());
        if !ok {
            warn.push("not code-signed for macOS (codesign --verify failed)".into());
        }
        let _ = std::process::Command::new("xattr").args(["-dr", "com.apple.quarantine"]).arg(dir).output();
    } else if cfg!(windows) {
        let script = format!(
            "Get-ChildItem -Recurse -Include *.dll,*.exe -LiteralPath '{}' | Get-AuthenticodeSignature | Where-Object {{ $_.Status -ne 'Valid' }} | Measure-Object | Select-Object -ExpandProperty Count",
            dir.display()
        );
        if let Ok(o) = std::process::Command::new("powershell").args(["-NoProfile", "-Command", &script]).output()
            && String::from_utf8_lossy(&o.stdout).trim().parse::<u32>().is_ok_and(|n| n > 0)
        {
            warn.push("not Authenticode-signed".into());
        }
    }
    warn
}

// ---------------------------------------------------------------- store

/// `<root>/<name>/source.toml`: where an install came from, for `update`.
#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Record {
    pub src: String,
    /// Installed versions, oldest first; the last is current unless rolled back.
    #[serde(default)]
    pub versions: Vec<String>,
    #[serde(default)]
    pub trust: Option<String>,
}

fn record_path(kind: Kind, name: &str) -> PathBuf {
    kind.root().join(name).join("source.toml")
}

pub fn record(kind: Kind, name: &str) -> Option<Record> {
    fs::read_to_string(record_path(kind, name)).ok().and_then(|s| toml::from_str(&s).ok())
}

/// The version `current` names.
pub fn current(kind: Kind, name: &str) -> Option<String> {
    fs::read_to_string(kind.root().join(name).join("current")).ok().map(|s| s.trim().to_string()).filter(|s| !s.is_empty())
}

fn version_dir(v: &str) -> String {
    let s: String = v.chars().map(|c| if c.is_ascii_alphanumeric() || "-_.+".contains(c) { c } else { '_' }).collect();
    if s.is_empty() || s.starts_with('.') { format!("v{s}") } else { s }
}

fn write_atomic(p: &Path, s: &str) -> Result<()> {
    let tmp = p.with_extension("tmp");
    fs::write(&tmp, s)?;
    fs::rename(&tmp, p)?;
    Ok(())
}

/// Move the checked folder into `<root>/<name>/<version>/`, switch `current`
/// to it and keep the newest [`KEEP_VERSIONS`].
fn place(kind: Kind, item: &Item, staged: &Path, src: &str, trust: &Trust) -> Result<PathBuf> {
    let base = kind.root().join(&item.name);
    // An older flat install (`<name>/plugin.toml`): becomes a version dir.
    if base.join("plugin.toml").is_file() || base.join("engine.toml").is_file() {
        let old = kind.root().join(format!(".{}.flat", item.name));
        let _ = fs::remove_dir_all(&old);
        fs::rename(&base, &old)?;
        fs::create_dir_all(&base)?;
        fs::rename(&old, base.join("previous"))?;
        write_atomic(&base.join("current"), "previous")?;
    }
    fs::create_dir_all(&base)?;
    let vd = version_dir(&item.version);
    let dest = base.join(&vd);
    let _ = fs::remove_dir_all(&dest);
    fs::rename(staged, &dest).or_else(|_| copy_dir(staged, &dest))?;
    write_atomic(&base.join("current"), &vd)?;
    let mut rec = record(kind, &item.name).unwrap_or_default();
    if let Some(prev) = rec.versions.last().cloned().or_else(|| base.join("previous").is_dir().then(|| "previous".to_string()))
        && prev != vd
        && !rec.versions.contains(&prev)
    {
        rec.versions.push(prev);
    }
    rec.versions.retain(|v| v != &vd);
    rec.versions.push(vd.clone());
    while rec.versions.len() > KEEP_VERSIONS {
        let old = rec.versions.remove(0);
        let _ = fs::remove_dir_all(base.join(&old));
    }
    rec.src = src.to_string();
    rec.trust = Some(trust.text());
    write_atomic(&record_path(kind, &item.name), &toml::to_string(&rec)?)?;
    Ok(dest)
}

fn copy_dir(from: &Path, to: &Path) -> Result<()> {
    fs::create_dir_all(to)?;
    for e in fs::read_dir(from)?.flatten() {
        let p = e.path();
        if p.is_dir() {
            copy_dir(&p, &to.join(e.file_name()))?;
        } else {
            fs::copy(&p, to.join(e.file_name()))?;
        }
    }
    Ok(())
}

/// Append to `$MOUSER_HOME/logs/plugins-audit.log`.
pub fn audit(line: &str) {
    let dir = crate::storage::resolve_home().join("logs");
    let _ = fs::create_dir_all(&dir);
    if let Ok(mut f) = fs::OpenOptions::new().create(true).append(true).open(dir.join("plugins-audit.log")) {
        let t = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0);
        let _ = writeln!(f, "{t} {line}");
    }
}

// ---------------------------------------------------------------- the operations

/// What the user is asked before an install (the host-drawn card).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Card {
    pub kind: Kind,
    pub name: String,
    pub version: String,
    pub source: String,
    pub signer: String,
    pub permissions: Vec<String>,
    pub warnings: Vec<String>,
}

impl Card {
    pub fn lines(&self) -> Vec<String> {
        let mut v = vec![
            format!("Install {} {} {}?", self.kind.name(), self.name, self.version),
            format!("from: {}", self.source),
            format!("signer: {}", self.signer),
        ];
        if self.permissions.is_empty() {
            v.push("asks for: nothing".into());
        } else {
            v.push("asks for:".into());
            v.extend(self.permissions.iter().map(|c| format!("  {c:<16} {}", crate::plugins::capability_text(c))));
        }
        v.extend(self.warnings.iter().map(|w| format!("warning: {w}")));
        v
    }
}

/// Options shared by the CLI and the `esc :` line.
#[derive(Clone, Copy, Default)]
pub struct Opts<'a> {
    /// Install without asking (the CLI's `--yes`).
    pub yes: bool,
    pub version: Option<&'a str>,
    /// First-run install of an official item: no card (decision 4).
    pub first_run: bool,
}

/// The answer to a [`Card`]; `None` means "nobody to ask" (refused).
pub type Confirm<'a> = &'a mut dyn FnMut(&Card) -> bool;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Installed {
    pub kind: Kind,
    pub name: String,
    pub version: String,
    pub dir: PathBuf,
    pub trust: Trust,
    pub warnings: Vec<String>,
    /// The version it replaced.
    pub replaced: Option<String>,
}

impl Installed {
    pub fn summary(&self) -> String {
        let r =
            self.replaced.as_ref().map(|v| format!(" (was {v}; `mouser {} rollback {}` goes back)", self.kind.name(), self.name)).unwrap_or_default();
        let w = if self.warnings.is_empty() { String::new() } else { format!("; warnings: {}", self.warnings.join("; ")) };
        format!("installed {} {} {}{r}; {}{w}", self.kind.name(), self.name, self.version, self.trust.text())
    }
}

/// Install `src` as a `kind`: resolve, download, verify, unpack, probe, ask, place.
pub fn install(src: &Src, kind: Kind, opts: Opts<'_>, progress: Progress<'_>, confirm: Confirm<'_>) -> Result<Installed> {
    match src {
        Src::Path(p) => return install_path(p, kind, opts, confirm),
        Src::Crate(c) => return install_crate(c, kind, opts, confirm),
        _ => {}
    }
    let a = resolve(src, kind, opts.version)?;
    let work = tempdir_in(&kind.root())?;
    let r = (|| {
        let file = download(&a, &work.join("dl"), progress)?;
        let trust = verify(&a, &file)?;
        let root = unpack(&file, &work.join("unpacked"))?;
        let item = inspect(&root, kind, a.version.as_deref())?;
        if let Src::Official(n) = src
            && &item.name != n
        {
            bail!("the official {n} asset holds {:?}", item.name);
        }
        let mut warnings = platform_checks(&root);
        if trust == Trust::Unsigned {
            warnings.insert(0, "unsigned: anyone who controls the server could have changed it".into());
        }
        let card = Card {
            kind,
            name: item.name.clone(),
            version: item.version.clone(),
            source: format!("{src} ({})", a.url),
            signer: trust.text(),
            permissions: item.permissions.clone(),
            warnings: warnings.clone(),
        };
        ask(&card, &item, opts, a.official, confirm)?;
        finish(kind, &item, &root, &src.to_string(), trust.clone(), warnings, &a)
    })();
    let _ = fs::remove_dir_all(&work);
    if let Err(e) = &r {
        audit(&format!("{} install {src} failed: {e:#}", kind.name()));
    }
    r
}

fn ask(card: &Card, item: &Item, opts: Opts<'_>, official: bool, confirm: Confirm<'_>) -> Result<()> {
    // Official first-run items skip the card (decision 4); an update that
    // adds permissions always asks.
    let added = added_permissions(card.kind, &item.name, &item.permissions);
    let needed = !(official && opts.first_run) || !added.is_empty();
    if needed && !opts.yes && !confirm(card) {
        bail!("not installed: {} {} was declined", card.kind.name(), card.name);
    }
    Ok(())
}

/// Permissions `perms` asks for that the installed version didn't.
fn added_permissions(kind: Kind, name: &str, perms: &[String]) -> Vec<String> {
    if kind != Kind::Plugin {
        return vec![];
    }
    let Some(old) = crate::plugins::find(name) else { return vec![] };
    perms.iter().filter(|p| !old.manifest.permissions.contains(p)).cloned().collect()
}

fn finish(kind: Kind, item: &Item, root: &Path, src: &str, trust: Trust, warnings: Vec<String>, a: &Asset) -> Result<Installed> {
    if let Trust::Author { key, new: true } = &trust {
        remember_key(&a.origin, key)?;
    }
    let replaced = current(kind, &item.name);
    let dir = place(kind, item, root, src, &trust)?;
    after_place(kind, &item.name);
    audit(&format!("{} install {} {} from {src}: {}", kind.name(), item.name, item.version, trust.text()));
    Ok(Installed {
        kind,
        name: item.name.clone(),
        version: item.version.clone(),
        dir,
        trust,
        warnings,
        replaced: replaced.filter(|r| r != &version_dir(&item.version)),
    })
}

/// What the old install functions did after copying: priority entry, crash records.
fn after_place(kind: Kind, name: &str) {
    match kind {
        Kind::Engine => {
            crate::engines::clear_sentinel(name);
            crate::engines::clear_failure(name);
        }
        Kind::Plugin => crate::plugins::note_installed(name),
    }
}

fn tempdir_in(root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(root)?;
    let n = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let d = root.join(format!(".dl-{}-{n}", std::process::id()));
    fs::create_dir_all(&d)?;
    Ok(d)
}

fn install_path(p: &Path, kind: Kind, opts: Opts<'_>, confirm: Confirm<'_>) -> Result<Installed> {
    let p = if let Some(rest) = p.to_str().and_then(|s| s.strip_prefix("~/")) {
        dirs::home_dir().unwrap_or_default().join(rest)
    } else {
        p.to_path_buf()
    };
    if !p.exists() {
        bail!("{} does not exist", p.display());
    }
    let work = tempdir_in(&kind.root())?;
    let r = (|| {
        let is_archive = p.is_file() && {
            let n = p.to_string_lossy().to_lowercase();
            n.ends_with(".tar.gz") || n.ends_with(".tgz") || n.ends_with(".zip")
        };
        let src_dir = if is_archive {
            unpack(&p, &work.join("unpacked"))?
        } else {
            let d = if p.is_dir() { p.clone() } else { p.parent().map(Path::to_path_buf).unwrap_or_else(|| PathBuf::from(".")) };
            let into = work.join("copy");
            copy_dir_shallow(&d, &into)?;
            into
        };
        let item = inspect(&src_dir, kind, None)?;
        let card = Card {
            kind,
            name: item.name.clone(),
            version: item.version.clone(),
            source: p.display().to_string(),
            signer: Trust::Local.text(),
            permissions: item.permissions.clone(),
            warnings: vec![],
        };
        // A local install is the user's own choice: asked only when it adds permissions.
        if !added_permissions(kind, &item.name, &item.permissions).is_empty() {
            ask(&card, &item, opts, false, confirm)?;
        }
        if kind == Kind::Plugin {
            crate::plugins::check_commands_free(&src_dir)?;
        }
        let a = Asset { origin: p.display().to_string(), ..Default::default() };
        finish(kind, &item, &src_dir, &p.display().to_string(), Trust::Local, vec![], &a)
    })();
    let _ = fs::remove_dir_all(&work);
    r
}

/// Copy a source folder: its files and one level of subfolders (no dot files).
fn copy_dir_shallow(from: &Path, to: &Path) -> Result<()> {
    fn go(from: &Path, to: &Path, depth: u32) -> Result<()> {
        fs::create_dir_all(to)?;
        for e in fs::read_dir(from)?.flatten() {
            let n = e.file_name();
            if n.to_string_lossy().starts_with('.') {
                continue;
            }
            let ft = e.file_type()?;
            if ft.is_dir() && depth > 0 {
                go(&e.path(), &to.join(&n), depth - 1)?;
            } else if ft.is_file() {
                fs::copy(e.path(), to.join(&n)).with_context(|| format!("copying {}", e.path().display()))?;
            }
        }
        Ok(())
    }
    go(from, to, 2)
}

/// `crate:NAME`: `cargo binstall` the mode plugin, then register it from its
/// `--manifest` output.
fn install_crate(name: &str, kind: Kind, opts: Opts<'_>, confirm: Confirm<'_>) -> Result<Installed> {
    if kind != Kind::Plugin {
        bail!("crate:NAME installs mode plugins only; engines come as signed release assets");
    }
    let cargo = std::env::var("MOUSER_CARGO").unwrap_or_else(|_| "cargo".into());
    let work = tempdir_in(&kind.root())?;
    let r = (|| {
        let out = std::process::Command::new(&cargo)
            .args(["binstall", "--no-confirm", "--quiet", "--root"])
            .arg(work.join("root"))
            .arg(name)
            .output()
            .with_context(|| format!("running {cargo} binstall (install cargo-binstall first)"))?;
        if !out.status.success() {
            bail!("cargo binstall {name} failed: {}", String::from_utf8_lossy(&out.stderr).trim());
        }
        let bin = work.join("root/bin").join(format!("{name}{}", std::env::consts::EXE_SUFFIX));
        let m = std::process::Command::new(&bin).arg("--manifest").output().with_context(|| format!("running {} --manifest", bin.display()))?;
        if !m.status.success() {
            bail!("{name} --manifest failed: it is not a Mouser mode plugin");
        }
        let d = work.join("item");
        fs::create_dir_all(&d)?;
        fs::copy(&bin, d.join(bin.file_name().unwrap_or_default()))?;
        fs::write(d.join("plugin.toml"), &m.stdout)?;
        let item = inspect(&d, kind, None)?;
        let card = Card {
            kind,
            name: item.name.clone(),
            version: item.version.clone(),
            source: format!("crate:{name} (cargo binstall)"),
            signer: "crates.io (not signed by Mouser)".into(),
            permissions: item.permissions.clone(),
            warnings: vec![],
        };
        ask(&card, &item, opts, false, confirm)?;
        let a = Asset { origin: format!("crate:{name}"), ..Default::default() };
        finish(kind, &item, &d, &format!("crate:{name}"), Trust::Local, vec![], &a)
    })();
    let _ = fs::remove_dir_all(&work);
    r
}

/// Names installed under `kind`'s root that `update` can reach.
pub fn updatable(kind: Kind) -> Vec<String> {
    let Ok(rd) = fs::read_dir(kind.root()) else { return vec![] };
    let mut v: Vec<String> =
        rd.flatten().filter(|e| e.path().join("source.toml").is_file()).map(|e| e.file_name().to_string_lossy().into_owned()).collect();
    v.sort();
    v
}

/// `update NAME`: install the newest version from where it came from (a
/// local path is re-read). Ok(None) when it is already current.
pub fn update(kind: Kind, name: &str, opts: Opts<'_>, progress: Progress<'_>, confirm: Confirm<'_>) -> Result<Option<Installed>> {
    let rec = record(kind, name).ok_or_else(|| anyhow!("{name} was not installed by `mouser {} install` (nothing to update from)", kind.name()))?;
    let mut src = Src::parse(&rec.src)?;
    // A pinned tag stays pinned only with --version; update means "latest".
    if let Src::Github { tag, .. } = &mut src
        && opts.version.is_none()
    {
        *tag = None;
    }
    if !matches!(src, Src::Path(_) | Src::Crate(_)) {
        let a = resolve(&src, kind, opts.version)?;
        if let (Some(v), Some(c)) = (&a.version, current(kind, name))
            && version_dir(v) == c
        {
            return Ok(None);
        }
    }
    install(&src, kind, opts, progress, confirm).map(Some)
}

/// `rollback NAME`: switch `current` back to the version before.
pub fn rollback(kind: Kind, name: &str) -> Result<String> {
    let mut rec = record(kind, name).ok_or_else(|| anyhow!("{name} has no earlier version to roll back to"))?;
    let cur = current(kind, name).unwrap_or_default();
    let prev = rec.versions.iter().rev().find(|v| **v != cur && kind.root().join(name).join(v).is_dir()).cloned();
    let Some(prev) = prev else { bail!("{name} has no earlier version to roll back to (only {cur} is kept)") };
    write_atomic(&kind.root().join(name).join("current"), &prev)?;
    rec.versions.retain(|v| v != &prev);
    rec.versions.push(prev.clone());
    write_atomic(&record_path(kind, name), &toml::to_string(&rec)?)?;
    after_place(kind, name);
    audit(&format!("{} rollback {name}: {cur} -> {prev}", kind.name()));
    Ok(prev)
}

/// The lock's items for this kind that aren't installed or shipped, and the
/// ones that could come from the offline bundle (`plugins/` beside Mouser).
pub fn missing_official() -> Vec<LockItem> {
    lock()
        .item
        .into_iter()
        .filter(|i| match i.kind {
            Kind::Engine => crate::engines::find(&i.name).is_none(),
            Kind::Plugin => crate::plugins::find(&i.name).is_none(),
        })
        .collect()
}

/// An offline-bundle copy of a lock asset (`plugins/<file>` beside Mouser, or
/// `MOUSER_OFFLINE_BUNDLE`), if there is one.
pub fn offline_copy(a: &LockAsset) -> Option<PathBuf> {
    let file = a.url.rsplit('/').next()?;
    let mut dirs: Vec<PathBuf> = Vec::new();
    if let Some(d) = std::env::var_os("MOUSER_OFFLINE_BUNDLE") {
        dirs.push(PathBuf::from(d));
    } else if let Some(exe) = std::env::current_exe().ok().and_then(|e| e.parent().map(Path::to_path_buf)) {
        dirs.push(exe.join("plugins"));
        dirs.push(exe.join("../Resources/plugins"));
    }
    dirs.into_iter().map(|d| d.join(file)).find(|p| p.is_file())
}

/// Install an official lock item from the offline bundle: the same checks
/// (sha256, the official signature, probe) without the network.
pub fn install_offline(it: &LockItem, file: &Path, opts: Opts<'_>, confirm: Confirm<'_>) -> Result<Installed> {
    let ts = targets();
    let la = ts.iter().find_map(|t| it.assets.get(t)).ok_or_else(|| anyhow!("{} has no asset for this machine", it.name))?;
    let a = Asset {
        file: file.file_name().unwrap_or_default().to_string_lossy().into_owned(),
        url: file.display().to_string(),
        version: Some(it.version.clone()),
        sha256: Some(la.sha256.clone()),
        size: Some(la.size),
        minisig: Some(la.minisig.clone()),
        official: true,
        author_key: None,
        origin: format!("official:{}", it.name),
    };
    let got = hex::encode(sha2::Sha256::digest(fs::read(file)?));
    if !got.eq_ignore_ascii_case(&la.sha256) {
        bail!("{}: sha256 mismatch in the offline bundle", file.display());
    }
    let trust = verify(&a, file)?;
    let work = tempdir_in(&it.kind.root())?;
    let r = (|| {
        let root = unpack(file, &work.join("unpacked"))?;
        let item = inspect(&root, it.kind, Some(&it.version))?;
        if item.name != it.name {
            bail!("the offline {} asset holds {:?}", it.name, item.name);
        }
        let card = Card {
            kind: it.kind,
            name: item.name.clone(),
            version: item.version.clone(),
            source: format!("offline bundle {}", file.display()),
            signer: trust.text(),
            permissions: item.permissions.clone(),
            warnings: vec![],
        };
        ask(&card, &item, opts, true, confirm)?;
        finish(it.kind, &item, &root, &it.name, trust.clone(), vec![], &a)
    })();
    let _ = fs::remove_dir_all(&work);
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_parse() {
        assert_eq!(Src::parse("commander").unwrap(), Src::Official("commander".into()));
        assert_eq!(
            Src::parse("gh:WeirdBrains/mouser-chat@v0.2.0").unwrap(),
            Src::Github { owner: "WeirdBrains".into(), repo: "mouser-chat".into(), tag: Some("v0.2.0".into()) }
        );
        assert_eq!(Src::parse("gh:a/b").unwrap(), Src::Github { owner: "a".into(), repo: "b".into(), tag: None });
        assert!(Src::parse("gh:a").is_err() && Src::parse("gh:a/../b").is_err() && Src::parse("gh:a/b@").is_err());
        assert!(Src::parse("http://x/y").unwrap_err().to_string().contains("https"));
        assert_eq!(Src::parse("https://x/y.tar.gz").unwrap(), Src::Url("https://x/y.tar.gz".into()));
        assert_eq!(Src::parse("./flat").unwrap(), Src::Path("./flat".into()));
        assert_eq!(Src::parse("crate:mouser-chat").unwrap(), Src::Crate("mouser-chat".into()));
        assert!(Src::parse("Not A Name").is_err());
        for s in ["commander", "gh:a/b@v1", "https://x/y.zip", "crate:x"] {
            assert_eq!(Src::parse(s).unwrap().to_string(), s);
        }
    }

    #[test]
    fn asset_names_and_selection() {
        assert_eq!(
            split_asset_name("mouser-chat-0.2.0-x86_64-unknown-linux-gnu.tar.gz"),
            Some(("mouser-chat".into(), "0.2.0".into(), "x86_64-unknown-linux-gnu".into(), "tar.gz".into()))
        );
        assert_eq!(split_asset_name("skia-v1.4.0-universal-apple-darwin.zip").unwrap().1, "1.4.0");
        assert_eq!(split_asset_name("README.md"), None);
        let mine = targets()[0].clone();
        let a = format!("x-1.0.0-{mine}.tar.gz");
        let files = ["x-1.0.0-sparc-sun-solaris.tar.gz", a.as_str(), "x-1.0.0-x.tar.gz.sha256"];
        assert_eq!(pick_asset(&files, Some("x")).unwrap(), a);
        let e = pick_asset(&["x-1.0.0-riscv64gc-unknown-linux-gnu.tar.gz"], None).unwrap_err().to_string();
        assert!(e.contains("no asset for") && e.contains("riscv64gc"), "{e}");
    }

    #[test]
    fn unpack_refuses_escapes() {
        let t = tempfile::tempdir().unwrap();
        // A tar.gz with ../evil.
        let p = t.path().join("bad.tar.gz");
        {
            let gz = flate2::write::GzEncoder::new(fs::File::create(&p).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            let mut h = tar::Header::new_gnu();
            let data = b"x";
            h.set_size(1);
            h.set_mode(0o644);
            h.as_gnu_mut().unwrap().name[..9].copy_from_slice(b"../evil\0\0");
            h.set_cksum();
            b.append(&h, &data[..]).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let e = unpack(&p, &t.path().join("out")).unwrap_err().to_string();
        assert!(e.contains("escapes"), "{e}");
        assert!(!t.path().join("evil").exists());
        // A good zip with one top folder is entered.
        let z = t.path().join("ok.zip");
        {
            let mut w = zip::ZipWriter::new(fs::File::create(&z).unwrap());
            w.start_file("item/plugin.toml", zip::write::SimpleFileOptions::default()).unwrap();
            w.write_all(b"x").unwrap();
            w.finish().unwrap();
        }
        let root = unpack(&z, &t.path().join("out2")).unwrap();
        assert!(root.ends_with("item") && root.join("plugin.toml").is_file());
        // A tar.gz made with `tar -C stage -czf x.tar.gz .` (a `./` entry, then `./plugin.toml`).
        let dot = t.path().join("dot.tar.gz");
        {
            let stage = t.path().join("stage");
            fs::create_dir_all(&stage).unwrap();
            fs::write(stage.join("plugin.toml"), "x").unwrap();
            let gz = flate2::write::GzEncoder::new(fs::File::create(&dot).unwrap(), flate2::Compression::fast());
            let mut b = tar::Builder::new(gz);
            b.append_dir_all(".", &stage).unwrap();
            b.into_inner().unwrap().finish().unwrap();
        }
        let root = unpack(&dot, &t.path().join("out3")).unwrap();
        assert!(root.join("plugin.toml").is_file(), "{}", root.display());
    }

    #[test]
    fn the_shipped_lock_parses_and_the_official_key_is_pinned() {
        let l: Lock = toml::from_str(include_str!("plugins.lock")).unwrap();
        for (name, kind) in
            [("commander", Kind::Plugin), ("chat", Kind::Plugin), ("preferences", Kind::Plugin), ("mouser", Kind::Engine), ("skia", Kind::Engine)]
        {
            let i = l.item.iter().find(|i| i.name == name).unwrap_or_else(|| panic!("{name} in plugins.lock"));
            assert_eq!(i.kind, kind);
            for t in ["x86_64-unknown-linux-gnu", "aarch64-apple-darwin", "x86_64-apple-darwin", "x86_64-pc-windows-msvc"] {
                let a = &i.assets[t];
                assert!(a.url.starts_with("https://github.com/WeirdBrains/") && a.sha256.len() == 64 && a.size > 0, "{name} {t}");
                assert!(minisign_verify::Signature::decode(&a.minisig).is_ok(), "{name} {t}");
            }
        }
        let k = OFFICIAL_KEY.expect("the WeirdBrains key is pinned");
        assert!(minisign_verify::PublicKey::from_base64(k).is_ok());
    }
}
