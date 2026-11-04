//! Registering Mouser as a Windows default-terminal choice (docs/windows.md
//! §Default terminal). All of it is per user (`HKEY_CURRENT_USER`).
//!
//! What Windows reads:
//! - `HKCU\Console\%%Startup` `DelegationConsole` / `DelegationTerminal`
//!   (REG_SZ CLSIDs). The inbox conhost hands a new console session to the
//!   console host named by `DelegationConsole` (Windows Terminal's OpenConsole),
//!   which then calls `ITerminalHandoff3::EstablishPtyHandoff` on the COM
//!   class named by `DelegationTerminal`.
//! - The Settings "Default terminal application" list comes only from packaged
//!   apps' `com.microsoft.windows.console.host` / `.terminal.host` app
//!   extensions, so an unpackaged program cannot appear there.
//!
//! So Mouser registers its handoff class under `HKCU\Software\Classes\CLSID`
//! (`LocalServer32` = mouser.exe; COM adds `-Embedding`) and, only when the
//! user asks (`mouser default-terminal set`), points `DelegationTerminal` at it
//! with Windows Terminal's OpenConsole as `DelegationConsole`. Registration
//! never changes the default; unregistering puts back what was there before
//! if (and only if) the current default is still Mouser.
//!
//! The registry is behind [`Registry`] so this logic is tested on every OS.

use anyhow::{Result, bail};
use std::collections::BTreeMap;
use std::path::Path;

/// Mouser's `ITerminalHandoff3` COM class.
pub const MOUSER_HANDOFF_CLSID: &str = "{EE272979-892E-4723-9CE6-56F6C13F92EE}";
/// `ITerminalHandoff3` / `ITerminalHandoff2` interface ids (microsoft/terminal
/// `src/host/proxy/ITerminalHandoff.idl`).
pub const IID_ITERMINALHANDOFF3: &str = "{6F23DA90-15C5-4203-9DB0-64E73F1B1B00}";
pub const IID_ITERMINALHANDOFF2: &str = "{AA6B364F-4A50-4176-9002-0AE755E7B5EF}";

/// "Let Windows decide" (both values) and the legacy Windows Console Host.
pub const CLSID_DEFAULT: &str = "{00000000-0000-0000-0000-000000000000}";
pub const CLSID_CONHOST: &str = "{B23D10C0-E52E-411E-9D5B-C09FDF709C7D}";

pub const STARTUP_KEY: &str = r"Console\%%Startup";
pub const DELEGATION_CONSOLE: &str = "DelegationConsole";
pub const DELEGATION_TERMINAL: &str = "DelegationTerminal";

/// Our bookkeeping: that we registered, and what `%%Startup` held before `set`.
pub const MOUSER_KEY: &str = r"Software\Mouser\DefaultTerminal";
const PREV_CONSOLE: &str = "PreviousDelegationConsole";
const PREV_TERMINAL: &str = "PreviousDelegationTerminal";
/// Stored in place of a previous value that did not exist.
const ABSENT: &str = "<absent>";

pub fn clsid_key() -> String {
    format!(r"Software\Classes\CLSID\{MOUSER_HANDOFF_CLSID}")
}

/// A Windows Terminal build whose OpenConsole can hand sessions to Mouser.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowsTerminal {
    Stable,
    Preview,
}

impl WindowsTerminal {
    /// Package family name (`%LOCALAPPDATA%\Microsoft\WindowsApps\<pfn>`).
    pub fn family(self) -> &'static str {
        match self {
            Self::Stable => "Microsoft.WindowsTerminal_8wekyb3d8bbwe",
            Self::Preview => "Microsoft.WindowsTerminalPreview_8wekyb3d8bbwe",
        }
    }
    /// Its OpenConsole (`IConsoleHandoff`) CLSID, used as `DelegationConsole`.
    pub fn console_clsid(self) -> &'static str {
        match self {
            Self::Stable => "{2EACA947-7F5F-4CFA-BA87-8F7FBEEFBE69}",
            Self::Preview => "{06EC847C-C0A5-46B8-92CB-7C92F6E35CD5}",
        }
    }
    /// Its own terminal CLSID (`DelegationTerminal` when it is the default).
    pub fn terminal_clsid(self) -> &'static str {
        match self {
            Self::Stable => "{E12CFF52-A866-4C77-9A90-F570A7AA2C6B}",
            Self::Preview => "{86633F1F-6454-40EC-89CE-DA4EBA977EE2}",
        }
    }
}

