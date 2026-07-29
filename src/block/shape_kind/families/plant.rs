//! Cross and crop plants: two diagonal planes, drawn by the plant emitter.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// The cross billboard plant (grass/fern/flower). No collision; its item is a
/// flat sprite of the top tile.
pub struct CrossFamily;

impl ShapeSim for CrossFamily {}

impl ShapeRender for CrossFamily {
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

/// The planted-crop lattice — like [`CrossFamily`] for item purposes.
pub struct CropFamily;

impl ShapeSim for CropFamily {}

impl ShapeRender for CropFamily {
    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

impl ShapePlacement for CrossFamily {}

impl ShapePlacement for CropFamily {}
