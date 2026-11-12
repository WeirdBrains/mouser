//! Decoration plugins (docs/plugins.md §Decoration plugins, docs/tabs.md).
//!
//! A decoration draws window chrome (tab bar, title strip, borders) and may
//! ask the host for PTYs in that window. Every enabled decoration runs.
//! [`compose`] stacks their chrome and [`arbitrate`] merges PTY ops. The
//! built-in `tabs` plugin is one decoration. Another program can speak the
//! same newline-delimited JSON (`transport = "decoration"`).
//!
//! With no decoration enabled, the window keeps its built-in sessions: one
//! PTY at start, more only through the built-in tab keys, and the strip is
//! drawn only when two or more tabs are open.

use crate::plugins::API;
use anyhow::{Context, Result, bail};
use serde::{Deserialize, Serialize};
use std::io::{BufRead, BufReader, Write};
use std::path::Path;
use std::process::{Child, ChildStdin, Stdio};
use std::sync::mpsc;
use std::time::Duration;

/// Built-in decoration. Off until `mouser plugin enable tabs`.
pub const TABS_PLUGIN: &str = "tabs";

/// Pixels added under one row of tab titles. Matches the built-in strip.
pub fn bar_height(cell_h: i32) -> i32 {
    cell_h.max(1) + 10
}

/// `[tabs]` in preferences.toml.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabsConfig {
    /// Login shell for a new tab. Unset: `$SHELL`, else `/bin/sh`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// One PTY per entry, each with its own shell or command.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tab: Vec<TabSpec>,
}

/// One `[[tabs.tab]]` entry.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSpec {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Login shell program. Ignored when `command` is set.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub shell: Option<String>,
    /// argv run instead of a login shell.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub command: Vec<String>,
}

/// `[tabs]` from preferences, or the defaults.
pub fn tabs_config() -> TabsConfig {
    crate::storage::load_preferences().tabs.unwrap_or_default()
}

/// How the host started a PTY. Exit policy matches `mouser-app`'s sessions.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum PtyKind {
    Shell,
    Command,
    Exec,
    Handoff,
}

/// One PTY the decoration can see. The host is the source of truth.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PtyInfo {
    pub id: u64,
    pub title: String,
    pub shell: String,
    pub command: Vec<String>,
    pub kind: PtyKind,
    pub hold: bool,
    pub exited: Option<u32>,
}

/// Shells and handoffs always close. An exec closes unless it was held.
/// A command closes only when it exits 0.
pub fn closes_on_exit(kind: PtyKind, hold: bool, code: u32) -> bool {
    match kind {
        PtyKind::Shell | PtyKind::Handoff => true,
        PtyKind::Exec => !hold,
        PtyKind::Command => code == 0,
    }
}

/// What the host passes in on every call.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DecorCtx {
    pub width: i32,
    pub height: i32,
    pub cell_w: i32,
    pub cell_h: i32,
    pub bg: u32,
    pub fg: u32,
    pub accent: u32,
    pub ptys: Vec<PtyInfo>,
    pub focused: Option<u64>,
}

impl Default for DecorCtx {
    fn default() -> Self {
        Self { width: 800, height: 600, cell_w: 8, cell_h: 16, bg: 0x101418, fg: 0xd0d0d0, accent: 0x2fd8e5, ptys: Vec::new(), focused: None }
    }
}

/// Chrome the host reserves and paints. Coordinates are window pixels.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Chrome {
    pub top: i32,
    pub bottom: i32,
    pub left: i32,
    pub right: i32,
    pub ops: Vec<DrawOp>,
    pub hits: Vec<Hit>,
}

