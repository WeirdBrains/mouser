//! Commander's media player (MODES.md: "a player will appear").
//!
//! Two `ffmpeg` readers of the same file start at the same seek point: one
//! decodes video to RGBA frames at a constant rate (so frame `i` has PTS
//! `start + i / fps`), the other decodes PCM (f32) at the output device's rate.
//! (One process muxing both on a pipe would need a container parser; two
//! readers keep it plain.) Mouser plays the audio itself through `cpal`, and
//! the audio is the master clock: the position is the number of samples the
//! device has taken, minus its reported output latency. A frame is shown when
//! the clock reaches its PTS; late frames are dropped, early ones held.
//! Without audio a wall clock is used. Pause silences the output and freezes
//! the clock (the readers block on full queues); volume is applied in the
//! callback; a seek flushes the queues and restarts the readers with `-ss`.

use crate::Waker;
use std::collections::VecDeque;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Condvar, Mutex};
use std::time::{Duration, Instant};

/// One decoded frame.
#[derive(Clone)]
pub struct Frame {
    pub key: u64,
    pub rgba: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
    /// Presentation time in seconds.
    pub pts: f64,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub fps: f64,
    pub video: bool,
    pub audio: bool,
}

/// ffmpeg and ffprobe are on PATH.
pub fn available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        ["ffmpeg", "ffprobe"]
            .iter()
            .all(|t| Command::new(t).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()))
    })
}

fn rate(s: &str) -> Option<f64> {
    let (n, d) = s.split_once('/').unwrap_or((s, "1"));
    let (n, d): (f64, f64) = (n.parse().ok()?, d.parse().ok()?);
    (d > 0.0 && n > 0.0).then_some(n / d)
}

/// Streams, frame rate and duration from ffprobe's `-of json`.
pub fn parse_probe(json: &str) -> Option<Info> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut i = Info::default();
    for s in v.get("streams")?.as_array()? {
        match s.get("codec_type").and_then(|t| t.as_str()) {
            Some("video") if !i.video => {
                // Cover art in audio files is a video stream too.
                let still = s.get("disposition").and_then(|d| d.get("attached_pic")).and_then(|a| a.as_i64()) == Some(1);
                if !still {
                    i.video = true;
                    i.width = s.get("width")?.as_u64()? as u32;
                    i.height = s.get("height")?.as_u64()? as u32;
                    i.fps =
                        s.get("avg_frame_rate").or(s.get("r_frame_rate")).and_then(|r| r.as_str()).and_then(rate).unwrap_or(25.0).clamp(1.0, 120.0);
                }
            }
            Some("audio") => i.audio = true,
            _ => {}
        }
    }
    i.duration = v.get("format").and_then(|f| f.get("duration")).and_then(|d| d.as_str()).and_then(|d| d.parse().ok()).unwrap_or(0.0);
    (i.video || i.audio).then_some(i)
}

fn probe(p: &Path) -> Option<Info> {
    let out = Command::new("ffprobe")
        .args([
            "-v",
            "error",
            "-show_entries",
            "stream=codec_type,width,height,avg_frame_rate,r_frame_rate:stream_disposition=attached_pic:format=duration",
            "-of",
            "json",
        ])
        .arg(p)
        .stdin(Stdio::null())
        .output()
        .ok()?;
    parse_probe(&String::from_utf8_lossy(&out.stdout))
}

static NEXT_KEY: AtomicU64 = AtomicU64::new(3 << 61);

/// `m:ss` / `h:mm:ss`.
pub fn clock(t: f64) -> String {
    let s = t.max(0.0) as u64;
    if s >= 3600 { format!("{}:{:02}:{:02}", s / 3600, s / 60 % 60, s % 60) } else { format!("{}:{:02}", s / 60, s % 60) }
}

/// Audio clock: `start + consumed / rate - latency`, never before `start`.
pub fn audio_clock(start: f64, consumed_frames: u64, rate: u32, latency: f64) -> f64 {
    start + (consumed_frames as f64 / rate.max(1) as f64 - latency).max(0.0)
}

