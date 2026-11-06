//! Screen colour sampler for the theme editor Dropper button.
//!
//! Prefers `hyprpicker` (Hyprland/Omarchy), then `grim`+`slurp` (any wlroots),
//! then ImageMagick `import` on X11. Returns `None` when nothing is available
//! or the user cancels.

use std::process::{Command, Stdio};

/// Pick one screen pixel as `0xRRGGBB`. Blocks until the user clicks or cancels.
pub fn pick() -> Result<u32, String> {
    if which("hyprpicker") {
        return hyprpicker();
    }
    if which("grim") && which("slurp") {
        return grim_slurp();
    }
    if which("import") {
        return imagemagick_import();
    }
    Err("No colour picker found (install hyprpicker, or grim+slurp)".into())
}

fn which(bin: &str) -> bool {
    Command::new("sh").args(["-c", &format!("command -v {bin} >/dev/null 2>&1")]).status().map(|s| s.success()).unwrap_or(false)
}

fn hyprpicker() -> Result<u32, String> {
    let out = Command::new("hyprpicker")
        .args(["-f", "hex", "-b", "-q"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("hyprpicker: {e}"))?;
    if !out.status.success() {
        return Err("Dropper cancelled".into());
    }
    parse_hex_bytes(&out.stdout)
}

fn grim_slurp() -> Result<u32, String> {
    let geom = Command::new("slurp").args(["-p"]).stdout(Stdio::piped()).stderr(Stdio::null()).output().map_err(|e| format!("slurp: {e}"))?;
    if !geom.status.success() {
        return Err("Dropper cancelled".into());
    }
    let region = String::from_utf8_lossy(&geom.stdout).trim().to_string();
    if region.is_empty() {
        return Err("Dropper cancelled".into());
    }
    let out = Command::new("grim")
        .args(["-g", &region, "-t", "ppm", "-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("grim: {e}"))?;
    if !out.status.success() {
        return Err("grim failed".into());
    }
    parse_ppm_pixel(&out.stdout)
}

fn imagemagick_import() -> Result<u32, String> {
    // One-pixel crop under the pointer after the user clicks.
    let out = Command::new("import")
        .args(["-window", "root", "-crop", "1x1", "+repage", "ppm:-"])
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .output()
        .map_err(|e| format!("import: {e}"))?;
    if !out.status.success() {
        return Err("Dropper cancelled".into());
    }
    parse_ppm_pixel(&out.stdout)
}

fn parse_hex_bytes(raw: &[u8]) -> Result<u32, String> {
    let s = std::str::from_utf8(raw).map_err(|_| "picker: not utf8".to_string())?;
    let s = s.trim().trim_start_matches('#');
    // hyprpicker may print ANSI colour codes around the hex when -b is not enough; strip non-hex.
    let hex: String = s.chars().filter(|c| c.is_ascii_hexdigit()).take(6).collect();
    if hex.len() != 6 {
        return Err(format!("picker: bad hex '{s}'"));
    }
    u32::from_str_radix(&hex, 16).map_err(|_| format!("picker: bad hex '{hex}'"))
}

/// Minimal PPM (P6 or P3) reader: first pixel only.
fn parse_ppm_pixel(data: &[u8]) -> Result<u32, String> {
    let mut i = 0usize;
    fn skip_ws_and_comments(data: &[u8], i: &mut usize) {
        loop {
            while *i < data.len() && data[*i].is_ascii_whitespace() {
                *i += 1;
            }
            if *i < data.len() && data[*i] == b'#' {
                while *i < data.len() && data[*i] != b'\n' {
                    *i += 1;
                }
                continue;
            }
            break;
        }
    }
    let tok = |data: &[u8], i: &mut usize| -> Result<String, String> {
        skip_ws_and_comments(data, i);
        let start = *i;
        while *i < data.len() && !data[*i].is_ascii_whitespace() {
            *i += 1;
        }
        if start == *i {
            return Err("ppm: truncated".into());
        }
        String::from_utf8(data[start..*i].to_vec()).map_err(|_| "ppm: utf8".into())
    };
    let magic = tok(data, &mut i)?;
    let _w = tok(data, &mut i)?;
    let _h = tok(data, &mut i)?;
    let max: u32 = tok(data, &mut i)?.parse().map_err(|_| "ppm max".to_string())?;
    if max == 0 || max > 65535 {
        return Err("ppm: bad max".into());
    }
    while i < data.len() && (data[i] == b' ' || data[i] == b'\t' || data[i] == b'\r') {
        i += 1;
    }
    if i < data.len() && data[i] == b'\n' {
        i += 1;
    }
    let scale = |v: u32| -> u32 { if max == 255 { v } else { (v * 255 / max).min(255) } };
    let (r, g, b) = match magic.as_str() {
        "P6" => {
            let mut read = || -> Result<u32, String> {
                if max < 256 {
                    if i >= data.len() {
                        return Err("ppm: short".into());
                    }
                    let v = data[i] as u32;
                    i += 1;
                    Ok(v)
                } else {
                    if i + 1 >= data.len() {
                        return Err("ppm: short".into());
                    }
                    let v = u16::from_be_bytes([data[i], data[i + 1]]) as u32;
                    i += 2;
                    Ok(v)
                }
            };
            (read()?, read()?, read()?)
        }
        "P3" => {
            let a: u32 = tok(data, &mut i)?.parse().map_err(|_| "ppm".to_string())?;
            let b: u32 = tok(data, &mut i)?.parse().map_err(|_| "ppm".to_string())?;
            let c: u32 = tok(data, &mut i)?.parse().map_err(|_| "ppm".to_string())?;
            (a, b, c)
        }
        _ => return Err(format!("ppm: unsupported {magic}")),
    };
    Ok((scale(r) << 16) | (scale(g) << 8) | scale(b))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_hex() {
        assert_eq!(parse_hex_bytes(b"#2FD8E5\n").unwrap(), 0x2FD8E5);
        assert_eq!(parse_hex_bytes(b"04192A").unwrap(), 0x04192A);
    }

    #[test]
    fn parses_p6() {
        let mut ppm = b"P6\n1 1\n255\n".to_vec();
        ppm.extend_from_slice(&[0x2F, 0xD8, 0xE5]);
        assert_eq!(parse_ppm_pixel(&ppm).unwrap(), 0x2FD8E5);
    }
}