impl Chrome {
    pub fn active(&self) -> bool {
        self.top > 0 || self.bottom > 0 || self.left > 0 || self.right > 0 || !self.ops.is_empty()
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum DrawOp {
    Fill { x: i32, y: i32, w: i32, h: i32, color: u32 },
    Text { x: i32, y: i32, text: String, color: u32 },
}

/// A click target. `id` is handed back as [`DecorEvent::Hit`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Hit {
    pub x: i32,
    pub y: i32,
    pub w: i32,
    pub h: i32,
    pub id: String,
    /// Decoration that owns this target. Empty before [`compose`] fills it in.
    #[serde(default, skip_serializing_if = "String::is_empty")]
    pub owner: String,
}

/// Topmost hit containing (`x`, `y`). Later hits win, so [`compose`] paints
/// higher-priority decorations last.
pub fn hit_test(chrome: &Chrome, x: i32, y: i32) -> Option<Hit> {
    chrome.hits.iter().rev().find(|h| x >= h.x && y >= h.y && x < h.x.saturating_add(h.w) && y < h.y.saturating_add(h.h)).cloned()
}

const HIT_SEP: char = '\u{1f}';

/// `owner` and the decoration's own hit id, for the render hook.
pub fn pack_hit(owner: &str, id: &str) -> String {
    format!("{owner}{HIT_SEP}{id}")
}

/// Inverse of [`pack_hit`]. A string with no separator has an empty owner.
pub fn unpack_hit(packed: &str) -> (String, String) {
    match packed.split_once(HIT_SEP) {
        Some((owner, id)) => (owner.to_string(), id.to_string()),
        None => (String::new(), packed.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum DecorEvent {
    /// Open a tab. `shell` overrides `[tabs] shell` for this tab only.
    NewTab {
        shell: Option<String>,
    },
    CloseTab {
        id: u64,
    },
    Focus {
        id: u64,
    },
    Cycle {
        delta: i32,
    },
    Hit {
        id: String,
    },
    /// A key while the focused PTY has already exited.
    KeyOnExited,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "kebab-case")]
pub enum PtyOp {
    Spawn { shell: Option<String>, command: Vec<String>, title: String },
    Close { id: u64 },
    Focus { id: u64 },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "kebab-case")]
pub enum PtyNote {
    Exited {
        id: u64,
        code: u32,
    },
    Title {
        id: u64,
        title: String,
    },
    /// The focused PTY changed. Informational: decorations should not fight it.
    Focused {
        id: u64,
    },
    /// A chrome hit. Delivered to every decoration; only the owner also gets
    /// [`DecorEvent::Hit`].
    ChromeHit {
        owner: String,
        id: String,
    },
}

/// In-process or out-of-process decoration. Called on the session thread.
pub trait Decoration: Send {
    fn chrome(&mut self, ctx: &DecorCtx) -> Chrome;
    fn event(&mut self, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp>;
    fn note(&mut self, note: &PtyNote, ctx: &DecorCtx) -> Vec<PtyOp>;
    fn failed(&self) -> bool {
        false
    }
}

/// The built-in tabs decoration. Shell choice is read from preferences on
/// each event, so a change applies to the next tab. `pinned` replaces that
/// read (tests).
#[derive(Debug, Default)]
pub struct TabsDecoration {
    pub pinned: Option<TabsConfig>,
}

impl Decoration for TabsDecoration {
    fn chrome(&mut self, ctx: &DecorCtx) -> Chrome {
        tabs_chrome(ctx)
    }

    fn event(&mut self, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp> {
        let config = self.pinned.clone().unwrap_or_else(tabs_config);
        tabs_event(&config, ev, ctx)
    }

    fn note(&mut self, note: &PtyNote, ctx: &DecorCtx) -> Vec<PtyOp> {
        tabs_note(note, ctx)
    }
}

/// File name used as a tab title.
pub fn shell_label(shell: &Option<String>, command: &[String]) -> String {
    let raw = command.first().cloned().or_else(|| shell.clone());
    match raw {
        Some(p) if !p.is_empty() => std::path::Path::new(&p).file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or(p),
        _ => "shell".into(),
    }
}

/// What to do with the window's first PTY, and which further PTYs to open.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct StartupPlan {
    /// Replaces the default login shell when the CLI did not pass a command.
    pub first: Option<FirstTab>,
    pub extra: Vec<PtyOp>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FirstTab {
    pub title: String,
    pub shell: Option<String>,
    pub command: Vec<String>,
}

fn spec_op(t: &TabSpec) -> PtyOp {
    let command = t.command.clone();
    let shell = if command.is_empty() { t.shell.clone() } else { None };
    let title = t.title.clone().unwrap_or_else(|| shell_label(&t.shell.clone().or(shell.clone()), &command));
    PtyOp::Spawn { shell, command, title }
}

/// `cli_default_shell`: the window was going to start the login shell
/// (no `-e`, no handoff). The first `[[tabs.tab]]` replaces that shell;
/// the rest are extra PTYs. A CLI command is kept, and every `[[tabs.tab]]`
/// is an extra PTY.
pub fn startup_plan(config: &TabsConfig, cli_default_shell: bool) -> StartupPlan {
    if !cli_default_shell {
        return StartupPlan { first: None, extra: config.tab.iter().map(spec_op).collect() };
    }
    if config.tab.is_empty() {
        if config.shell.is_none() {
            return StartupPlan::default();
        }
        let shell = config.shell.clone();
        return StartupPlan { first: Some(FirstTab { title: shell_label(&shell, &[]), shell, command: Vec::new() }), extra: Vec::new() };
    }
    let first_op = spec_op(&config.tab[0]);
    let PtyOp::Spawn { shell, command, title } = first_op else { unreachable!() };
    StartupPlan { first: Some(FirstTab { title, shell, command }), extra: config.tab.iter().skip(1).map(spec_op).collect() }
}

fn mix(a: u32, b: u32, t: f32) -> u32 {
    let ch = |s: u32, d: u32| (s as f32 + (d as f32 - s as f32) * t).round() as u32;
    let (ar, ag, ab) = ((a >> 16) & 255, (a >> 8) & 255, a & 255);
    let (br, bg, bb) = ((b >> 16) & 255, (b >> 8) & 255, b & 255);
    (ch(ar, br) << 16) | (ch(ag, bg) << 8) | ch(ab, bb)
}

/// Tab strip along the top, plus a `+` button. Shown for a single tab too,
/// so the chrome is visible as soon as the plugin is on.
pub fn tabs_chrome(ctx: &DecorCtx) -> Chrome {
    let cw = ctx.cell_w.max(1);
    let h = bar_height(ctx.cell_h);
    let width = ctx.width.max(cw);
    let strip = mix(ctx.bg, ctx.fg, 0.06);
    let mut ops = vec![
        DrawOp::Fill { x: 0, y: 0, w: width, h, color: strip },
        DrawOp::Fill { x: 0, y: h - 1, w: width, h: 1, color: mix(ctx.bg, ctx.fg, 0.2) },
    ];
    let mut hits = Vec::new();
    let plus_w = cw * 3;
    let n = ctx.ptys.len();
    let tabs_w = (width - plus_w).max(cw);
    if n > 0 {
        let tw = (tabs_w / n as i32).clamp(cw * 6, cw * 28);
        for (i, pty) in ctx.ptys.iter().enumerate() {
            let x = i as i32 * tw;
            if x >= tabs_w {
                break;
            }
            let w = tw.min(tabs_w - x);
            let active = ctx.focused == Some(pty.id);
            if active {
                ops.push(DrawOp::Fill { x: x + 1, y: 2, w: w - 2, h: h - 2, color: ctx.bg });
                ops.push(DrawOp::Fill { x: x + 1, y: h - 2, w: w - 2, h: 2, color: ctx.accent });
            }
            let close_w = cw * 2;
            let close_x = x + w - close_w - 2;
            let label = if pty.exited.is_some() { format!("{} · exited", pty.title) } else { pty.title.clone() };
            let max_chars = ((close_x - x) / cw - 1).max(0) as usize;
            let text: String = label.chars().take(max_chars).collect();
            let color = if active { ctx.fg } else { mix(strip, ctx.fg, 0.65) };
            ops.push(DrawOp::Text { x: x + cw, y: (h - ctx.cell_h.max(1)) / 2, text, color });
            ops.push(DrawOp::Text { x: close_x + cw / 2, y: (h - ctx.cell_h.max(1)) / 2, text: "x".into(), color: mix(strip, ctx.fg, 0.55) });
            hits.push(Hit { x, y: 0, w: close_x - x, h, id: format!("focus:{}", pty.id), owner: String::new() });
            hits.push(Hit { x: close_x, y: 0, w: close_w, h, id: format!("close:{}", pty.id), owner: String::new() });
        }
    }
    let plus_x = width - plus_w;
    ops.push(DrawOp::Text { x: plus_x + cw, y: (h - ctx.cell_h.max(1)) / 2, text: "+".into(), color: ctx.fg });
    hits.push(Hit { x: plus_x, y: 0, w: plus_w, h, id: "new".into(), owner: String::new() });
    Chrome { top: h, bottom: 0, left: 0, right: 0, ops, hits }
}

fn spawn_shell(config: &TabsConfig, shell: Option<String>) -> PtyOp {
    let shell = shell.filter(|s| !s.is_empty()).or_else(|| config.shell.clone());
    let title = shell_label(&shell, &[]);
    PtyOp::Spawn { shell, command: Vec::new(), title }
}

fn id_known(ctx: &DecorCtx, id: u64) -> bool {
    ctx.ptys.iter().any(|p| p.id == id)
}

/// Tab keys, clicks and per-tab shell choice.
pub fn tabs_event(config: &TabsConfig, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp> {
    match ev {
        DecorEvent::NewTab { shell } => vec![spawn_shell(config, shell.clone())],
        DecorEvent::CloseTab { id } if id_known(ctx, *id) => vec![PtyOp::Close { id: *id }],
        DecorEvent::Focus { id } if id_known(ctx, *id) => vec![PtyOp::Focus { id: *id }],
        DecorEvent::Cycle { delta } => {
            let ids: Vec<u64> = ctx.ptys.iter().map(|p| p.id).collect();
            if ids.is_empty() {
                return Vec::new();
            }
            let i = ctx.focused.and_then(|id| ids.iter().position(|x| *x == id)).unwrap_or(0);
            let n = ids.len() as i32;
            let j = (i as i32 + delta).rem_euclid(n) as usize;
            vec![PtyOp::Focus { id: ids[j] }]
        }
        DecorEvent::Hit { id } if id == "new" => vec![spawn_shell(config, None)],
        DecorEvent::Hit { id } => match id.split_once(':') {
            Some(("focus", n)) => n.parse().ok().filter(|id| id_known(ctx, *id)).map(|id| vec![PtyOp::Focus { id }]).unwrap_or_default(),
            Some(("close", n)) => n.parse().ok().filter(|id| id_known(ctx, *id)).map(|id| vec![PtyOp::Close { id }]).unwrap_or_default(),
            _ => Vec::new(),
        },
        DecorEvent::KeyOnExited => {
            let Some(id) = ctx.focused else { return Vec::new() };
            let exited = ctx.ptys.iter().any(|p| p.id == id && p.exited.is_some());
            if exited { vec![PtyOp::Close { id }] } else { Vec::new() }
        }
        DecorEvent::CloseTab { .. } | DecorEvent::Focus { .. } => Vec::new(),
    }
}

fn neighbor(ctx: &DecorCtx, id: u64) -> Option<u64> {
    let i = ctx.ptys.iter().position(|p| p.id == id)?;
    if ctx.ptys.len() <= 1 {
        return None;
    }
    let j = if i + 1 < ctx.ptys.len() { i + 1 } else { i - 1 };
    Some(ctx.ptys[j].id)
}

/// A shell that exits closes its tab and, when it was focused, focuses a neighbour.
/// A non-zero command stays so the exit banner can be read.
pub fn tabs_note(note: &PtyNote, ctx: &DecorCtx) -> Vec<PtyOp> {
    match note {
        PtyNote::Title { .. } => Vec::new(),
        PtyNote::Exited { id, code } => {
            let Some(info) = ctx.ptys.iter().find(|p| p.id == *id) else { return Vec::new() };
            if !closes_on_exit(info.kind, info.hold, *code) {
                return Vec::new();
            }
            let mut ops = Vec::new();
            if ctx.focused == Some(*id)
                && let Some(next) = neighbor(ctx, *id)
            {
                ops.push(PtyOp::Focus { id: next });
            }
            ops.push(PtyOp::Close { id: *id });
            ops
        }
        PtyNote::Focused { .. } | PtyNote::ChromeHit { .. } => Vec::new(),
    }
}

/// One JSON object per line, both ways. `op` is `hello`, `chrome`, `event` or `note`.
#[derive(Debug, Serialize)]
struct Request<'a> {
    api: u32,
    op: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    ctx: Option<&'a DecorCtx>,
    #[serde(skip_serializing_if = "Option::is_none")]
    event: Option<&'a DecorEvent>,
    #[serde(skip_serializing_if = "Option::is_none")]
    note: Option<&'a PtyNote>,
}

#[derive(Debug, Default, Deserialize)]
struct Reply {
    #[serde(default)]
    chrome: Chrome,
    #[serde(default)]
    ops: Vec<PtyOp>,
}

/// A `transport = "decoration"` program. The environment is scrubbed the same
/// way as a command plugin. One reply is expected per request; a timeout or
/// a bad reply stops the process.
pub struct DecorationProc {
    child: Child,
    stdin: Option<ChildStdin>,
    lines: Option<mpsc::Receiver<std::io::Result<String>>>,
    failed: bool,
}

impl DecorationProc {
    pub fn spawn(argv: &[String], dir: &Path) -> Result<Self> {
        let (prog, args) = argv.split_first().context("decoration run.command is empty")?;
        let mut c = crate::process::command(prog);
        c.args(args).current_dir(dir).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::null());
        c.env_clear();
        for k in ["PATH", "HOME", "LANG", "SystemRoot", "TEMP", "TMP", "USERPROFILE"] {
            if let Some(v) = std::env::var_os(k) {
                c.env(k, v);
            }
        }
        c.env("MOUSER_PLUGIN_API", API.to_string());
        let mut child = c.spawn().with_context(|| format!("starting {prog}"))?;
        let stdin = child.stdin.take().context("stdin")?;
        let stdout = child.stdout.take().context("stdout")?;
        let (tx, rx) = mpsc::channel();
        std::thread::spawn(move || {
            let mut r = BufReader::new(stdout);
            loop {
                let mut line = String::new();
                match r.read_line(&mut line) {
                    Ok(0) => break,
                    Ok(_) => {
                        if tx.send(Ok(line)).is_err() {
                            break;
                        }
                    }
                    Err(e) => {
                        let _ = tx.send(Err(e));
                        break;
                    }
                }
            }
        });
        let mut proc = Self { child, stdin: Some(stdin), lines: Some(rx), failed: false };
        // Handshake: the program must answer once so a dead command fails at start.
        proc.rpc(&Request { api: API, op: "hello", ctx: None, event: None, note: None })?;
        Ok(proc)
    }

    fn rpc(&mut self, req: &Request) -> Result<Reply> {
        if self.failed {
            bail!("decoration stopped");
        }
        let mut body = serde_json::to_vec(req)?;
        body.push(b'\n');
        let Some(stdin) = self.stdin.as_mut() else { bail!("decoration stdin closed") };
        if stdin.write_all(&body).and_then(|()| stdin.flush()).is_err() {
            self.fail();
            bail!("decoration stopped writing");
        }
        let Some(rx) = self.lines.as_ref() else { bail!("decoration stopped") };
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(Ok(line)) => serde_json::from_str(line.trim()).context("decoration reply was not JSON"),
            Ok(Err(e)) => {
                self.fail();
                Err(e).context("reading decoration")
            }
            Err(_) => {
                self.fail();
                bail!("decoration did not answer within 500ms")
            }
        }
    }

    fn fail(&mut self) {
        self.failed = true;
        let _ = self.child.kill();
        let _ = self.child.wait();
        self.stdin.take();
        self.lines.take();
    }
}

impl Drop for DecorationProc {
    fn drop(&mut self) {
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Decoration for DecorationProc {
    fn chrome(&mut self, ctx: &DecorCtx) -> Chrome {
        match self.rpc(&Request { api: API, op: "chrome", ctx: Some(ctx), event: None, note: None }) {
            Ok(r) => r.chrome,
            Err(e) => {
                log::warn!("decoration chrome: {e:#}");
                self.failed = true;
                Chrome::default()
            }
        }
    }

    fn event(&mut self, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp> {
        match self.rpc(&Request { api: API, op: "event", ctx: Some(ctx), event: Some(ev), note: None }) {
            Ok(r) => r.ops,
            Err(e) => {
                log::warn!("decoration event: {e:#}");
                self.failed = true;
                Vec::new()
            }
        }
    }

    fn note(&mut self, note: &PtyNote, ctx: &DecorCtx) -> Vec<PtyOp> {
        match self.rpc(&Request { api: API, op: "note", ctx: Some(ctx), event: None, note: Some(note) }) {
            Ok(r) => r.ops,
            Err(e) => {
                log::warn!("decoration note: {e:#}");
                self.failed = true;
                Vec::new()
            }
        }
    }

    fn failed(&self) -> bool {
        self.failed
    }
}

/// One decoration's contribution, highest priority first in [`compose`] and
/// [`arbitrate`]. Priority is the plugin registry order (`providers`).
#[derive(Clone, Copy, PartialEq, Eq)]
enum Edge {
    Top,
    Bottom,
    Left,
    Right,
}

fn op_origin(op: &DrawOp) -> (i32, i32, i32, i32) {
    match op {
        DrawOp::Fill { x, y, w, h, .. } => (*x, *y, *w, *h),
        DrawOp::Text { x, y, .. } => (*x, *y, 1, 1),
    }
}

fn intersect(a: (i32, i32, i32, i32), b: (i32, i32, i32, i32)) -> Option<(i32, i32, i32, i32)> {
    let (ax, ay, aw, ah) = a;
    let (bx, by, bw, bh) = b;
    if aw <= 0 || ah <= 0 || bw <= 0 || bh <= 0 {
        return None;
    }
    let x0 = ax.max(bx);
    let y0 = ay.max(by);
    let x1 = ax.saturating_add(aw).min(bx.saturating_add(bw));
    let y1 = ay.saturating_add(ah).min(by.saturating_add(bh));
    (x1 > x0 && y1 > y0).then_some((x0, y0, x1 - x0, y1 - y0))
}

fn classify(chrome: &Chrome, width: i32, height: i32, x: i32, y: i32) -> Option<Edge> {
    if chrome.top > 0 && y < chrome.top {
        Some(Edge::Top)
    } else if chrome.bottom > 0 && y >= height.saturating_sub(chrome.bottom) {
        Some(Edge::Bottom)
    } else if chrome.left > 0 && x < chrome.left {
        Some(Edge::Left)
    } else if chrome.right > 0 && x >= width.saturating_sub(chrome.right) {
        Some(Edge::Right)
    } else {
        None
    }
}

struct Shift {
    dx: i32,
    dy: i32,
    band: (i32, i32, i32, i32),
}

/// Stack every decoration's chrome into disjoint bands.
///
/// `slots` is highest priority first (registry order). Each positive inset
/// reserves a band on that edge: top bands grow downward from y = 0, bottom
/// bands grow upward from the window bottom, left and right fill the middle
/// between those. Higher priority is the outer band. Draw ops and hits are
/// translated into that band and clipped to it, so a decoration cannot paint
/// or receive clicks in another decoration's band or in the terminal grid.
/// One slot is unmoved. No slots yield an empty chrome.
pub fn compose(width: i32, height: i32, slots: &[(&str, &Chrome)]) -> Chrome {
    let width = width.max(0);
    let height = height.max(0);
    let total_top: i32 = slots.iter().map(|(_, c)| c.top.max(0)).sum();
    let total_bottom: i32 = slots.iter().map(|(_, c)| c.bottom.max(0)).sum();
    let total_left: i32 = slots.iter().map(|(_, c)| c.left.max(0)).sum();
    let total_right: i32 = slots.iter().map(|(_, c)| c.right.max(0)).sum();
    let mut top_used = 0;
    let mut bottom_used = 0;
    let mut left_used = 0;
    let mut right_used = 0;
    let mut hits_out: Vec<(usize, Hit)> = Vec::new();
    let mut ops_out: Vec<(usize, DrawOp)> = Vec::new();
    for (index, (name, chrome)) in slots.iter().enumerate() {
        let shifts = [
            (Edge::Top, Shift { dx: 0, dy: top_used, band: (0, top_used, width, chrome.top.max(0)) }),
            (Edge::Bottom, Shift { dx: 0, dy: -bottom_used, band: (0, height - bottom_used - chrome.bottom.max(0), width, chrome.bottom.max(0)) }),
            (
                Edge::Left,
                Shift { dx: left_used, dy: 0, band: (left_used, total_top, chrome.left.max(0), (height - total_top - total_bottom).max(0)) },
            ),
            (
                Edge::Right,
                Shift {
                    dx: -right_used,
                    dy: 0,
                    band: (width - right_used - chrome.right.max(0), total_top, chrome.right.max(0), (height - total_top - total_bottom).max(0)),
                },
            ),
        ];
        if chrome.top > 0 {
            top_used += chrome.top;
        }
        if chrome.bottom > 0 {
            bottom_used += chrome.bottom;
        }
        if chrome.left > 0 {
            left_used += chrome.left;
        }
        if chrome.right > 0 {
            right_used += chrome.right;
        }
        for op in &chrome.ops {
            let (x, y, w, h) = op_origin(op);
            let Some(edge) = classify(chrome, width, height, x, y) else { continue };
            let Some((_, shift)) = shifts.iter().find(|(e, _)| *e == edge) else { continue };
            let (nx, ny) = (x.saturating_add(shift.dx), y.saturating_add(shift.dy));
            match op {
                DrawOp::Fill { color, .. } => {
                    if let Some((cx, cy, cw, ch)) = intersect((nx, ny, w, h), shift.band) {
                        ops_out.push((index, DrawOp::Fill { x: cx, y: cy, w: cw, h: ch, color: *color }));
                    }
                }
                DrawOp::Text { text, color, .. } => {
                    if intersect((nx, ny, 1, 1), shift.band).is_some() {
                        ops_out.push((index, DrawOp::Text { x: nx, y: ny, text: text.clone(), color: *color }));
                    }
                }
            }
        }
        for hit in &chrome.hits {
            let Some(edge) = classify(chrome, width, height, hit.x, hit.y) else { continue };
            let Some((_, shift)) = shifts.iter().find(|(e, _)| *e == edge) else { continue };
            let (nx, ny) = (hit.x.saturating_add(shift.dx), hit.y.saturating_add(shift.dy));
            if let Some((cx, cy, cw, ch)) = intersect((nx, ny, hit.w, hit.h), shift.band) {
                hits_out.push((index, Hit { x: cx, y: cy, w: cw, h: ch, id: hit.id.clone(), owner: (*name).to_string() }));
            }
        }
    }
    // Low priority first so a later (higher priority) hit wins `hit_test`.
    ops_out.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
    hits_out.sort_by_key(|(index, _)| std::cmp::Reverse(*index));
    Chrome {
        top: total_top,
        bottom: total_bottom,
        left: total_left,
        right: total_right,
        ops: ops_out.into_iter().map(|(_, op)| op).collect(),
        hits: hits_out.into_iter().map(|(_, hit)| hit).collect(),
    }
}

/// Merge PTY ops. `parts` is highest priority first.
///
/// Every spawn is kept. Spawns from a lower priority are applied first, so
/// the highest priority spawn is opened last and becomes the focused tab.
/// For each PTY id, the highest priority decoration that mentioned it chooses
/// Focus or Close (its own last op for that id). A lower priority cannot
/// close an id the higher one focused, or focus an id the higher one closed.
/// At most one Focus survives: the highest priority decoration's focus.
pub fn arbitrate(parts: &[(&str, &[PtyOp])]) -> Vec<PtyOp> {
    let mut spawns = Vec::new();
    let mut chosen: Vec<(u64, bool)> = Vec::new();
    let mut focus_owner: Option<usize> = None;
    for (pi, (_, ops)) in parts.iter().enumerate() {
        let mut local: Vec<(u64, bool)> = Vec::new();
        for op in *ops {
            match op {
                PtyOp::Spawn { .. } => spawns.push((pi, op.clone())),
                PtyOp::Focus { id } => upsert(&mut local, *id, true),
                PtyOp::Close { id } => upsert(&mut local, *id, false),
            }
        }
        for (id, is_focus) in local {
            if chosen.iter().any(|(have, _)| *have == id) {
                continue;
            }
            if is_focus {
                if focus_owner.is_some() {
                    continue;
                }
                focus_owner = Some(pi);
            }
            chosen.push((id, is_focus));
        }
    }
    let mut out = Vec::new();
    for (id, is_focus) in &chosen {
        if !is_focus {
            out.push(PtyOp::Close { id: *id });
        }
    }
    let mut spawns: Vec<_> = spawns.into_iter().collect();
    spawns.sort_by_key(|(pi, _)| std::cmp::Reverse(*pi));
    out.extend(spawns.into_iter().map(|(_, op)| op));
    for (id, is_focus) in &chosen {
        if *is_focus {
            out.push(PtyOp::Focus { id: *id });
        }
    }
    out
}

fn upsert(local: &mut Vec<(u64, bool)>, id: u64, is_focus: bool) {
    if let Some(slot) = local.iter_mut().find(|(have, _)| *have == id) {
        slot.1 = is_focus;
    } else {
        local.push((id, is_focus));
    }
}

/// What each decoration receives for one chrome click.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Fanout {
    pub name: String,
    /// `Hit` for the owner only. Empty when this decoration does not own it.
    pub event: Option<DecorEvent>,
    /// Broadcast, including to the owner.
    pub note: PtyNote,
}

/// The owner is told about the hit. Every decoration, owner included, is
/// told with [`PtyNote::ChromeHit`] so it can redraw without acting twice.
pub fn fanout_hit(names: &[&str], owner: &str, id: &str) -> Vec<Fanout> {
    let note = PtyNote::ChromeHit { owner: owner.to_string(), id: id.to_string() };
    names
        .iter()
        .map(|name| Fanout { name: (*name).to_string(), event: (*name == owner).then(|| DecorEvent::Hit { id: id.to_string() }), note: note.clone() })
        .collect()
}

/// Run `chrome`. A panic or [`Decoration::failed`] drops only this decoration.
pub fn isolate_chrome(driver: &mut dyn Decoration, ctx: &DecorCtx) -> Option<Chrome> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| driver.chrome(ctx)));
    match result {
        Ok(chrome) if !driver.failed() => Some(chrome),
        _ => None,
    }
}

/// Run `event`. A panic or [`Decoration::failed`] drops only this decoration.
pub fn isolate_event(driver: &mut dyn Decoration, ev: &DecorEvent, ctx: &DecorCtx) -> Option<Vec<PtyOp>> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| driver.event(ev, ctx)));
    match result {
        Ok(ops) if !driver.failed() => Some(ops),
        _ => None,
    }
}

