//! Torch orientation: how a placed torch sits in its cell.
//!
//! A torch has no inventory or tick (unlike a furnace/chest), so its only
//! per-instance state is *which way it is mounted*. That orientation is stored in
//! the owning chunk's torch map (see [`Chunk::torches`](crate::chunk::Chunk)),
//! keyed by local block index just like the furnace/chest maps. This module owns
//! the orientation enum plus the single shared model transform that BOTH the
//! in-world mesher ([`mesh::torch`](crate::mesh)) and the selection outline
//! ([`render`](crate::render)) build from — so the outline traces the rendered
//! pole exactly, by construction.

use crate::mathh::{IVec3, Mat4, Vec3};

/// Half-width of the square pole, in cell units (the pole is `2/16` across, like a
/// Minecraft torch). Shared by the model and the outline.
pub const POLE_HALF: f32 = 1.0 / 16.0;
/// Height of the pole from its base, in cell units (`10/16`, the torch's textured
/// length). The flame cap sits at the top.
pub const POLE_HEIGHT: f32 = 10.0 / 16.0;
/// A wall torch's base pivots this far up its wall before leaning out, so the lit
/// tip clears the block it is mounted on (matches the vanilla wall-torch raise).
const WALL_PIVOT_Y: f32 = 3.5 / 16.0;
/// Lean of a wall torch from vertical, in radians (22.5°, per the feature spec).
const WALL_TILT: f32 = 22.5 * std::f32::consts::PI / 180.0;

/// Warm hue that emitter (torch/furnace) block-light multiplies into a surface's
/// tint. The cool channels sit below 1 so a lit surface reads orange-yellow; red
/// stays full so the light only warms, never darkens. Shared by the chunk mesher
/// (static blocks) and the render side (hand, held item, particles) so warm light
/// looks the same everywhere.
pub const TORCH_TINT: [f32; 3] = [1.0, 0.82, 0.52];
/// How strongly a fully block-lit, sky-dark surface warms toward [`TORCH_TINT`]
/// (0 = none, 1 = full). Kept modest for a "subtle yellow glow".
pub const TORCH_WARM_STRENGTH: f32 = 0.6;

/// Multiply `base` toward [`TORCH_TINT`] by `warm` (0..1): `0` leaves it unchanged,
/// `1` fully warms it. Tints a surface yellow in proportion to torch/furnace light.
#[inline]
pub fn warm_tint(base: [f32; 3], warm: f32) -> [f32; 3] {
    let w = warm.clamp(0.0, 1.0);
    [
        base[0] * (1.0 - w + w * TORCH_TINT[0]),
        base[1] * (1.0 - w + w * TORCH_TINT[1]),
        base[2] * (1.0 - w + w * TORCH_TINT[2]),
    ]
}

/// The warm-tint amount (0..[`TORCH_WARM_STRENGTH`]) for a cell with normalized
/// skylight `sky01` and block-light `block01` (each `0..1`): proportional to how
/// much block-light reaches it AND to how UNlit-by-sky it is — a fully skylit cell
/// takes no yellow. The single warmth formula shared by the mesher and render.
#[inline]
pub fn warm_amount(sky01: f32, block01: f32) -> f32 {
    block01 * (1.0 - sky01) * TORCH_WARM_STRENGTH
}

/// How a placed torch is oriented in its cell. A `Floor` torch stands vertical and
/// centered on the block below it; a wall torch is mounted on one side of the cell
/// and leans away from that wall. The four wall variants name the horizontal
/// direction the torch LEANS toward — equal to the face normal of the block it was
/// placed against (pointing away from the wall) — matching the cardinal convention
/// the furnace [`Facing`](crate::furnace::Facing) uses. Stored per-cell in the
/// owning chunk's torch map.
#[repr(u8)]
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq)]
pub enum TorchPlacement {
    #[default]
    Floor = 0,
    /// Leans toward `-Z` (mounted on the `+Z` wall).
    North = 1,
    /// Leans toward `+Z`.
    South = 2,
    /// Leans toward `-X`.
    West = 3,
    /// Leans toward `+X`.
    East = 4,
}

impl TorchPlacement {
    /// Stable byte for the save codec.
    #[inline]
    pub fn to_u8(self) -> u8 {
        self as u8
    }

