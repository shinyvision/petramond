use super::Mobs;
use crate::mob::MobTagValue;

impl Mobs {
    /// Toggle the particle-emitter bundle registered under `key` (a
    /// `particle_emitters.json` row, any namespace) on the mob at `index`.
    /// `false` for a bad index, an unregistered key, or an activation past the
    /// per-mob cap. Keeps `list` private, like [`damage_mob`](Self::damage_mob).
    pub fn set_mob_emitter(&mut self, index: usize, key: &str, active: bool) -> bool {
        let Some(bundle) = crate::particle_emitters::by_key(key) else {
            return false;
        };
        // A one-shot burst bundle is an event, not attachable state.
        if bundle.burst.is_some() {
            return false;
        }
        self.mob_mut(index)
            .is_some_and(|m| m.set_emitter_active(bundle.id, active))
    }

    /// Toggle a NAMED model animation on the mob at `index` — the animation
    /// sibling of [`set_mob_emitter`](Self::set_mob_emitter). `false` for a
    /// bad index or an activation past the per-mob cap. The name is not
    /// validated against the model (the sim never loads models); the renderer
    /// skips unknown names.
    pub fn set_mob_anim(&mut self, index: usize, name: &str, active: bool) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_anim_active(name, active))
    }

    /// Set an ACTIVE named animation's playback rate on the mob at `index`
    /// (see `Instance::set_anim_rate`): `0` freezes the layer mid-stroke,
    /// negative reverses. `false` for a bad index or an inactive anim.
    pub fn set_mob_anim_rate(&mut self, index: usize, name: &str, rate: f32) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_anim_rate(name, rate))
    }

    /// Seek an ACTIVE named animation's phase on the mob at `index` toward
    /// the absolute `target` at `|rate|`/s, landing exactly (see
    /// `Instance::set_anim_seek`). `false` for a bad index or an inactive
    /// anim.
    pub fn set_mob_anim_seek(&mut self, index: usize, name: &str, target: f32, rate: f32) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_anim_seek(name, target, rate))
    }

    /// Authoritative playback state of an ACTIVE named animation on the mob
    /// at `index`. `None` covers a bad index or inactive name.
    pub fn mob_anim_state(&self, index: usize, name: &str) -> Option<&super::instance::AnimLayer> {
        self.list.get(index)?.anim_state(name)
    }

    /// Latch a mod's kinematic locomotion intent on the mob at `index` for
    /// THIS tick (see [`Instance::set_drive`]): a horizontal world-space
    /// velocity plus optionally an absolute yaw (the mob-facing convention:
    /// yaw `0` faces `-Z`, facing `(-sin yaw, 0, -cos yaw)`). `false` for a
    /// bad index or a dead mob.
    pub fn set_mob_drive(
        &mut self,
        index: usize,
        vel_x: f32,
        vel_z: f32,
        yaw: Option<f32>,
    ) -> bool {
        self.mob_mut(index)
            .is_some_and(|m| m.set_drive(vel_x, vel_z, yaw))
    }

    /// A live mob's mod KV entry (see [`Instance::mod_kv`]).
    pub fn mod_kv_get(&self, index: usize, key: &str) -> Option<&[u8]> {
        self.list.get(index)?.mod_kv().get(key).map(Vec::as_slice)
    }

    /// Store a mod KV entry on the mob at `index`; `false` = no such mob.
    pub fn mod_kv_set(&mut self, index: usize, key: String, value: Vec<u8>) -> bool {
        match self.list.get_mut(index) {
            Some(m) => {
                m.mod_kv_mut().insert(key, value);
                true
            }
            None => false,
        }
    }

    /// Remove a mod KV entry from the mob at `index`; returns whether it was
    /// present.
    pub fn mod_kv_remove(&mut self, index: usize, key: &str) -> bool {
        self.list
            .get_mut(index)
            .is_some_and(|m| m.mod_kv_mut().remove(key).is_some())
    }

    /// A live mob's tag value.
    pub fn mob_tag(&self, index: usize, key: &str) -> Option<&MobTagValue> {
        self.list.get(index)?.tags().get(key)
    }

    /// A live mob's whole tag map (the `MobTagsGet` HostCall's bulk read).
    pub fn mob_tags(
        &self,
        index: usize,
    ) -> Option<&std::collections::BTreeMap<String, MobTagValue>> {
        Some(self.list.get(index)?.tags())
    }

    /// Live-list indices of non-dead mobs carrying `key` — with `want`, only
    /// those whose stored value EQUALS it. The `MobsWithTag` HostCall's
    /// filter, kept here so `list` stays private and the predicate is
    /// unit-testable.
    pub fn indices_with_tag(&self, key: &str, want: Option<&MobTagValue>) -> Vec<usize> {
        self.list
            .iter()
            .enumerate()
            .filter(|(_, m)| !m.is_dead())
            .filter(|(_, m)| match (m.tags().get(key), want) {
                (Some(have), Some(want)) => have == want,
                (Some(_), None) => true,
                (None, _) => false,
            })
            .map(|(i, _)| i)
            .collect()
    }

    /// Store a tag on the mob at `index`. `false` = no such mob, or the map
    /// already holds [`MAX_MOB_TAGS`](crate::mob::MAX_MOB_TAGS) entries and
    /// `key` would be a NEW one (replacing an existing key always succeeds).
    pub fn set_mob_tag(&mut self, index: usize, key: String, value: MobTagValue) -> bool {
        match self.list.get_mut(index) {
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

    /// Remove a tag from the mob at `index`; returns whether it was present.
    pub fn remove_mob_tag(&mut self, index: usize, key: &str) -> bool {
        self.list
            .get_mut(index)
            .is_some_and(|m| m.tags_mut().remove(key).is_some())
    }
}
