use super::Mobs;
use crate::mob::{Instance, MobId, MobTagValue};
use petramond_math::math::Tilt;

impl Mobs {
    pub(crate) fn exposure_mut(
        &mut self,
        id: MobId,
    ) -> Option<&mut petramond_world::exposure::BodyExposure> {
        Some(self.mob_mut(id)?.exposure_mut())
    }

    /// Toggle the particle-emitter bundle registered under `key` (a
    /// `particle_emitters.json` row, any namespace) on the mob `id`.
    /// `false` for a gone mob, an unregistered key, or an activation past the
    /// per-mob cap. Keeps `list` private, like [`damage_mob`](Self::damage_mob).
    pub fn set_mob_emitter(&mut self, id: MobId, key: &str, active: bool) -> bool {
        let Some(bundle) = petramond_world::particle_emitters::by_key(key) else {
            return false;
        };
        // A one-shot burst bundle is an event, not attachable state.
        if bundle.burst.is_some() {
            return false;
        }
        self.mob_mut(id)
            .is_some_and(|m| m.set_emitter_active(bundle.id, active))
    }

    /// Toggle a NAMED model animation on the mob `id` — the animation
    /// sibling of [`set_mob_emitter`](Self::set_mob_emitter). `false` for a
    /// gone mob or an activation past the per-mob cap. The name is not
    /// validated against the model (the sim never loads models); the renderer
    /// skips unknown names.
    pub fn set_mob_anim(&mut self, id: MobId, name: &str, active: bool) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_active(name, active))
    }

    /// Set an ACTIVE named animation's playback rate on the mob `id`
    /// (see `Instance::set_anim_rate`): `0` freezes the layer mid-stroke,
    /// negative reverses. `false` for a gone mob or an inactive anim.
    pub fn set_mob_anim_rate(&mut self, id: MobId, name: &str, rate: f32) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_rate(name, rate))
    }

    /// Seek an ACTIVE named animation's phase on the mob `id` toward
    /// the absolute `target` at `|rate|`/s, landing exactly (see
    /// `Instance::set_anim_seek`). `false` for a gone mob or an inactive
    /// anim.
    pub fn set_mob_anim_seek(&mut self, id: MobId, name: &str, target: f32, rate: f32) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_seek(name, target, rate))
    }

    /// Authoritative playback state of an ACTIVE named animation on the mob
    /// `id`. `None` covers a gone mob or inactive name.
    pub fn mob_anim_state(&self, id: MobId, name: &str) -> Option<&super::instance::AnimLayer> {
        self.get(id)?.anim_state(name)
    }

    /// Latch a mod's kinematic locomotion intent on the mob `id` for
    /// THIS tick (see `Instance::set_drive`): an optional horizontal
    /// world-space velocity (replaces wish locomotion), an optional vertical
    /// velocity (composes; upward from the ground = a launch), and
    /// optionally an absolute yaw (the mob-facing convention: yaw `0` faces
    /// `-Z`, facing `(-sin yaw, 0, -cos yaw)`). `false` for a gone mob or a
    /// dead mob.
    pub fn set_mob_drive(
        &mut self,
        id: MobId,
        horizontal: Option<[f32; 2]>,
        vertical: Option<f32>,
        yaw: Option<f32>,
        while_walking: bool,
        gait: bool,
    ) -> bool {
        self.mob_mut(id).is_some_and(|m| {
            m.set_drive(super::super::kinematics::DriveIntent {
                horizontal,
                vertical,
                yaw,
                while_walking,
                gait,
            })
        })
    }

    /// Author a live mob's pose for THIS tick (see `Instance::set_kinematic`):
    /// feet position, facing yaw and body tilt, written in place of the
    /// engine's own motion. `Err(distance)` when the placement is farther
    /// from the current position than the external sweep bound allows —
    /// the same envelope a drive's velocity is held to — `Ok(false)` for a
    /// gone mob or a dead mob.
    pub fn set_mob_kinematic(
        &mut self,
        id: MobId,
        pos: petramond_math::world_pos::WorldPos,
        yaw: f32,
        tilt: Tilt,
    ) -> Result<bool, f32> {
        let Some(m) = self.mob_mut(id) else {
            return Ok(false);
        };
        let distance = (pos - m.pos).length();
        if distance > petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE {
            return Err(distance);
        }
        Ok(m.set_kinematic(super::super::kinematics::KinematicPose { pos, yaw, tilt }))
    }

    /// A live mob's tag value.
    pub fn mob_tag(&self, id: MobId, key: &str) -> Option<&MobTagValue> {
        self.get(id)?.tags().get(key)
    }

    /// A live mob's whole tag map (the `MobTagsGet` HostCall's bulk read).
    pub fn mob_tags(
        &self,
        id: MobId,
    ) -> Option<&std::collections::BTreeMap<String, MobTagValue>> {
        Some(self.get(id)?.tags())
    }

    /// The non-dead mobs carrying `key` — with `want`, only those whose
    /// stored value EQUALS it — each with its position in
    /// [`instances`](Self::instances). The `MobsWithTag` HostCall's filter,
    /// kept here so the predicate is unit-testable.
    pub fn with_tag<'a>(
        &'a self,
        key: &'a str,
        want: Option<&'a MobTagValue>,
    ) -> impl Iterator<Item = (usize, &'a Instance)> + 'a {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.is_dead())
            .filter(move |(_, m)| match (m.tags().get(key), want) {
                (Some(have), Some(want)) => have == want,
                (Some(_), None) => true,
                (None, _) => false,
            })
    }

    /// Store a tag on the mob `id`. `false` = no such mob, or the map
    /// already holds [`MAX_MOB_TAGS`](crate::mob::MAX_MOB_TAGS) entries and
    /// `key` would be a NEW one (replacing an existing key always succeeds).
    pub fn set_mob_tag(&mut self, id: MobId, key: String, value: MobTagValue) -> bool {
        match self.mob_mut(id) {
            Some(m) => {
                if m.tags().len() >= crate::mob::MAX_MOB_TAGS && !m.tags().contains_key(&key) {
                    return false;
                }
                m.tags_mut().insert(key, value);
                true
            }
            None => false,
        }
    }

    /// Remove a tag from the mob `id`; returns whether it was present.
    pub fn remove_mob_tag(&mut self, id: MobId, key: &str) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.tags_mut().remove(key).is_some())
    }
}
