mod appearance_ui;
mod approval_ui;
mod chat_host;
mod mcp_tools;
mod remote_list;
mod select;
mod ssh_ui;
mod tabs;
mod term_view;

use anyhow::Result;
use clap::Parser;
use mouser_core::compression::{CompressionAlgo, CompressionConfig};
use mouser_core::connections::{
    Connection, ConnectionKind, SshConnection, TunnelConnection, TunnelSpec, TunnelType, WaypipeConnection, X11Connection, delete_connection,
    list_connections, load_connection, save_connection,
};
use mouser_core::scrollback::ScrollbackConfig;
use mouser_core::types::Size;
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[allow(dead_code)]
enum Mode {
    Shell,
    Command,
    Config,
}

struct AppState {
    mode: Mode,
    command_buf: String,
    sidebar_open: bool,
    sidebar_collapsed: bool,
    sidebar_width: Option<i32>,
    /// MOUSER_OPEN_SIDEBAR forced it open for this run: don't save that state
    /// unless the user changes the sidebar.
    sidebar_forced: bool,
    look: appearance_ui::Appearance,
    ssh: ssh_ui::SshUi,
    pending_delete: Option<String>,
    pending_edit: Option<Connection>,
    /// Mode plugins (docs/modes.md); commander is on at start.
    modes: mouser_render::modes::Host,
    /// The mode's images from the last layout and the grid top they are relative to.
    mode_images: (Vec<mouser_render::modes::ModeImage>, i32),
    /// A lone Esc waiting to see whether `:` follows (generation).
    esc_pending: Option<u64>,
    esc_gen: u64,
    remote: remote_list::RemoteLister,
    /// When an Esc last closed a mode window or viewer.
    esc_closed_at: Option<std::time::Instant>,
    /// Plugin approval popups (always on top; own the keyboard while open).
    approvals: approval_ui::Approvals,
}
use mouser_render::ui;
use mouser_render::{RenderHooks, Renderer, run as run_renderer};
use serde::Deserialize;
use std::fs;
use std::io::Write;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "mouser")]
#[command(version, about = "Mouser terminal emulator", long_about = None)]
#[command(after_help = "Examples:\n  mouser -e btop\n  mouser --working-directory ~/src -- nvim .\n  mouser --hold -e make test")]
struct Cli {
    /// Run a command instead of the login shell; everything after it is its arguments.
    #[arg(short = 'e', long = "command", value_name = "CMD", num_args = 1.., allow_hyphen_values = true, conflicts_with = "trailing")]
    exec: Vec<String>,
    /// Command to run, after `--` (same as -e).
    #[arg(last = true, value_name = "CMD")]
    trailing: Vec<String>,
    /// Start in this directory instead of $HOME.
    #[arg(long = "working-directory", visible_alias = "dir", value_name = "DIR")]
    working_directory: Option<PathBuf>,
    /// Window title (until the program sets its own).
    #[arg(long, short = 'T')]
    title: Option<String>,
    /// Wayland app_id / X11 WM_CLASS (default "Mouser").
    #[arg(long = "class", visible_alias = "app-id", value_name = "CLASS")]
    class: Option<String>,
    /// Keep the window open after the command exits.
    #[arg(long)]
    hold: bool,
    /// Path to TOML config
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override config dir (tests/manual runs)
    #[arg(long = "config-dir")]
    config_dir: Option<PathBuf>,
    #[command(subcommand)]
    command: Option<Command>,
}

#[derive(clap::Subcommand, Debug)]
enum Command {
    /// Show images in this terminal (Fuzzball graphics, or Kitty graphics as a fallback; works in tmux with allow-passthrough).
    ///
    /// Formats: PNG, JPEG, GIF (first frame), WebP, BMP. Video and audio are not
    /// supported. Works in Mouser, other Fuzzball terminals and Kitty-graphics
    /// terminals; inside tmux it uses Kitty Unicode placeholders.
    Show {
        /// Image files; `-` reads stdin. Shown one after another.
        #[arg(required = true)]
        files: Vec<String>,
        /// Width in cells (height follows the aspect ratio).
        #[arg(long)]
        width: Option<u32>,
        /// Height in cells (width follows the aspect ratio).
        #[arg(long)]
        height: Option<u32>,
        /// Fit the terminal width (default).
        #[arg(long, conflicts_with = "original")]
        fit: bool,
        /// Natural size: one image pixel per screen pixel (the terminal still fits it to the screen).
        #[arg(long)]
        original: bool,
        /// Image id to use (default: derived from the process id).
        #[arg(long)]
        id: Option<u32>,
    },
    /// Manage GPU rendering engines (mouser, skia, custom); the same as
    /// `mouser plugin … --kind engine` (engines are plugins); see docs/engines.md.
    Engine {
        #[command(subcommand)]
        action: PluginCommand,
    },
    /// Mouser's MCP server (tabs, screen, shell, files for MCP clients); see docs/mcp.md.
    Mcp {
        #[command(subcommand)]
        action: McpCommand,
    },
    /// Model providers (OpenAI-compatible, Anthropic, Google, local); see docs/models.md.
    Model {
        #[command(subcommand)]
        action: ModelCommand,
    },
    /// Manage plugins (mode, model-provider, tool-provider, command; engines too); see docs/plugins.md.
    Plugin {
        /// Only this kind: mode, engine, model-provider, tool-provider, command.
        /// With `engine` this is `mouser engine …`.
        #[arg(long, global = true)]
        kind: Option<String>,
        #[command(subcommand)]
        action: PluginCommand,
    },
}

#[derive(clap::Subcommand, Debug)]
enum McpCommand {
    /// Speak MCP on stdin/stdout, bridged to the running Mouser (for clients that start servers).
    Serve {
        #[arg(long, required = true)]
        stdio: bool,
    },
    /// Show whether it's on, the endpoints, the certificate and where the token is.
    Status,
    /// Turn the server on (loopback HTTP; `--https` adds TLS with a self-signed or your cert).
    Enable {
        #[arg(long)]
        https: bool,
        #[arg(long)]
        port: Option<u16>,
        #[arg(long)]
        https_port: Option<u16>,
    },
    /// Turn it off.
    Disable,
    /// Print (`--show`) or replace (`--rotate`) the bearer token.
    Token {
        #[arg(long, conflicts_with = "rotate")]
        show: bool,
        #[arg(long)]
        rotate: bool,
    },
    /// MCP servers Mouser connects to (`[[mcp.servers]]`; tools in the sidebar).
    Servers {
        #[command(subcommand)]
        action: McpServersCommand,
    },
}

#[derive(clap::Subcommand, Debug)]
enum ModelCommand {
    /// List providers (built-in, configured, plugins) and their models; * marks the default.
    List,
    /// Add or update: `add openai --key-stdin`, or `add NAME --api openai|anthropic|google --url URL [--model M]…`.
    Add {
        name: String,
        #[arg(long)]
        api: Option<String>,
        #[arg(long)]
        url: Option<String>,
        /// Read the API key from stdin (stored in the OS keychain, never in preferences).
        #[arg(long)]
        key_stdin: bool,
        /// Models it offers, first is its default (repeatable).
        #[arg(long = "model")]
        models: Vec<String>,
        /// PEM certificate/CA to trust for an https URL.
        #[arg(long)]
        ca: Option<String>,
    },
    /// Remove a configured provider and its key.
    Remove { name: String },
    /// Stream one reply from PROVIDER[/MODEL] (default: the default model).
    Test {
        spec: Option<String>,
        #[arg(long, default_value = "Say hello in five words.")]
        prompt: String,
        /// Offer a demo tool (get_time) and require a call.
        #[arg(long)]
        tool: bool,
    },
    /// Set the default model: PROVIDER[/MODEL].
    Default { spec: String },
    /// The built-in local model (gemma-local): status, download, delete, url, dir.
    Local {
        #[command(subcommand)]
        action: Option<LocalCommand>,
    },
}

#[derive(clap::Subcommand, Debug)]
enum LocalCommand {
    /// Engine, server, models dir and which weights are downloaded.
    Status,
    /// Download weights (gemma-4-e2b 2.84 GB, gemma-4-e4b 4.59 GB); shows the size and asks first.
    Download {
        id: Option<String>,
        /// Skip the question (you already agreed to the size shown by `status`).
        #[arg(long)]
        yes: bool,
    },
    /// Delete downloaded weights.
    Delete { id: String },
    /// Use an OpenAI/Anthropic-compatible server instead of the built-in engine.
    Url {
        #[arg(required_unless_present = "clear")]
        url: Option<String>,
        #[arg(long)]
        api: Option<String>,
        /// Model name the server expects.
        #[arg(long)]
        model: Option<String>,
        #[arg(long)]
        key_stdin: bool,
        #[arg(long, conflicts_with_all = ["url", "api", "model", "key_stdin"])]
        clear: bool,
    },
    /// Where model files go (`MOUSER_MODELS_DIR` overrides).
    Dir {
        #[arg(required_unless_present = "clear")]
        path: Option<String>,
        #[arg(long, conflicts_with = "path")]
        clear: bool,
    },
}

#[derive(clap::Subcommand, Debug)]
enum McpServersCommand {
    /// Add or update: `add NAME --url URL [--token-stdin] [--ca PEM]` or `add NAME -- COMMAND ARGS…`.
    Add {
        name: String,
        #[arg(long)]
        url: Option<String>,
        /// Read a bearer token from stdin (stored in the OS keychain, never in preferences).
        #[arg(long)]
        token_stdin: bool,
        /// PEM certificate/CA to trust for an https URL.
        #[arg(long)]
        ca: Option<String>,
        /// Environment for a stdio server, K=V (V may be keychain:NAME); repeatable.
        #[arg(long)]
        env: Vec<String>,
        #[arg(last = true)]
        command: Vec<String>,
    },
    /// List configured servers and MCP plugins.
    List,
    /// Remove a server, its token and its permissions.
    Remove { name: String },
    /// Connect and list its tools; `--call TOOL [--args JSON]` calls one (asks first).
    Test {
        name: String,
        #[arg(long)]
        call: Option<String>,
        #[arg(long, requires = "call")]
        args: Option<String>,
    },
}

#[derive(clap::Subcommand, Debug)]
enum PluginCommand {
    /// List plugins (or engines) by priority, with what each provides and its state.
    List,
    /// Install from a directory with plugin.toml (or an engine's engine.toml).
    Install { path: String },
    /// Remove an installed plugin or engine (and a plugin's saved permissions).
    Uninstall { name: String },
    /// Turn a plugin or engine on (built-ins included).
    Enable { name: String },
    /// Turn a plugin or engine off without removing it (not the mouser engine).
    Disable { name: String },
    /// Engines: put this engine first in the priority order (and unskip it).
    Use { name: String },
    /// Set the priority order (the rest keep their order; for engines mouser stays last).
    Priority {
        #[arg(required = true)]
        names: Vec<String>,
    },
    /// Move one: first, last, up, down or a 1-based position.
    Move { name: String, to: String },
    /// Engines: stop skipping one that crashed Mouser (and clear its failure) without selecting it.
    #[command(visible_alias = "restore")]
    Unskip {
        #[arg(required_unless_present = "all", conflicts_with = "all")]
        name: Option<String>,
        /// Every engine.
        #[arg(long)]
        all: bool,
    },
    /// List every saved permission (no NAME), show or set one (`cap=allow|ask|never ...`, `*=never` for all), or `NAME revoke`.
    Permissions { name: Option<String>, set: Vec<String> },
    /// Start an MCP plugin, do the handshake and list its tools.
    Probe { name: String },
}

/// `mouser plugin …` / `mouser engine …` as a plugin_cli action and the kind it
/// is limited to (`mouser engine X` is `mouser plugin X --kind engine`).
fn plugin_action(c: &Command) -> Option<(mouser_app::plugin_cli::Action, Option<String>, &'static str)> {
    use mouser_app::plugin_cli::Action;
    let (action, kind, who) = match c {
        Command::Engine { action } => (action, Some("engine".to_string()), "mouser engine"),
        Command::Plugin { action, kind } => (action, kind.clone(), "mouser plugin"),
        _ => return None,
    };
    let a = match action {
        PluginCommand::List => Action::List,
        PluginCommand::Install { path } => Action::Install(path.clone()),
        PluginCommand::Uninstall { name } => Action::Uninstall(name.clone()),
        PluginCommand::Enable { name } => Action::Enable(name.clone()),
        PluginCommand::Disable { name } => Action::Disable(name.clone()),
        PluginCommand::Use { name } => Action::Use(name.clone()),
        PluginCommand::Unskip { name, .. } => Action::Unskip(name.clone()),
        PluginCommand::Priority { names } => Action::Priority(names.clone()),
        PluginCommand::Move { name, to } => Action::Move(name.clone(), to.clone()),
        PluginCommand::Permissions { name: None, .. } => Action::PermissionsList,
        PluginCommand::Permissions { name: Some(name), set } if set.len() == 1 && set[0] == "revoke" => Action::Revoke(name.clone()),
        PluginCommand::Permissions { name: Some(name), set } => Action::Permissions(name.clone(), set.clone()),
        PluginCommand::Probe { name } => Action::Probe(name.clone()),
    };
    Some((a, kind, who))
}

#[derive(Debug, Deserialize, Clone)]
struct UiConfig {
    font_family: Option<String>,
    font_size: Option<f32>,
    theme: Option<String>,
}

/// `[selection]` in config.toml.
#[derive(Debug, Deserialize, Clone, Default)]
struct SelectionConfig {
    /// Characters besides letters and digits that a double-click keeps in a word.
    word_chars: Option<String>,
}

#[derive(Debug, Deserialize, Clone)]
#[allow(dead_code)]
struct ColorsConfig {
    foreground: Option<String>,
    background: Option<String>,
    cursor: Option<String>,
    palette: Option<Vec<String>>,
}