    /// Inverse of [`to_u8`](Self::to_u8); unknown bytes fall back to `Floor`.
    #[inline]
    pub fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::North,
            2 => Self::South,
            3 => Self::West,
            4 => Self::East,
            _ => Self::Floor,
        }
    }

    /// The placement for a torch put against the face whose outward normal (pointing
    /// back toward the player, as [`RaycastHit`](crate::player) reports it) is
    /// `normal`: `+Y` → a `Floor` torch on the block below, a horizontal normal → a
    /// wall torch leaning that way. A `-Y` normal (the underside of a block) or a
    /// zero normal yields `None` — torches don't hang from ceilings.
    pub fn from_place_normal(normal: IVec3) -> Option<Self> {
        match (normal.x, normal.y, normal.z) {
            (0, 1, 0) => Some(Self::Floor),
            (1, 0, 0) => Some(Self::East),
            (-1, 0, 0) => Some(Self::West),
            (0, 0, 1) => Some(Self::South),
            (0, 0, -1) => Some(Self::North),
            _ => None,
        }
    }

    /// Whether this is a wall mount (vs a floor torch).
    #[inline]
    pub fn is_wall(self) -> bool {
        !matches!(self, Self::Floor)
    }

    /// The block the torch needs as support: the cell directly below for a `Floor`
    /// torch, or the wall cell behind it for a wall torch. Given the torch's own
    /// cell `pos`, returns the support cell's position.
    pub fn support_cell(self, pos: IVec3) -> IVec3 {
        match self.lean() {
            // A wall torch is supported by the wall behind it (opposite its lean).
            Some(dir) => pos - dir,
            // A floor torch rests on the block below.
            None => pos - IVec3::new(0, 1, 0),
        }
    }

    /// The outward normal of the support face this torch is mounted to.
    pub fn support_normal(self) -> IVec3 {
        self.lean().unwrap_or(IVec3::new(0, 1, 0))
    }

    /// The horizontal unit direction a wall torch leans toward (away from its wall);
    /// `None` for a floor torch.
    fn lean(self) -> Option<IVec3> {
        match self {
            Self::Floor => None,
            Self::North => Some(IVec3::new(0, 0, -1)),
            Self::South => Some(IVec3::new(0, 0, 1)),
            Self::West => Some(IVec3::new(-1, 0, 0)),
            Self::East => Some(IVec3::new(1, 0, 0)),
        }
    }

    /// Transform mapping the torch's LOCAL model space — a pole standing on its base
    /// at the origin, `±POLE_HALF` across in X/Z and `0..POLE_HEIGHT` tall — into
    /// its CELL-local space (`0..1` per axis). A floor torch is just centered on the
    /// block; a wall torch pivots at its base against the wall and leans
    /// [`WALL_TILT`] out. The caller adds the cell's world origin. The mesher and
    /// the selection outline both build from this, so the outline hugs the model.
    pub fn model_transform(self) -> Mat4 {
        match self.lean() {
            None => Mat4::from_translation(Vec3::new(0.5, 0.0, 0.5)),
            Some(dir) => {
                let d = Vec3::new(dir.x as f32, 0.0, dir.z as f32);
                // Pivot at the base, against the wall behind the torch (the `-d`
                // face of the cell), raised so the tip clears the support block.
                let pivot = Vec3::new(0.5 - 0.5 * d.x, WALL_PIVOT_Y, 0.5 - 0.5 * d.z);
                // Tilt the local +Y up-axis toward the lean direction `d`: the
                // rotation axis is horizontal and perpendicular to `d`.
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
        // Unknown bytes fall back to Floor rather than panicking.
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
        // A ceiling (downward normal) and a degenerate zero normal are not placeable.
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
        // An east-leaning torch is mounted on the wall to its west.
        assert_eq!(TorchPlacement::East.support_cell(p), IVec3::new(4, 10, -3));
        assert_eq!(TorchPlacement::North.support_cell(p), IVec3::new(5, 10, -2));
    }

    #[test]
    fn floor_torch_base_is_centered_on_the_cell_floor() {
        // The local base centre (origin) maps to the centre of the cell floor.
        let m = TorchPlacement::Floor.model_transform();
        let base = m.transform_point3(Vec3::ZERO);
        assert!((base - Vec3::new(0.5, 0.0, 0.5)).length() < 1e-6);
    }

    #[test]
    fn wall_torch_leans_its_tip_toward_the_lean_direction() {
        // An east-leaning torch: its tip should sit further in +X than its base, and
        // its base should be at the west wall (x ~ 0).
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
