//! Secrets (the MCP server token now; API keys in P4): the OS keychain first
//! (macOS Keychain, Windows Credential Manager, Secret Service on Linux), else a
//! 0600 file under $MOUSER_HOME with a warning. `MOUSER_SECRET_STORE=file` forces
//! the file (tests, headless boxes). Values are never logged.

use anyhow::{Context, Result};
use std::fs;
use std::path::PathBuf;

/// Where a secret ended up.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Store {
    Keychain,
    /// The fallback file (the caller shows the warning).
    File(PathBuf),
}

impl Store {
    pub fn describe(&self) -> String {
        match self {
            Store::Keychain => "the OS keychain".into(),
            Store::File(p) => format!("{} (0600; no OS keychain available)", p.display()),
        }
    }
}

const SERVICE: &str = "mouser";

/// The keychain account: the name plus the MOUSER_HOME, so two homes don't share it.
fn account(name: &str) -> String {
    use sha2::Digest;
    let h = sha2::Sha256::digest(crate::storage::resolve_home().display().to_string().as_bytes());
    format!("{name}@{}", hex::encode(&h[..6]))
}

fn file(name: &str) -> PathBuf {
    crate::storage::resolve_home().join("secrets").join(name)
}

fn force_file() -> bool {
    std::env::var("MOUSER_SECRET_STORE").is_ok_and(|v| v == "file")
}

fn keychain_get(name: &str) -> Option<String> {
    if force_file() {
        return None;
    }
    keyring::Entry::new(SERVICE, &account(name)).ok()?.get_password().ok()
}

fn keychain_set(name: &str, v: &str) -> bool {
    !force_file() && keyring::Entry::new(SERVICE, &account(name)).and_then(|e| e.set_password(v)).is_ok() && keychain_get(name).as_deref() == Some(v)
}

pub fn get(name: &str) -> Option<(String, Store)> {
    if let Some(v) = keychain_get(name) {
        return Some((v, Store::Keychain));
    }
    let p = file(name);
    fs::read_to_string(&p).ok().map(|v| (v.trim().to_string(), Store::File(p)))
}

pub fn set(name: &str, v: &str) -> Result<Store> {
    if keychain_set(name, v) {
        let _ = fs::remove_file(file(name));
        return Ok(Store::Keychain);
    }
    let p = file(name);
    let d = p.parent().context("secrets dir")?;
    fs::create_dir_all(d)?;
    let tmp = d.join(format!(".{name}.tmp"));
    {
        let mut o = fs::OpenOptions::new();
        o.write(true).create(true).truncate(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::{OpenOptionsExt, PermissionsExt};
            o.mode(0o600);
            let _ = fs::set_permissions(d, fs::Permissions::from_mode(0o700));
        }
        use std::io::Write;
        o.open(&tmp)?.write_all(v.as_bytes())?;
    }
    fs::rename(&tmp, &p)?;
    Ok(Store::File(p))
}

pub fn delete(name: &str) {
    if !force_file()
        && let Ok(e) = keyring::Entry::new(SERVICE, &account(name))
    {
        let _ = e.delete_credential();
    }
    let _ = fs::remove_file(file(name));
}

/// A new random token (32 bytes, hex) from the OS RNG.
pub fn random_token() -> String {
    let mut b = [0u8; 32];
    rustls::crypto::ring::default_provider().secure_random.fill(&mut b).expect("OS random");
    hex::encode(b)
}

/// The MCP server token, made on first use.
pub fn mcp_token() -> Result<(String, Store)> {
    if let Some(t) = get("mcp-token").filter(|t| t.0.len() >= 32) {
        return Ok(t);
    }
    let t = random_token();
    let s = set("mcp-token", &t)?;
    Ok((t, s))
}

/// Replace the MCP token (old clients stop working).
pub fn rotate_mcp_token() -> Result<Store> {
    set("mcp-token", &random_token())
}

/// Equal in constant time (for token checks).
pub fn ct_eq(a: &[u8], b: &[u8]) -> bool {
    a.len() == b.len() && a.iter().zip(b).fold(0u8, |x, (p, q)| x | (p ^ q)) == 0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn file_fallback_is_private_and_token_is_stable_until_rotated() {
        let t = tempfile::tempdir().unwrap();
        unsafe {
            std::env::set_var("MOUSER_HOME", t.path());
            std::env::set_var("MOUSER_SECRET_STORE", "file");
        }
        let (a, s) = mcp_token().unwrap();
        assert_eq!(a.len(), 64);
        assert!(matches!(s, Store::File(_)));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let m = fs::metadata(file("mcp-token")).unwrap().permissions().mode() & 0o777;
            assert_eq!(m, 0o600);
        }
        assert_eq!(mcp_token().unwrap().0, a);
        rotate_mcp_token().unwrap();
        assert_ne!(mcp_token().unwrap().0, a);
        assert!(ct_eq(b"abc", b"abc") && !ct_eq(b"abc", b"abd") && !ct_eq(b"ab", b"abc"));
        delete("mcp-token");
        assert!(get("mcp-token").is_none());
        unsafe {
            std::env::remove_var("MOUSER_SECRET_STORE");
            std::env::remove_var("MOUSER_HOME");
        }
    }
}
