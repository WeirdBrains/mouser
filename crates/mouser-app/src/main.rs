mod appearance_ui;
mod ssh_ui;

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
    sidebar_collapsed: bool,
    sidebar_width: Option<i32>,
    /// MOUSER_OPEN_SIDEBAR forced it open for this run: don't save that state
    /// unless the user changes the sidebar.
    sidebar_forced: bool,
    look: appearance_ui::Appearance,
    ssh: ssh_ui::SshUi,
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
    let get_images = {
        let parser = parser.clone();
        Arc::new(move |scrolled_back: usize, rows: usize, cell: (u32, u32)| -> Vec<mouser_render::ImageDraw> {
            let mut p = parser.lock().unwrap();
            p.cell_px = cell;
            let top = if p.screen.in_alt_screen() { p.lines_scrolled } else { p.lines_scrolled - scrolled_back as i64 };
            p.graphics
                .visible(top, rows, cell.1)
                .into_iter()
                .map(|(pl, im)| mouser_render::ImageDraw {
                    key: im.key,
                    rgba: im.rgba.clone(),
                    img_w: im.width,
                    img_h: im.height,
                    src: pl.src,
                    col: pl.col as i32,
                    row: (pl.row - top) as i32,
                    x_off: pl.x_off as i32,
                    y_off: pl.y_off as i32,
                    width: pl.width,
                    height: pl.height,
                    z: pl.z,
                })
                .collect()
        })
    };
    let on_key = {
        let writer = shared_writer.clone();
        let app_state = app_state.clone();
        Arc::new(move |bytes: Vec<u8>| {
            let mut state = app_state.lock().unwrap();
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
                    } else if bytes == b"\x1b[21~" {
                        // F10: Mouser command line
                        state.mode = Mode::Command;
                        state.command_buf.clear();
                    } else if bytes == b"\x1b\x1b" {
                        // Alt+Esc toggles the connections sidebar; plain Esc goes to the shell.
                        state.sidebar_open = !state.sidebar_open;
                        persist_sidebar(&mut state);
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

    std::thread::spawn({
        let parser = parser.clone();
        let sb = scrollback.clone();
        move || {
            while let Ok(bytes) = rx.recv() {
                parser.lock().unwrap().feed(&bytes);
                sb.maybe_compress_idle();
                if rx.is_empty() {
                    wake();
                }
            }
        }
    });

    let get_colors = {
        let app_state = app_state.clone();
        Arc::new(move || app_state.lock().unwrap().look.colors())
    };
    let get_font = {
        let app_state = app_state.clone();
        Arc::new(move || app_state.lock().unwrap().look.font())
    };
    let shell_exited = Arc::new(std::sync::atomic::AtomicBool::new(false));
    let on_resize = {
        let shell_exited = shell_exited.clone();
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
                    let shell_exited = shell_exited.clone();
                    std::thread::spawn(move || {
                        pump_pty(&mut reader, &tx);
                        // EOF / EIO: the shell exited (Ctrl+D, `exit`, ...).
                        shell_exited.store(true, std::sync::atomic::Ordering::SeqCst);
                        wake();
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
            let mut st = app_state.lock().unwrap();
            let panel = st.ssh.panel_view(vp);
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
            layout.panel = panel;
            Some(layout)
        })
    };
    let on_ui_action = {
        let app_state = app_state.clone();
        let connect_writer = shared_writer.clone();
        Arc::new(move |act: ui::UiAction| match act.clone() {
            ui::UiAction::OpenAdd(ui::Category::Ssh) => app_state.lock().unwrap().ssh.open_add(),
            ui::UiAction::OpenEdit { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_edit(&name),
            ui::UiAction::OpenDeleteConfirm { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_delete(&name),
            ui::UiAction::Notice => app_state.lock().unwrap().ssh.open_migrate(),
            ui::UiAction::Connect { category, name } => {
                let st = app_state.lock().unwrap();
                let cmd = if category == ui::Category::Ssh { Some(st.ssh.connect_command(&name)) } else { None };
                drop(st);
                if let Some(cmd) = cmd {
                    let mut w = connect_writer.clone();
                    let _ = w.write_all(cmd.as_bytes());
                }
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
                if st.ssh.panel.is_some() && !matches!(act, ui::UiAction::FontSize(_)) {
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
        })
    };
    discover_font_families();
    let hooks = RenderHooks {
        on_key_bytes: on_key.clone(),
        get_images: Some(get_images),
        on_key_event: Some({
            let (writer, app_state, parser, on_key) = (shared_writer.clone(), app_state.clone(), parser.clone(), on_key.clone());
            Arc::new(move |legacy: Option<Vec<u8>>, input: mouser_core::kitty::KeyInput| {
                use mouser_core::kitty::{Encoded, encode_key};
                let ui_owns = {
                    let st = app_state.lock().unwrap();
                    st.mode == Mode::Command
                        || st.ssh.panel.is_some()
                        || st.look.panel.is_some()
                        || st.pending_edit.is_some()
                        || st.pending_delete.is_some()
                };
                // Mouser's own hotkeys (F10, Alt+Esc) keep working in any mode.
                let hotkey = legacy.as_deref().is_some_and(|b| b == b"\x1b[21~" || b == b"\x1b\x1b");
                let flags = {
                    let p = parser.lock().unwrap();
                    p.kitty_keyboard.current(p.screen.in_alt_screen())
                };
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
        fetch_lines: fetch,
        get_colors,
        get_font: Some(get_font),
        get_cursor: Some(get_cursor),
        get_ui_model: Some(get_ui_model),
        on_ui_action: Some(on_ui_action),
        set_waker: Some(Arc::new(|w| {
            let _ = WAKER.set(w);
        })),
        should_exit: Some({
            let shell_exited = shell_exited.clone();
            Arc::new(move || shell_exited.load(std::sync::atomic::Ordering::SeqCst))
        }),
        get_status: Some({
            let app_state = app_state.clone();
            Arc::new(move || {
                let st = app_state.lock().unwrap();
                if st.mode == Mode::Command { Some(format!(":{}", st.command_buf)) } else { st.look.status() }
            })
        }),
    };
    let r = Renderer { font_size: font_size0, font_family: font_family0, ..Renderer::default() };
    {
        // Restore the last saved sidebar state (fresh install: collapsed strip);
        // MOUSER_OPEN_SIDEBAR forces it open for this run only.
        let mut st = app_state.lock().unwrap();
        let s = startup_sidebar(&prefs0, std::env::var("MOUSER_OPEN_SIDEBAR").ok().as_deref());
        (st.sidebar_open, st.sidebar_collapsed, st.sidebar_forced) = (s.open, s.collapsed, s.forced);
    }
    let result = run_renderer(r, hooks);
    // Window closed (by the user or because the shell exited): remember the sidebar.
    {
        let st = app_state.lock().unwrap();
        if !st.sidebar_forced {
            persist_sidebar_state(&st);
        }
    }
    result?;
    // Reap the shell if it is still around (window closed by the user).
    if let Some(mut c) = child.lock().unwrap().take()
        && c.try_wait().ok().flatten().is_none()
    {
        let _ = c.kill();
        let _ = c.wait();
    }
    Ok(())
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

/// Forward PTY output to `tx` until EOF or a read error (EIO on Linux once the
/// shell has exited and the last slave fd closed).
fn pump_pty(reader: &mut dyn Read, tx: &crossbeam_channel::Sender<Vec<u8>>) {
    let mut buf = [0u8; 8192];
    loop {
        match reader.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => {
                if tx.send(buf[..n].to_vec()).is_err() {
                    break;
                }
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(_) => break,
        }
    }
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

fn handle_command(cmd: &str, writer: &SharedWriter, app_state: &Arc<Mutex<AppState>>) {
    let trimmed = cmd.trim();
    if trimmed == "open-sidebar" || trimmed == ":open-sidebar" {
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

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn pump_stops_at_eof_and_on_error() {
        let (tx, rx) = unbounded();
        let mut data: &[u8] = b"bye\r\n";
        pump_pty(&mut data, &tx);
        assert_eq!(rx.try_recv().unwrap(), b"bye\r\n");
        struct Eio;
        impl Read for Eio {
            fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
                Err(std::io::Error::from_raw_os_error(5))
            }
        }
        pump_pty(&mut Eio, &tx); // returns instead of spinning
        assert!(rx.try_recv().is_err());
    }

    #[cfg(unix)]
    #[test]
    fn shell_exit_ends_the_pty_stream() {
        // A real PTY session: Ctrl+D (0x04) at an empty `sh` prompt ends it.
        let unique = std::env::temp_dir().join(format!("mouser-exit-{}", std::process::id()));
        std::fs::create_dir_all(&unique).unwrap();
        let mut cmd = portable_pty_cmd();
        cmd.env("HOME", &unique);
        let pty = mouser_pty::portable_pty::native_pty_system();
        let pair = pty.openpty(mouser_pty::portable_pty::PtySize { rows: 24, cols: 80, pixel_width: 0, pixel_height: 0 }).unwrap();
        let mut child = pair.slave.spawn_command(cmd).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let mut writer = pair.master.take_writer().unwrap();
        let (tx, rx) = unbounded();
        let done = std::thread::spawn(move || pump_pty(&mut reader, &tx));
        std::thread::sleep(std::time::Duration::from_millis(300));
        writer.write_all(&[0x04]).unwrap();
        let status = child.wait().unwrap();
        assert!(status.success());
        drop(writer);
        drop(pair.master);
        done.join().unwrap();
        while rx.try_recv().is_ok() {}
        let _ = std::fs::remove_dir_all(unique);
    }

    #[cfg(unix)]
    fn portable_pty_cmd() -> mouser_pty::portable_pty::CommandBuilder {
        let mut c = mouser_pty::portable_pty::CommandBuilder::new("/bin/sh");
        c.env("PS1", "$ ");
        c
    }
}
