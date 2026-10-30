//! SSH hosts from the SSH config (source of truth), with live reload and the
//! add/edit/delete/migrate panels.

use mouser_core::appearance::{self, EditKey, Outcome};
use mouser_core::connections::SshConnection;
use mouser_core::ssh_config::{self, SshForm, SshHostEntry};
use mouser_render::{UiViewport, ui};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

pub enum SshPanel {
    Form(Box<SshForm>),
    ConfirmDelete(String),
    ConfirmMigrate(usize),
}

pub struct SshUi {
    pub root: PathBuf,
    hosts: Arc<Mutex<Vec<SshHostEntry>>>,
    pub panel: Option<SshPanel>,
    /// SSH entries still in ~/.mouser/connections (offered for migration).
    pub legacy: usize,
}

impl SshUi {
    /// Load hosts and start polling the config (and Includes) for changes.
    pub fn start(root: PathBuf) -> Self {
        let hosts = Arc::new(Mutex::new(ssh_config::load_hosts(&root)));
        {
            let hosts = hosts.clone();
            let root = root.clone();
            std::thread::spawn(move || {
                let mut last = ssh_config::fingerprint(&root);
                loop {
                    std::thread::sleep(Duration::from_millis(1500));
                    let now = ssh_config::fingerprint(&root);
                    if now != last {
                        last = now;
                        *hosts.lock().unwrap() = ssh_config::load_hosts(&root);
                        crate::wake();
                    }
                }
            });
        }
        let legacy = ssh_config::mouser_ssh_connections().len();
        Self { root, hosts, panel: None, legacy }
    }

    fn reload(&self) {
        *self.hosts.lock().unwrap() = ssh_config::load_hosts(&self.root);
    }

    pub fn names(&self) -> (Vec<String>, Vec<String>) {
        let hosts = self.hosts.lock().unwrap();
        let all = hosts.iter().map(|h| h.alias.clone()).collect();
        let ro = hosts.iter().filter(|h| h.read_only.is_some()).map(|h| h.alias.clone()).collect();
        (all, ro)
    }

    fn entry(&self, alias: &str) -> Option<SshHostEntry> {
        self.hosts.lock().unwrap().iter().find(|h| h.alias == alias).cloned()
    }

    /// The command that launches `alias` (ssh reads the config itself).
    /// `ssh [-F cfg] alias`, run directly as a tab's process.
    pub fn connect_argv(&self, alias: &str) -> Vec<String> {
        let mut v = vec!["ssh".to_string()];
        if std::env::var_os("MOUSER_SSH_CONFIG").is_some() {
            v.push("-F".into());
            v.push(self.root.display().to_string());
        }
        v.push(alias.to_string());
        v
    }

    #[allow(dead_code)]
    pub fn connect_command(&self, alias: &str) -> String {
        if std::env::var_os("MOUSER_SSH_CONFIG").is_some() {
            format!("ssh -F '{}' {}\r", self.root.display(), alias)
        } else {
            format!("ssh {alias}\r")
        }
    }

    pub fn open_add(&mut self) {
        self.panel = Some(SshPanel::Form(Box::new(SshForm::new(None, &SshConnection::default(), None))));
    }

    pub fn open_edit(&mut self, alias: &str) {
        if let Some(h) = self.entry(alias) {
            self.panel = Some(SshPanel::Form(Box::new(SshForm::new(Some(alias), &h.conn, h.read_only))));
        }
    }

    pub fn open_delete(&mut self, alias: &str) {
        self.panel = Some(SshPanel::ConfirmDelete(alias.to_string()));
    }

    pub fn open_migrate(&mut self) {
        self.legacy = ssh_config::mouser_ssh_connections().len();
        if self.legacy > 0 {
            self.panel = Some(SshPanel::ConfirmMigrate(self.legacy));
        }
    }

    /// Returns a status message, if any.
    pub fn key(&mut self, bytes: &[u8]) -> Option<String> {
        let key = appearance::parse_key(bytes)?;
        let outcome = match &mut self.panel {
            Some(SshPanel::Form(f)) => f.handle(key),
            Some(_) => match key {
                EditKey::Enter | EditKey::Save => Outcome::Save,
                EditKey::Cancel => Outcome::Close,
                _ => Outcome::Continue,
            },
            None => return None,
        };
        self.apply(outcome)
    }

