//! Commander's media player (MODES.md: "a player will appear"). Video frames
//! come from `ffmpeg` (raw RGBA on a pipe) and are shown through Mouser's
//! image path; sound plays through `ffplay -nodisp`. Pause, seek and volume
//! restart the helpers at the current position. Nothing runs until a file is
//! opened, and everything stops when the player is dropped.

use crate::Waker;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

/// One decoded frame.
#[derive(Clone)]
pub struct Frame {
    pub key: u64,
    pub rgba: Arc<Vec<u8>>,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Default)]
pub struct Info {
    pub width: u32,
    pub height: u32,
    pub duration: f64,
    pub video: bool,
    pub audio: bool,
}

/// ffmpeg, ffprobe and ffplay are on PATH.
pub fn available() -> bool {
    static OK: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *OK.get_or_init(|| {
        ["ffmpeg", "ffprobe", "ffplay"]
            .iter()
            .all(|t| Command::new(t).arg("-version").stdout(Stdio::null()).stderr(Stdio::null()).status().is_ok_and(|s| s.success()))
    })
}

/// Streams and duration from ffprobe's `-of json`.
pub fn parse_probe(json: &str) -> Option<Info> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let mut i = Info::default();
    for s in v.get("streams")?.as_array()? {
        match s.get("codec_type").and_then(|t| t.as_str()) {
            Some("video") if !i.video => {
                // Cover art in audio files is a video stream too; it has no frame rate worth playing.
                let still = s.get("disposition").and_then(|d| d.get("attached_pic")).and_then(|a| a.as_i64()) == Some(1);
                if !still {
                    i.video = true;
                    i.width = s.get("width")?.as_u64()? as u32;
                    i.height = s.get("height")?.as_u64()? as u32;
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
        .args(["-v", "error", "-show_entries", "stream=codec_type,width,height:stream_disposition=attached_pic:format=duration", "-of", "json"])
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

pub struct Player {
    pub path: PathBuf,
    pub info: Info,
    /// 0..=100.
    pub volume: u8,
    base: f64,
    since: Option<Instant>,
    procs: Vec<Child>,
    frame: Arc<Mutex<Option<Frame>>>,
    epoch: Arc<AtomicU64>,
    /// Decode size (fit to the view's width).
    size: (u32, u32),
    waker: Option<Waker>,
    pub error: Option<String>,
}

impl Player {
    /// Probe `path` and start playing, decoding video `width` px wide.
    pub fn open(path: &Path, width: u32, waker: Option<Waker>) -> Self {
        let info = probe(path);
        let mut p = Self {
            path: path.to_path_buf(),
            info: info.unwrap_or_default(),
            volume: 80,
            base: 0.0,
            since: None,
            procs: Vec::new(),
            frame: Arc::new(Mutex::new(None)),
            epoch: Arc::new(AtomicU64::new(0)),
            size: (0, 0),
            waker,
            error: info.is_none().then(|| "ffprobe found no audio or video".to_string()),
        };
        if p.info.video {
            let w = width.clamp(64, 1280) & !1;
            let h = ((w as f64 * p.info.height as f64 / p.info.width.max(1) as f64) as u32).max(2) & !1;
            p.size = (w, h);
        }
        if p.error.is_none() {
            p.play();
        }
        p
    }

    pub fn playing(&self) -> bool {
        self.since.is_some()
    }

    pub fn position(&self) -> f64 {
        let t = self.base + self.since.map_or(0.0, |s| s.elapsed().as_secs_f64());
        if self.info.duration > 0.0 { t.min(self.info.duration) } else { t }
    }

    pub fn frame(&self) -> Option<Frame> {
        self.frame.lock().unwrap().clone()
    }

    /// Called every frame: notice the end.
    pub fn tick(&mut self) {
        if self.playing() && self.info.duration > 0.0 && self.position() >= self.info.duration {
            self.base = self.info.duration;
            self.since = None;
            self.kill();
        }
    }

    fn kill(&mut self) {
        self.epoch.fetch_add(1, Ordering::SeqCst);
        for mut c in self.procs.drain(..) {
            let _ = c.kill();
            std::thread::spawn(move || c.wait());
        }
    }

    fn spawn(&mut self) {
        self.kill();
        let g = self.epoch.load(Ordering::SeqCst);
        let ss = format!("{:.3}", self.base);
        if self.info.video {
            let (w, h) = self.size;
            let child = Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-nostdin", "-ss", &ss, "-re", "-i"])
                .arg(&self.path)
                .args(["-an", "-vf", &format!("scale={w}:{h}"), "-f", "rawvideo", "-pix_fmt", "rgba", "pipe:1"])
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(mut c) => {
                    let mut out = c.stdout.take().unwrap();
                    let (frame, epoch, waker) = (self.frame.clone(), self.epoch.clone(), self.waker.clone());
                    std::thread::spawn(move || {
                        let mut buf = vec![0u8; (w * h * 4) as usize];
                        while out.read_exact(&mut buf).is_ok() && epoch.load(Ordering::SeqCst) == g {
                            let f = Frame { key: NEXT_KEY.fetch_add(1, Ordering::Relaxed), rgba: Arc::new(buf.clone()), w, h };
                            *frame.lock().unwrap() = Some(f);
                            if let Some(w) = &waker {
                                w();
                            }
                        }
                    });
                    self.procs.push(c);
                }
                Err(e) => self.error = Some(format!("ffmpeg: {e}")),
            }
        }
        if self.info.audio {
            let child = Command::new("ffplay")
                .args(["-nodisp", "-autoexit", "-vn", "-loglevel", "quiet", "-ss", &ss, "-volume", &self.volume.to_string()])
                .arg(&self.path)
                .stdin(Stdio::null())
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn();
            match child {
                Ok(c) => self.procs.push(c),
                Err(e) => self.error = Some(format!("ffplay: {e}")),
            }
        }
    }

    pub fn play(&mut self) {
        if self.info.duration > 0.0 && self.base >= self.info.duration - 0.05 {
            self.base = 0.0;
        }
        self.spawn();
        self.since = Some(Instant::now());
    }

    pub fn pause(&mut self) {
        self.base = self.position();
        self.since = None;
        self.kill();
    }

    pub fn toggle(&mut self) {
        if self.playing() { self.pause() } else { self.play() }
    }

    pub fn seek(&mut self, by: f64) {
        let max = if self.info.duration > 0.0 { self.info.duration } else { f64::MAX };
        let playing = self.playing();
        self.base = (self.position() + by).clamp(0.0, max);
        if playing {
            self.since = Some(Instant::now());
            self.spawn();
        }
    }

    pub fn change_volume(&mut self, by: i32) {
        self.volume = (self.volume as i32 + by).clamp(0, 100) as u8;
        if self.playing() {
            self.seek(0.0);
        }
    }

    /// Running helper processes (tests).
    pub fn processes(&mut self) -> usize {
        self.procs.retain_mut(|c| c.try_wait().ok().flatten().is_none());
        self.procs.len()
    }
}

impl Drop for Player {
    fn drop(&mut self) {
        self.kill();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffprobe_json() {
        let j = r#"{"streams":[{"codec_type":"video","width":640,"height":360,"disposition":{"attached_pic":0}},{"codec_type":"audio"}],"format":{"duration":"12.5"}}"#;
        assert_eq!(parse_probe(j), Some(Info { width: 640, height: 360, duration: 12.5, video: true, audio: true }));
        let art = r#"{"streams":[{"codec_type":"audio"},{"codec_type":"video","width":500,"height":500,"disposition":{"attached_pic":1}}],"format":{"duration":"3"}}"#;
        assert_eq!(parse_probe(art).map(|i| (i.video, i.audio)), Some((false, true)), "cover art is not a video");
        assert_eq!(parse_probe(r#"{"streams":[],"format":{}}"#), None);
        assert_eq!(clock(83.9), "1:23");
        assert_eq!(clock(3725.0), "1:02:05");
    }

    /// Real decode when ffmpeg is installed: frames arrive and change; pause stops the helpers.
    #[test]
    fn plays_a_generated_clip() {
        if !available() {
            eprintln!("ffmpeg not installed, skipped");
            return;
        }
        let t = tempfile::tempdir().unwrap();
        let clip = t.path().join("c.mp4");
        let ok = Command::new("ffmpeg")
            .args(["-loglevel", "error", "-f", "lavfi", "-i", "testsrc=size=160x90:rate=10:duration=3", "-pix_fmt", "yuv420p"])
            .arg(&clip)
            .status()
            .is_ok_and(|s| s.success());
        if !ok {
            eprintln!("could not encode a clip, skipped");
            return;
        }
        let mut p = Player::open(&clip, 160, None);
        assert!(p.info.video && (p.info.duration - 3.0).abs() < 0.5, "{:?}", p.info);
        let first = (0..100).find_map(|_| {
            std::thread::sleep(std::time::Duration::from_millis(20));
            p.frame()
        });
        let first = first.expect("a frame");
        assert_eq!((first.w, first.h), (160, 90));
        std::thread::sleep(std::time::Duration::from_millis(600));
        let later = p.frame().unwrap();
        assert_ne!(first.key, later.key);
        assert_ne!(first.rgba, later.rgba, "the picture moves");
        p.pause();
        assert!(!p.playing());
        assert_eq!(p.processes(), 0);
        let at = p.position();
        p.seek(1.0);
        assert!((p.position() - (at + 1.0)).abs() < 0.01);
        p.change_volume(50);
        assert_eq!(p.volume, 100);
    }
}
