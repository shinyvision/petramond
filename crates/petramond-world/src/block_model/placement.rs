//! Placement-facing transforms: how an authored model footprint maps into the world
//! under a placed [`Facing`] — the placement yaw/transform, authored-offset ↔ world-cell
//! mapping, and the oriented per-cell collision/selection bake.

use glam::{Mat4, Vec3};

use crate::block::Aabb;
use crate::facing::Facing;
use crate::mathh::IVec3;

use super::{box_corners, footprint, instance, BlockModelKind, CellInstance, OrientedCellInstance};

/// Yaw that rotates the authored model front (`-Z`, North) to `facing`.
pub fn placement_yaw(facing: Facing) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match facing {
        Facing::North => 0.0,
        Facing::South => PI,
        Facing::East => -FRAC_PI_2,
        Facing::West => FRAC_PI_2,
    }
}

/// Transform from authored FOOTPRINT space into space relative to the rotated
/// footprint's minimum corner (its base cell) for a model placed at `facing`. The
/// base itself stays an integer anchor: add it as a cell, never as a float
/// translation, so a model far from the origin keeps its sub-texel detail.
pub fn placement_transform(kind: BlockModelKind, facing: Facing) -> Mat4 {
    placement_transform_fp(footprint(kind), facing)
}

/// [`placement_transform`] with an explicit footprint instead of `footprint(kind)`. Used by
/// `ModelInstance::build` to bake the render templates: that runs INSIDE the `INSTANCES`
/// `LazyLock` init, so going through `footprint(kind)` (→ `instance(kind)`) would re-enter
/// the half-built lock and deadlock. The footprint is already known locally there.
pub fn placement_transform_fp(fp: [u8; 3], facing: Facing) -> Mat4 {
    let sx = fp[0] as f32;
    let sz = fp[2] as f32;
    let shift = match facing {
        Facing::North => Vec3::ZERO,
        Facing::South => Vec3::new(sx, 0.0, sz),
        Facing::East => Vec3::new(sz, 0.0, 0.0),
        Facing::West => Vec3::new(0.0, 0.0, sx),
    };
    Mat4::from_translation(shift) * Mat4::from_rotation_y(placement_yaw(facing))
}

/// World cell occupied by authored `offset` for a model whose rotated footprint starts at
/// `base`.
pub fn world_cell_for_offset(
    base: IVec3,
    kind: BlockModelKind,
    offset: [u8; 3],
    facing: Facing,
) -> IVec3 {
    base + cell_rel_for_offset(footprint(kind), offset, facing)
}

/// Inverse of `world_cell_for_offset`: find the rotated-footprint base from a world
/// cell and its stored authored offset.
pub fn base_from_cell(cell: IVec3, kind: BlockModelKind, offset: [u8; 3], facing: Facing) -> IVec3 {
    cell - cell_rel_for_offset(footprint(kind), offset, facing)
}

/// Placement anchor used by the player: the clicked cell is the model's near-left
/// bottom authored cell — the footprint cell nearest the placer, on their left.
/// Which authored cell that is depends on the pose: a standard authored front
/// (−Z) presents `[fp_x − 1, 0, 0]`; a back-to-front model meets the player with
/// its authored +Z face, so its near-left cell is `[0, 0, fp_z − 1]`.
pub fn base_from_front_left_anchor(anchor: IVec3, kind: BlockModelKind, facing: Facing) -> IVec3 {
    let fp = footprint(kind);
    let front_left = match super::def(kind).orientation {
        super::PlacementOrientation::BackToFront => [0, 0, fp[2].saturating_sub(1)],
        _ => [fp[0].saturating_sub(1), 0, 0],
    };
    anchor - cell_rel_for_offset(fp, front_left, facing)
}

