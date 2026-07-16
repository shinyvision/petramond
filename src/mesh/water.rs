//! Water-surface GEOMETRY for the chunk mesher.
//!
//! Owns everything about how a water cell is *shaped* into a mesh: its per-corner
//! surface heights (so adjacent cells join into one continuous sloped sheet), the
//! top tile + flow rotation, the per-vertex height warp, the exposed-step decision
//! for a full cell standing over a shorter neighbour, and the surface's reverse
//! winding. The fluid MATH (`fluid_height`, `fills_cell`, `surface_flow_dir`) stays
//! in `crate::world::water`; this module only turns those values into vertices.

use crate::atlas::Tile;
use crate::block::Block;

/// Whether a water cell's side face toward a neighbouring water cell is culled or
/// kept as the exposed vertical step (a full cell standing over a shorter,
/// open-surface neighbour — rendered as a band trimmed to the neighbour's surface).
pub(super) enum SideVsWater {
    /// Cull the face (the two water surfaces meet, nothing to draw).
    Cull,
    /// Keep it: render the exposed step, trimming its bottom to the neighbour.
    ExposedStep,
}

/// The resolved surface shape of one water cell, computed once before its faces are
/// emitted.
pub(super) struct WaterSurface {
    /// 2x2 corner heights, indexed `[cx][cz]`: the average surface height of the
    /// up-to-4 water cells meeting at each corner.
    corner_h: [[f32; 2]; 2],
    /// Top-face tile: the still tile, or the animated flow tile when the cell flows.
    top_tile: Tile,
    /// Quantized (8-bit) flow heading the shader rotates the flow tile by. 0 when still.
    top_angle: u32,
    /// Full-height water (capped from above, or a falling column): fills to the top,
    /// sides render full height rather than sloping.
    full: bool,
}

impl WaterSurface {
    /// Compute the surface shape for the water cell at world `(wx, wy, wz)`. `full`
    /// is the cell's `fills_cell` result (passed in since the caller already has the
    /// `water_fills_cell` lookup); `block_at`/`fluid_at` sample the world for the
    /// corner-height average and the flow gradient.
    pub(super) fn new<B, F, S>(
        wx: i32,
        wy: i32,
        wz: i32,
        full: bool,
        block_at: &B,
        fluid_at: &F,
        still_at: &S,
    ) -> Self
    where
        B: Fn(i32, i32, i32) -> Block,
        F: Fn(i32, i32, i32) -> Option<f32>,
        S: Fn(i32, i32, i32) -> bool,
    {
        // 2x2 corner heights, indexed [cx][cz]: average the up-to-4 water cells
        // meeting at each corner.
        let mut corner_h = [[1.0f32; 2]; 2];
        for cx in 0..2i32 {
            for cz in 0..2i32 {
                let mut sum = 0.0;
                let mut cnt = 0;
                for ox2 in (cx - 1)..=cx {
                    for oz2 in (cz - 1)..=cz {
                        if let Some(h) = fluid_at(wx + ox2, wy, wz + oz2) {
                            sum += h;
                            cnt += 1;
                        }
                    }
                }
                corner_h[cx as usize][cz as usize] = if cnt == 0 { 1.0 } else { sum / cnt as f32 };
            }
        }

        // Flow vector from the surface gradient: shared with entity physics so the
        // current push matches the texture heading.
        let mut top_tile = crate::atlas::engine().water_still;
        let mut top_angle = 0u32;
        let flow = crate::world::water::surface_flow_dir(wx, wy, wz, block_at, fluid_at, still_at);
        if flow.length_squared() > 0.0 {
            top_tile = crate::atlas::engine().water_flow;
            // Continuous flow heading: the shader rotates the flow tile by this
            // angle so a cell streaming into a corner points diagonally, not snapped
            // to a cardinal. atan2(x, z) keeps +Z=0/-X=-90/+X=+90/-Z=180 so the
            // cardinals match the texture's built-in down-flow. Quantized to 8 bits.
            let a = flow.x.atan2(flow.z);
            let frac = a / std::f32::consts::TAU + 0.5;
            top_angle = ((frac * 256.0) as i32).rem_euclid(256) as u32;
        }

        Self {
            corner_h,
            top_tile,
            top_angle,
            full,
        }
    }

    /// The top-face tile (still or flow).
    #[inline]
    pub(super) fn top_tile(&self) -> Tile {
        self.top_tile
    }

    /// The quantized flow heading carried in the top vertex's overlay bits.
    #[inline]
    pub(super) fn top_angle(&self) -> u32 {
        self.top_angle
    }

    /// Classify a water side face toward a neighbouring water cell. The face is kept
    /// (as the exposed step) only when this cell is full to the top while the
    /// neighbour's surface is recessed — otherwise the two surfaces meet and it culls.
    #[inline]
    pub(super) fn side_against_water(&self, is_side: bool, neighbour_full: bool) -> SideVsWater {
        if is_side && self.full && !neighbour_full {
            SideVsWater::ExposedStep
        } else {
            SideVsWater::Cull
        }
    }

    /// Warp a face's quad corners to the water surface in place. TOP verts go to
    /// their corner's surface height so the top slopes and every side's top edge
    /// meets it exactly (a full cell spans the whole block). Exposed-step faces
    /// additionally pull their BOTTOM verts up to the neighbour's surface (= the
    /// shared corner height), drawing only the band above the neighbour.
    pub(super) fn warp_quad(
        &self,
        corners: &mut [[f32; 3]; 4],
        base_x: f32,
        base_y: f32,
        base_z: f32,
        exposed_step: bool,
    ) {
        for p in corners.iter_mut() {
            let cx = ((p[0] - base_x) as usize).min(1);
            let cz = ((p[2] - base_z) as usize).min(1);
            if p[1] > base_y + 0.5 {
                p[1] = base_y
                    + if self.full {
                        1.0
                    } else {
                        self.corner_h[cx][cz]
                    };
            } else if exposed_step {
                p[1] = base_y + self.corner_h[cx][cz];
            }
        }
    }
}

/// The reverse-winding triangles for a water TOP face's `tris`. The transparent
/// pass is back-face culled, so the surface also needs the reverse winding to stay
/// visible from underneath when submerged. Side/bottom faces stay single-sided.
#[inline]
pub(super) fn top_back_winding(tris: [u32; 6]) -> [u32; 6] {
    [tris[0], tris[2], tris[1], tris[3], tris[5], tris[4]]
}
