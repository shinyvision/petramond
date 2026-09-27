use super::*;

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
