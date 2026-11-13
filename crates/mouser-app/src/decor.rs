//! Host side of decoration plugins (docs/plugins.md, docs/tabs.md). Every
//! enabled decoration runs. The scheduler in `mouser_core::decoration`
//! stacks their chrome and merges PTY ops. With none enabled, the built-in
//! sessions are unchanged.

use crate::tabs::{self, Tabs};
use crate::wake;
use mouser_core::decoration::{self, Chrome, DecorCtx, DecorEvent, Decoration, DecorationProc, PtyInfo, PtyKind, PtyOp, StartupPlan, TabsDecoration};
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::{self, Kind, Registry, Transport};
use mouser_core::scrollback::ScrollbackConfig;
use mouser_core::types::Size;
use mouser_render::ui::UiAction;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, Sender};
use std::thread;
use std::time::Instant;

/// One job for a decoration's worker. The worker owns the [`Decoration`] and
/// runs a single job at a time, so a slow plugin cannot re-enter itself.
enum Work {
    Chrome(DecorCtx),
    Event(DecorEvent, DecorCtx),
    Note(decoration::PtyNote, DecorCtx),
    /// The hit's owner runs the event and then the note. Everyone else runs the note.
    Hit {
        event: Option<DecorEvent>,
        note: decoration::PtyNote,
        ctx: DecorCtx,
    },
    Shutdown,
}

enum Reply {
    Chrome(decoration::Outcome<Chrome>),
    Ops(decoration::Outcome<Vec<PtyOp>>),
    Shutdown,
}

struct Envelope {
    index: usize,
    cancel: decoration::Cancel,
    work: Work,
    tx: Sender<(usize, Reply)>,
}

struct Slot {
    name: String,
    tx: Sender<Envelope>,
}

fn serve(mut driver: Box<dyn Decoration>, rx: Receiver<Envelope>) {
    while let Ok(env) = rx.recv() {
        if matches!(env.work, Work::Shutdown) {
            driver.shutdown();
            break;
        }
        let reply = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| match &env.work {
            Work::Chrome(ctx) => Reply::Chrome(finish_call(env.cancel.is_cancelled(), decoration::isolate_chrome(driver.as_mut(), ctx, &env.cancel))),
            Work::Event(ev, ctx) => {
                Reply::Ops(finish_call(env.cancel.is_cancelled(), decoration::isolate_event(driver.as_mut(), ev, ctx, &env.cancel)))
            }
            Work::Note(note, ctx) => {
                Reply::Ops(finish_call(env.cancel.is_cancelled(), decoration::isolate_note(driver.as_mut(), note, ctx, &env.cancel)))
            }
            Work::Hit { event, note, ctx } => {
                Reply::Ops(finish_call(env.cancel.is_cancelled(), hit_reply(driver.as_mut(), event.as_ref(), note, ctx, &env.cancel)))
            }
            Work::Shutdown => Reply::Shutdown,
        }));
        let reply = match reply {
            Ok(reply) => reply,
            Err(_) => match &env.work {
                Work::Chrome(_) => Reply::Chrome(decoration::Outcome::Failed),
                _ => Reply::Ops(decoration::Outcome::Failed),
            },
        };
        let _ = env.tx.send((env.index, reply));
    }
}

fn finish_call<T>(cancelled: bool, outcome: decoration::Outcome<T>) -> decoration::Outcome<T> {
    match outcome {
        decoration::Outcome::Done(_) if cancelled => decoration::Outcome::Cancelled,
        other => other,
    }
}

fn hit_reply(
    driver: &mut dyn Decoration,
    event: Option<&DecorEvent>,
    note: &decoration::PtyNote,
    ctx: &DecorCtx,
    cancel: &decoration::Cancel,
) -> decoration::Outcome<Vec<PtyOp>> {
    let mut ops = Vec::new();
    if let Some(ev) = event {
        match decoration::isolate_event(driver, ev, ctx, cancel) {
            decoration::Outcome::Done(v) => ops.extend(v),
            other => return other,
        }
    }
    match decoration::isolate_note(driver, note, ctx, cancel) {
        decoration::Outcome::Done(v) => {
            ops.extend(v);
            decoration::Outcome::Done(ops)
        }
        other => other,
    }
}

fn start_slot(name: String, driver: Box<dyn Decoration>) -> Slot {
    let (tx, rx) = mpsc::channel();
    let label = format!("mouser-decor-{name}");
    thread::Builder::new().name(label).spawn(move || serve(driver, rx)).expect("decoration worker");
    Slot { name, tx }
}

