//! The replicated bodies a frame presents, as client mods read them
//! (`ClientEntities`), and the camera anchor resolved against them — built
//! from the same interpolated rows the frame draws, so a mod's follow camera
//! and the body it follows never disagree about where that body is.

use mod_api::{ClientEntityData, EntityRef};
use petramond_math::math::Vec3;
use petramond_math::world_pos::WorldPos;

use super::Game;

/// A player body's collision footprint, `[width, height]`.
const PLAYER_SIZE: [f32; 2] = [0.6, 1.8];

/// A player-convention look (camera yaw/pitch) as a unit vector.
fn player_look(yaw: f32, pitch: f32) -> [f32; 3] {
    let cp = pitch.cos();
    Vec3::new(yaw.sin() * cp, pitch.sin(), yaw.cos() * cp)
        .normalize_or_zero()
        .to_array()
}

/// A mob-convention look (its yaw and pitch) as a unit vector.
fn mob_look(yaw: f32, pitch: f32) -> [f32; 3] {
    let cp = pitch.cos();
    Vec3::new(-yaw.sin() * cp, pitch.sin(), -yaw.cos() * cp)
        .normalize_or_zero()
        .to_array()
}

impl Game {
    /// Every player and mob this frame draws, interpolated as drawn. The
    /// local body is one of them unless it has none (a spectator).
    pub(crate) fn presented_entities(&self) -> Vec<ClientEntityData> {
        let alpha = self.tick_alpha();
        let per_tick = 1.0 / petramond::events::tick::TICK_DT;
        let entities = &self.replica.entities;
        let mut rows = Vec::new();
        let player = &self.local.player;
        if !player.is_spectator() {
            let eye = self.local.cam.pos;
            rows.push(ClientEntityData {
                id: EntityRef::Player(mod_api::PlayerId(entities.self_id().0)),
                mob_kind: None,
                name: self.capture_player_name().map(str::to_owned),
                feet: player.pos.to_array(),
                eye: eye.to_array(),
                look: player_look(player.yaw, player.pitch),
                body_facing: [player.yaw.sin(), player.yaw.cos()],
                velocity: player.vel.to_array(),
                size: PLAYER_SIZE,
            });
        }
        for (id, remote) in entities.players().iter_with_ids() {
            if !remote.curr.visible {
                continue;
            }
            let (feet, yaw, pitch) =
                super::remote_players::interpolate(&remote.prev, &remote.curr, alpha);
            let eye = feet + Vec3::new(0.0, petramond::player::EYE, 0.0);
            let step = remote.curr.transform.pos - remote.prev.transform.pos;
            let body_yaw = remote.pose.body_yaw;
            rows.push(ClientEntityData {
                id: EntityRef::Player(mod_api::PlayerId(id.0)),
                mob_kind: None,
                name: entities.roster().get(&id).cloned(),
                feet: feet.to_array(),
                eye: eye.to_array(),
                look: player_look(yaw, pitch),
                body_facing: [body_yaw.sin(), body_yaw.cos()],
                velocity: (step * per_tick).to_array(),
                size: PLAYER_SIZE,
            });
        }
        for mob in entities.mobs().iter() {
            if mob.curr.dead {
                continue;
            }
            let (feet, yaw) = mob.interpolated_pose(alpha);
            let def = petramond::mob::def(petramond::mob::Mob(mob.curr.kind_id));
            let head_yaw = mob.prev.head_yaw + (mob.curr.head_yaw - mob.prev.head_yaw) * alpha;
            let head_pitch =
                mob.prev.head_pitch + (mob.curr.head_pitch - mob.prev.head_pitch) * alpha;
            let step = mob.curr.pos - mob.prev.pos;
            rows.push(ClientEntityData {
                id: EntityRef::Mob(mob.curr.id),
                mob_kind: Some(mod_api::MobId(mob.curr.kind_id)),
                name: None,
                feet: feet.to_array(),
                eye: (feet + Vec3::new(0.0, def.eye_height, 0.0)).to_array(),
                look: mob_look(yaw + head_yaw, head_pitch),
                body_facing: [-yaw.sin(), -yaw.cos()],
                velocity: (step * per_tick).to_array(),
                size: [def.size.half_width * 2.0, def.size.height],
            });
        }
        rows
    }

    /// Where an anchored claim's offset is measured from: the anchor's feet
    /// as this frame presents them, or — when it is not in the frame — its
    /// last presented feet, flagged missing.
    pub(super) fn anchor_feet(&mut self, anchor: EntityRef) -> Option<WorldPos> {
        let found = self
            .presented_entities_cache
            .iter()
            .find(|row| row.id == anchor)
            .map(|row| WorldPos::new(row.feet[0], row.feet[1], row.feet[2]));
        match found {
            Some(feet) => {
                self.last_anchor_feet = Some((anchor, feet));
                self.anchor_missing = false;
                Some(feet)
            }
            None => {
                self.anchor_missing = true;
                self.last_anchor_feet
                    .filter(|(id, _)| *id == anchor)
                    .map(|(_, feet)| feet)
            }
        }
    }

    /// Whether this frame's anchored camera lost its anchor.
    pub fn camera_anchor_missing(&self) -> bool {
        self.anchor_missing
    }

    /// Rebuild the frame's entity rows and hand them to the client mods.
    /// Once per frame, before the view claims resolve.
    pub fn publish_presented_entities(&mut self) {
        self.presented_entities_cache = self.presented_entities();
        self.client_mods.presented().lock().entities = self.presented_entities_cache.clone();
    }
}
