use glam::IVec3;
use petramond_world::block::Block;
use petramond_world::tile::Tile;

pub(super) enum SideVsFluid {
    Cull,
    ExposedStep,
}

#[inline]
pub(super) fn side_vs_fluid(full: bool, is_side: bool, neighbour_full: bool) -> SideVsFluid {
    if is_side && full && !neighbour_full {
        SideVsFluid::ExposedStep
    } else {
        SideVsFluid::Cull
    }
}

pub(super) struct FluidReads<'r> {
    pub(super) block_at: &'r dyn Fn(i32, i32, i32) -> Block,
    pub(super) height_at: &'r dyn Fn(i32, i32, i32) -> Option<f32>,
    pub(super) still_at: &'r dyn Fn(i32, i32, i32) -> bool,
}

pub(super) struct FluidSurface {
    corner_h: [[f32; 2]; 2],
    top_tile: Tile,
    top_angle: u32,
    flows: bool,
    full: bool,
}

impl FluidSurface {
    pub(super) fn new(
        pos: IVec3,
        fluid: Block,
        full: bool,
        falling: bool,
        reads: &FluidReads<'_>,
    ) -> Self {
        let IVec3 {
            x: wx,
            y: wy,
            z: wz,
        } = pos;
        let fluid_at = reads.height_at;
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

        let mut top_angle = 0u32;
        let flow = petramond_world::fluid_math::surface_flow_dir(
            wx,
            wy,
            wz,
            fluid,
            &reads.block_at,
            &reads.height_at,
            &reads.still_at,
        );
        let streams = flow.length_squared() > 0.0;
        if streams {
            let a = flow.x.atan2(flow.z);
            let frac = a / std::f32::consts::TAU + 0.5;
            top_angle = ((frac * 256.0) as i32).rem_euclid(256) as u32;
        }
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
        let reads = FluidReads {
            block_at: &block_at,
            height_at: &fluid,
            still_at: &|_, _, _| true,
        };
        let mut surface = Self::new(
            IVec3::new(x, y, z),
            block,
            block_at(x, y + 1, z).fluid() == Some(block),
            false,
            &reads,
        );
        surface.top_tile = tile;
        surface.top_angle = 0;
        surface.flows = false;
        surface
    }

    #[inline]
    pub(super) fn top_tile(&self) -> Tile {
        self.top_tile
    }

    #[inline]
    pub(super) fn flows(&self) -> bool {
        self.flows
    }

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