pub struct Decor {
    pub on: bool,
    slots: Vec<Slot>,
    plan: StartupPlan,
    extras_done: bool,
    /// PTY ids already reported with [`PtyNote::Exited`].
    told: Vec<u64>,
    next_call: u64,
}

impl Decor {
    /// `cli_default_shell`: this window would have started the login shell.
    pub fn load(cli_default_shell: bool) -> Self {
        let config = decoration::tabs_config();
        let reg = Registry::load();
        let listed: Vec<_> = reg.providers(Kind::Decoration).iter().map(|e| (e.name.clone(), e.dir.is_none(), e.transport)).collect();
        let mut slots = Vec::new();
        let mut tabs_plugin = false;
        for (name, builtin, transport) in listed {
            if plugins::is_quarantined(&name) {
                log::warn!("decoration {name} is quarantined; not loading it");
                continue;
            }
            let is_tabs = name == decoration::TABS_PLUGIN && builtin;
            let driver = if is_tabs {
                tabs_plugin = true;
                Some(Box::new(TabsDecoration::default()) as Box<dyn Decoration>)
            } else if transport == Transport::Decoration {
                open_process(&name)
            } else {
                log::warn!("decoration {name} has no host driver");
                None
            };
            if let Some(driver) = driver {
                slots.push(start_slot(name, driver));
            }
        }
        if slots.is_empty() {
            return Self::off();
        }
        Self {
            on: true,
            slots,
            plan: if tabs_plugin { decoration::startup_plan(&config, cli_default_shell) } else { StartupPlan::default() },
            extras_done: false,
            told: Vec::new(),
            next_call: 0,
        }
    }

    fn off() -> Self {
        Self { on: false, slots: Vec::new(), plan: StartupPlan::default(), extras_done: true, told: Vec::new(), next_call: 0 }
    }

    pub fn plan(&self) -> &StartupPlan {
        &self.plan
    }

    pub fn chrome(&mut self, ctx: &DecorCtx) -> Chrome {
        let parts = self.fan(
            ctx,
            |ctx| Work::Chrome(ctx.clone()),
            |reply| match reply {
                Reply::Chrome(outcome) => Some(outcome),
                Reply::Ops(_) | Reply::Shutdown => None,
            },
        );
        if !self.on {
            return Chrome::default();
        }
        let refs: Vec<(&str, &Chrome)> = parts.iter().map(|(name, chrome)| (name.as_str(), chrome)).collect();
        decoration::compose(ctx.width, ctx.height, &refs)
    }

