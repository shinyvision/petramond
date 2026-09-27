use super::FontError;

pub(super) const PAD: u32 = 1;
const MIN_W: u32 = 64;
const MAX_W: u32 = 4096;
const MAX_H: u32 = 8192;

const SLACK_NUM: u64 = 5;
const SLACK_DEN: u64 = 4;
const SPARE_ROWS: u64 = 4;

#[derive(Debug)]
struct Shelf {
    y: u32,
    h: u32,
    x: u32,
}

#[derive(Debug)]
pub(super) struct Shelves {
    size: (u32, u32),
    shelves: Vec<Shelf>,
    bottom: u32,
}

impl Shelves {
    pub fn for_glyphs(glyphs: usize, cell: (u32, u32)) -> Result<Shelves, FontError> {
        let (cw, ch) = (cell.0 + PAD, cell.1 + PAD);
        let area = glyphs as u64 * u64::from(cw) * u64::from(ch) * SLACK_NUM / SLACK_DEN;
        let side = ((area as f64).sqrt().ceil() as u32).max(cw).max(MIN_W);
        let width = side.next_power_of_two().min(MAX_W);
        if cw > width {
            return Err(FontError::Parse(format!(
                "a glyph {cw} px wide does not fit the {MAX_W} px atlas"
            )));
        }
        let per_row = u64::from((width / cw).max(1));
        let rows = (glyphs as u64 * SLACK_NUM / SLACK_DEN).div_ceil(per_row) + SPARE_ROWS;
        let height = (rows * u64::from(ch)).max(1);
        if height > u64::from(MAX_H) {
            return Err(FontError::Parse(format!(
                "glyph atlas {width}x{height} exceeds {MAX_H} px; restrict the font's ranges"
            )));
        }
        Ok(Shelves {
            size: (width, height as u32),
            shelves: Vec::new(),
            bottom: 0,
        })
    }

    pub fn size(&self) -> (u32, u32) {
        self.size
    }

    pub fn place(&mut self, w: u32, h: u32) -> Option<[u32; 2]> {
        if w == 0 || h == 0 {
            return Some([0, 0]);
        }
        let (width, height) = self.size;
        if w + PAD > width {
            return None;
        }
        let fits = |s: &Shelf| s.h >= h + PAD && s.h <= h + PAD + h / 2 && s.x + w + PAD <= width;
        if let Some(shelf) = self.shelves.iter_mut().find(|s| fits(s)) {
            let at = [shelf.x, shelf.y];
            shelf.x += w + PAD;
            return Some(at);
        }
        if self.bottom + h + PAD > height {
            return None;
        }
        let shelf = Shelf {
            y: self.bottom,
            h: h + PAD,
            x: w + PAD,
        };
        self.bottom += shelf.h;
        let at = [0, shelf.y];
        self.shelves.push(shelf);
        Some(at)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placed_rects_never_overlap_and_stay_inside() {
        let sizes: Vec<(u32, u32)> = (0..300).map(|i| (3 + i % 9, 5 + i % 13)).collect();
        let mut atlas = Shelves::for_glyphs(sizes.len(), (11, 17)).unwrap();
        let (w, h) = atlas.size();
        let at: Vec<[u32; 2]> = sizes
            .iter()
            .map(|&(gw, gh)| atlas.place(gw, gh).expect("sized for every glyph"))
            .collect();
        for (i, (&[x, y], &(gw, gh))) in at.iter().zip(&sizes).enumerate() {
            assert!(x + gw <= w && y + gh <= h, "glyph {i} leaves the atlas");
            for (j, (&[ox, oy], &(ow, oh))) in at.iter().zip(&sizes).enumerate().skip(i + 1) {
                let apart = x + gw <= ox || ox + ow <= x || y + gh <= oy || oy + oh <= y;
                assert!(apart, "glyphs {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn the_size_never_changes_and_a_full_atlas_refuses() {
        let mut atlas = Shelves::for_glyphs(4, (5, 7)).unwrap();
        let size = atlas.size();
        let mut placed = 0;
        while atlas.place(5, 7).is_some() {
            placed += 1;
            assert_eq!(atlas.size(), size);
            assert!(placed < 10_000, "a fixed atlas fills up");
        }
        assert!(placed >= 4, "room for the glyphs it was sized for");
    }

    #[test]
    fn empty_glyphs_take_no_room() {
        let mut atlas = Shelves::for_glyphs(2, (4, 4)).unwrap();
        assert_eq!(atlas.place(0, 0), Some([0, 0]));
        assert_eq!(atlas.place(4, 4), Some([0, 0]));
        assert_eq!(atlas.place(4, 4), Some([5, 0]));
    }
}
