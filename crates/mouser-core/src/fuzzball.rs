//! Fuzzball terminal side: capability query, graphics (upload / place / delete /
//! query), keyboard mode, plugins and shell integration.
//!
//! The wire format (params, framing, replies, key events, percent-encoding) comes from
//! the [`fuzzball`] crate; the spec lives at <https://weirdbrains.github.io/fuzzball/>.
//! This module keeps the terminal's state: upload assembly, images and placements,
//! keyboard mode per screen, plugin grants and the shell's advertisement.
//!
//! Messages arrive as the body of `DCS = F z <sub><verb> ; <params> [: payload] ST`
//! (the parser strips `DCS = F`; the body here starts with `z`).

use crate::graphics::{self, Graphics, Placement};
use fuzzball::keyboard::{Key, KeyKind, Locks, Modifiers};
use fuzzball::{ErrorCode as Error, Params, Reply};
use std::collections::HashMap;
use std::time::{Duration, Instant};

pub use fuzzball::shell::{pct_decode, pct_encode};
/// Protocol version implemented (`v`).
pub const VERSION: u32 = fuzzball::VERSION;
/// Largest payload accepted per message, in wire bytes (`max_chunk`).
pub const MAX_CHUNK: usize = fuzzball::MAX_CHUNK;
/// Largest DCS body the parser keeps for a Fuzzball message.
pub const MAX_MESSAGE: usize = fuzzball::DEFAULT_MAX_MESSAGE;
/// Partial uploads are dropped after this long without a chunk.
pub const UPLOAD_TIMEOUT: Duration = Duration::from_secs(10);

/// Capability reply body (after `id=N,`).
pub fn capabilities() -> String {
    format!(
        "v={VERSION},sub=gkps,raw=0,max_chunk={MAX_CHUNK},g_mem={},g_max_dim={},g_fmt=rgba:rgb:png,g_max_place={},g_fit=w,g_reflow=1,k=1,k_ime=0,k_release=1",
        graphics::MAX_BYTES,
        graphics::MAX_DIM,
        graphics::MAX_PLACEMENTS
    )
}

/// Where the cursor is, for placements.
#[derive(Debug, Clone, Copy)]
pub struct Ctx {
    /// Absolute row of the cursor (lines scrolled off + screen row).
    pub abs_row: i64,
    pub col: u32,
    pub cell_w: u32,
    pub cell_h: u32,
    pub alt: bool,
    /// Screen width in columns.
    pub screen_cols: usize,
}

/// What the parser should do after a message.
#[derive(Debug, Default, PartialEq, Eq)]
pub struct Effect {
    pub reply: Option<String>,
    /// Move the cursor this many cells right of the anchor and down, like printed text.
    pub cursor: Option<(u32, u32)>,
    /// Graphics changed (redraw).
    pub changed: bool,
}

#[derive(Debug)]
struct Upload {
    id: u32,
    fmt: Fmt,
    w: u32,
    h: u32,
    size: usize,
    data: Vec<u8>,
    last: Instant,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Fmt {
    Rgba,
    Rgb,
    Png,
}

#[derive(Debug, Default)]
pub struct Fuzzball {
    pub gfx: Graphics,
    upload: Option<Upload>,
    /// Keyboard mode per screen: [main, alternate].
    keyboard: [bool; 2],
    /// Bytes the Kitty graphics store holds of the shared quota.
    pub other_mem: usize,
    /// `p h` handshakes waiting for the user (the app answers with [`plugin_reply`]).
    pub plugin_hellos: Vec<PluginHello>,
    /// Plugins this tab granted, with their capabilities (gone with the tab).
    pub plugin_grants: HashMap<String, Vec<String>>,
    /// What the shell integration advertised (`s a`); None while a command runs.
    pub shell: Option<ShellCaps>,
    /// The last integration this tab advertised, kept while a command runs:
    /// tells "busy, catches up at the next prompt" from "no integration".
    pub shell_seen: Option<ShellCaps>,
    /// `s r` replies from the shell: (id, ok, msg).
    pub shell_replies: Vec<(u32, bool, String)>,
}

/// A shell integration's advertisement (`DCS = F z s a ; v=0,caps=cwd,shell=zsh ST`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShellCaps {
    pub shell: String,
    pub caps: Vec<String>,
    /// `file=PCT` with `caps=cwdfile` (PowerShell): the shell reads a folder
    /// to change to from this file at its next prompt.
    pub file: Option<std::path::PathBuf>,
}