    fn ops(&mut self, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp> {
        let parts = self.fan(ctx, |ctx| Work::Event(ev.clone(), ctx.clone()), ops_of);
        self.arbitrate_parts(&parts)
    }

    fn dispatch_hit(&mut self, owner: &str, id: &str, ctx: &DecorCtx) -> Vec<PtyOp> {
        let owner = if owner.is_empty() && self.slots.len() == 1 { self.slots[0].name.clone() } else { owner.to_string() };
        let note = decoration::PtyNote::ChromeHit { owner: owner.clone(), id: id.to_string() };
        let jobs: Vec<Work> = self
            .slots
            .iter()
            .map(|slot| Work::Hit {
                event: (slot.name == owner).then(|| DecorEvent::Hit { id: id.to_string() }),
                note: note.clone(),
                ctx: ctx.clone(),
            })
            .collect();
        let parts = self.fan_jobs(jobs, ops_of);
        self.arbitrate_parts(&parts)
    }

    /// Send `work` to every worker, then wait until each has replied or [`decoration::REPLY_TIMEOUT`]
    /// has passed since the send. Arrival order is ignored: the returned vec follows slot order.
    fn fan<T>(
        &mut self,
        ctx: &DecorCtx,
        work: impl Fn(&DecorCtx) -> Work,
        map: impl Fn(Reply) -> Option<decoration::Outcome<T>>,
    ) -> Vec<(String, T)> {
        let jobs: Vec<Work> = self.slots.iter().map(|_| work(ctx)).collect();
        self.fan_jobs(jobs, map)
    }

    fn fan_jobs<T>(&mut self, jobs: Vec<Work>, map: impl Fn(Reply) -> Option<decoration::Outcome<T>>) -> Vec<(String, T)> {
        self.drop_quarantined();
        let cancel = decoration::Cancel::new(self.alloc_call());
        let (tx, rx) = mpsc::channel();
        let mut waiting = 0;
        let n = self.slots.len().min(jobs.len());
        for (index, (slot, work)) in self.slots.iter().zip(jobs).enumerate() {
            let env = Envelope { index, cancel: cancel.clone(), work, tx: tx.clone() };
            if slot.tx.send(env).is_ok() {
                waiting += 1;
            }
        }
        drop(tx);
        let answers = wait_replies(rx, n, waiting, &cancel);
        let mut dead = Vec::new();
        let mut parts = Vec::new();
        for (i, answer) in answers.into_iter().enumerate() {
            let name = self.slots[i].name.clone();
            match answer.and_then(&map) {
                Some(decoration::Outcome::Done(value)) => {
                    plugins::note_decoration_ok(&name);
                    parts.push((name, value));
                }
                Some(decoration::Outcome::Cancelled) => {
                    if plugins::note_decoration_miss(&name) {
                        log::warn!(
                            "decoration {name} quarantined after {} consecutive deadline misses (3 retries); `mouser plugin unquarantine {name}` releases it",
                            decoration::DEADLINE_RETRIES + 1
                        );
                        dead.push(i);
                    }
                }
                Some(decoration::Outcome::Ignored) => {
                    let quarantined = plugins::note_decoration_miss(&name);
                    if quarantined {
                        log::warn!("decoration {name} quarantined after ignoring the deadline; `mouser plugin unquarantine {name}` releases it");
                    } else {
                        log::warn!("decoration {name} ignored the cancel and was stopped for this session");
                    }
                    dead.push(i);
                }
                _ => {
                    if plugins::note_decoration_miss(&name) {
                        log::warn!("decoration {name} quarantined after missing the deadline; `mouser plugin unquarantine {name}` releases it");
                    } else {
                        log::warn!("decoration {name} missed the deadline; the others keep running");
                    }
                    dead.push(i);
                }
            }
        }
        for i in dead.into_iter().rev() {
            self.drop_slot(i);
        }
        self.on = !self.slots.is_empty();
        parts
    }

    fn alloc_call(&mut self) -> u64 {
        self.next_call = self.next_call.wrapping_add(1);
        self.next_call
    }

    fn drop_quarantined(&mut self) {
        let mut i = 0;
        while i < self.slots.len() {
            if plugins::is_quarantined(&self.slots[i].name) {
                let name = self.slots[i].name.clone();
                self.drop_slot(i);
                log::warn!("decoration {name} is quarantined; removed from this window");
            } else {
                i += 1;
            }
        }
    }

    fn arbitrate_parts(&self, parts: &[(String, Vec<PtyOp>)]) -> Vec<PtyOp> {
        let refs: Vec<(&str, &[PtyOp])> = parts.iter().map(|(name, ops)| (name.as_str(), ops.as_slice())).collect();
        decoration::arbitrate(&refs)
    }

    fn drop_slot(&mut self, index: usize) {
        let (gone, _) = mpsc::channel();
        let _ = self.slots[index].tx.send(Envelope { index, cancel: decoration::Cancel::never(), work: Work::Shutdown, tx: gone });
        self.slots.remove(index);
    }

    /// PTYs configured as extra tabs. Once per window, after the grid size is known.
    pub fn take_extras(&mut self) -> Vec<PtyOp> {
        if self.extras_done || !self.on {
            return Vec::new();
        }
        self.extras_done = true;
        self.plan.extra.clone()
    }
}

fn ops_of(reply: Reply) -> Option<decoration::Outcome<Vec<PtyOp>>> {
    match reply {
        Reply::Ops(outcome) => Some(outcome),
        Reply::Chrome(_) | Reply::Shutdown => None,
    }
}

fn wait_replies(rx: Receiver<(usize, Reply)>, n: usize, mut waiting: usize, cancel: &decoration::Cancel) -> Vec<Option<Reply>> {
    let mut out = (0..n).map(|_| None).collect::<Vec<_>>();
    let deadline = Instant::now() + decoration::REPLY_TIMEOUT;
    let mut grace_end: Option<Instant> = None;
    while waiting > 0 {
        let remain = match grace_end {
            None => deadline.saturating_duration_since(Instant::now()),
            Some(end) => end.saturating_duration_since(Instant::now()),
        };
        if remain.is_zero() {
            if grace_end.is_none() {
                cancel.cancel();
                grace_end = Some(Instant::now() + decoration::KILL_GRACE + std::time::Duration::from_millis(30));
                continue;
            }
            break;
        }
        match rx.recv_timeout(remain) {
            Ok((i, reply)) if i < n && out[i].is_none() => {
                out[i] = Some(reply);
                waiting -= 1;
            }
            Ok(_) => {}
            Err(RecvTimeoutError::Timeout) | Err(RecvTimeoutError::Disconnected) => {
                if grace_end.is_none() {
                    cancel.cancel();
                    grace_end = Some(Instant::now() + decoration::KILL_GRACE + std::time::Duration::from_millis(30));
                    continue;
                }
                break;
            }
        }
    }
    out
}

fn open_process(name: &str) -> Option<Box<dyn Decoration>> {
    let inst = plugins::find(name)?;
    if Store::load().get(name, "run") != Decision::Allow {
        log::warn!("decoration {name} needs `mouser plugin permissions {name} run=allow` before it can run");
        return None;
    }
    match DecorationProc::spawn(&inst.argv(), &inst.dir) {
        Ok(p) => Some(Box::new(p)),
        Err(e) => {
            log::warn!("decoration {name}: {e:#}");
            None
        }
    }
}

pub fn pty_ctx(tabs: &Tabs) -> DecorCtx {
    let ptys = tabs
        .list
        .iter()
        .map(|s| {
            let (kind, command, hold) = match &s.launch {
                tabs::Launch::Shell => (PtyKind::Shell, Vec::new(), false),
                tabs::Launch::Command(c) => (PtyKind::Command, c.clone(), false),
                tabs::Launch::Exec { argv, hold } => (PtyKind::Exec, argv.clone(), *hold),
                tabs::Launch::Handoff => (PtyKind::Handoff, Vec::new(), false),
            };
            PtyInfo {
                id: s.id,
                title: s.title(),
                shell: s.shell_program.lock().unwrap().clone().unwrap_or_default(),
                command,
                kind,
                hold,
                exited: *s.exited.lock().unwrap(),
            }
        })
        .collect();
    DecorCtx { ptys, focused: tabs.active().map(|s| s.id), ..DecorCtx::default() }
}

pub fn apply(tabs: &mut Tabs, op: PtyOp, sb: &ScrollbackConfig, wake: fn()) {
    match op {
        PtyOp::Spawn { shell, command, title } => {
            let launch = if command.is_empty() { tabs::Launch::Shell } else { tabs::Launch::Command(command) };
            let s = tabs::Session::new(title, launch, Size { cols: 80, rows: 24 }, sb.clone());
            if let Some(program) = shell.filter(|p| !p.is_empty()) {
                *s.shell_program.lock().unwrap() = Some(program);
            }
            tabs.open(s, wake);
        }
        PtyOp::Close { id } => {
            if let Some(i) = tabs.list.iter().position(|s| s.id == id) {
                tabs.close(i);
                let i = tabs.active;
                tabs.focus(i, wake);
            }
        }
        PtyOp::Focus { id } => {
            if let Some(i) = tabs.list.iter().position(|s| s.id == id) {
                tabs.focus(i, wake);
            }
        }
    }
}

/// `true` when the decoration handled `act` (tab keys and clicks).
pub fn on_action(decor: &mut Decor, tabs: &mut Tabs, act: &UiAction, sb: &ScrollbackConfig) -> bool {
    if !decor.on {
        return false;
    }
    let ev = match act {
        UiAction::NewTab => DecorEvent::NewTab { shell: None },
        UiAction::CloseActiveTab => {
            let Some(id) = tabs.active().map(|s| s.id) else { return true };
            DecorEvent::CloseTab { id }
        }
        UiAction::CloseTab(i) => {
            let Some(s) = tabs.list.get(*i) else { return true };
            DecorEvent::CloseTab { id: s.id }
        }
        UiAction::SelectTab(i) => {
            let Some(s) = tabs.list.get(*i) else { return true };
            DecorEvent::Focus { id: s.id }
        }
        UiAction::CycleTab(d) => DecorEvent::Cycle { delta: *d },
        _ => return false,
    };
    let before = focused_id(tabs);
    let ctx = pty_ctx(tabs);
    let ops = decor.ops(&ev, &ctx);
    for op in ops {
        apply(tabs, op, sb, wake);
    }
    note_focus(decor, tabs, before, sb);
    true
}

pub fn on_hit(decor: &mut Decor, tabs: &mut Tabs, packed: &str, sb: &ScrollbackConfig) {
    if !decor.on {
        return;
    }
    let before = focused_id(tabs);
    let (owner, id) = decoration::unpack_hit(packed);
    let ctx = pty_ctx(tabs);
    let ops = decor.dispatch_hit(&owner, &id, &ctx);
    for op in ops {
        apply(tabs, op, sb, wake);
    }
    note_focus(decor, tabs, before, sb);
}

fn focused_id(tabs: &Tabs) -> Option<u64> {
    tabs.active().map(|s| s.id)
}

fn note_focus(decor: &mut Decor, tabs: &mut Tabs, before: Option<u64>, sb: &ScrollbackConfig) {
    let now = focused_id(tabs);
    if now != before
        && let Some(id) = now
    {
        on_note(decor, tabs, &decoration::PtyNote::Focused { id }, sb);
    }
}

/// Reap closed tabs. When a decoration is on, tell it about each exit once
/// (so it can focus a neighbour) and resize the tab that remains in front.
/// Tabs the session itself marked closed are still removed.
pub fn reap_window(tabs: &mut Tabs, decor: &mut Decor, sb: &ScrollbackConfig) -> bool {
    if decor.on {
        let pending: Vec<(u64, u32)> = tabs
            .list
            .iter()
            .filter_map(|s| {
                let code = *s.exited.lock().unwrap();
                let dead = code.is_some() || s.closed.load(std::sync::atomic::Ordering::SeqCst);
                dead.then_some((s.id, code.unwrap_or(0)))
            })
            .filter(|(id, _)| !decor.told.contains(id))
            .collect();
        for (id, code) in pending {
            decor.told.push(id);
            let note = decoration::PtyNote::Exited { id, code };
            on_note(decor, tabs, &note, sb);
        }
    }
    let empty = tabs.reap();
    if decor.on && !empty {
        let i = tabs.active;
        tabs.focus(i, wake);
    }
    empty
}

pub fn on_note(decor: &mut Decor, tabs: &mut Tabs, note: &decoration::PtyNote, sb: &ScrollbackConfig) {
    if !decor.on {
        return;
    }
    let before = focused_id(tabs);
    let ctx = pty_ctx(tabs);
    let parts = decor.fan(&ctx, |ctx| Work::Note(note.clone(), ctx.clone()), ops_of);
    for op in decor.arbitrate_parts(&parts) {
        apply(tabs, op, sb, wake);
    }
    if !matches!(note, decoration::PtyNote::Focused { .. }) {
        note_focus(decor, tabs, before, sb);
    }
}

/// Shell program for the first PTY, when the plan replaces the login shell.
pub fn first_shell(plan: &StartupPlan) -> Option<String> {
    plan.first.as_ref().filter(|f| f.command.is_empty()).and_then(|f| f.shell.clone())
}

pub fn first_command(plan: &StartupPlan) -> Option<(String, Vec<String>)> {
    plan.first.as_ref().filter(|f| !f.command.is_empty()).map(|f| (f.title.clone(), f.command.clone()))
}

/// Keep a decoration alive across hook callbacks.
pub type Shared = Arc<std::sync::Mutex<Decor>>;

#[cfg(test)]
mod tests {
    use super::*;
    use mouser_core::decoration::{PtyNote, TabSpec, TabsConfig};

