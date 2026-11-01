//! The permission broker (docs/plugins.md §Permissions): allow / ask / never per
//! plugin and capability, persisted in `$MOUSER_HOME/plugins/permissions.toml`.
//!
//! Asking is the app's job: it draws the approval popup outside the terminal
//! grid and feeds the answer back through [`Broker::answer`].

use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::fs;
use std::path::PathBuf;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Decision {
    Allow,
    #[default]
    Ask,
    Never,
}

impl Decision {
    pub fn parse(s: &str) -> Option<Self> {
        match s {
            "allow" | "always" => Some(Decision::Allow),
            "ask" => Some(Decision::Ask),
            "never" | "deny" => Some(Decision::Never),
            _ => None,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Decision::Allow => "allow",
            Decision::Ask => "ask",
            Decision::Never => "never",
        }
    }
}

/// What the user picked in the approval popup.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Answer {
    Once,
    Always,
    Deny,
    Never,
}

impl Answer {
    pub fn granted(self) -> bool {
        matches!(self, Answer::Once | Answer::Always)
    }
}

/// Where a request comes from; decides whether answers may be remembered.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Scope {
    /// An installed plugin: answers can be saved.
    Installed,
    /// A Fuzzball plugin from a program in a local tab: saved under `fuzzball:<name>`.
    FuzzballLocal,
    /// A Fuzzball plugin over ssh (or another remote). With the host's identity
    /// (`user@host:port` + host key fingerprint) an "Allow always" is saved for
    /// that host, plugin and capabilities only; without one it always asks.
    FuzzballRemote { host: Option<String> },
}

impl Scope {
    /// Whether "Allow always" can be saved.
    pub fn persistable(&self) -> bool {
        !matches!(self, Scope::FuzzballRemote { host: None })
    }

    /// Whether "Never" is offered (remote: only once / always / deny).
    pub fn offers_never(&self) -> bool {
        !matches!(self, Scope::FuzzballRemote { .. })
    }
}

/// The key permissions are saved under.
pub fn key(plugin: &str, scope: &Scope) -> String {
    match scope {
        Scope::Installed => plugin.to_string(),
        Scope::FuzzballRemote { host: Some(h) } => format!("ssh:{h}/{plugin}"),
        _ => format!("fuzzball:{plugin}"),
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize, PartialEq, Eq)]
pub struct Store {
    #[serde(default)]
    pub plugins: BTreeMap<String, BTreeMap<String, Decision>>,
}

pub fn path() -> PathBuf {
    crate::plugins::dir().join("permissions.toml")
}

impl Store {
    pub fn load() -> Self {
        fs::read_to_string(path()).ok().and_then(|s| toml::from_str(&s).ok()).unwrap_or_default()
    }

    pub fn save(&self) -> Result<()> {
        let d = crate::plugins::dir();
        fs::create_dir_all(&d)?;
        let tmp = d.join(".permissions.toml.tmp");
        fs::write(&tmp, toml::to_string_pretty(self)?)?;
        fs::rename(tmp, path())?;
        Ok(())
    }

    pub fn get(&self, key: &str, cap: &str) -> Decision {
        self.plugins.get(key).and_then(|m| m.get(cap)).copied().unwrap_or_default()
    }

    pub fn set(&mut self, key: &str, cap: &str, d: Decision) {
        let m = self.plugins.entry(key.to_string()).or_default();
        if d == Decision::Ask {
            m.remove(cap);
        } else {
            m.insert(cap.to_string(), d);
        }
        if m.is_empty() {
            self.plugins.remove(key);
        }
    }
}

/// The broker's verdict before asking.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Check {
    Allow,
    Deny,
    /// Show the popup for these capabilities.
    Ask(Vec<String>),
}

pub struct Broker;

impl Broker {
    /// Check every capability: any `never` denies, all `allow` allows, else ask
    /// for the ones not yet allowed. Remote Fuzzball always asks for all.
    pub fn check(plugin: &str, caps: &[String], scope: &Scope) -> Check {
        let st = Store::load();
        let k = key(plugin, scope);
        // A local `never` for this Fuzzball name also holds over ssh.
        let local = key(plugin, &Scope::FuzzballLocal);
        let remote = matches!(scope, Scope::FuzzballRemote { .. });
        if caps.iter().any(|c| st.get(&k, c) == Decision::Never || (remote && st.get(&local, c) == Decision::Never)) {
            return Check::Deny;
        }
        if !scope.persistable() {
            return Check::Ask(caps.to_vec());
        }
        let ask: Vec<String> = caps.iter().filter(|c| st.get(&k, c) != Decision::Allow).cloned().collect();
        if ask.is_empty() { Check::Allow } else { Check::Ask(ask) }
    }

    /// Record an answer (Once/Deny change nothing on disk).
    pub fn answer(plugin: &str, caps: &[String], scope: &Scope, a: Answer) -> Result<()> {
        let d = match a {
            Answer::Always => Decision::Allow,
            Answer::Never => Decision::Never,
            _ => return Ok(()),
        };
        if !scope.persistable() || (d == Decision::Never && !scope.offers_never()) {
            return Ok(());
        }
        let mut st = Store::load();
        let k = key(plugin, scope);
        for c in caps {
            st.set(&k, c, d);
        }
        st.save()
    }
}

