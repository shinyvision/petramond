//! The cube path: face culling for a cube-drawn cell, then ONE face emitter
//! shared by both culls — the exposure-mask fast path and the per-face
//! neighbourhood cull — that turns each visible face into either a deferred
//! greedy [`FlatFace`] or a pushed quad.

use std::cell::OnceCell;

use glam::{IVec3, Vec3};
use petramond_world::block::Block;
use petramond_world::block_state::LogAxis;
use petramond_world::chunk::{SECTION_SIZE, SECTION_VOLUME};
use petramond_world::light::BlockLight6;
use petramond_world::tile::Tile;

use super::super::boxset::cell_wears_snow;
use super::super::face::{quad_for, Face, FACES};
use super::super::face_emit::push_cube_face_with_cell_uvs;
use super::super::greedy::FlatFace;
use super::super::vertex::{transition::Transition, BlockLightVertexExt, UV_MODE_NONE};
use super::cube_face::{
    cube_face_tile, cube_face_uv_turn, face_axes, face_index, facing_face, log_side_cell_uvs,
    log_side_uvs_apply,
};
use super::exposed_masks::{mask_has, ExposedMasks};
use super::foliage::{self, CrownCorners};
use super::lighting::{boundary_plane, face_lighting};
use super::mesher::{Cell, SectionMesher};
use super::transition;

/// A cube cell's per-cell face setup, resolved once before its faces.
struct CubeCell {
    cell: Cell,
    tiles: [Tile; 3],
    /// Row-declared side treatment: `(base, overlay, overlay tint)`, or `None`
    /// for the plain side tile.
    side_style: Option<(Tile, Option<Tile>, [f32; 3])>,
    log_axis: LogAxis,
    /// A directional-front row's front face and tile.
    front: Option<(Face, Tile)>,
    /// The cell's minimum corner in mesh space.
    base: Vec3,
    /// Whether a flat face may defer to the greedy merge: an opaque cube, or a
    /// box family meshing as its full cube.
    mergeable: bool,
    /// The crown's corner shape of a canopy cell, resolved by the first face
    /// that survives culling.
    crown: OnceCell<CrownCorners>,
}

impl CubeCell {
    /// The row's per-slot UV turn for one face (mirrors `cube_face_tile`'s
    /// slot mapping); zero on the faces a horizontal log's explicit cell UVs
    /// already remap.
    fn uv_turn(&self, face: Face) -> u32 {
        if log_side_uvs_apply(self.log_axis, face) {
            0
        } else {
            cube_face_uv_turn(
                self.cell.block,
                face,
                self.front.map(|(f, _)| f),
                self.log_axis,
            )
        }
    }
}

#[inline]
fn is_side(face: Face) -> bool {
    matches!(face, Face::PosX | Face::NegX | Face::PosZ | Face::NegZ)
}

/// All four corners share AO and every light channel — the greedy merge
/// condition: a run of such faces collapses into one tiled quad,
/// pixel-identical.
#[inline]
fn is_flat(ao: [u32; 4], light6: [u32; 4], block6: [BlockLight6; 4]) -> bool {
    ao[0] == ao[1]
        && ao[1] == ao[2]
        && ao[2] == ao[3]
        && light6[0] == light6[1]
        && light6[1] == light6[2]
        && light6[2] == light6[3]
        && block6[0] == block6[1]
        && block6[1] == block6[2]
        && block6[2] == block6[3]
}