    fn decor_on(config: TabsConfig, cli_shell: bool) -> Decor {
        Decor {
            on: true,
            slots: vec![start_slot(decoration::TABS_PLUGIN.into(), Box::new(TabsDecoration { pinned: Some(config.clone()) }))],
            plan: decoration::startup_plan(&config, cli_shell),
            extras_done: false,
            told: Vec::new(),
            next_call: 0,
        }
    }

    #[test]
    fn actions_open_a_configured_shell_close_it_and_resize_the_one_in_front() {
        let config = TabsConfig {
            shell: Some("/bin/sh".into()),
            tab: vec![TabSpec { title: Some("extra".into()), shell: Some("/bin/sh".into()), command: vec![] }],
        };
        let mut decor = decor_on(config, true);
        assert_eq!(decor.plan.first.as_ref().unwrap().title, "extra");
        let mut tabs = Tabs::default();
        let first = tabs::Session::new("extra", tabs::Launch::Shell, Size { cols: 10, rows: 4 }, ScrollbackConfig::default());
        *first.shell_program.lock().unwrap() = Some("/bin/sh".into());
        tabs.open(first, || {});
        let sb = ScrollbackConfig::default();
        assert!(on_action(&mut decor, &mut tabs, &UiAction::NewTab, &sb));
        assert_eq!(tabs.list.len(), 2);
        assert_eq!(tabs.list[1].shell_program.lock().unwrap().as_deref(), Some("/bin/sh"));
        assert!(matches!(tabs.list[1].launch, tabs::Launch::Shell));
        let id = tabs.list[0].id;
        assert!(on_action(&mut decor, &mut tabs, &UiAction::CloseTab(0), &sb));
        assert!(tabs.list.iter().all(|s| s.id != id));
        assert_eq!(tabs.list.len(), 1);
        // A second shell, then only the focused one takes the new size.
        assert!(on_action(&mut decor, &mut tabs, &UiAction::NewTab, &sb));
        tabs.resize_active(30, 10, 200, 160, || {});
        let active_id = tabs.active().unwrap().id;
        on_hit(&mut decor, &mut tabs, &format!("focus:{active_id}"), &sb);
        let hidden = tabs.list.iter().find(|s| s.id != active_id).unwrap();
        // Neither has been started (resize_active starts only the active one).
        assert!(tabs.active().unwrap().parser.lock().unwrap().screen.size.cols >= 10);
        let _ = hidden;
        let focused = tabs.list.iter().position(|s| s.id == active_id).unwrap();
        // Park on the other tab without a stored mismatch, then grow only the active.
        tabs.resize(20, 8, 100, 80, || {});
        tabs.focus(focused, || {});
        tabs.resize_active(36, 14, 300, 200, || {});
        let active_size = tabs.active().unwrap().parser.lock().unwrap().screen.size;
        assert_eq!(active_size, Size { cols: 36, rows: 14 });
        let other = tabs.list.iter().find(|s| s.id != tabs.active().unwrap().id).unwrap();
        assert_eq!(other.parser.lock().unwrap().screen.size, Size { cols: 20, rows: 8 });
        // Shell exit asks the decoration to close that tab and stay on the other.
        let gone = tabs.active().unwrap().id;
        on_note(&mut decor, &mut tabs, &PtyNote::Exited { id: gone, code: 0 }, &sb);
        assert!(tabs.list.iter().all(|s| s.id != gone));
        assert_eq!(tabs.list.len(), 1);
        for s in &tabs.list {
            s.kill();
        }
    }