/// Placement anchor for [`super::PlacementOrientation::Centered`] models: the clicked
/// cell is the footprint's horizontal CENTRE cell (even sizes bias toward
/// −X/−Z) on the TOP layer — a hanging fixture grows downward from the cell it
/// is placed against. No facing: centered models always store the default.
pub fn base_from_centered_anchor(anchor: IVec3, kind: BlockModelKind) -> IVec3 {
    let fp = footprint(kind);
    anchor
        - IVec3::new(
            (fp[0] as i32 - 1) / 2,
            fp[1] as i32 - 1,
            (fp[2] as i32 - 1) / 2,
        )
}

/// Occupied world cells plus their authored offsets for this oriented model placement.
pub fn oriented_footprint_cells(
    base: IVec3,
    kind: BlockModelKind,
    facing: Facing,
) -> Vec<(IVec3, [u8; 3])> {
    instance(kind)
        .cells
        .iter()
        .map(|c| {
            (
                world_cell_for_offset(base, kind, c.offset, facing),
                c.offset,
            )
        })
        .collect()
}

fn cell_rel_for_offset(footprint: [u8; 3], offset: [u8; 3], facing: Facing) -> IVec3 {
    let sx = footprint[0] as i32;
    let sz = footprint[2] as i32;
    let dx = offset[0] as i32;
    let dy = offset[1] as i32;
    let dz = offset[2] as i32;
    match facing {
        Facing::North => IVec3::new(dx, dy, dz),
        Facing::South => IVec3::new(sx - 1 - dx, dy, sz - 1 - dz),
        Facing::East => IVec3::new(sz - 1 - dz, dy, dx),
        Facing::West => IVec3::new(dz, dy, sx - 1 - dx),
    }
}

pub(super) fn oriented_cell_instance(
    cell: &CellInstance,
    footprint: [u8; 3],
    facing: Facing,
) -> OrientedCellInstance {
    let rel = cell_rel_for_offset(footprint, cell.offset, facing);
    let relf = Vec3::new(rel.x as f32, rel.y as f32, rel.z as f32);
    let collision = cell
        .collision
        .iter()
        .map(|b| local_aabb_to_footprint(b, cell.offset))
        .map(|b| localize_aabb(transform_footprint_aabb(&b, footprint, facing), relf))
        .collect();

    let (selection_min, selection_max) = if cell.selection_min == cell.selection_max {
        ([0.0; 3], [0.0; 3])
    } else {
        let b = Aabb {
            min: cell.selection_min,
            max: cell.selection_max,
        };
        let b = local_aabb_to_footprint(&b, cell.offset);
        let b = localize_aabb(transform_footprint_aabb(&b, footprint, facing), relf);
        (b.min, b.max)
    };

    OrientedCellInstance {
        offset: cell.offset,
        collision,
        selection_min,
        selection_max,
    }
}

fn local_aabb_to_footprint(b: &Aabb, offset: [u8; 3]) -> Aabb {
    Aabb {
        min: [
            b.min[0] + offset[0] as f32,
            b.min[1] + offset[1] as f32,
            b.min[2] + offset[2] as f32,
        ],
        max: [
            b.max[0] + offset[0] as f32,
            b.max[1] + offset[1] as f32,
            b.max[2] + offset[2] as f32,
        ],
    }
}

fn localize_aabb(b: Aabb, rel: Vec3) -> Aabb {
    let mut out = Aabb {
        min: [b.min[0] - rel.x, b.min[1] - rel.y, b.min[2] - rel.z],
        max: [b.max[0] - rel.x, b.max[1] - rel.y, b.max[2] - rel.z],
    };
    for i in 0..3 {
        out.min[i] = out.min[i].clamp(0.0, 1.0);
        out.max[i] = out.max[i].clamp(0.0, 1.0);
    }
    out
}

fn transform_footprint_aabb(b: &Aabb, footprint: [u8; 3], facing: Facing) -> Aabb {
    let mut mn = Vec3::splat(f32::INFINITY);
    let mut mx = Vec3::splat(f32::NEG_INFINITY);
    for p in box_corners(Vec3::from(b.min), Vec3::from(b.max)) {
        let q = transform_footprint_point(p, footprint, facing);
        mn = mn.min(q);
        mx = mx.max(q);
    }
    Aabb {
        min: mn.to_array(),
        max: mx.to_array(),
    }
}

