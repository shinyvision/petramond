//! The per-edge movement gate every navigation search shares: hazards, then
//! the body's real sweep through partial shapes.

use std::cell::RefCell;

use petramond_math::math::IVec3;
use petramond_world::block::Aabb;
use rustc_hash::FxHashMap;

use super::{classify_boxes, hazards, CellShape};
use crate::mob::path::{self, PathParams};
use crate::world::SectionCursor;
use petramond_world::collision;

/// The collision boxes navigation must sweep against in `c` — the PARTIAL
/// shapes only. Full cubes are already resolved exactly by the cell probes and
/// empty cells contribute nothing, so both answer an empty slice.
fn nav_partial_boxes(cur: &SectionCursor<'_>, c: IVec3) -> &'static [Aabb] {
    let boxes = cur.collision_boxes(c);
    match classify_boxes(boxes) {
        CellShape::Partial => boxes,
        CellShape::Empty | CellShape::Full => &[],
    }
}

/// The top of the standing surface under foothold `cell`, in `[0, 1]` above
/// the floor cell's base: 1.0 for a full cube (or fluid/air — feet at the
/// cell base), a partial floor's highest box top otherwise (a slab-top
/// foothold stands half a block below its cell base).
pub(super) fn floor_top(cur: &SectionCursor<'_>, floor: IVec3) -> f32 {
    let boxes = cur.collision_boxes(floor);
    if boxes.is_empty() {
        return 1.0;
    }
    boxes
        .iter()
        .fold(0.0f32, |acc, b| acc.max(b.max[1]))
        .clamp(0.0, 1.0)
}

/// The per-edge movement gate: refuses entering hazardous terrain from safety,
/// then checks whether the mob's REAL body AABB (its true half-width and height, feet at
/// the true floor height) can sweep that move through the real collision boxes
/// of every PARTIAL cell it crosses — with the ordinary [`collision::STEP_HEIGHT`]
/// allowance, so a low shape (a slab lying in the way) is stepped over while a
/// tall thin one (a ladder panel, a pane, a closed door) blocks exactly where
/// it physically blocks. The destination pose is checked too, so a jump-up
/// into a partial shape is refused instead of bonked into.
///
/// Cells with no partial collision cost one memoized classification each, so
/// over plain terrain the gate is a cheap table lookup and the sweep only runs
/// where partial shapes actually are. Shared with `mob::confined`, whose
/// reachability fill must agree with the routes this gate admits (a lone
/// fence refuses the jump from below; a step beside it opens the way over).
pub(in crate::mob) fn navigation_step_gate<'c, 'w>(
    cur: &'c SectionCursor<'w>,
    params: PathParams,
    height: f32,
) -> impl Fn(IVec3, IVec3) -> bool + use<'c, 'w> {
    navigation_step_gate_on(
        cur,
        params,
        height,
        [(); 4].map(|()| path::CellMemo::<1024>::default()),
    )
}

/// [`navigation_step_gate`] on the caller's memos (partial shapes, hazardous
/// cells, footholds in hazard, plain columns): a search over a known box
/// brings dense ones.
pub(super) fn navigation_step_gate_on<'c, 'w, M: path::CellCache>(
    cur: &'c SectionCursor<'w>,
    params: PathParams,
    height: f32,
    memos: [M; 4],
) -> impl Fn(IVec3, IVec3) -> bool + use<'c, 'w, M> {
    let [partial_here, hazard_cells, hazard_footholds, plain_columns] = memos;
    let hazard_allowed = hazards::step_gate_on(cur, params, hazard_cells, hazard_footholds);
    let cache: RefCell<FxHashMap<IVec3, &'static [Aabb]>> = RefCell::new(FxHashMap::default());
    let height = height.max(0.5);
    let narrow = params.half_width <= 0.5;
    // A foothold's column, floor through the cell over the head, holding no
    // partial shape and no hazard: everything either half of the gate reads
    // for a level step out of it. Most ground is plain, and a level step
    // between plain columns is settled by this one fact per column.
    let plain = move |c: IVec3| {
        plain_columns.get(c, |c| {
            (c.y - 1..=c.y + params.head_cells()).all(|y| {
                let at = IVec3::new(c.x, y, c.z);
                let block = cur.physics_block(at);
                !hazards::hazardous(block, params.tolerated)
                    && classify_boxes(cur.boxes_of(at, block)) != CellShape::Partial
            })
        })
    };
    move |from: IVec3, to: IVec3| {
        if narrow && from.y == to.y && from != to && plain(from) && plain(to) {
            let (a, b) = (
                IVec3::new(to.x, from.y, from.z),
                IVec3::new(from.x, from.y, to.z),
            );
            if (from.x == to.x || from.z == to.z) || (plain(a) && plain(b)) {
                return true;
            }
        }
        if !hazard_allowed(from, to) {
            return false;
        }
        let dx = (to.x - from.x) as f32;
        let dz = (to.z - from.z) as f32;
        if dx == 0.0 && dz == 0.0 {
            return true;
        }
        let boxes_at = |x: i32, y: i32, z: i32| -> &'static [Aabb] {
            cache
                .borrow_mut()
                .entry(IVec3::new(x, y, z))
                .or_insert_with(|| nav_partial_boxes(cur, IVec3::new(x, y, z)))
        };
        // Fast path: no partial shape anywhere the body could touch during this
        // step (both columns, floor through head, padded for wide bodies).
        let diagonal = dx != 0.0 && dz != 0.0;
        let half_width = params.half_width.max(0.0);
        let pad = ((half_width - 0.5).max(0.0)).ceil() as i32;
        let body_y = from.y.min(to.y);
        let y_lo = body_y - 1;
        let y_hi = from.y.max(to.y) + params.head_cells();
        let mut any_partial = false;
        'scan: for x in (from.x.min(to.x) - pad)..=(from.x.max(to.x) + pad) {
            for z in (from.z.min(to.z) - pad)..=(from.z.max(to.z) + pad) {
                for y in y_lo..=y_hi {
                    // Straight from the world: the boxes cache is for the
                    // rare accurate sweep, not for every cell a search sees.
                    let here = partial_here.get(IVec3::new(x, y, z), |c| {
                        !nav_partial_boxes(cur, c).is_empty()
                    });
                    if here {
                        // A DIAGONAL step near a partial shape at body level is
                        // refused outright: the sweep below is axis-ordered
                        // (X then Z), which can clear an L-shaped path while
                        // the TRUE straight diagonal the mob walks clips the
                        // shape's corner — the walking-against-a-trough bug.
                        // Cardinal detours around the shape stay available
                        // (and are what a watching player expects to see).
                        // Partial FLOORS (a slab underfoot) don't trigger
                        // this; only boxes the body itself could touch do.
                        if diagonal && y >= body_y {
                            return false;
                        }
                        any_partial = true;
                        if !diagonal {
                            break 'scan;
                        }
                    }
                }
            }
        }
        if !any_partial {
            return true;
        }

        // Accurate sweep: the body starts standing at `from` (feet on the real
        // floor top) and must travel the full horizontal move.
        let (hw, h) = (f64::from(half_width), f64::from(height));
        let feet = f64::from(from.y - 1) + f64::from(floor_top(cur, from - IVec3::Y));
        let dest_feet = f64::from(to.y - 1) + f64::from(floor_top(cur, to - IVec3::Y));
        let cx = f64::from(from.x) + 0.5;
        let cz = f64::from(from.z) + 0.5;
        // A climb rises in its own column first — a partial shape over the head
        // (a pane) stops the jump though neither standing pose touches it —
        // and then moves over at the height it lands at: swept from the lower
        // level, a stair's own back step would block the climb onto it. A
        // nav-solid landing (a fence top) keeps the ground-level sweep: pens
        // hold by design.
        // A jump lifts a body one block. A floor sunk below its cell (a chest's
        // or a lantern's top) makes one cell up onto a full top more than that.
        if dest_feet - feet > f64::from(path::CLIMB_CELLS) + 1e-3 {
            return false;
        }
        let climb = dest_feet > feet + 1e-3 && !cur.physics_block(to - IVec3::Y).nav_reads_solid();
        if dest_feet > feet + 1e-3 {
            let lmin = [cx - hw, dest_feet + 1e-3, cz - hw];
            let lmax = [cx + hw, dest_feet + h, cz + hw];
            if collision::aabb_hits_cells(lmin, lmax, boxes_at) {
                return false;
            }
        }
        let sweep_feet = if climb { dest_feet + 1e-3 } else { feet };
        let min = [cx - hw, sweep_feet, cz - hw];
        let max = [cx + hw, sweep_feet + h, cz + hw];
        let (moved, _, _) =
            collision::step_horizontal(min, max, dx, dz, collision::STEP_HEIGHT, boxes_at);
        if (moved[0] - dx).abs() >= 1e-3 || (moved[2] - dz).abs() >= 1e-3 {
            return false;
        }

        // Destination pose: standing at `to` must not intersect a partial shape.
        let tx = f64::from(to.x) + 0.5;
        let tz = f64::from(to.z) + 0.5;
        let dmin = [tx - hw, dest_feet + 1e-3, tz - hw];
        let dmax = [tx + hw, dest_feet + h, tz + hw];
        !collision::aabb_hits_cells(dmin, dmax, boxes_at)
    }
}