    #[test]
    fn a_failed_command_stays_until_a_key() {
        let mut decor = decor_on(TabsConfig::default(), true);
        let mut tabs = Tabs::default();
        let s = tabs::Session::new("make", tabs::Launch::Command(vec!["make".into()]), Size { cols: 8, rows: 4 }, ScrollbackConfig::default());
        s.finish(2);
        tabs.open(s, || {});
        let sb = ScrollbackConfig::default();
        let id = tabs.list[0].id;
        on_note(&mut decor, &mut tabs, &PtyNote::Exited { id, code: 2 }, &sb);
        assert_eq!(tabs.list.len(), 1);
        let ctx = pty_ctx(&tabs);
        assert!(ctx.ptys[0].exited == Some(2));
        let ops = decor.ops(&DecorEvent::KeyOnExited, &ctx);
        for op in ops {
            apply(&mut tabs, op, &sb, || {});
        }
        assert!(tabs.list.iter().all(|s| s.id != id));
    }

    struct Boom;

    impl Decoration for Boom {
        fn chrome(&mut self, _: &DecorCtx, _: &decoration::Cancel) -> Chrome {
            panic!("boom");
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            panic!("boom");
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            panic!("boom");
        }
    }

    struct SpawnBand;

    impl Decoration for SpawnBand {
        fn chrome(&mut self, _: &DecorCtx, _: &decoration::Cancel) -> Chrome {
            Chrome { bottom: 6, ..Chrome::default() }
        }
        fn event(&mut self, ev: &DecorEvent, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            match ev {
                DecorEvent::NewTab { .. } => vec![PtyOp::Spawn { shell: None, command: vec!["sleep".into(), "30".into()], title: "nap".into() }],
                _ => Vec::new(),
            }
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
    }

