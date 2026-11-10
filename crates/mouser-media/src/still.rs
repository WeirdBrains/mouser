//! Pictures decoded for a view: RGBA scaled down to fit a box, keyed by the
//! file (path, size and change time) so a host uploads each one once. GIFs
//! show their first frame. Decoding runs on a thread; [`Stills`] keeps the
//! results and wakes the view when one is ready.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

/// Largest picture kept (a fullscreen view in a big window).
pub const MAX_SIDE: u32 = 2048;
/// Files larger than this are not decoded.
pub const MAX_BYTES: u64 = 64 << 20;

#[derive(Debug, Clone)]
pub struct Still {
    pub key: u64,
    pub w: u32,
    pub h: u32,
    pub rgba: Arc<Vec<u8>>,
}

/// The cache key of `path` as it is now (`None`: not a readable file).
pub fn key(path: &Path) -> Option<u64> {
    let m = std::fs::metadata(path).ok().filter(|m| m.is_file())?;
    let mut h = std::collections::hash_map::DefaultHasher::new();
    path.hash(&mut h);
    m.len().hash(&mut h);
    m.modified().ok().hash(&mut h);
    Some(h.finish() | 1 << 62)
}

/// Decode `path`, scaled to fit `max` × `max`.
pub fn decode(path: &Path, max: u32) -> Result<Still, String> {
    let key = key(path).ok_or_else(|| format!("{} is not a readable file", path.display()))?;
    let len = std::fs::metadata(path).map(|m| m.len()).unwrap_or(0);
    if len > MAX_BYTES {
        return Err(format!("{} is too large to show ({} MB)", path.display(), len >> 20));
    }
    let img = image::ImageReader::open(path)
        .map_err(|e| e.to_string())?
        .with_guessed_format()
        .map_err(|e| e.to_string())?
        .decode()
        .map_err(|e| format!("can't show {}: {e}", path.display()))?;
    let max = max.clamp(16, MAX_SIDE);
    let img = if img.width() > max || img.height() > max { img.thumbnail(max, max) } else { img };
    let rgba = img.to_rgba8();
    Ok(Still { key, w: rgba.width(), h: rgba.height(), rgba: Arc::new(rgba.into_raw()) })
}

#[derive(Debug, Clone)]
pub enum Slot {
    Loading,
    Ready(Still),
    Failed(String),
}

/// Pictures being decoded or ready, by path.
#[derive(Default, Clone)]
pub struct Stills {
    map: Arc<Mutex<HashMap<PathBuf, Slot>>>,
}

impl Stills {
    /// The picture at `path`: starts decoding it the first time (then
    /// `Loading` until `wake` is called).
    pub fn get(&self, path: &Path, wake: Option<crate::Waker>) -> Slot {
        let mut m = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(s) = m.get(path) {
            return s.clone();
        }
        m.insert(path.to_path_buf(), Slot::Loading);
        let (map, p) = (self.map.clone(), path.to_path_buf());
        let _ = std::thread::Builder::new().name("still".into()).spawn(move || {
            let s = match decode(&p, MAX_SIDE) {
                Ok(s) => Slot::Ready(s),
                Err(e) => Slot::Failed(e),
            };
            map.lock().unwrap_or_else(|e| e.into_inner()).insert(p, s);
            if let Some(w) = wake {
                w();
            }
        });
        Slot::Loading
    }

    /// Forget everything (a new chat).
    pub fn clear(&self) {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).clear();
    }
}

/// `w` × `h` fitted inside `r`, centred (never scaled up past 1:1 when `grow` is false).
pub fn fit(r: mouser_mode_api::Rect, w: u32, h: u32, grow: bool) -> mouser_mode_api::Rect {
    let (w, h) = (w.max(1) as f64, h.max(1) as f64);
    let mut s = (r.w as f64 / w).min(r.h as f64 / h);
    if !grow {
        s = s.min(1.0);
    }
    let (fw, fh) = ((w * s).round() as i32, (h * s).round() as i32);
    mouser_mode_api::Rect { x: r.x + (r.w - fw) / 2, y: r.y + (r.h - fh) / 2, w: fw.max(1), h: fh.max(1) }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn decodes_scaled_and_keys_by_file() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("a.png");
        image::RgbaImage::from_pixel(300, 100, image::Rgba([255, 0, 0, 255])).save(&p).unwrap();
        let s = decode(&p, 150).unwrap();
        assert_eq!((s.w, s.h, s.rgba.len()), (150, 50, 150 * 50 * 4));
        assert_eq!(key(&p), Some(s.key));
        assert!(decode(&d.path().join("none.png"), 100).is_err());
        std::fs::write(d.path().join("bad.png"), b"not a png").unwrap();
        assert!(decode(&d.path().join("bad.png"), 100).unwrap_err().contains("can't show"));
        let stills = Stills::default();
        let (tx, rx) = std::sync::mpsc::channel();
        let tx = Mutex::new(tx);
        let wake: crate::Waker = Arc::new(move || {
            let _ = tx.lock().unwrap().send(());
        });
        assert!(matches!(stills.get(&p, Some(wake)), Slot::Loading));
        rx.recv_timeout(std::time::Duration::from_secs(5)).unwrap();
        assert!(matches!(stills.get(&p, None), Slot::Ready(s) if s.w == 300));
    }

    #[test]
    fn fits_centred() {
        let r = mouser_mode_api::Rect { x: 0, y: 0, w: 200, h: 100 };
        assert_eq!(fit(r, 400, 100, true), mouser_mode_api::Rect { x: 0, y: 25, w: 200, h: 50 });
        assert_eq!(fit(r, 50, 50, false), mouser_mode_api::Rect { x: 75, y: 25, w: 50, h: 50 });
        assert_eq!(fit(r, 50, 50, true), mouser_mode_api::Rect { x: 50, y: 0, w: 100, h: 100 });
    }
}
