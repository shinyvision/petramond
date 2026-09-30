//! Geometry helpers that build small meshes in the 32-byte `mesh::Vertex`
//! format for the held-item hand, dropped item-entities, and the isometric
//! inventory icons.
//!
//! These all reuse the block atlas + `tile_uv()` table the chunk mesher uses, so
//! a held log / dropped log / log icon are textured identically to the world
//! block. The default helpers draw full-bright for inventory icons; the `_lit`
//! variants pack sampled world skylight for hand/items while keeping AO = 3.
//!
//! ## Packing conventions (shared with `block.wgsl`'s `packed` layout)
//! The bit positions live in `mesh::vertex` and this module encodes through
//! those constants — see [`mesh::pack_vertex`](petramond_mesh::Vertex) for the
//! layout of both words. For the textured path (`cube_textured`,
//! `billboard_quad`) we set the tile, corner, shade, AO = 3, skylight = 63.
//!
//! ### Out-of-world foliage tint + grass-side overlay
//! Icons / held items / dropped cubes have no biome context, so foliage greens
//! using a single fixed temperate colour from [`foliage_tint`]. Each cube face is
//! classified exactly like the chunk mesher: grass-top / short-grass / fern get
//! the grass tint; all leaves get the foliage tint; grass-block SIDES render as a
//! dirt base plus the tinted grayscale `GrassSideOverlay` — its tile rides the
//! `packed2` overlay payload with the has-overlay flag set (the same
//! overlay-composite path the chunk mesher uses, which `model3d.wgsl` mirrors).

use super::foliage_tint::{self, FaceMaterial};
use super::lighting::{self, DynLight};
use petramond_math::face::Face;
use petramond_math::facing::Facing;
use petramond_mesh::face::FaceShading;
use petramond_mesh::vertex::BlockLightVertexExt;
use petramond_mesh::{
    pack_cell_uv, pack_tint, Vertex, UV_MODE_CELL_LOCAL, UV_MODE_SHIFT, UV_MODE_THIN_U,
    UV_MODE_THIN_V,
};
use petramond_world::block::Block;
use petramond_world::block_state::{HeldBlockState, LogAxis};
use petramond_world::tile::Tile;

use glam::Vec3;

const FULL_AO: u32 = 3 << petramond_mesh::AO_SHIFT;

const ALL_FACES: [Face; 6] = Face::ALL;

#[inline]
fn face_bits_textured_lit(mat: FaceMaterial, face: Face, light: DynLight) -> u32 {
    (mat.base_tile.index() as u32)
        | (face.shade_idx() << petramond_mesh::SHADE_SHIFT)
        | if mat.overlay_tile.is_some() {
            petramond_mesh::OVERLAY_FLAG
        } else {
            0
        }
        | FULL_AO
        | lighting::skylight_bits(light.sky)
        | light.block.packed_bits()
}

/// The `packed2` companion to [`face_bits_textured_lit`]: the overlay TILE lives
/// in the second word (see `mesh::pack_overlay`), so a material with one must
/// contribute to both.
///
/// Block light rides THREE words — `packed_bits` (chroma high nibble) above,
/// `packed2_bits` (red) beside this, and `tint_word` (chroma low byte) on the
/// tint — so every quad emitter here has to write all three or a coloured lamp
/// renders grey on held/dropped/placed dynamic geometry.
#[inline]
fn face_bits2(mat: FaceMaterial) -> u32 {
    match mat.overlay_tile {
        Some(o) => petramond_mesh::pack_overlay(o.index() as u32),
        None => 0,
    }
}

#[inline]
fn push_quad_with(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    mut vertex: impl FnMut(usize, [f32; 3]) -> Vertex,
) {
    let start = verts.len() as u32;
    for (corner, pos) in corners.into_iter().enumerate() {
        verts.push(vertex(corner, pos));
    }
    indices.extend_from_slice(&[start, start + 1, start + 2, start, start + 2, start + 3]);
}