    #[test]
    fn a_crashing_decoration_is_dropped_and_the_other_still_runs() {
        let mut decor = Decor {
            on: true,
            slots: vec![start_slot("boom".into(), Box::new(Boom)), start_slot("band".into(), Box::new(SpawnBand))],
            plan: StartupPlan::default(),
            extras_done: true,
            told: Vec::new(),
            next_call: 0,
        };
        let chrome = decor.chrome(&DecorCtx::default());
        assert!(decor.on);
        assert_eq!(decor.slots.len(), 1);
        assert_eq!(chrome.bottom, 6);
        let mut tabs = Tabs::default();
        assert!(on_action(&mut decor, &mut tabs, &UiAction::NewTab, &ScrollbackConfig::default()));
        assert_eq!(tabs.list.len(), 1);
        assert_eq!(tabs.list[0].title(), "nap");
    }

    struct Pause {
        ms: u64,
        top: i32,
        color: u32,
    }

    impl Decoration for Pause {
        fn chrome(&mut self, _: &DecorCtx, cancel: &decoration::Cancel) -> Chrome {
            let step = std::time::Duration::from_millis(5);
            let mut left = self.ms;
            while left > 0 {
                if cancel.is_cancelled() {
                    return Chrome::default();
                }
                let slice = left.min(5);
                thread::sleep(step);
                left -= slice;
            }
            Chrome {
                top: self.top,
                ops: vec![decoration::DrawOp::Fill { x: 0, y: 0, w: 10, h: self.top.max(1), color: self.color }],
                ..Chrome::default()
            }
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
    }

