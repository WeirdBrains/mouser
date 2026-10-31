//! Session server / UI process split (docs/engines.md §8), Unix only.
//!
//! The process the user starts keeps the sessions (PTYs, parsers, scrollback,
//! tabs, images) and serves [`RenderHooks`] over a socketpair to a child
//! `mouser __ui` process that owns the window and the engine. When the UI
//! process dies abnormally (a native crash in any engine), the server skips
//! that engine and starts a new UI, which draws the same sessions again.

use crate::frame::{CursorShape, ImageDraw};
use crate::{Colors, RenderHooks, Renderer, SelectEvent, UiViewport, ui};
use anyhow::{Context, Result, bail};
use mouser_core::{fuzzball, input::MouseEvent, kitty};
use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::collections::{HashMap, HashSet, VecDeque};
use std::io::{self, BufReader, Read, Write};
use std::os::fd::{FromRawFd, OwnedFd};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex, OnceLock, mpsc};
use std::time::{Duration, Instant};

/// Bumped whenever a message changes. Both ends are the same binary
/// (`/proc/self/exe`), so a mismatch means a broken install.
pub const PROTOCOL: u32 = 1;
/// UI exit code for "protocol mismatch, run in-process instead".
const EXIT_MISMATCH: i32 = 3;
/// More crashes than this within [`CRASH_WINDOW`] -> the CPU renderer.
const CRASH_LIMIT: usize = 5;
const CRASH_WINDOW: Duration = Duration::from_secs(30);

#[derive(Serialize, Deserialize, Debug)]
enum Call {
    Hello(u32),
    KeyBytes(Vec<u8>),
    KeyEvent(Option<Vec<u8>>, kitty::KeyInput, Option<fuzzball::KeyEvent>),
    Images(usize, usize, (u32, u32)),
    Resize(u16, u16, u16, u16),
    View(usize, usize),
    CursorShape,
    Mouse(MouseEvent),
    Focus(bool),
    Paste,
    Select(SelectEvent),
    Copy,
    PastePrimary,
    HoldFrame,
    Title,
    Tabs,
    Lines(usize, usize),
    Colors,
    Font,
    Cursor,
    UiModel(UiViewport),
    UiAction(ui::UiAction),
    Status,
    ShouldExit,
    Engine(String),
    WindowSize(u32, u32),
}

/// Which optional hooks the server has.
#[derive(Serialize, Deserialize, Debug, Default, Clone, Copy)]
struct Has {
    key_event: bool,
    images: bool,
    view: bool,
    cursor_shape: bool,
    mouse: bool,
    focus: bool,
    paste: bool,
    select: bool,
    copy: bool,
    paste_primary: bool,
    hold_frame: bool,
    title: bool,
    tabs: bool,
    font: bool,
    cursor: bool,
    ui_model: bool,
    ui_action: bool,
    status: bool,
    should_exit: bool,
}

#[derive(Serialize, Deserialize, Debug)]
struct HelloReply {
    version: u32,
    renderer: Renderer,
    has: Has,
    /// Shown in the status bar of a restarted UI (why it restarted).
    notice: Option<String>,
}

/// Server -> UI, on the events socket.
#[derive(Serialize, Deserialize, Debug)]
enum Event {
    Wake,
    ClipStore(String, bool),
    ClipLoad(bool),
}

/// UI -> server answers on the events socket.
#[derive(Serialize, Deserialize, Debug)]
enum Answer {
    Stored(bool),
    Loaded(Option<Option<String>>),
}

fn io_err(e: impl std::fmt::Display) -> io::Error {
    io::Error::other(e.to_string())
}

fn send<T: Serialize>(w: &mut impl Write, v: &T) -> io::Result<()> {
    let body = bincode::serialize(v).map_err(io_err)?;
    send_raw(w, &body)
}

fn send_raw(w: &mut impl Write, body: &[u8]) -> io::Result<()> {
    let mut b = Vec::with_capacity(body.len() + 4);
    b.extend_from_slice(&(body.len() as u32).to_le_bytes());
    b.extend_from_slice(body);
    w.write_all(&b)?;
    w.flush()
}

