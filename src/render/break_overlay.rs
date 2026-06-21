//! Block-break crack overlay geometry.
//!
//! From a [`BreakOverlayView`] (target block + crack stage 0..9), builds the
//! six faces of that block's cube, each textured with the matching
//! `Tile::DestroyStage{stage}` and slightly **inflated** so the crack sits just
//! proud of the block face (no z-fighting). The dedicated `break_overlay.wgsl`
//! pipeline samples that grayscale crack tile and alpha-blends it over the world.
//!
//! Geometry is in WORLD space (the break pipeline's vertex shader transforms by
//! `view_proj`, like the block pipeline) and full-bright. Built into a
//! caller-owned `Vec` whose capacity is reused frame to frame.

use glam::Vec3;

use super::block_model::push_cube_textured;
use super::BreakOverlayView;
use crate::atlas::Tile;
use crate::mesh::Vertex;

/// How far (metres) the crack cube is inflated past the block faces so the overlay
/// wins the depth `LessEqual` test against the block surface without z-fighting.
const INFLATE: f32 = 0.003;

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
pub fn build_break_overlay(
    view: &BreakOverlayView,
    verts: &mut Vec<Vertex>,
    indices: &mut Vec<u32>,
) -> u32 {
    verts.clear();
    indices.clear();
    let tile = destroy_tile(view.stage);
    // Origin = block min corner minus the inflation; size = 1 + 2*inflate so the
    // cube straddles the block symmetrically.
    let origin = Vec3::new(
        view.block.x as f32 - INFLATE,
        view.block.y as f32 - INFLATE,
        view.block.z as f32 - INFLATE,
    );
    push_cube_textured(verts, indices, [tile; 3], origin, 1.0 + 2.0 * INFLATE);
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
    fn builds_one_inflated_cube_with_the_stage_tile() {
        let mut v = Vec::new();
        let mut i = Vec::new();
        let view = BreakOverlayView {
            block: IVec3::new(3, 64, -7),
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
        // Inflated: the cube spans slightly beyond [3,4] on x.
        let min_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::INFINITY, f32::min);
        let max_x = v
            .iter()
            .map(|vert| vert.pos[0])
            .fold(f32::NEG_INFINITY, f32::max);
        assert!(
            min_x < 3.0 && max_x > 4.0,
            "cube should be inflated past the block"
        );
    }

    #[test]
    fn reuses_buffers() {
        let mut v = Vec::with_capacity(32);
        let mut i = Vec::with_capacity(48);
        let cap = v.capacity();
        let view = BreakOverlayView {
            block: IVec3::ZERO,
            stage: 0,
        };
        build_break_overlay(&view, &mut v, &mut i);
        build_break_overlay(&view, &mut v, &mut i);
        assert_eq!(v.len(), 24);
        assert!(v.capacity() >= cap);
    }
}