/// Per-user registry access, `HKCU`-relative paths. `name` "" is the
/// key's default value.
pub trait Registry {
    fn get(&self, key: &str, name: &str) -> Option<String>;
    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<()>;
    /// Delete a value; missing is fine.
    fn delete_value(&mut self, key: &str, name: &str) -> Result<()>;
    /// Delete a key and everything under it; missing is fine.
    fn delete_tree(&mut self, key: &str) -> Result<()>;
    fn key_exists(&self, key: &str) -> bool;
    /// The key has no values and no subkeys.
    fn is_empty_key(&self, key: &str) -> bool;
}

/// An in-memory registry for tests (keys compare case-insensitively, as on Windows).
#[derive(Debug, Default, Clone)]
pub struct MemRegistry {
    pub values: BTreeMap<(String, String), String>,
    pub keys: std::collections::BTreeSet<String>,
}

impl MemRegistry {
    fn norm(s: &str) -> String {
        s.to_ascii_lowercase()
    }
}

impl Registry for MemRegistry {
    fn get(&self, key: &str, name: &str) -> Option<String> {
        self.values.get(&(Self::norm(key), Self::norm(name))).cloned()
    }
    fn set(&mut self, key: &str, name: &str, value: &str) -> Result<()> {
        let k = Self::norm(key);
        let mut p = String::new();
        for part in k.split('\\') {
            if !p.is_empty() {
                p.push('\\');
            }
            p.push_str(part);
            self.keys.insert(p.clone());
        }
        self.values.insert((k, Self::norm(name)), value.to_string());
        Ok(())
    }
    fn delete_value(&mut self, key: &str, name: &str) -> Result<()> {
        self.values.remove(&(Self::norm(key), Self::norm(name)));
        Ok(())
    }
    fn delete_tree(&mut self, key: &str) -> Result<()> {
        let k = Self::norm(key);
        let under = |p: &str| p == k || p.starts_with(&format!("{k}\\"));
        self.keys.retain(|p| !under(p));
        self.values.retain(|(p, _), _| !under(p));
        Ok(())
    }
    fn key_exists(&self, key: &str) -> bool {
        self.keys.contains(&Self::norm(key))
    }
    fn is_empty_key(&self, key: &str) -> bool {
        let k = Self::norm(key);
        let sub = format!("{k}\\");
        !self.keys.iter().any(|p| p.starts_with(&sub)) && !self.values.keys().any(|(p, _)| *p == k)
    }
}

