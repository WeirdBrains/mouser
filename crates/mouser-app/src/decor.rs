//! Host side of a decoration plugin (docs/tabs.md). One decoration runs per
//! window: the first enabled one in plugin order. It draws the chrome and
//! decides which PTYs exist. With none enabled, the built-in sessions are
//! unchanged.

use crate::tabs::{self, Tabs};
use crate::wake;
use mouser_core::decoration::{self, Chrome, DecorCtx, DecorEvent, Decoration, DecorationProc, PtyInfo, PtyKind, PtyOp, StartupPlan, TabsDecoration};
use mouser_core::permissions::{Decision, Store};
use mouser_core::plugins::{self, Kind, Registry, Transport};
use mouser_core::scrollback::ScrollbackConfig;
use mouser_core::types::Size;
use mouser_render::ui::UiAction;
use std::sync::Arc;

pub struct Decor {
    pub on: bool,
    driver: Option<Box<dyn Decoration>>,
    plan: StartupPlan,
    extras_done: bool,
    /// PTY ids already reported with [`PtyNote::Exited`].
    told: Vec<u64>,
}

impl Decor {
    /// `cli_default_shell`: this window would have started the login shell.
    pub fn load(cli_default_shell: bool) -> Self {
        let config = decoration::tabs_config();
        let reg = Registry::load();
        let Some(entry) = reg.providers(Kind::Decoration).into_iter().next() else {
            return Self::off();
        };
        let tabs_plugin = entry.name == decoration::TABS_PLUGIN && entry.dir.is_none();
        let driver = if tabs_plugin {
            Some(Box::new(TabsDecoration::default()) as Box<dyn Decoration>)
        } else if entry.transport == Transport::Decoration {
            open_process(&entry.name)
        } else {
            log::warn!("decoration {} has no host driver", entry.name);
            None
        };
        match driver {
            Some(driver) => Self {
                on: true,
                driver: Some(driver),
                plan: if tabs_plugin { decoration::startup_plan(&config, cli_default_shell) } else { StartupPlan::default() },
                extras_done: false,
                told: Vec::new(),
            },
            None => Self::off(),
        }
    }

    fn off() -> Self {
        Self { on: false, driver: None, plan: StartupPlan::default(), extras_done: true, told: Vec::new() }
    }

    pub fn plan(&self) -> &StartupPlan {
        &self.plan
    }

    pub fn chrome(&mut self, ctx: &DecorCtx) -> Chrome {
        let Some(driver) = self.driver.as_mut() else { return Chrome::default() };
        let chrome = driver.chrome(ctx);
        if driver.failed() {
            log::warn!("decoration stopped; this window keeps the built-in sessions");
            self.on = false;
            self.driver = None;
            return Chrome::default();
        }
        chrome
    }

    fn ops(&mut self, ev: &DecorEvent, ctx: &DecorCtx) -> Vec<PtyOp> {
        let Some(driver) = self.driver.as_mut() else { return Vec::new() };
        let ops = driver.event(ev, ctx);
        if driver.failed() {
            self.on = false;
            self.driver = None;
        }
        ops
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
    let ctx = pty_ctx(tabs);
    let ops = decor.ops(&ev, &ctx);
    for op in ops {
        apply(tabs, op, sb, wake);
    }
    true
}

pub fn on_hit(decor: &mut Decor, tabs: &mut Tabs, id: &str, sb: &ScrollbackConfig) {
    if !decor.on {
        return;
    }
    let ctx = pty_ctx(tabs);
    let ops = decor.ops(&DecorEvent::Hit { id: id.to_string() }, &ctx);
    for op in ops {
        apply(tabs, op, sb, wake);
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
    let ctx = pty_ctx(tabs);
    let Some(driver) = decor.driver.as_mut() else { return };
    let ops = driver.note(note, &ctx);
    if driver.failed() {
        decor.on = false;
        decor.driver = None;
    }
    for op in ops {
        apply(tabs, op, sb, wake);
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
            driver: Some(Box::new(TabsDecoration { pinned: Some(config.clone()) })),
            plan: decoration::startup_plan(&config, cli_shell),
            extras_done: false,
            told: Vec::new(),
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
}
