use crate::block::Block;
use crate::mathh::{IVec3, Vec3};

pub const FALLING: u8 = 0x80;
pub const LEVEL_MASK: u8 = 0x0F;

#[inline]
pub fn level(meta: u8) -> u8 {
    (meta & LEVEL_MASK).min(7)
}
#[inline]
pub fn is_falling(meta: u8) -> bool {
    meta & FALLING != 0
}
#[inline]
pub fn is_source(meta: u8) -> bool {
    meta & (LEVEL_MASK | FALLING) == 0
}
#[inline]
pub fn amount(meta: u8) -> u8 {
    if is_source(meta) || is_falling(meta) {
        8
    } else {
        8 - level(meta)
    }
}
pub const FLOW_DIR_EPS_SQ: f32 = 1e-4;

/// Rendered/contact surface height (0..1) of a fluid cell — `1.0` when
/// [`fills_cell`] says the cell presents no open surface, else the canonical
/// meta->height mapping shared with the mesher so flow geometry and simulation
/// stay in lockstep: `amount / 9`, so a source's top sits slightly recessed
/// (8/9) and reads as liquid, and each level steps down from there.
pub fn fluid_height(meta: u8, above: Block, fluid: Block) -> f32 {
    if fills_cell(meta, above, fluid) {
        return 1.0;
    }
    amount(meta) as f32 / 9.0
}

/// True when this fluid cell renders (and contact-probes) as a full
/// block-height volume rather than an open, recessed/sloped surface: more of
/// the SAME FLUID directly above (a mid-column cell), or a FALLING stream cell
/// (a full column that joins seamlessly to the cell above and to the fluid it
/// lands in — no mid-waterfall step).
///
/// Solid lids do not cap fluid: a cell under any solid block keeps its
/// recessed 8/9 surface. Still-source flow rules ([`surface_flow_dir`] and
/// the mesher's still side tiles) keep these pockets calm. The
/// mesher, buoyancy/contact probes, and the submerged-camera test share this
/// one rule.
pub fn fills_cell(meta: u8, above: Block, fluid: Block) -> bool {
    above.fluid() == Some(fluid) || is_falling(meta)
}

#[inline]
pub fn is_still_source(meta: u8) -> bool {
    is_source(meta)
}

/// Horizontal direction of the rendered fluid flow at a cell, using the same
/// surface-gradient rule that rotates the flowing-fluid top texture. Returns
/// zero for still/flat fluid and for cells of a DIFFERENT fluid (`fluid`
/// selects which body the probes read).
///
/// Flow direction is a statement about the SIM STATE, not about rendered
/// heights: between two STILL SOURCES there is no flow — period — so their
/// height difference contributes nothing. Without that rule, the recessed
/// 8/9 cell under any block sitting in the sea slopes against its full
/// mid-column neighbours and the whole neighbourhood grows animated flow
/// streaks plus a phantom current, on fluid that is entirely still. Real
/// gradients survive: flowing/falling metas, and the pull toward an open
/// air edge (where a source genuinely will spread).
pub fn surface_flow_dir<B, F, S>(
    wx: i32,
    wy: i32,
    wz: i32,
    fluid: Block,
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
        let nh = if nb.fluid() == Some(fluid) {
            if i_am_still && still_at(nx, wy, nz) {
                continue;
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
pub const CARDINALS: [IVec3; 4] = [
    IVec3::new(0, 0, -1),
    IVec3::new(1, 0, 0),
    IVec3::new(0, 0, 1),
    IVec3::new(-1, 0, 0),
];
