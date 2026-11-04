//! Crash restarts for a mode plugin. The host respawns a plugin that exited
//! or broke its pipe after a back-off of 1 s, 2 s, then 4 s. More than
//! `max_restarts` crashes (preference, default 3) within [`WINDOW`] and it
//! gives up: the plugin stays disabled and the status bar offers a restart
//! ([`Restarts::reset`]). Shells never depend on this: they live in the
//! session/PTY server.

use std::time::{Duration, Instant};

/// Crashes older than this are forgotten.
pub const WINDOW: Duration = Duration::from_secs(30);
pub const DEFAULT_MAX_RESTARTS: u32 = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Respawn after this wait.
    Restart(Duration),
    /// Leave it disabled until the user restarts it.
    GiveUp,
}

#[derive(Debug, Clone)]
pub struct Restarts {
    max: u32,
    /// The first wait; each next one doubles (1 s, 2 s, 4 s).
    unit: Duration,
    crashes: Vec<Instant>,
    gave_up: bool,
}

impl Restarts {
    pub fn new(max_restarts: u32) -> Self {
        Self::with_unit(max_restarts, Duration::from_secs(1))
    }

    /// Shorter waits (tests).
    pub fn with_unit(max_restarts: u32, unit: Duration) -> Self {
        Self { max: max_restarts, unit, crashes: Vec::new(), gave_up: false }
    }

    /// The plugin crashed at `now`: wait and restart, or give up.
    pub fn crashed(&mut self, now: Instant) -> Decision {
        if self.gave_up {
            return Decision::GiveUp;
        }
        self.crashes.retain(|t| now.duration_since(*t) < WINDOW);
        self.crashes.push(now);
        let n = self.crashes.len() as u32;
        if n > self.max {
            self.gave_up = true;
            return Decision::GiveUp;
        }
        Decision::Restart(self.unit * (1u32 << (n - 1).min(6)))
    }

    pub fn gave_up(&self) -> bool {
        self.gave_up
    }

    /// The user pressed restart: start counting again.
    pub fn reset(&mut self) {
        self.crashes.clear();
        self.gave_up = false;
    }
}

impl Default for Restarts {
    fn default() -> Self {
        Self::new(DEFAULT_MAX_RESTARTS)
    }
}
