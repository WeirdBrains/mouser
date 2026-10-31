//! Clipboard and primary selection through the window's Wayland connection.
//!
//! Setting the primary selection needs a client with keyboard focus
//! (`zwp_primary_selection`); tools such as `wl-copy --primary` go through
//! data-control, which some compositors don't forward. Off Wayland (X11,
//! macOS, Windows) these return false/None and callers fall back to commands.

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use std::sync::{Mutex, OnceLock};

    static CLIP: OnceLock<Mutex<smithay_clipboard::Clipboard>> = OnceLock::new();

    pub fn init(window: &winit::window::Window) {
        use raw_window_handle::{HasDisplayHandle, RawDisplayHandle};
        if let Ok(h) = window.display_handle()
            && let RawDisplayHandle::Wayland(w) = h.as_raw()
        {
            // SAFETY: the display outlives the window and the clipboard, which
            // is only dropped at process exit.
            let c = unsafe { smithay_clipboard::Clipboard::new(w.display.as_ptr()) };
            let _ = CLIP.set(Mutex::new(c));
        }
    }

    pub fn store(text: &str, primary: bool) -> bool {
        let Some(c) = CLIP.get() else { return false };
        let c = c.lock().unwrap();
        if primary {
            c.store_primary(text.to_string())
        } else {
            c.store(text.to_string())
        }
        true
    }

    pub fn load(primary: bool) -> Option<Option<String>> {
        let c = CLIP.get()?.lock().unwrap();
        Some(if primary { c.load_primary() } else { c.load() }.ok())
    }
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
mod imp {
    pub fn init(_: &winit::window::Window) {}
    pub fn store(_: &str, _: bool) -> bool {
        false
    }
    pub fn load(_: bool) -> Option<Option<String>> {
        None
    }
}

type StoreFn = Box<dyn Fn(&str, bool) -> bool + Send + Sync>;
type LoadFn = Box<dyn Fn(bool) -> Option<Option<String>> + Send + Sync>;
static REMOTE: std::sync::RwLock<Option<(StoreFn, LoadFn)>> = std::sync::RwLock::new(None);

/// In the session server (no window of its own): route clipboard requests to
/// the UI process, which owns the Wayland connection (docs/engines.md §8).
pub fn set_remote(store: StoreFn, load: LoadFn) {
    *REMOTE.write().unwrap() = Some((store, load));
}

pub fn clear_remote() {
    *REMOTE.write().unwrap() = None;
}

/// Called once the window exists.
pub fn init(window: &winit::window::Window) {
    imp::init(window)
}

/// Store `text` in the clipboard or the primary selection; false when this
/// window has no Wayland clipboard (use another way).
pub fn store(text: &str, primary: bool) -> bool {
    if let Some((store, _)) = REMOTE.read().unwrap().as_ref() {
        return store(text, primary);
    }
    imp::store(text, primary)
}

/// `None` when there is no Wayland clipboard; `Some(None)` when it is empty.
pub fn load(primary: bool) -> Option<Option<String>> {
    if let Some((_, load)) = REMOTE.read().unwrap().as_ref() {
        return load(primary);
    }
    imp::load(primary)
}
