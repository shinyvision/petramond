use crate::mathh::{IVec3, Mat4, Vec3};

pub const MAX_SELECTION_BOXES: usize = 3;

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

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum SelectionShape {
    Box {
        origin: IVec3,
        min: Vec3,
        max: Vec3,
    },
    Posed {
        origin: IVec3,
        transform: Mat4,
        min: Vec3,
        max: Vec3,
    },
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
