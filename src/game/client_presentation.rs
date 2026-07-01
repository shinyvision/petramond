//! Game-owned client presentation/activity helpers.
//!
//! These methods expose neutral animation, activity, light, and mesh-budget policy
//! to sibling game modules without moving renderer DTOs into `Game`.

use crate::camera::Frustum;
use crate::mathh::{voxel_at, IVec3, Vec3};

use super::{tick, Game};

/// Chest-lid open/close speed (fraction per second)
const CHEST_LID_SPEED: f32 = 3.5;

/// Door swing open/close speed (fraction per second). A touch slower than the chest
/// lid so the 90 degree swing reads as a deliberate door, not a snap.
const DOOR_SWING_SPEED: f32 = 4.5;

/// How near (blocks) a mob/dropped item must be, and it must also be in the camera
/// frustum, for its animation to force a redraw. Past this it's too small on screen to
/// read, so it keeps simulating but doesn't hold the frame rate up while the player idles.
const ENTITY_ACTIVITY_RANGE: f32 = 50.0;

impl Game {
    /// The transient open progress (`0.0` closed .. `1.0` open) of the chest at
    /// `pos`, or `0.0` if it isn't tracked. The presentation snapshot reads this
    /// to bake the chest's lid hinge; the easing/animation lives in
    /// [`advance_chest_lids`](Self::advance_chest_lids).
    #[inline]
    pub(super) fn chest_lid_angle(&self, pos: IVec3) -> f32 {
        self.chest_lids.get(&pos).copied().unwrap_or(0.0)
    }

    /// Advance the transient chest-lid animation by `dt`: the open chest's lid eases
    /// toward fully open, every other tracked lid toward closed, and lids that reach
    /// closed (and aren't the open chest) are dropped. The open/closed target is
    /// derived from the menu's edit target (the open chest's position), so the lid
    /// follows the GUI being open, purely client-side, never saved.
    pub(super) fn advance_chest_lids(&mut self, dt: f32) {
        let step = (dt * CHEST_LID_SPEED).clamp(0.0, 1.0);
        let open = self.menu.target().open_chest();
        // Ensure the open chest is tracked so it animates from closed on the first frame.
        if let Some(pos) = open {
            self.chest_lids.entry(pos).or_insert(0.0);
        }
        self.chest_lids.retain(|&pos, lid| {
            let target = if Some(pos) == open { 1.0 } else { 0.0 };
            if *lid < target {
                *lid = (*lid + step).min(target);
            } else if *lid > target {
                *lid = (*lid - step).max(target);
            }
            // Keep while still animating, or while it is the open chest.
            *lid > f32::EPSILON || Some(pos) == open
        });
    }

    /// The transient swing angle (`0.0` closed .. `1.0` open) of the door whose LOWER
    /// cell is `lower`. While a door is mid-swing the eased value is read from
    /// [`door_swings`](Self::door_swings); once it settles the entry is dropped and the
    /// door rests at its logical open state (read straight from the door map). The
    /// presentation snapshot calls this per visible door to bake its hinge.
    #[inline]
    pub(super) fn door_swing_angle(&self, lower: IVec3) -> f32 {
        if let Some(&a) = self.door_swings.get(&lower) {
            return a;
        }
        // Not animating: rest at the door's logical state.
        match self.world.door_state_at(lower.x, lower.y, lower.z) {
            Some(s) if s.open => 1.0,
            _ => 0.0,
        }
    }

    /// Advance the transient door-swing animation by `dt`: each tracked door eases
    /// toward its current logical open state (flipped on the tick by [`World::toggle_door`]),
    /// and a door that reaches its target is dropped (it then rests at that state). Purely
    /// client-side, never saved, like [`advance_chest_lids`](Self::advance_chest_lids).
    pub(super) fn advance_door_swings(&mut self, dt: f32) {
        let step = (dt * DOOR_SWING_SPEED).clamp(0.0, 1.0);
        self.door_swings.retain(|&lower, angle| {
            let target = match self.world.door_state_at(lower.x, lower.y, lower.z) {
                Some(s) if s.open => 1.0,
                Some(_) => 0.0,
                // The door was removed while swinging: stop tracking it.
                None => return false,
            };
            if *angle < target {
                *angle = (*angle + step).min(target);
            } else if *angle > target {
                *angle = (*angle - step).max(target);
            }
            // Keep only while still travelling toward the target.
            (*angle - target).abs() > f32::EPSILON
        });
    }

    /// Fraction (`0..1`) into the next fixed tick, the blend factor the scene uses to
    /// interpolate each entity's render pose between its previous and current tick, so the
    /// mobs and dropped items (which simulate at 20 TPS) move smoothly at any frame rate.
    #[inline]
    pub(super) fn tick_alpha(&self) -> f32 {
        (self.tick_accumulator / tick::TICK_DT).clamp(0.0, 1.0)
    }

    /// Whether anything on screen is currently moving or pending, so the app shell knows
    /// this frame would differ from the last and must be drawn. Covers a mob or dropped
    /// item that is both close and in view (see [`entity_animating_in_view`]), live
    /// particles, an in-progress mining crack, chest lids mid-swing, and chunks still
    /// awaiting a (re)mesh. Camera motion, raw input, and open-menu interaction are tracked
    /// by the shell; slow sky/fog drift by the host's keep-alive redraw. A mob behind the
    /// player or far off keeps simulating but does NOT hold the frame at full rate.
    ///
    /// [`entity_animating_in_view`]: Self::entity_animating_in_view
    pub(super) fn is_visually_active(&self) -> bool {
        !self.particles.is_empty()
            || self.mining.is_mining()
            || self.world.has_terrain_frame_work()
            || self
                .chest_lids
                .values()
                .any(|&lid| lid > f32::EPSILON && lid < 1.0)
            || !self.door_swings.is_empty()
            || self.entity_animating_in_view()
    }

    /// Is a mob or dropped item both within [`ENTITY_ACTIVITY_RANGE`] and inside the
    /// camera frustum? Only then does its per-frame animation/interpolation actually
    /// change the rendered image and warrant holding the frame rate up. Off-screen or
    /// distant entities still simulate on the tick; they just don't force redraws,
    /// which is what lets a stationary player idle in a populated overworld.
    fn entity_animating_in_view(&self) -> bool {
        let eye = self.cam.pos;
        let r2 = ENTITY_ACTIVITY_RANGE * ENTITY_ACTIVITY_RANGE;
        let frustum = Frustum::from_view_proj(self.cam.view_proj());
        // A coarse upright box around the entity's feet, generous enough that an entity
        // near a frustum edge isn't missed for the frame it slips into view.
        let visible = |p: Vec3| {
            (p - eye).length_squared() <= r2
                && frustum.aabb_visible(p - Vec3::new(0.5, 0.0, 0.5), p + Vec3::new(0.5, 2.0, 0.5))
        };
        self.world.mobs().instances().iter().any(|m| visible(m.pos))
            || self.world.item_entities().iter().any(|d| visible(d.pos))
    }

    /// Combined light + warm-tint amount at the player's eye, for lighting the
    /// first-person hand / held item: it brightens AND warms near torches/furnaces.
    pub(super) fn held_item_light(&self) -> (u8, u8) {
        let c = voxel_at(self.cam.pos);
        self.world.dynamic_light_at_world(c.x, c.y, c.z)
    }

    pub(super) fn tick_mesh_budget(&mut self) {
        const MESH_BUDGET: usize = 4;
        self.world.tick_mesh_budget(MESH_BUDGET);
    }
}
