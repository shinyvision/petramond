//! Selection outlines: the wireframe the targeting code hands the renderer
//! for the block under the crosshair. Built from the same shape facets the
//! hit test reads, so it lives with the block domain, not in the foundation
//! math crate.

use crate::mathh::{IVec3, Mat4, Vec3};

/// How many boxes a [`SelectionShape::Boxes`] outline can trace before the
/// targeting code falls back to their union box.
pub const MAX_SELECTION_BOXES: usize = 3;

/// A fixed-capacity list of cell-local boxes (the first `len` are live).
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SelectionBoxes {
    pub boxes: [(Vec3, Vec3); MAX_SELECTION_BOXES],
    pub len: u8,
}

impl SelectionBoxes {
    #[inline]
    pub fn iter(self) -> impl Iterator<Item = (Vec3, Vec3)> {
        self.boxes.into_iter().take(self.len as usize)
    }
}

/// A selection outline, anchored at an integer cell: every variant's geometry
/// is local to `origin`, so the outline stays exact however far out it is.
#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SelectionShape {
    Box {
        origin: IVec3,
        min: Vec3,
        max: Vec3,
    },
    /// A box carried through a model transform: the outline's corners are
    /// `transform`-mapped from the box's model-local `[min, max]` and offset
    /// by `origin` (the cell), so the wireframe traces geometry drawn posed —
    /// a wall torch's tilted pole.
    Posed {
        origin: IVec3,
        transform: Mat4,
        min: Vec3,
        max: Vec3,
    },
    /// A shape made from a small fixed list of boxes local to `origin`, so the
    /// outline traces the resolved volume (a stair's steps, a fence's arms)
    /// instead of one full block cube.
    Boxes {
        origin: IVec3,
        boxes: SelectionBoxes,
    },
}

impl SelectionShape {
    pub fn full_block(block: IVec3) -> Self {
        Self::Box {
            origin: block,
            min: Vec3::ZERO,
            max: Vec3::ONE,
        }
    }
}
