//! Geometry helpers that build small meshes in the 32-byte [`mesh::Vertex`]
//! format for the held-item hand, dropped item-entities, and the isometric
//! inventory icons.
//!
//! These all reuse the block atlas + `tile_uv()` table the chunk mesher uses, so
//! a held log / dropped log / log icon are textured identically to the world
//! block. The default helpers draw full-bright for inventory icons; the `_lit`
//! variants pack sampled world skylight for hand/items while keeping AO = 3.
//!
//! ## Packing conventions (shared with `block.wgsl`'s `packed` layout)
//! The vertex packs word 1 as
//! `0..8 tile | 8..10 corner | 10..12 shade | 12..20 overlay | 20 flag | 21..23 AO | 23..29 SKYlight | 29..32 UV mode`
//! and word 2 (`packed2`) as `0..6 block light | 6..16 cell-local uv | rest
//! reserved`. For the textured path ([`cube_textured`], [`billboard_quad`]) we set
//! the tile, corner, shade, AO = 3, skylight = 63.
//!
//! ### Out-of-world foliage tint + grass-side overlay
//! Icons / held items / dropped cubes have no biome context, so foliage greens
//! using a single fixed temperate colour from [`foliage_tint`]. Each cube face is
//! classified exactly like the chunk mesher: grass-top / short-grass / fern get
//! the grass tint; all leaves get the foliage tint; grass-block SIDES render as a
//! dirt base plus the tinted grayscale `GrassSideOverlay` — its tile is packed in
//! bits 12..20 with the has-overlay flag at **bit 20** (the same overlay-composite
//! path the chunk mesher uses, which `model3d.wgsl` mirrors). Note bit 20 is
//! overloaded: in the textured path it means "has grass-side overlay"; the
//! solid-color path ([`cube_solid`]) reuses the same bit for [`SOLID_COLOR_FLAG`].
//! The two never collide because a solid cuboid carries no tile/overlay and the
//! shader reads the flag only on the appropriate branch.
//!
//! ### Solid-color sentinel ([`SOLID_COLOR_FLAG`])
//! The skin hand has no texture. [`cube_solid`] packs the RGB tint into the
//! `tint` field (as every textured vertex already carries a tint) and sets the
//! reserved flag at **bit 20** (the chunk mesher's "has-overlay" bit, which has
//! no meaning in the model3d pipeline). The STEP 2 model3d fragment shader reads
//! this bit: when set it outputs the interpolated `tint` directly (solid color,
//! atlas ignored); when clear it samples the atlas at the reconstructed uv. Keep
//! this convention identical between this module and `model3d.wgsl`.

use super::foliage_tint::{self, FaceMaterial};
use super::lighting::{self, DynLight};
use crate::atlas::Tile;
use crate::block::Block;
use crate::block_state::{HeldBlockState, LogAxis, SlabState, StairState};
use crate::mesh::face::Face;
use crate::mesh::{pack_cell_uv, Vertex, UV_MODE_CELL_LOCAL, UV_MODE_SHIFT};

use glam::Vec3;

/// Bit 20 of `Vertex::packed`: when set, the model3d fragment shader treats the
/// vertex's `tint` as the final solid color and ignores the atlas. Mirrors the
/// chunk-mesher "has-overlay" bit position, which is unused by the model3d pass.
pub const SOLID_COLOR_FLAG: u32 = 1 << 20;

/// Max AO (no occlusion) packed into bits 21..23.
const FULL_AO: u32 = 3 << 21;

/// The six cube faces (`PosX, NegX, PosY, NegY, PosZ, NegZ`). Dynamic geometry
/// shares the chunk mesher's [`mesh::face::Face`](crate::mesh::face::Face): its
/// `shade_idx` bakes the same "top bright, bottom dark" directional shading and
/// its `quad_box` winds corners identically, so a held / dropped / icon cube is
/// byte-identical to the world block. `quad_box(min, max)` also spans non-cube
/// boxes (the chest's inset body and lid).
const ALL_FACES: [Face; 6] = Face::ALL;

#[inline]
fn face_bits_textured_lit(mat: FaceMaterial, face: Face, skylight: u8) -> u32 {
    let (ov_tile, ov_flag) = match mat.overlay_tile {
        Some(o) => (o.index() as u32, 1u32),
        None => (0, 0),
    };
    (mat.base_tile.index() as u32)
        | (face.shade_idx() << 10)
        | (ov_tile << 12)
        | (ov_flag << 20)
        | FULL_AO
        | lighting::skylight_bits(skylight)
}

