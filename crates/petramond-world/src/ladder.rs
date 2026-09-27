use crate::block::Aabb;
use crate::facing::Facing;
use crate::mathh::IVec3;

pub const THICKNESS: f32 = 1.0 / 16.0;

#[inline]
pub fn support_cell(pos: IVec3, facing: Facing) -> IVec3 {
    pos - facing.dir()
}

static PANEL_NORTH: [Aabb; 1] = [Aabb {
    min: [0.0, 0.0, 1.0 - THICKNESS],
    max: [1.0, 1.0, 1.0],
}];
static PANEL_SOUTH: [Aabb; 1] = [Aabb {
    min: [0.0, 0.0, 0.0],
    max: [1.0, 1.0, THICKNESS],
}];
static PANEL_WEST: [Aabb; 1] = [Aabb {
    min: [1.0 - THICKNESS, 0.0, 0.0],
    max: [1.0, 1.0, 1.0],
}];
static PANEL_EAST: [Aabb; 1] = [Aabb {
    min: [0.0, 0.0, 0.0],
    max: [THICKNESS, 1.0, 1.0],
}];

pub fn collision_boxes(facing: Facing) -> &'static [Aabb] {
    match facing {
        Facing::North => &PANEL_NORTH,
        Facing::South => &PANEL_SOUTH,
        Facing::West => &PANEL_WEST,
        Facing::East => &PANEL_EAST,
    }
}

pub fn panel_box(facing: Facing, thickness: f32, height: f32) -> Aabb {
    match facing {
        Facing::North => Aabb {
            min: [0.0, 0.0, 1.0 - thickness],
            max: [1.0, height, 1.0],
        },
        Facing::South => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, height, thickness],
        },
        Facing::West => Aabb {
            min: [1.0 - thickness, 0.0, 0.0],
            max: [1.0, height, 1.0],
        },
        Facing::East => Aabb {
            min: [0.0, 0.0, 0.0],
            max: [thickness, height, 1.0],
        },
    }
}

static PANEL_INTERN: std::sync::Mutex<Vec<PanelIntern>> = std::sync::Mutex::new(Vec::new());

type PanelIntern = ((u8, u32, u32), &'static [Aabb]);

pub fn collision_boxes_dim(facing: Facing, thickness: f32, height: f32) -> &'static [Aabb] {
    if thickness == THICKNESS && height == 1.0 {
        return collision_boxes(facing);
    }
    let key = (facing.to_u8(), thickness.to_bits(), height.to_bits());
    let mut intern = PANEL_INTERN.lock().expect("panel intern");
    if let Some(&(_, boxes)) = intern.iter().find(|(k, _)| *k == key) {
        return boxes;
    }
    let leaked: &'static [Aabb] =
        Box::leak(vec![panel_box(facing, thickness, height)].into_boxed_slice());
    intern.push((key, leaked));
    leaked
}

pub fn panel_aabb_dim(facing: Facing, thickness: f32, height: f32) -> ([f32; 3], [f32; 3]) {
    let b = panel_box(facing, thickness, height);
    (b.min, b.max)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn support_is_the_wall_behind_the_panel() {
        let p = IVec3::new(5, 10, -3);
        assert_eq!(support_cell(p, Facing::East), IVec3::new(4, 10, -3));
        assert_eq!(support_cell(p, Facing::North), IVec3::new(5, 10, -2));
    }

    #[test]
    fn panel_hugs_the_supporting_wall() {
        let t = THICKNESS;
        let aabb = |f| panel_aabb_dim(f, THICKNESS, 1.0);
        assert_eq!(aabb(Facing::East), ([0.0, 0.0, 0.0], [t, 1.0, 1.0]));
        assert_eq!(aabb(Facing::West), ([1.0 - t, 0.0, 0.0], [1.0, 1.0, 1.0]));
        assert_eq!(aabb(Facing::South), ([0.0, 0.0, 0.0], [1.0, 1.0, t]));
        assert_eq!(aabb(Facing::North), ([0.0, 0.0, 1.0 - t], [1.0, 1.0, 1.0]));
    }

    #[test]
    fn parameterized_panel_thickens_and_shortens_the_box() {
        let boxes = collision_boxes_dim(Facing::East, 4.0 / 16.0, 12.0 / 16.0);
        assert_eq!(boxes.len(), 1);
        assert_eq!(boxes[0].min, [0.0, 0.0, 0.0]);
        assert_eq!(boxes[0].max, [4.0 / 16.0, 12.0 / 16.0, 1.0]);
        assert!(std::ptr::eq(
            collision_boxes_dim(Facing::East, THICKNESS, 1.0),
            collision_boxes(Facing::East)
        ));
        assert!(std::ptr::eq(
            collision_boxes_dim(Facing::West, 5.0 / 16.0, 1.0),
            collision_boxes_dim(Facing::West, 5.0 / 16.0, 1.0)
        ));
    }
}