#[inline]
fn push_quad(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    tint: u32,
    base_bits: u32,
    packed2: u32,
) {
    push_quad_with(verts, indices, corners, |corner, pos| Vertex {
        pos,
        tint,
        packed: base_bits | ((corner as u32) << petramond_mesh::CORNER_SHIFT),
        packed2,
    });
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
        let [u, v] = petramond_mesh::face::log_side_cell_uv(face, axis, local)?;
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
    tint: u32,
    base_bits: u32,
    packed2: u32,
) {
    push_quad_with(verts, indices, corners, |corner, pos| {
        let (u, v) = cell_uvs[corner];
        Vertex {
            pos,
            tint,
            packed: base_bits | ((corner as u32) << petramond_mesh::CORNER_SHIFT),
            packed2: packed2 | pack_cell_uv(u, v),
        }
    });
}

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
    push_cube_faces_lit(
        verts,
        indices,
        expand_tiles(tiles),
        [0; 6],
        origin,
        size,
        light,
    );
}

#[inline]
fn expand_tiles(tiles: [Tile; 3]) -> [Tile; 6] {
    let [top, bottom, side] = tiles;
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

#[cfg(test)]
fn block_icon_faces(block: Block) -> [Tile; 6] {
    block_icon_faces_with_state(block, HeldBlockState::None)
}

pub(super) fn block_icon_faces_with_state(block: Block, state: HeldBlockState) -> [Tile; 6] {
    let mut faces = expand_tiles(block.tiles());
    if block.is_axial() {
        let axis = match state {
            HeldBlockState::Log(axis) => axis,
            _ => LogAxis::Y,
        };
        faces = expand_log_tiles(block.tiles(), axis);
    }
    if let Some(front) = block.front_tile() {
        faces[4] = front;
    }
    faces
}

fn block_icon_uv_turns(block: Block, state: HeldBlockState) -> [u8; 6] {
    let [top, bottom, side] = block.uv_turns();
    if block.is_axial() {
        let axis = match state {
            HeldBlockState::Log(axis) => axis,
            _ => LogAxis::Y,
        };
        return match axis {
            LogAxis::X => [top, bottom, side, side, side, side],
            LogAxis::Y => [side, side, top, bottom, side, side],
            LogAxis::Z => [side, side, side, side, top, bottom],
        };
    }
    let mut turns = [side, side, top, bottom, side, side];
    if block.front_tile().is_some() {
        turns[4] = 0;
    }
    turns
}

pub(super) fn push_cube_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    uv_turns: [u8; 6],
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
    push_box_faces_lit(verts, indices, faces, uv_turns, origin, max, light);
}

pub(super) fn push_box_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    uv_turns: [u8; 6],
    min: Vec3,
    max: Vec3,
    light: DynLight,
) {
    for ((tile, face), turn) in faces.into_iter().zip(ALL_FACES).zip(uv_turns) {
        let mat = foliage_tint::face_material(tile);
        push_quad(
            verts,
            indices,
            face.quad_box(min.to_array(), max.to_array()),
            light.block.tint_word(mat.tint),
            face_bits_textured_lit(mat, face, light)
                | petramond_mesh::vertex::pack_uv_turn(turn as u32),
            light.block.packed2_bits()
                | face_bits2(mat)
                | petramond_mesh::vertex::pack_uv_turn2(turn as u32),
        );
    }
}

#[allow(clippy::too_many_arguments)]
fn push_log_cube_faces_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    faces: [Tile; 6],
    uv_turns: [u8; 6],
    axis: LogAxis,
    origin: Vec3,
    size: f32,
    light: DynLight,
) {
    let max = Vec3::new(origin.x + size, origin.y + size, origin.z + size);
    for (i, (tile, face)) in faces.into_iter().zip(ALL_FACES).enumerate() {
        let mat = foliage_tint::face_material(tile);
        let corners = face.quad_box(origin.to_array(), max.to_array());
        let word2 = light.block.packed2_bits() | face_bits2(mat);
        if let Some(cell_uvs) = log_side_cell_uvs(axis, face) {
            push_quad_cell_uvs(
                verts,
                indices,
                corners,
                cell_uvs,
                light.block.tint_word(mat.tint),
                face_bits_textured_lit(mat, face, light) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT),
                word2,
            );
        } else {
            let turn = uv_turns[i] as u32;
            push_quad(
                verts,
                indices,
                corners,
                light.block.tint_word(mat.tint),
                face_bits_textured_lit(mat, face, light)
                    | petramond_mesh::vertex::pack_uv_turn(turn),
                word2 | petramond_mesh::vertex::pack_uv_turn2(turn),
            );
        }
    }
}

