//! The context menu (right-click, or `.` while commander has the keyboard):
//! every action that applies where it was opened, from one registry. Each
//! action names the contexts (what commander shows, and the item under the
//! pointer) where it applies; the menu shows the action's current key from
//! the keymap, so the menu and the keys never drift apart.

use crate::modes::Keymap;
use mouser_mode_api::MenuEntry;

/// Where the menu was opened.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Cx {
    /// A folder in the listing.
    Folder,
    /// A file that is not media.
    File,
    /// An image, video or audio file in the listing.
    Media,
    /// The listing, not on an item.
    Empty,
    /// The image view (not fullscreen).
    Image,
    /// The image view, fullscreen.
    FsImage,
    /// The player (not fullscreen).
    Player,
    /// The player, fullscreen.
    FsPlayer,
    /// The text, Markdown, PDF or document view, or the manual.
    Text,
}

use Cx::*;

/// One action in the registry.
pub struct Act {
    pub id: &'static str,
    pub arg: i32,
    pub label: &'static str,
    pub cx: &'static [Cx],
}

const fn a(id: &'static str, arg: i32, label: &'static str, cx: &'static [Cx]) -> Act {
    Act { id, arg, label, cx }
}

const ITEMS: &[Cx] = &[Folder, File, Media];
const VIEWS: &[Cx] = &[Image, FsImage, Player, FsPlayer, Text];
const IMAGE: &[Cx] = &[Image, FsImage];
const PLAYER: &[Cx] = &[Player, FsPlayer];
const LIST: &[Cx] = &[Folder, File, Media, Empty];

/// Every action the menu can show, in menu order. Actions without a key
/// binding (copy, move, rename ...) have their key shown as nothing.
pub const ACTIONS: &[Act] = &[
    // On an item.
    a("open", 0, "open", &[Folder, File]),
    a("open", 0, "play", &[Media]),
    a("open_fullscreen", 0, "play / view fullscreen", &[Media]),
    a("sync_item", 0, "sync the shell here", &[Folder]),
    a("copy", 0, "copy (paste in another folder)", ITEMS),
    a("copy_to", 0, "copy to…", ITEMS),
    a("move_to", 0, "move to…", ITEMS),
    a("rename", 0, "rename…", ITEMS),
    a("delete", 0, "delete…", ITEMS),
    a("copy_path", 0, "copy path", &[Folder, File, Media, Image, FsImage, Player, FsPlayer, Text]),
    a("mark", 0, "mark / unmark", ITEMS),
    a("pipe", 0, "pipe into a command…", ITEMS),
    // The player.
    a("play_pause", 0, "play / pause", PLAYER),
    a("seek_back", 0, "back 10 s", PLAYER),
    a("seek_forward", 0, "forward 10 s", PLAYER),
    a("volume_down", 0, "volume down", PLAYER),
    a("volume_up", 0, "volume up", PLAYER),
    // The image.
    a("zoom_in", 0, "zoom in", IMAGE),
    a("zoom_out", 0, "zoom out", IMAGE),
    a("zoom_fit", 0, "fit", IMAGE),
    a("zoom_actual", 0, "100% (actual pixels)", IMAGE),
    a("left", 0, "pan left / previous image", IMAGE),
    a("right", 0, "pan right / next image", IMAGE),
    a("up", 0, "pan up", IMAGE),
    a("down", 0, "pan down", IMAGE),
    // Text.
    a("up", 0, "scroll up", &[Text]),
    a("down", 0, "scroll down", &[Text]),
    a("page_up", 0, "page up", &[Text]),
    a("page_down", 0, "page down", &[Text]),
    a("copy_text", 0, "copy text", &[Text]),
    a("left", 0, "previous file", &[Text]),
    a("right", 0, "next file", &[Text]),
    // Any file view.
    a("fullscreen", 0, "fullscreen", &[Image, Player]),
    a("fullscreen", 0, "exit fullscreen", &[FsImage, FsPlayer]),
    a("close", 0, "close", VIEWS),
    // The folder.
    a("parent", 0, "back", LIST),
    a("sync", 0, "sync the shell to this folder", &[Empty]),
    a("new_folder", 0, "new folder…", &[Empty]),
    a("paste", 0, "paste", &[Folder, Empty]),
    a("sort", 0, "sort by name", &[Empty]),
    a("sort", 1, "sort by modified", &[Empty]),
    a("sort", 2, "sort by size", &[Empty]),
    a("reverse", 0, "reverse order", &[Empty]),
    a("switch_view", 0, "switch view", &[Empty]),
    a("bigger", 0, "larger tiles", &[Empty]),
    a("smaller", 0, "smaller tiles", &[Empty]),
    a("hidden", 0, "show / hide dotfiles", &[Empty]),
    a("search", 0, "search…", &[Empty]),
    a("mark_all", 0, "mark all / clear marks", &[Empty]),
    a("first", 0, "first item", &[Empty]),
    a("last", 0, "last item", &[Empty]),
    a("options", 0, "options", &[Empty]),
    a("man", 0, "manual", &[Empty]),
];

/// Bindings that are never menu entries: the header's own overflow menu,
/// and `back`, the second key of `parent` (the menu shows that one).
pub const NOT_IN_MENUS: &[&str] = &["menu", "back"];

/// Whether an action can run now (else its entry is greyed out).
pub type Enabled<'a> = &'a dyn Fn(&str) -> bool;

