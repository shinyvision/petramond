//! Fence connection shape shared by placement, collision, selection, and
//! meshing. A fence stores NO per-cell state: its shape is a 4-bit mask of
//! horizontal connections, resolved from the current neighbours every time it
//! is queried (like stair corners and panes), so placing or removing a
//! neighbour reshapes the fence through the ordinary neighbourhood remesh with
//! nothing to persist.
//!
//! A fence connects toward a side when the neighbour offers wood-tight backing:
//! another fence (any wood type), a full solid OPAQUE cube (leaves, glass and
//! other transparent blocks never join), or the flat high/back side of a stair.
//! A slab cell joins only as a full stack; single slabs never do. Rendered, a
//! fence is a centre post growing a pair of horizontal rails per connected
//! side; collision/selection use the simpler post + full-height arm runs
//! (pane-style, at the post's thickness).

use crate::block::Aabb;
use crate::connect;
use crate::mathh::{IVec3, Vec3, MAX_SELECTION_BOXES};

/// The post's horizontal extent: `4/16` across, centred in the cell.
pub const POST_LO: f32 = 6.0 / 16.0;
pub const POST_HI: f32 = 10.0 / 16.0;

/// A rail is inset half a texel from each post face, so it sits centred on the
/// post and its cross bounds land on half-texels (the cell-local UV rounding
/// keeps the face sampling exact). Derived from the post so a wider/narrower
/// modded post carries its rails with it.
pub const RAIL_INSET: f32 = 0.5 / 16.0;

/// The rail cross extent for a post spanning `post_lo..post_hi` — inset
/// [`RAIL_INSET`] from each face. The mesher and item form both derive the rail
/// from the shape's own post params through here, rather than from fixed
/// constants, so the rail tracks a modded post's thickness.
#[inline]
pub const fn rail_cross(post_lo: f32, post_hi: f32) -> (f32, f32) {
    (post_lo + RAIL_INSET, post_hi - RAIL_INSET)
}

/// The top rail sits 2/16 below the cell top; the bottom rail 2/16 above the
/// cell floor. Both are 3/16 thick.
pub const RAIL_TOP_LO: f32 = 11.0 / 16.0;
pub const RAIL_TOP_HI: f32 = 14.0 / 16.0;
pub const RAIL_BOT_LO: f32 = 2.0 / 16.0;
pub const RAIL_BOT_HI: f32 = 5.0 / 16.0;

/// Cell-local boxes lifted to world space for the selection outline (a fence
/// has at most 2 runs, under the outline cap).
#[inline]
pub fn world_boxes(origin: IVec3, boxes: &[Aabb]) -> ([(Vec3, Vec3); MAX_SELECTION_BOXES], u8) {
    connect::world_boxes(origin, boxes)
}

/// The out-of-world fence (inventory icon, held item, dropped stack): two posts
/// at the cell's edges joined by the two rails — a complete fence segment in
/// one cell, drawn with the same cell-local UVs as the placed shape. The posts
/// sit at the edges so the rails get their full visible span and the segment
/// reads as a fence, not a pair of columns. Derived from the shape's post
/// extent so a modded wall's item matches its placed form.
///
/// A post `post_lo..post_hi` wide is drawn as thickness `post_hi - post_lo`, one
/// post flush to each cell edge; the rails bridge the gap between the inner post
/// faces at the [`rail_cross`] depth.
pub const fn item_posts(post_lo: f32, post_hi: f32) -> [Aabb; 2] {
    let thickness = post_hi - post_lo;
    [
        Aabb {
            min: [0.0, 0.0, post_lo],
            max: [thickness, 1.0, post_hi],
        },
        Aabb {
            min: [1.0 - thickness, 0.0, post_lo],
            max: [1.0, 1.0, post_hi],
        },
    ]
}

/// The item rails bridge the gap between the two [`item_posts`] along X; their
/// ends butt against the post faces, so only the four long faces are ever drawn.
pub const fn item_rails(post_lo: f32, post_hi: f32) -> [Aabb; 2] {
    let thickness = post_hi - post_lo;
    let rail_lo = post_lo + RAIL_INSET;
    let rail_hi = post_hi - RAIL_INSET;
    [
        Aabb {
            min: [thickness, RAIL_TOP_LO, rail_lo],
            max: [1.0 - thickness, RAIL_TOP_HI, rail_hi],
        },
        Aabb {
            min: [thickness, RAIL_BOT_LO, rail_lo],
            max: [1.0 - thickness, RAIL_BOT_HI, rail_hi],
        },
    ]
}

#[cfg(test)]
mod tests {
    // The connection mask + box tests moved to `crate::connect` (the shared,
    // param-driven owner); here only the fence's fixed item segment remains.
    use super::*;

    #[test]
    fn item_shape_is_two_posts_bridged_by_the_rails() {
        let posts = item_posts(POST_LO, POST_HI);
        for post in posts {
            assert_eq!(post.min[1], 0.0);
            assert_eq!(post.max[1], 1.0);
            assert_eq!(post.min[2], POST_LO);
            assert_eq!(post.max[2], POST_HI);
        }
        let [west_post, east_post] = posts;
        for rail in item_rails(POST_LO, POST_HI) {
            // The rails exactly bridge the gap, butting both post faces.
            assert_eq!(rail.min[0], west_post.max[0]);
            assert_eq!(rail.max[0], east_post.min[0]);
            assert!(rail.min[1] > 0.0 && rail.max[1] < 1.0);
            assert!(rail.min[2] >= POST_LO && rail.max[2] <= POST_HI);
        }
    }
}
