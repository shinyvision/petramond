//! Face emission for one fluid cell: which faces survive the neighbourhood,
//! the surface-shaped quad each one draws, the medium lane it carries and the
//! stream it rides. Every per-fluid decision reads the fluid's own row.

use std::cell::OnceCell;

use glam::IVec3;
use petramond_world::block::Block;
use petramond_world::block_state::LogAxis;
use petramond_world::fluid::medium::medium_index;
use petramond_world::light::{BlockLight6, LightRgb};
use petramond_world::tile::TileTint;

use super::super::face::{quad_for, Face, FaceShading, FACES};
use super::super::face_emit::{push_cube_face, FaceSpec};
use super::super::fluid::{side_vs_fluid, FluidReads, FluidSurface, SideVsFluid};
use super::super::tint;
use super::super::vertex::{
    pack_fluid_face, push_back_face, Vertex, FLUID_FLOW_FLAG2, FLUID_MEDIUM_MASK,
    FLUID_MEDIUM_SHIFT,
};
use super::cube_face::cube_face_tile;
use super::lighting::{boundary_plane, face_lighting, self_lit_face};
use super::mesher::Cell;
use super::neighbourhood::Neighbourhood;

/// The vertex streams a fluid face can land in.
pub(super) struct FluidStreams<'m> {
    pub opaque: &'m mut Vec<Vertex>,
    pub transparent: &'m mut Vec<Vertex>,
    pub transparent_two_sided: &'m mut Vec<Vertex>,
}

/// Emit every visible face of the fluid `cell` (its block is the fluid). When
/// the cell is `resident` the fluid owns the cell's meta; otherwise the fluid is
/// contained in a host block and always renders a stationary surface. `cell_tint` is the
/// cell's `petramond:tint` multiply. Face positions are emitted relative to the
/// mesh-space origin `anchor`.
pub(super) fn emit_fluid_cell(
    nb: &Neighbourhood<'_>,
    out: FluidStreams<'_>,
    cell: &Cell,
    anchor: IVec3,
    tint_of: impl Fn(Option<TileTint>) -> [f32; 3],
    cell_tint: Option<[f32; 3]>,
) {
    let (fluid, resident, pos) = (cell.block, cell.resident, cell.world);
    let def = fluid
        .fluid_def()
        .expect("a fluid-class block carries its fluid row");
    let medium = &def.medium;
    let lane_index = medium_index(fluid).expect("every fluid row has a medium index");
    // A contained fluid has no meta: it fills its cell only under more of itself.
    let fills = |p: IVec3| {
        if resident {
            nb.fluid_fills(p, fluid)
        } else {
            nb.block(p + IVec3::Y).fluid() == Some(fluid)
        }
    };
    let full = fills(pos);
    // A submerged cell draws nothing; ocean and lava-sea interiors are the
    // bulk of every fluid cell, so one test beats six culled faces.
    if resident && full && FACES.iter().all(|f| nb.fluid_fills(pos + f.dir(), fluid)) {
        return;
    }
    let opaque = medium.is_opaque();
    let self_lit = self_lit(fluid, medium.self_lit);
    // Sixteen corner-height samples plus a flow gradient: deferred to the
    // first face that survives culling.
    let surface_cell = OnceCell::new();
    let surface = || {
        surface_cell.get_or_init(|| {
            let block_at = |x, y, z| nb.block(IVec3::new(x, y, z));
            if !resident {
                return FluidSurface::stationary(
                    pos.to_array(),
                    fluid,
                    fluid.fluid_still_tile(),
                    block_at,
                );
            }
            FluidSurface::new(
                pos,
                fluid,
                full,
                nb.fluid_falling(pos, fluid),
                &FluidReads {
                    block_at: &block_at,
                    height_at: &|x, y, z| nb.fluid_height(IVec3::new(x, y, z), fluid),
                    still_at: &|x, y, z| nb.fluid_still(IVec3::new(x, y, z), fluid),
                },
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
        let front = pos + face.dir();
        let is_top = matches!(face, Face::PosY);
        let is_side = matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ);
        // A covered top still draws when it cannot meet the cover's underside:
        // a see-through body writes no depth, and a recessed surface sits below
        // the lid. An opaque full-height top would z-fight the lid instead.
        if nb.covers_face(front, face) && !(is_top && (!opaque || !full)) {
            continue;
        }
        if is_side && !nb.loaded(front) {
            continue;
        }
        let front_block = nb.block(front);
        if fluid.merges_with_self() && front_block == fluid {
            continue;
        }
        let mut exposed_step = false;
        if front_block.fluid() == Some(fluid) {
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
                _ if nb.fluid_still(pos, fluid) => (still, false),
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

        let (mut ao, light6, mut block6) =
            face_lighting(nb, face, front, boundary_plane(face), true);
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
        let start = push_cube_face(
            stream,
            &FaceSpec {
                face,
                corners,
                base_tile: tile,
                overlay: top_angle,
                has_overlay: false,
                cell_uvs: None,
                uv_turn: 0,
                tint,
                light: (ao, light6, block6),
                dyed: cell_tint.is_some(),
            },
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
