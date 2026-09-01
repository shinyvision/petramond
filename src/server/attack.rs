//! The attack dispatch: one primary-button press becomes ONE
//! [`AttackAttempt`] — the most primitive gesture possible: what the
//! crosshair held (a block cell, a live mob, another player) and who
//! pressed. Nothing else rides the attempt; every gate belongs to the
//! consumer that cares about it, read from the actor's state.
//!
//! The attempt walks `CONSUMERS` — an ordered registry, the interact
//! dispatch's shape. A consumer either claims the press (it became a swing:
//! the hand swings, the cooldown arms, the Attack edge latches — whoever
//! lands what) or passes. Mods participate through the `attack_attempt`
//! bus event; the engine's melee — the crosshair hit, the air punch — is
//! the entry after it. A press at a BLOCK is mining's, which the held button
//! runs on its own; the melee passes on it, so it swings nothing.

use super::entities::MOB_ATTACK_UP_RATIO;
use super::game::{ServerGame, ATTACK_COOLDOWN_TICKS};
use crate::events::tick::TickEvents;
use crate::events::{AttackAttempt, DamageSource, Outcome};
use crate::player::{self, PlayerId};
use petramond_math::math::Vec3;

/// Horizontal knockback speed of a player's melee hit on another player
/// (m/s), with the same [`MOB_ATTACK_UP_RATIO`] upward pop — tuned to read
/// like a mob strike of ordinary strength.
const PVP_ATTACK_KNOCKBACK: f32 = 5.0;

/// One consumer's verdict on the attempt.
enum Claim {
    /// Not this consumer's business — the walk continues.
    Pass,
    /// The press became a swing; the walk ends.
    Swung,
}

/// One attack consumer: offered the attempt, it claims or passes. The
/// signature is the whole contract — the acting session, the attempt, the
/// tick's event sink, nothing else.
type Consumer = fn(&mut ServerGame, usize, &AttackAttempt, &mut TickEvents) -> Claim;

/// The consumer registry, in claim order. A new engine capability is a new
/// entry here, never a branch in the dispatcher.
const CONSUMERS: &[Consumer] = &[
    // Mods first: every press, a press at nothing included — a handler's
    // Cancel is a claim (a pack landing its swings at the animation's own
    // impact, wherever the tool actually arrives).
    ServerGame::consume_registered_attack,
    // The engine's melee: the crosshair's mob or player, else a punch at
    // the air; a press at a block is mining's and swings nothing.
    ServerGame::consume_melee,
];