fn recv<T: DeserializeOwned>(r: &mut impl Read) -> io::Result<T> {
    let mut l = [0u8; 4];
    r.read_exact(&mut l)?;
    let mut b = vec![0; u32::from_le_bytes(l) as usize];
    r.read_exact(&mut b)?;
    bincode::deserialize(&b).map_err(io_err)
}

static UI_COMMAND: OnceLock<(PathBuf, Vec<String>)> = OnceLock::new();

/// Program and arguments that start a UI process (`mouser __ui`).
pub fn set_ui_command(program: PathBuf, args: Vec<String>) {
    let _ = UI_COMMAND.set((program, args));
}

/// Whether this run splits sessions and UI (Unix with a UI command, unless
/// `MOUSER_SPLIT=0`).
pub fn enabled() -> bool {
    cfg!(unix) && UI_COMMAND.get().is_some() && std::env::var("MOUSER_SPLIT").map_or(true, |v| v != "0")
}

// ---------------------------------------------------------------- server

#[derive(Default)]
struct Shared {
    events: Mutex<Option<UnixStream>>,
    answers: Mutex<Option<mpsc::Sender<Answer>>>,
    clip: Mutex<()>,
    engine: Mutex<String>,
    size: Mutex<Option<(u32, u32)>>,
}

impl Shared {
    fn event(&self, e: &Event) -> bool {
        let mut g = self.events.lock().unwrap();
        match g.as_mut() {
            Some(s) => send(s, e).is_ok(),
            None => false,
        }
    }

    /// Ask the UI to do a clipboard operation (it owns the Wayland connection).
    fn ask(&self, e: Event) -> Option<Answer> {
        let _one = self.clip.lock().unwrap();
        let (tx, rx) = mpsc::channel();
        *self.answers.lock().unwrap() = Some(tx);
        let a = if self.event(&e) { rx.recv_timeout(Duration::from_secs(2)).ok() } else { None };
        *self.answers.lock().unwrap() = None;
        a
    }
}

fn has(h: &RenderHooks) -> Has {
    Has {
        key_event: h.on_key_event.is_some(),
        images: h.get_images.is_some(),
        view: h.fetch_view.is_some(),
        cursor_shape: h.get_cursor_shape.is_some(),
        mouse: h.on_mouse.is_some(),
        focus: h.on_focus.is_some(),
        paste: h.on_paste.is_some(),
        select: h.on_select.is_some(),
        copy: h.on_copy.is_some(),
        paste_primary: h.on_paste_primary.is_some(),
        hold_frame: h.hold_frame.is_some(),
        title: h.get_title.is_some(),
        tabs: h.get_tabs.is_some(),
        font: h.get_font.is_some(),
        cursor: h.get_cursor.is_some(),
        ui_model: h.get_ui_model.is_some(),
        ui_action: h.on_ui_action.is_some(),
        status: h.get_status.is_some(),
        should_exit: h.should_exit.is_some(),
    }
}

/// Image pixels go over once per key; later frames send an empty buffer and
/// the UI uses its copy. Both ends keep exactly the keys of the last reply.
fn strip_images(mut v: Vec<ImageDraw>, sent: &mut HashSet<u64>) -> Vec<ImageDraw> {
    let mut now = HashSet::new();
    for d in &mut v {
        if sent.contains(&d.key) || now.contains(&d.key) {
            d.rgba = Arc::new(Vec::new());
        }
        now.insert(d.key);
    }
    *sent = now;
    v
}

fn fill_images(mut v: Vec<ImageDraw>, cache: &mut HashMap<u64, Arc<Vec<u8>>>) -> Vec<ImageDraw> {
    let mut now = HashMap::new();
    for d in &mut v {
        if d.rgba.is_empty()
            && let Some(p) = now.get(&d.key).or_else(|| cache.get(&d.key))
        {
            d.rgba = Arc::clone(p);
        }
        now.insert(d.key, Arc::clone(&d.rgba));
    }
    *cache = now;
    v
}

fn ser<T: Serialize>(v: &T) -> Vec<u8> {
    bincode::serialize(v).unwrap_or_default()
}

