//! The host side: start a plugin, run the handshake, pump its messages, and
//! restart it after a crash with back-off ([`crate::supervisor`]). Mouser
//! owns one [`Supervisor`] per enabled plugin. Nothing here touches a shell:
//! shells live in the session server, so a plugin crash never affects them.

use crate::codec::{self, Wire};
use crate::mode::Context;
use crate::protocol::{Accepted, ApiRange, Hello, In, Out, Welcome, negotiate};
use crate::shm::Ring;
use crate::supervisor::{Decision, Restarts};
use std::io::{BufReader, BufWriter};
use std::path::PathBuf;
use std::process::{Child, ChildStdin, Command, Stdio};
use std::sync::Arc;
use std::sync::mpsc::{Receiver, TryRecvError, channel};
use std::time::{Duration, Instant};

/// How to start a plugin.
#[derive(Debug, Clone)]
pub struct Spec {
    pub name: String,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub env: Vec<(String, String)>,
    /// Shared-memory ring: slots and bytes per slot (`None`: inline images).
    pub shm: Option<(u32, usize)>,
}

/// What Mouser grants a plugin after reading its `Hello`.
#[derive(Debug, Clone, Default)]
pub struct Grants {
    pub granted: Vec<String>,
    pub denied: Vec<(String, String)>,
}

/// A running, handshaken plugin.
pub struct Connection {
    child: Child,
    tx: BufWriter<ChildStdin>,
    rx: Receiver<Out>,
    pub hello: Hello,
    pub api: u32,
    pub ring: Option<Ring>,
}

/// Called from the reader thread when a message arrives (wake the UI).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

/// How long the plugin may take to say hello.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);

/// Start `spec`, read its `Hello`, negotiate and answer. `grant` decides
/// permissions; `restore` is the state it saved before a crash.
pub fn connect(
    spec: &Spec,
    ctx: Context,
    restore: Option<Vec<u8>>,
    grant: &mut dyn FnMut(&Hello) -> Grants,
    notify: Option<Notify>,
) -> Result<Connection, String> {
    let mut cmd = Command::new(&spec.program);
    cmd.args(&spec.args).envs(spec.env.iter().map(|(k, v)| (k, v))).stdin(Stdio::piped()).stdout(Stdio::piped()).stderr(Stdio::inherit());
    let mut child = cmd.spawn().map_err(|e| format!("can't start {}: {e}", spec.program.display()))?;
    let mut tx = BufWriter::new(child.stdin.take().ok_or("no stdin")?);
    let mut out = BufReader::with_capacity(1 << 20, child.stdout.take().ok_or("no stdout")?);
    let (send, rx) = channel();
    std::thread::Builder::new()
        .name(format!("plugin-{}", spec.name))
        .spawn(move || {
            let mut buf = Vec::new();
            while let Ok(Some(m)) = codec::read::<Out>(&mut out, Wire::MsgPack, &mut buf) {
                if send.send(m).is_err() {
                    break;
                }
                if let Some(n) = &notify {
                    n();
                }
            }
            // The plugin is gone: let the UI notice now.
            drop(send);
            if let Some(n) = &notify {
                n();
            }
        })
        .map_err(|e| e.to_string())?;
    let fail = |mut child: Child, why: String| {
        let _ = child.kill();
        let _ = child.wait();
        Err(why)
    };
    let hello = match rx.recv_timeout(HELLO_TIMEOUT) {
        Ok(Out::Hello(h)) => h,
        Ok(_) => return fail(child, "plugin spoke before saying hello".into()),
        Err(_) => return fail(child, format!("{} sent no hello", spec.name)),
    };
    let api = match negotiate(ApiRange::CURRENT, hello.api) {
        Ok(a) => a,
        Err(reason) => {
            let _ = codec::write(&mut tx, Wire::MsgPack, &Welcome::Rejected { reason: reason.clone() });
            return fail(child, reason);
        }
    };
    let g = grant(&hello);
    let ring = match (spec.shm, hello.shm) {
        (Some((slots, bytes)), true) => Ring::create(&spec.name, slots, bytes).ok(),
        _ => None,
    };
    let accepted = Accepted {
        api,
        session: format!("{}-{}", spec.name, child.id()),
        ctx,
        granted: g.granted,
        denied: g.denied,
        shm: ring.as_ref().map(Ring::spec),
        mcp: None,
        restore,
    };
    if let Err(e) = codec::write(&mut tx, Wire::MsgPack, &Welcome::Accepted(Box::new(accepted))) {
        return fail(child, e.to_string());
    }
    Ok(Connection { child, tx, rx, hello, api, ring })
}

impl Connection {
    pub fn send(&mut self, m: &In) -> std::io::Result<()> {
        codec::write(&mut self.tx, Wire::MsgPack, m)
    }

