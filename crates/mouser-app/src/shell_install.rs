//! `mouser shell-integration --install` / `--uninstall`: a managed block in the
//! shell's startup file that loads the integration (docs/cli.md §Shell
//! integration). Edits are idempotent, keep a backup of the file and write
//! through symlinks; a load line the user wrote themselves is left alone.

use anyhow::{Context, Result, bail};
use std::path::{Path, PathBuf};

pub const BEGIN: &str = "# >>> mouser shell integration >>>";
pub const END: &str = "# <<< mouser shell integration <<<";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shell {
    Zsh,
    Bash,
    Fish,
    Pwsh,
}

impl Shell {
    pub fn parse(s: &str) -> Option<Shell> {
        Some(match s {
            "zsh" => Shell::Zsh,
            "bash" => Shell::Bash,
            "fish" => Shell::Fish,
            "pwsh" | "powershell" => Shell::Pwsh,
            _ => return None,
        })
    }

    pub fn name(self) -> &'static str {
        match self {
            Shell::Zsh => "zsh",
            Shell::Bash => "bash",
            Shell::Fish => "fish",
            Shell::Pwsh => "pwsh",
        }
    }

    /// The shell from `$SHELL` (a path, maybe `-zsh` for a login shell); on
    /// Windows with no `$SHELL`, PowerShell.
    pub fn detect(env_shell: Option<&str>, windows: bool) -> Option<Shell> {
        match env_shell.filter(|s| !s.is_empty()) {
            Some(s) => {
                let base = s.rsplit(['/', '\\']).next().unwrap_or(s).trim_start_matches('-');
                let base = base.strip_suffix(".exe").unwrap_or(base);
                Shell::parse(base)
            }
            None if windows => Some(Shell::Pwsh),
            None => None,
        }
    }

    /// The lines between the markers: load the integration when `mouser` is on PATH.
    pub fn load_lines(self) -> &'static [&'static str] {
        match self {
            Shell::Zsh => &["if command -v mouser >/dev/null 2>&1; then eval \"$(mouser shell-integration zsh)\"; fi"],
            Shell::Bash => &["if command -v mouser >/dev/null 2>&1; then eval \"$(mouser shell-integration bash)\"; fi"],
            Shell::Fish => &["if command -q mouser; mouser shell-integration fish | source; end"],
            Shell::Pwsh => {
                &["if (Get-Command mouser -ErrorAction SilentlyContinue) { mouser shell-integration pwsh | Out-String | Invoke-Expression }"]
            }
        }
    }
}

/// Where startup files live (from the environment; tests pass their own).
#[derive(Debug, Clone, Default)]
pub struct Places {
    pub home: PathBuf,
    pub zdotdir: Option<PathBuf>,
    pub xdg_config_home: Option<PathBuf>,
    /// PowerShell's `$PROFILE` (CurrentUserCurrentHost), when known.
    pub pwsh_profile: Option<PathBuf>,
    pub windows: bool,
}

