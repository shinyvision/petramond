use super::raster::segment;

/// A deck cell of a [`span`]: `level` is the standing height (the deck's top surface) in half
/// blocks, `edge` marks the outer lanes a railing stands on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct SpanCell {
    pub pos: [i32; 2],
    pub t: f32,
    pub level: f32,
    pub edge: bool,
    pub side: i8,
}

pub struct Span {
    pub cells: Vec<SpanCell>,
    /// Columns under the deck's edge lanes, every `spacing` blocks, with the deck level there.
    pub supports: Vec<([i32; 2], f32)>,
    pub length: f32,
}

/// A walkable deck from standing level `a_level` at `a` to `b_level` at `b`, rising in half
/// steps, three cells wide.
pub fn span(a: [i32; 2], a_level: f32, b: [i32; 2], b_level: f32, spacing: i32) -> Span {
    let length = (((b[0] - a[0]).pow(2) + (b[1] - a[1]).pow(2)) as f32).sqrt();
    let level = |t: f32| ((a_level + (b_level - a_level) * t) * 2.0).round() / 2.0;
    let cells = segment(a, b, 1.45)
        .into_iter()
        .map(|c| SpanCell {
            pos: c.pos,
            t: c.t,
            level: level(c.t),
            edge: c.offset.abs() > 0.55,
            side: if c.offset.abs() <= 0.55 {
                0
            } else if c.offset > 0.0 {
                1
            } else {
                -1
            },
        })
        .collect();
    let mut supports = Vec::new();
    if length > 0.0 {
        let (ux, uz) = ((b[0] - a[0]) as f32 / length, (b[1] - a[1]) as f32 / length);
        let mut k = 1;
        while ((k * spacing.max(1)) as f32) < length - 1.0 {
            let t = (k * spacing) as f32 / length;
            let (px, pz) = (
                a[0] as f32 + (b[0] - a[0]) as f32 * t,
                a[1] as f32 + (b[1] - a[1]) as f32 * t,
            );
            for side in [-1.0f32, 1.0] {
                let pos = [
                    (px - uz * side).round() as i32,
                    (pz + ux * side).round() as i32,
                ];
                supports.push((pos, level(t)));
            }
            k += 1;
        }
    }
    Span {
        cells,
        supports,
        length,
    }
}