/// The frame to show at clock `t`: pops every frame that is due and returns
/// the newest of them with how many older due frames were dropped (late).
/// Frames still in the future stay queued (held).
pub fn pick<T>(queue: &mut VecDeque<T>, t: f64, pts: impl Fn(&T) -> f64) -> (Option<T>, usize) {
    let mut shown = None;
    let mut dropped = 0;
    while queue.front().is_some_and(|f| pts(f) <= t) {
        if shown.is_some() {
            dropped += 1;
        }
        shown = queue.pop_front();
    }
    (shown, dropped)
}

/// Wall clock for files without audio: runs while playing, frozen while paused.
#[derive(Debug, Clone, Copy)]
pub struct WallClock {
    pub start: f64,
    acc: Duration,
    since: Option<Instant>,
}

impl WallClock {
    pub fn new(start: f64) -> Self {
        Self { start, acc: Duration::ZERO, since: None }
    }
    pub fn run(&mut self, now: Instant) {
        self.since.get_or_insert(now);
    }
    pub fn stop(&mut self, now: Instant) {
        if let Some(s) = self.since.take() {
            self.acc += now.duration_since(s);
        }
    }
    pub fn at(&self, now: Instant) -> f64 {
        self.start + (self.acc + self.since.map_or(Duration::ZERO, |s| now.duration_since(s))).as_secs_f64()
    }
}

/// State shared with the reader threads and the audio callback.
#[derive(Default)]
struct Shared {
    /// Bumped by every seek and on close; stale readers stop.
    epoch: AtomicU64,
    closed: AtomicBool,
    paused: AtomicBool,
    /// f32 bits.
    volume: AtomicU32,
    /// Audio frames the device has taken since the last seek.
    consumed: AtomicU64,
    /// Device output latency, ns.
    latency_ns: AtomicU64,
    audio: Mutex<VecDeque<f32>>,
    audio_cv: Condvar,
    video: Mutex<VecDeque<Frame>>,
    video_cv: Condvar,
    audio_eof: AtomicBool,
    video_eof: AtomicBool,
    /// Late frames dropped (diagnostics).
    dropped: AtomicU64,
}

/// Output device format.
#[derive(Debug, Clone, Copy)]
struct Out {
    rate: u32,
    channels: u16,
}

pub struct Player {
    pub path: PathBuf,
    pub info: Info,
    /// 0..=100.
    pub volume: u8,
    s: Arc<Shared>,
    out: Option<Out>,
    start: f64,
    wall: WallClock,
    procs: Vec<Child>,
    shown: Option<Frame>,
    size: (u32, u32),
    waker: Option<Waker>,
    pub error: Option<String>,
    /// `MOUSER_PLAYER_LOG=<file>`: `clock pts` per shown frame.
    log: Option<std::fs::File>,
    /// When the last seek was asked for, until its first frame (seek gap).
    seek_at: Option<Instant>,
    pub last_seek_gap: Option<Duration>,
}

impl Player {
    /// Probe `path` and start playing, decoding video `width` px wide.
    pub fn open(path: &Path, width: u32, waker: Option<Waker>) -> Self {
        let info = probe(path);
        let s = Arc::new(Shared::default());
        s.volume.store(0.8f32.to_bits(), Ordering::Relaxed);
        let mut p = Self {
            path: path.to_path_buf(),
            info: info.unwrap_or_default(),
            volume: 80,
            s,
            out: None,
            start: 0.0,
            wall: WallClock::new(0.0),
            procs: Vec::new(),
            shown: None,
            size: (0, 0),
            waker,
            error: info.is_none().then(|| "ffprobe found no audio or video".to_string()),
            log: std::env::var_os("MOUSER_PLAYER_LOG").and_then(|f| std::fs::File::create(f).ok()),
            seek_at: None,
            last_seek_gap: None,
        };
        if p.info.video {
            let w = width.clamp(64, 1280) & !1;
            let h = ((w as f64 * p.info.height as f64 / p.info.width.max(1) as f64) as u32).max(2) & !1;
            p.size = (w, h);
        }
        if p.error.is_some() {
            return p;
        }
        if p.info.audio {
            match start_output(p.s.clone()) {
                Ok(o) => p.out = Some(o),
                Err(e) => p.error = Some(format!("audio output: {e} (playing without sound)")),
            }
        }
        p.spawn();
        p.ticker();
        p
    }