    fn sched(slots: Vec<Slot>) -> Decor {
        Decor { on: true, slots, plan: StartupPlan::default(), extras_done: true, told: Vec::new(), next_call: 0 }
    }

    fn fill_at(ops: &[decoration::DrawOp], color: u32) -> (i32, i32) {
        ops.iter()
            .find_map(|op| match op {
                decoration::DrawOp::Fill { y, h, color: c, .. } if *c == color => Some((*y, *h)),
                _ => None,
            })
            .unwrap()
    }

    #[test]
    fn two_slow_decorations_finish_together_in_priority_order() {
        let mut decor = sched(vec![
            start_slot("high".into(), Box::new(Pause { ms: 80, top: 10, color: 1 })),
            start_slot("low".into(), Box::new(Pause { ms: 80, top: 8, color: 2 })),
        ]);
        let started = Instant::now();
        let chrome = decor.chrome(&DecorCtx { width: 80, height: 40, ..DecorCtx::default() });
        let elapsed = started.elapsed();
        // Serial would be ~160 ms. 150 ms is still under that sum and leaves room for scheduling jitter.
        assert!(elapsed < std::time::Duration::from_millis(150), "parallel broadcast took {elapsed:?}");
        assert_eq!(chrome.top, 18);
        assert_eq!(decor.slots.len(), 2);
        assert_eq!(fill_at(&chrome.ops, 1), (0, 10));
        assert_eq!(fill_at(&chrome.ops, 2), (10, 8));
    }

    /// Sleeps through the cancel flag. The host drops it after the grace period.
    struct Stubborn {
        ms: u64,
    }

    impl Decoration for Stubborn {
        fn chrome(&mut self, _: &DecorCtx, _: &decoration::Cancel) -> Chrome {
            thread::sleep(std::time::Duration::from_millis(self.ms));
            Chrome { top: 30, ..Chrome::default() }
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
    }

    #[test]
    fn a_hung_decoration_is_dropped_at_the_deadline_and_the_other_chrome_applies() {
        let mut decor = sched(vec![
            start_slot("hung".into(), Box::new(Stubborn { ms: 400 })),
            start_slot("fast".into(), Box::new(Pause { ms: 0, top: 5, color: 3 })),
        ]);
        let started = Instant::now();
        let chrome = decor.chrome(&DecorCtx { width: 80, height: 40, ..DecorCtx::default() });
        let elapsed = started.elapsed();
        assert!(elapsed >= std::time::Duration::from_millis(100), "returned in {elapsed:?}, before the deadline");
        assert!(elapsed < std::time::Duration::from_millis(350), "stalled {elapsed:?}");
        assert_eq!(chrome.top, 5);
        assert_eq!(decor.slots.len(), 1);
        assert_eq!(decor.slots[0].name, "fast");
        assert_eq!(fill_at(&chrome.ops, 3), (0, 5));
    }

    struct Watch {
        calls: Arc<std::sync::atomic::AtomicU64>,
        second_cancelled: Arc<std::sync::atomic::AtomicBool>,
    }

