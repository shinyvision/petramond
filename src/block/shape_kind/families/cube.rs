//! The plain full cube: every facet is a trait default except the face answer.
//!
//! Sim, render, and placement for this family live together here; the shared
//! seam helpers and the singleton table stay in the parent.

use super::*;

/// Plain full cube — every facet is the trait default except the face
/// answer: a cube's faces are geometrically complete (material gates are the
/// ASKER's rule and apply to `Cube` answers only, so air and glass answering
/// here is safe).
pub struct CubeFamily;

impl ShapeSim for CubeFamily {
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
