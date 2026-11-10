//! The download manager (EVENTS.md section 14; docs/downloads.md): installs and
//! updates run on their own threads, never the UI or render thread, at most
//! [`MAX_TOTAL`] at once and [`MAX_PER_HOST`] per host. Each job has an id,
//! progress and a cancel flag; one job failing never touches the others.
//! A running Mouser writes the job list to `$MOUSER_HOME/downloads/jobs.toml`
//! (for `mouser downloads`) and picks up `cancel-<id>` files from there.

use crate::install::{self, Kind, Opts, Src};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Condvar, Mutex, OnceLock};

pub const MAX_TOTAL: usize = 4;
pub const MAX_PER_HOST: usize = 2;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum State {
    Queued,
    Running { done: u64, total: Option<u64> },
    Done(String),
    Failed(String),
    Cancelled,
}

impl State {
    pub fn finished(&self) -> bool {
        matches!(self, State::Done(_) | State::Failed(_) | State::Cancelled)
    }
}

/// What a job does.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Op {
    Install,
    Update,
    /// First run: an official lock item (no card, decision 4).
    FirstRun,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Job {
    pub id: u32,
    pub kind: Kind,
    pub src: String,
    pub op: Op,
    /// The server's host (the per-host cap), or "" for local sources.
    pub host: String,
    pub state: State,
}

impl Job {
    pub fn line(&self) -> String {
        let s = match &self.state {
            State::Queued => "queued".to_string(),
            State::Running { done, total: Some(t) } if *t > 0 => format!("{}% of {}", done * 100 / t, human(*t)),
            State::Running { done, .. } => format!("{} so far", human(*done)),
            State::Done(m) => format!("done: {m}"),
            State::Failed(e) => format!("FAILED: {e}"),
            State::Cancelled => "cancelled".into(),
        };
        format!("#{} {} {} {s}", self.id, self.kind.name(), self.src)
    }
}

fn human(b: u64) -> String {
    if b >= 1 << 20 { format!("{:.1} MB", b as f64 / (1 << 20) as f64) } else { format!("{} KB", b.div_ceil(1024)) }
}

/// Asks the user about an install; runs on the job's thread and may block.
pub type Asker = Arc<dyn Fn(&install::Card) -> bool + Send + Sync>;
/// Called after every change (the UI wakes up and redraws the status line).
pub type Notify = Arc<dyn Fn() + Send + Sync>;

struct Inner {
    jobs: Vec<Job>,
    queue: VecDeque<u32>,
    cancel: Vec<(u32, Arc<AtomicBool>)>,
    next: u32,
    running: Vec<(u32, String)>,
    /// Highest concurrency seen (total, per host), for the tests.
    peak: (usize, usize),
}

pub struct Manager {
    inner: Mutex<Inner>,
    idle: Condvar,
    asker: Mutex<Option<Asker>>,
    notify: Mutex<Option<Notify>>,
    /// Whether to mirror the job list to disk (the app's manager does).
    persist: bool,
}

impl Manager {
    pub fn new(persist: bool) -> Arc<Self> {
        Arc::new(Manager {
            inner: Mutex::new(Inner { jobs: vec![], queue: VecDeque::new(), cancel: vec![], next: 1, running: vec![], peak: (0, 0) }),
            idle: Condvar::new(),
            asker: Mutex::new(None),
            notify: Mutex::new(None),
            persist,
        })
    }

    pub fn set_asker(&self, a: Asker) {
        *self.asker.lock().unwrap() = Some(a);
    }
    pub fn set_notify(&self, n: Notify) {
        *self.notify.lock().unwrap() = Some(n);
    }

    /// Queue an install, update or first-run job; returns its id.
    pub fn enqueue(self: &Arc<Self>, kind: Kind, src: &str, op: Op) -> u32 {
        let host = host_of(src);
        let id = {
            let mut g = self.inner.lock().unwrap();
            let id = g.next;
            g.next += 1;
            g.jobs.push(Job { id, kind, src: src.to_string(), op, host, state: State::Queued });
            g.queue.push_back(id);
            g.cancel.push((id, Arc::new(AtomicBool::new(false))));
            id
        };
        self.changed();
        self.schedule();
        id
    }

    pub fn jobs(&self) -> Vec<Job> {
        self.inner.lock().unwrap().jobs.clone()
    }

    pub fn peak(&self) -> (usize, usize) {
        self.inner.lock().unwrap().peak
    }