/// Drop everything saved under `key` (a plugin name, `fuzzball:<name>` or
/// `ssh:<host>/<name>`). Returns whether there was anything.
pub fn forget(key: &str) -> Result<bool> {
    let mut st = Store::load();
    let had = st.plugins.remove(key).is_some();
    if had {
        st.save()?;
    }
    Ok(had)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    #[serial_test::serial]
    fn allow_ask_never_persisted_and_remote_never_remembered() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let caps = vec!["run".to_string(), "notify".to_string()];
        let i = Scope::Installed;
        assert_eq!(Broker::check("p", &caps, &i), Check::Ask(caps.clone()));
        Broker::answer("p", &caps[..1], &i, Answer::Always).unwrap();
        assert_eq!(Broker::check("p", &caps, &i), Check::Ask(vec!["notify".into()]));
        Broker::answer("p", &caps[1..], &i, Answer::Once).unwrap();
        assert_eq!(Broker::check("p", &caps, &i), Check::Ask(vec!["notify".into()]));
        Broker::answer("p", &caps[1..], &i, Answer::Always).unwrap();
        assert_eq!(Broker::check("p", &caps, &i), Check::Allow);
        assert_eq!(Store::load().get("p", "run"), Decision::Allow);
        // Remote Fuzzball without a host identity: asks every time, nothing saved.
        let r = Scope::FuzzballRemote { host: None };
        Broker::answer("p", &caps, &r, Answer::Always).unwrap();
        assert_eq!(Broker::check("p", &caps, &r), Check::Ask(caps.clone()));
        assert!(!Store::load().plugins.contains_key("fuzzball:p"));
        // Local Fuzzball is saved apart from the installed plugin of that name.
        let l = Scope::FuzzballLocal;
        Broker::answer("p", &caps, &l, Answer::Never).unwrap();
        assert_eq!(Broker::check("p", &caps, &l), Check::Deny);
        assert_eq!(Broker::check("p", &caps, &i), Check::Allow);
        // A saved never also denies a remote one.
        assert_eq!(Broker::check("p", &caps, &r), Check::Deny);
        assert!(forget("p").unwrap());
        assert!(!forget("p").unwrap());
        assert_eq!(Broker::check("p", &caps, &i), Check::Ask(caps.clone()));
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }

    #[test]
    #[serial_test::serial]
    fn ssh_allow_always_is_per_host_plugin_and_capabilities() {
        let t = tempfile::tempdir().unwrap();
        unsafe { std::env::set_var("MOUSER_HOME", t.path()) };
        let h1 = Scope::FuzzballRemote { host: Some("m@10.0.0.1:22 SHA256:aaa".into()) };
        let caps = vec!["notify".to_string(), "screen.read".to_string()];
        assert_eq!(Broker::check("p", &caps, &h1), Check::Ask(caps.clone()));
        Broker::answer("p", &caps, &h1, Answer::Always).unwrap();
        assert_eq!(Broker::check("p", &caps, &h1), Check::Allow);
        assert_eq!(Broker::check("p", &caps[..1], &h1), Check::Allow, "fewer capabilities are covered");
        // More capabilities: asks for the new one only.
        let more = vec!["notify".to_string(), "shell.type".to_string()];
        assert_eq!(Broker::check("p", &more, &h1), Check::Ask(vec!["shell.type".into()]));
        // Another host, the same host with another key, another plugin, or a local tab: ask.
        for s in [
            Scope::FuzzballRemote { host: Some("m@10.0.0.2:22 SHA256:aaa".into()) },
            Scope::FuzzballRemote { host: Some("m@10.0.0.1:22 SHA256:bbb".into()) },
            Scope::FuzzballRemote { host: None },
            Scope::FuzzballLocal,
        ] {
            assert_eq!(Broker::check("p", &caps, &s), Check::Ask(caps.clone()), "{s:?}");
        }
        assert_eq!(Broker::check("q", &caps, &h1), Check::Ask(caps.clone()));
        // Never is not saved for remote scopes (not offered there).
        Broker::answer("q", &caps, &h1, Answer::Never).unwrap();
        assert_eq!(Broker::check("q", &caps, &h1), Check::Ask(caps.clone()));
        // Saved under a revocable key.
        let k = key("p", &h1);
        assert_eq!(k, "ssh:m@10.0.0.1:22 SHA256:aaa/p");
        assert!(Store::load().plugins.contains_key(&k));
        assert!(forget(&k).unwrap());
        assert_eq!(Broker::check("p", &caps, &h1), Check::Ask(caps.clone()));
        // A local never also blocks the remote one.
        Broker::answer("p", &caps, &Scope::FuzzballLocal, Answer::Never).unwrap();
        assert_eq!(Broker::check("p", &caps, &h1), Check::Deny);
        unsafe { std::env::remove_var("MOUSER_HOME") };
    }
}