    pub fn action(&mut self, act: &ui::UiAction) -> Option<String> {
        let outcome = match (&mut self.panel, act) {
            (Some(SshPanel::Form(f)), ui::UiAction::FocusField(i)) => {
                f.focus = (*i).min(f.fields.len() - 1);
                Outcome::Continue
            }
            (Some(SshPanel::Form(f)), ui::UiAction::SavePanel) => {
                if f.read_only.is_some() {
                    Outcome::Close
                } else {
                    f.handle(EditKey::Save)
                }
            }
            (Some(_), ui::UiAction::SavePanel) => Outcome::Save,
            (Some(_), ui::UiAction::ClosePanel) => Outcome::Close,
            _ => Outcome::Continue,
        };
        self.apply(outcome)
    }

    fn apply(&mut self, outcome: Outcome) -> Option<String> {
        match outcome {
            Outcome::Close => {
                self.panel = None;
                None
            }
            Outcome::Save => {
                let msg = match self.panel.take()? {
                    SshPanel::Form(f) => match f.save(&self.root) {
                        Ok(alias) => format!("Saved Host {alias} in {}", self.root.display()),
                        Err(e) => {
                            let mut f = f;
                            f.error = Some(e);
                            self.panel = Some(SshPanel::Form(f));
                            return None;
                        }
                    },
                    SshPanel::ConfirmDelete(alias) => match ssh_config::delete_host(&self.root, &alias) {
                        Ok(()) => format!("Deleted Host {alias}"),
                        Err(e) => format!("Could not delete {alias}: {e}"),
                    },
                    SshPanel::ConfirmMigrate(_) => {
                        let m = ssh_config::migrate_mouser_ssh(&self.root);
                        self.legacy = ssh_config::mouser_ssh_connections().len();
                        if m.skipped.is_empty() {
                            format!("Moved {} SSH connection(s) to {}", m.moved.len(), self.root.display())
                        } else {
                            let skipped: Vec<String> = m.skipped.iter().map(|(n, e)| format!("{n} ({e})")).collect();
                            format!("Moved {}; kept {}", m.moved.len(), skipped.join(", "))
                        }
                    }
                };
                self.reload();
                Some(msg)
            }
            _ => None,
        }
    }

    pub fn panel_view(&self, vp: UiViewport) -> Option<ui::Panel> {
        Some(match self.panel.as_ref()? {
            SshPanel::Form(f) => {
                let fields: Vec<ui::EditorFieldView> = f
                    .fields
                    .iter()
                    .map(|x| ui::EditorFieldView { label: x.label.clone(), value: x.value.clone(), valid: x.is_valid(), swatch: None })
                    .collect();
                let title = match &f.original {
                    Some(a) if f.read_only.is_some() => format!("SSH {a} (read-only)"),
                    Some(a) => format!("Edit SSH {a}"),
                    None => "Add SSH host".to_string(),
                };
                let note = match &f.read_only {
                    Some(r) => r.clone(),
                    None => format!("Saved to {}. Other options are kept.", self.root.display()),
                };
                ui::form_panel(
                    vp.width,
                    vp.height,
                    vp.cell_w,
                    vp.cell_h,
                    &title,
                    &fields,
                    f.focus,
                    Some(&note),
                    f.error.as_deref(),
                    f.read_only.is_none(),
                )
            }
            SshPanel::ConfirmDelete(a) => {
                ui::confirm_panel(vp.width, vp.height, vp.cell_w, vp.cell_h, &format!("Delete Host {a} from {}?", self.root.display()), "Delete")
            }
            SshPanel::ConfirmMigrate(n) => ui::confirm_panel(
                vp.width,
                vp.height,
                vp.cell_w,
                vp.cell_h,
                &format!("Move {n} SSH connection(s) from ~/.mouser into {}?", self.root.display()),
                "Move",
            ),
        })
    }
}