/// Run `note`. A panic or [`Decoration::failed`] drops only this decoration.
pub fn isolate_note(driver: &mut dyn Decoration, note: &PtyNote, ctx: &DecorCtx) -> Option<Vec<PtyOp>> {
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| driver.note(note, ctx)));
    match result {
        Ok(ops) if !driver.failed() => Some(ops),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pty(id: u64, kind: PtyKind, title: &str) -> PtyInfo {
        PtyInfo { id, title: title.into(), shell: String::new(), command: Vec::new(), kind, hold: false, exited: None }
    }

    fn ctx(ptys: Vec<PtyInfo>, focused: u64) -> DecorCtx {
        DecorCtx { ptys, focused: Some(focused), width: 400, cell_w: 8, cell_h: 16, ..DecorCtx::default() }
    }

    #[test]
    fn new_tab_uses_default_shell_unless_the_event_overrides_it() {
        let config = TabsConfig { shell: Some("/bin/zsh".into()), tab: vec![] };
        let c = ctx(vec![pty(1, PtyKind::Shell, "shell")], 1);
        let op = tabs_event(&config, &DecorEvent::NewTab { shell: None }, &c);
        assert_eq!(op, vec![PtyOp::Spawn { shell: Some("/bin/zsh".into()), command: vec![], title: "zsh".into() }]);
        let op = tabs_event(&config, &DecorEvent::NewTab { shell: Some("/usr/bin/fish".into()) }, &c);
        assert_eq!(op, vec![PtyOp::Spawn { shell: Some("/usr/bin/fish".into()), command: vec![], title: "fish".into() }]);
        let op = tabs_event(&TabsConfig::default(), &DecorEvent::Hit { id: "new".into() }, &c);
        assert_eq!(op, vec![PtyOp::Spawn { shell: None, command: vec![], title: "shell".into() }]);
    }

    #[test]
    fn startup_plan_assigns_a_shell_per_tab() {
        let config = TabsConfig {
            shell: Some("/bin/bash".into()),
            tab: vec![
                TabSpec { title: Some("work".into()), shell: Some("/bin/zsh".into()), command: vec![] },
                TabSpec { title: None, shell: None, command: vec!["/usr/bin/fish".into(), "-l".into()] },
            ],
        };
        let plan = startup_plan(&config, true);
        let first = plan.first.unwrap();
        assert_eq!(first.title, "work");
        assert_eq!(first.shell.as_deref(), Some("/bin/zsh"));
        assert!(first.command.is_empty());
        assert_eq!(plan.extra, vec![PtyOp::Spawn { shell: None, command: vec!["/usr/bin/fish".into(), "-l".into()], title: "fish".into() }]);
        // A CLI command stays; configured tabs are extra PTYs.
        let plan = startup_plan(&config, false);
        assert!(plan.first.is_none());
        assert_eq!(plan.extra.len(), 2);
        // Only a default shell: the first PTY uses it, nothing extra opens.
        let plan = startup_plan(&TabsConfig { shell: Some("/bin/bash".into()), ..TabsConfig::default() }, true);
        assert_eq!(plan.first.unwrap().shell.as_deref(), Some("/bin/bash"));
        assert!(plan.extra.is_empty());
        assert!(startup_plan(&TabsConfig::default(), true).first.is_none());
    }

    #[test]
    fn switch_close_and_cycle_follow_the_open_ptys() {
        let config = TabsConfig::default();
        let c = ctx(vec![pty(1, PtyKind::Shell, "a"), pty(2, PtyKind::Shell, "b"), pty(7, PtyKind::Command, "c")], 2);
        assert_eq!(tabs_event(&config, &DecorEvent::Cycle { delta: 1 }, &c), vec![PtyOp::Focus { id: 7 }]);
        assert_eq!(tabs_event(&config, &DecorEvent::Cycle { delta: -1 }, &c), vec![PtyOp::Focus { id: 1 }]);
        assert_eq!(tabs_event(&config, &DecorEvent::Hit { id: "focus:7".into() }, &c), vec![PtyOp::Focus { id: 7 }]);
        assert_eq!(tabs_event(&config, &DecorEvent::Hit { id: "close:1".into() }, &c), vec![PtyOp::Close { id: 1 }]);
        assert!(tabs_event(&config, &DecorEvent::CloseTab { id: 9 }, &c).is_empty());
        assert!(tabs_event(&config, &DecorEvent::KeyOnExited, &c).is_empty());
    }

    #[test]
    fn shell_exit_closes_the_tab_and_a_failed_command_stays() {
        let mut a = pty(1, PtyKind::Shell, "zsh");
        let mut b = pty(2, PtyKind::Command, "make");
        b.command = vec!["make".into()];
        let c = ctx(vec![a.clone(), b.clone()], 1);
        assert_eq!(tabs_note(&PtyNote::Exited { id: 1, code: 0 }, &c), vec![PtyOp::Focus { id: 2 }, PtyOp::Close { id: 1 }]);
        assert!(tabs_note(&PtyNote::Exited { id: 2, code: 2 }, &c).is_empty());
        assert_eq!(tabs_note(&PtyNote::Exited { id: 2, code: 0 }, &c), vec![PtyOp::Close { id: 2 }]);
        a.kind = PtyKind::Exec;
        a.hold = true;
        assert!(!closes_on_exit(a.kind, a.hold, 1));
        assert!(closes_on_exit(PtyKind::Handoff, false, 9));
        // The exit banner stays until a key; then the tab closes.
        b.exited = Some(2);
        let c = ctx(vec![b], 2);
        assert_eq!(tabs_event(&TabsConfig::default(), &DecorEvent::KeyOnExited, &c), vec![PtyOp::Close { id: 2 }]);
    }

    #[test]
    fn chrome_hits_select_close_and_new_and_reserve_the_top() {
        let c = ctx(vec![pty(3, PtyKind::Shell, "zsh"), pty(4, PtyKind::Shell, "bash")], 4);
        let chrome = tabs_chrome(&c);
        assert_eq!(chrome.top, bar_height(16));
        assert!(chrome.bottom == 0 && chrome.left == 0);
        assert!(chrome.active());
        let focus = hit_test(&chrome, 10, 4).unwrap();
        assert_eq!(focus.id, "focus:3");
        let new = hit_test(&chrome, c.width - 4, 4).unwrap();
        assert_eq!(new.id, "new");
        // The close control is the right edge of the first tab, above the tab body.
        let close = chrome.hits.iter().find(|h| h.id == "close:3").unwrap();
        assert_eq!(hit_test(&chrome, close.x + 1, 4).unwrap().id, "close:3");
        let exited = {
            let mut p = pty(3, PtyKind::Shell, "zsh");
            p.exited = Some(1);
            p
        };
        let text = tabs_chrome(&ctx(vec![exited], 3)).ops.into_iter().find_map(|op| match op {
            DrawOp::Text { text, .. } if text.contains("exited") => Some(text),
            _ => None,
        });
        assert_eq!(text.as_deref(), Some("zsh · exited"));
    }

    #[cfg(unix)]
    #[test]
    fn stdio_decoration_answers_chrome_and_events() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let run = dir.path().join("run");
        std::fs::write(
            &run,
            r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"op":"event"'*) printf '%s\n' '{"ops":[{"op":"spawn","shell":"/bin/zsh","command":[],"title":"zsh"}]}' ;;
    *'"op":"note"'*) printf '%s\n' '{"ops":[{"op":"close","id":4}]}' ;;
    *) printf '%s\n' '{"chrome":{"top":18,"bottom":1,"left":2,"right":3,"ops":[{"op":"fill","x":0,"y":0,"w":8,"h":8,"color":7}],"hits":[{"x":1,"y":2,"w":3,"h":4,"id":"new"}]}}' ;;
  esac