/// The menu for `cx`: label, the action's key now, and whether it can run.
/// `item`: the listing index the menu is for (the entries' argument for
/// item actions).
pub fn entries(cx: Cx, item: Option<usize>, keys: &Keymap, enabled: Enabled<'_>) -> Vec<MenuEntry> {
    ACTIONS
        .iter()
        .filter(|x| x.cx.contains(&cx))
        .map(|x| {
            let key = keys.keys.iter().find(|k| k.0 == x.id).map(|k| k.1.clone()).unwrap_or_default();
            // Sort is the one action whose argument is its own; items carry
            // their index (the menu picks the item it was opened on).
            let arg = if x.id == "sort" { x.arg } else { item.map_or(x.arg, |i| i as i32) };
            MenuEntry {
                id: format!("menu:{}", x.id),
                arg,
                label: x.label.into(),
                key: if x.id == "sort" { String::new() } else { key },
                enabled: enabled(x.id),
            }
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys() -> Keymap {
        Keymap::new(crate::commander::BINDINGS)
    }

    fn ids(cx: Cx) -> Vec<(String, String)> {
        entries(cx, Some(0), &keys(), &|_| true).into_iter().map(|e| (e.id.trim_start_matches("menu:").to_string(), e.key)).collect()
    }

    fn has(cx: Cx, want: &[(&str, &str)]) {
        let got = ids(cx);
        for (id, key) in want {
            assert!(got.iter().any(|(i, k)| i == id && k == key), "{cx:?}: {id} ({key}) missing from {got:?}");
        }
    }

    #[test]
    fn fullscreen_image() {
        has(
            FsImage,
            &[
                ("close", "q"),
                ("zoom_in", "="),
                ("zoom_out", "-"),
                ("zoom_fit", "0"),
                ("zoom_actual", "1"),
                ("left", "left"),
                ("up", "up"),
                ("fullscreen", "f"),
            ],
        );
        assert!(!ids(FsImage).iter().any(|(i, _)| i == "play_pause" || i == "delete"));
    }

    #[test]
    fn playing_media_fullscreen_and_not() {
        for cx in [FsPlayer, Player] {
            has(
                cx,
                &[
                    ("play_pause", "k"),
                    ("seek_back", "j"),
                    ("seek_forward", "l"),
                    ("volume_down", "-"),
                    ("volume_up", "="),
                    ("fullscreen", "f"),
                    ("close", "q"),
                ],
            );
            assert!(!ids(cx).iter().any(|(i, _)| i == "zoom_in"));
        }
        let fs = entries(FsPlayer, None, &keys(), &|_| true);
        assert!(fs.iter().any(|e| e.label == "exit fullscreen"));
    }

    #[test]
    fn text_view() {
        has(
            Text,
            &[("up", "up"), ("down", "down"), ("page_up", "pageup"), ("page_down", "pagedown"), ("close", "q"), ("copy_text", ""), ("copy_path", "")],
        );
    }

    #[test]
    fn list_file_media_and_folder() {
        let item = [
            ("open", "enter"),
            ("copy", ""),
            ("copy_to", ""),
            ("move_to", ""),
            ("rename", ""),
            ("delete", ""),
            ("copy_path", ""),
            ("mark", "space"),
            ("pipe", "|"),
        ];
        has(File, &item);
        has(Media, &item);
        has(Media, &[("open_fullscreen", "")]);
        assert!(entries(Media, Some(3), &keys(), &|_| true).iter().any(|e| e.label == "play" && e.arg == 3), "media: play, on that item");
        has(Folder, &[("open", "enter"), ("parent", "u"), ("sync_item", ""), ("delete", "")]);
        assert!(!ids(File).iter().any(|(i, _)| i == "play_pause" || i == "new_folder"));
    }

    #[test]
    fn empty_area() {
        has(Empty, &[("new_folder", ""), ("paste", ""), ("sort", ""), ("switch_view", "v"), ("sync", "s"), ("hidden", "h"), ("search", "/")]);
        let sorts = entries(Empty, None, &keys(), &|_| true).into_iter().filter(|e| e.id == "menu:sort").map(|e| e.arg).collect::<Vec<_>>();
        assert_eq!(sorts, [0, 1, 2]);
        assert!(!ids(Empty).iter().any(|(i, _)| i == "delete" || i == "open"));
    }

    #[test]
    fn rebinding_shows_in_the_menu() {
        let mut k = keys();
        k.apply("commander", "[commander.keys]\nplay_pause = \"p\"\n").unwrap();
        let e = entries(Player, None, &k, &|_| true);
        assert_eq!(e.iter().find(|e| e.id == "menu:play_pause").unwrap().key, "p");
    }

    #[test]
    fn disabled_entries_are_greyed() {
        let e = entries(File, Some(0), &keys(), &|id| id != "delete");
        assert!(!e.iter().find(|e| e.id == "menu:delete").unwrap().enabled);
    }

    /// Every binding is in some menu (except those listed), and every menu
    /// id is a binding or one of the menu-only actions the mode handles.
    #[test]
    fn registry_covers_every_binding() {
        for b in crate::commander::BINDINGS {
            assert!(NOT_IN_MENUS.contains(&b.id) || ACTIONS.iter().any(|a| a.id == b.id), "{} is in no menu", b.id);
        }
        let menu_only = [
            "open_fullscreen",
            "sync_item",
            "copy",
            "copy_to",
            "move_to",
            "rename",
            "delete",
            "copy_path",
            "copy_text",
            "new_folder",
            "paste",
            "sort",
            "reverse",
        ];
        for a in ACTIONS {
            assert!(crate::commander::BINDINGS.iter().any(|b| b.id == a.id) || menu_only.contains(&a.id), "{} is unknown", a.id);
        }
    }
}