impl ServerGame {
    /// Attack, on the tick: resolve a buffered primary-button press (consumed once, so a
    /// press never lands more than one hit). The damage lands the tick *after* the click —
    /// `pending_attack` is latched per frame and consumed here. Rate-limited by
    /// [`ATTACK_COOLDOWN_TICKS`]: the cooldown counts down one tick at a time and an attack
    /// is refused (no swing, no damage) while it's running, so mashing the button can't
    /// land a hit every tick — only one swing per cooldown connects, so an owl can't be
    /// spam-clicked to death. A press that a consumer claims (a mob hit, a punch at the
    /// air, a pack taking the swing) arms the cooldown and reports `swung_hand`; a click
    /// on a block (mining) does neither.
    pub fn tick_attack(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        sess.attack_cooldown = sess.attack_cooldown.saturating_sub(1);
        // Consume the press AND its targets whether or not it lands (no
        // queuing past one tick); it only resolves once the cooldown elapsed.
        let mob_target = std::mem::take(&mut sess.pending_attack_mob);
        let player_target = std::mem::take(&mut sess.pending_attack_player);
        let pressed = std::mem::take(&mut sess.pending_attack);
        // A mod-denied swing is CONSUMED and dropped, never queued: the press
        // is spent the same as one the cooldown ate, so releasing the claim
        // cannot fire a stored punch. It arms no cooldown either — a denied
        // action did not happen, so nothing about it may be felt afterwards.
        if sess
            .player
            .denied_actions()
            .denies(mod_api::BodyAction::Attack)
        {
            return;
        }
        if !pressed || sess.attack_cooldown != 0 {
            return;
        }
        // The claimed targets resolve through the authoritative validators
        // BEFORE any consumer (mods included) can observe them: a forged,
        // vanished, dead, occluded or out-of-reach claim is no target at all.
        let mob = self
            .authoritative_mob_target(s, mob_target)
            .map(|idx| self.world.mobs().instances()[idx].id());
        let target = player_target
            .and_then(|t| self.authoritative_player_target(s, PlayerId(t)))
            .map(|t| self.sessions[t].id);
        let look = self.sessions[s].look;
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
            // The cooldown arms SCALED by the claimed attribute: a pack whose
            // own pacing gates the hand (a swing animation barring attacks
            // mid-arc) claims 0.0 here and the animation becomes the rate
            // limit; with no claim the constant stands.
            self.sessions[s].attack_cooldown = self.sessions[s].player.scaled_ticks(
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

    /// The mod consumer: dispatch the attempt to every registered
    /// `attack_attempt` handler; a handler's Cancel is a claim. Dispatched
    /// within the sessions view so handlers (and the host calls they make —
    /// `PlayerState`, `Players`, `DamageMob` naming the presser) resolve the
    /// acting session.
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
                bus,
                ..
            } = self;
            Self::with_sessions_view(sessions, s, |sess| {
                bus.attack_attempt(
                    world,
                    &mut sess.player,
                    &mut sess.gui_state,
                    events,
                    &mut ev,
                ) == Outcome::Cancel
            })
        };
        if claimed {
            Claim::Swung
        } else {
            Claim::Pass
        }
    }

    /// The engine's melee: damage the targeted mob or PLAYER (rolling the
    /// held weapon's damage; a mob kill spawns loot), or — looking at nothing
    /// — punch the air. A click on a block is mining's and swings nothing.
    fn consume_melee(
        &mut self,
        s: usize,
        attempt: &AttackAttempt,
        events: &mut TickEvents,
    ) -> Claim {
        if let Some(target) = attempt.target {
            if let Some(t) = self.sessions.iter().position(|sess| sess.id == target) {
                self.resolve_player_attack(s, t, events);
            }
            Claim::Swung
        } else if let Some(mob_id) = attempt.mob {
            if let Some(idx) = self.world.mobs().index_of_id(mob_id) {
                let damage = self.roll_attack_damage(s);
                let from = self.sessions[s].player.body_center();
                // The pipeline may cancel the damage; the swing still happened
                // and still arms the cooldown.
                self.damage_mob_through_pipeline(
                    s,
                    idx,
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

    /// The held weapon's damage roll for session `s` (the same roll a mob hit
    /// uses; deterministic off the spawn counter).
    fn roll_attack_damage(&mut self, s: usize) -> f32 {
        let (lo, hi) =
            petramond_world::item::attack_damage(self.sessions[s].player.inventory.selected());
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        lo + crate::entity::hash01(self.spawn_counter as u64) * (hi - lo)
    }

    /// PvP target validation, the player twin of `authoritative_mob_target`:
    /// the claimed session exists, is not the attacker, both ends are alive
    /// non-spectators, and the target's body AABB is within
    /// `player::REACH + 1.0` of the attacker's EYE measured to the AABB's
    /// closest point — the same closest-point-plus-slack rule the
    /// block-target reach check uses (`apply_player_update`). Any failure =
    /// no target (the press degrades to the air punch the swing already is).
    fn authoritative_player_target(&self, s: usize, target: PlayerId) -> Option<usize> {
        let t = self.sessions.iter().position(|sess| sess.id == target)?;
        if t == s {
            return None; // self-attack impossible (targeting skips own id; belt and braces)
        }
        let attacker = &self.sessions[s].player;
        if attacker.is_spectator() || attacker.health() == 0 {
            return None; // spectators and the dead can't attack
        }
        let victim = &self.sessions[t].player;
        if victim.is_spectator() || victim.health() == 0 {
            return None; // spectators and the dead can't be attacked
        }
        let eye = attacker.eye();
        let lo = victim.pos - Vec3::new(player::HALF_W, 0.0, player::HALF_W);
        let hi = victim.pos + Vec3::new(player::HALF_W, player::HEIGHT, player::HALF_W);
        let closest = eye.clamp(lo, hi);
        ((closest - eye).length() <= player::REACH + 1.0).then_some(t)
    }

    /// PvP: one validated melee hit on session `t`, through the single
    /// [`damage_player`](ServerGame::damage_player) funnel with
    /// [`DamageSource::PlayerAttack`]. An applied hit shoves the victim away
    /// from the attacker; engine immunity or a cancelled `player_damage_pre`
    /// suppresses damage AND knockback — the same contract as mob strikes.
    fn resolve_player_attack(&mut self, s: usize, t: usize, events: &mut TickEvents) {
        let from = self.sessions[s].player.body_center();
        let damage = self.roll_attack_damage(s);
        let amount = damage.max(0.0).round() as i32;
        let attacker_id = self.sessions[s].id;
        let source = DamageSource::PlayerAttack(attacker_id);
        if self.damage_player(t, amount, source, Some(from), events) {
            self.shove_player(t, from);
        }
    }

    /// The melee shove: push session `t` horizontally away from `from` with
    /// the mob strike's upward pop ratio — what every applied player-on-
    /// player hit does, the engine's own and a mod's landed on a player's
    /// behalf alike.
    pub(super) fn shove_player(&mut self, t: usize, from: Vec3) {
        let away = self.sessions[t].player.body_center() - from;
        let dir = Vec3::new(away.x, 0.0, away.z).normalize_or_zero();
        let impulse = dir * PVP_ATTACK_KNOCKBACK
            + Vec3::new(0.0, PVP_ATTACK_KNOCKBACK * MOB_ATTACK_UP_RATIO, 0.0);
        self.sessions[t].player.apply_knockback(impulse);
    }
}
