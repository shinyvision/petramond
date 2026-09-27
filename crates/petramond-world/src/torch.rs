use crate::facing::Facing;
use crate::mathh::{IVec3, Mat4, Vec3};

pub const POLE_HALF: f32 = 1.0 / 16.0;
pub const POLE_HEIGHT: f32 = 10.0 / 16.0;
const WALL_PIVOT_Y: f32 = 3.5 / 16.0;
const WALL_TILT: f32 = 22.5 * std::f32::consts::PI / 180.0;

crate::wire_enum::wire_enum! {
    pub enum TorchPlacement: u8 {
        Floor = 0,
        North = 1,
        South = 2,
        West = 3,
        East = 4,
    }
    default Floor
}

impl crate::block::CellView for TorchPlacement {
    fn owns(block: crate::block::Block) -> bool {
        crate::block::shape_kind_families::is_torch(block)
    }
    fn from_cell(s: crate::block::ShapeState) -> Self {
        TorchPlacement::from_u8(s.byte(0))
    }
}
impl crate::block::CellCodec for TorchPlacement {
    fn to_cell(&self) -> crate::block::ShapeState {
        crate::block::ShapeState::new(&[self.to_u8()])
    }
}

impl TorchPlacement {
    pub fn from_place_normal(normal: IVec3) -> Option<Self> {
        if normal == IVec3::new(0, 1, 0) {
            return Some(Self::Floor);
        }
        Facing::from_horizontal_normal(normal).map(Self::from_wall_facing)
    }

    #[inline]
    pub fn is_wall(self) -> bool {
        !matches!(self, Self::Floor)
    }

    pub fn support_cell(self, pos: IVec3) -> IVec3 {
        match self.lean() {
            Some(dir) => pos - dir,
            None => pos - IVec3::new(0, 1, 0),
        }
    }

    pub fn support_normal(self) -> IVec3 {
        self.lean().unwrap_or(IVec3::new(0, 1, 0))
    }

    fn lean(self) -> Option<IVec3> {
        self.wall_facing().map(Facing::dir)
    }

    fn wall_facing(self) -> Option<Facing> {
        match self {
            Self::Floor => None,
            Self::North => Some(Facing::North),
            Self::South => Some(Facing::South),
            Self::West => Some(Facing::West),
            Self::East => Some(Facing::East),
        }
    }

    fn from_wall_facing(facing: Facing) -> Self {
        match facing {
            Facing::North => Self::North,
            Facing::South => Self::South,
            Facing::West => Self::West,
            Facing::East => Self::East,
        }
    }

    pub fn model_transform(self) -> Mat4 {
        match self.lean() {
            None => Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5)),
            Some(dir) => {
                let d = Vec3::new(dir.x as f32, 0.0, dir.z as f32);
                let pivot = Vec3::new(0.5 - 0.5 * d.x, WALL_PIVOT_Y, 0.5 - 0.5 * d.z);
                let axis = Vec3::Y.cross(d).normalize();
                Mat4::from_translation(pivot) * Mat4::from_axis_angle(axis, WALL_TILT)
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn placement_byte_roundtrips() {
        for p in [
            TorchPlacement::Floor,
            TorchPlacement::North,
            TorchPlacement::South,
            TorchPlacement::West,
            TorchPlacement::East,
        ] {
            assert_eq!(TorchPlacement::from_u8(p.to_u8()), p);
        }
        assert_eq!(TorchPlacement::from_u8(200), TorchPlacement::Floor);
    }

    #[test]
    fn place_normal_maps_faces_to_mounts() {
        use TorchPlacement::*;
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(0, 1, 0)),
            Some(Floor)
        );
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(1, 0, 0)),
            Some(East)
        );
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(-1, 0, 0)),
            Some(West)
        );
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(0, 0, 1)),
            Some(South)
        );
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(0, 0, -1)),
            Some(North)
        );
        assert_eq!(
            TorchPlacement::from_place_normal(IVec3::new(0, -1, 0)),
            None
        );
        assert_eq!(TorchPlacement::from_place_normal(IVec3::ZERO), None);
    }

    #[test]
    fn support_is_below_for_floor_and_behind_for_walls() {
        let p = IVec3::new(5, 10, -3);
        assert_eq!(TorchPlacement::Floor.support_cell(p), IVec3::new(5, 9, -3));
        assert_eq!(TorchPlacement::East.support_cell(p), IVec3::new(4, 10, -3));
        assert_eq!(TorchPlacement::North.support_cell(p), IVec3::new(5, 10, -2));
    }

    #[test]
    fn floor_torch_base_is_centered_on_the_cell_floor() {
        let m = TorchPlacement::Floor.model_transform();
        let base = m.transform_point3(Vec3::ZERO);
        assert!((base - Vec3::new(0.5, 0.0, 0.5)).length() < 1e-6);
    }

    #[test]
    fn wall_torch_leans_its_tip_toward_the_lean_direction() {
        let m = TorchPlacement::East.model_transform();
        let base = m.transform_point3(Vec3::ZERO);
        let tip = m.transform_point3(Vec3::new(0.0, POLE_HEIGHT, 0.0));
        assert!(
            base.x.abs() < 1e-6,
            "base sits on the west wall, got x={}",
            base.x
        );
        assert!(tip.x > base.x + 0.1, "tip leans east of the base");
        assert!(tip.y > base.y + 0.5, "tip is still mostly above the base");
    }
}
