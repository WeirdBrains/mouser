mod appearance_ui;
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
    let cli = Cli::parse();
    if let Some(Command::Show { files, width, height, fit: _, original, id }) = &cli.command {
        let o = mouser_app::show::Options { width: *width, height: *height, original: *original, id: *id };
        if let Err(e) = mouser_app::show::run(files, &o) {
            eprintln!("mouser show: {e:#}");
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
            mouser_render::layered_image_draws(&[(&p.kitty_gfx.gfx, kitty), (&p.fuzzball.gfx, fuzz)], &mut scaler.lock().unwrap())
        })
    };
    let on_key = {
        let writer = shared_writer.clone();
        let tabs_k = tabs.clone();
        let app_state = app_state.clone();
        let parser = parser.clone();
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
        let (tabs_ui, sb_cfg) = (tabs.clone(), sb_cfg.clone());
        Arc::new(move |act: ui::UiAction| match act.clone() {
            ui::UiAction::OpenAdd(ui::Category::Ssh) => app_state.lock().unwrap().ssh.open_add(),
            ui::UiAction::OpenEdit { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_edit(&name),
            ui::UiAction::OpenDeleteConfirm { category: ui::Category::Ssh, name } => app_state.lock().unwrap().ssh.open_delete(&name),
            ui::UiAction::Notice => app_state.lock().unwrap().ssh.open_migrate(),
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
                    st.mode == Mode::Command
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
                let Some(text) = get_clipboard() else { return };
                let mut st = app_state.lock().unwrap();
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
                let _ = w.write_all(&mouser_core::input::paste_bytes(&text, &modes));
            })
        }),
        on_select: Some({
            let parser = parser.clone();
            Arc::new(move |ev: mouser_render::SelectEvent| {
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
            let (parser, writer) = (parser.clone(), shared_writer.clone());
            Arc::new(move || {
                let Some(text) = get_primary() else { return };
                let modes = parser.get().lock().unwrap().modes;
                let mut w = writer.clone();
                let _ = w.write_all(&mouser_core::input::paste_bytes(&text, &modes));
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
        set_waker: Some(Arc::new(|w| {
            let _ = WAKER.set(w);
        })),
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
                if st.mode == Mode::Command { Some(format!(":{}", st.command_buf)) } else { st.look.status() }
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
    let result = run_renderer(r, hooks);
    // Window closed (by the user or because the shell exited): remember the sidebar.
    {
        let st = app_state.lock().unwrap();
        if !st.sidebar_forced {
            persist_sidebar_state(&st);
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
        for l in sb.get_lines(start, end.min(sb_total) - start) {
            let (text, pt) = select::scrollback_view(&l, fg);
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

fn handle_command(cmd: &str, writer: &tabs::ActiveWriter, app_state: &Arc<Mutex<AppState>>) {
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
