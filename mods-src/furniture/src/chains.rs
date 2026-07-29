//! Chains: three single-cell rows — `furniture:chain` (vertical, the
//! item-linked base), `furniture:chain_ns`, `furniture:chain_ew` — sharing
//! ONE custom shape (`shapes.json` + the bakes in `lib.rs`); the axis
//! is block IDENTITY (the ladder-row pattern), so the bake orients each cell
//! from its block id alone and placement needs no per-cell state. The
//! placement plan picks the sibling row from the clicked face's normal and
//! returns it as the plan's block override. Placement is fully
//! deterministic, so the ENGINE predicts it whole: the client runs the same
//! plan + gates against its replica and ghosts the exact write — the mod
//! ships no placement predictor of its own.

use mod_sdk::*;

/// The chain family: the shared shape-kind id and its three axis rows —
/// vertical (the item-linked base), north/south, east/west.
pub(super) struct Chains {
    pub(super) shape: u8,
    pub(super) rows: [BlockId; 3],
}

/// The chain is real geometry: a run of flat RINGS, each with a genuine hole,
/// every ring turned 90 degrees from the one before, authored for the VERTICAL
/// row and mapped onto the other two axes by [`to_axis`].
///
/// Rings, not crossed planes (2026-07-29, per Rachel: *"make the chain properly
/// 3D such that no weird culling occurs"*). The old shape was two flat plates
/// with the links alpha-CUT out of the tile, and it failed the way cutout
/// planes always fail off-axis: you saw one plate's silhouette through the
/// other's holes, the two cutouts never lined up, and the whole thing read as
/// flat black cardboard. Nothing about the art could fix that — the geometry
/// was two quads. The tile carries only SHADING now (`gen_chain.py`).
///
/// One link is [`LINK_W`] wide x `LINK_PITCH + 1` tall x [`LINK_T`] thick, with
/// a real 1x3 hole; links sit every [`LINK_PITCH`] texels, so consecutive links
/// SHARE their bar row — the upper ring's bottom bar crosses the lower ring's
/// top bar, which is what reads as interlock. `LINK_PITCH` must divide 16 with
/// an even link count per cell or the alternating pattern breaks at the cell
/// boundary: 8 was the first cut (two chunky links), 4 is Rachel's "smaller,
/// vertically" (four links of 5).
const LINK_PITCH: f32 = 4.0;
const LINK_W: [f32; 2] = [6.5, 9.5];
const LINK_T: [f32; 2] = [7.0, 9.0];

/// One link, its bottom bar's floor at texel `ya`, ring plane in x/y
/// (`turned = false`) or z/y.
fn ring(ya: f32, turned: bool) -> [ShapeAabb; 4] {
    let top = ya + LINK_PITCH;
    let (w, t) = (LINK_W, LINK_T);
    let raw = [
        bx([w[0], ya, t[0]], [w[1], ya + 1.0, t[1]]), // bottom bar
        bx([w[0], top, t[0]], [w[1], top + 1.0, t[1]]), // top bar
        bx([w[0], ya + 1.0, t[0]], [w[0] + 1.0, top, t[1]]), // legs
        bx([w[1] - 1.0, ya + 1.0, t[0]], [w[1], top, t[1]]),
    ];
    // The turned ring swaps its wide and thin horizontal axes; both extents
    // are centred on the cell axis, so a plain coordinate swap is exact.
    raw.map(|b| {
        if turned {
            ShapeAabb {
                min: [b.min[2], b.min[1], b.min[0]],
                max: [b.max[2], b.max[1], b.max[0]],
            }
        } else {
            b
        }
    })
}

const fn bx(min: [f32; 3], max: [f32; 3]) -> ShapeAabb {
    ShapeAabb {
        min: [min[0] / 16.0, min[1] / 16.0, min[2] / 16.0],
        max: [max[0] / 16.0, max[1] / 16.0, max[2] / 16.0],
    }
}

/// Map a VERTICAL-row box onto axis `i` of [`Chains::rows`] (0 vertical, 1
/// north/south, 2 east/west) — the run axis swaps with one of the cross axes,
/// so one authored table serves all three rows.
fn to_axis(b: ShapeAabb, i: usize) -> ShapeAabb {
    let pick = |v: [f32; 3]| match i {
        1 => [v[0], v[2], v[1]],
        2 => [v[1], v[0], v[2]],
        _ => v,
    };
    let (lo, hi) = (pick(b.min), pick(b.max));
    ShapeAabb {
        min: [lo[0].min(hi[0]), lo[1].min(hi[1]), lo[2].min(hi[2])],
        max: [lo[0].max(hi[0]), lo[1].max(hi[1]), lo[2].max(hi[2])],
    }
}