#[derive(Debug, Deserialize, Clone)]
struct ScrollbackToml {
    max_scrollback_lines: Option<usize>,
    chunk_lines: Option<usize>,
    idle_compress_ms: Option<u64>,
    prefetch_chunks: Option<usize>,
    search_case_sensitive: Option<bool>,
}

#[derive(Debug, Deserialize, Clone)]
struct CompressionToml {
    algorithm: Option<String>,
    level: Option<i32>,
}

#[derive(Debug, Deserialize, Clone)]
struct AppConfig {
    ui: Option<UiConfig>,
    #[serde(default)]
    selection: Option<SelectionConfig>,
    colors: Option<ColorsConfig>,
    scrollback: Option<ScrollbackToml>,
    compression: Option<CompressionToml>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui: Some(UiConfig { font_family: None, font_size: Some(16.0), theme: Some("dark".into()) }),
            selection: None,
            colors: Some(ColorsConfig { foreground: None, background: None, cursor: None, palette: None }),
            scrollback: Some(ScrollbackToml {
                max_scrollback_lines: Some(200_000),
                chunk_lines: Some(1024),
                idle_compress_ms: Some(15_000),
                prefetch_chunks: Some(1),
                search_case_sensitive: Some(false),
            }),
            compression: Some(CompressionToml { algorithm: Some("zstd".into()), level: Some(3) }),
        }
    }
}

fn load_config(cli: &Cli) -> Result<AppConfig> {
    if let Some(path) = &cli.config {
        let s = fs::read_to_string(path)?;
        let cfg: AppConfig = toml::from_str(&s)?;
        return Ok(merge_with_defaults(cfg));
    }
    // Default config directory: explicit CLI override or MOUSER_HOME (via core storage resolver)
    let base = if let Some(dir) = &cli.config_dir { dir.clone() } else { mouser_core::storage::resolve_home() };
    let path = base.join("config.toml");
    if path.exists() {
        let s = fs::read_to_string(path)?;
        let cfg: AppConfig = toml::from_str(&s)?;
        Ok(merge_with_defaults(cfg))
    } else {
        Ok(AppConfig::default())
    }
}

fn merge_with_defaults(mut cfg: AppConfig) -> AppConfig {
    let def = AppConfig::default();
    let mut out = def.clone();
    if let Some(ui) = cfg.ui.take() {
        out.ui = Some(UiConfig {
            font_family: ui.font_family.or(out.ui.as_ref().and_then(|u| u.font_family.clone())),
            font_size: ui.font_size.or(out.ui.as_ref().and_then(|u| u.font_size)),
            theme: ui.theme.or(out.ui.as_ref().and_then(|u| u.theme.clone())),
        });
    }
    if let Some(colors) = cfg.colors.take() {
        out.colors = Some(colors);
    }
    if let Some(sb) = cfg.scrollback.take() {
        out.scrollback = Some(ScrollbackToml {
            max_scrollback_lines: sb.max_scrollback_lines.or(out.scrollback.as_ref().and_then(|s| s.max_scrollback_lines)),
            chunk_lines: sb.chunk_lines.or(out.scrollback.as_ref().and_then(|s| s.chunk_lines)),
            idle_compress_ms: sb.idle_compress_ms.or(out.scrollback.as_ref().and_then(|s| s.idle_compress_ms)),
            prefetch_chunks: sb.prefetch_chunks.or(out.scrollback.as_ref().and_then(|s| s.prefetch_chunks)),
            search_case_sensitive: sb.search_case_sensitive.or(out.scrollback.as_ref().and_then(|s| s.search_case_sensitive)),
        });
    }
    if let Some(cmp) = cfg.compression.take() {
        out.compression = Some(cmp);
    }
    out
}

fn make_scrollback_config(app: &AppConfig) -> ScrollbackConfig {
    let sb = app.scrollback.as_ref().unwrap();
    let cmp = app.compression.as_ref().unwrap();
    ScrollbackConfig {
        max_scrollback_lines: sb.max_scrollback_lines.unwrap_or(200_000),
        chunk_lines: sb.chunk_lines.unwrap_or(1024),
        idle_compress_ms: sb.idle_compress_ms.unwrap_or(15_000),
        prefetch_chunks: sb.prefetch_chunks.unwrap_or(1),
        compression: CompressionConfig {
            algorithm: match cmp.algorithm.as_deref() {
                Some("lz4") => CompressionAlgo::Lz4,
                Some("none") => CompressionAlgo::None,
                _ => CompressionAlgo::Zstd,
            },
            level: cmp.level.unwrap_or(3),
        },
    }
}

/// PTY writer; `None` until the shell is spawned at the window's real size.
struct SharedWriter(std::sync::Arc<Mutex<Option<Box<dyn Write + Send>>>>);

impl Clone for SharedWriter {
    fn clone(&self) -> Self {
        SharedWriter(self.0.clone())
    }
}

impl Write for SharedWriter {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self.0.lock().unwrap().as_mut() {
            Some(w) => w.write(buf),
            None => Ok(buf.len()),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self.0.lock().unwrap().as_mut() {
            Some(w) => w.flush(),
            None => Ok(()),
        }
    }
}

/// What the first tab runs, from `-e` / `--` / `--working-directory` / `--title` / `--hold`.
struct FirstTab {
    name: String,
    launch: tabs::Launch,
    cwd: Option<PathBuf>,
}

fn first_tab(cli: &Cli) -> FirstTab {
    let argv = if cli.exec.is_empty() { cli.trailing.clone() } else { cli.exec.clone() };
    let cwd = cli.working_directory.clone().filter(|d| {
        let ok = d.is_dir();
        if !ok {
            eprintln!("mouser: working directory {} does not exist; using $HOME", d.display());
        }
        ok
    });
    let (default_name, launch) = if argv.is_empty() {
        ("shell".to_string(), tabs::Launch::Shell)
    } else {
        let prog = std::path::Path::new(&argv[0]).file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_else(|| argv[0].clone());
        (prog, tabs::Launch::Exec { argv, hold: cli.hold })
    };
    FirstTab { name: cli.title.clone().unwrap_or(default_name), launch, cwd }
}

