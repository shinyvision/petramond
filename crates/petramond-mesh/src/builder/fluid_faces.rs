//! Face emission for one fluid cell: which faces survive the neighbourhood,
//! the surface-shaped quad each one draws, the medium lane it carries and the
//! stream it rides. Every per-fluid decision reads the fluid's own row.

use std::cell::OnceCell;

use glam::IVec3;
use petramond_world::block::Block;
use petramond_world::block_state::LogAxis;
use petramond_world::fluid::medium::medium_index;
use petramond_world::fluid_math;
use petramond_world::light::{BlockLight6, LightRgb};
use petramond_world::tile::TileTint;

use super::super::face::{quad_for, Face, FACES};
use super::super::face_emit::push_cube_face_with_cell_uvs;
use super::super::fluid::{side_vs_fluid, FluidSurface, SideVsFluid};
use super::super::tint;
use super::super::vertex::{
    pack_fluid_face, push_back_face, Vertex, FLUID_FLOW_FLAG2, FLUID_MEDIUM_MASK,
    FLUID_MEDIUM_SHIFT, UV_MODE_NONE,
};
use super::cube_face::{cube_face_tile, self_lit_face};
use super::pad::SectionMeshPad;

/// Per-cell fluid meta reads. A pad-backed mesh samples pad-locally, the
/// closure-backed mesher through its world closures; both answer the same.
pub(super) struct FluidProbe<'a, 'p, B, M> {
    pub pad: Option<&'a SectionMeshPad<'p>>,
    /// World coordinates of the section's local origin.
    pub origin: IVec3,
    pub block_at: &'a B,
    pub meta_at: &'a M,
}

impl<B, M> FluidProbe<'_, '_, B, M>
where
    B: Fn(i32, i32, i32) -> Block,
    M: Fn(i32, i32, i32) -> u8,
{
    #[inline]
    fn block(&self, p: IVec3) -> Block {
        (self.block_at)(p.x, p.y, p.z)
    }

    #[inline]
    fn meta(&self, p: IVec3) -> u8 {
        (self.meta_at)(p.x, p.y, p.z)
    }

    /// Whether `p` holds `fluid` filling its whole cell (`fluid_math::fills_cell`).
    #[inline]
    pub(super) fn fills(&self, p: IVec3, fluid: Block) -> bool {
        match self.pad {
            Some(pad) => {
                let l = p - self.origin;
                pad.fluid_fills_local(l.x, l.y, l.z, fluid)
            }
            None => {
                self.block(p).fluid() == Some(fluid)
                    && fluid_math::fills_cell(self.meta(p), self.block(p + IVec3::Y), fluid)
            }
        }
    }

    #[inline]
    fn still(&self, p: IVec3, fluid: Block) -> bool {
        match self.pad {
            Some(pad) => {
                let l = p - self.origin;
                pad.fluid_still_local(l.x, l.y, l.z, fluid)
            }
            None => {
                self.block(p).fluid() == Some(fluid) && fluid_math::is_still_source(self.meta(p))
            }
        }
    }

    #[inline]
    fn falling(&self, p: IVec3, fluid: Block) -> bool {
        match self.pad {
            Some(pad) => {
                let l = p - self.origin;
                pad.fluid_falling_local(l.x, l.y, l.z, fluid)
            }
            None => fluid_math::is_falling(self.meta(p)),
        }
    }

    #[inline]
    fn height(&self, p: IVec3, fluid: Block) -> Option<f32> {
        match self.pad {
            Some(pad) => {
                let l = p - self.origin;
                pad.fluid_height_local(l.x, l.y, l.z, fluid)
            }
            None => (self.block(p).fluid() == Some(fluid))
                .then(|| fluid_math::fluid_height(self.meta(p), self.block(p + IVec3::Y), fluid)),
        }
    }
}

/// Everything else a fluid cell's faces read of the section around them.
pub(super) struct FluidNeighbourhood<'a, 'p, B, M, N, C, F> {
    pub probe: &'a FluidProbe<'a, 'p, B, M>,
    pub loaded: &'a N,
    /// The cube path's cull: is a face toward this cell hidden by it?
    pub covered: &'a C,
    /// `(ao, sky light, block light)` per corner of a face lit from its front cell.
    pub light_face: &'a F,
}

/// The vertex streams a fluid face can land in.
pub(super) struct FluidStreams<'m> {
    pub opaque: &'m mut Vec<Vertex>,
    pub transparent: &'m mut Vec<Vertex>,
    pub transparent_two_sided: &'m mut Vec<Vertex>,
}