impl SectionMesher<'_> {
    /// Emit a cube-drawn cell. `masks` culls its faces from the exposure
    /// bitsets (fast-path candidates only); without them each face asks the
    /// neighbourhood whether its front cell covers it. `whole_stack` marks a
    /// box family meshing as its full cube.
    pub(super) fn emit_cube(
        &mut self,
        cell: &Cell,
        whole_stack: bool,
        masks: Option<&ExposedMasks>,
    ) {
        let cube = self.cube_cell(cell, whole_stack);
        for face in FACES {
            let (dx, dy, dz) = face.dir();
            let front = cell.world + IVec3::new(dx, dy, dz);
            let front_block = self.nb.block(front);
            let visible = match masks {
                Some(masks) => mask_has(masks, face, cell.idx),
                // A block that MERGES WITH ITSELF draws no interior face
                // against its own kind: a glass wall reads as one pane rather
                // than stacked frames, and an ice sheet as one volume rather
                // than double-blended slabs. Leaves opt out — their interior
                // faces are the canopy's depth near the camera, and are exactly
                // what the far LOD drops once mips read the cutouts as a dense
                // canopy.
                None => {
                    !self.nb.covers_face(front, face)
                        && !(cell.block.merges_with_self() && front_block == cell.block)
                }
            };
            if visible {
                self.emit_cube_face(&cube, face, front, front_block);
            }
        }
    }

    fn cube_cell(&self, cell: &Cell, whole_stack: bool) -> CubeCell {
        let block = cell.block;
        // Row-declared side treatments, resolved once per cell — the mesher
        // reads row fields, never concrete block ids. A `covered_side` row
        // (grass) swaps its sides to that tile while a snow-cover block sits
        // directly on top — derived from the neighbour above at mesh time, so
        // it heals itself the moment the cover is placed or dug. Otherwise a
        // `side_overlay` row composites its base under the biome-tinted
        // overlay (dirt + grass overlay).
        let side_style = match block
            .covered_side()
            .filter(|_| cell_wears_snow(&self.nb, cell.world + IVec3::Y))
        {
            Some(t) => Some((t, None, self.tints.tile(t.world_tint(), cell.column))),
            None => block.side_overlay().map(|so| {
                (
                    so.base,
                    Some(so.overlay),
                    self.tints.tile(so.overlay.world_tint(), cell.column),
                )
            }),
        };
        let log_axis = if block.is_log() {
            self.section.log_axis(cell.lx, cell.ly, cell.lz)
        } else {
            LogAxis::Y
        };
        // A directional-front row (furnace, lit furnace) draws its `front`
        // tile on the face its stored entity facing points to; the other sides
        // keep the plain side tile. The lit furnace is its own block row, so
        // "lit" is just this row read.
        let front = block.front_tile().map(|front| {
            (
                facing_face(self.section.entity_facing(cell.lx, cell.ly, cell.lz)),
                front,
            )
        });
        CubeCell {
            cell: *cell,
            tiles: block.tiles(),
            side_style,
            log_axis,
            front,
            base: (cell.world - self.anchor).as_vec3(),
            mergeable: block.is_opaque() || whole_stack,
            crown: OnceCell::new(),
        }
    }

    /// The transition plan for one cube face, reading the neighbourhood.
    fn plan_transition(&self, pos: IVec3, face: Face, block: u16) -> Option<Transition> {
        let nb = &self.nb;
        transition::Context {
            rules: self.rules,
            block: &|x, y, z| nb.block(IVec3::new(x, y, z)),
            known: &|x, y, z| nb.loaded(IVec3::new(x, y, z)),
            blocked: &|x, y, z| nb.transition_blocked(IVec3::new(x, y, z)),
            covered: &|p, f| nb.covers_face(p, f),
        }
        .plan(pos, face, block)
    }

    /// One visible cube face: tile, tints, lighting and transition, then
    /// either a deferred greedy [`FlatFace`] (a plain opaque face whose four
    /// corners are flat) or a quad pushed into the stream the block rides.
    fn emit_cube_face(
        &mut self,
        cube: &CubeCell,
        face: Face,
        front: IVec3,
        front_block: Block,
    ) {
        let cell = &cube.cell;
        let block = cell.block;
        let (base_tile, overlay_tile, tint) = match cube.side_style {
            Some(style) if is_side(face) => style,
            _ => {
                let t = cube_face_tile(block, face, cube.tiles, cube.front, cube.log_axis);
                (t, None, self.tints.tile(t.world_tint(), cell.column))
            }
        };
        let tint = self.tints.cube(cell.idx, tint);
        let base_tile = base_tile.face_variation(cell.world.to_array(), face.normal_code());
        let (ao, light6, block6) = face_lighting(&self.nb, face, front, boundary_plane(face), true);
        let transition = self.plan_transition(cell.world, face, block.id());
        let dyed = self.tints.tinted(cell.idx);
        let uv_turn = cube.uv_turn(face);

        // Asked corner-free: a face bound for the greedy merge never builds
        // its quad at all (the merged quad rebuilds one for the whole run).
        if transition.is_none()
            && overlay_tile.is_none()
            && cube.mergeable
            && is_flat(ao, light6, block6)
            && !log_side_uvs_apply(cube.log_axis, face)
        {
            let fi = face_index(face);
            self.greedy.faces[fi * SECTION_VOLUME + cell.idx] = FlatFace {
                gen: self.greedy_gen,
                // UV turn in bits 12..13, dyed flag in bit 31 (both part of
                // the merge key).
                tile: base_tile.index() as u32 | (uv_turn << 12) | ((dyed as u32) << 31),
                shade: FlatFace::shade(ao[0], light6[0], block6[0]),
                tint: block6[0].tint_word(tint),
            };
            // Slice index = the cell's coord along this face's normal axis.
            let s = [cell.lx, cell.ly, cell.lz][face_axes(face).0];
            self.greedy.slice_counts[fi * SECTION_SIZE + s] += 1;
            return;
        }

        let base = cube.base.to_array();
        let corners = quad_for(face, base[0], base[1], base[2]);
        let log_uvs = log_side_cell_uvs(cube.log_axis, face, corners, base);
        let (overlay, has_overlay) = match overlay_tile {
            Some(o) => (o.index() as u32, true),
            None => (0, false),
        };
        // Translucent blocks (ice) blend in their own depth-writing pass;
        // their texels sit below the opaque pass's cutout and would discard to
        // nothing there, and the fluid pass draws after them. A leaf face
        // against the SAME leaves is what the far LOD drops.
        let vbuf = if block.is_translucent() {
            &mut self.out.translucent
        } else if block.is_leaves() && front_block == block {
            &mut self.out.leaf_interior
        } else {
            &mut self.out.opaque
        };
        let start = push_cube_face_with_cell_uvs(
            vbuf,
            corners,
            base_tile,
            overlay,
            has_overlay,
            UV_MODE_NONE,
            log_uvs,
            uv_turn,
            tint,
            face,
            ao,
            light6,
            block6,
            dyed,
        );
        // A transition recolours the face; a set may add a biome tint.
        if let Some(plan) = transition {
            let set = &self.rules.sets[plan.set as usize];
            plan.apply(
                &mut vbuf[start as usize..start as usize + 4],
                self.tints.tile(set.tint, cell.column),
            );
        }
        // Canopy dressing is decided per cell; the crown's corner shape is
        // resolved lazily by the first face that survives culling.
        if block.is_canopy() {
            let nb = &self.nb;
            let crown = cube.crown.get_or_init(|| {
                CrownCorners::new(
                    cell.world.to_array(),
                    cube.base,
                    |x, y, z| nb.block(IVec3::new(x, y, z)),
                    |x, y, z| nb.loaded(IVec3::new(x, y, z)),
                )
            });
            let faces_air = front_block == Block::Air && nb.loaded(front);
            foliage::dress_face(vbuf, start, face, cell.world.to_array(), faces_air, crown);
        }
    }
}