fn dispatch(h: &RenderHooks, c: Call, sent: &mut HashSet<u64>, sh: &Shared, hello: &dyn Fn() -> HelloReply) -> Vec<u8> {
    match c {
        Call::Hello(_) => ser(&hello()),
        Call::KeyBytes(b) => ser(&(h.on_key_bytes)(b)),
        Call::KeyEvent(b, k, f) => ser(&h.on_key_event.as_ref().map(|x| x(b, k, f))),
        Call::Images(a, b, c) => ser(&strip_images(h.get_images.as_ref().map(|x| x(a, b, c)).unwrap_or_default(), sent)),
        Call::Resize(a, b, c, d) => ser(&(h.on_resize)(a, b, c, d)),
        Call::View(a, b) => ser(&h.fetch_view.as_ref().map(|x| x(a, b))),
        Call::CursorShape => ser(&h.get_cursor_shape.as_ref().map(|x| x()).unwrap_or_default()),
        Call::Mouse(m) => ser(&h.on_mouse.as_ref().is_some_and(|x| x(m))),
        Call::Focus(f) => ser(&h.on_focus.as_ref().map(|x| x(f))),
        Call::Paste => ser(&h.on_paste.as_ref().map(|x| x())),
        Call::Select(e) => ser(&h.on_select.as_ref().map(|x| x(e))),
        Call::Copy => ser(&h.on_copy.as_ref().map(|x| x())),
        Call::PastePrimary => ser(&h.on_paste_primary.as_ref().map(|x| x())),
        Call::HoldFrame => ser(&h.hold_frame.as_ref().is_some_and(|x| x())),
        Call::Title => ser(&h.get_title.as_ref().and_then(|x| x())),
        Call::Tabs => ser(&h.get_tabs.as_ref().map(|x| x()).unwrap_or_default()),
        Call::Lines(a, b) => ser(&(h.fetch_lines)(a, b)),
        Call::Colors => ser(&(h.get_colors)()),
        Call::Font => ser(&h.get_font.as_ref().map(|x| x()).unwrap_or((15.0, None))),
        Call::Cursor => ser(&h.get_cursor.as_ref().and_then(|x| x())),
        Call::UiModel(v) => ser(&h.get_ui_model.as_ref().and_then(|x| x(v))),
        Call::UiAction(a) => ser(&h.on_ui_action.as_ref().map(|x| x(a))),
        Call::Status => ser(&h.get_status.as_ref().and_then(|x| x())),
        Call::ShouldExit => ser(&h.should_exit.as_ref().is_some_and(|x| x())),
        Call::Engine(e) => {
            *sh.engine.lock().unwrap() = e.clone();
            ser(&h.on_engine.as_ref().map(|x| x(e)))
        }
        Call::WindowSize(w, hh) => {
            *sh.size.lock().unwrap() = Some((w, hh));
            ser(&h.on_window_size.as_ref().map(|x| x(w, hh)))
        }
    }
}

/// Why the UI process ended.
enum UiEnd {
    Normal,
    Mismatch,
    Crashed(String),
    /// Killed from outside (SIGKILL, SIGTERM, ...): not the engine's fault.
    Killed(String),
}

fn describe(st: std::process::ExitStatus) -> String {
    use std::os::unix::process::ExitStatusExt;
    match st.signal() {
        Some(s) => format!("signal {s}{}", signal_name(s).map(|n| format!(" ({n})")).unwrap_or_default()),
        None => format!("exit code {}", st.code().unwrap_or(-1)),
    }
}

fn killed(st: std::process::ExitStatus) -> bool {
    use std::os::unix::process::ExitStatusExt;
    matches!(st.signal(), Some(libc::SIGKILL | libc::SIGTERM | libc::SIGINT | libc::SIGHUP))
}

fn signal_name(s: i32) -> Option<&'static str> {
    Some(match s {
        libc::SIGILL => "SIGILL",
        libc::SIGABRT => "SIGABRT",
        libc::SIGBUS => "SIGBUS",
        libc::SIGFPE => "SIGFPE",
        libc::SIGKILL => "SIGKILL",
        libc::SIGSEGV => "SIGSEGV",
        libc::SIGTERM => "SIGTERM",
        _ => return None,
    })
}

