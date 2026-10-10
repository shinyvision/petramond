//! `petramond:paint`: a rectangular patch of per-texel tints in the 16×16 texel space of
//! the tile that wears it, and the [`Coat`] a wearer's tint and paint make together.
//!
//! Bytes: `[x, y, w, h]` then `w * h` RGB triples, row-major, top row first. `(x, y)` is
//! the patch's top-left texel with `y` counted down from the tile's top row, and the
//! patch must lie inside the tile. Anything else is not a paint and is ignored.
//!
//! A painted texel shows its paint multiplied over the tile's dye-base twin; any other
//! texel shows the wearer's `petramond:tint` the same way, or the ordinary texture when
//! there is none. Sprite items (held, dropped, and as icons) and cloth sheets wear paint.
//! Chunk-meshed blocks and model items do not: they only wear the tint.

use crate::block::TINT_KV_KEY;

pub const PAINT_KEY: &str = "petramond:paint";

/// Texels along one side of the tile space a paint addresses.
pub const GRID: u8 = 16;

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Paint<'a> {
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
    rgb: &'a [u8],
}

/// `len` texels of one paint row sharing a colour, starting at tile texel `(x, y)`.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct PaintRun {
    pub x: u8,
    pub y: u8,
    pub len: u8,
    pub rgb: [f32; 3],
}

impl<'a> Paint<'a> {
    pub fn parse(bytes: &'a [u8]) -> Option<Self> {
        let (&[x, y, w, h], rgb) = bytes.split_first_chunk()?;
        let fits = |at: u8, len: u8| len >= 1 && u16::from(at) + u16::from(len) <= u16::from(GRID);
        (fits(x, w) && fits(y, h) && rgb.len() == usize::from(w) * usize::from(h) * 3)
            .then_some(Self { x, y, w, h, rgb })
    }

    /// The paint on tile texel `(tx, ty)`, `None` outside the patch.
    pub fn texel(&self, tx: u8, ty: u8) -> Option<[u8; 3]> {
        let (col, row) = (tx.checked_sub(self.x)?, ty.checked_sub(self.y)?);
        (col < self.w && row < self.h).then(|| self.at(col, row))
    }

    /// Every painted texel exactly once, as maximal same-colour runs within each row,
    /// top row first.
    pub fn runs(self) -> impl Iterator<Item = PaintRun> + 'a {
        (0..self.h).flat_map(move |row| {
            let mut col = 0;
            std::iter::from_fn(move || {
                let start = col;
                let rgb = (start < self.w).then(|| self.at(start, row))?;
                while col < self.w && self.at(col, row) == rgb {
                    col += 1;
                }
                Some(PaintRun {
                    x: self.x + start,
                    y: self.y + row,
                    len: col - start,
                    rgb: unit(rgb),
                })
            })
        })
    }

    fn at(&self, col: u8, row: u8) -> [u8; 3] {
        let i = (usize::from(row) * usize::from(self.w) + usize::from(col)) * 3;
        [self.rgb[i], self.rgb[i + 1], self.rgb[i + 2]]
    }
}

/// What a wearer's data does to its tile: an optional whole-tile tint with an optional
/// paint patch over it. Borrows the wearer's bytes.
#[derive(Copy, Clone, Debug, Default, PartialEq)]
pub struct Coat<'a> {
    pub tint: Option<[f32; 3]>,
    pub paint: Option<Paint<'a>>,
}

impl<'a> Coat<'a> {
    /// Read the coat out of any keyed store: `get` answers a data key with its bytes.
    pub fn read(get: impl Fn(&str) -> Option<&'a [u8]>) -> Self {
        Self {
            tint: get(TINT_KV_KEY)
                .and_then(|bytes| <[u8; 3]>::try_from(bytes).ok())
                .map(unit),
            paint: get(PAINT_KEY).and_then(Paint::parse),
        }
    }

    pub fn of_stack_data(data: &'a crate::item::variant::VariantMap) -> Self {
        Self::read(|key| data.get(key).map(Vec::as_slice))
    }

    /// The multiply tile texel `(tx, ty)` wears over its dye-base twin: its paint, else
    /// the tint. `None` = the texel shows the ordinary texture.
    pub fn texel(&self, tx: u8, ty: u8) -> Option<[f32; 3]> {
        self.paint
            .and_then(|paint| paint.texel(tx, ty))
            .map(unit)
            .or(self.tint)
    }
}

fn unit([r, g, b]: [u8; 3]) -> [f32; 3] {
    [r, g, b].map(|c| f32::from(c) / 255.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn patch(x: u8, y: u8, w: u8, h: u8, texels: &[[u8; 3]]) -> Vec<u8> {
        let mut bytes = vec![x, y, w, h];
        bytes.extend(texels.iter().flatten());
        bytes
    }

    #[test]
    fn parse_rejects_patches_that_leave_the_tile_or_miscount_their_texels() {
        let red = [255, 0, 0];
        assert!(Paint::parse(&patch(15, 15, 1, 1, &[red])).is_some());
        assert!(Paint::parse(&patch(0, 0, 16, 16, &[red; 256])).is_some());
        assert!(Paint::parse(&patch(15, 0, 2, 1, &[red; 2])).is_none());
        assert!(Paint::parse(&patch(0, 16, 1, 1, &[red])).is_none());
        assert!(Paint::parse(&patch(0, 0, 0, 1, &[])).is_none());
        assert!(Paint::parse(&patch(0, 0, 2, 2, &[red; 3])).is_none());
        assert!(Paint::parse(&patch(0, 0, 1, 1, &[red; 2])).is_none());
        assert!(Paint::parse(&[0, 0, 1]).is_none());
    }

    #[test]
    fn coat_shows_paint_over_tint_and_tint_elsewhere() {
        let paint = patch(2, 3, 2, 1, &[[255, 0, 0], [0, 255, 0]]);
        let tint = [0u8, 0, 255];
        let both = Coat::read(|key| match key {
            PAINT_KEY => Some(&paint[..]),
            TINT_KV_KEY => Some(&tint[..]),
            _ => None,
        });
        assert_eq!(both.texel(3, 3), Some([0.0, 1.0, 0.0]));
        assert_eq!(both.texel(4, 3), Some([0.0, 0.0, 1.0]));
        assert_eq!(both.texel(2, 2), Some([0.0, 0.0, 1.0]));

        let only_paint = Coat::read(|key| (key == PAINT_KEY).then_some(&paint[..]));
        assert_eq!(only_paint.texel(2, 3), Some([1.0, 0.0, 0.0]));
        assert_eq!(only_paint.texel(1, 3), None);

        let malformed = Coat::read(|key| (key == PAINT_KEY).then_some(&paint[..5]));
        assert_eq!(malformed, Coat::default());
    }

    #[test]
    fn runs_cover_each_row_and_split_where_the_colour_changes() {
        let (a, b) = ([10, 20, 30], [40, 50, 60]);
        let bytes = patch(5, 6, 3, 2, &[a, a, b, b, b, b]);
        let runs: Vec<(u8, u8, u8)> = Paint::parse(&bytes)
            .unwrap()
            .runs()
            .map(|run| (run.x, run.y, run.len))
            .collect();
        assert_eq!(runs, [(5, 6, 2), (7, 6, 1), (5, 7, 3)]);
    }
}
