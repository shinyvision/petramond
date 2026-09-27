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

    pub fn set_mob_emitter(&mut self, id: MobId, key: &str, active: bool) -> bool {
        let Some(bundle) = petramond_world::particle_emitters::by_key(key) else {
            return false;
        };
        if bundle.burst.is_some() {
            return false;
        }
        self.mob_mut(id)
            .is_some_and(|m| m.set_emitter_active(bundle.id, active))
    }

    pub fn set_mob_anim(&mut self, id: MobId, name: &str, active: bool) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_active(name, active))
    }

    pub fn set_mob_anim_rate(&mut self, id: MobId, name: &str, rate: f32) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_rate(name, rate))
    }

    pub fn set_mob_anim_seek(&mut self, id: MobId, name: &str, target: f32, rate: f32) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.set_anim_seek(name, target, rate))
    }

    pub fn mob_anim_state(&self, id: MobId, name: &str) -> Option<&super::instance::AnimLayer> {
        self.get(id)?.anim_state(name)
    }

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

    pub fn mob_tag(&self, id: MobId, key: &str) -> Option<&MobTagValue> {
        self.get(id)?.tags().get(key)
    }

    pub fn mob_tags(&self, id: MobId) -> Option<&std::collections::BTreeMap<String, MobTagValue>> {
        Some(self.get(id)?.tags())
    }

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

    pub fn remove_mob_tag(&mut self, id: MobId, key: &str) -> bool {
        self.mob_mut(id)
            .is_some_and(|m| m.tags_mut().remove(key).is_some())
    }
}