/// Run one UI process to its end.
fn run_ui(hooks: &RenderHooks, sh: &Arc<Shared>, cpu: bool, hello: &dyn Fn() -> HelloReply) -> Result<UiEnd> {
    let (prog, args) = UI_COMMAND.get().context("no UI command")?;
    let (calls, calls_ui) = UnixStream::pair()?;
    let (events, events_ui) = UnixStream::pair()?;
    let mut cmd = std::process::Command::new(prog);
    cmd.args(args).stdin(OwnedFd::from(calls_ui)).stdout(OwnedFd::from(events_ui));
    if cpu {
        cmd.env("MOUSER_RENDERER", "cpu");
    }
    let mut child = cmd.spawn().with_context(|| format!("starting the UI process {}", prog.display()))?;
    // Close our copies of the UI's ends, or a dead UI never reads as EOF.
    drop(cmd);
    log::info!("UI process {} started", child.id());
    *sh.events.lock().unwrap() = Some(events.try_clone()?);
    {
        let sh = sh.clone();
        let mut r = BufReader::new(events);
        std::thread::Builder::new().name("ui-answers".into()).spawn(move || {
            while let Ok(a) = recv::<Answer>(&mut r) {
                if let Some(tx) = sh.answers.lock().unwrap().as_ref() {
                    let _ = tx.send(a);
                }
            }
        })?;
    }
    let mut sent = HashSet::new();
    let mut r = BufReader::new(calls.try_clone()?);
    let mut w = calls;
    while let Ok(c) = recv::<Call>(&mut r) {
        let reply = dispatch(hooks, c, &mut sent, sh, hello);
        if send_raw(&mut w, &reply).is_err() {
            break;
        }
    }
    *sh.events.lock().unwrap() = None;
    let st = child.wait()?;
    Ok(if st.success() {
        UiEnd::Normal
    } else if st.code() == Some(EXIT_MISMATCH) {
        UiEnd::Mismatch
    } else if killed(st) {
        UiEnd::Killed(describe(st))
    } else {
        UiEnd::Crashed(describe(st))
    })
}

/// Serve `hooks` to UI processes until the window is closed or the session
/// ends; restart the UI (with its engine skipped) after a crash.
pub fn serve(renderer: Renderer, hooks: RenderHooks) -> Result<()> {
    let sh = Arc::new(Shared::default());
    if let Some(set_waker) = &hooks.set_waker {
        let sh = sh.clone();
        set_waker(Arc::new(move || {
            sh.event(&Event::Wake);
        }));
    }
    {
        let (a, b) = (sh.clone(), sh.clone());
        crate::clipboard::set_remote(
            Box::new(move |text: &str, primary: bool| matches!(a.ask(Event::ClipStore(text.to_string(), primary)), Some(Answer::Stored(true)))),
            Box::new(move |primary: bool| match b.ask(Event::ClipLoad(primary)) {
                Some(Answer::Loaded(v)) => v,
                _ => None,
            }),
        );
    }
    let h = has(&hooks);
    let notice: Mutex<Option<String>> = Mutex::new(None);
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    let mut cpu = false;
    loop {
        let hello = || HelloReply {
            version: PROTOCOL,
            renderer: Renderer { size: *sh.size.lock().unwrap(), ..renderer.clone() },
            has: h,
            notice: notice.lock().unwrap().clone(),
        };
        match run_ui(&hooks, &sh, cpu, &hello)? {
            UiEnd::Normal => return Ok(()),
            UiEnd::Mismatch => {
                log::warn!("UI process speaks another protocol; drawing in this process");
                crate::clipboard::clear_remote();
                return crate::run(renderer, hooks);
            }
            UiEnd::Killed(how) => {
                if hooks.should_exit.as_ref().is_some_and(|x| x()) {
                    return Ok(());
                }
                let now = Instant::now();
                crashes.push_back(now);
                while crashes.front().is_some_and(|t| now - *t > CRASH_WINDOW) {
                    crashes.pop_front();
                }
                if crashes.len() > CRASH_LIMIT * 2 {
                    bail!("the UI process keeps being killed ({how})");
                }
                eprintln!("mouser: the UI process was killed ({how}); restarting the window, sessions are kept");
                *notice.lock().unwrap() = Some(format!("the window was restarted after the UI process was killed ({how}); sessions are kept"));
            }
            UiEnd::Crashed(how) => {
                if hooks.should_exit.as_ref().is_some_and(|x| x()) {
                    return Ok(());
                }
                let engine = std::mem::take(&mut *sh.engine.lock().unwrap());
                let now = Instant::now();
                crashes.push_back(now);
                while crashes.front().is_some_and(|t| now - *t > CRASH_WINDOW) {
                    crashes.pop_front();
                }
                if engine == "cpu" || (cpu && crashes.len() > CRASH_LIMIT) {
                    bail!("the UI process keeps crashing ({how}), even on the CPU renderer");
                }
                let reason = format!("the UI process crashed ({how}) while drawing with {engine}");
                eprintln!("mouser: {reason}; restarting the window, sessions are kept");
                let msg = if engine == mouser_core::engines::DEFAULT || engine.is_empty() || crashes.len() > CRASH_LIMIT {
                    cpu = true;
                    format!("{reason}; restarted on the CPU renderer")
                } else {
                    mouser_core::engines::mark_skipped(&engine, &reason);
                    format!("{reason}; {engine} is skipped (mouser engine unskip {engine})")
                };
                *notice.lock().unwrap() = Some(msg);
            }
        }
    }
}

