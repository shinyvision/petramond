use super::entities::MOB_ATTACK_UP_RATIO;
use super::game::ServerGame;
use crate::events::tick::TickEvents;
use crate::events::{AttackAttempt, DamageSource, Outcome};
use crate::player::{self, PlayerId};
use crate::rules::combat::ATTACK_COOLDOWN_TICKS;
use petramond_math::math::Vec3;

const PVP_ATTACK_KNOCKBACK: f32 = 5.0;

enum Claim {
    Pass,
    Swung,
}

type Consumer = fn(&mut ServerGame, usize, &AttackAttempt, &mut TickEvents) -> Claim;

const CONSUMERS: &[Consumer] = &[
    ServerGame::consume_registered_attack,
    ServerGame::consume_melee,
];

impl ServerGame {
    /// Resolves one buffered click per tick, consumed once so a press can't land two hits.
    /// Damage lands the tick after the click (the press is latched per frame by
    /// `InputLatches::latch_attack` and consumed here).
    /// [`ATTACK_COOLDOWN_TICKS`] paces it and the swing plays out fully before the next one, so
    /// you can't spam-click an owl to death. A press during cooldown is held one deep and fires
    /// when it clears, so mashing chains instead of getting eaten.
    /// A claimed press (mob hit, air punch, pack takes the swing) arms the cooldown and reports
    /// `swung_hand`. A block click (mining) does neither.
    pub fn tick_attack(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        sess.sim.attack_cooldown = sess.sim.attack_cooldown.saturating_sub(1);
        if sess
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Attack)
        {
            sess.input.take_attack();
            return;
        }
        if sess.sim.attack_cooldown != 0 {
            return;
        }
        let Some(click) = sess.input.take_attack() else {
            return;
        };
        let mob =
            super::mob_target::authoritative_mob_target(&self.world, &self.sessions[s], click.mob);
        let target = click
            .player
            .and_then(|t| self.authoritative_player_target(s, t))
            .map(|t| self.sessions[t].id);
        let look = self.sessions[s].input.look;
        let attempt = AttackAttempt {
            block: look.map(|t| t.block),
            face: look.map(|t| t.normal),
            mob,
            target,
            player: self.sessions[s].id,
        };
        let mut swung = false;
        for consumer in CONSUMERS {
            match consumer(self, s, &attempt, events) {
                Claim::Pass => continue,
                Claim::Swung => swung = true,
            }
            break;
        }
        if swung {
            self.sessions[s].sim.attack_cooldown = self.sessions[s].player.scaled_ticks(
                mod_api::PlayerAttribute::AttackCooldown,
                ATTACK_COOLDOWN_TICKS,
            );
            events.player(s).swung_hand = true;
            self.sessions[s].latch_swing(
                petramond_world::inventory::Hand::Main,
                mod_api::SwingKind::Attack,
            );
        }
    }

    fn consume_registered_attack(
        &mut self,
        s: usize,
        attempt: &AttackAttempt,
        events: &mut TickEvents,
    ) -> Claim {
        let mut ev = *attempt;
        let claimed = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            bus.attack_attempt(world, sessions, actor, events, &mut ev) == Outcome::Cancel
        };
        if claimed {
            Claim::Swung
        } else {
            Claim::Pass
        }
    }

    fn consume_melee(
        &mut self,
        s: usize,
        attempt: &AttackAttempt,
        events: &mut TickEvents,
    ) -> Claim {
        if let Some(target) = attempt.target {
            if let Some(t) = self.sessions.index_of(target) {
                self.resolve_player_attack(s, t, events);
            }
            Claim::Swung
        } else if let Some(mob_id) = attempt.mob {
            if self.world.mobs().contains(mob_id) {
                let damage = self.roll_attack_damage(s);
                let from = self.sessions[s].player.body_center();
                self.damage_mob_through_pipeline(
                    mob_id,
                    damage,
                    DamageSource::PlayerAttack(self.sessions[s].id),
                    Some(from),
                    None,
                    events,
                );
            }
            Claim::Swung
        } else if attempt.block.is_none() {
            Claim::Swung
        } else {
            Claim::Pass
        }
    }

    fn roll_attack_damage(&mut self, s: usize) -> f32 {
        let (lo, hi) =
            petramond_world::item::attack_damage(self.sessions[s].player.inventory.selected());
        lo + crate::entity::hash01(self.seeds.draw() as u64) * (hi - lo)
    }

    fn authoritative_player_target(&self, s: usize, target: PlayerId) -> Option<usize> {
        let t = self.sessions.index_of(target)?;
        if t == s {
            return None;
        }
        let attacker = &self.sessions[s].player;
        if attacker.is_spectator() || attacker.health() == 0 {
            return None;
        }
        let victim = &self.sessions[t].player;
        if victim.is_spectator() || victim.health() == 0 {
            return None;
        }
        let rel = victim.pos - attacker.eye();
        let lo = rel - Vec3::new(player::HALF_W, 0.0, player::HALF_W);
        let hi = rel + Vec3::new(player::HALF_W, player::HEIGHT, player::HALF_W);
        let closest = Vec3::ZERO.clamp(lo, hi);
        (closest.length() <= player::REACH + 1.0).then_some(t)
    }

    fn resolve_player_attack(&mut self, s: usize, t: usize, events: &mut TickEvents) {
        let from = self.sessions[s].player.body_center();
        let damage = self.roll_attack_damage(s);
        let amount = damage.max(0.0).round() as i32;
        let attacker_id = self.sessions[s].id;
        let source = DamageSource::PlayerAttack(attacker_id);
        if self.damage_player(t, amount, source, Some(from), events) {
            let scale = self.weapon_knockback(source);
            self.shove_player(t, from, scale);
        }
    }

    pub(super) fn weapon_knockback(&self, source: DamageSource) -> f32 {
        let DamageSource::PlayerAttack(id) = source else {
            return 1.0;
        };
        self.sessions
            .iter()
            .find(|sess| sess.id == id)
            .and_then(|sess| sess.player.inventory.selected()?.tool())
            .map_or(1.0, |t| t.knockback)
    }

    pub(super) fn shove_player(
        &mut self,
        t: usize,
        from: petramond_math::world_pos::WorldPos,
        scale: f32,
    ) {
        let away = self.sessions[t].player.body_center() - from;
        let dir = Vec3::new(away.x, 0.0, away.z).normalize_or_zero();
        let impulse = (dir * PVP_ATTACK_KNOCKBACK
            + Vec3::new(0.0, PVP_ATTACK_KNOCKBACK * MOB_ATTACK_UP_RATIO, 0.0))
            * scale;
        self.sessions[t].player.apply_knockback(impulse);
    }
}
