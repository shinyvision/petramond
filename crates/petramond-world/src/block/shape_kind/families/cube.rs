use super::*;

pub struct CubeFamily;

impl ShapeSim for CubeFamily {
    fn collision_state_free(&self) -> bool {
        true
    }

    fn nav_follows_row(&self) -> bool {
        true
    }

    fn accepts_row_uv_rotation(&self) -> bool {
        true
    }

    fn hosts_fluid(&self) -> bool {
        true
    }

    fn full_face(
        &self,
        _p: &ShapeParams,
        _nb: &dyn ShapeNeighborhood,
        _pos: IVec3,
        _b: Block,
        _dir: IVec3,
    ) -> Option<crate::block::shape_kind::facets::FullFace> {
        Some(crate::block::shape_kind::facets::FullFace::Cube)
    }
}

impl ShapeRender for CubeFamily {}

impl ShapePlacement for CubeFamily {}