    impl Decoration for Watch {
        fn chrome(&mut self, _: &DecorCtx, cancel: &decoration::Cancel) -> Chrome {
            let n = self.calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
            if n == 0 {
                while !cancel.is_cancelled() {
                    thread::sleep(std::time::Duration::from_millis(5));
                }
                return Chrome::default();
            }
            self.second_cancelled.store(cancel.is_cancelled(), std::sync::atomic::Ordering::Relaxed);
            Chrome { top: 6, ..Chrome::default() }
        }
        fn event(&mut self, _: &DecorEvent, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
        fn note(&mut self, _: &PtyNote, _: &DecorCtx, _: &decoration::Cancel) -> Vec<PtyOp> {
            Vec::new()
        }
    }

    #[test]
    fn a_cancel_stops_the_call_and_does_not_cancel_the_next_one() {
        let calls = Arc::new(std::sync::atomic::AtomicU64::new(0));
        let second_cancelled = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let mut decor = sched(vec![start_slot("watch".into(), Box::new(Watch { calls: calls.clone(), second_cancelled: second_cancelled.clone() }))]);
        let started = Instant::now();
        let first = decor.chrome(&DecorCtx { width: 40, height: 20, ..DecorCtx::default() });
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_millis(280), "cancel took {elapsed:?}");
        assert_eq!(first.top, 0);
        assert_eq!(decor.slots.len(), 1, "a decoration that stops on cancel stays loaded");
        let second = decor.chrome(&DecorCtx { width: 40, height: 20, ..DecorCtx::default() });
        assert_eq!(second.top, 6);
        assert!(!second_cancelled.load(std::sync::atomic::Ordering::Relaxed));
        assert_eq!(calls.load(std::sync::atomic::Ordering::Relaxed), 2);
    }

    #[cfg(unix)]
    fn external(dir: &std::path::Path, name: &str, body: &str) -> Slot {
        use std::os::unix::fs::PermissionsExt;
        let path = dir.join(name);
        std::fs::write(&path, body).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        let proc = decoration::DecorationProc::spawn(&[path.display().to_string()], dir).unwrap();
        start_slot(name.into(), Box::new(proc))
    }

    #[cfg(unix)]
    #[test]
    fn an_external_decoration_stops_on_kill_and_the_next_call_still_runs() {
        let dir = tempfile::tempdir().unwrap();
        let body = r#"#!/bin/bash
n=0
while IFS= read -r line; do
  case "$line" in
    *'"op":"hello"'*) printf '%s\n' '{}' ;;
    *'"op":"kill"'*) printf '%s\n' '{"cancelled":true}' ;;
    *)
      n=$((n + 1))
      if [ "$n" -eq 1 ]; then
        if IFS= read -t 0.25 -r extra; then
          case "$extra" in
            *'"op":"kill"'*) printf '%s\n' '{"cancelled":true}' ;;
          esac
        fi
      else
        printf '%s\n' '{"chrome":{"top":4}}'
      fi
      ;;
  esac
done
"#;
        let mut decor = sched(vec![external(dir.path(), "stop", body)]);
        let started = Instant::now();
        let first = decor.chrome(&DecorCtx { width: 20, height: 10, ..DecorCtx::default() });
        assert!(started.elapsed() < std::time::Duration::from_millis(350), "{:?}", started.elapsed());
        assert_eq!(first.top, 0);
        assert_eq!(decor.slots.len(), 1);
        let second = decor.chrome(&DecorCtx { width: 20, height: 10, ..DecorCtx::default() });
        assert_eq!(second.top, 4);
    }

    #[cfg(unix)]
    #[test]
    fn an_external_decoration_that_ignores_kill_is_stopped() {
        let dir = tempfile::tempdir().unwrap();
        let slow = r#"#!/bin/sh
while IFS= read -r line; do
  case "$line" in
    *'"op":"hello"'*) printf '%s\n' '{}' ;;
    *) sleep 2; printf '%s\n' '{"chrome":{"top":9}}' ;;
  esac
done
"#;
        let mut decor = sched(vec![external(dir.path(), "ignore", slow), start_slot("fast".into(), Box::new(Pause { ms: 0, top: 5, color: 3 }))]);
        let started = Instant::now();
        let chrome = decor.chrome(&DecorCtx { width: 40, height: 20, ..DecorCtx::default() });
        let elapsed = started.elapsed();
        assert!(elapsed < std::time::Duration::from_millis(350), "waited {elapsed:?}");
        assert_eq!(chrome.top, 5);
        assert_eq!(decor.slots.len(), 1);
        assert_eq!(decor.slots[0].name, "fast");
    }
}
