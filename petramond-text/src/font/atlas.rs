//! Shelf packing of per-glyph bitmaps into one atlas.
//!
//! Every glyph keeps its own tight rect, so a tall glyph from a fallback face
//! costs only its own space — it never enlarges every other glyph's cell.

use super::FontError;

/// Blank pixels between neighbouring glyphs, so filtering at a quad's edge
/// can never pick up a neighbour's ink.
const PAD: u32 = 1;
/// Narrowest atlas: small fonts stay one compact strip.
const MIN_W: u32 = 64;
/// Widest atlas row, and the tallest atlas accepted — the portable GPU
/// texture limit. A font needing more must restrict its ranges.
const MAX_W: u32 = 4096;
const MAX_H: u32 = 8192;

/// Pack `sizes` (`(w, h)` per glyph) and return each glyph's `[x, y]` plus
/// the atlas size. Zero-sized glyphs (spaces) take no room and sit at the
/// origin.
pub(super) fn pack(sizes: &[(u32, u32)]) -> Result<(Vec<[u32; 2]>, (u32, u32)), FontError> {
    let area: u64 = sizes
        .iter()
        .map(|&(w, h)| u64::from(w + PAD) * u64::from(h + PAD))
        .sum();
    let widest = sizes.iter().map(|&(w, _)| w + PAD).max().unwrap_or(0);
    let side = ((area as f64).sqrt().ceil() as u32).max(widest).max(MIN_W);
    let width = side.next_power_of_two().min(MAX_W);
    if widest > width {
        return Err(FontError::Parse(format!(
            "a glyph {widest} px wide does not fit the {MAX_W} px atlas"
        )));
    }

    // Tallest first keeps each shelf's wasted headroom small.
    let mut order: Vec<usize> = (0..sizes.len()).collect();
    order.sort_by_key(|&i| std::cmp::Reverse((sizes[i].1, sizes[i].0)));
    let mut at = vec![[0u32; 2]; sizes.len()];
    let (mut x, mut y, mut shelf_h) = (0u32, 0u32, 0u32);
    for i in order {
        let (w, h) = sizes[i];
        if w == 0 || h == 0 {
            continue;
        }
        if x + w + PAD > width {
            y += shelf_h;
            x = 0;
            shelf_h = 0;
        }
        at[i] = [x, y];
        x += w + PAD;
        shelf_h = shelf_h.max(h + PAD);
    }
    let height = (y + shelf_h).max(1);
    if height > MAX_H {
        return Err(FontError::Parse(format!(
            "glyph atlas {width}x{height} exceeds {MAX_H} px; restrict the font's ranges"
        )));
    }
    Ok((at, (width, height)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn packed_rects_never_overlap_and_stay_inside() {
        let sizes: Vec<(u32, u32)> = (0..300).map(|i| (3 + i % 9, 5 + i % 13)).collect();
        let (at, (w, h)) = pack(&sizes).unwrap();
        for (i, (&[x, y], &(gw, gh))) in at.iter().zip(&sizes).enumerate() {
            assert!(x + gw <= w && y + gh <= h, "glyph {i} leaves the atlas");
            for (j, (&[ox, oy], &(ow, oh))) in at.iter().zip(&sizes).enumerate().skip(i + 1) {
                let apart = x + gw <= ox || ox + ow <= x || y + gh <= oy || oy + oh <= y;
                assert!(apart, "glyphs {i} and {j} overlap");
            }
        }
    }

    #[test]
    fn one_tall_glyph_costs_only_its_own_shelf() {
        let mut sizes = vec![(5, 7); 200];
        let (_, (_, short)) = pack(&sizes).unwrap();
        sizes.push((5, 40));
        let (_, (_, with_tall)) = pack(&sizes).unwrap();
        assert!(with_tall <= short + 41, "{short} -> {with_tall}");
    }

    #[test]
    fn empty_glyphs_take_no_room() {
        let (at, size) = pack(&[(0, 0), (4, 4)]).unwrap();
        assert_eq!(at[0], [0, 0]);
        assert_eq!(size, (MIN_W, 5));
    }
}
