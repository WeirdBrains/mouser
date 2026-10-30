use anyhow::Result;
use clap::Parser;
use crossbeam_channel::unbounded;
use mouser_core::compression::{CompressionAlgo, CompressionConfig};
use mouser_core::connections::{
    Connection, ConnectionKind, SshConnection, TunnelConnection, TunnelSpec, TunnelType, WaypipeConnection, X11Connection, delete_connection,
    list_connections, load_connection, save_connection,
};
use mouser_core::parser::TerminalParser;
use mouser_core::scrollback::{Scrollback, ScrollbackConfig};
use mouser_core::types::Size;
use mouser_pty::spawn_shell;
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
    pending_delete: Option<String>,
    pending_edit: Option<Connection>,
}
use mouser_render::ui;
use mouser_render::{RenderHooks, Renderer, run as run_renderer};
use serde::Deserialize;
use std::fs;
use std::io::{Read, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

#[derive(Parser, Debug)]
#[command(name = "mouser")]
#[command(about = "Mouser terminal emulator", long_about = None)]
struct Cli {
    /// Path to TOML config
    #[arg(long)]
    config: Option<PathBuf>,
    /// Override config dir (tests/manual runs)
    #[arg(long = "config-dir")]
    config_dir: Option<PathBuf>,
}

#[derive(Debug, Deserialize, Clone)]
struct UiConfig {
    font_family: Option<String>,
    font_size: Option<f32>,
    theme: Option<String>,
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
    colors: Option<ColorsConfig>,
    scrollback: Option<ScrollbackToml>,
    compression: Option<CompressionToml>,
}

impl Default for AppConfig {
    fn default() -> Self {
        Self {
            ui: Some(UiConfig { font_family: None, font_size: Some(16.0), theme: Some("dark".into()) }),
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

fn main() -> Result<()> {
    env_logger::init();
    let cli = Cli::parse();
    let app_cfg = load_config(&cli)?;
    // Ensure built-in themes exist on disk
    let _ = mouser_core::themes::write_builtins_if_absent();
    let sb_cfg = make_scrollback_config(&app_cfg);
    let scrollback = Arc::new(Scrollback::new(sb_cfg));

    // The shell is spawned on the first resize, once the real grid size is
    // known, so its first prompt is drawn for the right width.
    let master: Arc<Mutex<Option<Box<dyn mouser_pty::MasterPty + Send>>>> = Arc::new(Mutex::new(None));
    let child: Arc<Mutex<Option<Box<dyn mouser_pty::Child + Send>>>> = Arc::new(Mutex::new(None));
    let shared_writer = SharedWriter(Arc::new(Mutex::new(None)));
    let (tx, rx) = unbounded::<Vec<u8>>();

    let size = Size { cols: 100, rows: 30 };
    let parser = TerminalParser::new(size, scrollback.clone(), shared_writer.clone());
    let parser = Arc::new(Mutex::new(parser));

    // App UI state
    let app_state = Arc::new(Mutex::new(AppState {
        mode: Mode::Shell,
        command_buf: String::new(),
        sidebar_open: false,
        pending_delete: None,
        pending_edit: None,
    }));

    // Hooks for renderer
    let fetch = {
        let parser = parser.clone();
        let sb = scrollback.clone();
        Arc::new(move |scrolled_back: usize, rows: usize| -> (Vec<String>, usize) {
            let screen_lines = {
                let p = parser.lock().unwrap();
                if p.screen.in_alt_screen() {
                    return (p.screen.visible_lines(), 0);
                }
                p.screen.visible_lines()
            };
            let sb_total = sb.total_lines();
            let back = scrolled_back.min(sb_total);
            if back == 0 {
                return (screen_lines, 0);
            }
            // Window of `rows` lines ending `back` lines above the live screen.
            let total = sb_total + screen_lines.len();
            let end = total - back;
            let start = end.saturating_sub(rows);
            let mut out = Vec::with_capacity(rows);
            if start < sb_total {
                out.extend(sb.get_lines(start, end.min(sb_total) - start));
            }
            if end > sb_total {
                out.extend(screen_lines.into_iter().take(end - sb_total));
            }
            sb.prefetch_around(start, end);
            (out, back)
        })
    };
    let get_cursor = {
        let parser = parser.clone();
        Arc::new(move || {
            let p = parser.lock().unwrap();
            p.screen.cursor_visible.then_some((p.screen.cursor.x, p.screen.cursor.y))
        })
    };
    let on_key = {
        let writer = shared_writer.clone();
        let app_state = app_state.clone();
        Arc::new(move |bytes: Vec<u8>| {
            let mut state = app_state.lock().unwrap();
            match state.mode {
                Mode::Shell => {
                    if bytes == b"\x1b" && (state.pending_edit.is_some() || state.pending_delete.is_some()) {
                        state.pending_edit = None;
                        state.pending_delete = None;
                    } else if bytes == b"\x1b\x1b" {
                        // Alt+Esc toggles the connections sidebar; plain Esc goes to the shell.
                        state.sidebar_open = !state.sidebar_open;
                    } else {
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
                        drop(state);
                        handle_command(&cmd, &writer, &app_state);
                    } else if let Ok(s) = std::str::from_utf8(&bytes) {
                        state.command_buf.push_str(s);
                    }
                }
                Mode::Config => {
                    state.mode = Mode::Shell;
                }
            }
        })
    };

    std::thread::spawn({
        let parser = parser.clone();
        let sb = scrollback.clone();
        move || {
            while let Ok(bytes) = rx.recv() {
                parser.lock().unwrap().feed(&bytes);
                sb.maybe_compress_idle();
            }
        }
    });

    // Theme colours, re-read from preferences at most once a second.
    let get_colors = {
        let cache: Mutex<Option<(std::time::Instant, (u32, u32))>> = Mutex::new(None);
        Arc::new(move || {
            let mut c = cache.lock().unwrap();
            match *c {
                Some((at, colors)) if at.elapsed() < std::time::Duration::from_secs(1) => colors,
                _ => {
                    let prefs = mouser_core::storage::load_preferences();
                    let colors = mouser_core::themes::resolve_active_colors(&prefs);
                    *c = Some((std::time::Instant::now(), colors));
                    colors
                }
            }
        })
    };
    let on_resize = {
        let parser = parser.clone();
        let master = master.clone();
        let child = child.clone();
        let writer = shared_writer.clone();
        Arc::new(move |cols: u16, rows: u16| {
            parser.lock().unwrap().resize(Size { cols: cols as usize, rows: rows as usize });
            let mut m = master.lock().unwrap();
            if let Some(m) = m.as_mut() {
                let _ = mouser_pty::set_master_winsize(&mut **m, cols, rows);
                return;
            }
            match spawn_shell(cols, rows) {
                Ok(handles) => {
                    let tx = tx.clone();
                    let mut reader = handles.reader;
                    std::thread::spawn(move || {
                        let mut buf = [0u8; 8192];
                        while let Ok(n) = reader.read(&mut buf) {
                            if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            }
                        }
                    });
                    *writer.0.lock().unwrap() = Some(handles.writer);
                    *child.lock().unwrap() = Some(handles.child);
                    *m = Some(handles.master);
                }
                Err(e) => log::error!("failed to spawn shell: {e}"),
            }
        })
    };
    let get_ui_model = {
        let app_state = app_state.clone();
        Arc::new(move |vp: mouser_render::UiViewport| -> Option<ui::UiLayout> {
            let st = app_state.lock().unwrap();
            if !st.sidebar_open && st.pending_delete.is_none() && st.pending_edit.is_none() {
                return None;
            }
            let mut groups: std::collections::BTreeMap<ui::Category, Vec<String>> = Default::default();
            if let Ok(list) = list_connections() {
                for c in list {
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
            Some(ui::build_sidebar_layout_metrics(vp.width, vp.height, vp.cell_w, vp.cell_h, &groups, modal))
        })
    };
    let on_ui_action = {
        let app_state = app_state.clone();
        Arc::new(move |act: ui::UiAction| match act {
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
            ui::UiAction::ToggleAdvanced | ui::UiAction::None => {}
        })
    };
    let hooks = RenderHooks {
        on_key_bytes: on_key,
        on_resize,
        fetch_lines: fetch,
        get_colors,
        get_cursor: Some(get_cursor),
        get_ui_model: Some(get_ui_model),
        on_ui_action: Some(on_ui_action),
    };
    let prefs = mouser_core::storage::load_preferences();
    let mut r = Renderer::default();
    let ui_cfg = app_cfg.ui.as_ref();
    if let Some(fs) = ui_cfg.and_then(|u| u.font_size).or(prefs.font_size) {
        r.font_size = fs;
    }
    r.font_family = ui_cfg.and_then(|u| u.font_family.clone()).or(prefs.font_family);
    if std::env::var_os("MOUSER_OPEN_SIDEBAR").is_some() {
        app_state.lock().unwrap().sidebar_open = true;
    }
    run_renderer(r, hooks)?;
    Ok(())
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

fn handle_command(cmd: &str, writer: &SharedWriter, app_state: &Arc<Mutex<AppState>>) {
    let trimmed = cmd.trim();
    if trimmed == "open-sidebar" {
        app_state.lock().unwrap().sidebar_open = true;
    } else if trimmed == "close-sidebar" {
        app_state.lock().unwrap().sidebar_open = false;
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
    } else {
        let mut w = writer.clone();
        let _ = w.write_all(format!("\r\n[mouser] unknown command: {trimmed}\r\n").as_bytes());
    }
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