pub(super) fn facing_yaw(facing: Facing) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match facing {
        Facing::South => 0.0,
        Facing::North => PI,
        Facing::East => FRAC_PI_2,
        Facing::West => -FRAC_PI_2,
    }
}

pub(super) fn orient_faces_to_block(verts: &mut [Vertex], start: usize, facing: Facing, pos: Vec3) {
    let (ys, yc) = facing_yaw(facing).sin_cos();
    for v in verts[start..].iter_mut() {
        let [x, y, z] = v.pos;
        let dx = x - 0.5;
        let dz = z - 0.5;
        let rx = 0.5 + dx * yc + dz * ys;
        let rz = 0.5 - dx * ys + dz * yc;
        v.pos = [pos.x + rx, pos.y + y, pos.z + rz];
    }
}

pub(super) const UV_SLICE_SHIFT: u32 = UV_MODE_SHIFT;

/// The per-face UV-slice modes (`ALL_FACES` order) a box of this extent needs.
///
/// A face only a panel's THICKNESS deep on one of its two in-plane axes would
/// squish a whole tile flat across that strip, so it crops its tile to a
/// matching slice instead. The shader's crop is a fixed 3/16 (`THIN_SLICE` in
/// `block.wgsl`), so ONLY a box that thin qualifies — everything else stays
/// full-tile. One derivation for both hinged panels, replacing the door's
/// hand-written table.
///
/// These modes are for geometry drawn by the BLOCK pipeline. The crack overlay
/// runs its own shader, which decodes only cell-local UVs, so it carves thin
/// faces that way instead (see [`crate::break_overlay`]).
pub(super) fn thin_face_slice_modes(min: Vec3, max: Vec3) -> [u32; 6] {
    let size = max - min;
    let thin = |axis: usize| (size[axis] - petramond_world::door::THICKNESS).abs() < 1e-4;
    const UV_AXES: [(usize, usize); 6] = [(2, 1), (2, 1), (0, 2), (0, 2), (0, 1), (0, 1)];
    UV_AXES.map(|(u, v)| {
        if thin(v) {
            UV_MODE_THIN_V
        } else if thin(u) {
            UV_MODE_THIN_U
        } else {
            0
        }
    })
}

/// As [`push_box_faces_lit`] but, per face (`ALL_FACES` order):
/// - MIRRORS the texture horizontally where `mirror_u` is set — used by the door so
///   its BACK face is the mirror image of its front (hinge/handle stay on the same
///   physical side from either side). Mirroring is pure UV: the quad's corner indices
///   are swapped left↔right (`[1,0,3,2]`), flipping `u`, no geometry/winding change.
/// - applies a thin-face UV-SLICE mode from `slice_mode` (0 none, 1 crop-U, 2 crop-V),
///   packed at [`UV_SLICE_SHIFT`] so the shader crops a 3/16-deep face
///   to a matching strip of its tile instead of squishing the whole tile flat — used
///   by the door's thin side (crop-U) and top/bottom edge (crop-V) faces.
#[allow(clippy::too_many_arguments)]
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
        let bits = face_bits_textured_lit(mat, face, light) | (slice << UV_SLICE_SHIFT);
        let word2 = light.block.packed2_bits() | face_bits2(mat);
        let tint = light.block.tint_word(mat.tint);
        if mir {
            push_quad_uflip(verts, indices, corners, tint, bits, word2);
        } else {
            push_quad(verts, indices, corners, tint, bits, word2);
        }
    }
}