impl Places {
    pub fn startup_file(&self, shell: Shell) -> PathBuf {
        let nonempty = |p: &Option<PathBuf>| p.clone().filter(|p| !p.as_os_str().is_empty());
        match shell {
            Shell::Zsh => nonempty(&self.zdotdir).unwrap_or_else(|| self.home.clone()).join(".zshrc"),
            Shell::Bash => self.home.join(".bashrc"),
            // conf.d files load before config.fish: no edit of the user's own file.
            Shell::Fish => nonempty(&self.xdg_config_home).unwrap_or_else(|| self.home.join(".config")).join("fish/conf.d/mouser.fish"),
            Shell::Pwsh => nonempty(&self.pwsh_profile).unwrap_or_else(|| {
                if self.windows {
                    self.home.join("Documents/PowerShell/Microsoft.PowerShell_profile.ps1")
                } else {
                    nonempty(&self.xdg_config_home).unwrap_or_else(|| self.home.join(".config")).join("powershell/Microsoft.PowerShell_profile.ps1")
                }
            }),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Change {
    /// The block was added (the file is created if it was missing).
    Added,
    /// An older block was replaced.
    Updated,
    /// Removed.
    Removed,
    /// Nothing to do: already installed / nothing to remove.
    Unchanged,
    /// The file loads the integration itself (this 1-based line, outside the
    /// block); left as it is.
    Unmanaged(usize),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Outcome {
    pub change: Change,
    /// The file edited (the symlink's target when it is one).
    pub file: PathBuf,
    pub backup: Option<PathBuf>,
    /// The file's text after the change (what was written, or would be on a dry run).
    pub text: String,
}

/// The managed block's lines, `nl`-terminated.
pub fn block(shell: Shell, nl: &str) -> String {
    let mut s = String::new();
    s.push_str(BEGIN);
    s.push_str(nl);
    s.push_str("# Added by `mouser shell-integration --install`; remove with --uninstall.");
    s.push_str(nl);
    for l in shell.load_lines() {
        s.push_str(l);
        s.push_str(nl);
    }
    s.push_str(END);
    s.push_str(nl);
    s
}

/// Byte range of the managed block (through the end marker's line ending), if any.
fn find_block(text: &str) -> Result<Option<(usize, usize)>> {
    let mut start = None;
    let mut off = 0;
    for line in text.split_inclusive('\n') {
        let t = line.trim_end_matches(['\n', '\r']).trim();
        if t == BEGIN {
            if start.is_some() {
                bail!("two `{BEGIN}` lines without an end marker; fix the file by hand");
            }
            start = Some(off);
        } else if t == END {
            let Some(s) = start else { bail!("`{END}` without a start marker; fix the file by hand") };
            return Ok(Some((s, off + line.len())));
        }
        off += line.len();
    }
    if start.is_some() {
        bail!("`{BEGIN}` without an end marker; fix the file by hand");
    }
    Ok(None)
}

/// A line outside the block that already loads the integration (1-based).
fn unmanaged_line(text: &str, skip: Option<(usize, usize)>) -> Option<usize> {
    let mut off = 0;
    for (i, line) in text.split_inclusive('\n').enumerate() {
        let inside = skip.is_some_and(|(a, b)| off >= a && off < b);
        off += line.len();
        let code = line.trim_start();
        if !inside && !code.starts_with('#') && code.contains("mouser shell-integration") {
            return Some(i + 1);
        }
    }
    None
}

fn newline_of(text: &str) -> &'static str {
    if text.contains("\r\n") { "\r\n" } else { "\n" }
}

/// The text with the block in it (pure; see [`install`]).
pub fn with_block(text: &str, shell: Shell) -> Result<(Change, String)> {
    let range = find_block(text)?;
    if let Some(n) = unmanaged_line(text, range) {
        return Ok((Change::Unmanaged(n), text.to_string()));
    }
    let nl = newline_of(text);
    let b = block(shell, nl);
    match range {
        Some((s, e)) if text[s..e] == b => Ok((Change::Unchanged, text.to_string())),
        Some((s, e)) => Ok((Change::Updated, format!("{}{b}{}", &text[..s], &text[e..]))),
        None => {
            let mut out = text.to_string();
            if !out.is_empty() {
                if !out.ends_with('\n') {
                    out.push_str(nl);
                }
                out.push_str(nl);
            }
            out.push_str(&b);
            Ok((Change::Added, out))
        }
    }
}

/// The text without the block (pure; see [`uninstall`]).
pub fn without_block(text: &str) -> Result<(Change, String)> {
    let Some((mut s, e)) = find_block(text)? else {
        return Ok(match unmanaged_line(text, None) {
            Some(n) => (Change::Unmanaged(n), text.to_string()),
            None => (Change::Unchanged, text.to_string()),
        });
    };
    // Drop the blank line install put before the block.
    let before = &text[..s];
    for sep in ["\r\n\r\n", "\n\n"] {
        if before.ends_with(sep) {
            s -= sep.len() / 2;
            break;
        }
    }
    Ok((Change::Removed, format!("{}{}", &text[..s], &text[e..])))
}

/// Add (or refresh) the block in `path`. `dry_run` writes nothing.
pub fn install(path: &Path, shell: Shell, dry_run: bool) -> Result<Outcome> {
    edit(path, dry_run, |t| with_block(t, shell))
}

/// Take the block out of `path`. `dry_run` writes nothing.
pub fn uninstall(path: &Path, dry_run: bool) -> Result<Outcome> {
    edit(path, dry_run, without_block)
}

fn edit(path: &Path, dry_run: bool, f: impl Fn(&str) -> Result<(Change, String)>) -> Result<Outcome> {
    // Write through a symlink (dotfile managers link rc files): edit its target.
    let file = match std::fs::symlink_metadata(path) {
        Ok(m) if m.file_type().is_symlink() => std::fs::canonicalize(path).with_context(|| format!("{} is a broken symlink", path.display()))?,
        _ => path.to_path_buf(),
    };
    let existing = match std::fs::read(&file) {
        Ok(b) => Some(String::from_utf8(b).with_context(|| format!("{} is not UTF-8 text; not editing it", file.display()))?),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => None,
        Err(e) => return Err(e).with_context(|| format!("reading {}", file.display())),
    };
    let (change, text) = f(existing.as_deref().unwrap_or(""))?;
    let writes = matches!(change, Change::Added | Change::Updated | Change::Removed);
    let mut out = Outcome { change, file: file.clone(), backup: None, text };
    if !writes || dry_run {
        return Ok(out);
    }
    if existing.is_some() {
        let backup = free_backup_path(&file);
        std::fs::copy(&file, &backup).with_context(|| format!("backing up {} to {}", file.display(), backup.display()))?;
        out.backup = Some(backup);
    } else if let Some(dir) = file.parent() {
        std::fs::create_dir_all(dir).with_context(|| format!("creating {}", dir.display()))?;
    }
    write_replacing(&file, out.text.as_bytes())?;
    Ok(out)
}

/// `FILE.mouser-bak`, or `.mouser-bak.2`, `.3`… so an older backup is kept.
fn free_backup_path(file: &Path) -> PathBuf {
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let mut n = 1;
    loop {
        let cand = file.with_file_name(if n == 1 { format!("{name}.mouser-bak") } else { format!("{name}.mouser-bak.{n}") });
        if !cand.exists() {
            return cand;
        }
        n += 1;
    }
}

/// Write via a temporary file in the same folder and rename it over, keeping
/// the old file's permissions.
fn write_replacing(file: &Path, bytes: &[u8]) -> Result<()> {
    let dir = file.parent().filter(|d| !d.as_os_str().is_empty()).unwrap_or(Path::new("."));
    let name = file.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let tmp = dir.join(format!(".{name}.mouser-tmp.{}", std::process::id()));
    let res = (|| -> Result<()> {
        std::fs::write(&tmp, bytes).with_context(|| format!("writing {}", tmp.display()))?;
        if let Ok(m) = std::fs::metadata(file) {
            std::fs::set_permissions(&tmp, m.permissions())?;
        }
        std::fs::rename(&tmp, file).with_context(|| format!("writing {}", file.display()))?;
        Ok(())
    })();
    if res.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    res
}

impl Places {
    /// From this process's environment. PowerShell's profile path comes from
    /// PowerShell itself when it is installed (Documents can be redirected).
    pub fn from_env(shell: Shell) -> Places {
        let var = |k: &str| std::env::var_os(k).map(PathBuf::from).filter(|p| !p.as_os_str().is_empty());
        let windows = cfg!(windows);
        Places {
            home: var(if windows { "USERPROFILE" } else { "HOME" }).unwrap_or_else(|| PathBuf::from(".")),
            zdotdir: var("ZDOTDIR"),
            xdg_config_home: var("XDG_CONFIG_HOME"),
            pwsh_profile: if shell == Shell::Pwsh { pwsh_profile() } else { None },
            windows,
        }
    }
}

fn pwsh_profile() -> Option<PathBuf> {
    for exe in ["pwsh", "powershell"] {
        let out = mouser_core::process::command(exe)
            .args(["-NoProfile", "-NonInteractive", "-Command", "$PROFILE.CurrentUserCurrentHost"])
            .stdin(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .output();
        if let Ok(o) = out
            && o.status.success()
        {
            let p = String::from_utf8_lossy(&o.stdout).trim().to_string();
            if !p.is_empty() {
                return Some(PathBuf::from(p));
            }
        }
    }
    None
}

/// What to do (`mouser shell-integration`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Op {
    Install,
    Uninstall,
}

/// `mouser shell-integration [SHELL] --install|--uninstall [--rc FILE] [--dry-run]`.
pub fn run(op: Op, shell: Option<&str>, rc: Option<PathBuf>, dry_run: bool, out: &mut dyn std::io::Write) -> Result<()> {
    let sh = match shell {
        Some(s) => Shell::parse(s).with_context(|| format!("no integration for {s}; one of: zsh, bash, fish, pwsh"))?,
        None => {
            let env = std::env::var("SHELL").ok();
            Shell::detect(env.as_deref(), cfg!(windows)).with_context(|| {
                format!(
                    "can't tell the shell from $SHELL ({}); name it: mouser shell-integration zsh|bash|fish|pwsh --{}",
                    env.as_deref().unwrap_or("unset"),
                    if op == Op::Install { "install" } else { "uninstall" }
                )
            })?
        }
    };
    let path = match rc {
        Some(p) => p,
        None => Places::from_env(sh).startup_file(sh),
    };
    let o = match op {
        Op::Install => install(&path, sh, dry_run)?,
        Op::Uninstall => uninstall(&path, dry_run)?,
    };
    let f = o.file.display();
    let would = if dry_run { "would be " } else { "" };
    match &o.change {
        Change::Added | Change::Updated => {
            let what = if o.change == Change::Added { "added to" } else { "updated in" };
            writeln!(out, "{} integration {would}{what} {f}:", sh.name())?;
            for l in block(sh, "\n").lines() {
                writeln!(out, "    {l}")?;
            }
        }
        Change::Removed => writeln!(out, "{} integration {would}removed from {f}", sh.name())?,
        Change::Unchanged if op == Op::Install => writeln!(out, "{} integration is already installed in {f}; nothing changed", sh.name())?,
        Change::Unchanged => writeln!(out, "no Mouser block in {f}; nothing changed")?,
        Change::Unmanaged(n) => writeln!(
            out,
            "{f} line {n} already loads `mouser shell-integration` (not a Mouser-managed block); left as it is{}",
            if op == Op::Install { "" } else { ": remove that line by hand" }
        )?,
    }
    if let Some(b) = &o.backup {
        writeln!(out, "backup: {}", b.display())?;
    }
    if dry_run {
        writeln!(out, "dry run: nothing was written")?;
    } else if o.change == Change::Added || o.change == Change::Updated {
        writeln!(out, "open a new {} to load it; undo with: mouser shell-integration {} --uninstall", sh.name(), sh.name())?;
        if sh == Shell::Bash && cfg!(target_os = "macos") {
            writeln!(out, "note: macOS Terminal starts login shells, which read ~/.bash_profile; make sure it sources ~/.bashrc")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    const ALL: [Shell; 4] = [Shell::Zsh, Shell::Bash, Shell::Fish, Shell::Pwsh];

    #[test]
    fn detects_the_shell_from_its_path() {
        assert_eq!(Shell::detect(Some("/bin/zsh"), false), Some(Shell::Zsh));
        assert_eq!(Shell::detect(Some("-bash"), false), Some(Shell::Bash));
        assert_eq!(Shell::detect(Some("/usr/local/bin/fish"), false), Some(Shell::Fish));
        assert_eq!(Shell::detect(Some(r"C:\Program Files\PowerShell\7\pwsh.exe"), true), Some(Shell::Pwsh));
        assert_eq!(Shell::detect(Some("/bin/tcsh"), false), None);
        assert_eq!(Shell::detect(None, false), None);
        assert_eq!(Shell::detect(None, true), Some(Shell::Pwsh));
    }

    #[test]
    fn startup_files_follow_the_environment() {
        let p = Places { home: "/h".into(), ..Default::default() };
        assert_eq!(p.startup_file(Shell::Zsh), PathBuf::from("/h/.zshrc"));
        assert_eq!(p.startup_file(Shell::Bash), PathBuf::from("/h/.bashrc"));
        assert_eq!(p.startup_file(Shell::Fish), PathBuf::from("/h/.config/fish/conf.d/mouser.fish"));
        assert_eq!(p.startup_file(Shell::Pwsh), PathBuf::from("/h/.config/powershell/Microsoft.PowerShell_profile.ps1"));
        let p = Places {
            home: "/h".into(),
            zdotdir: Some("/z".into()),
            xdg_config_home: Some("/x".into()),
            pwsh_profile: Some("/p.ps1".into()),
            windows: false,
        };
        assert_eq!(p.startup_file(Shell::Zsh), PathBuf::from("/z/.zshrc"));
        assert_eq!(p.startup_file(Shell::Fish), PathBuf::from("/x/fish/conf.d/mouser.fish"));
        assert_eq!(p.startup_file(Shell::Pwsh), PathBuf::from("/p.ps1"));
    }

    #[test]
    fn install_appends_once_keeps_content_and_uninstall_restores_it() {
        for sh in ALL {
            let dir = tempfile::tempdir().unwrap();
            let rc = dir.path().join("rc");
            let original = "export A=1\nalias ll='ls -l'"; // no final newline
            std::fs::write(&rc, original).unwrap();

            let o = install(&rc, sh, false).unwrap();
            assert_eq!(o.change, Change::Added, "{sh:?}");
            let text = std::fs::read_to_string(&rc).unwrap();
            assert!(text.starts_with("export A=1\nalias ll='ls -l'\n\n# >>> mouser"), "{sh:?}: {text}");
            assert!(text.contains(&format!("mouser shell-integration {}", sh.name())), "{sh:?}");
            assert_eq!(std::fs::read_to_string(o.backup.as_ref().unwrap()).unwrap(), original);

            // Reinstall: no change, no new backup.
            let again = install(&rc, sh, false).unwrap();
            assert_eq!((again.change, again.backup), (Change::Unchanged, None));
            assert_eq!(std::fs::read_to_string(&rc).unwrap(), text);

            let u = uninstall(&rc, false).unwrap();
            assert_eq!(u.change, Change::Removed);
            assert_eq!(std::fs::read_to_string(&rc).unwrap(), "export A=1\nalias ll='ls -l'\n");
            assert_eq!(uninstall(&rc, false).unwrap().change, Change::Unchanged);
        }
    }

    #[test]
    fn an_old_block_is_replaced_in_place() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        std::fs::write(&rc, format!("a\n{BEGIN}\neval \"$(old)\"\n{END}\nb\n")).unwrap();
        let o = install(&rc, Shell::Zsh, false).unwrap();
        assert_eq!(o.change, Change::Updated);
        let t = std::fs::read_to_string(&rc).unwrap();
        assert_eq!(t, format!("a\n{}b\n", block(Shell::Zsh, "\n")));
    }

    #[test]
    fn a_line_the_user_wrote_is_left_alone() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        let mine = "PS1='$ '\neval \"$(mouser shell-integration bash)\"\n";
        std::fs::write(&rc, mine).unwrap();
        assert_eq!(install(&rc, Shell::Bash, false).unwrap().change, Change::Unmanaged(2));
        assert_eq!(uninstall(&rc, false).unwrap().change, Change::Unmanaged(2));
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), mine);
        // A commented-out line doesn't count.
        std::fs::write(&rc, "# eval \"$(mouser shell-integration bash)\"\n").unwrap();
        assert_eq!(install(&rc, Shell::Bash, false).unwrap().change, Change::Added);
    }

    #[test]
    fn a_broken_block_is_refused_without_writing() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".zshrc");
        let t = format!("x\n{BEGIN}\nstuff\n");
        std::fs::write(&rc, &t).unwrap();
        assert!(install(&rc, Shell::Zsh, false).is_err());
        assert!(uninstall(&rc, false).is_err());
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), t);
        assert!(!dir.path().join(".zshrc.mouser-bak").exists());
    }

    #[test]
    fn missing_file_and_folders_are_created_without_a_backup() {
        let dir = tempfile::tempdir().unwrap();
        let p = Places { home: dir.path().into(), ..Default::default() };
        let f = p.startup_file(Shell::Fish);
        let o = install(&f, Shell::Fish, false).unwrap();
        assert_eq!((o.change, o.backup), (Change::Added, None));
        assert_eq!(std::fs::read_to_string(&f).unwrap(), block(Shell::Fish, "\n"));
    }

    #[test]
    fn paths_with_spaces_and_quotes_work() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("my \"dot\" files $HOME 'x'");
        std::fs::create_dir(&sub).unwrap();
        let rc = sub.join(".zshrc");
        std::fs::write(&rc, "x\n").unwrap();
        install(&rc, Shell::Zsh, false).unwrap();
        assert!(sub.join(".zshrc.mouser-bak").exists());
        uninstall(&rc, false).unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "x\n");
        // The second backup doesn't overwrite the first.
        assert!(sub.join(".zshrc.mouser-bak.2").exists());
    }

    #[test]
    fn dry_run_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join(".bashrc");
        std::fs::write(&rc, "x\n").unwrap();
        let o = install(&rc, Shell::Bash, true).unwrap();
        assert_eq!(o.change, Change::Added);
        assert!(o.text.contains(BEGIN));
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "x\n");
        assert_eq!(std::fs::read_dir(dir.path()).unwrap().count(), 1);
    }

    #[test]
    fn crlf_files_keep_crlf() {
        let dir = tempfile::tempdir().unwrap();
        let rc = dir.path().join("profile.ps1");
        std::fs::write(&rc, "Set-Alias g git\r\n").unwrap();
        install(&rc, Shell::Pwsh, false).unwrap();
        let t = std::fs::read_to_string(&rc).unwrap();
        assert!(!t.replace("\r\n", "").contains('\n'), "{t:?}");
        uninstall(&rc, false).unwrap();
        assert_eq!(std::fs::read_to_string(&rc).unwrap(), "Set-Alias g git\r\n");
    }

    #[cfg(unix)]
    #[test]
    fn a_symlinked_rc_stays_a_symlink_and_keeps_its_mode() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("dotfiles-zshrc");
        std::fs::write(&real, "x\n").unwrap();
        std::fs::set_permissions(&real, std::fs::Permissions::from_mode(0o640)).unwrap();
        let link = dir.path().join(".zshrc");
        std::os::unix::fs::symlink(&real, &link).unwrap();
        let o = install(&link, Shell::Zsh, false).unwrap();
        assert!(std::fs::symlink_metadata(&link).unwrap().file_type().is_symlink());
        assert_eq!(o.file, std::fs::canonicalize(&real).unwrap());
        assert!(std::fs::read_to_string(&real).unwrap().contains(BEGIN));
        assert_eq!(std::fs::metadata(&real).unwrap().permissions().mode() & 0o777, 0o640);
    }

    #[test]
    fn the_installed_line_loads_the_integration_in_a_real_shell() {
        // bash and zsh, when present: the block evals `mouser shell-integration`
        // from a fake `mouser` on PATH (never the user's own startup files).
        for (sh, bin) in [(Shell::Bash, "bash"), (Shell::Zsh, "zsh")] {
            if std::process::Command::new(bin).arg("-c").arg("true").output().is_err() {
                continue;
            }
            let dir = tempfile::tempdir().unwrap();
            let fake = dir.path().join("mouser");
            std::fs::write(&fake, "#!/bin/sh\necho 'MOUSER_LOADED=yes'\n").unwrap();
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&fake, std::fs::Permissions::from_mode(0o755)).unwrap();
            }
            let rc = dir.path().join("rc");
            install(&rc, sh, false).unwrap();
            let out = std::process::Command::new(bin)
                .arg("-c")
                .arg(format!(". '{}'; echo \"$MOUSER_LOADED\"", rc.display()))
                .env("PATH", format!("{}:/usr/bin:/bin", dir.path().display()))
                .env("HOME", dir.path())
                .output()
                .unwrap();
            assert_eq!(String::from_utf8_lossy(&out.stdout).trim(), "yes", "{bin}: {out:?}");
        }
    }
}
