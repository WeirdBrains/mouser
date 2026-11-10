//! Views for mode plugins, drawn as `mouser-mode-api` panel items: the
//! [`Layout`] a mode shows its items in (list, grid or carousel) with the
//! geometry of each ([`grid`], [`carousel`]), the carousel's eased
//! [`anim::Slide`], image [`zoom`] and fit, a [`scroll`]bar, and the label
//! helpers in [`text`]. Commander is built on these; any mode can use them
//! (docs/building-modes.md §Views and media). Nothing here talks to Mouser:
//! a mode keeps its own state and sends the items in its panel.

pub mod anim;
pub mod carousel;
pub mod geom;
pub mod grid;
pub mod scroll;
pub mod text;
pub mod zoom;

/// How a mode lays out its items.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Hash)]
pub enum Layout {
    /// One row each (columns of text).
    #[default]
    List,
    /// Tiles in rows.
    Grid,
    /// One item large in the middle, its neighbours cut off at the edges.
    Carousel,
}

impl Layout {
    pub const ALL: [Layout; 3] = [Layout::List, Layout::Grid, Layout::Carousel];

    /// List → grid → carousel → list.
    pub fn next(self) -> Self {
        match self {
            Layout::List => Layout::Grid,
            Layout::Grid => Layout::Carousel,
            Layout::Carousel => Layout::List,
        }
    }

    pub fn name(self) -> &'static str {
        match self {
            Layout::List => "list",
            Layout::Grid => "grid",
            Layout::Carousel => "carousel",
        }
    }

    pub fn from_name(s: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|l| l.name() == s)
    }
}

#[cfg(test)]
mod tests {
    use super::Layout;

    #[test]
    fn layouts_cycle_and_round_trip_by_name() {
        assert_eq!(Layout::default(), Layout::List);
        assert_eq!(Layout::List.next().next().next(), Layout::List);
        for l in Layout::ALL {
            assert_eq!(Layout::from_name(l.name()), Some(l));
        }
        assert_eq!(Layout::from_name("table"), None);
    }
}