    /// Cancel one job (or all with `None`). Returns how many were cancelled.
    pub fn cancel(&self, id: Option<u32>) -> usize {
        let mut n = 0;
        {
            let mut g = self.inner.lock().unwrap();
            let ids: Vec<u32> = g.jobs.iter().filter(|j| !j.state.finished() && id.is_none_or(|i| i == j.id)).map(|j| j.id).collect();
            for i in ids {
                if let Some((_, f)) = g.cancel.iter().find(|(c, _)| *c == i) {
                    f.store(true, Ordering::SeqCst);
                }
                if let Some(p) = g.queue.iter().position(|q| *q == i) {
                    g.queue.remove(p);
                    if let Some(j) = g.jobs.iter_mut().find(|j| j.id == i) {
                        j.state = State::Cancelled;
                    }
                }
                n += 1;
            }
        }
        self.changed();
        self.idle.notify_all();
        n
    }

    /// Queue the failed jobs again (`retry`). Returns the new ids.
    pub fn retry(self: &Arc<Self>) -> Vec<u32> {
        let failed: Vec<(Kind, String, Op)> = {
            let mut g = self.inner.lock().unwrap();
            let f: Vec<_> = g.jobs.iter().filter(|j| matches!(j.state, State::Failed(_))).map(|j| (j.kind, j.src.clone(), j.op.clone())).collect();
            g.jobs.retain(|j| !matches!(j.state, State::Failed(_)));
            f
        };
        failed.into_iter().map(|(k, s, o)| self.enqueue(k, &s, o)).collect()
    }

    /// Block until nothing is queued or running.
    pub fn wait(&self) {
        let mut g = self.inner.lock().unwrap();
        while !g.queue.is_empty() || !g.running.is_empty() {
            g = self.idle.wait(g).unwrap();
        }
    }

    fn changed(&self) {
        if self.persist {
            let jobs = self.jobs();
            let d = dir();
            let _ = std::fs::create_dir_all(&d);
            if let Ok(s) = toml::to_string(&JobFile { job: jobs }) {
                let _ = std::fs::write(d.join("jobs.toml.tmp"), s).and_then(|_| std::fs::rename(d.join("jobs.toml.tmp"), d.join("jobs.toml")));
            }
        }
        if let Some(n) = self.notify.lock().unwrap().clone() {
            n();
        }
    }

    /// Start whatever the caps allow.
    fn schedule(self: &Arc<Self>) {
        loop {
            let pick = {
                let mut g = self.inner.lock().unwrap();
                if g.running.len() >= MAX_TOTAL {
                    None
                } else {
                    let per = |h: &str, g: &Inner| g.running.iter().filter(|(_, rh)| rh == h && !h.is_empty()).count();
                    let pos = g.queue.iter().position(|id| {
                        let h = g.jobs.iter().find(|j| j.id == *id).map(|j| j.host.clone()).unwrap_or_default();
                        h.is_empty() || per(&h, &g) < MAX_PER_HOST
                    });
                    pos.map(|p| {
                        let id = g.queue.remove(p).unwrap();
                        let j = g.jobs.iter_mut().find(|j| j.id == id).unwrap();
                        j.state = State::Running { done: 0, total: None };
                        let job = j.clone();
                        g.running.push((id, job.host.clone()));
                        let host_n = per(&job.host, &g);
                        g.peak = (g.peak.0.max(g.running.len()), g.peak.1.max(host_n));
                        let flag = g.cancel.iter().find(|(c, _)| *c == id).map(|(_, f)| f.clone()).unwrap();
                        (job, flag)
                    })
                }
            };
            let Some((job, flag)) = pick else { break };
            self.changed();
            let me = self.clone();
            std::thread::Builder::new()
                .name(format!("mouser-download-{}", job.id))
                .spawn(move || {
                    let r = me.run(&job, &flag);
                    {
                        let mut g = me.inner.lock().unwrap();
                        g.running.retain(|(i, _)| *i != job.id);
                        if let Some(j) = g.jobs.iter_mut().find(|j| j.id == job.id) {
                            j.state = match r {
                                _ if flag.load(Ordering::SeqCst) => State::Cancelled,
                                Ok(m) => State::Done(m),
                                Err(e) => State::Failed(format!("{e:#}")),
                            };
                        }
                    }
                    let _ = std::fs::remove_file(dir().join(format!("cancel-{}", job.id)));
                    me.changed();
                    me.idle.notify_all();
                    me.schedule();
                })
                .expect("download thread");
        }
    }