// ---------------------------------------------------------------- UI

struct Client {
    io: Mutex<(BufReader<UnixStream>, UnixStream)>,
    images: Mutex<HashMap<u64, Arc<Vec<u8>>>>,
}

impl Client {
    fn call<R: DeserializeOwned>(&self, c: Call) -> R {
        let mut g = self.io.lock().unwrap();
        let (r, w) = &mut *g;
        match send(w, &c).and_then(|_| recv(r)) {
            Ok(v) => v,
            Err(_) => {
                // The server is gone: nothing left to draw.
                std::process::exit(0);
            }
        }
    }
}

/// Entry point of `mouser __ui`: the calls socket is stdin, the events socket
/// stdout (stdout is then pointed at stderr so stray prints can't corrupt it).
pub fn ui_main() -> Result<()> {
    // SAFETY: fds 0 and 1 are the sockets the server passed; dup/dup2 only
    // rearrange descriptors this process owns.
    let (calls, events) = unsafe {
        let ev = libc::dup(1);
        if ev < 0 || libc::dup2(2, 1) < 0 {
            bail!("UI: cannot set up the events socket");
        }
        (UnixStream::from_raw_fd(0), UnixStream::from_raw_fd(ev))
    };
    let client = Arc::new(Client { io: Mutex::new((BufReader::new(calls.try_clone()?), calls)), images: Mutex::new(HashMap::new()) });
    let hello: HelloReply = client.call(Call::Hello(PROTOCOL));
    if hello.version != PROTOCOL {
        std::process::exit(EXIT_MISMATCH);
    }
    if let Some(n) = hello.notice.clone() {
        crate::set_notice(n);
    }
    let waker: Arc<OnceLock<crate::Waker>> = Arc::new(OnceLock::new());
    {
        let waker = waker.clone();
        let mut r = BufReader::new(events.try_clone()?);
        let mut w = events;
        std::thread::Builder::new().name("ui-events".into()).spawn(move || {
            while let Ok(e) = recv::<Event>(&mut r) {
                let a = match e {
                    Event::Wake => {
                        if let Some(wk) = waker.get() {
                            wk();
                        }
                        continue;
                    }
                    Event::ClipStore(t, p) => Answer::Stored(crate::clipboard::store(&t, p)),
                    Event::ClipLoad(p) => Answer::Loaded(crate::clipboard::load(p)),
                };
                if send(&mut w, &a).is_err() {
                    break;
                }
            }
        })?;
    }
    if std::env::var("MOUSER_UI").is_ok_and(|v| v == "headless") {
        return headless(&client, hello.notice.is_none());
    }
    let hooks = client_hooks(&client, hello.has, waker);
    crate::run(hello.renderer, hooks)
}

