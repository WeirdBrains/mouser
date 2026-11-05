//! Man pages generated from the clap definitions (clap_mangen), plus
//! hand-written FILES / ENVIRONMENT / EXIT STATUS / SECURITY / EXAMPLES
//! sections on mouser(1). `mouser man --out man` writes them; docs/cli.md.
use super::Cli;
use clap::CommandFactory;
use std::io::Write;
use std::path::Path;

/// Every environment variable Mouser reads, with what it does (also in
/// docs/cli.md). Test-only hooks (`MOUSER_TEST_*`, `MOUSER_FLAT_*`) are left out.
pub const ENVIRONMENT: &[(&str, &str)] = &[
    (
        "MOUSER_ENGINE_DIRS",
        "Folders of engines shipped with Mouser (a path list), replacing engines/ beside the executable; empty: none (docs/engines.md).",
    ),
    (
        "MOUSER_OFFLINE_BUNDLE",
        "Folder with the offline bundle's plugins.lock archives, instead of plugins/ beside the executable (docs/downloads.md).",
    ),
    ("MOUSER_CARGO", "cargo used for `install crate:NAME` (default cargo; needs cargo-binstall)."),
    ("MOUSER_HOME", "Root directory instead of ~/.mouser (config, themes, plugins, models, logs)."),
    ("MOUSER_ENGINE", "Rendering engine for this run (mouser, skia or a custom engine's name), ahead of the saved priority order."),
    ("MOUSER_RENDERER", "cpu: draw with the CPU renderer instead of a GPU engine."),
    ("MOUSER_PROC_FORK", "0: keep the sessions and the window in one process (no session server / UI split; Unix)."),
    ("MOUSER_UI", "headless: run the session server without a window (tests)."),
    ("MOUSER_SYSTEM_DARK", "1 or 0: whether the desktop prefers dark, overriding detection."),
    ("MOUSER_OPEN_SIDEBAR", "Open the sidebar for this run, whatever was saved."),
    ("MOUSER_MODELS_DIR", "Where local model weights live (before [models.local] dir and <home>/models)."),
    ("MOUSER_MODEL_URL_OVERRIDE", "Download URL for `mouser model download` instead of the pinned one."),
    ("MOUSER_GROKBOT_HOME", "Grok Bot desktop home used by `mouser model add grok --desktop` (default: the app's own)."),
    ("MOUSER_GROK_POLL_MS", "Poll interval in ms for the grok Cursor-agents and desktop providers."),
    ("MOUSER_SECRET_STORE", "file: keep API keys and tokens in a 0600 file under <home> instead of the OS keychain."),
    ("MOUSER_SSH_BIN", "ssh binary for remote commander and the connection manager (default ssh)."),
    ("MOUSER_SSH_CONFIG", "SSH config Mouser reads and edits (default ~/.ssh/config)."),
    ("MOUSER_REMOTE_BACKEND", "rsync, scp or cat: force how remote files are fetched."),
    ("MOUSER_PLAYER_LOG", "Write the media player's log to this file."),
    ("MOUSER_LLM_TIMINGS", "Print llama.cpp timings for the built-in local model."),
    (
        "MOUSER_PLUGIN_DIRS",
        "extra plugin folders (a path list), replacing the offline bundle and system folders; empty: only $MOUSER_HOME/plugins (docs/plugins.md).",
    ),
    ("MOUSER_MODE_WIRE", "json: mode plugins (mouser-mode-api) speak JSON instead of MessagePack on stdio, for debugging (docs/mode-protocol.md)."),
    ("RUST_LOG", "Log filter (env_logger), e.g. debug or mouser=trace."),
];

const FILES: &str = r#".SH FILES
.TP
.I ~/.mouser/config.toml
Font, colours, scrollback, compression, engines, models, MCP and plugin settings (TOML).
.TP
.I ~/.mouser/preferences.toml
Choices made in the app: theme, engine order, sidebar, last mode, mode panel size ([window] dock_height, dock_full).
.TP
.I ~/.mouser/modes.toml
Mode keys: the focus toggle ([focus] key, default ctrl+alt+k), each mode's keys ([<mode>.keys]) and the host's own keys while a mode has focus ([host.keys]: F fullscreen panel, ] taller, [ shorter, . context menu).
.TP
.I ~/.mouser/themes/
Built-in themes, written on first run or copied on edit.
.TP
.I ~/.mouser/plugins/
Installed plugins (modes, engines, model and tool providers, commands).
.TP
.I ~/.mouser/models/
Local model weights (see MOUSER_MODELS_DIR).
.PP
The root moves with \fBMOUSER_HOME\fR.
"#;

const REST: &str = r#".SH EXIT STATUS
0 on success; 1 on an error (the message goes to stderr); 2 for a usage error.
.SH SECURITY
API keys and MCP tokens are kept in the OS keychain (or a 0600 file with MOUSER_SECRET_STORE=file) and are never printed or logged.
Pass keys on stdin (\fB\-\-key\-stdin\fR), not as arguments. The MCP server listens on 127.0.0.1 unless \fBbind\fR is set, and then only over HTTPS with a token.
.SH EXAMPLES
.nf
mouser \-e btop
mouser \-\-working\-directory ~/src \-\- nvim .
mouser show photo.png
mouser md README.md
printf %s "$XAI_KEY" | mouser model add grok \-\-key\-stdin
mouser mcp cert status
mouser default\-terminal set
mouser help models
.fi
.SH SEE ALSO
\fBmouser help\fR (the built-in guides), https://mouser.weirdbrains.com
"#;