#[inline]
fn push_quad_uflip(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    corners: [[f32; 3]; 4],
    tint: u32,
    base_bits: u32,
    packed2: u32,
) {
    const MIRROR: [u32; 4] = [1, 0, 3, 2];
    push_quad_with(verts, indices, corners, |corner, pos| Vertex {
        pos,
        tint,
        packed: base_bits | (MIRROR[corner] << petramond_mesh::CORNER_SHIFT),
        packed2,
    });
}

pub(super) fn push_block_item_cube(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    origin: Vec3,
    size: f32,
) {
    push_block_item_cube_lit(verts, indices, block, origin, size, DynLight::FULL, true);
}

pub(super) fn push_block_item_cube_lit(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    origin: Vec3,
    size: f32,
    light: DynLight,
    sort_for_icon: bool,
) {
    push_block_item_cube_lit_with_state(
        verts,
        indices,
        block,
        HeldBlockState::None,
        origin,
        size,
        light,
        sort_for_icon,
    );
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_block_item_cube_lit_with_state(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    block: Block,
    state: HeldBlockState,
    origin: Vec3,
    size: f32,
    light: DynLight,
    sort_for_icon: bool,
) {
    if let Some(model) = super::block_entity_model::item_model(block) {
        super::block_entity_model::push_item(verts, indices, model, block, origin, size, light);
        return;
    }
    let faces = block_icon_faces_with_state(block, state);
    if let Some(boxes) = petramond_world::block::item_shape_bake::item_bake(block.id()) {
        if !boxes.is_empty() {
            for b in icon_painter_order(&boxes, |b| b, sort_for_icon) {
                for (i, face) in ALL_FACES.into_iter().enumerate() {
                    push_cell_local_face(
                        verts, indices, faces[i], origin, size, b.min, b.max, face, light,
                    );
                }
            }
            return;
        }
    }
    let k = block.shape_kind_def();
    let mut boxes = Vec::new();
    k.render.item_boxes(&k.params, block, state, &mut boxes);
    if !boxes.is_empty() {
        let bounds: Vec<petramond_world::block::Aabb> =
            boxes.iter().map(|b| b.posed_bounds()).collect();
        let order = icon_painter_order(&bounds, |b| b, sort_for_icon)
            .into_iter()
            .map(|b| {
                bounds
                    .iter()
                    .position(|x| std::ptr::eq(x, b))
                    .expect("own element")
            });
        for b in order.map(|i| &boxes[i]) {
            let box_faces = match b.material {
                Some(mat) => block_icon_faces_with_state(mat, HeldBlockState::None),
                None => faces,
            };
            for (i, face) in ALL_FACES.into_iter().enumerate() {
                if !b.faces[i] {
                    continue;
                }
                push_cell_local_face_styled(
                    verts,
                    indices,
                    b.tiles[i].unwrap_or(box_faces[i]),
                    origin,
                    size,
                    b.aabb.min,
                    b.aabb.max,
                    face,
                    light,
                    FaceArt {
                        uv_turns: b.uv_turns[i],
                        uv_rect: b.uv_rects[i],
                        pose: b.pose,
                    },
                );
            }
        }
        return;
    }
    if block.is_axial() {
        let axis = match state {
            HeldBlockState::Log(axis) => axis,
            _ => LogAxis::Y,
        };
        push_log_cube_faces_lit(
            verts,
            indices,
            faces,
            block_icon_uv_turns(block, state),
            axis,
            origin,
            size,
            light,
        );
        return;
    }
    push_cube_faces_lit(
        verts,
        indices,
        faces,
        block_icon_uv_turns(block, state),
        origin,
        size,
        light,
    );
}

#[allow(clippy::too_many_arguments)]
fn icon_painter_order<T>(
    items: &[T],
    aabb: impl Fn(&T) -> &petramond_world::block::Aabb,
    sort_for_icon: bool,
) -> Vec<&T> {
    let mut order: Vec<&T> = items.iter().collect();
    if sort_for_icon {
        let dir = crate::ui::icon::icon_view_dir();
        let depth = |t: &T| {
            let b = aabb(t);
            let c = |i: usize| (b.min[i] + b.max[i]) * 0.5;
            dir.x * c(0) + dir.y * c(1) + dir.z * c(2)
        };
        order.sort_by(|a, b| {
            depth(a)
                .partial_cmp(&depth(b))
                .unwrap_or(std::cmp::Ordering::Equal)
        });
    }
    order
}

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
    push_cell_local_face_styled(
        verts,
        indices,
        tile,
        origin,
        size,
        min,
        max,
        face,
        light,
        FaceArt::default(),
    );
}

