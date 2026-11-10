//! Audio output: which cpal host and device the player uses (docs/preferences.md
//! §Audio). `audio.host` is `auto`, `jack` or `alsa` on Linux (elsewhere only the
//! system host exists); `audio.output_device` names a device of that host, unset
//! meaning the host's default. A device that is gone falls back to the default.
//!
//! JACK is the optional `jack` cargo feature (cpal's `jack`). Building it needs
//! the JACK headers and `jack.pc` (Debian/Ubuntu `libjack-jackd2-dev`, Arch
//! `pipewire-jack` or `jack2`); libjack itself is loaded at run time, so a
//! machine without it still plays through ALSA. Without the feature `jack`
//! and `auto` use ALSA and preferences say "JACK not in this build". PipeWire's
//! JACK (pipewire-jack) counts as a JACK server.

use cpal::traits::{DeviceTrait, HostTrait};
use std::time::{Duration, Instant};

/// What `audio.host` asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostPref {
    Auto,
    Jack,
    Alsa,
}

/// The `audio.host` choices on this platform.
pub fn host_choices() -> &'static [&'static str] {
    if cfg!(target_os = "linux") { &["auto", "jack", "alsa"] } else { &["auto"] }
}

pub fn parse_host(s: Option<&str>) -> HostPref {
    match s.map(|s| s.trim().to_ascii_lowercase()).as_deref() {
        Some("jack") => HostPref::Jack,
        Some("alsa") => HostPref::Alsa,
        _ => HostPref::Auto,
    }
}

/// Hosts to try, in order: JACK first for `auto` (when a server answers), then
/// the platform default. `jack` still falls back so sound never just stops.
pub fn order(pref: HostPref) -> Vec<cpal::HostId> {
    #[cfg(target_os = "linux")]
    {
        #[cfg(feature = "jack")]
        let jack = cpal::available_hosts().into_iter().find(|h| *h == cpal::HostId::Jack);
        #[cfg(not(feature = "jack"))]
        let jack: Option<cpal::HostId> = None;
        let alsa = cpal::HostId::Alsa;
        return match (pref, jack) {
            (HostPref::Alsa, _) | (_, None) => vec![alsa],
            (HostPref::Auto | HostPref::Jack, Some(j)) => vec![j, alsa],
        };
    }
    #[allow(unreachable_code)]
    {
        let _ = pref;
        vec![cpal::default_host().id()]
    }
}

/// The index of the device to use: the named one, else the default's, else the first.
pub fn choose(names: &[String], wanted: Option<&str>, default: Option<&str>) -> Option<usize> {
    let wanted = wanted.filter(|w| !w.is_empty() && *w != "default");
    wanted
        .and_then(|w| names.iter().position(|n| n == w))
        .or_else(|| default.and_then(|d| names.iter().position(|n| n == d)))
        .or(if names.is_empty() { None } else { Some(0) })
}

/// The output device to open, its host's name, and a notice when the chosen
/// device was missing (the player shows it).
pub fn open(pref_host: Option<&str>, pref_device: Option<&str>) -> Result<(cpal::Device, &'static str, Option<String>), String> {
    for id in order(parse_host(pref_host)) {
        let Ok(host) = cpal::host_from_id(id) else { continue };
        // A JACK host with no server reachable has no default device.
        let Some(def) = host.default_output_device() else { continue };
        let def_name = def.name().ok();
        let wanted = pref_device.filter(|w| !w.is_empty() && *w != "default");
        let Some(w) = wanted else { return Ok((def, id.name(), None)) };
        let devs: Vec<cpal::Device> = host.output_devices().map(|d| d.collect()).unwrap_or_default();
        let names: Vec<String> = devs.iter().map(|d| d.name().unwrap_or_default()).collect();
        let notice = (!names.iter().any(|n| n == w)).then(|| format!("audio device \"{w}\" not found; using the default"));
        let dev = match choose(&names, Some(w), def_name.as_deref()) {
            Some(i) if notice.is_none() => devs.into_iter().nth(i).unwrap_or(def),
            _ => def,
        };
        return Ok((dev, id.name(), notice));
    }
    Err("no audio output device".into())
}