pub fn transform_footprint_point(p: Vec3, footprint: [u8; 3], facing: Facing) -> Vec3 {
    let sx = footprint[0] as f32;
    let sz = footprint[2] as f32;
    match facing {
        Facing::North => p,
        Facing::South => Vec3::new(sx - p.x, p.y, sz - p.z),
        Facing::East => Vec3::new(sz - p.z, p.y, p.x),
        Facing::West => Vec3::new(p.z, p.y, sx - p.x),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::block_model::PlacementOrientation;

    #[test]
    fn placement_transform_maps_authored_front_to_facing() {
        let authored_front = Vec3::NEG_Z;
        for (facing, want) in [
            (Facing::North, Vec3::NEG_Z),
            (Facing::South, Vec3::Z),
            (Facing::East, Vec3::X),
            (Facing::West, Vec3::NEG_X),
        ] {
            let got =
                Mat4::from_rotation_y(placement_yaw(facing)).transform_vector3(authored_front);
            assert!(
                got.distance(want) < 1e-5,
                "{facing:?} maps authored front to {got:?}, want {want:?}"
            );
        }
    }

    /// A back-to-front model stores the half-turned facing, so its authored +Z
    /// working face — not the convention's −Z — ends up on the placer's side.
    #[test]
    fn a_back_to_front_model_meets_the_player_with_its_authored_pos_z_face() {
        assert_eq!(
            PlacementOrientation::BackToFront.apply(Facing::North),
            Facing::South
        );
        assert_eq!(
            PlacementOrientation::BackToFront.apply(Facing::East),
            Facing::West
        );
        // The chiseling station is the shipped back-to-front row: a player
        // looking along +Z stores South, whose transform maps the authored +Z
        // boundary onto the near (z = 0) footprint edge — the face the player
        // sees — while `left_to_right` would have buried it on the far side.
        let kind = BlockModelKind::ChiselingStation;
        let fp = footprint(kind);
        assert_eq!(
            super::super::def(kind).orientation,
            PlacementOrientation::BackToFront
        );
        let player_side =
            transform_footprint_point(Vec3::new(0.0, 0.0, fp[2] as f32), fp, Facing::South);
        assert_eq!(
            player_side.z, 0.0,
            "authored +Z face must land nearest the player"
        );
        let far_side = transform_footprint_point(Vec3::new(0.0, 0.0, 0.0), fp, Facing::South);
        assert_eq!(far_side.z, fp[2] as f32);
    }

    /// The clicked cell is the footprint's near-left bottom cell under BOTH
    /// poses: min world Z (nearest the placer) and max world X (their left),
    /// with the model growing away from them. Each pose uses its shipped row:
    /// the workbench for the standard authored front, the chiseling station
    /// for back-to-front.
    #[test]
    fn the_anchor_cell_is_the_footprints_near_left_cell_for_both_poses() {
        let anchor = IVec3::new(50, 40, 30);
        for (kind, facing) in [
            (BlockModelKind::FurnitureWorkbench, Facing::North),
            (BlockModelKind::ChiselingStation, Facing::South),
        ] {
            let base = base_from_front_left_anchor(anchor, kind, facing);
            let cells: Vec<IVec3> = oriented_footprint_cells(base, kind, facing)
                .into_iter()
                .map(|(cell, _)| cell)
                .collect();
            assert!(cells.contains(&anchor), "{kind:?}: anchor not in footprint");
            let min_z = cells.iter().map(|c| c.z).min().unwrap();
            let max_x = cells.iter().map(|c| c.x).max().unwrap();
            assert_eq!(
                (anchor.x, anchor.z),
                (max_x, min_z),
                "{kind:?}: anchor must be the footprint's near-left corner"
            );
            // And the model never grows toward the player.
            assert_eq!(min_z, anchor.z, "{kind:?}: footprint behind the click");
            assert_eq!(base.y, anchor.y);
        }
    }
}