#[derive(Copy, Clone, Default)]
pub(super) struct FaceArt {
    pub uv_turns: u8,
    pub uv_rect: Option<[u8; 4]>,
    pub pose: Option<petramond_world::block::BoxPose>,
}

#[allow(clippy::too_many_arguments)]
pub(super) fn push_cell_local_face_styled(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tile: Tile,
    origin: Vec3,
    size: f32,
    min: [f32; 3],
    max: [f32; 3],
    face: Face,
    light: DynLight,
    art: FaceArt,
) {
    let normal_axis = match face {
        Face::PosX | Face::NegX => 0,
        Face::PosY | Face::NegY => 1,
        Face::PosZ | Face::NegZ => 2,
    };
    if (0..3).any(|a| a != normal_axis && max[a] - min[a] <= 0.0) {
        return;
    }
    let mat = foliage_tint::face_material(tile);
    let bits = face_bits_textured_lit(mat, face, light) | (UV_MODE_CELL_LOCAL << UV_MODE_SHIFT);
    let word2 = light.block.packed2_bits() | face_bits2(mat);
    let local = face.quad_box(min, max);
    let corners = local.map(|p| {
        let p = Vec3::from(p);
        let p = art.pose.map_or(p, |pose| pose.apply(p));
        (origin + p * size).to_array()
    });
    let style = petramond_world::block::ShapeFace {
        tile,
        swap_uv: false,
        uv_turns: art.uv_turns,
        tint: [1.0; 3],
        uv_rect: art.uv_rect,
    };
    push_quad_with(verts, indices, corners, |corner, pos| {
        let [u, v] = petramond_mesh::plane::cell_uv(face, local[corner]);
        let (u, v) = style.texel_uv(
            (u, v),
            petramond_mesh::plane::face_fraction(face, min, max, (u, v)),
        );
        let (u, v) = (u.clamp(0.0, 1.0), v.clamp(0.0, 1.0));
        Vertex {
            pos,
            tint: light.block.tint_word(mat.tint),
            packed: bits | ((corner as u32) << petramond_mesh::CORNER_SHIFT),
            packed2: word2 | pack_cell_uv((u * 16.0).round() as u32, (v * 16.0).round() as u32),
        }
    });
}

#[cfg(test)]
pub fn cube_textured(tiles: [Tile; 3], origin: Vec3, size: f32) -> (Vec<Vertex>, Vec<u32>) {
    let mut verts = Vec::with_capacity(24);
    let mut indices = Vec::with_capacity(36);
    push_cube_textured(&mut verts, &mut indices, tiles, origin, size);
    (verts, indices)
}

