//! Block-break crack overlay geometry.
//!
//! From a [`BreakOverlayView`] (target block + crack stage 0..9), builds the six
//! faces of that block's **exact** unit cube, each textured with the matching
//! `Tile::DestroyStage{stage}`. The cube is built at the block's integer world
//! coordinates with no inflation, so every face is *coincident* with the chunk
//! mesh's face for that block — same `quad_for` corners, same world positions.
//! The dedicated `break_overlay.wgsl` pipeline draws it depth `LessEqual` /
//! no-write so the crack lands on the block surface (no inflation to misalign the
//! decal at glancing angles).
//!
//! Coincident corners are *not* enough on their own: the chunk mesher flips each
//! face's triangulation diagonal per-AO (`should_flip` in `mesh::face`) while this
//! cube always splits 0->2, so the two surfaces interpolate depth a ULP apart per
//! pixel and would speckle-fight. The break pipeline therefore applies a small
//! polygon offset toward the camera (`BREAK_DEPTH_BIAS`) so the crack wins that tie
//! everywhere.
//!
//! Geometry is in WORLD space (the break pipeline's vertex shader transforms by
//! `view_proj`, like the block pipeline) and full-bright. Built into a
//! caller-owned `Vec` whose capacity is reused frame to frame.

use glam::Vec3;

use super::block_model::{push_box_faces_lit, push_cube_textured};
use super::BreakOverlayView;
use crate::atlas::Tile;
use crate::mesh::Vertex;

/// The destroy tile for crack `stage` (clamped 0..=9), as a [`Tile`].
#[inline]
fn destroy_tile(stage: u8) -> Tile {
    // `Tile::DestroyStage0..9` are contiguous and id-ordered, so the tile id is
    // `DestroyStage0 + stage`; `Tile::ALL` is id-ordered, so index it directly.
    let idx = Tile::DestroyStage0 as usize + stage.min(9) as usize;
    Tile::ALL[idx]
}

/// Build the crack overlay cube for `view` into `verts` / `indices` (cleared
/// first, capacity reused). Returns the index count (always 36, one cube). All six
/// faces use the same destroy tile so the crack reads from every angle.
///
/// The cube spans the block's exact `[block, block + 1]` cell with no inflation,
/// so each face lands on the same integer-coordinate plane the chunk mesh emitted
/// for that block. The pipeline's depth `LessEqual` + a small polygon offset
/// (`BREAK_DEPTH_BIAS`) put the crack on the surface without z-fighting (see the
/// module docs for why the offset is needed).
pub fn build_break_overlay(
    view: &BreakOverlayView,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    let tile = destroy_tile(view.stage);
    let base = Vec3::new(
        view.block.x as f32,
        view.block.y as f32,
        view.block.z as f32,
    );
    match view.block_kind.visual_aabb() {
        // A non-full-cube block (the chest) cracks over its inset visual box, so the
        // crack lands on the model rather than the empty cell faces around it.
        Some((mn, mx)) => {
            let min = base + Vec3::new(mn[0], mn[1], mn[2]);
            let max = base + Vec3::new(mx[0], mx[1], mx[2]);
            push_box_faces_lit(
                verts,
                indices,
                [tile; 6],
                min,
                max,
                super::lighting::FULL_SKYLIGHT,
            );
        }
        None => push_cube_textured(verts, indices, [tile; 3], base, 1.0),
    }
    indices.len() as u32
}

#[cfg(test)]
mod tests {
    use super::*;
    use glam::IVec3;

    #[test]
    fn destroy_tile_maps_stage_and_clamps() {
        assert_eq!(destroy_tile(0), Tile::DestroyStage0);
        assert_eq!(destroy_tile(9), Tile::DestroyStage9);
        // Out-of-range stages clamp to the last stage.
        assert_eq!(destroy_tile(42), Tile::DestroyStage9);
    }

    #[test]
    fn builds_one_coincident_cube_with_the_stage_tile() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = BreakOverlayView {
            block: IVec3::new(3, 64, -7),
            block_kind: crate::block::Block::Stone,
            stage: 4,
        };
        let n = build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert_eq!(n, 36);
        // Every face uses DestroyStage4 (tile id in bits 0..8 of packed).
        let want = Tile::DestroyStage4 as u8;
        for vert in &v {
            assert_eq!((vert.packed & 0xFF) as u8, want);
        }
        // Coincident, not inflated: the cube spans the block cell [3,4] on x
        // *exactly*, so its faces sit on the chunk mesh's faces and the crack wins
        // the depth tie via LessEqual instead of poking proud of the surface.
        let min_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert_eq!(min_x, 3.0, "cube min lands exactly on the block boundary");
        assert_eq!(max_x, 4.0, "cube max lands exactly on the block boundary");
    }

    #[test]
    fn reuses_buffers() {
        let mut v = Vec::with_capacity(32);
        let mut i = Vec::with_capacity(48);
        let cap = v.capacity();
        let view = BreakOverlayView {
            block: IVec3::ZERO,
            block_kind: crate::block::Block::Stone,
            stage: 0,
        };
        build_break_overlay(&view, &mut v, &mut i);
        build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert!(v.capacity() >= cap);
    }
}