done
"#,
        )
        .unwrap();
        std::fs::set_permissions(&run, std::fs::Permissions::from_mode(0o755)).unwrap();
        let mut proc = DecorationProc::spawn(&[run.display().to_string()], dir.path()).unwrap();
        let ctx = DecorCtx::default();
        let chrome = proc.chrome(&ctx);
        assert_eq!((chrome.top, chrome.bottom, chrome.left, chrome.right), (18, 1, 2, 3));
        assert_eq!(hit_test(&chrome, 1, 2).unwrap().id, "new");
        assert!(!chrome.ops.is_empty());
        let ops = proc.event(&DecorEvent::NewTab { shell: None }, &ctx);
        assert_eq!(ops, vec![PtyOp::Spawn { shell: Some("/bin/zsh".into()), command: vec![], title: "zsh".into() }]);
        let ops = proc.note(&PtyNote::Exited { id: 4, code: 0 }, &ctx);
        assert_eq!(ops, vec![PtyOp::Close { id: 4 }]);
        assert!(!proc.failed());
    }

    fn fill(x: i32, y: i32, w: i32, h: i32, color: u32) -> DrawOp {
        DrawOp::Fill { x, y, w, h, color }
    }

    fn hit(x: i32, y: i32, w: i32, h: i32, id: &str) -> Hit {
        Hit { x, y, w, h, id: id.into(), owner: String::new() }
    }

    #[test]
    fn higher_priority_top_band_is_the_outer_one() {
        let high = Chrome { top: 10, ops: vec![fill(0, 0, 40, 10, 1)], hits: vec![hit(0, 0, 40, 10, "a")], ..Chrome::default() };
        let low = Chrome { top: 8, ops: vec![fill(0, 0, 40, 8, 2)], hits: vec![hit(0, 0, 40, 8, "b")], ..Chrome::default() };
        let chrome = compose(100, 80, &[("high", &high), ("low", &low)]);
        assert_eq!((chrome.top, chrome.bottom, chrome.left, chrome.right), (18, 0, 0, 0));
        assert_eq!(chrome.ops[0], fill(0, 10, 40, 8, 2));
        assert_eq!(chrome.ops[1], fill(0, 0, 40, 10, 1));
        assert_eq!(hit_test(&chrome, 2, 2).unwrap().owner, "high");
        assert_eq!(hit_test(&chrome, 2, 12).unwrap().owner, "low");
        let alone = compose(100, 80, &[("high", &high)]);
        assert_eq!(alone.ops, vec![fill(0, 0, 40, 10, 1)]);
        assert_eq!(alone.hits[0].id, "a");
        assert!(compose(100, 80, &[]).ops.is_empty());
    }

    #[test]
    fn lower_priority_chrome_cannot_paint_outside_its_band() {
        let high = Chrome { top: 20, ops: vec![fill(0, 0, 50, 20, 1)], ..Chrome::default() };
        let low = Chrome { top: 10, ops: vec![fill(0, 0, 50, 40, 2)], hits: vec![hit(0, 0, 50, 40, "spill")], ..Chrome::default() };
        let chrome = compose(80, 60, &[("high", &high), ("low", &low)]);
        let low_fill = chrome.ops.iter().find(|op| matches!(op, DrawOp::Fill { color: 2, .. })).unwrap();
        assert_eq!(low_fill, &fill(0, 20, 50, 10, 2));
        let high_fill = chrome.ops.iter().find(|op| matches!(op, DrawOp::Fill { color: 1, .. })).unwrap();
        assert_eq!(high_fill, &fill(0, 0, 50, 20, 1));
        assert!(hit_test(&chrome, 2, 5).is_none());
        let spill = hit_test(&chrome, 2, 22).unwrap();
        assert_eq!((spill.owner.as_str(), spill.id.as_str(), spill.h), ("low", "spill", 10));
    }

    #[test]
    fn pty_ops_keep_every_spawn_and_the_highest_priority_focus() {
        let spawn = |title: &str| PtyOp::Spawn { shell: Some(title.into()), command: vec![], title: title.into() };
        let high = [PtyOp::Focus { id: 1 }, spawn("high")];
        let low = [PtyOp::Focus { id: 2 }, PtyOp::Close { id: 1 }, PtyOp::Close { id: 3 }, spawn("low")];
        let ops = arbitrate(&[("high", &high), ("low", &low)]);
        assert_eq!(ops, vec![PtyOp::Close { id: 3 }, spawn("low"), spawn("high"), PtyOp::Focus { id: 1 }]);
        let both_close = [PtyOp::Close { id: 4 }, PtyOp::Focus { id: 4 }];
        let ops = arbitrate(&[("high", &both_close), ("low", &[PtyOp::Close { id: 4 }])]);
        assert_eq!(ops, vec![PtyOp::Focus { id: 4 }]);
    }

    #[test]
    fn a_hit_goes_to_its_owner_and_a_note_goes_to_everyone() {
        let fan = fanout_hit(&["tabs", "status"], "tabs", "focus:1");
        assert_eq!(fan[0].event, Some(DecorEvent::Hit { id: "focus:1".into() }));
        assert_eq!(fan[1].event, None);
        assert_eq!(fan[0].note, PtyNote::ChromeHit { owner: "tabs".into(), id: "focus:1".into() });
        assert_eq!(fan[1].note, fan[0].note);
        assert_eq!(pack_hit("tabs", "focus:1").split('\u{1f}').collect::<Vec<_>>(), vec!["tabs", "focus:1"]);
        assert_eq!(unpack_hit(&pack_hit("tabs", "close:2")), ("tabs".into(), "close:2".into()));
    }

    struct Boom;

    impl Decoration for Boom {
        fn chrome(&mut self, _: &DecorCtx) -> Chrome {
            panic!("boom");
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx) -> Vec<PtyOp> {
            panic!("boom");
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx) -> Vec<PtyOp> {
            panic!("boom");
        }
    }

    struct Quiet;

    impl Decoration for Quiet {
        fn chrome(&mut self, _: &DecorCtx) -> Chrome {
            Chrome { top: 4, ..Chrome::default() }
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx) -> Vec<PtyOp> {
            vec![PtyOp::Focus { id: 9 }]
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx) -> Vec<PtyOp> {
            Vec::new()
        }
    }

    #[test]
    fn a_panicking_decoration_is_isolated() {
        let ctx = DecorCtx::default();
        let mut bad = Boom;
        let mut good = Quiet;
        assert!(isolate_chrome(&mut bad, &ctx).is_none());
        assert_eq!(isolate_chrome(&mut good, &ctx).unwrap().top, 4);
        assert!(isolate_event(&mut bad, &DecorEvent::KeyOnExited, &ctx).is_none());
        assert_eq!(isolate_event(&mut good, &DecorEvent::KeyOnExited, &ctx).unwrap(), vec![PtyOp::Focus { id: 9 }]);
        assert!(isolate_note(&mut bad, &PtyNote::Focused { id: 1 }, &ctx).is_none());
        assert!(isolate_note(&mut good, &PtyNote::Focused { id: 1 }, &ctx).unwrap().is_empty());
    }
}