fn roff_escape(s: &str) -> String {
    let e = s.replace('\\', "\\e").replace('-', "\\-");
    if e.starts_with('.') || e.starts_with('\'') { format!("\\&{e}") } else { e }
}

fn visible(c: &clap::Command) -> impl Iterator<Item = &clap::Command> {
    c.get_subcommands().filter(|s| !s.is_hide_set())
}

fn render(cmd: &clap::Command, path: &[String], out: &Path, count: &mut usize) -> std::io::Result<()> {
    let name = path.join("-");
    // clap wants &'static names without its `string` feature; a one-off command.
    let cmd = cmd.clone().name(&*name.clone().leak()).bin_name(path.join(" ")).version(env!("CARGO_PKG_VERSION"));
    let man = clap_mangen::Man::new(cmd.clone());
    let mut buf: Vec<u8> = Vec::new();
    man.render_title(&mut buf)?;
    man.render_name_section(&mut buf)?;
    man.render_synopsis_section(&mut buf)?;
    man.render_description_section(&mut buf)?;
    man.render_options_section(&mut buf)?;
    if visible(&cmd).next().is_some() {
        man.render_subcommands_section(&mut buf)?;
    }
    if path.len() == 1 {
        buf.write_all(FILES.as_bytes())?;
        writeln!(buf, ".SH ENVIRONMENT")?;
        for (k, v) in ENVIRONMENT {
            writeln!(buf, ".TP\n.B {}\n{}", roff_escape(k), roff_escape(v))?;
        }
        buf.write_all(REST.as_bytes())?;
    } else {
        writeln!(buf, ".SH SEE ALSO\n\\fBmouser\\fR(1)")?;
    }
    man.render_version_section(&mut buf)?;
    std::fs::write(out.join(format!("{name}.1")), buf)?;
    *count += 1;
    for sub in visible(&cmd) {
        let mut p = path.to_vec();
        p.push(sub.get_name().to_string());
        render(sub, &p, out, count)?;
    }
    Ok(())
}

/// Write mouser.1 and one page per visible subcommand. Returns how many.
pub fn generate(out: &Path) -> std::io::Result<usize> {
    std::fs::create_dir_all(out)?;
    let mut cmd = Cli::command();
    cmd.build();
    let mut n = 0;
    render(&cmd, &["mouser".to_string()], out, &mut n)?;
    Ok(n)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pages(c: &clap::Command, path: Vec<String>, out: &mut Vec<String>) {
        out.push(path.join("-"));
        for s in visible(c) {
            let mut p = path.clone();
            p.push(s.get_name().to_string());
            pages(s, p, out);
        }
    }

    #[test]
    fn every_command_flag_and_env_var_is_documented() {
        let d = tempfile::tempdir().unwrap();
        let n = generate(d.path()).unwrap();
        let mut cmd = Cli::command();
        cmd.build();
        let mut want = vec![];
        pages(&cmd, vec!["mouser".into()], &mut want);
        assert_eq!(n, want.len());
        let committed = Path::new(env!("CARGO_MANIFEST_DIR")).join("../../man");
        for p in &want {
            let page = std::fs::read_to_string(d.path().join(format!("{p}.1"))).unwrap();
            // Windows checkouts may turn LF into CRLF.
            let b = std::fs::read_to_string(committed.join(format!("{p}.1"))).unwrap_or_default().replace("\r\n", "\n");
            assert_eq!(page, b, "man/{p}.1 is stale; run `cargo run -p mouser-app -- man --out man`");
        }
        let top = std::fs::read_to_string(d.path().join("mouser.1")).unwrap();
        for s in [".SH FILES", ".SH ENVIRONMENT", ".SH EXIT STATUS", ".SH SECURITY", ".SH EXAMPLES", "working\\-directory", "hold"] {
            assert!(top.contains(s), "{s}");
        }
        // Every variable the man page lists is in docs/cli.md too.
        let cli = std::fs::read_to_string(Path::new(env!("CARGO_MANIFEST_DIR")).join("../../docs/cli.md")).unwrap();
        for (k, _) in ENVIRONMENT {
            assert!(top.contains(&roff_escape(k)) && cli.contains(k), "{k}");
        }
    }

    /// Every MOUSER_* variable the code reads is listed (tests' own hooks aside).
    #[test]
    fn no_env_var_is_missing() {
        let root = Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        let mut found = std::collections::BTreeSet::new();
        fn walk(d: &Path, found: &mut std::collections::BTreeSet<String>) {
            for e in std::fs::read_dir(d).unwrap().flatten() {
                let p = e.path();
                if p.is_dir() {
                    if !matches!(p.file_name().and_then(|n| n.to_str()), Some("target" | "tests" | ".git")) {
                        walk(&p, found);
                    }
                } else if p.extension().is_some_and(|x| x == "rs") {
                    let s = std::fs::read_to_string(&p).unwrap_or_default();
                    for part in s.split("env::var").skip(1) {
                        let part = part.trim_start_matches("_os").trim_start_matches('(');
                        if let Some(rest) = part.strip_prefix("\"MOUSER_") {
                            let name: String = rest.chars().take_while(|c| c.is_ascii_uppercase() || *c == '_').collect();
                            found.insert(format!("MOUSER_{name}"));
                        }
                    }
                }
            }
        }
        walk(&root.join("crates"), &mut found);
        for v in found.iter().filter(|v| !v.starts_with("MOUSER_TEST_") && !v.starts_with("MOUSER_FLAT_")) {
            assert!(ENVIRONMENT.iter().any(|(k, _)| k == v), "{v} is read but not documented in man.rs ENVIRONMENT");
        }
    }
}
