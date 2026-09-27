//! Chains: three rows (`chain`, `chain_ns`, `chain_ew`) share one shape. The axis is block
//! identity, so the bake needs no per-cell state. Placement picks the row from the clicked face's
//! normal, and the engine predicts it.

use mod_sdk::*;

use crate::keys;

/// The chain family: the shared shape-kind id and its three axis rows —
/// vertical (the item-linked base), north/south, east/west.
pub(super) struct Chains {
    pub(super) shape: u16,
    pub(super) rows: [BlockId; 3],
}

/// The chain is real geometry: a run of flat RINGS, each with a genuine hole,
/// every ring turned 90 degrees from the one before, authored for the VERTICAL
/// row and mapped onto the other two axes by [`to_axis`].
///
/// Each link has a real hole, so adjacent links remain distinct when viewed
/// off-axis. The tile provides shading for the geometry (`gen_chain.py`).
///
/// One link is exactly [`LINK_PITCH`] tall — [`LINK_W`] wide x [`LINK_T`] thick
/// with a real 1x2 hole — so consecutive links BUTT rather than overlap, and
/// the cell holds a whole number of them with nothing to clip.
///
/// `mesh::boxset` does not cull interpenetrating faces. Butted contact lets the
/// emitter cull flush faces on both sides, leaving no interior geometry.
///
/// `LINK_PITCH` must divide 16 with an EVEN link count so the 90-degree
/// alternation continues across cell boundaries.
const LINK_PITCH: f32 = 4.0;
const LINK_W: [f32; 2] = [6.5, 9.5];
const LINK_T: [f32; 2] = [7.0, 9.0];

fn ring(ya: f32, turned: bool) -> [ShapeAabb; 4] {
    let top = ya + LINK_PITCH - 1.0;
    let (w, t) = (LINK_W, LINK_T);
    let raw = [
        bx([w[0], ya, t[0]], [w[1], ya + 1.0, t[1]]),
        bx([w[0], top, t[0]], [w[1], top + 1.0, t[1]]),
        bx([w[0], ya + 1.0, t[0]], [w[0] + 1.0, top, t[1]]),
        bx([w[1] - 1.0, ya + 1.0, t[0]], [w[1], top, t[1]]),
    ];
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

pub(super) fn cell_links() -> Vec<ShapeAabb> {
    (0..(16.0 / LINK_PITCH) as i32)
        .flat_map(|k| ring(k as f32 * LINK_PITCH, k.rem_euclid(2) == 1))
        .collect()
}

impl Chains {
    pub(super) fn row_for_normal(&self, n: [i32; 3]) -> BlockId {
        if n[1] != 0 {
            self.rows[0]
        } else if n[0] != 0 {
            self.rows[2]
        } else {
            self.rows[1]
        }
    }

    pub(super) fn links_for(&self, block: BlockId) -> Vec<ShapeAabb> {
        let axis = self.rows.iter().position(|&r| r == block).unwrap_or(0);
        cell_links().into_iter().map(|b| to_axis(b, axis)).collect()
    }
}

pub(super) fn resolve_chains() -> Option<Chains> {
    Some(Chains {
        shape: resolve_shape(keys::CHAIN_SHAPE)?,
        rows: [
            resolve_block(keys::CHAIN)?,
            resolve_block(keys::CHAIN_NS)?,
            resolve_block(keys::CHAIN_EW)?,
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn no_two_link_boxes_share_volume() {
        let links = cell_links();
        for (i, a) in links.iter().enumerate() {
            for (j, b) in links.iter().enumerate().skip(i + 1) {
                let overlaps = (0..3).all(|k| a.min[k] < b.max[k] && b.min[k] < a.max[k]);
                assert!(!overlaps, "boxes {i} and {j} interpenetrate: {a:?} {b:?}");
            }
        }
    }

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

    #[test]
    fn consecutive_links_alternate_orientation() {
        let wide_x = |b: &ShapeAabb| b.max[0] - b.min[0] > b.max[2] - b.min[2];
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