/// Test UI without a window (`MOUSER_UI=headless`): reports the engine the
/// priority order would pick, then every 100 ms writes what a window would
/// show to `MOUSER_TEST_DUMP` (screen, scrollback, tabs, images).
/// `MOUSER_TEST_CRASH_ENGINE` aborts it like a crashing engine would.
fn headless(c: &Arc<Client>, first: bool) -> Result<()> {
    let dump = std::env::var_os("MOUSER_TEST_DUMP").map(PathBuf::from).context("MOUSER_TEST_DUMP is not set")?;
    let order = mouser_core::engines::priority();
    let engine = if std::env::var("MOUSER_RENDERER").is_ok_and(|v| v == "cpu") {
        "cpu".to_string()
    } else {
        crate::engine::first_usable(&order).unwrap_or_else(|| "cpu".into())
    };
    c.call::<Option<()>>(Call::Engine(engine.clone()));
    let (cols, rows) = (80u16, 24usize);
    c.call::<()>(Call::Resize(cols, rows as u16, 800, 480));
    // `MOUSER_TEST_UI_ACTIONS=NewTab,SelectTab:0`: done by the first UI only.
    if first {
        for a in std::env::var("MOUSER_TEST_UI_ACTIONS").unwrap_or_default().split(',') {
            let act = match a.split_once(':') {
                Some(("SelectTab", n)) => ui::UiAction::SelectTab(n.parse().unwrap_or(0)),
                _ if a == "NewTab" => ui::UiAction::NewTab,
                _ => continue,
            };
            c.call::<Option<()>>(Call::UiAction(act));
        }
    }
    let crash = crate::test_crash_engine();
    let mut cache = HashMap::new();
    for n in 1u32.. {
        if c.call::<bool>(Call::ShouldExit) {
            return Ok(());
        }
        if let Some((e, at)) = &crash
            && *e == engine
            && n >= *at
        {
            eprintln!("mouser: MOUSER_TEST_CRASH_ENGINE: aborting in engine {e} after {at} frames");
            std::process::abort();
        }
        let (screen, _, _): (Vec<String>, Vec<Vec<crate::CellPaint>>, usize) = c.call::<Option<_>>(Call::View(0, rows)).unwrap_or_default();
        let (back, _, _): (Vec<String>, Vec<Vec<crate::CellPaint>>, usize) = c.call::<Option<_>>(Call::View(40, rows)).unwrap_or_default();
        let tabs: ui::TabBar = c.call(Call::Tabs);
        let images = fill_images(c.call(Call::Images(0, rows, (10, 20))), &mut cache);
        let bytes: usize = images.iter().map(|d| d.rgba.len()).sum();
        let text = format!(
            "ui_pid={}\nengine={engine}\nframe={n}\ntabs={} active={}\nimages={} bytes={bytes}\n--- screen\n{}\n--- scrollback\n{}\n",
            std::process::id(),
            tabs.titles.len(),
            tabs.active,
            images.len(),
            screen.join("\n"),
            back.join("\n"),
        );
        let tmp = dump.with_extension("tmp");
        std::fs::write(&tmp, text)?;
        std::fs::rename(&tmp, &dump)?;
        std::thread::sleep(Duration::from_millis(100));
    }
    Ok(())
}

macro_rules! opt {
    ($on:expr, $c:ident, $f:expr) => {
        if $on {
            let $c = $c.clone();
            Some(Arc::new($f) as _)
        } else {
            None
        }
    };
}

