//! The golem's body as a tick reads it, and where it stands.

use mod_sdk::*;

use super::Ctx;
use crate::content::GOLEM;
use crate::geometry::{cell_of, feet_of};

pub struct Body {
    pub id: u64,
    pub pos: [f64; 3],
    pub cell: [i32; 3],
    pub on_ground: bool,
    pub yaw: f32,
    pub slots: Vec<Option<ItemStackData>>,
}

impl Body {
    pub fn actor(&self) -> EntityRef {
        EntityRef::Mob(self.id)
    }

    /// How far the body stands from the centre of its cell, horizontally.
    pub fn off_centre(&self) -> [f64; 2] {
        let centre = feet_of(self.cell);
        [centre[0] - self.pos[0], centre[2] - self.pos[2]]
    }
}

/// The foothold a grounded body stands on, as the navigator reads it. Feet
/// resting partway up a cell (a slab, a stair) stand in the cell above it,
/// not the one they floor to; and a footprint over a ledge's edge (a slab's
/// rim) may rest on the cell beside. Every route is asked from here.
pub(super) fn standing_cell(pos: [f64; 3]) -> [i32; 3] {
    let own = cell_of(pos);
    let half = f64::from(crate::content::GOLEM_HALF_WIDTH);
    let raised = pos[1] - pos[1].floor() > 0.01;
    let mut under = vec![own];
    for dx in [-half, half] {
        for dz in [-half, half] {
            let c = cell_of([pos[0] + dx, pos[1], pos[2] + dz]);
            if !under.contains(&c) {
                under.push(c);
            }
        }
    }
    if under.len() == 1 && !raised {
        return own;
    }
    let mut cells = Vec::with_capacity(under.len() * 2);
    for c in &under {
        if raised {
            cells.push(crate::geometry::offset(*c, [0, 1, 0]));
        }
        cells.push(*c);
    }
    cells
        .iter()
        .zip(footholds(GOLEM, cells.clone()))
        .find_map(|(c, ok)| ok.then_some(*c))
        .unwrap_or(own)
}

/// A body wedged under a block (physics pushed it there) cannot walk out;
/// set it down on the nearest free standing cell instead. Returns whether it
/// was moved.
pub(super) fn unwedge(ctx: &mut Ctx, body: &Body) -> bool {
    const HEIGHT: f64 = 1.5;
    let top = body.pos[1] + HEIGHT - 0.05;
    let head = cell_of([body.pos[0], top, body.pos[2]]);
    if head[1] == body.cell[1] {
        return false;
    }
    // Just inside the body's sides: a box it only brushes does not wedge it.
    let half = f64::from(crate::content::GOLEM_HALF_WIDTH) - 0.02;
    let reach_in = (top - f64::from(head[1])) as f32;
    let (x, z) = (
        body.pos[0] - f64::from(head[0]),
        body.pos[2] - f64::from(head[2]),
    );
    let wedged = get_block(head).is_some_and(|b| {
        ctx.caches.block(b).is_some_and(|info| {
            info.collision.iter().any(|(lo, hi)| {
                lo[1] < reach_in
                    && f64::from(lo[0]) < x + half
                    && f64::from(hi[0]) > x - half
                    && f64::from(lo[2]) < z + half
                    && f64::from(hi[2]) > z - half
            })
        })
    });
    if !wedged {
        return false;
    }
    let Some(spot) = standing_near(body).into_iter().next() else {
        return false;
    };
    trace!(
        "TRACE teleport: wedged at {:?}, set down at {spot:?}",
        body.cell
    );
    mob_kinematic(body.id, feet_of(spot), body.yaw, 0.0, 0.0);
    true
}

/// Whether `cell` is a foothold for the golem as the world stands.
pub(super) fn stands_at(cell: [i32; 3]) -> bool {
    footholds(GOLEM, vec![cell]).first() == Some(&true)
}

/// Standing room within two cells of the body, nearest first.
fn standing_near(body: &Body) -> Vec<[i32; 3]> {
    let mut around = Vec::new();
    for dy in [0, 1, -1] {
        for dx in -2..=2 {
            for dz in -2..=2 {
                around.push(crate::geometry::offset(body.cell, [dx, dy, dz]));
            }
        }
    }
    around.sort_by_key(|c| crate::geometry::manhattan(*c, body.cell));
    let free = footholds(GOLEM, around.clone());
    around
        .into_iter()
        .zip(free)
        .filter_map(|(c, ok)| ok.then_some(c))
        .collect()
}