    pub fn playing(&self) -> bool {
        !self.s.paused.load(Ordering::Relaxed)
    }

    /// The master clock.
    pub fn position(&self) -> f64 {
        let t = match self.out {
            Some(o) => {
                audio_clock(self.start, self.s.consumed.load(Ordering::Relaxed), o.rate, self.s.latency_ns.load(Ordering::Relaxed) as f64 / 1e9)
            }
            None => self.wall.at(Instant::now()),
        };
        if self.info.duration > 0.0 { t.min(self.info.duration) } else { t }
    }

    pub fn frame(&self) -> Option<Frame> {
        self.shown.clone()
    }

    pub fn dropped(&self) -> u64 {
        self.s.dropped.load(Ordering::Relaxed)
    }

    /// Called every drawn frame: pick the frame for the clock, notice the end.
    pub fn tick(&mut self) {
        let t = self.position();
        let (f, dropped) = {
            let mut q = self.s.video.lock().unwrap();
            pick(&mut q, t, |f: &Frame| f.pts)
        };
        if let Some(f) = f {
            self.s.dropped.fetch_add(dropped as u64, Ordering::Relaxed);
            self.s.video_cv.notify_all();
            if let Some(l) = self.log.as_mut() {
                use std::io::Write;
                let _ = writeln!(l, "{t:.4} {:.4}", f.pts);
            }
            if let Some(s) = self.seek_at.take() {
                self.last_seek_gap = Some(s.elapsed());
                if let Some(l) = self.log.as_mut() {
                    use std::io::Write;
                    let _ = writeln!(l, "# seek_gap_ms {:.1} at {:.3}", s.elapsed().as_secs_f64() * 1e3, f.pts);
                }
            }
            self.shown = Some(f);
        }
        // Without video the first audio after a seek ends the gap.
        if !self.info.video && self.seek_at.is_some() && self.s.consumed.load(Ordering::Relaxed) > 0 {
            self.last_seek_gap = self.seek_at.take().map(|s| s.elapsed());
        }
        let ended = (!self.info.video || self.s.video_eof.load(Ordering::Relaxed) && self.s.video.lock().unwrap().is_empty())
            && (self.out.is_none() || self.s.audio_eof.load(Ordering::Relaxed) && self.s.audio.lock().unwrap().is_empty())
            && t > self.start;
        if ended && self.playing() {
            self.pause();
        }
    }

    fn kill(&mut self) {
        self.s.epoch.fetch_add(1, Ordering::SeqCst);
        self.s.audio_cv.notify_all();
        self.s.video_cv.notify_all();
        for mut c in self.procs.drain(..) {
            let _ = c.kill();
            std::thread::spawn(move || c.wait());
        }
    }