/// Which Windows Terminal is installed for this user, Stable first. `exists`
/// checks a path (the package's `WindowsApps` alias folder under
/// `local_app_data`); the packaged COM class index in the registry counts too.
pub fn detect_windows_terminal(reg: &dyn Registry, local_app_data: Option<&Path>, exists: &dyn Fn(&Path) -> bool) -> Option<WindowsTerminal> {
    [WindowsTerminal::Stable, WindowsTerminal::Preview].into_iter().find(|wt| {
        let alias = local_app_data.is_some_and(|d| exists(&d.join("Microsoft").join("WindowsApps").join(wt.family())));
        let com = reg.key_exists(&format!(r"Software\Classes\PackagedCom\ClassIndex\{}", wt.console_clsid()));
        alias || com
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RegisterOutcome {
    Registered,
    /// Windows Terminal is not installed; nothing was written.
    SkippedNoWindowsTerminal,
}

/// Register Mouser's handoff class for this user. Never changes the default.
pub fn register(reg: &mut dyn Registry, exe: &Path, wt: Option<WindowsTerminal>) -> Result<RegisterOutcome> {
    if wt.is_none() {
        return Ok(RegisterOutcome::SkippedNoWindowsTerminal);
    }
    let exe = exe.to_string_lossy();
    if exe.contains('"') {
        bail!("unexpected quote in the Mouser path {exe}");
    }
    let key = clsid_key();
    reg.set(&key, "", "Mouser terminal handoff")?;
    reg.set(&format!(r"{key}\LocalServer32"), "", &format!("\"{exe}\""))?;
    reg.set(MOUSER_KEY, "Registered", "1")?;
    Ok(RegisterOutcome::Registered)
}

pub fn is_registered(reg: &dyn Registry) -> bool {
    reg.get(&format!(r"{}\LocalServer32", clsid_key()), "").is_some()
}

/// Whether `%%Startup` currently sends console sessions to Mouser.
pub fn is_default(reg: &dyn Registry) -> bool {
    reg.get(STARTUP_KEY, DELEGATION_TERMINAL).is_some_and(|v| v.eq_ignore_ascii_case(MOUSER_HANDOFF_CLSID))
}

/// Make Mouser the default terminal (only on explicit request): remember the
/// current pair, then point `%%Startup` at Windows Terminal's OpenConsole and
/// Mouser.
pub fn set_default(reg: &mut dyn Registry, wt: Option<WindowsTerminal>) -> Result<()> {
    let Some(wt) = wt else { bail!("Windows Terminal is not installed; Mouser needs its console host (OpenConsole) for handoff") };
    if !is_registered(reg) {
        bail!("Mouser is not registered; run `mouser default-terminal register` first");
    }
    if !is_default(reg) {
        for (name, prev) in [(DELEGATION_CONSOLE, PREV_CONSOLE), (DELEGATION_TERMINAL, PREV_TERMINAL)] {
            let v = reg.get(STARTUP_KEY, name).unwrap_or_else(|| ABSENT.to_string());
            reg.set(MOUSER_KEY, prev, &v)?;
        }
    }
    reg.set(STARTUP_KEY, DELEGATION_CONSOLE, wt.console_clsid())?;
    reg.set(STARTUP_KEY, DELEGATION_TERMINAL, MOUSER_HANDOFF_CLSID)?;
    Ok(())
}

/// If Mouser is the default, put back the pair from before `set_default`
/// (or remove the values, i.e. "Let Windows decide", if they did not exist).
/// Leaves `%%Startup` alone when something else is the default.
pub fn restore_default(reg: &mut dyn Registry) -> Result<bool> {
    let ours = is_default(reg);
    if ours {
        for (name, prev) in [(DELEGATION_CONSOLE, PREV_CONSOLE), (DELEGATION_TERMINAL, PREV_TERMINAL)] {
            match reg.get(MOUSER_KEY, prev) {
                Some(v) if v != ABSENT && !v.eq_ignore_ascii_case(MOUSER_HANDOFF_CLSID) => reg.set(STARTUP_KEY, name, &v)?,
                _ => reg.delete_value(STARTUP_KEY, name)?,
            }
        }
    }
    reg.delete_value(MOUSER_KEY, PREV_CONSOLE)?;
    reg.delete_value(MOUSER_KEY, PREV_TERMINAL)?;
    Ok(ours)
}

/// Remove everything Mouser wrote (uninstall): restore the default if it is
/// ours, delete the COM class and our bookkeeping key.
pub fn unregister(reg: &mut dyn Registry) -> Result<()> {
    restore_default(reg)?;
    reg.delete_tree(&clsid_key())?;
    reg.delete_tree(MOUSER_KEY)?;
    // Our parent key goes too if nothing else lives there.
    if reg.key_exists(r"Software\Mouser") && reg.is_empty_key(r"Software\Mouser") {
        reg.delete_tree(r"Software\Mouser")?;
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub windows_terminal: Option<WindowsTerminal>,
    pub registered: bool,
    pub is_default: bool,
    pub console: Option<String>,
    pub terminal: Option<String>,
}

pub fn status(reg: &dyn Registry, wt: Option<WindowsTerminal>) -> Status {
    Status {
        windows_terminal: wt,
        registered: is_registered(reg),
        is_default: is_default(reg),
        console: reg.get(STARTUP_KEY, DELEGATION_CONSOLE),
        terminal: reg.get(STARTUP_KEY, DELEGATION_TERMINAL),
    }
}

/// A friendly name for a delegation CLSID.
pub fn describe(clsid: Option<&str>) -> &'static str {
    let Some(c) = clsid else { return "Let Windows decide (not set)" };
    let is = |x: &str| c.eq_ignore_ascii_case(x);
    if is(MOUSER_HANDOFF_CLSID) {
        "Mouser"
    } else if is(CLSID_DEFAULT) {
        "Let Windows decide"
    } else if is(CLSID_CONHOST) {
        "Windows Console Host"
    } else if is(WindowsTerminal::Stable.terminal_clsid()) || is(WindowsTerminal::Stable.console_clsid()) {
        "Windows Terminal"
    } else if is(WindowsTerminal::Preview.terminal_clsid()) || is(WindowsTerminal::Preview.console_clsid()) {
        "Windows Terminal Preview"
    } else {
        "another terminal"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    const WT: Option<WindowsTerminal> = Some(WindowsTerminal::Stable);

    fn exe() -> PathBuf {
        PathBuf::from(r"C:\Program Files\Mouser\mouser.exe")
    }

    #[test]
    fn register_needs_windows_terminal_and_writes_local_server() {
        let mut r = MemRegistry::default();
        assert_eq!(register(&mut r, &exe(), None).unwrap(), RegisterOutcome::SkippedNoWindowsTerminal);
        assert!(r.values.is_empty() && r.keys.is_empty());
        assert_eq!(register(&mut r, &exe(), WT).unwrap(), RegisterOutcome::Registered);
        assert_eq!(r.get(&format!(r"{}\LocalServer32", clsid_key()), "").unwrap(), r#""C:\Program Files\Mouser\mouser.exe""#);
        assert!(is_registered(&r));
        // Registering never touches the default.
        assert!(!r.key_exists(STARTUP_KEY));
        assert!(!is_default(&r));
    }

    #[test]
    fn set_requires_registration_and_wt() {
        let mut r = MemRegistry::default();
        assert!(set_default(&mut r, WT).is_err());
        register(&mut r, &exe(), WT).unwrap();
        assert!(set_default(&mut r, None).is_err());
        set_default(&mut r, WT).unwrap();
        assert!(is_default(&r));
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_CONSOLE).unwrap(), WindowsTerminal::Stable.console_clsid());
    }

    #[test]
    fn restore_puts_back_previous_pair() {
        let mut r = MemRegistry::default();
        let wt = WindowsTerminal::Stable;
        r.set(STARTUP_KEY, DELEGATION_CONSOLE, wt.console_clsid()).unwrap();
        r.set(STARTUP_KEY, DELEGATION_TERMINAL, wt.terminal_clsid()).unwrap();
        register(&mut r, &exe(), WT).unwrap();
        set_default(&mut r, WT).unwrap();
        // A second set keeps the original "previous" pair, not Mouser.
        set_default(&mut r, WT).unwrap();
        assert!(restore_default(&mut r).unwrap());
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_CONSOLE).unwrap(), wt.console_clsid());
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_TERMINAL).unwrap(), wt.terminal_clsid());
        assert!(is_registered(&r));
    }

    #[test]
    fn restore_removes_values_that_were_absent() {
        let mut r = MemRegistry::default();
        register(&mut r, &exe(), WT).unwrap();
        set_default(&mut r, WT).unwrap();
        assert!(restore_default(&mut r).unwrap());
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_CONSOLE), None);
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_TERMINAL), None);
    }

    #[test]
    fn unregister_leaves_someone_elses_default_alone() {
        let mut r = MemRegistry::default();
        register(&mut r, &exe(), WT).unwrap();
        set_default(&mut r, WT).unwrap();
        // The user then picked the Windows Console Host in Settings.
        r.set(STARTUP_KEY, DELEGATION_CONSOLE, CLSID_CONHOST).unwrap();
        r.set(STARTUP_KEY, DELEGATION_TERMINAL, CLSID_CONHOST).unwrap();
        unregister(&mut r).unwrap();
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_TERMINAL).unwrap(), CLSID_CONHOST);
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_CONSOLE).unwrap(), CLSID_CONHOST);
        assert!(!is_registered(&r));
        assert!(!r.key_exists(&clsid_key()));
        assert!(!r.key_exists(r"Software\Mouser"));
    }

    #[test]
    fn unregister_restores_when_mouser_is_default() {
        let mut r = MemRegistry::default();
        r.set(STARTUP_KEY, DELEGATION_CONSOLE, CLSID_DEFAULT).unwrap();
        r.set(STARTUP_KEY, DELEGATION_TERMINAL, CLSID_DEFAULT).unwrap();
        register(&mut r, &exe(), WT).unwrap();
        set_default(&mut r, WT).unwrap();
        unregister(&mut r).unwrap();
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_TERMINAL).unwrap(), CLSID_DEFAULT);
        assert_eq!(r.get(STARTUP_KEY, DELEGATION_CONSOLE).unwrap(), CLSID_DEFAULT);
        assert!(r.keys.iter().all(|k| !k.contains("mouser") && !k.contains(&MOUSER_HANDOFF_CLSID.to_ascii_lowercase())));
        // Unregistering twice is fine.
        unregister(&mut r).unwrap();
    }

    #[test]
    fn detects_windows_terminal() {
        let r = MemRegistry::default();
        let lad = PathBuf::from(r"C:\Users\m\AppData\Local");
        assert_eq!(detect_windows_terminal(&r, Some(&lad), &|_| false), None);
        let stable = lad.join("Microsoft").join("WindowsApps").join(WindowsTerminal::Stable.family());
        assert_eq!(detect_windows_terminal(&r, Some(&lad), &|p| p == stable), WT);
        let preview = lad.join("Microsoft").join("WindowsApps").join(WindowsTerminal::Preview.family());
        assert_eq!(detect_windows_terminal(&r, Some(&lad), &|p| p == preview), Some(WindowsTerminal::Preview));
        let mut r = MemRegistry::default();
        r.set(&format!(r"Software\Classes\PackagedCom\ClassIndex\{}", WindowsTerminal::Stable.console_clsid()), "", "x").unwrap();
        assert_eq!(detect_windows_terminal(&r, None, &|_| false), WT);
    }

    #[test]
    fn describes_clsids() {
        assert_eq!(describe(None), "Let Windows decide (not set)");
        assert_eq!(describe(Some(&MOUSER_HANDOFF_CLSID.to_ascii_lowercase())), "Mouser");
        assert_eq!(describe(Some(CLSID_CONHOST)), "Windows Console Host");
        assert_eq!(describe(Some(WindowsTerminal::Stable.terminal_clsid())), "Windows Terminal");
    }
}
