//! A slim scrollbar item.

use mouser_mode_api::ui::{PanelItem, PanelStyle, Rect};

/// A scrollbar over `track` when `shown` of `total` don't all fit (first
/// shown is `first`); None when everything fits. `none` is the item's
/// action (it isn't clickable).
pub fn scrollbar<A>(track: Rect, first: usize, shown: usize, total: usize, none: A) -> Option<PanelItem<A>> {
    if total <= shown || total == 0 || track.w <= 0 || track.h <= 0 {
        return None;
    }
    let start = (first * 1000 / total) as u16;
    let len = (shown * 1000 / total).max(1) as u16;
    Some(PanelItem { rect: track, text: String::new(), style: PanelStyle::Scrollbar { start, len }, action: none })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_when_something_is_hidden() {
        let t = Rect { x: 0, y: 0, w: 5, h: 100 };
        assert!(scrollbar(t, 0, 10, 10, ()).is_none());
        let s = scrollbar(t, 25, 25, 100, ()).unwrap();
        assert_eq!(s.style, PanelStyle::Scrollbar { start: 250, len: 250 });
    }
}