fn client_hooks(c: &Arc<Client>, has: Has, waker: Arc<OnceLock<crate::Waker>>) -> RenderHooks {
    RenderHooks {
        on_key_bytes: {
            let c = c.clone();
            Arc::new(move |b| c.call(Call::KeyBytes(b)))
        },
        on_key_event: opt!(has.key_event, c, move |b, k, f| c.call::<Option<()>>(Call::KeyEvent(b, k, f)).unwrap_or(())),
        get_images: opt!(has.images, c, move |a, b, s| {
            let v: Vec<ImageDraw> = c.call(Call::Images(a, b, s));
            fill_images(v, &mut c.images.lock().unwrap())
        }),
        on_resize: {
            let c = c.clone();
            Arc::new(move |a, b, cc, d| c.call(Call::Resize(a, b, cc, d)))
        },
        fetch_view: opt!(has.view, c, move |a, b| c.call::<Option<_>>(Call::View(a, b)).unwrap_or_default()),
        get_cursor_shape: opt!(has.cursor_shape, c, move || c.call::<CursorShape>(Call::CursorShape)),
        on_mouse: opt!(has.mouse, c, move |m| c.call::<bool>(Call::Mouse(m))),
        on_focus: opt!(has.focus, c, move |f| c.call::<Option<()>>(Call::Focus(f)).unwrap_or(())),
        on_paste: opt!(has.paste, c, move || c.call::<Option<()>>(Call::Paste).unwrap_or(())),
        on_select: opt!(has.select, c, move |e| c.call::<Option<()>>(Call::Select(e)).unwrap_or(())),
        on_copy: opt!(has.copy, c, move || c.call::<Option<()>>(Call::Copy).unwrap_or(())),
        on_paste_primary: opt!(has.paste_primary, c, move || c.call::<Option<()>>(Call::PastePrimary).unwrap_or(())),
        hold_frame: opt!(has.hold_frame, c, move || c.call::<bool>(Call::HoldFrame)),
        get_title: opt!(has.title, c, move || c.call::<Option<String>>(Call::Title)),
        get_tabs: opt!(has.tabs, c, move || c.call::<ui::TabBar>(Call::Tabs)),
        fetch_lines: {
            let c = c.clone();
            Arc::new(move |a, b| c.call(Call::Lines(a, b)))
        },
        get_colors: {
            let c = c.clone();
            Arc::new(move || c.call::<Colors>(Call::Colors))
        },
        get_font: opt!(has.font, c, move || c.call::<(f32, Option<String>)>(Call::Font)),
        get_cursor: opt!(has.cursor, c, move || c.call::<Option<(usize, usize)>>(Call::Cursor)),
        get_ui_model: opt!(has.ui_model, c, move |v| c.call::<Option<ui::UiLayout>>(Call::UiModel(v))),
        on_ui_action: opt!(has.ui_action, c, move |a| c.call::<Option<()>>(Call::UiAction(a)).unwrap_or(())),
        get_status: opt!(has.status, c, move || c.call::<Option<String>>(Call::Status)),
        should_exit: opt!(has.should_exit, c, move || c.call::<bool>(Call::ShouldExit)),
        set_waker: Some(Arc::new(move |w| {
            let _ = waker.set(w);
        })),
        on_engine: {
            let c = c.clone();
            Some(Arc::new(move |e| c.call::<Option<()>>(Call::Engine(e)).unwrap_or(())))
        },
        on_window_size: {
            let c = c.clone();
            Some(Arc::new(move |w, h| c.call::<Option<()>>(Call::WindowSize(w, h)).unwrap_or(())))
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn img(key: u64, px: u8) -> ImageDraw {
        ImageDraw {
            key,
            rgba: Arc::new(vec![px; 4]),
            img_w: 1,
            img_h: 1,
            src: (0, 0, 1, 1),
            col: 0,
            row: 0,
            x_off: 0,
            y_off: 0,
            width: 1,
            height: 1,
            z: 0,
        }
    }

    #[test]
    fn image_pixels_cross_once_per_key() {
        let (mut sent, mut cache) = (HashSet::new(), HashMap::new());
        let a = fill_images(strip_images(vec![img(1, 7), img(2, 9)], &mut sent), &mut cache);
        assert_eq!((a[0].rgba.len(), a[1].rgba.len()), (4, 4));
        let wire = strip_images(vec![img(1, 7), img(2, 9)], &mut sent);
        assert!(wire.iter().all(|d| d.rgba.is_empty()), "second frame sends no pixels");
        let b = fill_images(wire, &mut cache);
        assert_eq!(*b[1].rgba, vec![9; 4]);
        // Key 1 scrolls away and comes back: sent again in full.
        fill_images(strip_images(vec![img(2, 9)], &mut sent), &mut cache);
        let wire = strip_images(vec![img(1, 7)], &mut sent);
        assert_eq!(wire[0].rgba.len(), 4);
    }

    #[test]
    fn messages_round_trip() {
        let (mut a, mut b) = UnixStream::pair().unwrap();
        send(&mut a, &Call::Resize(80, 24, 800, 600)).unwrap();
        assert!(matches!(recv::<Call>(&mut b).unwrap(), Call::Resize(80, 24, 800, 600)));
    }
}