    /// (Re)start the readers at `self.start`.
    fn spawn(&mut self) {
        self.kill();
        self.s.audio.lock().unwrap().clear();
        self.s.video.lock().unwrap().clear();
        self.s.consumed.store(0, Ordering::SeqCst);
        self.s.audio_eof.store(false, Ordering::SeqCst);
        self.s.video_eof.store(false, Ordering::SeqCst);
        let epoch = self.s.epoch.load(Ordering::SeqCst);
        let ss = format!("{:.3}", self.start);
        if self.info.video {
            let (w, h, fps) = (self.size.0, self.size.1, self.info.fps);
            let child = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &ss, "-i"])
                .arg(&self.path)
                .args(["-an", "-vf", &format!("fps={fps},scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut c) => {
                    let out = c.stdout.take().unwrap();
                    let (s, start) = (self.s.clone(), self.start);
                    std::thread::spawn(move || read_video(out, s, epoch, start, w, h, fps));
                    self.procs.push(c);
                }
                Err(e) => self.error = Some(format!("ffmpeg: {e}")),
            }
        }
        if let Some(o) = self.out {
            let child = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &ss, "-i"])
                .arg(&self.path)
                .args(["-vn", "-f", "f32le", "-ac", &o.channels.to_string(), "-ar", &o.rate.to_string(), "pipe:1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut c) => {
                    let out = c.stdout.take().unwrap();
                    let s = self.s.clone();
                    let cap = o.rate as usize * o.channels as usize / 2;
                    std::thread::spawn(move || read_audio(out, s, epoch, cap));
                    self.procs.push(c);
                }
                Err(e) => self.error = Some(format!("ffmpeg: {e}")),
            }
        }
        self.wall = WallClock::new(self.start);
        if self.playing() {
            self.wall.run(Instant::now());
        }
    }

    /// Wakes the UI at the frame rate while playing.
    fn ticker(&self) {
        let Some(w) = self.waker.clone() else { return };
        let s = self.s.clone();
        let step = Duration::from_secs_f64(if self.info.video { 0.5 / self.info.fps.max(1.0) } else { 0.25 });
        std::thread::spawn(move || {
            while !s.closed.load(Ordering::Relaxed) {
                std::thread::sleep(step);
                if !s.paused.load(Ordering::Relaxed) {
                    w();
                }
            }
        });
    }

    pub fn play(&mut self) {
        if self.info.duration > 0.0 && self.position() >= self.info.duration - 0.05 {
            self.seek_to(0.0);
        }
        self.s.paused.store(false, Ordering::SeqCst);
        self.wall.run(Instant::now());
    }

    pub fn pause(&mut self) {
        self.s.paused.store(true, Ordering::SeqCst);
        self.wall.stop(Instant::now());
    }

    pub fn toggle(&mut self) {
        if self.playing() { self.pause() } else { self.play() }
    }

    /// Jump to `t`: flush the queues and restart the readers there.
    pub fn seek_to(&mut self, t: f64) {
        let max = if self.info.duration > 0.0 { (self.info.duration - 0.1).max(0.0) } else { f64::MAX };
        self.start = t.clamp(0.0, max);
        self.seek_at = Some(Instant::now());
        self.spawn();
    }

    pub fn seek(&mut self, by: f64) {
        let t = self.position() + by;
        self.seek_to(t);
    }

    pub fn change_volume(&mut self, by: i32) {
        self.volume = (self.volume as i32 + by).clamp(0, 100) as u8;
        self.s.volume.store((self.volume as f32 / 100.0).to_bits(), Ordering::Relaxed);
    }

    /// Running helper processes (tests).
    pub fn processes(&mut self) -> usize {
        self.procs.retain_mut(|c| c.try_wait().ok().flatten().is_none());
        self.procs.len()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.s.closed.store(true, Ordering::SeqCst);
        self.kill();
    }
}

fn read_video(mut out: impl Read, s: Arc<Shared>, epoch: u64, start: f64, w: u32, h: u32, fps: f64) {
    let mut buf = vec![0u8; (w * h * 4) as usize];
    let mut i = 0u64;
    while out.read_exact(&mut buf).is_ok() {
        let f = Frame { key: NEXT_KEY.fetch_add(1, Ordering::Relaxed), rgba: Arc::new(buf.clone()), w, h, pts: start + i as f64 / fps };
        i += 1;
        let mut q = s.video.lock().unwrap();
        while q.len() >= 8 && s.epoch.load(Ordering::SeqCst) == epoch {
            q = s.video_cv.wait_timeout(q, Duration::from_millis(100)).unwrap().0;
        }
        if s.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        q.push_back(f);
    }
    if s.epoch.load(Ordering::SeqCst) == epoch {
        s.video_eof.store(true, Ordering::SeqCst);
    }
}

fn read_audio(mut out: impl Read, s: Arc<Shared>, epoch: u64, cap: usize) {
    let mut buf = vec![0u8; 16384];
    let mut carry = Vec::new();
    loop {
        let n = match out.read(&mut buf) {
            Ok(0) | Err(_) => break,
            Ok(n) => n,
        };
        carry.extend_from_slice(&buf[..n]);
        let whole = carry.len() / 4 * 4;
        let samples: Vec<f32> = carry[..whole].as_chunks::<4>().0.iter().map(|b| f32::from_le_bytes(*b)).collect();
        carry.drain(..whole);
        let mut q = s.audio.lock().unwrap();
        while q.len() >= cap && s.epoch.load(Ordering::SeqCst) == epoch {
            q = s.audio_cv.wait_timeout(q, Duration::from_millis(100)).unwrap().0;
        }
        if s.epoch.load(Ordering::SeqCst) != epoch {
            return;
        }
        q.extend(samples);
    }
    if s.epoch.load(Ordering::SeqCst) == epoch {
        s.audio_eof.store(true, Ordering::SeqCst);
    }
}

