//! A flag's design: one colour per texel of the cloth, and its form on a stack or a placed flag,
//! the engine's `petramond:paint` patch of per-texel tints.

pub(crate) const PAINT_KEY: &str = "petramond:paint";
pub(crate) const TINT_KEY: &str = "petramond:tint";

pub(crate) type Rgb = [u8; 3];

pub(crate) const WHITE: Rgb = [255; 3];

/// Tiles are this many texels on a side.
const TILE: u8 = 16;

/// The texel rect of a flag's tile that its cloth shows, and so the rect a design covers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct Canvas {
    pub x: u8,
    pub y: u8,
    pub w: u8,
    pub h: u8,
}

impl Canvas {
    /// `[x, y, w, h]`; `None` for an empty rect or one that leaves the tile.
    pub fn new([x, y, w, h]: [u8; 4]) -> Option<Canvas> {
        let fits = |at: u8, len: u8| len >= 1 && at.checked_add(len).is_some_and(|end| end <= TILE);
        (fits(x, w) && fits(y, h)).then_some(Canvas { x, y, w, h })
    }

    pub fn size(self) -> [u8; 2] {
        [self.w, self.h]
    }

    fn texels(self) -> usize {
        usize::from(self.w) * usize::from(self.h)
    }

    fn header(self) -> [u8; 4] {
        [self.x, self.y, self.w, self.h]
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Design {
    canvas: Canvas,
    /// Row-major, top row first.
    texels: Vec<Rgb>,
}

impl Design {
    pub fn filled(canvas: Canvas, color: Rgb) -> Design {
        Design {
            canvas,
            texels: vec![color; canvas.texels()],
        }
    }

    /// The design a paint patch holds; `None` unless the patch covers exactly `canvas`.
    pub fn from_paint(canvas: Canvas, paint: &[u8]) -> Option<Design> {
        let (header, rgb) = paint.split_at_checked(4)?;
        if header != canvas.header() || rgb.len() != canvas.texels() * 3 {
            return None;
        }
        Some(Design {
            canvas,
            texels: rgb.chunks_exact(3).map(|c| [c[0], c[1], c[2]]).collect(),
        })
    }

    pub fn to_paint(&self) -> Vec<u8> {
        let mut paint = Vec::with_capacity(4 + self.texels.len() * 3);
        paint.extend(self.canvas.header());
        paint.extend(self.texels.iter().flatten());
        paint
    }

    /// The design a tile's art already shows: its texels from `at`, `rgba` being the whole tile's
    /// pixels. A see-through texel reads as bare cloth. `None` if the rect leaves the tile.
    pub fn from_tile(canvas: Canvas, at: [u8; 2], rgba: &[u8]) -> Option<Design> {
        let source = Canvas::new([at[0], at[1], canvas.w, canvas.h])?;
        let tile = usize::from(TILE);
        if rgba.len() != tile * tile * 4 {
            return None;
        }
        let texels = (0..canvas.h)
            .flat_map(|y| (0..canvas.w).map(move |x| (x, y)))
            .map(|(x, y)| {
                let i = (usize::from(source.y + y) * tile + usize::from(source.x + x)) * 4;
                if rgba[i + 3] < 128 {
                    WHITE
                } else {
                    [rgba[i], rgba[i + 1], rgba[i + 2]]
                }
            })
            .collect();
        Some(Design { canvas, texels })
    }

    pub fn size(&self) -> [u8; 2] {
        self.canvas.size()
    }

    fn index(&self, [x, y]: [i32; 2]) -> Option<usize> {
        let inside = |v: i32, len: u8| (0..i32::from(len)).contains(&v);
        (inside(x, self.canvas.w) && inside(y, self.canvas.h))
            .then(|| y as usize * usize::from(self.canvas.w) + x as usize)
    }

    /// The colour at a texel of the canvas, `[0, 0]` its top left.
    pub fn get(&self, at: [i32; 2]) -> Option<Rgb> {
        self.index(at).map(|i| self.texels[i])
    }

    /// Colours one texel; a spot off the canvas is ignored.
    pub fn set(&mut self, at: [i32; 2], color: Rgb) {
        if let Some(i) = self.index(at) {
            self.texels[i] = color;
        }
    }

    /// Colours every texel on the straight run between two spots, both included, so a fast drag
    /// leaves no gaps. Either end may lie off the canvas.
    pub fn line(&mut self, from: [i32; 2], to: [i32; 2], color: Rgb) {
        let (dx, dy) = ((to[0] - from[0]).abs(), (to[1] - from[1]).abs());
        let (sx, sy) = ((to[0] - from[0]).signum(), (to[1] - from[1]).signum());
        let mut at = from;
        let mut err = dx - dy;
        loop {
            self.set(at, color);
            if at == to {
                break;
            }
            let twice = 2 * err;
            if twice > -dy {
                err -= dy;
                at[0] += sx;
            }
            if twice < dx {
                err += dx;
                at[1] += sy;
            }
        }
    }

    /// Recolours the patch of one colour that `at` belongs to, through edge neighbours.
    pub fn fill(&mut self, at: [i32; 2], color: Rgb) {
        let Some(old) = self.get(at) else {
            return;
        };
        if old == color {
            return;
        }
        let mut open = vec![at];
        while let Some(at) = open.pop() {
            if self.get(at) != Some(old) {
                continue;
            }
            self.set(at, color);
            open.extend(
                [[1, 0], [-1, 0], [0, 1], [0, -1]].map(|[dx, dy]| [at[0] + dx, at[1] + dy]),
            );
        }
    }

    pub fn rows(&self) -> impl Iterator<Item = &[Rgb]> {
        self.texels.chunks_exact(usize::from(self.canvas.w))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn canvas() -> Canvas {
        Canvas::new([0, 2, 16, 12]).expect("a rect inside the tile")
    }

    #[test]
    fn a_design_survives_the_trip_through_its_paint() {
        let mut design = Design::filled(canvas(), WHITE);
        design.set([3, 4], [200, 10, 30]);
        design.set([15, 11], [1, 2, 3]);
        let paint = design.to_paint();
        assert_eq!(Design::from_paint(canvas(), &paint), Some(design));
    }

    #[test]
    fn paint_for_another_rect_or_of_the_wrong_length_is_no_design() {
        let paint = Design::filled(canvas(), WHITE).to_paint();
        let other = Canvas::new([0, 0, 16, 12]).expect("a rect inside the tile");
        assert_eq!(Design::from_paint(other, &paint), None);
        assert_eq!(
            Design::from_paint(canvas(), &paint[..paint.len() - 1]),
            None
        );
        assert_eq!(Design::from_paint(canvas(), &[]), None);
    }

    #[test]
    fn a_rect_that_leaves_the_tile_is_no_canvas() {
        assert!(Canvas::new([0, 5, 16, 12]).is_none());
        assert!(Canvas::new([250, 0, 16, 12]).is_none());
        assert!(Canvas::new([0, 0, 0, 12]).is_none());
    }

    #[test]
    fn a_fast_drag_leaves_an_unbroken_line_even_from_off_the_canvas() {
        let ink = [9, 9, 9];
        let mut design = Design::filled(canvas(), WHITE);
        design.line([-3, 1], [9, 7], ink);
        let inked: Vec<[i32; 2]> = (0..12)
            .flat_map(|y| (0..16).map(move |x| [x, y]))
            .filter(|&at| design.get(at) == Some(ink))
            .collect();
        assert!(
            inked.iter().any(|at| at[0] == 0),
            "it enters where the drag crosses the edge"
        );
        assert_eq!(inked.last(), Some(&[9, 7]));
        for pair in inked.windows(2) {
            let step = [pair[1][0] - pair[0][0], pair[1][1] - pair[0][1]];
            assert!(step[0].abs() <= 1 && step[1].abs() <= 1, "gap at {pair:?}");
        }
    }

    #[test]
    fn a_fill_stops_at_other_colours_and_does_not_leak_through_corners() {
        let (ink, wall) = ([1, 1, 1], [7, 7, 7]);
        let mut design = Design::filled(canvas(), WHITE);
        design.line([0, 3], [3, 0], wall);
        design.fill([0, 0], ink);
        assert_eq!(design.get([1, 1]), Some(ink));
        assert_eq!(design.get([2, 1]), Some(wall));
        assert_eq!(design.get([2, 2]), Some(WHITE), "the diagonal wall holds");
    }

    #[test]
    fn a_tile_seeds_a_design_from_its_own_rect_with_bare_cloth_where_it_is_clear() {
        let mut rgba = vec![0u8; 16 * 16 * 4];
        let put = |rgba: &mut [u8], x: usize, y: usize, px: [u8; 4]| {
            rgba[(y * 16 + x) * 4..][..4].copy_from_slice(&px);
        };
        put(&mut rgba, 0, 0, [10, 20, 30, 255]);
        put(&mut rgba, 15, 11, [40, 50, 60, 255]);
        put(&mut rgba, 5, 5, [99, 99, 99, 0]);
        let design = Design::from_tile(canvas(), [0, 0], &rgba).expect("the rect fits");
        assert_eq!(design.get([0, 0]), Some([10, 20, 30]));
        assert_eq!(design.get([15, 11]), Some([40, 50, 60]));
        assert_eq!(design.get([5, 5]), Some(WHITE));
        assert!(Design::from_tile(canvas(), [0, 6], &rgba).is_none());
    }
}