/// Probing runs on one long-lived thread: cpal hosts (WASAPI's COM above all)
/// must not be enumerated from many threads at once.
fn probe<T: Send + 'static>(f: impl FnOnce() -> T + Send + 'static) -> T {
    type Job = Box<dyn FnOnce() + Send>;
    static WORKER: std::sync::OnceLock<std::sync::Mutex<std::sync::mpsc::Sender<Job>>> = std::sync::OnceLock::new();
    let tx = WORKER.get_or_init(|| {
        let (tx, rx) = std::sync::mpsc::channel::<Job>();
        std::thread::Builder::new()
            .name("audio-probe".into())
            .spawn(move || {
                for job in rx {
                    job();
                }
            })
            .expect("audio probe thread");
        std::sync::Mutex::new(tx)
    });
    let (rtx, rrx) = std::sync::mpsc::channel();
    let job: Job = Box::new(move || {
        let _ = rtx.send(f());
    });
    tx.lock().unwrap().send(job).expect("audio probe thread");
    rrx.recv().expect("audio probe thread")
}

/// The host `audio.host` resolves to right now (`"none"` without one).
pub fn active_host(pref_host: Option<&str>) -> &'static str {
    if cfg!(all(target_os = "linux", not(feature = "jack"))) && parse_host(pref_host) == HostPref::Jack {
        return "ALSA (JACK not in this build)";
    }
    let pref = parse_host(pref_host);
    probe(move || {
        for id in order(pref) {
            if let Ok(h) = cpal::host_from_id(id)
                && h.default_output_device().is_some()
            {
                return id.name();
            }
        }
        "none"
    })
}

/// Output device names of the host `audio.host` resolves to (cached briefly:
/// enumerating ALSA or connecting to JACK takes a moment).
pub fn output_devices(pref_host: Option<&str>) -> Vec<String> {
    static CACHE: std::sync::Mutex<Option<(Instant, HostPref, Vec<String>)>> = std::sync::Mutex::new(None);
    let pref = parse_host(pref_host);
    if let Some((t, p, v)) = CACHE.lock().unwrap().as_ref()
        && *p == pref
        && t.elapsed() < Duration::from_secs(3)
    {
        return v.clone();
    }
    let v = probe(move || {
        let mut v = Vec::new();
        for id in order(pref) {
            let Ok(host) = cpal::host_from_id(id) else { continue };
            if host.default_output_device().is_none() {
                continue;
            }
            if let Ok(devs) = host.output_devices() {
                v = devs.filter_map(|d| d.name().ok()).collect();
            }
            break;
        }
        v.dedup();
        v
    });
    *CACHE.lock().unwrap() = Some((Instant::now(), pref, v.clone()));
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    fn n(v: &[&str]) -> Vec<String> {
        v.iter().map(|s| s.to_string()).collect()
    }

    #[test]
    fn the_host_pref_parses_and_unknown_means_auto() {
        assert_eq!(parse_host(None), HostPref::Auto);
        assert_eq!(parse_host(Some("JACK")), HostPref::Jack);
        assert_eq!(parse_host(Some("alsa")), HostPref::Alsa);
        assert_eq!(parse_host(Some("pulse")), HostPref::Auto);
    }

    #[test]
    fn a_named_device_wins_and_a_missing_one_falls_back_to_the_default() {
        let names = n(&["hw:0", "pipewire", "default"]);
        assert_eq!(choose(&names, Some("pipewire"), Some("default")), Some(1));
        assert_eq!(choose(&names, Some("gone"), Some("default")), Some(2));
        assert_eq!(choose(&names, Some("default"), Some("hw:0")), Some(0));
        assert_eq!(choose(&names, None, None), Some(0));
        assert_eq!(choose(&[], Some("x"), None), None);
    }

    #[test]
    fn alsa_is_always_the_fallback_on_linux_and_the_system_host_elsewhere() {
        let o = order(HostPref::Alsa);
        assert_eq!(o.len(), 1);
        let auto = order(HostPref::Auto);
        assert_eq!(auto.last(), o.last());
        #[cfg(target_os = "linux")]
        assert_eq!(o, vec![cpal::HostId::Alsa]);
        #[cfg(not(target_os = "linux"))]
        assert_eq!(o, vec![cpal::default_host().id()]);
    }

    #[test]
    fn probing_never_panics_without_sound_hardware() {
        let _ = active_host(None);
        let _ = output_devices(Some("alsa"));
        let _ = open(Some("alsa"), Some("no such device"));
    }
}