/// Fill `data` (interleaved, `ch` channels) from the queue; returns frames taken.
fn fill(data: &mut [f32], ch: usize, s: &Shared) -> u64 {
    if s.paused.load(Ordering::Relaxed) {
        data.fill(0.0);
        return 0;
    }
    let vol = f32::from_bits(s.volume.load(Ordering::Relaxed));
    let mut q = s.audio.lock().unwrap();
    // Whole frames only, so the channels stay aligned.
    let n = (q.len().min(data.len()) / ch) * ch;
    for (d, v) in data.iter_mut().zip(q.drain(..n)) {
        *d = v * vol;
    }
    data[n..].fill(0.0);
    drop(q);
    s.audio_cv.notify_all();
    (n / ch) as u64
}

/// Open the default output device on its own thread (cpal streams are not
/// `Send` everywhere); it lives until the player closes.
fn start_output(s: Arc<Shared>) -> Result<Out, String> {
    let (tx, rx) = std::sync::mpsc::channel();
    std::thread::spawn(move || {
        use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
        let r = (|| {
            let dev = cpal::default_host().default_output_device().ok_or("no output device")?;
            let def = dev.default_output_config().map_err(|e| e.to_string())?;
            let cfg = cpal::StreamConfig { channels: def.channels().min(2), sample_rate: def.sample_rate(), buffer_size: cpal::BufferSize::Default };
            let ch = cfg.channels as usize;
            let s2 = s.clone();
            let stream = dev
                .build_output_stream(
                    &cfg,
                    move |data: &mut [f32], info: &cpal::OutputCallbackInfo| {
                        let ts = info.timestamp();
                        if let Some(l) = ts.playback.duration_since(&ts.callback) {
                            s2.latency_ns.store(l.as_nanos() as u64, Ordering::Relaxed);
                        }
                        let n = fill(data, ch, &s2);
                        s2.consumed.fetch_add(n, Ordering::Relaxed);
                    },
                    |_| {},
                    None,
                )
                .map_err(|e| e.to_string())?;
            stream.play().map_err(|e| e.to_string())?;
            Ok::<_, String>((stream, Out { rate: cfg.sample_rate.0, channels: cfg.channels }))
        })();
        match r {
            Ok((stream, out)) => {
                let _ = tx.send(Ok(out));
                while !s.closed.load(Ordering::Relaxed) {
                    std::thread::sleep(Duration::from_millis(100));
                }
                drop(stream);
            }
            Err(e) => {
                let _ = tx.send(Err(e));
            }
        }
    });
    rx.recv_timeout(Duration::from_secs(3)).map_err(|_| "timed out".to_string())?
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_json() {
        let j = r#"{"streams":[{"codec_type":"video","width":640,"height":360,"avg_frame_rate":"30000/1001","disposition":{"attached_pic":0}},{"codec_type":"audio"}],"format":{"duration":"12.5"}}"#;
        let i = parse_probe(j).unwrap();
        assert_eq!((i.width, i.height, i.video, i.audio, i.duration), (640, 360, true, true, 12.5));
        assert!((i.fps - 29.97).abs() < 0.01);
        let art = r#"{"streams":[{"codec_type":"audio"},{"codec_type":"video","width":500,"height":500,"disposition":{"attached_pic":1}}],"format":{"duration":"3"}}"#;
        assert_eq!(parse_probe(art).map(|i| (i.video, i.audio)), Some((false, true)), "cover art is not a video");
        assert_eq!(parse_probe(r#"{"streams":[],"format":{}}"#), None);
        assert_eq!(clock(83.9), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
    }

    #[test]
    fn audio_clock_counts_samples_minus_latency() {
        assert_eq!(audio_clock(10.0, 48_000, 48_000, 0.0), 11.0);
        assert!((audio_clock(10.0, 48_000, 48_000, 0.05) - 10.95).abs() < 1e-9);
        assert_eq!(audio_clock(10.0, 100, 48_000, 0.05), 10.0, "never before the seek point");
    }

    #[test]
    fn picks_due_frames_drops_late_holds_early() {
        let mut q: VecDeque<f64> = [1.0, 1.04, 1.08, 1.12].into();
        assert_eq!(pick(&mut q, 0.99, |f| *f), (None, 0), "early: held");
        assert_eq!(q.len(), 4);
        assert_eq!(pick(&mut q, 1.0, |f| *f), (Some(1.0), 0));
        assert_eq!(pick(&mut q, 1.10, |f| *f), (Some(1.08), 1), "1.04 was late: dropped");
        assert_eq!(q, [1.12]);
    }

    #[test]
    fn wall_clock_freezes_while_paused() {
        let t0 = Instant::now();
        let mut c = WallClock::new(5.0);
        assert_eq!(c.at(t0), 5.0);
        c.run(t0);
        let t1 = t0 + Duration::from_secs(2);
        assert_eq!(c.at(t1), 7.0);
        c.stop(t1);
        assert_eq!(c.at(t1 + Duration::from_secs(10)), 7.0, "paused");
        c.run(t1 + Duration::from_secs(10));
        assert_eq!(c.at(t1 + Duration::from_secs(11)), 8.0);
    }

    #[test]
    fn fill_applies_volume_and_counts_whole_frames() {
        let s = Shared::default();
        s.volume.store(0.5f32.to_bits(), Ordering::Relaxed);
        s.audio.lock().unwrap().extend([1.0, 1.0, 1.0, 1.0, 1.0]);
        let mut d = [9.0f32; 4];
        assert_eq!(fill(&mut d, 2, &s), 2);
        assert_eq!(d, [0.5; 4]);
        let mut d = [9.0f32; 4];
        assert_eq!(fill(&mut d, 2, &s), 0, "half a frame is not played");
        assert_eq!(d, [0.0; 4]);
        s.paused.store(true, Ordering::Relaxed);
        s.audio.lock().unwrap().extend([1.0; 4]);
        assert_eq!(fill(&mut d, 2, &s), 0, "paused: silence, clock frozen");
        assert_eq!(s.audio.lock().unwrap().len(), 5);
    }

    /// Real decode when ffmpeg is installed (video only: no audio device needed).
    #[test]
    fn plays_and_seeks_a_generated_clip() {
        if !available() {
            eprintln!("ffmpeg not installed, skipped");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let clip = t.path().join("c.mp4");
        let ok = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=160x90:rate=10:duration=6", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            return;
        }
        let mut p = Player::open(&clip, 160, None);
        assert!(p.info.video && !p.info.audio && (p.info.fps - 10.0).abs() < 0.01, "{:?}", p.info);
        let wait = |p: &mut Player, f: &dyn Fn(&Player) -> bool| {
            for _ in 0..200 {
                p.tick();
                if f(p) {
                    return true;
                }
                std::thread::sleep(Duration::from_millis(10));
            }
            false
        };
        assert!(wait(&mut p, &|p| p.frame().is_some()), "a frame");
        let first = p.frame().unwrap();
        assert_eq!((first.w, first.h), (160, 90));
        std::thread::sleep(Duration::from_millis(500));
        p.tick();
        let later = p.frame().unwrap();
        assert!(later.pts > first.pts && (later.pts - p.position()).abs() < 0.15, "frame follows the clock");
        p.pause();
        let at = p.position();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(p.position(), at, "paused: the clock is frozen");
        p.play();
        p.seek_to(4.0);
        assert!(wait(&mut p, &|p| p.frame().is_some_and(|f| f.pts >= 4.0)), "a frame after the seek");
        assert!((p.frame().unwrap().pts - 4.0).abs() < 0.15);
        assert!(p.last_seek_gap.is_some_and(|g| g < Duration::from_secs(2)));
        p.change_volume(50);
        assert_eq!(p.volume, 100);
        drop(p);
    }
}