fn main() -> Result<()> {
    env_logger::init();
    #[cfg(feature = "local-llm")]
    mouser_llm::register();
    // Isolated custom engine process (docs/engines.md §7); not a user command.
    let args: Vec<String> = std::env::args().collect();
    if args.get(1).map(String::as_str) == Some("__engine-host") {
        let (Some(lib), Some(name)) = (args.get(2), args.get(3)) else { std::process::exit(2) };
        let r = mouser_render::engine_host::serve(std::path::Path::new(lib), name, std::io::stdin().lock(), std::io::stdout().lock());
        if let Err(e) = r {
            eprintln!("mouser engine host: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    // UI process of a split window (docs/engines.md §8); not a user command.
    #[cfg(unix)]
    if args.get(1).map(String::as_str) == Some("__ui") {
        if let Err(e) = mouser_render::split::ui_main() {
            eprintln!("mouser UI: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if cfg!(unix)
        && let Ok(exe) = std::env::current_exe()
    {
        mouser_render::engine_host::set_host_command(exe.clone(), vec!["__engine-host".into()]);
        // /proc/self/exe: the very binary running now, even after an upgrade replaced the file.
        #[cfg(unix)]
        {
            let me = if cfg!(target_os = "linux") { std::path::PathBuf::from("/proc/self/exe") } else { exe };
            mouser_render::split::set_ui_command(me, vec!["__ui".into()]);
        }
    }
    let cli = Cli::parse();
    if let Some(Command::Show { files, width, height, fit: _, original, id }) = &cli.command {
        let o = mouser_app::show::Options { width: *width, height: *height, original: *original, id: *id };
        if let Err(e) = mouser_app::show::run(files, &o) {
            eprintln!("mouser show: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some((a, kind, who)) = cli.command.as_ref().and_then(plugin_action) {
        if let Err(e) = mouser_app::plugin_cli::run_kind(&a, kind.as_deref(), &mut std::io::stdout()) {
            eprintln!("{who}: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(Command::Mcp { action }) = &cli.command {
        use mouser_app::mcp_cli::Action;
        let a = match action {
            McpCommand::Serve { .. } => Action::ServeStdio,
            McpCommand::Status => Action::Status,
            McpCommand::Enable { https, port, https_port } => Action::Enable { https: *https, port: *port, https_port: *https_port },
            McpCommand::Disable => Action::Disable,
            McpCommand::Servers { action: McpServersCommand::List } => Action::ServersList,
            McpCommand::Servers { action: McpServersCommand::Remove { name } } => Action::ServersRemove(name.clone()),
            McpCommand::Servers { action: McpServersCommand::Test { name, call, args } } => {
                Action::ServersTest { name: name.clone(), call: call.clone(), args: args.clone() }
            }
            McpCommand::Servers { action: McpServersCommand::Add { name, url, token_stdin, ca, command, env } } => {
                let token = if *token_stdin {
                    let mut t = String::new();
                    std::io::stdin().read_line(&mut t)?;
                    if t.trim().is_empty() {
                        eprintln!("mouser mcp servers add: no token on stdin");
                        std::process::exit(2);
                    }
                    Some(t)
                } else {
                    None
                };
                Action::ServersAdd { name: name.clone(), command: command.clone(), env: env.clone(), url: url.clone(), token, ca: ca.clone() }
            }
            McpCommand::Token { rotate: true, .. } => Action::TokenRotate,
            McpCommand::Token { show: true, .. } => Action::TokenShow,
            McpCommand::Token { .. } => {
                eprintln!("mouser mcp token: use --show or --rotate");
                std::process::exit(2);
            }
        };
        if let Err(e) = mouser_app::mcp_cli::run(&a, &mut std::io::stdout()) {
            eprintln!("mouser mcp: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    if let Some(Command::Model { action }) = &cli.command {
        use mouser_app::model_cli::Action;
        let a = match action {
            ModelCommand::List => Action::List,
            ModelCommand::Local { action } => {
                use mouser_app::model_cli::LocalAction as L;
                Action::Local(match action {
                    None | Some(LocalCommand::Status) => L::Status,
                    Some(LocalCommand::Download { id, yes }) => L::Download { id: id.clone(), yes: *yes },
                    Some(LocalCommand::Delete { id }) => L::Delete(id.clone()),
                    Some(LocalCommand::Dir { path, .. }) => L::Dir(path.clone()),
                    Some(LocalCommand::Url { url, api, model, key_stdin, .. }) => {
                        let key = if *key_stdin {
                            let mut t = String::new();
                            std::io::stdin().read_line(&mut t)?;
                            Some(t)
                        } else {
                            None
                        };
                        L::Url { url: url.clone(), api: api.clone(), model: model.clone(), key }
                    }
                })
            }
            ModelCommand::Remove { name } => Action::Remove(name.clone()),
            ModelCommand::Default { spec } => Action::Default(spec.clone()),
            ModelCommand::Test { spec, prompt, tool } => Action::Test { spec: spec.clone(), prompt: prompt.clone(), tool: *tool },
            ModelCommand::Add { name, api, url, key_stdin, models, ca } => {
                let key = if *key_stdin {
                    let mut t = String::new();
                    std::io::stdin().read_line(&mut t)?;
                    if t.trim().is_empty() {
                        eprintln!("mouser model add: no key on stdin");
                        std::process::exit(2);
                    }
                    Some(t)
                } else {
                    None
                };
                Action::Add { name: name.clone(), api: api.clone(), url: url.clone(), key, models: models.clone(), ca: ca.clone() }
            }
        };
        if let Err(e) = mouser_app::model_cli::run(&a, &mut std::io::stdout()) {
            eprintln!("mouser model: {e:#}");
            std::process::exit(1);
        }
        return Ok(());
    }
    let app_cfg = load_config(&cli)?;
    select::set_word_chars(app_cfg.selection.as_ref().and_then(|s| s.word_chars.clone()));
    // Ensure built-in themes exist on disk
    let _ = mouser_core::themes::write_builtins_if_absent();
    let sb_cfg = make_scrollback_config(&app_cfg);

    // The first tab's shell is spawned on the first resize, once the real grid
    // size is known, so its first prompt is drawn for the right width.
    let size = Size { cols: 100, rows: 30 };
    let tabs = Arc::new(Mutex::new(tabs::Tabs::default()));
    let first = first_tab(&cli);
    tabs.lock().unwrap().open(tabs::Session::new_in(first.name, first.launch, first.cwd, size, sb_cfg.clone()), wake);
    let parser = ActiveParser { tabs: tabs.clone(), dummy: tabs.lock().unwrap().list[0].parser.clone() };
    let shared_writer = tabs::ActiveWriter(tabs.clone());

    let prefs0 = mouser_core::storage::load_preferences();
    // In-app choices (preferences.toml) win over config.toml.
    let config_font_size = app_cfg.ui.as_ref().and_then(|u| u.font_size).unwrap_or(16.0);
    let font_size0 = prefs0.font_size.unwrap_or(config_font_size);
    let font_family0 = prefs0.font_family.clone().or(app_cfg.ui.as_ref().and_then(|u| u.font_family.clone()));
    // App UI state
    let app_state = Arc::new(Mutex::new(AppState {
        mode: Mode::Shell,
        command_buf: String::new(),
        sidebar_open: prefs0.sidebar_open.unwrap_or(false),
        sidebar_collapsed: prefs0.sidebar_collapsed.unwrap_or(false),
        sidebar_width: prefs0.sidebar_width,
        sidebar_forced: false,
        ssh: ssh_ui::SshUi::start(mouser_core::ssh_config::ssh_config_path()),
        look: appearance_ui::Appearance::new(font_size0, font_family0.clone(), config_font_size),
        pending_delete: None,
        pending_edit: None,
        // Filled from the plugin registry below, once the chat host can be made.
        modes: mouser_render::modes::Host::new(vec![]),
        mode_images: (Vec::new(), 0),
        esc_pending: None,
        esc_gen: 0,
        remote: Default::default(),
        esc_closed_at: None,
        approvals: Default::default(),
    }));

    // Hooks for renderer
    let fetch = {
        let parser = parser.clone();
        let app_state = app_state.clone();
        Arc::new(move |scrolled_back: usize, rows: usize| -> (Vec<String>, Vec<Vec<mouser_render::CellPaint>>, usize) {
            let theme = app_state.lock().unwrap().look.theme_colors();
            let session = parser.session();
            let pa = parser.get();
            let mut p = pa.lock().unwrap();
            term_view::sync_theme(&mut p, &theme);
            let (mut out, mut paint, back) = view_rows(&p, scrolled_back, rows, theme.foreground);
            if let Some(s) = session {
                select::decorate(&p, &s.selection.lock().unwrap(), back, &mut out, &mut paint, &theme);
            }
            (out, paint, back)
        })
    };
    let get_cursor = {
        let parser = parser.clone();
        Arc::new(move || {
            let pa = parser.get();
            let p = pa.lock().unwrap();
            p.screen.cursor_visible.then_some((p.screen.cursor.x, p.screen.cursor.y))
        })
    };
    let get_images = {
        let parser = parser.clone();
        let app_state = app_state.clone();
        let scaler = Mutex::new(mouser_render::ImageScaler::default());
        Arc::new(move |scrolled_back: usize, rows: usize, cell: (u32, u32)| -> Vec<mouser_render::ImageDraw> {
            let pa = parser.get();
            let mut p = pa.lock().unwrap();
            p.cell_px = cell;
            let top = if p.screen.in_alt_screen() { p.lines_scrolled } else { p.lines_scrolled - scrolled_back as i64 };
            let alt = p.screen.in_alt_screen();
            let live = p.lines_scrolled - top;
            let screen = &p.screen;
            let row = |i: usize| {
                let y = i as i64 - live;
                (y >= 0 && (y as usize) < screen.size.rows).then(|| screen.row(y as usize))
            };
            let mut kitty = p.kitty_gfx.gfx.draws(alt, top, rows, cell);
            kitty.extend(p.kitty_gfx.placeholder_draws(rows, &row, cell));
            let fuzz = p.fuzzball.gfx.draws(alt, top, rows, cell);
            let mut v = mouser_render::layered_image_draws(&[(&p.kitty_gfx.gfx, kitty), (&p.fuzzball.gfx, fuzz)], &mut scaler.lock().unwrap());
            drop(p);
            let (imgs, top) = app_state.lock().unwrap().mode_images.clone();
            v.extend(imgs.iter().map(|m| mode_image_draw(m, top)));
            v
        })
    };
    let on_key = {
        let writer = shared_writer.clone();
        let tabs_k = tabs.clone();
        let app_state = app_state.clone();
        let parser = parser.clone();
        Arc::new(move |bytes: Vec<u8>| {
            let mut state = app_state.lock().unwrap();
            if state.approvals.open() {
                // The popup owns the keyboard: nothing reaches the tab or the mode.
                if let Some((req, a)) = state.approvals.key(&bytes) {
                    resolve_approval(&mut state, req, a, &app_state);
                }
                return;
            }
            match state.mode {
                Mode::Shell => {
                    if state.ssh.panel.is_some() {
                        if let Some(m) = state.ssh.key(&bytes) {
                            state.look.say(m);
                        }
                    } else if state.look.panel.is_some() {
                        state.look.key(&bytes);
                    } else if bytes == b"\x1b" && (state.pending_edit.is_some() || state.pending_delete.is_some()) {
                        state.pending_edit = None;
                        state.pending_delete = None;
                    } else if mode_key(&mut state, &bytes, &parser, &writer, &app_state) {
                        // Taken by the mode system (esc :, the mode's keys).
                    } else if bytes == b"\x1b[21~" {
                        // F10: Mouser command line
                        state.mode = Mode::Command;
                        state.command_buf.clear();
                    } else if bytes == b"\x1b\x1b" {
                        // Alt+Esc toggles the connections sidebar; plain Esc goes to the shell.
                        state.sidebar_open = !state.sidebar_open;
                        persist_sidebar(&mut state);
                    } else if let Some(t) = exited_tab(&tabs_k) {
                        // "[process exited N] press any key to close"
                        tabs_k.lock().unwrap().close(t);
                    } else {
                        let modes = parser.get().lock().unwrap().modes;
                        let bytes = mouser_core::input::apply_cursor_mode(bytes, &modes);
                        let bytes = if modes.newline && bytes == b"\r" { b"\r\n".to_vec() } else { bytes };
                        let mut w = writer.clone();
                        let _ = w.write_all(&bytes);
                    }
                }
                Mode::Command => {
                    if bytes == b"\x1b" {
                        state.mode = Mode::Shell;
                        state.command_buf.clear();
                    } else if bytes == b"\r" {
                        let cmd = std::mem::take(&mut state.command_buf);
                        state.mode = Mode::Shell;
                        drop(state);
                        handle_command(&cmd, &writer, &app_state);
                    } else if bytes == b"\x7f" || bytes == b"\x08" {
                        state.command_buf.pop();
                    } else if let Ok(s) = std::str::from_utf8(&bytes)
                        && !s.starts_with('\x1b')
                    {
                        state.command_buf.push_str(s);
                    }
                }
                Mode::Config => {
                    state.mode = Mode::Shell;
                }
            }
        })
    };

    let get_colors = {
        let (app_state, parser) = (app_state.clone(), parser.clone());
        Arc::new(move || {
            let mut c = app_state.lock().unwrap().look.colors();
            // DECSCNM reverse video swaps the window colours too.
            if parser.get().lock().unwrap().modes.reverse_video {
                std::mem::swap(&mut c.bg, &mut c.fg);
            }
            c
        })
    };
    let get_font = {
        let app_state = app_state.clone();
        Arc::new(move || app_state.lock().unwrap().look.font())
    };
    let on_resize = {
        let tabs = tabs.clone();
        Arc::new(move |cols: u16, rows: u16, pw: u16, ph: u16| tabs.lock().unwrap().resize(cols, rows, pw, ph, wake))
    };
    let get_ui_model = {
        let (app_state, tabs) = (app_state.clone(), tabs.clone());
        Arc::new(move |vp: mouser_render::UiViewport| -> Option<ui::UiLayout> {
            let (session, tab_h) = {
                let t = tabs.lock().unwrap();
                (t.active(), if t.bar().titles.len() > 1 { ui::tab_bar_height(vp.cell_h) } else { 0 })
            };
            let alt = session.as_ref().is_some_and(|s| s.parser.lock().unwrap().screen.in_alt_screen());
            let place = session.as_ref().and_then(|s| s.place());
            let sessions = tabs.lock().unwrap().list.clone();
            let mut st = app_state.lock().unwrap();
            plugin_hellos(&mut st, &sessions);
            let layout = ui_layout(&mut st, vp);
            attach_mode_view(&mut st, layout, vp, tab_h, alt, place)
        })
    };
    fn ui_layout(st: &mut AppState, vp: mouser_render::UiViewport) -> Option<ui::UiLayout> {
        {
            // The approval popup wins over every other panel.
            let panel = st.approvals.panel_view(vp);
            let panel = panel.or_else(|| st.ssh.panel_view(vp));
            let panel = panel.or_else(|| st.look.panel_view(vp));
            if !st.sidebar_open && st.pending_delete.is_none() && st.pending_edit.is_none() {
                return panel.map(|p| ui::UiLayout { panel: Some(p), ..ui::without_sidebar(vp.width, vp.height) });
            }
            let mut groups: std::collections::BTreeMap<ui::Category, Vec<String>> = Default::default();
            // SSH hosts come from the SSH config; the rest from ~/.mouser.
            let (ssh_names, ssh_read_only) = st.ssh.names();
            groups.insert(ui::Category::Ssh, ssh_names);
            if let Ok(list) = list_connections() {
                for c in list.into_iter().filter(|c| !matches!(c.kind, ConnectionKind::Ssh(_))) {
                    groups.entry(category_of(&c.kind)).or_default().push(c.name);
                }
            }
            let center = |w: i32, h: i32| ui::Rect { x: (vp.width - w) / 2, y: (vp.height - h) / 2, w, h };
            let modal = if let Some(name) = &st.pending_delete {
                Some(ui::ModalState {
                    kind: ui::ModalKind::ConfirmDelete { category: ui::Category::Ssh, name: name.clone() },
                    advanced_open: false,
                    rect: center(vp.cell_w * 40, vp.cell_h * 8),
                })
            } else {
                st.pending_edit.as_ref().map(|conn| ui::ModalState {
                    kind: ui::ModalKind::Edit { category: category_of(&conn.kind), name: conn.name.clone() },
                    advanced_open: false,
                    rect: center(vp.cell_w * 48, vp.cell_h * 12),
                })
            };
            let spec = ui::SidebarSpec {
                width_px: vp.width,
                height_px: vp.height,
                cell_w: vp.cell_w,
                cell_h: vp.cell_h,
                requested_width: st.sidebar_width,
                collapsed: st.sidebar_collapsed && modal.is_none(),
            };
            let mut layout = ui::build_sidebar(spec, &groups, modal);
            ui::mark_read_only(&mut layout, &ui::Category::Ssh, &ssh_read_only);
            if st.ssh.legacy > 0 {
                ui::add_notice(&mut layout, &ui::Category::Ssh, &format!("Move {} saved to ~/.ssh/config…", st.ssh.legacy), vp.cell_h);
            }
            ui::add_mcp(&mut layout, &mcp_rows(), vp.cell_h);
            ui::add_engines(&mut layout, &engine_rows(), vp.cell_h);
            layout.panel = panel;
            Some(layout)
        }
    }
    let on_ui_action = {
        let app_state = app_state.clone();
        let (tabs_ui, sb_cfg, writer_ui) = (tabs.clone(), sb_cfg.clone(), shared_writer.clone());
        Arc::new(move |act: ui::UiAction| match act.clone() {
            ui::UiAction::Approve(i) => {
                let mut st = app_state.lock().unwrap();
                if let Some((req, a)) = st.approvals.action(i) {
                    resolve_approval(&mut st, req, a, &app_state);
                }
            }
            // While a popup is open, clicks elsewhere do nothing (only its buttons answer).
            _ if app_state.lock().unwrap().approvals.open() => {}
            ui::UiAction::OpenAdd(ui::Category::Ssh) => app_state.lock().unwrap().ssh.open_add(),
            ui::UiAction::OpenEdit { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_edit(&name),
            ui::UiAction::OpenDeleteConfirm { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_delete(&name),
            ui::UiAction::Notice => app_state.lock().unwrap().ssh.open_migrate(),
            ui::UiAction::UseEngine(name) => use_engine(&name),
            ui::UiAction::McpServer(id) => mcp_server_click(&id, &app_state),
            ui::UiAction::McpTool(id, tool) => mcp_tool_click(&id, &tool, &app_state),
            ui::UiAction::ReorderEngines(order) => reorder_engines(&order),
            ui::UiAction::Connect { category, name } => {
                let argv = {
                    let st = app_state.lock().unwrap();
                    connect_argv(&st.ssh, &category, &name)
                };
                match argv {
                    Some(argv) => {
                        let s = tabs::Session::new(name.clone(), tabs::Launch::Command(argv), Size { cols: 100, rows: 30 }, sb_cfg.clone());
                        tabs_ui.lock().unwrap().open(s, wake);
                    }
                    None => app_state.lock().unwrap().look.say(format!("No connection named {name}")),
                }
            }
            ui::UiAction::NewTab => {
                let s = tabs::Session::new("shell", tabs::Launch::Shell, Size { cols: 100, rows: 30 }, sb_cfg.clone());
                tabs_ui.lock().unwrap().open(s, wake);
            }
            ui::UiAction::SelectTab(i) => tabs_ui.lock().unwrap().select(i),
            ui::UiAction::CycleTab(d) => tabs_ui.lock().unwrap().cycle(d),
            ui::UiAction::CloseTab(i) => tabs_ui.lock().unwrap().close(i),
            ui::UiAction::CloseActiveTab => {
                let mut t = tabs_ui.lock().unwrap();
                let i = t.active;
                t.close(i);
            }
            ui::UiAction::OpenAdd(cat) => {
                let mut st = app_state.lock().unwrap();
                let name = "new".to_string();
                let conn = match cat {
                    ui::Category::Ssh => Connection { name: name.clone(), kind: ConnectionKind::Ssh(SshConnection::default()) },
                    ui::Category::Waypipe => Connection { name: name.clone(), kind: ConnectionKind::Waypipe(WaypipeConnection::default()) },
                    ui::Category::Tunnels => Connection { name: name.clone(), kind: ConnectionKind::Tunnel(TunnelConnection::default()) },
                    ui::Category::X11 => Connection { name: name.clone(), kind: ConnectionKind::X11(X11Connection::default()) },
                };
                st.pending_edit = Some(conn);
            }
            ui::UiAction::OpenEdit { category: _, name } => {
                if let Ok(Some(conn)) = load_connection(&name) {
                    app_state.lock().unwrap().pending_edit = Some(conn);
                }
            }
            ui::UiAction::OpenDeleteConfirm { category: _, name } => {
                app_state.lock().unwrap().pending_delete = Some(name);
            }
            ui::UiAction::ConfirmDelete => {
                if let Some(name) = app_state.lock().unwrap().pending_delete.take() {
                    let _ = delete_connection(&name);
                }
            }
            ui::UiAction::CancelModal => {
                let mut st = app_state.lock().unwrap();
                st.pending_delete = None;
                st.pending_edit = None;
            }
            ui::UiAction::ToggleCollapse => {
                let mut st = app_state.lock().unwrap();
                st.sidebar_collapsed = !st.sidebar_collapsed;
                persist_sidebar(&mut st);
            }
            ui::UiAction::ResizeSidebar(w) => {
                let mut st = app_state.lock().unwrap();
                st.sidebar_width = Some(w);
                st.sidebar_collapsed = false;
            }
            ui::UiAction::ResizeSidebarDone => persist_sidebar(&mut app_state.lock().unwrap()),
            ui::UiAction::OpenAppearance
            | ui::UiAction::PickTheme(_)
            | ui::UiAction::SetMode(_)
            | ui::UiAction::EditTheme
            | ui::UiAction::FocusField(_)
            | ui::UiAction::PickFont(_)
            | ui::UiAction::ScrollPanel(_)
            | ui::UiAction::SavePanel
            | ui::UiAction::ClosePanel
            | ui::UiAction::FontSize(_) => {
                let mut st = app_state.lock().unwrap();
                let own_panel = st.ssh.panel.is_some() || st.look.panel.is_some();
                if !own_panel && matches!(act, ui::UiAction::ClosePanel) && (st.modes.close_window() || st.modes.escape()) {
                    // Closed the mode's window.
                } else if let (false, ui::UiAction::ScrollPanel(n)) = (own_panel, &act) {
                    let e = st.modes.action("scroll", *n);
                    apply_mode_effect(&mut st, e, &writer_ui);
                } else if st.ssh.panel.is_some() && !matches!(act, ui::UiAction::FontSize(_)) {
                    if let Some(m) = st.ssh.action(&act) {
                        st.look.say(m);
                    }
                } else {
                    st.look.action(&act);
                }
            }
            ui::UiAction::None => {
                let mut st = app_state.lock().unwrap();
                if st.ssh.panel.is_none() {
                    st.look.action(&act);
                }
            }
            ui::UiAction::ToggleAdvanced => {}
            ui::UiAction::Mode { action, arg } => {
                let mut st = app_state.lock().unwrap();
                let e = st.modes.action(&action, arg);
                apply_mode_effect(&mut st, e, &writer_ui);
            }
            ui::UiAction::ShowKeymap => app_state.lock().unwrap().modes.show_keys(),
            ui::UiAction::ShowModes => app_state.lock().unwrap().modes.show_modes(),
            ui::UiAction::SwitchMode(c) => {
                let mut st = app_state.lock().unwrap();
                st.modes.close_window();
                let m = st.modes.switch(&c.to_string()).unwrap_or_else(|e| e);
                st.look.say(m);
            }
        })
    };
    let notes = mcp_tools::start(tabs.clone(), app_state.clone());
    {
        // The mode switcher comes from the plugin registry (P6): enabled modes
        // in priority order (commander first by default), built-in or plugin.
        // The chat mode (P5)'s agent sees the tabs and runs tools through ChatHost.
        let reg = mouser_core::plugins::Registry::load();
        let modes = mouser_render::modes::Host::from_registry(&reg, |e| match e.name.as_str() {
            "chat" if e.transport == mouser_core::plugins::Transport::Builtin => {
                let host = Arc::new(chat_host::ChatHost::new(tabs.clone(), app_state.clone()));
                Some(Box::new(mouser_render::modes::chat::ChatMode::new(mouser_core::agent::Chat::new(host))) as Box<dyn mouser_render::modes::Mode>)
            }
            _ => None,
        });
        log::info!("modes: {}", if modes.names().is_empty() { "none (plain terminal)".to_string() } else { modes.names().join(", ") });
        let mut modes = modes;
        // Back in the mode (or no mode) the last window closed in.
        if let Some(last) = mouser_core::modes::load_last_mode() {
            modes.restore(last.as_deref());
        }
        let mut st = app_state.lock().unwrap();
        st.modes = modes;
        // modes.toml problems and modes left out (shown once).
        let notes = std::mem::take(&mut st.modes.notes);
        for n in &notes {
            log::warn!("{n}");
        }
        if let Some(n) = notes.into_iter().next() {
            st.look.say(n);
        }
    }
    mcp_clients();
    if !notes.is_empty() {
        for n in &notes {
            eprintln!("{n}");
        }
        app_state.lock().unwrap().look.say(notes.join("  "));
    }
    discover_font_families();
    let hooks = RenderHooks {
        on_key_bytes: on_key.clone(),
        get_images: Some(get_images),
        on_key_event: Some({
            let (writer, app_state, parser, on_key, tabs_e) =
                (shared_writer.clone(), app_state.clone(), parser.clone(), on_key.clone(), tabs.clone());
            Arc::new(move |legacy: Option<Vec<u8>>, input: mouser_core::kitty::KeyInput, fz: Option<mouser_core::fuzzball::KeyEvent>| {
                use mouser_core::kitty::{Encoded, encode_key};
                if exited_tab(&tabs_e).is_some() {
                    if let Some(b) = legacy {
                        on_key(b);
                    }
                    return;
                }
                let ui_owns = {
                    let st = app_state.lock().unwrap();
                    let alt = parser.get().lock().unwrap().screen.in_alt_screen();
                    // Keys for the current mode (and esc :) take the legacy path.
                    let mode_key =
                        !alt && legacy.as_deref().is_some_and(|b| st.esc_pending.is_some() || b == b"\x1b" || b == b"\x1b:" || st.modes.takes(b));
                    mode_key
                        || st.approvals.open()
                        || st.mode == Mode::Command
                        || st.ssh.panel.is_some()
                        || st.look.panel.is_some()
                        || st.pending_edit.is_some()
                        || st.pending_delete.is_some()
                };
                // Mouser's own hotkeys (F10, Alt+Esc) keep working in any mode.
                let hotkey = legacy.as_deref().is_some_and(|b| b == b"\x1b[21~" || b == b"\x1b\x1b");
                // Fuzzball keyboard mode wins over the Kitty keyboard fallback.
                let (fuzzball_on, flags) = {
                    let pa = parser.get();
                    let p = pa.lock().unwrap();
                    let alt = p.screen.in_alt_screen();
                    (p.fuzzball.keyboard(alt), p.kitty_keyboard.current(alt))
                };
                if fuzzball_on && !ui_owns && !hotkey {
                    if let Some(e) = fz {
                        let mut w = writer.clone();
                        let _ = w.write_all(mouser_core::fuzzball::encode_key(&e).as_bytes());
                    }
                    return;
                }
                if ui_owns || hotkey || flags == 0 {
                    if let Some(b) = legacy {
                        on_key(b);
                    }
                    return;
                }
                let bytes = match encode_key(&input, flags) {
                    Encoded::Bytes(b) => b,
                    Encoded::Legacy => match legacy {
                        Some(b) => b,
                        None => return,
                    },
                    Encoded::Nothing => return,
                };
                let mut w = writer.clone();
                let _ = w.write_all(&bytes);
            })
        }),
        on_resize,
        fetch_lines: Arc::new(|_, _| (Vec::new(), 0)),
        fetch_view: Some(fetch),
        get_cursor_shape: Some({
            let parser = parser.clone();
            Arc::new(move || match parser.get().lock().unwrap().screen.cursor_shape {
                mouser_core::grid::CursorShape::Block => mouser_render::CursorShape::Block,
                mouser_core::grid::CursorShape::Underline => mouser_render::CursorShape::Underline,
                mouser_core::grid::CursorShape::Bar => mouser_render::CursorShape::Bar,
            })
        }),
        on_mouse: Some({
            let (parser, writer) = (parser.clone(), shared_writer.clone());
            Arc::new(move |ev: mouser_core::input::MouseEvent| on_mouse(&parser.get(), &writer, ev))
        }),
        on_focus: Some({
            let (parser, writer) = (parser.clone(), shared_writer.clone());
            Arc::new(move |focused: bool| {
                let modes = parser.get().lock().unwrap().modes;
                if let Some(b) = mouser_core::input::encode_focus(focused, &modes) {
                    let mut w = writer.clone();
                    let _ = w.write_all(b);
                }
            })
        }),
        on_paste: Some({
            let (parser, writer, app_state) = (parser.clone(), shared_writer.clone(), app_state.clone());
            Arc::new(move || {
                if let Some(text) = get_clipboard() {
                    paste_text(&text, &parser, &writer, &app_state);
                }
            })
        }),
        on_select: Some({
            let parser = parser.clone();
            Arc::new(move |ev: mouser_render::SelectEvent| {
                log::debug!("select {ev:?}");
                let Some(session) = parser.session() else { return };
                let p = session.parser.lock().unwrap();
                let text = select::handle(&p, &mut session.selection.lock().unwrap(), ev);
                drop(p);
                if let Some(t) = text {
                    set_primary(&t);
                }
            })
        }),
        on_copy: Some({
            let parser = parser.clone();
            Arc::new(move || {
                let Some(session) = parser.session() else { return };
                let text = select::text(&session.selection.lock().unwrap());
                if let Some(t) = text {
                    set_clipboard(&t);
                }
            })
        }),
        on_paste_primary: Some({
            let (parser, writer, app_state) = (parser.clone(), shared_writer.clone(), app_state.clone());
            Arc::new(move || {
                if let Some(text) = get_primary() {
                    paste_text(&text, &parser, &writer, &app_state);
                }
            })
        }),
        hold_frame: Some({
            let parser = parser.clone();
            Arc::new(move || parser.get().lock().unwrap().modes.synchronized)
        }),
        get_title: Some({
            let parser = parser.clone();
            Arc::new(move || Some(parser.title()))
        }),
        get_colors,
        get_font: Some(get_font),
        get_cursor: Some(get_cursor),
        get_ui_model: Some(get_ui_model),
        on_ui_action: Some(on_ui_action),
        set_waker: Some({
            let app_state = app_state.clone();
            Arc::new(move |w: mouser_render::Waker| {
                app_state.lock().unwrap().modes.set_waker(w.clone());
                let _ = WAKER.set(w);
            })
        }),
        on_engine: Some(Arc::new(|e| *ACTIVE_ENGINE.lock().unwrap() = e)),
        on_window_size: None,
        // The window closes when the last tab's process has exited.
        should_exit: Some({
            let tabs = tabs.clone();
            Arc::new(move || tabs.lock().unwrap().reap())
        }),
        get_tabs: Some({
            let tabs = tabs.clone();
            Arc::new(move || tabs.lock().unwrap().bar())
        }),
        get_status: Some({
            let app_state = app_state.clone();
            Arc::new(move || {
                let st = app_state.lock().unwrap();
                if st.mode == Mode::Command { Some(format!(":{}", st.command_buf)) } else { st.look.status().or_else(engine_note) }
            })
        }),
    };
    let r = Renderer { font_size: font_size0, font_family: font_family0, title: cli.title.clone(), app_id: cli.class.clone(), ..Renderer::default() };
    {
        // Restore the last saved sidebar state (fresh install: collapsed strip);
        // MOUSER_OPEN_SIDEBAR forces it open for this run only.
        let mut st = app_state.lock().unwrap();
        let s = startup_sidebar(&prefs0, std::env::var("MOUSER_OPEN_SIDEBAR").ok().as_deref());
        (st.sidebar_open, st.sidebar_collapsed, st.sidebar_forced) = (s.open, s.collapsed, s.forced);
    }
    #[cfg(unix)]
    let result = if mouser_render::split::enabled() { mouser_render::split::serve(r, hooks) } else { run_renderer(r, hooks) };
    #[cfg(not(unix))]
    let result = run_renderer(r, hooks);
    // Window closed (by the user or because the shell exited): remember the sidebar.
    {
        let st = app_state.lock().unwrap();
        if !st.sidebar_forced {
            persist_sidebar_state(&st);
        }
        if let Err(e) = mouser_core::modes::save_last_mode(st.modes.current()) {
            log::warn!("could not save the last mode: {e}");
        }
    }
    result?;
    // Kill and reap anything still running (window closed by the user).
    for s in std::mem::take(&mut tabs.lock().unwrap().list) {
        s.kill();
    }
    Ok(())
}

/// The active parser, following tab switches.
#[derive(Clone)]
struct ActiveParser {
    tabs: Arc<Mutex<tabs::Tabs>>,
    /// Used once every tab is gone (the window is about to close).
    dummy: Arc<Mutex<tabs::Parser>>,
}

impl ActiveParser {
    fn session(&self) -> Option<Arc<tabs::Session>> {
        self.tabs.lock().unwrap().active()
    }
    fn get(&self) -> Arc<Mutex<tabs::Parser>> {
        self.session().map(|s| s.parser.clone()).unwrap_or_else(|| self.dummy.clone())
    }
    fn title(&self) -> String {
        self.session().map(|s| s.title()).unwrap_or_default()
    }
}

/// Paste into the command line, or into the active tab (bracketed when the
/// program asked for it, control characters dropped). Panels ignore pastes.
fn paste_text(text: &str, parser: &ActiveParser, writer: &tabs::ActiveWriter, app_state: &Mutex<AppState>) {
    let mut st = app_state.lock().unwrap();
    if st.approvals.open() {
        return;
    }
    if st.mode == Mode::Command {
        st.command_buf.push_str(text.lines().next().unwrap_or(""));
        return;
    }
    if st.ssh.panel.is_some() || st.look.panel.is_some() {
        return;
    }
    drop(st);
    let modes = parser.get().lock().unwrap().modes;
    let mut w = writer.clone();
    let _ = w.write_all(&mouser_core::input::paste_bytes(text, &modes));
}

/// Rows of the view `scrolled_back` lines above the live screen, with paint.
/// Scrollback lines are laid out in cells like the screen.
fn view_rows(p: &tabs::Parser, scrolled_back: usize, rows: usize, fg: u32) -> (Vec<String>, Vec<Vec<mouser_render::CellPaint>>, usize) {
    let screen_lines = p.screen.visible_lines();
    let screen_paint = term_view::screen_paint(p);
    if p.screen.in_alt_screen() {
        return (screen_lines, screen_paint, 0);
    }
    let sb = &p.scrollback;
    let sb_total = sb.total_lines();
    let back = scrolled_back.min(sb_total);
    if back == 0 {
        return (screen_lines, screen_paint, 0);
    }
    // Window of `rows` lines ending `back` lines above the live screen.
    let total = sb_total + screen_lines.len();
    let end = total - back;
    let start = end.saturating_sub(rows);
    let mut out = Vec::with_capacity(rows);
    let mut paint = Vec::with_capacity(rows);
    if start < sb_total {
        for l in sb.get_raw_lines(start, end.min(sb_total) - start) {
            let (text, mut pt) = select::scrollback_view(mouser_core::sbstyle::text(&l), fg);
            // Styled lines keep their colours and attributes.
            if mouser_core::sbstyle::is_styled(&l) {
                for (c, st) in pt.iter_mut().zip(mouser_core::sbstyle::cell_styles(&l, text.chars().count())) {
                    if st != Default::default() {
                        let keep = c.flags & (mouser_render::frame::paint::NO_GLYPH | mouser_render::frame::paint::WIDE);
                        let comb = c.comb;
                        *c = term_view::cell_paint(&mouser_core::grid::Cell { ch: ' ', comb, style: st }, p);
                        c.flags |= keep;
                        c.comb = comb;
                    }
                }
            }
            out.push(text);
            paint.push(pt);
        }
    }
    if end > sb_total {
        out.extend(screen_lines.into_iter().take(end - sb_total));
        paint.extend(screen_paint.into_iter().take(end - sb_total));
    }
    sb.prefetch_around(start, end);
    (out, paint, back)
}

/// Index of the active tab if its process already exited (waiting for a key).
fn exited_tab(tabs: &Mutex<tabs::Tabs>) -> Option<usize> {
    let t = tabs.lock().unwrap();
    let s = t.active()?;
    s.exited.lock().unwrap().is_some().then_some(t.active)
}

/// argv for a sidebar connection: run directly as a tab's process.
fn connect_argv(ssh: &ssh_ui::SshUi, category: &ui::Category, name: &str) -> Option<Vec<String>> {
    if *category == ui::Category::Ssh {
        return Some(ssh.connect_argv(name));
    }
    let conn = load_connection(name).ok().flatten()?;
    Some(match &conn.kind {
        ConnectionKind::Ssh(c) => mouser_core::connections::build_ssh_args(c),
        ConnectionKind::Waypipe(c) => mouser_core::connections::build_waypipe_args(c),
        ConnectionKind::Tunnel(c) => mouser_core::connections::build_tunnel_args(c),
        ConnectionKind::X11(c) => mouser_core::connections::build_x11_args(c),
    })
}

/// Mouse over the terminal: report it when the app asked (Shift bypasses),
/// open OSC 8 links on Ctrl+click, and turn the wheel into arrow keys on the
/// alternate screen (xterm's alternateScroll).
fn on_mouse(parser: &Mutex<tabs::Parser>, writer: &tabs::ActiveWriter, ev: mouser_core::input::MouseEvent) -> bool {
    use mouser_core::input::{MouseButton, MouseKind};
    let pa = parser;
    let p = pa.lock().unwrap();
    let mut w = writer.clone();
    if p.modes.mouse != mouser_core::parser::MouseMode::Off && !ev.shift {
        if let Some(b) = mouser_core::input::encode_mouse(&ev, &p.modes) {
            drop(p);
            let _ = w.write_all(&b);
        }
        return true;
    }
    if ev.kind == MouseKind::Press && ev.button == MouseButton::Left && ev.ctrl {
        if let Some(uri) = p.link_at(ev.col, ev.row) {
            open_link(uri);
            return true;
        }
        return false;
    }
    if ev.kind == MouseKind::Press && matches!(ev.button, MouseButton::WheelUp | MouseButton::WheelDown) && p.screen.in_alt_screen() {
        let key: &[u8] = if ev.button == MouseButton::WheelUp { b"\x1b[A" } else { b"\x1b[B" };
        let bytes = mouser_core::input::apply_cursor_mode(key.to_vec(), &p.modes);
        drop(p);
        for _ in 0..3 {
            let _ = w.write_all(&bytes);
        }
        return true;
    }
    false
}

/// Only web, mail and file links are opened, and never through a shell.
fn open_link(uri: &str) {
    let ok = ["http://", "https://", "mailto:", "file://"].iter().any(|s| uri.starts_with(s));
    if !ok {
        log::warn!("not opening link with unsupported scheme: {uri}");
        return;
    }
    let _ = std::process::Command::new("xdg-open").arg(uri).stdin(std::process::Stdio::null()).spawn();
}

/// Clipboard writers to try, in order: Wayland, X11, macOS.
fn copy_commands(primary: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return if primary { Vec::new() } else { vec![("pbcopy", vec![])] };
    }
    let sel = if primary { "primary" } else { "clipboard" };
    let mut v = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        v.push(("wl-copy", if primary { vec!["--primary"] } else { vec![] }));
    }
    v.push(("xclip", vec!["-selection", sel]));
    v.push(("xsel", vec![if primary { "--primary" } else { "--clipboard" }, "--input"]));
    v
}

fn paste_commands(primary: bool) -> Vec<(&'static str, Vec<&'static str>)> {
    if cfg!(target_os = "macos") {
        return if primary { Vec::new() } else { vec![("pbpaste", vec![])] };
    }
    let sel = if primary { "primary" } else { "clipboard" };
    let mut v = Vec::new();
    if std::env::var_os("WAYLAND_DISPLAY").is_some() {
        v.push(("wl-paste", if primary { vec!["--primary", "--no-newline"] } else { vec!["--no-newline"] }));
    }
    v.push(("xclip", vec!["-selection", sel, "-o"]));
    v.push(("xsel", vec![if primary { "--primary" } else { "--clipboard" }, "--output"]));
    v
}

fn write_clip(text: &str, primary: bool) {
    use std::process::{Command, Stdio};
    if mouser_render::clipboard::store(text, primary) {
        return;
    }
    for (cmd, args) in copy_commands(primary) {
        if let Ok(mut c) = Command::new(cmd).args(&args).stdin(Stdio::piped()).stdout(Stdio::null()).stderr(Stdio::null()).spawn() {
            if let Some(mut stdin) = c.stdin.take() {
                let _ = stdin.write_all(text.as_bytes());
            }
            std::thread::spawn(move || {
                let _ = c.wait();
            });
            return;
        }
    }
}

fn read_clip(primary: bool) -> Option<String> {
    if let Some(t) = mouser_render::clipboard::load(primary) {
        return t;
    }
    for (cmd, args) in paste_commands(primary) {
        if let Ok(out) = std::process::Command::new(cmd).args(&args).stderr(std::process::Stdio::null()).output() {
            return out.status.success().then(|| String::from_utf8_lossy(&out.stdout).to_string());
        }
    }
    None
}

/// Copy to the clipboard (Ctrl+Shift+C, OSC 52): wl-copy, xclip or xsel; pbcopy on macOS.
pub(crate) fn set_clipboard(text: &str) {
    write_clip(text, false);
}

fn get_clipboard() -> Option<String> {
    read_clip(false)
}

/// Selecting sets the primary selection (Linux), which a middle click pastes.
fn set_primary(text: &str) {
    write_clip(text, true);
}

fn get_primary() -> Option<String> {
    read_clip(true)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SidebarStart {
    open: bool,
    collapsed: bool,
    /// Forced open by the environment; the saved state is left alone.
    forced: bool,
}

/// Sidebar state at startup: `MOUSER_OPEN_SIDEBAR` (anything but empty/0/false/no)
/// forces it open and expanded; otherwise the last saved state, or the
/// collapsed strip when nothing was saved.
fn startup_sidebar(prefs: &mouser_core::storage::Preferences, env: Option<&str>) -> SidebarStart {
    if env.is_some_and(|v| !matches!(v.trim(), "" | "0" | "false" | "no")) {
        return SidebarStart { open: true, collapsed: false, forced: true };
    }
    SidebarStart { open: prefs.sidebar_open.unwrap_or(true), collapsed: prefs.sidebar_collapsed.unwrap_or(true), forced: false }
}

static FONT_FAMILIES: std::sync::OnceLock<Arc<Vec<String>>> = std::sync::OnceLock::new();

/// Installed font families once the background scan is done.
pub(crate) fn font_families() -> Option<Arc<Vec<String>>> {
    FONT_FAMILIES.get().cloned()
}

/// Enumerate installed families on a worker thread, then wake the renderer.
fn discover_font_families() {
    std::thread::spawn(|| {
        let t = std::time::Instant::now();
        let names = mouser_core::fuzzy::dedupe_names(mouser_render::text::installed_families(&mouser_render::text::system_db()));
        log::info!("found {} font families in {:?}", names.len(), t.elapsed());
        let _ = FONT_FAMILIES.set(Arc::new(names));
        wake();
    });
}

/// MCP client connections (`[[mcp.servers]]` and MCP plugins), started once.
fn mcp_clients() -> &'static Arc<mouser_core::mcp_client::Manager> {
    static M: std::sync::OnceLock<Arc<mouser_core::mcp_client::Manager>> = std::sync::OnceLock::new();
    M.get_or_init(|| mouser_core::mcp_client::Manager::start(wake))
}

/// Servers whose tools are shown in the sidebar (click a connected server to toggle).
fn mcp_open() -> std::sync::MutexGuard<'static, std::collections::HashSet<String>> {
    static O: std::sync::OnceLock<Mutex<std::collections::HashSet<String>>> = std::sync::OnceLock::new();
    O.get_or_init(Default::default).lock().unwrap()
}

/// Sidebar MCP section: each server's state and (when opened) its tools.
fn mcp_rows() -> Vec<ui::McpServerSpec> {
    use mouser_core::mcp_client::Status;
    let open = mcp_open().clone();
    mcp_clients()
        .snapshot()
        .into_iter()
        .map(|v| ui::McpServerSpec {
            tools: if open.contains(&v.id) { v.tools.iter().map(|t| t.name.clone()).collect() } else { vec![] },
            label: match (&v.status, v.tools.len()) {
                (Status::Connected, n) => format!("{} {} ({n})", if open.contains(&v.id) { "▾" } else { "▸" }, v.id),
                _ => v.id.clone(),
            },
            ok: v.status == Status::Connected,
            bad: matches!(v.status, Status::Failed(..) | Status::Denied),
            state: v.status.label().into(),
            note: match &v.status {
                Status::Failed(e, n) => Some(format!("{e} (retry in {n}s; click to retry now)")),
                Status::NeedsApproval => Some("click to allow it to start".into()),
                _ => None,
            },
            id: v.id,
        })
        .collect()
}

fn mcp_server_click(id: &str, app_state: &Arc<Mutex<AppState>>) {
    use mouser_core::mcp_client::{Source, Status, broker_name};
    let Some(v) = mcp_clients().snapshot().into_iter().find(|v| v.id == id) else { return };
    let mut st = app_state.lock().unwrap();
    if v.status == Status::NeedsApproval {
        let runs = match &v.source {
            Source::Plugin(_, argv) => argv.join(" "),
            Source::Config(_) => String::new(),
        };
        st.approvals.push(approval_ui::Request::new(
            &broker_name(id),
            vec!["run".into()],
            vec![format!("Start MCP plugin: {runs}")],
            mouser_core::permissions::Scope::Installed,
            approval_ui::Origin::McpRun { id: id.to_string() },
        ));
        wake();
        return;
    }
    if v.status == Status::Connected {
        let mut o = mcp_open();
        if !o.remove(id) {
            o.insert(id.to_string());
        }
        drop(o);
        wake();
        return;
    }
    st.look.say(format!("[mouser] reconnecting {id}"));
    drop(st);
    mcp_clients().reconnect(id);
}

/// `[mouser]` line: `mcp` (list), `mcp reconnect ID`, `mcp tools ID`, `mcp call ID TOOL [JSON]`.
fn mcp_command(line: &str, app_state: &Arc<Mutex<AppState>>) {
    let words: Vec<&str> = line.splitn(4, ' ').collect();
    let say = |m: String| app_state.lock().unwrap().look.say(m);
    match words.as_slice() {
        ["mcp"] | ["mcp", "servers"] => {
            let l: Vec<String> =
                mcp_clients().snapshot().iter().map(|v| format!("{} {} ({} tools)", v.id, v.status.label(), v.tools.len())).collect();
            say(if l.is_empty() { "[mouser] no MCP servers (mouser mcp servers add)".into() } else { format!("[mouser] MCP: {}", l.join(" · ")) });
        }
        ["mcp", "reconnect", id] => {
            let waiting = mcp_clients().snapshot().iter().any(|v| v.id == *id && v.status == mouser_core::mcp_client::Status::NeedsApproval);
            if waiting {
                mcp_server_click(id, app_state);
            } else {
                say(format!("[mouser] reconnecting {id}"));
                mcp_clients().reconnect(id);
            }
        }
        ["mcp", "tools", id] => {
            let v = mcp_clients().snapshot().into_iter().find(|v| v.id == *id);
            match v {
                Some(v) => {
                    mcp_open().insert(v.id.clone());
                    say(format!("[mouser] {id}: {}", v.tools.iter().map(|t| t.name.as_str()).collect::<Vec<_>>().join(", ")));
                }
                None => say(format!("[mouser] no MCP server {id}")),
            }
        }
        ["mcp", "call", id, rest] => {
            let (tool, args) = rest.split_once(' ').unwrap_or((rest, ""));
            let args = if args.trim().is_empty() { None } else { Some(args.trim()) };
            match args.map(serde_json::from_str::<serde_json::Value>).transpose() {
                Ok(a) => mcp_call(id, tool, a, app_state),
                Err(e) => say(format!("[mouser] mcp call: arguments must be JSON ({e})")),
            }
        }
        ["mcp", "call", id] => say(format!("[mouser] usage: mcp call {id} TOOL [JSON]")),
        _ => say("[mouser] mcp | mcp reconnect ID | mcp tools ID | mcp call ID TOOL [JSON]".into()),
    }
}

/// A tool row click: call it (through the broker) when it takes no required arguments.
fn mcp_tool_click(id: &str, tool: &str, app_state: &Arc<Mutex<AppState>>) {
    mcp_call(id, tool, None, app_state);
}

/// Call a tool through the broker (asks with the popup); `args: None` means `{}`
/// and is refused for tools with required arguments.
fn mcp_call(id: &str, tool: &str, args: Option<serde_json::Value>, app_state: &Arc<Mutex<AppState>>) {
    use mouser_core::mcp_client::{broker_name, check_tool, tool_cap};
    use mouser_core::permissions::Check;
    let desc = mcp_clients().tool_json(id, tool);
    let required: Vec<String> = desc
        .as_ref()
        .and_then(|d| d["inputSchema"]["required"].as_array().cloned())
        .unwrap_or_default()
        .iter()
        .filter_map(|v| v.as_str().map(String::from))
        .collect();
    let about = desc.as_ref().and_then(|d| d["description"].as_str().map(String::from)).unwrap_or_default();
    let mut st = app_state.lock().unwrap();
    if desc.is_none() {
        st.look.say(format!("[mouser] {id} has no tool {tool} (or is not connected)"));
        return;
    }
    if args.is_none() && !required.is_empty() {
        st.look.say(format!("[mouser] {id}: {tool} needs {}: {about}", required.join(", ")));
        return;
    }
    match check_tool(id, tool) {
        Check::Allow => {
            drop(st);
            spawn_mcp_call(id, tool, args, app_state);
        }
        Check::Deny => st.look.say(format!("[mouser] {id}: {tool} is set to never (mouser plugin permissions {})", broker_name(id))),
        Check::Ask(_) => {
            st.approvals.push(approval_ui::Request::new(
                &broker_name(id),
                vec![tool_cap(tool)],
                vec![
                    format!("Server: {id}"),
                    format!("Tool: {tool} {}", args.as_ref().map(|a| a.to_string()).unwrap_or_default()).chars().take(200).collect(),
                    about.chars().take(200).collect(),
                ],
                mouser_core::permissions::Scope::Installed,
                approval_ui::Origin::McpTool { id: id.to_string(), tool: tool.to_string(), args },
            ));
            wake();
        }
    }
}

/// Call an allowed tool off the UI thread; the result becomes a notice.
fn spawn_mcp_call(id: &str, tool: &str, args: Option<serde_json::Value>, app_state: &Arc<Mutex<AppState>>) {
    let (id, tool, app_state) = (id.to_string(), tool.to_string(), app_state.clone());
    std::thread::spawn(move || {
        let msg = match mcp_clients().call(&id, &tool, args.unwrap_or_else(|| serde_json::json!({}))) {
            Ok(t) => format!("[{id}] {tool}: {}", t.split_whitespace().collect::<Vec<_>>().join(" ").chars().take(400).collect::<String>()),
            Err(e) => format!("[{id}] {tool} failed: {e:#}"),
        };
        app_state.lock().unwrap().look.say(msg);
        wake();
    });
}

/// Sidebar Engines section: available engines in priority order with state.
fn engine_rows() -> Vec<(String, ui::EngineState, Option<String>)> {
    use mouser_core::engines;
    let active = ACTIVE_ENGINE.lock().unwrap().clone();
    // Enabled engines from the plugin registry (built-ins only if this build has them).
    let built = mouser_render::engine::available_builtins();
    let mut names: Vec<String> = mouser_core::plugins::Registry::load()
        .engines()
        .iter()
        .filter(|e| e.dir.is_some() || built.iter().any(|(b, _)| *b == e.name))
        .map(|e| e.name.clone())
        .collect();
    let order = engines::priority();
    names.sort_by_key(|n| order.iter().position(|o| o == n).unwrap_or(usize::MAX));
    names
        .into_iter()
        .map(|n| {
            let (state, note) = match engines::state(&n) {
                _ if n == active => (ui::EngineState::Active, None),
                engines::State::Ok => (ui::EngineState::Ok, None),
                engines::State::Skipped { reason, .. } => {
                    (ui::EngineState::Skipped, Some(reason.unwrap_or_else(|| "crashed Mouser while starting".into())))
                }
                engines::State::Failed { reason, .. } => (ui::EngineState::Failed, Some(reason)),
            };
            (n, state, note)
        })
        .collect()
}

/// Engines dragged into a new order: same as `mouser engine priority ...`.
/// Running windows apply it through their preferences watcher.
fn reorder_engines(order: &[String]) {
    match mouser_core::engines::set_priority(order) {
        Ok(l) => {
            log::info!("sidebar: engine priority {}", l.join(" > "));
            if override_set() {
                set_engine_note(format!(
                    "engine order saved ({}); MOUSER_ENGINE is set, so this window keeps its order until restarted without it",
                    l.join(" > ")
                ));
            }
        }
        Err(e) => set_engine_note(format!("cannot save the engine order: {e:#}")),
    }
}

fn override_set() -> bool {
    std::env::var("MOUSER_ENGINE").is_ok_and(|v| !v.trim().is_empty())
}

/// Short status-bar note from the Engines section (shown for 8 s).
static ENGINE_NOTE: std::sync::Mutex<Option<(std::time::Instant, String)>> = std::sync::Mutex::new(None);

fn set_engine_note(msg: String) {
    log::warn!("{msg}");
    *ENGINE_NOTE.lock().unwrap() = Some((std::time::Instant::now(), msg));
    if let Some(w) = WAKER.get() {
        w();
    }
}

fn engine_note() -> Option<String> {
    let g = ENGINE_NOTE.lock().unwrap();
    g.as_ref().filter(|(t, _)| t.elapsed() < std::time::Duration::from_secs(8)).map(|(_, m)| m.clone())
}

/// A click on an engine row: same as `mouser engine use NAME` (unskip, move
/// first). The window's preferences watcher then switches it live.
fn use_engine(name: &str) {
    match mouser_core::engines::select(name) {
        Ok(()) => log::info!("sidebar: engine {name} is now first"),
        Err(e) => log::warn!("sidebar: cannot use engine {name}: {e:#}"),
    }
    if override_set() {
        set_engine_note(format!("{name} is now first; MOUSER_ENGINE is set, so this window keeps its order until restarted without it"));
    }
}

/// The engine the window draws with now (sidebar engines section).
static ACTIVE_ENGINE: std::sync::Mutex<String> = std::sync::Mutex::new(String::new());
static WAKER: std::sync::OnceLock<mouser_render::Waker> = std::sync::OnceLock::new();

/// Ask the renderer for a redraw (from any thread). The event loop sleeps until
/// something calls this or a window event arrives.
pub(crate) fn wake() {
    if let Some(w) = WAKER.get() {
        w();
    }
}

/// Wake the renderer after `delay` (e.g. when a status message expires).
pub(crate) fn wake_after(delay: std::time::Duration) {
    std::thread::spawn(move || {
        std::thread::sleep(delay);
        wake();
    });
}

/// `theme [name]`, `mode m`, `edit-theme`, `font-size n`, `font [family]` (leading ':' optional).
fn appearance_command(cmd: &str) -> Option<(&'static str, &str)> {
    let cmd = cmd.trim_start_matches(':');
    let (head, arg) = cmd.split_once(' ').unwrap_or((cmd, ""));
    let name = ["theme", "mode", "edit-theme", "font-size", "font"].into_iter().find(|n| *n == head)?;
    Some((name, arg))
}

/// The user changed the sidebar: save it (this also ends a forced-open run's
/// "don't save" grace).
fn persist_sidebar(st: &mut AppState) {
    st.sidebar_forced = false;
    persist_sidebar_state(st);
}

/// Save sidebar open/collapsed/width to preferences.toml.
fn persist_sidebar_state(st: &AppState) {
    let (open, collapsed, width) = (st.sidebar_open, st.sidebar_collapsed, st.sidebar_width);
    let _ = mouser_core::storage::update_preferences(|p| {
        p.sidebar_open = Some(open);
        p.sidebar_collapsed = Some(collapsed);
        if width.is_some() {
            p.sidebar_width = width;
        }
    });
}

fn category_of(kind: &ConnectionKind) -> ui::Category {
    match kind {
        ConnectionKind::Ssh(_) => ui::Category::Ssh,
        ConnectionKind::Waypipe(_) => ui::Category::Waypipe,
        ConnectionKind::Tunnel(_) => ui::Category::Tunnels,
        ConnectionKind::X11(_) => ui::Category::X11,
    }
}

// no proxy needed; master is stored separately

/// A `[mouser]` command line that is not a Mouser command runs in the tab's
/// shell; a leading `!` forces the shell even when the name is a Mouser
/// command. `None`: nothing to run.
fn forced_shell(line: &str) -> Option<&str> {
    line.strip_prefix('!').map(str::trim).filter(|c| !c.is_empty())
}

/// Type `cmd` into the active tab's shell, keeping what was typed (like
/// commander's cd and pipe). Refused with a notice when another program owns
/// the terminal (it would get the keystrokes).
fn run_in_shell(cmd: &str, writer: &tabs::ActiveWriter, app_state: &Arc<Mutex<AppState>>) {
    let session = writer.0.lock().unwrap().active();
    let mut st = app_state.lock().unwrap();
    if session.as_ref().and_then(|s| s.shell_in_foreground()) == Some(false) {
        st.look.say("[mouser] the shell is busy (another program is running); not typing the command");
        return;
    }
    apply_mode_effect(&mut st, mouser_render::modes::Effect::Run(cmd.to_string()), writer);
}

fn handle_command(cmd: &str, writer: &tabs::ActiveWriter, app_state: &Arc<Mutex<AppState>>) {
    let trimmed = cmd.trim();
    if trimmed.is_empty() {
        return;
    }
    if let Some(c) = forced_shell(trimmed) {
        run_in_shell(c, writer, app_state);
    } else if trimmed == "!" {
    } else if app_state.lock().unwrap().modes.has(trimmed) {
        // `esc :c` / `:commander`: switch modes.
        let mut st = app_state.lock().unwrap();
        let m = st.modes.switch(trimmed).unwrap_or_else(|e| e);
        st.look.say(m);
    } else if trimmed == "mcp" || trimmed.starts_with("mcp ") {
        mcp_command(trimmed, app_state);
    } else if trimmed == "open-sidebar" || trimmed == ":open-sidebar" {
        let mut st = app_state.lock().unwrap();
        st.sidebar_open = true;
        st.sidebar_collapsed = false;
        persist_sidebar(&mut st);
    } else if let Some(cmd) = appearance_command(trimmed) {
        let mut st = app_state.lock().unwrap();
        match cmd {
            ("theme", arg) => st.look.command_theme(arg),
            ("mode", arg) => st.look.command_mode(arg),
            ("edit-theme", _) => {
                st.look.open_picker();
                st.look.open_editor();
            }
            ("font-size", arg) => match mouser_core::appearance::parse_size(arg) {
                Some(v) => st.look.set_font_size(v),
                None => st.look.say("Usage: :font-size 6-48"),
            },
            (_, arg) if arg.trim().is_empty() => st.look.open_font_picker(),
            (_, arg) => st.look.set_font_family((!arg.trim().is_empty()).then(|| arg.trim().to_string())),
        }
    } else if trimmed == "close-sidebar" || trimmed == ":close-sidebar" {
        let mut st = app_state.lock().unwrap();
        st.sidebar_open = false;
        persist_sidebar(&mut st);
    } else if let Some(name) = trimmed.strip_prefix("del-conn ") {
        let mut st = app_state.lock().unwrap();
        st.pending_delete = Some(name.to_string());
        drop(st);
        let mut w = writer.clone();
        let _ = w.write_all(format!("\r\n[mouser] confirm delete '{name}' with :yes or cancel with :no\r\n").as_bytes());
    } else if trimmed == "yes" || trimmed == ":yes" {
        let name_opt = app_state.lock().unwrap().pending_delete.take();
        if let Some(name) = name_opt {
            let _ = delete_connection(&name);
            let mut w = writer.clone();
            let _ = w.write_all(format!("\r\n[mouser] deleted '{name}'\r\n").as_bytes());
        }
    } else if trimmed == "no" || trimmed == ":no" {
        app_state.lock().unwrap().pending_delete = None;
    } else if let Some(name) = trimmed.strip_prefix("edit-conn ") {
        if let Ok(Some(conn)) = load_connection(name) {
            app_state.lock().unwrap().pending_edit = Some(conn.clone());
            let mut w = writer.clone();
            let _ = w.write_all(format!("\r\n[mouser] editing '{name}' — use :set key=value (:save-conn to persist)\r\n").as_bytes());
        } else {
            let mut w = writer.clone();
            let _ = w.write_all(format!("\r\n[mouser] no such connection: {name}\r\n").as_bytes());
        }
    } else if let Some(kv) = trimmed.strip_prefix("set ") {
        if let Some(ref mut conn) = app_state.lock().unwrap().pending_edit {
            apply_set(conn, kv);
        }
    } else if trimmed == "save-conn" || trimmed == ":save-conn" {
        if let Some(conn) = app_state.lock().unwrap().pending_edit.take() {
            let _ = save_connection(&conn);
            let mut w = writer.clone();
            let _ = w.write_all(format!("\r\n[mouser] saved '{}'\r\n", conn.name).as_bytes());
        }
    } else if let Some(rest) = trimmed.strip_prefix("connect ") {
        let mut w = writer.clone();
        let _ = w.write_all(format!("\r\n[mouser] launching connection: {rest}\r\n").as_bytes());
    } else if let Some(arg) = trimmed.strip_prefix("model download").filter(|r| r.is_empty() || r.starts_with(' ')) {
        model_download_request(arg.trim(), app_state);
    } else if command_plugin(trimmed, app_state) {
        // A command plugin's word: run (or asked to run) by the plugin.
    } else {
        // Not a Mouser command: the shell's.
        run_in_shell(trimmed, writer, app_state);
    }
}

/// Whether a tab's program is on another host (Fuzzball plugins from there are untrusted).
fn session_remote(s: &tabs::Session) -> bool {
    matches!(s.place(), Some(mouser_core::remote::Place::Remote { .. }))
        || s.foreground_argv().and_then(|a| a.first().cloned()).is_some_and(|p| {
            let p = p.rsplit('/').next().unwrap_or(&p).to_string();
            matches!(p.as_str(), "ssh" | "mosh" | "mosh-client" | "et" | "autossh")
        })
}

/// Pick up Fuzzball `p h` handshakes from every tab and run them by the broker.
fn plugin_hellos(st: &mut AppState, sessions: &[std::sync::Arc<tabs::Session>]) {
    use mouser_core::permissions::{Broker, Check, Scope};
    for s in sessions {
        let hellos = std::mem::take(&mut s.parser.lock().unwrap().fuzzball.plugin_hellos);
        for h in hellos {
            let scope = if session_remote(s) {
                let host = match s.place() {
                    Some(mouser_core::remote::Place::Remote { ssh: Some(t), .. }) => t.identity(),
                    _ => None,
                };
                Scope::FuzzballRemote { host }
            } else {
                Scope::FuzzballLocal
            };
            match Broker::check(&h.name, &h.caps, &scope) {
                Check::Allow => fuzzball_answer(s, h.id, &h.name, Some(&h.caps)),
                Check::Deny => fuzzball_answer(s, h.id, &h.name, None),
                Check::Ask(_) => {
                    let detail = vec![format!("Tab: {}   kind: {}   scope: this tab only, until it closes", s.title(), h.kind)];
                    st.approvals.push(approval_ui::Request::new(
                        &h.name,
                        h.caps.clone(),
                        detail,
                        scope,
                        approval_ui::Origin::Fuzzball { session: s.clone(), id: h.id },
                    ));
                }
            }
        }
    }
}

fn fuzzball_answer(s: &tabs::Session, id: u32, name: &str, granted: Option<&[String]>) {
    if let Some(c) = granted {
        s.parser.lock().unwrap().fuzzball.plugin_grants.insert(name.to_string(), c.to_vec());
    }
    let mut w = s.writer.clone();
    let _ = w.write_all(mouser_core::fuzzball::plugin_reply(id, granted).as_bytes());
    let _ = w.flush();
}

/// Apply the user's answer to a popup.
fn resolve_approval(st: &mut AppState, req: approval_ui::Request, a: mouser_core::permissions::Answer, app_state: &Arc<Mutex<AppState>>) {
    if !req.once_only {
        let _ = mouser_core::permissions::Broker::answer(&req.plugin, &req.caps, &req.scope, a);
    }
    match req.origin {
        approval_ui::Origin::ModelDownload { id } => {
            if a.granted() {
                spawn_model_download(&id, app_state);
            } else {
                st.look.say(format!("[mouser] {id} was not downloaded"));
            }
        }
        approval_ui::Origin::Fuzzball { session, id } => {
            fuzzball_answer(&session, id, &req.plugin, a.granted().then_some(&req.caps[..]));
            st.look.say(format!("[mouser] {} plugin {}", if a.granted() { "allowed" } else { "denied" }, req.plugin));
        }
        approval_ui::Origin::Mcp { reply } => {
            let _ = reply.send(a.granted());
        }
        approval_ui::Origin::McpRun { id } => {
            if a.granted() {
                mcp_clients().approve_run(&id);
            } else {
                st.look.say(format!("[mouser] {id} was not allowed to run"));
                mcp_clients().reconnect(&id);
            }
        }
        approval_ui::Origin::McpTool { id, tool, args } => {
            if a.granted() {
                spawn_mcp_call(&id, &tool, args, app_state);
            } else {
                st.look.say(format!("[mouser] {id}: {tool} was not allowed"));
            }
        }
        approval_ui::Origin::Command { line } => {
            if a.granted() {
                spawn_command_plugin(&req.plugin, &line, app_state);
            } else {
                st.look.say(format!("[mouser] {} was not allowed to run", req.plugin));
            }
        }
    }
    wake();
}

/// `[mouser] model download [ID]`: show the size and ask (every time) first.
fn model_download_request(arg: &str, app_state: &Arc<Mutex<AppState>>) {
    use mouser_core::models::local;
    let id = if arg.is_empty() { local::CATALOG[0].id } else { arg };
    let mut st = app_state.lock().unwrap();
    let Some(w) = local::weights(id) else {
        let ids: Vec<&str> = local::CATALOG.iter().map(|w| w.id).collect();
        st.look.say(format!("[mouser] unknown local model {id}; one of: {}", ids.join(", ")));
        return;
    };
    if w.installed() {
        st.look.say(format!("[mouser] {} is already downloaded: {}", w.id, w.path().display()));
        return;
    }
    st.approvals.push(approval_ui::Request::once("Model download", w.consent_text(), approval_ui::Origin::ModelDownload { id: w.id.to_string() }));
    wake();
}

/// Download approved weights off the UI thread, with progress notices every 10%.
fn spawn_model_download(id: &str, app_state: &Arc<Mutex<AppState>>) {
    use mouser_core::models::local;
    let Some(w) = local::weights(id) else { return };
    let app_state = app_state.clone();
    std::thread::spawn(move || {
        let say = |m: String| {
            app_state.lock().unwrap().look.say(m);
            wake();
        };
        say(format!("[mouser] downloading {} ({})…", w.label, local::human(w.size)));
        let mut last = 0;
        let r = local::download(local::Consent::given(w), &w.url(), &mut |d, t| {
            let tenth = d * 10 / t.max(1);
            if tenth > last && tenth < 10 {
                last = tenth;
                say(format!("[mouser] {} {}0% ({} of {})", w.id, tenth, local::human(d), local::human(t)));
            }
            true
        });
        say(match r {
            Ok(p) => format!("[mouser] {} downloaded and verified: {}", w.id, p.display()),
            Err(e) => format!("[mouser] {} download failed: {e:#}", w.id),
        });
    });
}

/// `[mouser]` line words owned by a command plugin. True when it was one.
fn command_plugin(line: &str, app_state: &Arc<Mutex<AppState>>) -> bool {
    use mouser_core::permissions::{Broker, Check, Scope};
    let (word, _) = mouser_core::plugins::split_line(line);
    let reg = mouser_core::plugins::Registry::load();
    let Some(e) = reg.command(word) else { return false };
    let name = e.name.clone();
    let caps = vec!["run".to_string()];
    let mut st = app_state.lock().unwrap();
    match Broker::check(&name, &caps, &Scope::Installed) {
        Check::Allow => {
            drop(st);
            spawn_command_plugin(&name, line, app_state);
        }
        Check::Deny => st.look.say(format!("[mouser] {name} is set to never run (mouser plugin permissions {name})")),
        Check::Ask(_) => {
            let argv = mouser_core::plugins::find(&name).map(|i| i.argv().join(" ")).unwrap_or_default();
            let detail = vec![format!("Command: {line}"), format!("Runs: {argv}")];
            st.approvals.push(approval_ui::Request::new(
                &name,
                caps,
                detail,
                Scope::Installed,
                approval_ui::Origin::Command { line: line.to_string() },
            ));
            wake();
        }
    }
    true
}

/// Run a command plugin off the UI thread; its reply becomes a notice.
fn spawn_command_plugin(name: &str, line: &str, app_state: &Arc<Mutex<AppState>>) {
    let (name, line, app_state) = (name.to_string(), line.to_string(), app_state.clone());
    std::thread::spawn(move || {
        let (word, args) = mouser_core::plugins::split_line(&line);
        let msg = match mouser_core::plugins::find(&name) {
            None => format!("[mouser] plugin {name} is gone"),
            Some(p) => {
                let cwd = std::env::current_dir().ok().map(|d| d.display().to_string());
                let req = mouser_core::plugins::CommandRequest { api: mouser_core::plugins::API, command: word.to_string(), args, cwd };
                match mouser_core::plugins::run_command(&p, &req) {
                    Ok(r) => match r.error {
                        Some(e) => format!("[{name}] error: {e}"),
                        None => format!("[{name}] {}", r.text.trim()),
                    },
                    Err(e) => format!("[mouser] {e:#}"),
                }
            }
        };
        app_state.lock().unwrap().look.say(msg);
        wake();
    });
}

fn apply_set(conn: &mut Connection, kv: &str) {
    if let Some((k, v)) = kv.split_once('=') {
        match &mut conn.kind {
            ConnectionKind::Ssh(s) => set_ssh(s, k.trim(), v.trim()),
            ConnectionKind::Waypipe(w) => set_waypipe(w, k.trim(), v.trim()),
            ConnectionKind::Tunnel(t) => set_tunnel(t, k.trim(), v.trim()),
            ConnectionKind::X11(x) => set_x11(x, k.trim(), v.trim()),
        }
    }
}

fn set_ssh(s: &mut SshConnection, k: &str, v: &str) {
    match k {
        "host" => s.host = v.into(),
        "user" => s.user = Some(v.into()),
        "port" => s.port = v.parse().ok(),
        "identity_file" => s.identity_file = Some(v.into()),
        "proxy_jump" => s.proxy_jump = Some(v.into()),
        "compression" => s.compression = Some(matches!(v, "1" | "true" | "on")),
        "server_alive_interval" => s.server_alive_interval = v.parse().ok(),
        "server_alive_count_max" => s.server_alive_count_max = v.parse().ok(),
        _ => {
            s.extra_options.insert(k.to_string(), v.to_string());
        }
    }
}

fn set_waypipe(w: &mut WaypipeConnection, k: &str, v: &str) {
    match k {
        "host" => w.host = v.into(),
        "ssh_user" => w.ssh_user = Some(v.into()),
        "ssh_port" => w.ssh_port = v.parse().ok(),
        "compression" => w.compression = Some(v.into()),
        "video" => w.video = Some(v.into()),
        "socket" => w.socket = Some(v.into()),
        "arg" => w.args.push(v.into()),
        _ => {}
    }
}

fn set_tunnel(t: &mut TunnelConnection, k: &str, v: &str) {
    match k {
        "host" => t.host = v.into(),
        "user" => t.user = Some(v.into()),
        "port" => t.port = v.parse().ok(),
        "forwardL" => t.forwards.push(TunnelSpec { tunnel_type: TunnelType::Local, spec: v.into() }),
        "forwardR" => t.forwards.push(TunnelSpec { tunnel_type: TunnelType::Remote, spec: v.into() }),
        "forwardD" => t.forwards.push(TunnelSpec { tunnel_type: TunnelType::Dynamic, spec: v.into() }),
        _ => {
            t.extra_options.insert(k.to_string(), v.to_string());
        }
    }
}

fn set_x11(x: &mut X11Connection, k: &str, v: &str) {
    match k {
        "host" => x.host = v.into(),
        "user" => x.user = Some(v.into()),
        "trusted" => x.trusted = matches!(v, "1" | "true" | "on"),
        _ => {
            x.extra_options.insert(k.to_string(), v.to_string());
        }
    }
}

/// Esc pressed within [`REPEAT_CLOSE`] of an Esc that closed something.
const REPEAT_CLOSE: std::time::Duration = std::time::Duration::from_millis(1500);

fn repeat_close_esc(closed_at: Option<std::time::Instant>, now: std::time::Instant) -> bool {
    closed_at.is_some_and(|t| now.saturating_duration_since(t) < REPEAT_CLOSE)
}

/// Mode keys in the shell (not in full-screen apps): `esc :` opens the
/// command line (a lone Esc goes to the shell after 500 ms), Esc closes the
/// mode's windows, and the current mode's bound keys go to it. True if taken.
fn mode_key(st: &mut AppState, bytes: &[u8], parser: &ActiveParser, writer: &tabs::ActiveWriter, app_state: &Arc<Mutex<AppState>>) -> bool {
    if st.esc_pending.take().is_some() {
        if bytes == b":" {
            st.mode = Mode::Command;
            st.command_buf.clear();
            return true;
        }
        let mut w = writer.clone();
        let _ = w.write_all(b"\x1b");
    }
    if parser.get().lock().unwrap().screen.in_alt_screen() {
        return false;
    }
    if bytes == b"\x1b:" {
        st.mode = Mode::Command;
        st.command_buf.clear();
        return true;
    }
    if bytes == b"\x1b" {
        let now = std::time::Instant::now();
        if st.modes.escape() {
            st.esc_closed_at = Some(now);
            return true;
        }
        // A second Esc right after one closed the viewer is the same "close"
        // gesture; held as a lone Esc it would reach the shell glued to the
        // next key (Esc c = readline's meta-c), swallowing the "c" of "cd".
        if repeat_close_esc(st.esc_closed_at, now) {
            st.esc_closed_at = Some(now);
            return true;
        }
        st.esc_gen += 1;
        let g = st.esc_gen;
        st.esc_pending = Some(g);
        let (app_state, writer) = (app_state.clone(), writer.clone());
        std::thread::spawn(move || {
            std::thread::sleep(ESC_WAIT);
            let mut st = app_state.lock().unwrap();
            if st.esc_pending == Some(g) {
                st.esc_pending = None;
                drop(st);
                let mut w = writer;
                let _ = w.write_all(b"\x1b");
            }
        });
        return true;
    }
    match st.modes.key(bytes) {
        Some(e) => {
            apply_mode_effect(st, e, writer);
            true
        }
        None => false,
    }
}

/// How long a lone Esc waits for `:` before going to the shell.
const ESC_WAIT: std::time::Duration = std::time::Duration::from_millis(500);

fn apply_mode_effect(st: &mut AppState, e: mouser_render::modes::Effect, writer: &tabs::ActiveWriter) {
    use mouser_render::modes::Effect;
    match e {
        Effect::None => {}
        Effect::Type(s) => {
            let mut w = writer.clone();
            let _ = w.write_all(s.as_bytes());
        }
        Effect::Run(cmd) => {
            let mut w = writer.clone();
            let _ = w.write_all(&mouser_core::modes::keep_line(&cmd));
            drop(w);
            let session = writer.0.lock().unwrap().active();
            std::thread::spawn(move || {
                // Yank the line back once the shell owns the terminal again
                // (the command finished); a builtin like cd is done at once.
                std::thread::sleep(std::time::Duration::from_millis(150));
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(600);
                let mut idle = 0;
                while idle < 3 && std::time::Instant::now() < deadline {
                    match session.as_ref().and_then(|s| s.shell_in_foreground()) {
                        Some(false) => idle = 0,
                        _ => idle += 1,
                    }
                    std::thread::sleep(std::time::Duration::from_millis(50));
                }
                // To the same session, even if another tab is active now.
                if let Some(s) = session {
                    let mut sw = s.writer.clone();
                    let _ = sw.write_all(mouser_core::modes::RESTORE_LINE);
                }
            });
        }
        Effect::Open(p) => {
            let opener = if cfg!(target_os = "macos") { "open" } else { "xdg-open" };
            match std::process::Command::new(opener)
                .arg(&p)
                .stdin(std::process::Stdio::null())
                .stdout(std::process::Stdio::null())
                .stderr(std::process::Stdio::null())
                .spawn()
            {
                Ok(mut c) => {
                    std::thread::spawn(move || c.wait());
                    st.look.say(format!("Opening {}", p.display()));
                }
                Err(e) => st.look.say(format!("{opener}: {e}")),
            }
        }
        Effect::ShowKeys => st.modes.show_keys(),
        Effect::Say(m) => st.look.say(m),
        Effect::Pass => {}
    }
}

/// Put the current mode's view (docked above the prompt) into the layout.
/// Full-screen apps (alternate screen) get the whole window.
fn attach_mode_view(
    st: &mut AppState,
    layout: Option<ui::UiLayout>,
    vp: mouser_render::UiViewport,
    tab_h: i32,
    alt: bool,
    place: Option<mouser_core::remote::Place>,
) -> Option<ui::UiLayout> {
    use mouser_core::remote::Place;
    use mouser_render::frame::PADDING;
    match place {
        Some(Place::Local(d)) => st.modes.set_cwd(Some(d)),
        Some(Place::Remote { label, dir, ssh }) => {
            let listing = match (&dir, &ssh) {
                (Some(d), Some(t)) => st.remote.get(t, d, wake),
                // A remote folder but no ssh to ask: say so (never local files).
                (Some(_), None) => {
                    Some(Err("remote: no ssh side channel for this session (only ssh is supported); the folder cannot be listed".into()))
                }
                _ => None,
            };
            st.modes.set_remote(&label, dir.as_deref(), listing, ssh.as_ref());
        }
        None => {}
    }
    if alt {
        st.mode_images.0.clear();
        return layout;
    }
    let text_w = layout.as_ref().map_or(vp.width, |l| l.sidebar_rect.x);
    let area = ui::Rect { x: 0, y: PADDING + tab_h, w: text_w, h: vp.height - PADDING - tab_h };
    let view = st.modes.view(&mouser_render::modes::Context { viewport: vp, area });
    if view.dock.is_none() && view.panel.is_none() {
        st.mode_images.0.clear();
        return layout;
    }
    let mut l = layout.unwrap_or_else(|| ui::without_sidebar(vp.width, vp.height));
    let inset = view.dock.as_ref().map_or(0, |d| d.rect.h + 8);
    l.dock = view.dock.map(|panel| ui::Dock { panel, inset });
    if l.panel.is_none() {
        l.panel = view.panel;
    }
    st.mode_images = (view.images, PADDING + tab_h + inset);
    Some(l)
}

/// A mode image (window pixels) as a draw relative to the grid at `top`.
fn mode_image_draw(m: &mouser_render::modes::ModeImage, top: i32) -> mouser_render::ImageDraw {
    use mouser_render::frame::PADDING;
    mouser_render::ImageDraw {
        key: m.key,
        rgba: m.rgba.clone(),
        img_w: m.width,
        img_h: m.height,
        src: m.src,
        col: 0,
        row: 0,
        x_off: m.rect.x - PADDING,
        y_off: m.rect.y - top,
        width: m.rect.w.max(1) as u32,
        height: m.rect.h.max(1) as u32,
        z: 1,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn esc_after_closing_the_viewer_is_not_held_for_the_shell() {
        let t = std::time::Instant::now();
        assert!(repeat_close_esc(Some(t), t + std::time::Duration::from_millis(400)));
        assert!(!repeat_close_esc(Some(t), t + std::time::Duration::from_secs(2)), "later Esc: the shell's (vi mode)");
        assert!(!repeat_close_esc(None, t));
    }

    #[test]
    fn bang_forces_the_shell() {
        assert_eq!(forced_shell("!echo hi"), Some("echo hi"));
        assert_eq!(forced_shell("! theme dark"), Some("theme dark"));
        assert_eq!(forced_shell("!"), None);
        assert_eq!(forced_shell("theme dark"), None);
    }

    #[test]
    fn sidebar_restores_saved_state_env_forces_open() {
        use mouser_core::storage::Preferences;
        let fresh = Preferences::default();
        let start = |p: &Preferences, e| {
            let s = startup_sidebar(p, e);
            (s.open, s.collapsed, s.forced)
        };
        // Fresh install: collapsed strip.
        assert_eq!(start(&fresh, None), (true, true, false));
        assert_eq!(start(&fresh, Some("0")), (true, true, false));
        // Last saved state wins.
        let expanded = Preferences { sidebar_open: Some(true), sidebar_collapsed: Some(false), ..Default::default() };
        assert_eq!(start(&expanded, None), (true, false, false));
        let hidden = Preferences { sidebar_open: Some(false), sidebar_collapsed: Some(true), ..Default::default() };
        assert_eq!(start(&hidden, Some("")), (false, true, false));
        // The env var forces it open for the run, flagged so it isn't saved.
        assert_eq!(start(&hidden, Some("1")), (true, false, true));
        assert_eq!(start(&fresh, Some("yes")), (true, false, true));
    }

    fn noop() {}

    fn cmd_tab(argv: &[&str]) -> Arc<tabs::Session> {
        let argv = argv.iter().map(|a| a.to_string()).collect();
        tabs::Session::new("t", tabs::Launch::Command(argv), Size { cols: 40, rows: 5 }, ScrollbackConfig::default())
    }

    #[cfg(unix)]
    fn wait_until(mut f: impl FnMut() -> bool) {
        let t = std::time::Instant::now();
        while !f() {
            assert!(t.elapsed() < std::time::Duration::from_secs(5), "timed out");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
    }

    fn cli(args: &[&str]) -> Cli {
        Cli::try_parse_from(std::iter::once("mouser").chain(args.iter().copied())).unwrap()
    }

    #[test]
    fn cli_definition_is_valid() {
        use clap::CommandFactory;
        Cli::command().debug_assert();
    }

    #[test]
    fn cli_exec_forms() {
        let c = cli(&["-e", "btop"]);
        assert_eq!(c.exec, ["btop"]);
        let c = cli(&["-e", "nvim", "-u", "NONE", "--clean"]);
        assert_eq!(c.exec, ["nvim", "-u", "NONE", "--clean"]);
        let c = cli(&["--working-directory", "/tmp", "--title", "T", "--", "ls", "-la"]);
        assert_eq!(c.trailing, ["ls", "-la"]);
        assert_eq!(c.working_directory.as_deref(), Some(std::path::Path::new("/tmp")));
        assert_eq!(c.title.as_deref(), Some("T"));
        let c = cli(&["--dir", "/tmp", "--app-id", "btop", "--hold", "-e", "true"]);
        assert!(c.hold);
        assert_eq!(c.class.as_deref(), Some("btop"));
        assert!(cli(&["show", "x.png"]).command.is_some());
        assert!(matches!(
            cli(&["engine", "unskip", "--all"]).command,
            Some(Command::Engine { action: PluginCommand::Unskip { name: None, all: true } })
        ));
        assert!(matches!(
            cli(&["engine", "restore", "skia"]).command,
            Some(Command::Engine { action: PluginCommand::Unskip { name: Some(_), all: false } })
        ));
        // P6: `mouser engine X` is `mouser plugin X --kind engine`.
        let pa = |a: &[&str]| plugin_action(cli(a).command.as_ref().unwrap()).map(|(a, k, _)| (a, k));
        for (e, p) in [
            (vec!["engine", "list"], vec!["plugin", "list", "--kind", "engine"]),
            (vec!["engine", "use", "skia"], vec!["plugin", "--kind", "engine", "use", "skia"]),
            (vec!["engine", "priority", "skia", "mouser"], vec!["plugin", "priority", "skia", "mouser", "--kind", "engine"]),
            (vec!["engine", "disable", "skia"], vec!["plugin", "disable", "skia", "--kind", "engine"]),
            (vec!["engine", "move", "skia", "up"], vec!["plugin", "move", "skia", "up", "--kind", "engine"]),
            (vec!["engine", "unskip", "--all"], vec!["plugin", "unskip", "--all", "--kind", "engine"]),
        ] {
            assert_eq!(pa(&e), pa(&p), "{e:?}");
        }
        assert_eq!(pa(&["plugin", "list"]).unwrap().1, None);
        assert!(Cli::try_parse_from(["mouser", "engine", "unskip"]).is_err());
        assert!(Cli::try_parse_from(["mouser", "--version"]).is_err_and(|e| e.kind() == clap::error::ErrorKind::DisplayVersion));
    }

    #[test]
    fn first_tab_from_cli() {
        let f = first_tab(&cli(&[]));
        assert_eq!((f.name.as_str(), &f.launch, f.cwd), ("shell", &tabs::Launch::Shell, None));
        let tmp = std::env::temp_dir();
        let f = first_tab(&cli(&["--dir", tmp.to_str().unwrap(), "-e", "/usr/bin/btop", "-x"]));
        assert_eq!(f.name, "btop");
        assert_eq!(f.launch, tabs::Launch::Exec { argv: vec!["/usr/bin/btop".into(), "-x".into()], hold: false });
        assert_eq!(f.cwd.as_deref(), Some(tmp.as_path()));
        let f = first_tab(&cli(&["--title", "Mon", "--hold", "--dir", "/nonexistent-dir-xyz", "--", "htop"]));
        assert_eq!(f.name, "Mon");
        assert_eq!(f.launch, tabs::Launch::Exec { argv: vec!["htop".into()], hold: true });
        assert_eq!(f.cwd, None);
    }

    #[cfg(unix)]
    #[test]
    fn exec_tab_runs_in_working_directory() {
        let d = std::env::temp_dir().join(format!("mouser-cwd-{}", std::process::id()));
        std::fs::create_dir_all(&d).unwrap();
        let argv = vec!["sh".into(), "-c".into(), "pwd; sleep 0.3".into()];
        let s = tabs::Session::new_in(
            "t",
            tabs::Launch::Exec { argv, hold: false },
            Some(d.clone()),
            Size { cols: 80, rows: 5 },
            ScrollbackConfig::default(),
        );
        s.start(80, 5, 0, 0, noop);
        let want = d.canonicalize().unwrap().display().to_string();
        wait_until(|| s.parser.lock().unwrap().screen.visible_lines().iter().any(|l| l.trim_end() == want));
        wait_until(|| s.closed.load(std::sync::atomic::Ordering::SeqCst));
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn exit_policy() {
        assert!(tabs::closes_on_exit(&tabs::Launch::Exec { argv: vec![], hold: false }, 1));
        assert!(!tabs::closes_on_exit(&tabs::Launch::Exec { argv: vec![], hold: true }, 0));
        assert!(tabs::closes_on_exit(&tabs::Launch::Shell, 1));
        assert!(tabs::closes_on_exit(&tabs::Launch::Command(vec![]), 0));
        assert!(!tabs::closes_on_exit(&tabs::Launch::Command(vec![]), 255));
        assert_eq!(tabs::exit_banner(3), "\r\n\x1b[0m[process exited 3] press any key to close");
    }

    #[cfg(unix)]
    #[test]
    fn command_tabs_close_or_show_their_exit_code() {
        let mut t = tabs::Tabs::default();
        t.resize(40, 5, 360, 90, noop);
        t.open(cmd_tab(&["/bin/sh", "-c", "echo hi; exit 0"]), noop);
        let bad = cmd_tab(&["/bin/sh", "-c", "echo oops >&2; exit 3"]);
        t.open(bad.clone(), noop);
        assert_eq!(t.active, 1);
        wait_until(|| t.list[0].closed.load(std::sync::atomic::Ordering::SeqCst) && bad.exited.lock().unwrap().is_some());
        assert!(!t.reap());
        assert_eq!(t.list.len(), 1);
        assert_eq!(t.active, 0);
        let screen = bad.parser.lock().unwrap().screen.visible_lines().join("\n");
        assert!(screen.contains("oops") && screen.contains("[process exited 3]"), "{screen}");
        t.close(0);
        assert!(t.reap());
    }

    #[test]
    fn tab_selection_cycles_and_survives_closes() {
        let mut t = tabs::Tabs::default();
        for _ in 0..4 {
            t.open(cmd_tab(&["/bin/true"]), noop); // size unknown: not started
        }
        assert_eq!(t.active, 3);
        t.cycle(1);
        assert_eq!(t.active, 0);
        t.cycle(-1);
        assert_eq!(t.active, 3);
        t.select(1);
        t.close(3);
        assert_eq!((t.list.len(), t.active), (3, 1));
        t.close(1);
        assert_eq!((t.list.len(), t.active), (2, 0));
        assert_eq!(t.bar().titles.len(), 2);
    }
}
