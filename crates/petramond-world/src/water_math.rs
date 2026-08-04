//! Water-cell metadata layout and pure surface math, shared by the flow SIM
//! (which owns spreading/re-leveling over the world) and the MESHER (surface
//! heights, flow direction for the flowing-water texture) — one canonical
//! meta→height/flow mapping so geometry and simulation cannot drift.

use crate::block::Block;
use crate::mathh::{IVec3, Vec3};

/// `meta` byte layout for a water cell:
///   bit 7     — FALLING: a vertical stream (full amount, renders full).
///   bits 0..4 — `level`: 0 = source, 1..=7 = flowing distance from a source.
pub const FALLING: u8 = 0x80;
pub const LEVEL_MASK: u8 = 0x0F;

#[inline]
pub fn level(meta: u8) -> u8 {
    // Clamped: metadata written before the 7-cell reach (and its retired
    // thickness bits, 0x70) may still be in old saves; the first flow check
    // rewrites such a cell clean.
    (meta & LEVEL_MASK).min(7)
}
#[inline]
pub fn is_falling(meta: u8) -> bool {
    meta & FALLING != 0
}
/// A full, still source: level 0 and not falling (worldgen water is all this).
#[inline]
pub fn is_source(meta: u8) -> bool {
    meta & (LEVEL_MASK | FALLING) == 0
}
/// How much water the cell holds, 1..=8: full for sources and falling cells,
/// `8 - level` for flowing ones. Drives spreading strength and surface height.
#[inline]
pub fn amount(meta: u8) -> u8 {
    if is_source(meta) || is_falling(meta) {
        8
    } else {
        8 - level(meta)
    }
}
pub const FLOW_DIR_EPS_SQ: f32 = 1e-4;

/// Rendered/contact surface height (0..1) of a water cell — `1.0` when
/// [`fills_cell`] says the cell presents no open surface, else the canonical
/// meta->height mapping shared with the mesher so flow geometry and simulation
/// stay in lockstep: `amount / 9`, so a source's top sits slightly recessed
/// (8/9) and reads as liquid, and each level steps down from there.
pub fn fluid_height(meta: u8, above: Block) -> f32 {
    if fills_cell(meta, above) {
        return 1.0;
    }
    amount(meta) as f32 / 9.0
}

/// True when this water cell renders (and contact-probes) as a full
/// block-height volume rather than an open, recessed/sloped surface: more
/// WATER directly above (a mid-column cell), or a FALLING stream cell (a full
/// column that joins seamlessly to the cell above and to the water it lands
/// in — no mid-waterfall step).
///
/// SOLID lids deliberately do NOT cap: water under ANY block — ice, stone, a
/// placed block — keeps the same recessed 8/9 pocket under it, uniformly
/// (three lid variants were tried on 2026-07-16 and all rejected by playtest:
/// any-solid seals, still-source-under-solid seals, still-source-under-ice
/// seals). The calm look of those pockets comes from the STILL-SOURCE flow
/// rules instead ([`surface_flow_dir`] + the mesher's still side tiles), not
/// from faking the height. The flow SIM is untouched by all of this; the
/// mesher, buoyancy/contact probes, and the underwater-camera test share this
/// one rule.
pub fn fills_cell(meta: u8, above: Block) -> bool {
    above == Block::Water || is_falling(meta)
}

/// Whether this water meta is a STILL SOURCE — exposed for the mesher's flow
/// probe (see [`surface_flow_dir`]): two adjacent still sources never flow
/// into each other, whatever their rendered heights.
#[inline]
pub fn is_still_source(meta: u8) -> bool {
    is_source(meta)
}

/// Horizontal direction of the rendered water flow at a cell, using the same
/// surface-gradient rule that rotates the flowing-water top texture. Returns
/// zero for still/flat water and for non-water cells.
///
/// Flow direction is a statement about the SIM STATE, not about rendered
/// heights: between two STILL SOURCES there is no flow — period — so their
/// height difference contributes nothing. Without that rule, the recessed
/// 8/9 cell under any block sitting in the sea slopes against its full
/// mid-column neighbours and the whole neighbourhood grows animated flow
/// streaks plus a phantom current, on water that is entirely still. Real
/// gradients survive: flowing/falling metas, and the pull toward an open
/// air edge (where a source genuinely will spread).
pub fn surface_flow_dir<B, F, S>(
    wx: i32,
    wy: i32,
    wz: i32,
    block_at: &B,
    fluid_at: &F,
    still_at: &S,
) -> Vec3
where
    B: Fn(i32, i32, i32) -> Block,
    F: Fn(i32, i32, i32) -> Option<f32>,
    S: Fn(i32, i32, i32) -> bool,
{
    let Some(my_h) = fluid_at(wx, wy, wz) else {
        return Vec3::ZERO;
    };
    let i_am_still = still_at(wx, wy, wz);

    let mut fvx = 0.0f32;
    let mut fvz = 0.0f32;
    for d in CARDINALS {
        let (nx, nz) = (wx + d.x, wz + d.z);
        let nb = block_at(nx, wy, nz);
        let nh = if nb == Block::Water {
            if i_am_still && still_at(nx, wy, nz) {
                continue; // still source ↔ still source: no flow between them
            }
            fluid_at(nx, wy, nz).unwrap_or(my_h)
        } else if nb == Block::Air {
            0.0
        } else {
            continue;
        };
        let diff = my_h - nh;
        fvx += d.x as f32 * diff;
        fvz += d.z as f32 * diff;
    }

    let flow = Vec3::new(fvx, 0.0, fvz);
    if flow.length_squared() > FLOW_DIR_EPS_SQ {
        flow.normalize()
    } else {
        Vec3::ZERO
    }
}

pub const DOWN: IVec3 = IVec3::new(0, -1, 0);
pub const UP: IVec3 = IVec3::new(0, 1, 0);
/// North (-Z), east (+X), south (+Z), west (-X) — the horizontal flow set.
pub const CARDINALS: [IVec3; 4] = [
    IVec3::new(0, 0, -1),
    IVec3::new(1, 0, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(-1, 0, 0),
];
