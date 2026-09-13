//! Fluid-surface GEOMETRY for the chunk mesher.
//!
//! Owns everything about how a fluid cell (water or lava — the same meta-driven
//! machine) is *shaped* into a mesh: its per-corner surface heights (so adjacent
//! cells join into one continuous sloped sheet), the top tile + flow rotation,
//! the per-vertex height warp, the exposed-step decision for a full cell standing
//! over a shorter neighbour, and the surface's reverse winding. The fluid MATH
//! (`fluid_height`, `fills_cell`, `surface_flow_dir`) stays in
//! `petramond_world::fluid_math`; this module only turns those values into
//! vertices.

use petramond_world::block::Block;
use petramond_world::tile::Tile;

/// Whether a fluid cell's side face toward a neighbouring cell of the SAME fluid
/// is culled or kept as the exposed vertical step (a full cell standing over a
/// shorter, open-surface neighbour — rendered as a band trimmed to the
/// neighbour's surface).
pub(super) enum SideVsFluid {
    /// Cull the face (the two fluid surfaces meet, nothing to draw).
    Cull,
    /// Keep it: render the exposed step, trimming its bottom to the neighbour.
    ExposedStep,
}

/// Classify a fluid side face toward a neighbouring same-fluid cell. The face is
/// kept (as the exposed step) only when this cell is full to the top while the
/// neighbour's surface is recessed — otherwise the two surfaces meet and it
/// culls.
///
/// Takes the cell's `fills_cell` answer rather than a whole [`FluidSurface`]:
/// this decides the CULL, and a submerged cell (the bulk of an ocean) culls
/// every face, so the surface resolve behind it — sixteen corner-height
/// samples plus a flow gradient — must not be paid to reach this answer.
#[inline]
pub(super) fn side_vs_fluid(full: bool, is_side: bool, neighbour_full: bool) -> SideVsFluid {
    if is_side && full && !neighbour_full {
        SideVsFluid::ExposedStep
    } else {
        SideVsFluid::Cull
    }
}

/// The resolved surface shape of one fluid cell, computed once before its faces
/// are emitted.
pub(super) struct FluidSurface {
    /// 2x2 corner heights, indexed `[cx][cz]`: the average surface height of the
    /// up-to-4 same-fluid cells meeting at each corner.
    corner_h: [[f32; 2]; 2],
    /// Top-face tile: the fluid's still tile, or its animated flow tile when the
    /// cell flows.
    top_tile: Tile,
    /// Quantized (8-bit) flow heading the shader rotates the flow tile by. 0 when still.
    top_angle: u32,
    /// Whether the top shows the flow strip (streaming or falling).
    flows: bool,
    /// Full-height fluid (capped from above, or a falling column): fills to the
    /// top, sides render full height rather than sloping.
    full: bool,
}

impl FluidSurface {
    /// Compute the surface shape for the fluid cell at world `(wx, wy, wz)`. `full`
    /// is the cell's `fills_cell` result and `falling` its FALLING meta bit (passed
    /// in since the caller already has the meta lookup); `block_at`/`fluid_at`
    /// sample the world for the corner-height average and the flow gradient;
    /// `block` names the fluid (its row owns the still/flow tiles).
    pub(super) fn new<B, F, S>(
        wx: i32,
        wy: i32,
        wz: i32,
        fluid: Block,
        full: bool,
        falling: bool,
        block_at: &B,
        fluid_at: &F,
        still_at: &S,
    ) -> Self
    where
        B: Fn(i32, i32, i32) -> Block,
        F: Fn(i32, i32, i32) -> Option<f32>,
        S: Fn(i32, i32, i32) -> bool,
    {
        // 2x2 corner heights, indexed [cx][cz]: average the up-to-4 same-fluid
        // cells meeting at each corner.
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
        let mut top_angle = 0u32;
        let flow = petramond_world::fluid_math::surface_flow_dir(
            wx, wy, wz, fluid, block_at, fluid_at, still_at,
        );
        let streams = flow.length_squared() > 0.0;
        if streams {
            // Continuous flow heading: the shader rotates the flow tile by this
            // angle so a cell streaming into a corner points diagonally, not snapped
            // to a cardinal. atan2(x, z) keeps +Z=0/-X=-90/+X=+90/-Z=180 so the
            // cardinals match the texture's built-in down-flow. Quantized to 8 bits.
            let a = flow.x.atan2(flow.z);
            let frac = a / std::f32::consts::TAU + 0.5;
            top_angle = ((frac * 256.0) as i32).rem_euclid(256) as u32;
        }
        // A FALLING stream is vertical flow whatever its horizontal gradient: a
        // column in open air has a symmetric neighbourhood (zero gradient), yet
        // its exposed top is streaming fluid, never a calm surface.
        let flows = streams || falling;
        let top_tile = if flows {
            fluid.fluid_flow_tile()
        } else {
            fluid.fluid_still_tile()
        };

        Self {
            corner_h,
            top_tile,
            top_angle,
            flows,
            full,
        }
    }

    pub(super) fn stationary(
        pos: [i32; 3],
        block: Block,
        tile: Tile,
        block_at: impl Fn(i32, i32, i32) -> Block,
    ) -> Self {
        let [x, y, z] = pos;
        let fluid = |x, y, z| {
            (block_at(x, y, z).fluid() == Some(block)).then(|| {
                if block_at(x, y + 1, z).fluid() == Some(block) {
                    1.0
                } else {
                    8.0 / 9.0
                }
            })
        };
        let mut surface = Self::new(
            x,
            y,
            z,
            block,
            block_at(x, y + 1, z).fluid() == Some(block),
            false,
            &block_at,
            &fluid,
            &|_, _, _| true,
        );
        surface.top_tile = tile;
        surface.top_angle = 0;
        surface.flows = false;
        surface
    }

    /// The top-face tile (the fluid's still or flow strip base).
    #[inline]
    pub(super) fn top_tile(&self) -> Tile {
        self.top_tile
    }

    /// Whether the top face shows the flow strip.
    #[inline]
    pub(super) fn flows(&self) -> bool {
        self.flows
    }

    /// The quantized flow heading carried in the top vertex's overlay bits.
    #[inline]
    pub(super) fn top_angle(&self) -> u32 {
        self.top_angle
    }

    /// Warp a face's quad corners to the fluid surface in place. TOP verts go to
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
