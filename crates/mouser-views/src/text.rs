//! Labels: clipped names, `x–y of N`, header buttons that fit, and a path
//! split into clickable segments.

/// `s` in at most `cols` characters, ending in `…` when cut.
pub fn clip_text(s: &str, cols: i32) -> String {
    let cols = cols.max(1) as usize;
    if s.chars().count() <= cols {
        s.to_string()
    } else {
        let mut t: String = s.chars().take(cols.saturating_sub(1)).collect();
        t.push('…');
        t
    }
}

/// `x–y of N` for a view showing `shown` items from `first` of `total`.
pub fn range_label(first: usize, shown: usize, total: usize) -> String {
    if total == 0 {
        return "0 of 0".into();
    }
    format!("{}–{} of {total}", (first + 1).min(total), (first + shown).min(total))
}

/// How many header buttons (widths, right to left) fit in `room` px; when
/// not all do, a ••• menu `menu_w` wide takes the rest.
pub fn header_fit(widths: &[i32], room: i32, menu_w: i32) -> usize {
    let all: i32 = widths.iter().sum();
    if all <= room {
        return widths.len();
    }
    let mut used = menu_w;
    widths
        .iter()
        .take_while(|w| {
            used += **w;
            used <= room
        })
        .count()
}

/// A shown path as segments, each with how many levels up it is
/// (`/home/me` → `/` 2, `home` 1, `/me` 0).
pub fn path_segments(shown: &str) -> Vec<(String, usize)> {
    let parts: Vec<&str> = shown.split('/').collect();
    let mut out: Vec<String> = Vec::new();
    for (i, p) in parts.iter().enumerate() {
        match (i, p.is_empty()) {
            (0, true) => out.push("/".into()),
            (_, true) => {}
            (0, false) => out.push(p.to_string()),
            _ if out.last().is_some_and(|l| l == "/") && out.len() == 1 => out.push(p.to_string()),
            _ => out.push(format!("/{p}")),
        }
    }
    let n = out.len();
    out.into_iter().enumerate().map(|(i, t)| (t, n - 1 - i)).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels() {
        assert_eq!(clip_text("abcdef", 4), "abc…");
        assert_eq!(clip_text("abc", 4), "abc");
        assert_eq!(range_label(0, 10, 25), "1–10 of 25");
        assert_eq!(range_label(20, 10, 25), "21–25 of 25");
        assert_eq!(range_label(0, 0, 0), "0 of 0");
        assert_eq!(header_fit(&[10, 10, 10], 30, 5), 3);
        assert_eq!(header_fit(&[10, 10, 10], 29, 5), 2);
        let s: Vec<(String, usize)> = path_segments("/home/me");
        assert_eq!(s, [("/".to_string(), 2), ("home".to_string(), 1), ("/me".to_string(), 0)]);
        assert_eq!(path_segments("~/src"), [("~".to_string(), 1), ("/src".to_string(), 0)]);
    }
}