    /// Ask it to stop, then make sure it has.
    pub fn stop(mut self, reason: &str, grace: Duration) {
        let _ = self.send(&In::Stopping { reason: reason.into() });
        let end = Instant::now() + grace;
        while Instant::now() < end {
            if let Ok(Some(_)) = self.child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        let _ = self.child.kill();
        let _ = self.child.wait();
    }
}

impl Drop for Connection {
    fn drop(&mut self) {
        if let Ok(None) = self.child.try_wait() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

/// Where a supervised plugin is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum State {
    Running,
    /// Crashed; starting again at this time.
    Waiting {
        until: Instant,
        crashes: u32,
    },
    /// Gave up after too many crashes (or it was incompatible): stays off
    /// until the user restarts it.
    Disabled {
        reason: String,
    },
}

/// One plugin, kept running.
pub struct Supervisor {
    pub spec: Spec,
    ctx: Context,
    grant: Box<dyn FnMut(&Hello) -> Grants + Send>,
    restarts: Restarts,
    conn: Option<Connection>,
    state: State,
    saved: Option<Vec<u8>>,
    crashes: u32,
    notify: Option<Notify>,
}

impl Supervisor {
    /// `max_restarts` is the `plugins.max_restarts` preference.
    pub fn new(spec: Spec, ctx: Context, max_restarts: u32, grant: impl FnMut(&Hello) -> Grants + Send + 'static) -> Self {
        Self::with_restarts(spec, ctx, Restarts::new(max_restarts), grant)
    }

    pub fn with_restarts(spec: Spec, ctx: Context, restarts: Restarts, grant: impl FnMut(&Hello) -> Grants + Send + 'static) -> Self {
        let mut s = Self {
            spec,
            ctx,
            grant: Box::new(grant),
            restarts,
            conn: None,
            state: State::Waiting { until: Instant::now(), crashes: 0 },
            saved: None,
            crashes: 0,
            notify: None,
        };
        s.start(Instant::now());
        s
    }

    /// Like [`Supervisor::new`], with a callback for arriving messages; it is
    /// set before the first start.
    pub fn with_notify(spec: Spec, ctx: Context, restarts: Restarts, notify: Notify, grant: impl FnMut(&Hello) -> Grants + Send + 'static) -> Self {
        let mut s = Self {
            spec,
            ctx,
            grant: Box::new(grant),
            restarts,
            conn: None,
            state: State::Waiting { until: Instant::now(), crashes: 0 },
            saved: None,
            crashes: 0,
            notify: Some(notify),
        };
        s.start(Instant::now());
        s
    }

    /// Send the new drawing area to later restarts too.
    pub fn set_ctx(&mut self, ctx: Context) {
        self.ctx = ctx;
    }

    pub fn state(&self) -> &State {
        &self.state
    }

    pub fn connection(&self) -> Option<&Connection> {
        self.conn.as_ref()
    }

    fn start(&mut self, now: Instant) {
        match connect(&self.spec, self.ctx, self.saved.clone(), &mut *self.grant, self.notify.clone()) {
            Ok(c) => {
                self.conn = Some(c);
                self.state = State::Running;
            }
            Err(e) if e.contains("update") => self.state = State::Disabled { reason: e },
            Err(_) => self.crashed(now),
        }
    }

    fn crashed(&mut self, now: Instant) {
        self.conn = None;
        self.crashes += 1;
        self.state = match self.restarts.crashed(now) {
            Decision::Restart(wait) => State::Waiting { until: now + wait, crashes: self.crashes },
            Decision::GiveUp => State::Disabled { reason: format!("{} stopped after {} crashes", self.spec.name, self.crashes) },
        };
    }

    /// Send a message; a broken pipe counts as a crash.
    pub fn send(&mut self, m: &In, now: Instant) -> bool {
        let ok = self.conn.as_mut().is_some_and(|c| c.send(m).is_ok());
        if !ok && self.state == State::Running {
            self.crashed(now);
        }
        ok
    }

    /// Drain the plugin's messages and handle crashes and restarts. Call it
    /// every frame or when woken. `StateSave` is kept here for the restart.
    pub fn poll(&mut self, now: Instant) -> Vec<Out> {
        let mut out = Vec::new();
        if let State::Waiting { until, .. } = self.state
            && now >= until
        {
            self.start(now);
        }
        let mut dead = false;
        if let Some(c) = &mut self.conn {
            loop {
                match c.rx.try_recv() {
                    Ok(Out::StateSave(b)) => self.saved = Some(b),
                    Ok(m) => out.push(m),
                    Err(TryRecvError::Empty) => break,
                    Err(TryRecvError::Disconnected) => {
                        dead = true;
                        break;
                    }
                }
            }
        }
        if dead {
            self.crashed(now);
        }
        out
    }

    /// The status bar's restart action: count again from zero and start now.
    pub fn restart(&mut self, now: Instant) {
        if let Some(c) = self.conn.take() {
            c.stop("restart", Duration::from_millis(500));
        }
        self.restarts.reset();
        self.crashes = 0;
        self.start(now);
    }

    /// Stop for good (quit, disable, upgrade).
    pub fn stop(&mut self, reason: &str) {
        if let Some(c) = self.conn.take() {
            c.stop(reason, Duration::from_millis(500));
        }
        self.state = State::Disabled { reason: reason.into() };
    }

    /// A status-bar line, or `None` while it runs normally.
    pub fn status(&self, now: Instant) -> Option<String> {
        match &self.state {
            State::Running => None,
            State::Waiting { until, .. } => {
                Some(format!("{} crashed, restarting in {} s", self.spec.name, until.saturating_duration_since(now).as_secs_f32().ceil() as u64))
            }
            State::Disabled { reason } => Some(format!("{reason} (restart)")),
        }
    }
}