    fn run(&self, job: &Job, flag: &Arc<AtomicBool>) -> anyhow::Result<String> {
        let me_jobs = &self.inner;
        let id = job.id;
        let mut last = std::time::Instant::now();
        let mut progress = |done: u64, total: Option<u64>| {
            if flag.load(Ordering::SeqCst) || dir().join(format!("cancel-{id}")).exists() {
                flag.store(true, Ordering::SeqCst);
                return false;
            }
            if last.elapsed().as_millis() >= 100 || total == Some(done) {
                last = std::time::Instant::now();
                if let Some(j) = me_jobs.lock().unwrap().jobs.iter_mut().find(|j| j.id == id) {
                    j.state = State::Running { done, total };
                }
                self.changed();
            }
            true
        };
        let asker = self.asker.lock().unwrap().clone();
        let mut confirm = |c: &install::Card| asker.as_ref().is_some_and(|a| a(c));
        let opts = Opts { first_run: job.op == Op::FirstRun, ..Default::default() };
        match job.op {
            Op::Update => {
                let name = job.src.as_str();
                Ok(match install::update(job.kind, name, opts, &mut progress, &mut confirm)? {
                    Some(i) => i.summary(),
                    None => format!("{name} is up to date"),
                })
            }
            Op::FirstRun => {
                let l = install::lock();
                let it = l.find(&job.src).ok_or_else(|| anyhow::anyhow!("{} is not in plugins.lock", job.src))?;
                let ts = install::targets();
                if let Some(file) = ts.iter().find_map(|t| it.assets.get(t)).and_then(install::offline_copy) {
                    return Ok(install::install_offline(it, &file, opts, &mut confirm)?.summary() + " (offline bundle)");
                }
                Ok(install::install(&Src::Official(job.src.clone()), job.kind, opts, &mut progress, &mut confirm)?.summary())
            }
            Op::Install => Ok(install::install(&Src::parse(&job.src)?, job.kind, opts, &mut progress, &mut confirm)?.summary()),
        }
    }

    /// First run, and every start: queue every `plugins.lock` item that
    /// isn't installed or shipped, and every official one installed at an
    /// older version than this Mouser pins. Returns how many were queued.
    pub fn first_run(self: &Arc<Self>) -> usize {
        let mut missing = install::missing_official();
        missing.extend(install::outdated_official());
        for it in &missing {
            self.enqueue(it.kind, &it.name, Op::FirstRun);
        }
        missing.len()
    }

    /// One status-bar line while anything is active; the failures after.
    pub fn status(&self) -> Option<String> {
        let jobs = self.jobs();
        let active: Vec<String> = jobs
            .iter()
            .filter(|j| !j.state.finished())
            .map(|j| match &j.state {
                State::Running { done, total: Some(t) } if *t > 0 => format!("{} {}%", j.src, done * 100 / t),
                State::Running { .. } => format!("{} …", j.src),
                _ => format!("{} queued", j.src),
            })
            .collect();
        let failed = jobs.iter().filter(|j| matches!(j.state, State::Failed(_))).count();
        if active.is_empty() && failed == 0 {
            return None;
        }
        let mut s = if active.is_empty() { String::new() } else { format!("downloads: {}", active.join(" · ")) };
        if failed > 0 {
            if !s.is_empty() {
                s.push_str("; ");
            }
            s.push_str(&format!("{failed} failed (esc :plugin retry; esc :downloads for why)"));
        }
        Some(s)
    }
}

#[derive(Serialize, Deserialize, Default)]
struct JobFile {
    #[serde(default)]
    job: Vec<Job>,
}

pub fn dir() -> PathBuf {
    crate::storage::resolve_home().join("downloads")
}

/// The running Mouser's job list, as last written.
pub fn saved_jobs() -> Vec<Job> {
    std::fs::read_to_string(dir().join("jobs.toml")).ok().and_then(|s| toml::from_str::<JobFile>(&s).ok()).map(|f| f.job).unwrap_or_default()
}

/// Ask the running Mouser to cancel a job (`mouser downloads --cancel ID`).
pub fn request_cancel(id: u32) -> anyhow::Result<()> {
    std::fs::create_dir_all(dir())?;
    std::fs::write(dir().join(format!("cancel-{id}")), "")?;
    Ok(())
}

fn host_of(src: &str) -> String {
    match Src::parse(src) {
        Ok(Src::Url(u)) => crate::mcp::Target::parse(&u).map(|t| t.host).unwrap_or_default(),
        Ok(Src::Github { .. }) => "github.com".into(),
        Ok(Src::Official(n)) => install::lock()
            .find(&n)
            .and_then(|i| install::targets().iter().find_map(|t| i.assets.get(t)).and_then(|a| crate::mcp::Target::parse(&a.url).ok()))
            .map(|t| t.host)
            .unwrap_or_default(),
        _ => String::new(),
    }
}

/// The app's one manager.
pub fn global() -> &'static Arc<Manager> {
    static M: OnceLock<Arc<Manager>> = OnceLock::new();
    M.get_or_init(|| Manager::new(true))
}