/// Emit every visible face of the `fluid` cell at `pos`. `resident` = the cell's
/// block IS the fluid and owns the cell's meta; otherwise the fluid is contained
/// in a host block and always renders a stationary surface. `cell_tint` is the
/// cell's `petramond:tint` multiply. Face positions are emitted relative to the
/// mesh-space origin `anchor`.
#[allow(clippy::too_many_arguments)]
pub(super) fn emit_fluid_cell<B, M, N, C, F>(
    nbh: &FluidNeighbourhood<'_, '_, B, M, N, C, F>,
    out: FluidStreams<'_>,
    fluid: Block,
    resident: bool,
    pos: IVec3,
    anchor: IVec3,
    tint_of: impl Fn(Option<TileTint>) -> [f32; 3],
    cell_tint: Option<[f32; 3]>,
) where
    B: Fn(i32, i32, i32) -> Block,
    M: Fn(i32, i32, i32) -> u8,
    N: Fn(i32, i32, i32) -> bool,
    C: Fn(IVec3, Face) -> bool,
    F: Fn(Face, IVec3) -> ([u32; 4], [u32; 4], [BlockLight6; 4]),
{
    let probe = nbh.probe;
    let def = fluid
        .fluid_def()
        .expect("a fluid-class block carries its fluid row");
    let medium = &def.medium;
    let lane_index = medium_index(fluid).expect("every fluid row has a medium index");
    // A contained fluid has no meta: it fills its cell only under more of itself.
    let fills = |p: IVec3| {
        if resident {
            probe.fills(p, fluid)
        } else {
            probe.block(p + IVec3::Y).fluid() == Some(fluid)
        }
    };
    let full = fills(pos);
    // A submerged cell draws nothing; ocean and lava-sea interiors are the
    // bulk of every fluid cell, so one test beats six culled faces.
    if resident
        && full
        && FACES.iter().all(|f| {
            let (dx, dy, dz) = f.dir();
            probe.fills(pos + IVec3::new(dx, dy, dz), fluid)
        })
    {
        return;
    }
    let opaque = medium.is_opaque();
    let self_lit = self_lit(fluid, medium.self_lit);
    // Sixteen corner-height samples plus a flow gradient: deferred to the
    // first face that survives culling.
    let surface_cell = OnceCell::new();
    let surface = || {
        surface_cell.get_or_init(|| {
            let block_at = |x, y, z| probe.block(IVec3::new(x, y, z));
            if !resident {
                return FluidSurface::stationary(
                    pos.to_array(),
                    fluid,
                    fluid.fluid_still_tile(),
                    block_at,
                );
            }
            FluidSurface::new(
                pos.x,
                pos.y,
                pos.z,
                fluid,
                full,
                probe.falling(pos, fluid),
                &block_at,
                &|x, y, z| probe.height(IVec3::new(x, y, z), fluid),
                &|x, y, z| probe.still(IVec3::new(x, y, z), fluid),
            )
        })
    };
    let FluidStreams {
        opaque: opaque_stream,
        transparent,
        transparent_two_sided,
    } = out;
    let base = (pos - anchor).as_vec3();

    for face in FACES {
        let (dx, dy, dz) = face.dir();
        let front = pos + IVec3::new(dx, dy, dz);
        let is_top = matches!(face, Face::PosY);
        let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
        // A covered top still draws when it cannot meet the cover's underside:
        // a see-through body writes no depth, and a recessed surface sits below
        // the lid. An opaque full-height top would z-fight the lid instead.
        if (nbh.covered)(front, face) && !(is_top && (!opaque || !full)) {
            continue;
        }
        if is_side && !(nbh.loaded)(front.x, front.y, front.z) {
            continue;
        }
        let nb = probe.block(front);
        if fluid.merges_with_self() && nb == fluid {
            continue;
        }
        let mut exposed_step = false;
        if nb.fluid() == Some(fluid) {
            match side_vs_fluid(full, is_side, fills(front)) {
                SideVsFluid::ExposedStep => exposed_step = true,
                SideVsFluid::Cull => continue,
            }
        }

        let (tile, flow_strip, tint) = if resident {
            let still = fluid.fluid_still_tile();
            let (tile, flow_strip) = match face {
                Face::PosY => (surface().top_tile(), surface().flows()),
                Face::NegY => (still, false),
                // A still source's sides are calm fluid: the step walls of the
                // recessed pocket under a block sitting in the sea must not stream.
                _ if probe.still(pos, fluid) => (still, false),
                _ => (fluid.fluid_flow_tile(), true),
            };
            (tile, flow_strip, tint_of(still.world_tint()))
        } else {
            let tile = cube_face_tile(fluid, face, fluid.tiles(), None, LogAxis::Y);
            (tile, false, tint::NO_TINT)
        };
        let tint = match cell_tint {
            Some(m) => [tint[0] * m[0], tint[1] * m[1], tint[2] * m[2]],
            None => tint,
        };
        let tile = tile.face_variation(pos.to_array(), face.normal_code());

        let mut corners = quad_for(face, base.x, base.y, base.z);
        surface().warp_quad(&mut corners, base.x, base.y, base.z, exposed_step);
        let top_angle = if is_top { surface().top_angle() } else { 0 };

        let (mut ao, light6, mut block6) = (nbh.light_face)(face, front);
        if let Some((emission, fraction)) = self_lit {
            self_lit_face(emission, fraction, &mut ao, &mut block6);
        }

        let stream = if opaque {
            &mut *opaque_stream
        } else if is_top {
            &mut *transparent_two_sided
        } else {
            &mut *transparent
        };
        let start = push_cube_face_with_cell_uvs(
            stream,
            corners,
            tile,
            top_angle,
            false,
            UV_MODE_NONE,
            None,
            0,
            tint,
            face,
            ao,
            light6,
            block6,
            cell_tint.is_some(),
        );
        let lane = pack_fluid_face(lane_index, flow_strip);
        for v in &mut stream[start as usize..start as usize + 4] {
            debug_assert_eq!(
                v.packed2 & ((FLUID_MEDIUM_MASK << FLUID_MEDIUM_SHIFT) | FLUID_FLOW_FLAG2),
                0,
                "another payload already occupies the fluid medium lane"
            );
            v.packed2 |= lane;
        }
        // The opaque stream culls back faces; the surface must read from inside.
        if opaque && is_top {
            push_back_face(stream, start);
        }
    }
}

/// The emission a fluid's faces light themselves with, and the fraction.
fn self_lit(fluid: Block, fraction: f32) -> Option<(BlockLight6, f32)> {
    let [r, g, b] = fluid.light_emission_rgb();
    (fraction > 0.0 && r | g | b != 0)
        .then(|| (BlockLight6::from_x2(LightRgb::new(r, g, b)), fraction))
}