pub fn push_billboard_quad(
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
    tile: Tile,
    center: Vec3,
    size: f32,
) {
    let h = size * 0.5;
    let front = [
        [center.x - h, center.y - h, center.z],
        [center.x + h, center.y - h, center.z],
        [center.x + h, center.y + h, center.z],
        [center.x - h, center.y + h, center.z],
    ];
    let back = [
        [center.x + h, center.y - h, center.z],
        [center.x - h, center.y - h, center.z],
        [center.x - h, center.y + h, center.z],
        [center.x + h, center.y + h, center.z],
    ];
    let tint = foliage_tint::face_material(tile).tint;
    let base = (tile.index() as u32)
        | (Face::PosY.shade_idx() << petramond_mesh::SHADE_SHIFT)
        | FULL_AO
        | lighting::skylight_bits(lighting::FULL_SKYLIGHT);
    let tint = pack_tint(tint);
    push_quad(verts, indices, front, tint, base, 0);
    push_quad(verts, indices, back, tint, base, 0);
}

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
    use petramond_mesh::SHADES;

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
        let tiles = [
            Tile::named("grass_top"),
            Tile::named("dirt"),
            Tile::named("stone"),
        ];
        let (v, _) = cube_textured(tiles, Vec3::ZERO, 1.0);
        let face_tile =
            |face_idx: usize| v[face_idx * 4].packed & petramond_mesh::vertex::TILE_MASK;
        assert_eq!(face_tile(0), Tile::named("stone").index() as u32);
        assert_eq!(face_tile(1), Tile::named("stone").index() as u32);
        assert_eq!(face_tile(2), Tile::named("grass_top").index() as u32);
        assert_eq!(face_tile(3), Tile::named("dirt").index() as u32);
        assert_eq!(face_tile(4), Tile::named("stone").index() as u32);
        assert_eq!(face_tile(5), Tile::named("stone").index() as u32);
    }

    #[test]
    fn block_icon_faces_default_expands_top_bottom_side() {
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
            false,
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
            assert_eq!(
                (vert.packed >> petramond_mesh::vertex::SKY_SHIFT) & 0x3F,
                63
            );
            assert_eq!((vert.packed >> petramond_mesh::vertex::AO_SHIFT) & 0x3, 3);
            assert_eq!(vert.packed & petramond_mesh::OVERLAY_FLAG, 0);
        }
    }

    #[test]
    fn cube_textured_face_shade_indices_match_mesher() {
        let (v, _) = cube_textured([Tile::named("stone"); 3], Vec3::ZERO, 1.0);
        let shade =
            |face_idx: usize| (v[face_idx * 4].packed >> petramond_mesh::vertex::SHADE_SHIFT) & 0x3;
        assert_eq!(shade(0), 2);
        assert_eq!(shade(1), 2);
        assert_eq!(shade(2), 0);
        assert_eq!(shade(3), 3);
        assert_eq!(shade(4), 1);
        assert_eq!(shade(5), 1);
        const { assert!(SHADES[0] > SHADES[3]) };
    }
    #[test]
    fn billboard_quad_is_double_sided() {
        let (v, i) = billboard_quad(Tile::named("poppy"), Vec3::ZERO, 1.0);
        assert_eq!(v.len(), 8);
        assert_eq!(i.len(), 12);
        for vert in &v {
            assert_eq!(
                vert.packed & petramond_mesh::vertex::TILE_MASK,
                Tile::named("poppy").index() as u32
            );
            assert_eq!(vert.packed & petramond_mesh::OVERLAY_FLAG, 0);
        }
    }

    #[test]
    fn cube_textured_tints_grass_top_and_overlays_sides() {
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
        let top = &v[2 * 4];
        assert_eq!(
            top.packed & petramond_mesh::vertex::TILE_MASK,
            Tile::named("grass_top").index() as u32
        );
        assert_eq!(top.tint, pack_tint(grass));
        assert_eq!(
            top.packed & petramond_mesh::OVERLAY_FLAG,
            0,
            "top has no overlay flag"
        );

        for idx in [0usize, 1, 4, 5] {
            let s = &v[idx * 4];
            assert_eq!(
                s.packed & petramond_mesh::vertex::TILE_MASK,
                Tile::named("dirt").index() as u32,
                "side base = dirt"
            );
            assert_eq!(
                s.packed & petramond_mesh::OVERLAY_FLAG,
                petramond_mesh::OVERLAY_FLAG,
                "side has overlay flag"
            );
            assert_eq!(
                (s.packed2 >> petramond_mesh::vertex::OVERLAY_SHIFT2)
                    & petramond_mesh::vertex::OVERLAY_MASK,
                Tile::named("grass_side_overlay").index() as u32,
                "side overlay tile = grass-side overlay"
            );
            assert_eq!(s.tint, pack_tint(grass), "side overlay tinted green");
        }

        let bot = &v[3 * 4];
        assert_eq!(
            bot.packed & petramond_mesh::vertex::TILE_MASK,
            Tile::named("dirt").index() as u32
        );
        assert_eq!(bot.tint, pack_tint(foliage_tint::NO_TINT));
        assert_eq!(bot.packed & petramond_mesh::OVERLAY_FLAG, 0);
    }
}
