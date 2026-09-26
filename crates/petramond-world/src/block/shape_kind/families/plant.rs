//! Cross and crop plants: two diagonal planes, drawn by the plant emitter.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// The cross billboard plant (grass/fern/flower). No collision; its item is a
/// flat sprite of the top tile.
pub struct CrossFamily;

impl ShapeSim for CrossFamily {
    fn collision_state_free(&self) -> bool {
        true
    }

    fn nav_follows_row(&self) -> bool {
        true
    }
}

impl ShapeRender for CrossFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Plant(PlantPlanes::Cross)
    }

    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

/// The planted-crop lattice — like [`CrossFamily`] for item purposes.
pub struct CropFamily;

impl ShapeSim for CropFamily {
    fn collision_state_free(&self) -> bool {
        true
    }

    fn nav_follows_row(&self) -> bool {
        true
    }
}

impl ShapeRender for CropFamily {
    fn mesh_emitter(&self, _p: &ShapeParams) -> MeshEmitter {
        MeshEmitter::Plant(PlantPlanes::Crop)
    }

    fn item_render(&self, _p: &ShapeParams, block: Block) -> ItemRender {
        ItemRender::Tile(block.tiles()[0])
    }
}

impl ShapePlacement for CrossFamily {}

impl ShapePlacement for CropFamily {}