impl ShellCaps {
    pub fn has(&self, cap: &str) -> bool {
        self.caps.iter().any(|c| c == cap)
    }
}

/// The request Mouser writes to the shell's input to change directory
/// silently (`s c`, only after the shell advertised `caps=cwd`).
pub fn shell_cd(id: u32, path: &[u8]) -> Vec<u8> {
    fuzzball::shell::ChangeDir::new(id, path).encode()
}

/// A program in the tab asks to act as a plugin (`DCS = F z p h ; id=N,name=…,caps=a+b ST`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PluginHello {
    pub id: u32,
    pub name: String,
    pub kind: String,
    pub caps: Vec<String>,
}

/// Most handshakes queued per tab (more are refused with EBUSY-like EPERM).
pub const MAX_HELLOS: usize = 4;

/// The reply to a handshake once the user answered.
pub fn plugin_reply(id: u32, granted: Option<&[String]>) -> String {
    let b = match granted {
        Some(c) => fuzzball::plugins::granted(id, c),
        None => fuzzball::plugins::denied(id),
    };
    String::from_utf8_lossy(&b).into_owned()
}

fn num(p: &Params, k: &str) -> Option<u32> {
    p.num(k)
}

/// An error reply `ESC P = F z <sub> r ; id=N,ok=0,err=…,msg=… ST`.
fn err_reply(sub: u8, id: u32, e: Error, msg: &str) -> String {
    Reply::error(sub, id, e, msg).to_message().encode_string()
}

