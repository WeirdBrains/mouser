//! Views and media the host runs for a mode (mode-api 0.4, docs/mode-protocol.md
//! §Host views and media). A mode registers them by id with
//! [`crate::Host::hosted`] ([`crate::protocol::Out::Hosted`]): where each one
//! goes and what it shows. Mouser then lays it out and draws it, loads the
//! thumbnails, plays the media (pictures, video and sound, inline and
//! fullscreen), takes its keys and clicks and redraws it as it animates or
//! plays. The mode hears what happened as events:
//!
//! - `view.selected` `{id, index}`: the selection moved (keys, a click);
//! - `view.activated` `{id, index}`: Enter or a double-click on an item;
//! - `view.layout` `{id, layout}`: the layout key (`v`) switched it;
//! - `media.state` `{id, playing, position, duration, fullscreen, error}`:
//!   a player started, paused, was sought, opened or closed fullscreen;
//! - `media.ended` `{id}`: playback reached the end.
//!
//! Only send them when the host offers them: [`crate::protocol::Accepted::has`]
//! with [`FEATURE_VIEWS`] / [`FEATURE_MEDIA`]. An older Mouser offers neither
//! (and couldn't read the message), so draw the panel yourself there, e.g.
//! with the `mouser-views` and `mouser-media` crates.
//!
//! Paths are on this computer and need `files.read`; without it Mouser
//! draws the item's label and says so instead of reading the file.

use crate::ui::Rect;

/// [`crate::protocol::Accepted::features`]: the host runs [`Hosted::Items`].
pub const FEATURE_VIEWS: &str = "views.1";
/// [`crate::protocol::Accepted::features`]: the host runs [`Hosted::Media`].
pub const FEATURE_MEDIA: &str = "media.1";

/// One view or player the host runs for the mode.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct HostedView {
    /// The mode's own name for it; events carry it back. Keep it stable
    /// across views: selection, scroll and a playing video follow the id.
    pub id: String,
    /// Where it goes (window pixels, inside the mode's dock or panel).
    pub rect: Rect,
    /// Over the dock (`false`) or the floating panel (`true`).
    #[serde(default)]
    pub in_panel: bool,
    pub content: Hosted,
    /// Keep it registered without drawing it: an item view keeps its
    /// selection and scroll, a player keeps playing (a clip scrolled out of
    /// a transcript). It takes no keys while off screen.
    #[serde(default)]
    pub offscreen: bool,
}

/// What a hosted view shows.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub enum Hosted {
    /// Items in a list, grid or carousel. Arrow keys, Home / End, Page Up /
    /// Down move the selection while `focused`, Enter activates it and `v`
    /// cycles the layout; clicks select, a second click activates.
    Items {
        layout: ViewLayout,
        items: Vec<HostedItem>,
        /// Move the selection here (applied when it changes from the last
        /// one sent, so the user's own moves stick); `None`: keep it.
        select: Option<u32>,
        /// The view takes the navigation keys (one focused view at a time;
        /// the last one wins).
        focused: bool,
        /// Grid tile side and carousel item height, px (`0`: Mouser picks).
        tile: i32,
    },
    /// A picture, video or sound file: a thumbnail, or a player with play /
    /// pause, progress and fullscreen (⤢ or a click). Fullscreen: space plays /
    /// pauses, ← / → seek 10 s, `f` / Enter / Esc close.
    Media {
        /// Absolute path on this computer.
        path: String,
        /// Start playing as soon as it is shown (video and sound).
        autoplay: bool,
    },
}

/// How [`Hosted::Items`] lays out its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ViewLayout {
    #[default]
    List,
    Grid,
    Carousel,
}

impl ViewLayout {
    pub fn name(self) -> &'static str {
        match self {
            ViewLayout::List => "list",
            ViewLayout::Grid => "grid",
            ViewLayout::Carousel => "carousel",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        [ViewLayout::List, ViewLayout::Grid, ViewLayout::Carousel].into_iter().find(|l| l.name() == s)
    }

    /// List → grid → carousel → list (the `v` key).
    pub fn next(self) -> Self {
        match self {
            ViewLayout::List => ViewLayout::Grid,
            ViewLayout::Grid => ViewLayout::Carousel,
            ViewLayout::Carousel => ViewLayout::List,
        }
    }
}

/// One item of [`Hosted::Items`].
#[derive(Debug, Clone, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub struct HostedItem {
    pub label: String,
    /// A second column in the list (size, date ...).
    #[serde(default)]
    pub detail: String,
    /// A picture file to show as its thumbnail (grid and carousel).
    #[serde(default)]
    pub image: Option<String>,
    #[serde(default)]
    pub icon: ItemIcon,
}

/// What a tile shows without a thumbnail.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize, serde::Deserialize)]
pub enum ItemIcon {
    #[default]
    None,
    Folder,
    /// A type badge (`PanelStyle::Badge` index).
    Badge(u8),
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn layouts_cycle_and_round_trip() {
        let mut l = ViewLayout::List;
        for want in ["grid", "carousel", "list"] {
            l = l.next();
            assert_eq!(l.name(), want);
            assert_eq!(ViewLayout::from_name(want), Some(l));
        }
        assert_eq!(ViewLayout::from_name("tree"), None);
    }
}