/// This cell's whole chain, as the VERTICAL row authors it: every ring whose
/// span reaches into the cell, CLIPPED to it (a ring crossing the cell top
/// reappears as the stub ring the cell above clips at its floor — same pure
/// pattern, so a stack of cells is one unbroken run). The lantern's bail clips
/// this same list rather than restating it (`lanterns::bail`), so a lamp
/// strung under a chain continues the run exactly.
pub(super) fn cell_links() -> Vec<ShapeAabb> {
    let mut out = Vec::new();
    // One anchor below the cell (the stub) through one at its top; anchors
    // whose ring misses the cell clip to nothing, so the bounds only need to
    // be generous, not exact, and they survive a LINK_PITCH retune.
    for k in -1i32..=(16.0 / LINK_PITCH) as i32 {
        for b in ring(k as f32 * LINK_PITCH, k.rem_euclid(2) == 1) {
            let (lo, hi) = (b.min[1].max(0.0), b.max[1].min(1.0));
            if hi > lo {
                out.push(ShapeAabb {
                    min: [b.min[0], lo, b.min[2]],
                    max: [b.max[0], hi, b.max[2]],
                });
            }
        }
    }
    out
}

impl Chains {
    /// The row for a clicked face's normal: top/bottom hangs a vertical
    /// chain, a side face lays it along that face's axis (vanilla rule).
    pub(super) fn row_for_normal(&self, n: [i32; 3]) -> BlockId {
        if n[1] != 0 {
            self.rows[0]
        } else if n[0] != 0 {
            self.rows[2]
        } else {
            self.rows[1]
        }
    }

    /// The link boxes for a placed chain cell, oriented by its block id (the
    /// bake's whole orientation input — a pure function of the cell).
    pub(super) fn links_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let axis = self.rows.iter().position(|&r| r == block).unwrap_or(0);
        cell_links().into_iter().map(|b| to_axis(b, axis)).collect()
    }
}

/// Resolve the chain family at init: the shared shape kind and its three
/// axis rows (registry-only calls, legal on any instance — the bakes and the
/// placement plan run on both). `None` when the pack content didn't load (a
/// row renamed or removed) — the chair half of the mod keeps working and
/// chains fall back to the row's static (cube) shape.
pub(super) fn resolve_chains() -> Option<Chains> {
    Some(Chains {
        shape: resolve_shape("furniture:chain")?,
        rows: [
            resolve_block("furniture:chain")?,
            resolve_block("furniture:chain_ns")?,
            resolve_block("furniture:chain_ew")?,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A chain cell's pattern must CONTINUE across the cell boundary: every
    /// box the cell top cuts off must be met, in the identical pattern one
    /// cell up, by a box starting at the floor that overlaps its footprint.
    /// A miss here is a one-texel gap at EVERY cell boundary of every run —
    /// the wrap stub (`cell_links`' `k = -1` ring) is exactly what closes it,
    /// and this is what fails if the anchor bounds ever stop covering it.
    #[test]
    fn a_stack_of_chain_cells_has_no_seam_at_the_boundary() {
        let links = cell_links();
        let tops: Vec<_> = links.iter().filter(|b| b.max[1] == 1.0).collect();
        assert!(!tops.is_empty(), "some box must reach the cell top");
        for t in &tops {
            assert!(
                links.iter().any(|b| b.min[1] == 0.0
                    && b.min[0] < t.max[0]
                    && t.min[0] < b.max[0]
                    && b.min[2] < t.max[2]
                    && t.min[2] < b.max[2]),
                "{t:?} ends at the cell top with nothing continuing it above"
            );
        }
    }

    /// Every height of the cell holds chain matter — a bare band would read
    /// as a cut run repeated once per cell.
    #[test]
    fn the_chain_occupies_every_height() {
        let links = cell_links();
        for i in 0..32 {
            let y = (i as f32 + 0.5) / 32.0;
            assert!(
                links.iter().any(|b| b.min[1] <= y && y < b.max[1]),
                "no chain matter at y {y}"
            );
        }
    }

    /// Consecutive links alternate orientation — that is what reads as a
    /// chain rather than a ladder of parallel plates, and nothing else in the
    /// bake enforces it.
    #[test]
    fn consecutive_links_alternate_orientation() {
        let wide_x = |b: &ShapeAabb| b.max[0] - b.min[0] > b.max[2] - b.min[2];
        // Classify each bar row's owner by which axis its bar is wide in.
        let links = cell_links();
        let mut orientations = Vec::new();
        let mut ya = 0.0f32;
        while ya < 1.0 {
            let bar = links
                .iter()
                .filter(|b| b.min[1] <= ya && ya < b.max[1])
                .max_by(|a, b| {
                    let w = |x: &ShapeAabb| (x.max[0] - x.min[0]) + (x.max[2] - x.min[2]);
                    w(a).partial_cmp(&w(b)).unwrap()
                })
                .expect("a bar at every anchor");
            orientations.push(wide_x(bar));
            ya += LINK_PITCH / 16.0;
        }
        assert!(orientations.len() >= 2);
        for pair in orientations.windows(2) {
            assert_ne!(pair[0], pair[1], "adjacent links must be turned 90°");
        }
    }
}