type Res = Result<String, (Error, &'static str)>;

fn eff_err(id: Option<u32>, quiet: u32, e: Error, msg: &str) -> Effect {
    Effect { reply: id.filter(|_| quiet < 2).map(|id| err_reply(b's', id, e, msg)), ..Default::default() }
}

impl Fuzzball {
    pub fn keyboard(&self, alt: bool) -> bool {
        self.keyboard[alt as usize]
    }

    /// The alternate screen was left: its keyboard mode and placements go with it.
    pub fn leave_alt(&mut self) {
        self.keyboard[1] = false;
        self.gfx.leave_alt();
    }

    /// Drop a partial upload idle for more than [`UPLOAD_TIMEOUT`] at `now`.
    /// Returns when the pending upload (if any) will expire.
    pub fn expire_upload(&mut self, now: Instant) -> Option<Instant> {
        let deadline = self.upload.as_ref()?.last + UPLOAD_TIMEOUT;
        if now >= deadline {
            self.upload = None;
            return None;
        }
        Some(deadline)
    }

    /// Whether a chunked upload is in progress.
    pub fn upload_pending(&self) -> bool {
        self.upload.is_some()
    }

    /// Handle one message body (starting with `z`).
    pub fn handle(&mut self, body: &[u8], ctx: Ctx) -> Effect {
        let Some(body) = body.strip_prefix(b"z") else { return Effect::default() };
        if body.len() < 2 {
            return Effect::default();
        }
        let (sub, verb) = (body[0] as char, body[1] as char);
        let rest = &body[2..];
        let rest = rest.strip_prefix(b";").unwrap_or(rest);
        let (params, payload) = match rest.iter().position(|&b| b == b':') {
            Some(i) => (&rest[..i], Some(&rest[i + 1..])),
            None => (rest, None),
        };
        let p = Params::parse(&String::from_utf8_lossy(params));
        let id = num(&p, "id").filter(|&i| i > 0);
        let quiet = num(&p, "quiet").unwrap_or(0);
        let mut eff = Effect::default();
        let res: Res = match (sub, verb) {
            ('c', 'q') => Ok(capabilities()),
            ('g', 'u') => self.upload(&p, payload, &mut eff),
            ('g', 'p') => self.place(&p, ctx, &mut eff),
            ('g', 'd') => self.delete(&p, ctx, &mut eff),
            ('g', 'q') => Ok(format!(
                "ok=1,mem_used={},mem_max={},images={},cell_w={},cell_h={}",
                self.gfx.mem_used(),
                graphics::MAX_BYTES,
                self.gfx.images.len(),
                ctx.cell_w,
                ctx.cell_h
            )),
            ('k', 's') => self.keyboard_set(&p, ctx.alt),
            ('k', 'q') => Ok(format!("ok=1,on={}", u8::from(self.keyboard(ctx.alt)))),
            ('p', 'h') => self.plugin_hello(&p, id),
            ('p', 'q') => match p.get("name") {
                Some(n) => Ok(match self.plugin_grants.get(n) {
                    Some(c) => format!("ok=1,granted={}", c.join("+")),
                    None => "ok=1,granted=".to_string(),
                }),
                None => Err((Error::Invalid, "name is required")),
            },
            ('p', 'b') => match p.get("name") {
                Some(n) => {
                    self.plugin_grants.remove(n);
                    Ok("ok=1".into())
                }
                None => Err((Error::Invalid, "name is required")),
            },
            ('s', 'a') => {
                self.shell = match p.get("caps") {
                    None | Some("") => None,
                    Some(c) if num(&p, "v").is_none_or(|v| v == VERSION) => {
                        let file = p.get("file").and_then(pct_decode).map(|f| std::path::PathBuf::from(String::from_utf8_lossy(&f).into_owned()));
                        let caps =
                            ShellCaps { shell: p.get("shell").unwrap_or_default().to_string(), caps: c.split('+').map(String::from).collect(), file };
                        self.shell_seen = Some(caps.clone());
                        Some(caps)
                    }
                    Some(_) => return eff_err(id, quiet, Error::Version, "version not available"),
                };
                Ok(if id.is_some() { "ok=1".into() } else { String::new() })
            }
            ('s', 'r') => {
                if let Some(id) = id {
                    let msg = p.get("msg").and_then(pct_decode).map(|m| String::from_utf8_lossy(&m).into_owned()).unwrap_or_default();
                    self.shell_replies.push((id, num(&p, "ok") == Some(1), msg));
                    if self.shell_replies.len() > 16 {
                        self.shell_replies.remove(0);
                    }
                }
                Ok(String::new())
            }
            _ => Err((Error::NotSupported, "unknown subsystem or verb")),
        };
        // Continuation chunks with more=1 stay silent.
        let silent = matches!(&res, Ok(s) if s.is_empty());
        // `s r` is itself a reply: never answered.
        if (sub, verb) == ('s', 'r') {
            return eff;
        }
        let reply_sub = if matches!(sub, 'c' | 'g' | 'k' | 'p' | 's') { sub as u8 } else { b'c' };
        eff.reply = match (id, res) {
            (Some(id), Ok(s)) if !silent && quiet == 0 => Some(Reply::raw(reply_sub, id, Params::parse(&s)).to_message().encode_string()),
            (Some(id), Err((e, msg))) if quiet < 2 => Some(err_reply(reply_sub, id, e, msg)),
            _ => None,
        };
        eff
    }

    /// The shell left its prompt (or exited): no silent actions until it advertises again.
    pub fn shell_gone(&mut self) {
        self.shell = None;
    }

    /// Queue a handshake for the user; the reply comes later (silent now).
    fn plugin_hello(&mut self, p: &Params, id: Option<u32>) -> Res {
        let Some(id) = id else { return Err((Error::Invalid, "id is required")) };
        if num(p, "v").is_some_and(|v| v != VERSION) {
            return Err((Error::Version, "version not available"));
        }
        let name = p.get("name").unwrap_or_default().to_string();
        if !crate::engines::valid_name(&name) {
            return Err((Error::Invalid, "name must be a-z, 0-9, - or _"));
        }
        let kind = p.get("kind").unwrap_or("tool-provider").to_string();
        if !matches!(kind.as_str(), "mode" | "model-provider" | "tool-provider" | "command") {
            return Err((Error::Invalid, "unknown kind"));
        }
        let caps: Vec<String> = p.get("caps").map(|c| c.split('+').filter(|c| !c.is_empty()).map(str::to_string).collect()).unwrap_or_default();
        if caps.len() > 9 || caps.iter().any(|c| !crate::plugins::known_capability(c)) {
            return Err((Error::Invalid, "unknown capability"));
        }
        if self.plugin_hellos.len() >= MAX_HELLOS {
            return Err((Error::Permission, "too many pending requests"));
        }
        self.plugin_hellos.push(PluginHello { id, name, kind, caps });
        Ok(String::new())
    }

    fn keyboard_set(&mut self, p: &Params, alt: bool) -> Res {
        if num(p, "v").is_some_and(|v| v != VERSION) {
            return Err((Error::Version, "version not available"));
        }
        let on = match num(p, "on") {
            Some(1) => true,
            Some(0) => false,
            _ => return Err((Error::Invalid, "on must be 0 or 1")),
        };
        self.keyboard[alt as usize] = on;
        Ok(format!("ok=1,on={}", u8::from(on)))
    }

    fn upload(&mut self, p: &Params, payload: Option<&[u8]>, eff: &mut Effect) -> Res {
        let id = num(p, "id").filter(|&i| i > 0).ok_or((Error::Invalid, "id required"))?;
        if let Some(u) = &self.upload
            && (u.id != id || u.last.elapsed() > UPLOAD_TIMEOUT)
        {
            let stale = u.last.elapsed() > UPLOAD_TIMEOUT;
            self.upload = None;
            if !stale {
                return Err((Error::Protocol, "chunk for another id aborted the upload"));
            }
        }
        match p.get("enc").unwrap_or("b64") {
            "b64" => {}
            "raw" => {
                self.upload = None;
                return Err((Error::Permission, "raw payloads are not enabled"));
            }
            _ => {
                self.upload = None;
                return Err((Error::NotSupported, "unknown enc"));
            }
        }
        let payload = payload.ok_or((Error::Invalid, "payload required"))?;
        let len = num(p, "len").ok_or((Error::Invalid, "len required"))? as usize;
        if len != payload.len() {
            self.upload = None;
            return Err((Error::Protocol, "len does not match payload"));
        }
        if len > MAX_CHUNK {
            self.upload = None;
            return Err((Error::NoMemory, "chunk larger than max_chunk"));
        }
        let bytes = graphics::base64_decode(payload).ok_or((Error::Invalid, "bad base64"));
        let bytes = match bytes {
            Ok(b) => b,
            Err(e) => {
                self.upload = None;
                return Err(e);
            }
        };
        let more = num(p, "more") == Some(1);
        if self.upload.is_none() {
            if let Some(cs) = p.get("cs")
                && cs != "srgb"
            {
                return Err((Error::NotSupported, "only cs=srgb is supported"));
            }
            let fmt = match p.get("fmt") {
                Some("rgba") => Fmt::Rgba,
                Some("rgb") => Fmt::Rgb,
                Some("png") => Fmt::Png,
                Some(_) => return Err((Error::NotSupported, "unknown fmt")),
                None => return Err((Error::Invalid, "fmt required")),
            };
            let size = num(p, "size").ok_or((Error::Invalid, "size required"))? as usize;
            let (w, h) = (num(p, "w").unwrap_or(0), num(p, "h").unwrap_or(0));
            if fmt != Fmt::Png {
                if w == 0 || h == 0 {
                    return Err((Error::Invalid, "w and h required"));
                }
                if w > graphics::MAX_DIM || h > graphics::MAX_DIM {
                    return Err((Error::NoMemory, "larger than g_max_dim"));
                }
                let bpp = if fmt == Fmt::Rgba { 4 } else { 3 };
                if size != w as usize * h as usize * bpp {
                    return Err((Error::Invalid, "size does not match w*h"));
                }
            }
            if size > graphics::MAX_BYTES {
                return Err((Error::NoMemory, "larger than g_mem"));
            }
            self.upload = Some(Upload { id, fmt, w, h, size, data: Vec::with_capacity(size.min(MAX_CHUNK * 4)), last: Instant::now() });
        }
        let u = self.upload.as_mut().unwrap();
        u.data.extend_from_slice(&bytes);
        u.last = Instant::now();
        if u.data.len() > u.size {
            self.upload = None;
            return Err((Error::Protocol, "more data than size"));
        }
        if more {
            return Ok(String::new());
        }
        let u = self.upload.take().unwrap();
        if u.data.len() != u.size {
            return Err((Error::Protocol, "less data than size"));
        }
        let (w, h, rgba) = match u.fmt {
            Fmt::Rgba => (u.w, u.h, u.data),
            Fmt::Rgb => (u.w, u.h, u.data.as_chunks::<3>().0.iter().flat_map(|p| [p[0], p[1], p[2], 255]).collect()),
            Fmt::Png => graphics::decode_png(&u.data).map_err(|_| (Error::Invalid, "bad png"))?,
        };
        self.gfx.insert(id, w, h, rgba, graphics::MAX_BYTES.saturating_sub(self.other_mem)).map_err(|_| (Error::NoMemory, "over g_mem"))?;
        eff.changed = true;
        Ok(format!("ok=1,w={w},h={h}"))
    }

    fn place(&mut self, p: &Params, ctx: Ctx, eff: &mut Effect) -> Res {
        let id = num(p, "id").filter(|&i| i > 0).ok_or((Error::Invalid, "id required"))?;
        let img = self.gfx.images.get(&id).ok_or((Error::NotFound, "no such image"))?;
        let (iw, ih) = (img.width, img.height);
        let sx = num(p, "sx").unwrap_or(0);
        let sy = num(p, "sy").unwrap_or(0);
        if sx >= iw || sy >= ih {
            return Err((Error::Invalid, "source rectangle outside image"));
        }
        let sw = num(p, "sw").unwrap_or(iw - sx).min(iw - sx);
        let sh = num(p, "sh").unwrap_or(ih - sy).min(ih - sy);
        if sw == 0 || sh == 0 {
            return Err((Error::Invalid, "empty source rectangle"));
        }
        let (cw, ch) = (ctx.cell_w.max(1) as u64, ctx.cell_h.max(1) as u64);
        let (cols, rows) = match (num(p, "cols").filter(|&c| c > 0), num(p, "rows").filter(|&r| r > 0)) {
            (Some(c), Some(r)) => (c, r),
            (Some(c), None) => (c, ((c as u64 * cw * sh as u64).div_ceil(sw as u64 * ch)).max(1) as u32),
            (None, Some(r)) => (((r as u64 * ch * sw as u64).div_ceil(sh as u64 * cw)).max(1) as u32, r),
            (None, None) => ((sw as u64).div_ceil(cw) as u32, (sh as u64).div_ceil(ch) as u32),
        };
        // `fit=w`: as wide as the screen right of the cursor, rows from the aspect.
        let fill = match p.get("fit") {
            None => false,
            Some("w") => true,
            Some(_) => return Err((Error::Invalid, "fit must be w")),
        };
        let (cols, rows) = if fill {
            let c = (ctx.screen_cols.max(1) as u32).saturating_sub(ctx.col).max(1);
            (c, ((c as u64 * cw * sh as u64).div_ceil(sw as u64 * ch)).max(1) as u32)
        } else {
            (cols, rows)
        };
        if cols > 1000 || rows > 1000 {
            return Err((Error::Invalid, "placement too large"));
        }
        let z = match p.get("z").unwrap_or("0") {
            "-1" => -1,
            "0" => 0,
            "1" => 1,
            _ => return Err((Error::Invalid, "z must be -1 0 or 1")),
        };
        let pid = num(p, "pid").unwrap_or(0);
        let mut pl = Placement {
            image_id: id,
            id: pid,
            row: ctx.abs_row,
            col: ctx.col,
            cols,
            rows,
            req_col: ctx.col,
            req_cols: cols,
            req_rows: rows,
            src: (sx, sy, sw, sh),
            z,
            alt: ctx.alt,
            fill,
            ..Placement::default()
        };
        pl.fit(ctx.screen_cols);
        let (cols, rows) = (pl.cols, pl.rows);
        self.gfx.place(pl).map_err(|_| (Error::NoMemory, "over g_max_place"))?;
        eff.changed = true;
        if num(p, "move").unwrap_or(1) == 1 {
            eff.cursor = Some((cols, rows - 1));
        }
        Ok("ok=1".into())
    }

    fn delete(&mut self, p: &Params, ctx: Ctx, eff: &mut Effect) -> Res {
        let free = num(p, "free") == Some(1);
        let alt = ctx.alt;
        match p.get("what") {
            Some("all") => self.gfx.delete(free, |q| q.alt == alt),
            Some("image") => {
                let id = num(p, "id").ok_or((Error::Invalid, "id required"))?;
                if !self.gfx.images.contains_key(&id) {
                    return Err((Error::NotFound, "no such image"));
                }
                let pid = num(p, "pid");
                self.gfx.delete(free, |q| q.alt == alt && q.image_id == id && pid.is_none_or(|x| q.id == x));
                if free && pid.is_none() {
                    self.gfx.remove_image(id);
                }
            }
            Some("cell") => {
                let (r, c) = (ctx.abs_row, ctx.col);
                self.gfx.delete(free, |q| q.alt == alt && r >= q.row && r < q.row + q.rows as i64 && c >= q.col && c < q.col + q.cols);
            }
            Some(_) => return Err((Error::NotSupported, "unknown what")),
            None => return Err((Error::Invalid, "what required")),
        }
        eff.changed = true;
        Ok("ok=1".into())
    }
}

/// Fuzzball keyboard modifier bits (keyboard.md §2).
pub mod mods {
    use fuzzball::keyboard::Modifiers as M;
    pub const SHIFT: u8 = M::SHIFT.bits();
    pub const ALT: u8 = M::ALT.bits();
    pub const CTRL: u8 = M::CTRL.bits();
    pub const SUPER: u8 = M::SUPER.bits();
    pub const HYPER: u8 = M::HYPER.bits();
    pub const META: u8 = M::META.bits();
    pub const ALTGR: u8 = M::ALT_GR.bits();
}

/// Press, repeat or release.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KeyAction {
    Press,
    Repeat,
    Release,
}

/// One key event for Fuzzball keyboard mode.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct KeyEvent {
    pub action: KeyAction,
    /// W3C UI Events `code`, e.g. `KeyA`.
    pub code: String,
    /// W3C `key` name, or the unmodified character it produces.
    pub key: KeyName,
    pub mods: u8,
    pub locks: u8,
    /// Text inserted (presses and repeats only).
    pub text: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum KeyName {
    Named(String),
    Char(char),
}

/// `DCS = F z k e ; … ST` for an event.
pub fn encode_key(e: &KeyEvent) -> String {
    let kind = match e.action {
        KeyAction::Press => KeyKind::Press,
        KeyAction::Repeat => KeyKind::Repeat,
        KeyAction::Release => KeyKind::Release,
    };
    let key = match &e.key {
        KeyName::Named(n) => Key::Named(n.clone()),
        KeyName::Char(c) => Key::Char(*c),
    };
    let mut ev =
        fuzzball::keyboard::KeyEvent::new(kind, e.code.clone(), key).with_mods(Modifiers::from_bits(e.mods)).with_locks(Locks::from_bits(e.locks));
    ev.text = e.text.clone();
    ev.to_message().encode_string()
}