#[inline]
fn face_bits_solid_lit(face: Face, skylight: u8) -> u32 {
    (face.shade_idx() << 10) | FULL_AO | lighting::skylight_bits(skylight) | SOLID_COLOR_FLAG
}

/// Append a textured quad (4 verts, 6 indices) to `verts`/`indices`. `packed2`
/// carries the second vertex word (block light in bits 0..6).
#[inline]
fn push_quad(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    tint: [f32; 3],
    base_bits: u32,
    packed2: u32,
) {
    let start = verts.len() as u32;
    for (corner, pos) in corners.into_iter().enumerate() {
        verts.push(Vertex {
            pos,
            tint,
            packed: base_bits | ((corner as u32) << 8),
            packed2,
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

#[inline]
fn uv_16ths(value: f32) -> u32 {
    (value.clamp(0.0, 1.0) * 16.0).round() as u32
}

#[inline]
fn log_side_cell_uvs(axis: LogAxis, face: Face) -> Option<[(u32, u32); 4]> {
    let mut uvs = [(0, 0); 4];
    for (i, local) in face
        .quad_box([0.0, 0.0, 0.0], [1.0, 1.0, 1.0])
        .into_iter()
        .enumerate()
    {
        let [u, v] = face.log_side_cell_uv(axis, local)?;
        uvs[i] = (uv_16ths(u), uv_16ths(v));
    }
    Some(uvs)
}

#[inline]
fn push_quad_cell_uvs(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    cell_uvs: [(u32, u32); 4],
    tint: [f32; 3],
    base_bits: u32,
    packed2: u32,
) {
    let start = verts.len() as u32;
    for (corner, pos) in corners.into_iter().enumerate() {
        let (u, v) = cell_uvs[corner];
        verts.push(Vertex {
            pos,
            tint,
            packed: base_bits | ((corner as u32) << 8),
            packed2: packed2 | pack_cell_uv(u, v),
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

/// Append a full-bright textured cube spanning `[origin, origin + size]`, per-face
/// tiles `[top, bottom, side]` (matching `Block::tiles()`), into the caller-owned
/// `verts`/`indices` (capacity reused, nothing cleared). Indices are re-based onto
/// the running vertex count so this composes with prior geometry. 24 verts / 36
/// indices, back-face culled (CCW front faces).
///
/// Each face is foliage-tinted out-of-world via [`foliage_tint::face_material`],
/// mirroring the chunk mesher: a grass block tints its top green and renders its
/// sides as dirt + a tinted grass-side overlay, leaves tint with the foliage
/// colour, and everything else stays untinted.
pub fn push_cube_textured(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tiles: [Tile; 3],
    origin: Vec3,
    size: f32,
) {
    push_cube_textured_lit(verts, indices, tiles, origin, size, DynLight::FULL);
}

pub(super) fn push_cube_textured_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tiles: [Tile; 3],
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    push_cube_faces_lit(verts, indices, expand_tiles(tiles), origin, size, light);
}

/// Expand the `[top, bottom, side]` model into the 6 per-face tiles in `ALL_FACES`
/// order, with `side` on every horizontal face. The inverse mapping of
/// [`push_cube_textured_lit`].
#[inline]
fn expand_tiles(tiles: [Tile; 3]) -> [Tile; 6] {
    let [top, bottom, side] = tiles;
    // ALL_FACES: PosX, NegX, PosY, NegY, PosZ, NegZ.
    [side, side, top, bottom, side, side]
}

#[inline]
fn expand_log_tiles(tiles: [Tile; 3], axis: LogAxis) -> [Tile; 6] {
    let [top, bottom, side] = tiles;
    match axis {
        LogAxis::X => [top, bottom, side, side, side, side],
        LogAxis::Y => [side, side, top, bottom, side, side],
        LogAxis::Z => [side, side, side, side, top, bottom],
    }
}

/// The 6 per-face tiles (`ALL_FACES` order) for drawing `block` as an inventory /
/// held / dropped-item cube. Most blocks just expand their `[top, bottom, side]`
/// model; the furnace puts its front on a single visible face so the item reads as
/// a furnace instead of four mouths (the placed block is meshed directionally).
#[cfg(test)]
fn block_icon_faces(block: Block) -> [Tile; 6] {
    block_icon_faces_with_state(block, HeldBlockState::None)
}

pub(super) fn block_icon_faces_with_state(block: Block, state: HeldBlockState) -> [Tile; 6] {
    let mut faces = expand_tiles(block.tiles());
    if block.is_log() {
        let axis = match state {
            HeldBlockState::Log(axis) => axis,
            _ => LogAxis::Y,
        };
        faces = expand_log_tiles(block.tiles(), axis);
    }
    // Index 4 = PosZ, one of the two side faces the isometric icon presents, so a
    // directional block shows its front there instead of repeating the side art.
    match block {
        Block::Furnace => faces[4] = crate::atlas::engine().furnace_front,
        Block::Chest => faces[4] = crate::atlas::engine().chest_front,
        _ => {}
    }
    faces
}

pub(super) fn push_cube_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
    push_box_faces_lit(verts, indices, faces, origin, max, light);
}

/// Append a textured box spanning `[min, max]` with explicit per-face tiles
/// (`ALL_FACES` order: PosX, NegX, PosY, NegY, PosZ, NegZ), lit by `skylight`. Like
/// [`push_cube_faces_lit`] but for an arbitrary (non-cube) box — used to build the
/// chest's inset body and hinged lid. 24 verts / 36 indices, back-face culled.
pub(super) fn push_box_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    min: Vec3,
    max: Vec3,
    light: DynLight,
) {
    for (tile, face) in faces.into_iter().zip(ALL_FACES) {
        let mat = foliage_tint::face_material(tile);
        push_quad(
            verts,
            indices,
            face.quad_box(min.to_array(), max.to_array()),
            mat.tint,
            face_bits_textured_lit(mat, face, light.sky),
            lighting::blocklight_word(light.block),
        );
    }
}

fn push_log_cube_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    axis: LogAxis,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
    for (tile, face) in faces.into_iter().zip(ALL_FACES) {
        let mat = foliage_tint::face_material(tile);
        let corners = face.quad_box(origin.to_array(), max.to_array());
        let word2 = lighting::blocklight_word(light.block);
        if let Some(cell_uvs) = log_side_cell_uvs(axis, face) {
            push_quad_cell_uvs(
                verts,
                indices,
                corners,
                cell_uvs,
                mat.tint,
                face_bits_textured_lit(mat, face, light.sky)
                    | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
                word2,
            );
        } else {
            push_quad(
                verts,
                indices,
                corners,
                mat.tint,
                face_bits_textured_lit(mat, face, light.sky),
                word2,
            );
        }
    }
}

/// Packed bit shift for the UV mode field (bits 29..32, above the 6-bit skylight
/// that tops out at bit 28). Dynamic thin geometry uses 1 = crop U and 2 = crop V;
/// chunk-meshed stairs use the remaining modes for cell-local side UVs.
pub(super) const UV_SLICE_SHIFT: u32 = UV_MODE_SHIFT;

/// As [`push_box_faces_lit`] but, per face (`ALL_FACES` order):
/// - MIRRORS the texture horizontally where `mirror_u` is set — used by the door so
///   its BACK face is the mirror image of its front (hinge/handle stay on the same
///   physical side from either side). Mirroring is pure UV: the quad's corner indices
///   are swapped left↔right (`[1,0,3,2]`), flipping `u`, no geometry/winding change.
/// - applies a thin-face UV-SLICE mode from `slice_mode` (0 none, 1 crop-U, 2 crop-V),
///   packed into bits 29..32 ([`UV_SLICE_SHIFT`]) so the shader crops a 3/16-deep face
///   to a matching strip of its tile instead of squishing the whole tile flat — used
///   by the door's thin side (crop-U) and top/bottom edge (crop-V) faces.
pub(super) fn push_box_faces_lit_mirrored(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    min: Vec3,
    max: Vec3,
    light: DynLight,
    mirror_u: [bool; 6],
    slice_mode: [u32; 6],
) {
    for (((tile, face), mir), slice) in faces
        .into_iter()
        .zip(ALL_FACES)
        .zip(mirror_u)
        .zip(slice_mode)
    {
        let mat = foliage_tint::face_material(tile);
        let corners = face.quad_box(min.to_array(), max.to_array());
        let bits = face_bits_textured_lit(mat, face, light.sky) | (slice << UV_SLICE_SHIFT);
        let word2 = lighting::blocklight_word(light.block);
        if mir {
            push_quad_uflip(verts, indices, corners, mat.tint, bits, word2);
        } else {
            push_quad(verts, indices, corners, mat.tint, bits, word2);
        }
    }
}

/// [`push_quad`] with the texture mirrored horizontally: each geometric corner is given
/// the UV of its left↔right partner (`corner_uv` maps 0/1/2/3 to bl/br/tr/tl, so the
/// swap `[1,0,3,2]` flips `u`). Geometry + winding are unchanged.
#[inline]
fn push_quad_uflip(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    tint: [f32; 3],
    base_bits: u32,
    packed2: u32,
) {
    const MIRROR: [u32; 4] = [1, 0, 3, 2];
    let start = verts.len() as u32;
    for (i, pos) in corners.into_iter().enumerate() {
        verts.push(Vertex {
            pos,
            tint,
            packed: base_bits | (MIRROR[i] << 8),
            packed2,
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

/// As [`push_box_faces_lit`] but recessing the four side faces 1/16 inward (via
/// [`cactus_quad`](crate::mesh::face::cactus_quad)) so the box reads as a cactus —
/// the icon / held / dropped counterpart of the chunk mesher's inset cactus.
fn push_cactus_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    min: Vec3,
    max: Vec3,
    light: DynLight,
) {
    for (tile, face) in faces.into_iter().zip(ALL_FACES) {
        let mat = foliage_tint::face_material(tile);
        push_quad(
            verts,
            indices,
            crate::mesh::face::cactus_quad(face, min.to_array(), max.to_array()),
            mat.tint,
            face_bits_textured_lit(mat, face, light.sky),
            lighting::blocklight_word(light.block),
        );
    }
}

/// Append `block` as an inventory / held / dropped cube into `[origin, origin+size]`,
/// full-bright. The single entry point so every place a block is drawn as a small cube
/// shares the cactus special-case (its recessed spiny sides); see the `_lit` variant.
pub(super) fn push_block_item_cube(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    origin: Vec3,
    size: f32,
) {
    push_block_item_cube_lit(verts, indices, block, origin, size, DynLight::FULL);
}

/// As [`push_block_item_cube`] but lit by `skylight` (a held item / dropped stack samples
/// world light). Per-face tiles come from [`block_icon_faces`] (so a furnace shows its
/// front); the cactus draws via [`push_cactus_faces_lit`] so its inset sides match the
/// placed block, every other block is a plain cube.
pub(super) fn push_block_item_cube_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    push_block_item_cube_lit_with_state(
        verts,
        indices,
        block,
        HeldBlockState::None,
        origin,
        size,
        light,
    );
}

pub(super) fn push_block_item_cube_lit_with_state(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    state: HeldBlockState,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let faces = block_icon_faces_with_state(block, state);
    if block.render_shape() == crate::block::RenderShape::Stair {
        let stair = match state {
            HeldBlockState::Stair(state) => state,
            _ => StairState::new(crate::facing::Facing::South, Default::default()),
        };
        push_stair_item_lit(verts, indices, faces, stair, origin, size, light);
    } else if block.render_shape() == crate::block::RenderShape::Slab {
        let slab = match state {
            HeldBlockState::Slab(state) => crate::slab::normalize_state(block, state),
            _ => crate::slab::default_state(block),
        };
        push_slab_item_lit(verts, indices, slab, origin, size, light);
    } else if block == Block::Cactus {
        let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
        push_cactus_faces_lit(verts, indices, faces, origin, max, light);
    } else if block.is_log() {
        let axis = match state {
            HeldBlockState::Log(axis) => axis,
            _ => LogAxis::Y,
        };
        push_log_cube_faces_lit(verts, indices, faces, axis, origin, size, light);
    } else {
        push_cube_faces_lit(verts, indices, faces, origin, size, light);
    }
}

fn push_slab_item_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    state: SlabState,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    for (slot, block) in crate::slab::layer_slots(state) {
        let faces = block_icon_faces_with_state(block, HeldBlockState::None);
        for face in Face::ALL {
            let (quads, n) = crate::mesh::slab::layer_quads(state, slot, face);
            for &(min, max) in quads.iter().take(n) {
                push_cell_local_face(
                    verts,
                    indices,
                    faces[face as usize],
                    origin,
                    size,
                    min,
                    max,
                    face,
                    light,
                );
            }
        }
    }
}

fn push_stair_item_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    state: StairState,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let shape = crate::stair::shape(state);
    for face in Face::ALL {
        for outer in [true, false] {
            let (quads, n) = crate::mesh::stair::plane_quads(shape, face, outer);
            for &(min, max) in quads.iter().take(n) {
                push_cell_local_face(
                    verts,
                    indices,
                    faces[face as usize],
                    origin,
                    size,
                    min,
                    max,
                    face,
                    light,
                );
            }
        }
    }
}

/// One `face` of the cell-local box `[min, max]` scaled into `[origin, origin +
/// size]`: like [`push_quad`] but with cell-local UVs ([`UV_MODE_CELL_LOCAL`]),
/// so a partial face samples the matching sub-rectangle of its tile. Shared by
/// the stair item cube (hand / drop / icon) and the stair break-crack overlay,
/// so a stair reads as a cut-out full block everywhere it is drawn.
#[allow(clippy::too_many_arguments)]
pub(super) fn push_cell_local_face(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tile: Tile,
    origin: Vec3,
    size: f32,
    min: [f32; 3],
    max: [f32; 3],
    face: Face,
    light: DynLight,
) {
    let mn = origin + Vec3::new(min[0], min[1], min[2]) * size;
    let mx = origin + Vec3::new(max[0], max[1], max[2]) * size;
    let mat = foliage_tint::face_material(tile);
    let bits = face_bits_textured_lit(mat, face, light.sky) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT);
    let word2 = lighting::blocklight_word(light.block);
    let corners = face.quad_box(mn.to_array(), mx.to_array());
    let local = face.quad_box(min, max);
    let start = verts.len() as u32;
    for (corner, pos) in corners.into_iter().enumerate() {
        let [u, v] = crate::mesh::plane::cell_uv(face, local[corner]);
        verts.push(Vertex {
            pos,
            tint: mat.tint,
            packed: bits | ((corner as u32) << 8),
            packed2: word2 | pack_cell_uv((u * 16.0).round() as u32, (v * 16.0).round() as u32),
        });
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

/// A full-bright textured cube spanning `[origin, origin + size]`, per-face tiles
/// `[top, bottom, side]` (matching `Block::tiles()`). 24 verts / 36 indices, back-face
/// culled (CCW front faces).
///
/// Test-only convenience: live render bakes into caller buffers via the append-style
/// [`push_cube_textured`] / [`push_cube_textured_lit`].
#[cfg(test)]
pub fn cube_textured(tiles: [Tile; 3], origin: Vec3, size: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    push_cube_textured(&mut verts, &mut indices, tiles, origin, size);
    (verts, indices)
}

/// Append a full-bright solid-color cuboid spanning `[origin, origin + size]` with
/// RGB `tint` into the caller-owned `verts`/`indices` (capacity reused). The
/// model3d fragment shader reads [`SOLID_COLOR_FLAG`] and outputs `tint` directly.
/// 24 verts / 36 indices. Test-only; live render passes explicit light via
/// [`push_cube_solid_lit`].
#[cfg(test)]
pub fn push_cube_solid(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tint: [f32; 3],
    origin: Vec3,
    size: f32,
) {
    push_cube_solid_lit(verts, indices, tint, origin, size, DynLight::FULL);
}

pub(super) fn push_cube_solid_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tint: [f32; 3],
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
    for face in ALL_FACES {
        push_quad(
            verts,
            indices,
            face.quad_box(origin.to_array(), max.to_array()),
            tint,
            face_bits_solid_lit(face, light.sky),
            lighting::blocklight_word(light.block),
        );
    }
}

/// A full-bright solid-color cuboid spanning `[origin, origin + size]` with RGB
/// `tint`. 24 verts / 36 indices.
///
/// Test-only convenience; live render uses [`push_cube_solid_lit`].
#[cfg(test)]
pub fn cube_solid(tint: [f32; 3], origin: Vec3, size: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    push_cube_solid(&mut verts, &mut indices, tint, origin, size);
    (verts, indices)
}

/// Camera basis vectors for building world-space camera-facing billboards. The
/// renderer derives these from the view matrix each frame (the camera's right and
/// up axes) so a billboard quad always faces the viewer.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct BillboardBasis {
    pub right: Vec3,
    pub up: Vec3,
}

/// Append a **double-sided**, camera-facing billboard quad (8 verts / 12 indices)
/// for `tile`, centred at world `center` and `size` across, oriented by the camera
/// `basis`. Full-bright and untinted; reuses the textured packing so the opaque
/// block pipeline samples the tile (its `< 0.5` alpha discard cuts out the cross
/// plant cleanly). Used by world item-entities (sprite kind).
///
/// Both windings are emitted so the sprite is visible regardless of which way the
/// camera basis winds the quad — the item-entity pass shares the back-face-culling
/// opaque pipeline, so a single-sided quad could silently vanish if the basis sign
/// ever regressed. Double-siding is cheap (8 verts) and removes that risk.
pub(super) fn push_billboard_world_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tile: Tile,
    center: Vec3,
    size: f32,
    basis: BillboardBasis,
    light: DynLight,
) {
    let h = size * 0.5;
    let r = basis.right * h;
    let u = basis.up * h;
    // Corners CCW from the camera's view: bl, br, tr, tl (matches corner_uv).
    let bl = (center - r - u).to_array();
    let br = (center + r - u).to_array();
    let tr = (center + r + u).to_array();
    let tl = (center - r + u).to_array();
    // Sprites are flat: use the brightest (top) shade so they read evenly. Fern /
    // short-grass sprites get the fixed grass tint (flowers stay untinted).
    let tint = foliage_tint::face_material(tile).tint;
    let base = (tile.index() as u32)
        | (Face::PosY.shade_idx() << 10)
        | FULL_AO
        | lighting::skylight_bits(light.sky);
    let word2 = lighting::blocklight_word(light.block);
    // Front winding (faces the camera) + reversed winding (faces away), so the
    // sprite never culls from either side.
    push_quad(verts, indices, [bl, br, tr, tl], tint, base, word2);
    push_quad(verts, indices, [br, bl, tl, tr], tint, base, word2);
}

/// Append a flat, upright, double-sided billboard quad of one `tile`, centered on
/// `center` in the X (right) / Y (up) plane, `size` tall & wide, full-bright, into
/// the caller-owned `verts`/`indices` (capacity reused). Emitted in both windings
/// so it is visible from either side under back-face culling. 8 verts / 12 indices.
pub fn push_billboard_quad(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tile: Tile,
    center: Vec3,
    size: f32,
) {
    let h = size * 0.5;
    // Front-facing (+Z) winding: 0 bl, 1 br, 2 tr, 3 tl (matches corner_uv).
    let front = [
        [center.x - h, center.y - h, center.z],
        [center.x + h, center.y - h, center.z],
        [center.x + h, center.y + h, center.z],
        [center.x - h, center.y + h, center.z],
    ];
    // Back-facing winding: same positions, reversed so CCW points the other way.
    let back = [
        [center.x + h, center.y - h, center.z],
        [center.x - h, center.y - h, center.z],
        [center.x - h, center.y + h, center.z],
        [center.x + h, center.y + h, center.z],
    ];
    // Sprites are flat: use the brightest (top) shade so they read evenly. Fern /
    // short-grass sprites get the fixed grass tint (flowers stay untinted).
    let tint = foliage_tint::face_material(tile).tint;
    let base = (tile.index() as u32)
        | (Face::PosY.shade_idx() << 10)
        | FULL_AO
        | lighting::skylight_bits(lighting::FULL_SKYLIGHT);
    push_quad(verts, indices, front, tint, base, 0);
    push_quad(verts, indices, back, tint, base, 0);
}

/// A flat, upright, double-sided billboard quad of one `tile`, centered on
/// `center` in the X (right) / Y (up) plane, `size` tall & wide, full-bright. 8 verts
/// / 12 indices.
///
/// Test-only convenience: live render bakes into caller buffers via
/// [`push_billboard_quad`].
#[cfg(test)]
pub fn billboard_quad(tile: Tile, center: Vec3, size: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(8);
    let mut indices = Vec::with_capacity(12);
    push_billboard_quad(&mut verts, &mut indices, tile, center, size);
    (verts, indices)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::mesh::SHADES;

    fn uv_mode(v: &Vertex) -> u32 {
        (v.packed >> UV_MODE_SHIFT) & 0x7
    }

    fn cell_uv16(v: &Vertex) -> (u32, u32) {
        ((v.packed2 >> 6) & 0x1F, (v.packed2 >> 11) & 0x1F)
    }

    #[test]
    fn cube_textured_has_24_verts_36_indices() {
        let (v, i) = cube_textured(
            [
                Tile::named("oak_log_top"),
                Tile::named("oak_log_top"),
                Tile::named("oak_log_side"),
            ],
            Vec3::ZERO,
            1.0,
        );
        assert_eq!(v.len(), 24);
        assert_eq!(i.len(), 36);
    }

    #[test]
    fn cube_textured_uses_per_face_tiles() {
        // Distinct top/bottom/side so we can check each face samples the right tile.
        let tiles = [
            Tile::named("grass_top"),
            Tile::named("dirt"),
            Tile::named("stone"),
        ];
        let (v, _) = cube_textured(tiles, Vec3::ZERO, 1.0);
        // Faces emitted in ALL_FACES order: PosX, NegX, PosY, NegY, PosZ, NegZ.
        // 4 verts per face; the tile id is bits 0..8 of `packed`.
        let face_tile = |face_idx: usize| (v[face_idx * 4].packed & 0xFF) as u8;
        // PosX (side), NegX (side)
        assert_eq!(face_tile(0), Tile::named("stone").index() as u8);
        assert_eq!(face_tile(1), Tile::named("stone").index() as u8);
        // PosY (top), NegY (bottom)
        assert_eq!(face_tile(2), Tile::named("grass_top").index() as u8);
        assert_eq!(face_tile(3), Tile::named("dirt").index() as u8);
        // PosZ (side), NegZ (side)
        assert_eq!(face_tile(4), Tile::named("stone").index() as u8);
        assert_eq!(face_tile(5), Tile::named("stone").index() as u8);
    }

    #[test]
    fn block_icon_faces_default_expands_top_bottom_side() {
        // A normal block just expands its 3-tile model: side on every horizontal.
        let faces = block_icon_faces(Block::OakLog);
        let [top, bottom, side] = Block::OakLog.tiles();
        assert_eq!(faces, [side, side, top, bottom, side, side]);
    }

    #[test]
    fn horizontal_log_item_rotates_bark_face_uvs() {
        let mut verts = Vec::new();
        let mut indices = Vec::new();
        push_block_item_cube_lit_with_state(
            &mut verts,
            &mut indices,
            Block::OakLog,
            HeldBlockState::Log(LogAxis::X),
            Vec3::ZERO,
            1.0,
            DynLight::FULL,
        );

        assert_eq!(verts.len(), 24);
        assert_eq!(indices.len(), 36);
        let top_bark = &verts[2 * 4..3 * 4];
        assert!(
            top_bark.iter().all(|v| uv_mode(v) == UV_MODE_CELL_LOCAL),
            "horizontal log side faces must rotate their bark UVs"
        );
        let mut uvs = top_bark.iter().map(cell_uv16).collect::<Vec<_>>();
        uvs.sort_unstable();
        assert_eq!(uvs, vec![(0, 0), (0, 16), (16, 0), (16, 16)]);

        let pos_x_end_cap = &verts[0..4];
        assert!(
            pos_x_end_cap.iter().all(|v| uv_mode(v) == 0),
            "end caps keep the regular cube UVs"
        );
    }

    #[test]
    fn furnace_icon_shows_front_on_exactly_one_face() {
        // The reported bug was four fronts; the item must show the front once, with
        // furnace_side on the other three horizontal faces (top/bottom are the top).
        let faces = block_icon_faces(Block::Furnace);
        assert_eq!(faces[2], Tile::named("furnace_top"), "PosY top");
        assert_eq!(faces[3], Tile::named("furnace_top"), "NegY bottom");
        assert_eq!(
            faces[4],
            Tile::named("furnace_front"),
            "front on PosZ (visible in the icon)"
        );
        for i in [0usize, 1, 5] {
            assert_eq!(
                faces[i],
                Tile::named("furnace_side"),
                "face {i} is a plain side"
            );
        }
        assert_eq!(
            faces
                .iter()
                .filter(|&&t| t == Tile::named("furnace_front"))
                .count(),
            1,
            "exactly one front face, not four"
        );
    }

    #[test]
    fn cube_textured_is_full_bright() {
        let (v, _) = cube_textured([Tile::named("stone"); 3], Vec3::ZERO, 1.0);
        for vert in &v {
            // skylight (bits 23..29) is full (63).
            assert_eq!((vert.packed >> 23) & 0x3F, 63);
            // AO (bits 21..23) is full (3).
            assert_eq!((vert.packed >> 21) & 0x3, 3);
            // textured path never sets the solid-color flag.
            assert_eq!(vert.packed & SOLID_COLOR_FLAG, 0);
        }
    }

    #[test]
    fn cube_textured_face_shade_indices_match_mesher() {
        let (v, _) = cube_textured([Tile::named("stone"); 3], Vec3::ZERO, 1.0);
        let shade = |face_idx: usize| (v[face_idx * 4].packed >> 10) & 0x3;
        assert_eq!(shade(0), 2); // PosX
        assert_eq!(shade(1), 2); // NegX
        assert_eq!(shade(2), 0); // PosY (top, brightest)
        assert_eq!(shade(3), 3); // NegY (bottom, darkest)
        assert_eq!(shade(4), 1); // PosZ
        assert_eq!(shade(5), 1); // NegZ
                                 // SHADES table is the brightness these indices reference.
        const { assert!(SHADES[0] > SHADES[3]) };
    }

    #[test]
    fn cube_solid_sets_flag_and_carries_tint() {
        let tint = [0.9, 0.7, 0.6];
        let (v, i) = cube_solid(tint, Vec3::ZERO, 1.0);
        assert_eq!(v.len(), 24);
        assert_eq!(i.len(), 36);
        for vert in &v {
            assert_eq!(vert.packed & SOLID_COLOR_FLAG, SOLID_COLOR_FLAG);
            assert_eq!(vert.tint, tint);
            assert_eq!((vert.packed >> 23) & 0x3F, 63);
        }
    }

    #[test]
    fn billboard_quad_is_double_sided() {
        let (v, i) = billboard_quad(Tile::named("poppy"), Vec3::ZERO, 1.0);
        assert_eq!(v.len(), 8); // two quads (front + back)
        assert_eq!(i.len(), 12);
        for vert in &v {
            assert_eq!(
                (vert.packed & 0xFF) as u8,
                Tile::named("poppy").index() as u8
            );
            assert_eq!(vert.packed & SOLID_COLOR_FLAG, 0);
        }
    }

    #[test]
    fn cube_textured_tints_grass_top_and_overlays_sides() {
        // Block::Grass tiles = [GrassTop, Dirt, GrassSide].
        let (v, _) = cube_textured(
            [
                Tile::named("grass_top"),
                Tile::named("dirt"),
                Tile::named("grass_side"),
            ],
            Vec3::ZERO,
            1.0,
        );
        let grass = foliage_tint::default_grass_color();
        // Faces emitted in ALL_FACES order: PosX, NegX, PosY, NegY, PosZ, NegZ.
        // Top face (PosY = index 2): GrassTop tinted green, no overlay.
        let top = &v[2 * 4];
        assert_eq!(
            (top.packed & 0xFF) as u8,
            Tile::named("grass_top").index() as u8
        );
        assert_eq!(top.tint, grass);
        assert_eq!(top.packed & SOLID_COLOR_FLAG, 0, "top has no overlay flag");

        // Side faces (PosX 0, NegX 1, PosZ 4, NegZ 5): dirt base + tinted
        // grass-side overlay (bit 20 = has-overlay), overlay tile in bits 12..20.
        for idx in [0usize, 1, 4, 5] {
            let s = &v[idx * 4];
            assert_eq!(
                (s.packed & 0xFF) as u8,
                Tile::named("dirt").index() as u8,
                "side base = dirt"
            );
            // Bit 20 (overlay flag) set; overlay tile = GrassSideOverlay.
            assert_eq!(
                s.packed & SOLID_COLOR_FLAG,
                SOLID_COLOR_FLAG,
                "side has overlay flag"
            );
            assert_eq!(
                ((s.packed >> 12) & 0xFF) as u8,
                Tile::named("grass_side_overlay").index() as u8,
                "side overlay tile = grass-side overlay"
            );
            assert_eq!(s.tint, grass, "side overlay tinted green");
        }

        // Bottom face (NegY = index 3): plain dirt, untinted, no overlay.
        let bot = &v[3 * 4];
        assert_eq!((bot.packed & 0xFF) as u8, Tile::named("dirt").index() as u8);
        assert_eq!(bot.tint, foliage_tint::NO_TINT);
        assert_eq!(bot.packed & SOLID_COLOR_FLAG, 0);
    }

    #[test]
    fn cube_textured_leaves_use_foliage_tint() {
        let (v, _) = cube_textured([Tile::named("oak_leaves"); 3], Vec3::ZERO, 1.0);
        let foliage = foliage_tint::default_foliage_color();
        for vert in &v {
            assert_eq!(
                (vert.packed & 0xFF) as u8,
                Tile::named("oak_leaves").index() as u8
            );
            assert_eq!(vert.tint, foliage);
            assert_eq!(vert.packed & SOLID_COLOR_FLAG, 0, "leaves carry no overlay");
        }
    }

    #[test]
    fn flower_billboard_stays_untinted() {
        let (v, _) = billboard_quad(Tile::named("poppy"), Vec3::ZERO, 1.0);
        for vert in &v {
            assert_eq!(
                vert.tint,
                foliage_tint::NO_TINT,
                "flowers are not biome-tinted"
            );
            assert_eq!(vert.packed & SOLID_COLOR_FLAG, 0);
        }
    }

    #[test]
    fn fern_billboard_gets_grass_tint() {
        let (v, _) = billboard_quad(Tile::named("fern"), Vec3::ZERO, 1.0);
        let grass = foliage_tint::default_grass_color();
        for vert in &v {
            assert_eq!(vert.tint, grass, "ferns tint with the grass colour");
        }
    }
}
