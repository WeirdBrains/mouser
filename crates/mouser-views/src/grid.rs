//! Grid geometry: square tiles with a label under each, in rows that
//! scroll to keep the selection on screen.

use mouser_mode_api::ui::Rect;

/// How the tiles look: side, gap, label height under each, and px kept free
/// at the right (for a scrollbar or overhanging badges).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Tiles {
    pub size: i32,
    pub gap: i32,
    pub label_h: i32,
    pub reserve: i32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Grid {
    pub body: Rect,
    /// Tile side, px.
    pub tile: i32,
    pub gap: i32,
    /// Height of a tile plus its label and the gap.
    pub cell_h: i32,
    pub per_row: usize,
    pub vis_rows: usize,
    /// First row shown.
    pub first: usize,
    pub total_rows: usize,
}

impl Grid {
    /// `len` items in `body`. `scroll` is the first row shown last frame;
    /// the result keeps the `selected` item's row in view.
    pub fn new(body: Rect, t: Tiles, len: usize, selected: usize, scroll: usize) -> Self {
        let Tiles { size: tile, gap, label_h, reserve } = t;
        let per_row = ((body.w - reserve + gap) / (tile + gap).max(1)).max(1) as usize;
        let cell_h = tile + label_h + gap;
        let vis_rows = ((body.h + gap) / cell_h.max(1)).max(1) as usize;
        let row = selected / per_row;
        let mut first = scroll.min(row);
        if row >= first + vis_rows {
            first = row + 1 - vis_rows;
        }
        Grid { body, tile, gap, cell_h, per_row, vis_rows, first, total_rows: len.div_ceil(per_row) }
    }

    /// Items a page holds.
    pub fn page(&self) -> usize {
        self.per_row * self.vis_rows
    }

    /// Index of the first item shown.
    pub fn start(&self) -> usize {
        self.first * self.per_row
    }

    /// Item `n`'s tile (n counts from 0 over all items; it may be off screen).
    pub fn tile_rect(&self, n: usize) -> Rect {
        let i = n.saturating_sub(self.start());
        let (r, c) = (i / self.per_row, i % self.per_row);
        Rect { x: self.body.x + c as i32 * (self.tile + self.gap), y: self.body.y + r as i32 * self.cell_h, w: self.tile, h: self.tile }
    }

    /// The items shown (a range of indices into `len` items).
    pub fn shown(&self, len: usize) -> std::ops::Range<usize> {
        self.start().min(len)..(self.start() + self.page()).min(len)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const T: Tiles = Tiles { size: 100, gap: 10, label_h: 20, reserve: 0 };

    #[test]
    fn rows_scroll_to_the_selection() {
        let body = Rect { x: 10, y: 20, w: 330, h: 250 };
        // 100px tiles, 10 gap: 3 per row; cell 100+20+10 = 130: 2 rows.
        let g = Grid::new(body, T, 20, 0, 0);
        assert_eq!((g.per_row, g.vis_rows, g.total_rows, g.page()), (3, 2, 7, 6));
        assert_eq!(g.tile_rect(4), Rect { x: 120, y: 150, w: 100, h: 100 });
        let g = Grid::new(body, T, 20, 10, 0);
        assert_eq!(g.first, 2, "row 3 (item 10) is the last shown");
        assert_eq!(g.shown(20), 6..12);
        assert_eq!(g.tile_rect(6).y, 20);
        let g = Grid::new(body, T, 20, 0, 5);
        assert_eq!(g.first, 0, "scrolled back up to the selection");
        assert_eq!(Grid::new(Rect { w: 50, ..body }, T, 3, 0, 0).per_row, 1, "always one column");
    }
}
