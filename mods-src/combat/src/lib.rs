//! combat — the shield.
//!
//! A shield, crafted at the pack's weapons workbench (4 planks + 4 iron
//! ingots), raised by holding the use button. While it is up, monster melee
//! coming at your FRONT is cancelled, the body moves at half speed, and the
//! hands are barred from attacking, mining and interacting. A hit it absorbs
//! knocks it aside for [`IMPACT_TICKS`], during which the next attacker gets
//! through.
//!
//! Every rule lives in [`guard`], in ONE pure function. This file is the
//! wiring, in three pieces:
//!
//! - The **server** tick system runs the rule for every player and publishes
//!   the answer through the body primitives (`set_player_speed_scale`,
//!   `set_player_denied_actions`, `set_player_held_pose`,
//!   `set_player_bone_pose`).
//! - The **client** frame hook runs the same function on the same snapshot and
//!   publishes the POSE and the ARM for the local player, a round trip
//!   earlier. Both halves, never one: in third person a shield that raises
//!   before the arm holding it is a shield detached from its own fist.
//! - **Blocking** is a `player_damage_pre` handler that re-runs the same
//!   function against the victim's live snapshot and cancels only frontal
//!   `MobAttack` hits. Falls, PvP and other mods' damage pass, and a cancelled
//!   hit applies no knockback either.
//!
//! ## The recoil clock is server state, and the client is TOLD
//!
//! Everything else here derives from local input, which is why it predicts for
//! free. A hit landing does not: nothing the client can see implies it. So the
//! server owns the window and sends the EDGE through `emit_event_to`, and each
//! side runs the same envelope off its own clock.
//!
//! [`IMPACT_TICKS`]: guard::IMPACT_TICKS

mod guard;

use guard::{covers, guard_of, Guard, BLOCK_SOUND, IMPACT_SECONDS, IMPACT_TICKS, SHIELD_ITEM};
use mod_sdk::*;

const GUARD_SYSTEM: u32 = 1;
const DAMAGE_HANDLER: u32 = 1;
const IMPACT_HANDLER: u32 = 2;
const RAISE_HANDLER: u32 = 3;

/// The cue the server sends the wielder's client when their shield takes a
/// hit. No payload: the client already knows the rule, and the only thing it
/// could not know is that this instant happened.
const IMPACT_EVENT: &str = "combat:shield_impact";

#[derive(Default)]
struct Combat {
    /// The shield's session id, resolved once at init.
    shield: Option<ItemId>,
    /// SERVER: the tick each reeling player's shield took its hit.
    /// Self-pruning, so a player who disconnects mid-recoil needs no hook.
    impacts: Vec<(PlayerId, u64)>,
    /// CLIENT: seconds since the local player's own cue arrived. A client
    /// instance has no tick to read, and an animation wants elapsed time.
    recoil: Option<f32>,
}

impl Combat {
    /// SERVER: how far through their recoil window `player` is at `now`.
    fn impact_of(&self, player: PlayerId, now: u64) -> Option<f32> {
        let at = self
            .impacts
            .iter()
            .find(|(id, _)| *id == player)
            .map(|(_, at)| *at)?;
        let elapsed = now.saturating_sub(at);
        (elapsed < IMPACT_TICKS).then(|| elapsed as f32 / IMPACT_TICKS as f32)
    }

    /// CLIENT: advance the recoil clock one frame and answer this frame's
    /// progress through the window (`None` once it has run out).
    fn advance_recoil(&mut self, dt: f32) -> Option<f32> {
        let age = self.recoil? + dt.max(0.0);
        self.recoil = (age < IMPACT_SECONDS).then_some(age);
        self.recoil.map(|age| age / IMPACT_SECONDS)
    }

    /// Publish one body's claims. Shared by both sides so the two can never
    /// pose a player differently; `authority` adds the claims only a server
    /// may make.
    fn publish(&self, player: PlayerId, guard: Guard, authority: bool) {
        // How fast this body moves and what it may do are the server's to
        // resolve: a client predicting either would argue with the validator.
        if authority {
            set_player_speed_scale(player, guard.speed_scale());
            set_player_denied_actions(player, guard.denied());
        }
        set_player_held_pose(
            player,
            guard.pose(guard.main_holds),
            guard.pose(guard.off_holds),
        );
        set_player_bone_pose(player, guard.arms());
    }

    /// The blocking half of `player_damage_pre`: cancel a frontal monster
    /// strike, and knock the shield aside for doing it.
    fn on_damage(&mut self, shield: ItemId, payload: &EventPayload) -> Outcome {
        // Falls, PvP and other mods' damage are not the shield's job.
        let EventPayload::PlayerDamagePre {
            source: DamageSource::MobAttack { .. },
            origin,
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        // The dispatch names its victim, so the pure rule on their live
        // snapshot decides — no cached flag to go stale or hit the wrong body.
        let state = player_state();
        let Some(me) = state.id else {
            return Outcome::Continue;
        };
        let now = current_tick();
        if !guard_of(shield, &state, self.impact_of(me, now)).absorbs() || !covers(&state, *origin)
        {
            return Outcome::Continue;
        }
        // Spatial: a block is something bystanders hear too.
        emit_sound(BLOCK_SOUND, Some(state.pos));
        self.impacts.retain(|(id, _)| *id != me);
        self.impacts.push((me, now));
        // Only the wielder needs telling: every other screen picks the recoil
        // up from the replicated pose the tick system publishes anyway.
        emit_event_to(me, IMPACT_EVENT, &[]);
        Outcome::Cancel
    }
}

impl Mod for Combat {
    fn init(&mut self) {
        // Registry-only, and legal on every instance (server, worldgen,
        // client) — the client half needs the id just as much.
        self.shield = resolve_item(SHIELD_ITEM);
        if self.shield.is_none() {
            log("[combat] 'combat:shield' did not resolve — staying inert");
            return;
        }
        match runtime_side() {
            RuntimeSide::Server => {
                // The tick's earliest seam, so a guard raised this tick is up
                // before the Mobs stage swings at it.
                register_tick_system(Stage::Mining, AttachSide::Before, 0, GUARD_SYSTEM);
                register_event_handler(EventKind::PlayerDamagePre, 0, DAMAGE_HANDLER);
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
            }
            RuntimeSide::Client => {
                // The one thing a client cannot derive from the input it sees.
                register_event_handler(EventKind::ModEvent, 0, IMPACT_HANDLER);
                // ...and the one it can: the same raise, a round trip earlier.
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
            }
            RuntimeSide::Worldgen => {}
        }
    }

    /// Publish every player's guard. Idempotent every tick — no state to reset
    /// and no edge to miss; a released guard is just the neutral claim.
    fn tick_system(&mut self, system: u32) {
        debug_assert_eq!(system, GUARD_SYSTEM);
        let Some(shield) = self.shield else {
            return;
        };
        let now = current_tick();
        self.impacts
            .retain(|(_, at)| now.saturating_sub(*at) < IMPACT_TICKS);
        for entry in players() {
            let guard = guard_of(shield, &entry.state, self.impact_of(entry.id, now));
            self.publish(entry.id, guard, true);
        }
    }

    fn handle_event(&mut self, handler: u32, payload: &mut EventPayload) -> Outcome {
        let Some(shield) = self.shield else {
            return Outcome::Continue;
        };
        match handler {
            DAMAGE_HANDLER => self.on_damage(shield, payload),
            // The press nothing else wanted. A shield in either hand takes it
            // and keeps it until the button comes up; the guard is up for
            // exactly as long as the gesture is ours. `Cancel` only stops
            // later handlers being offered the same press — taking it is not
            // an interaction, so nothing jabs.
            RAISE_HANDLER => {
                let state = player_state();
                let holds = |slot| !state.spectator && slot == Some(shield);
                match state.id {
                    Some(me) if holds(state.held) || holds(state.off_held) => {
                        hold_use(me);
                        Outcome::Cancel
                    }
                    _ => Outcome::Continue,
                }
            }
            // The wielder's client hearing that its shield just took a hit.
            // Starting the clock is the whole handler; what the recoil looks
            // like is the shared rule's business on both sides.
            IMPACT_HANDLER => {
                if matches!(payload, EventPayload::ModEvent { key, .. } if key == IMPACT_EVENT) {
                    self.recoil = Some(0.0);
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    /// The PREDICTED half: the same rule, the same snapshot, one round trip
    /// earlier. Presentation only — a speed scale a batch late is
    /// imperceptible next to a shield that visibly lags the button.
    fn client_frame(&mut self, frame: &ClientFrameData) {
        let Some(shield) = self.shield else {
            return;
        };
        let impact = self.advance_recoil(frame.dt);
        let state = player_state();
        let Some(me) = state.id else {
            return;
        };
        self.publish(me, guard_of(shield, &state, impact), false);
    }
}

register_mod!(Combat);

#[cfg(test)]
mod tests {
    use super::*;

    const SHIELD: ItemId = ItemId(7);
    const ME: PlayerId = PlayerId(3);

    /// Whole ticks, open at the far end: a hit at T leaves the shield down for
    /// T..T+IMPACT_TICKS. An off-by-one is a shield either free for one tick or
    /// vulnerable for one too many, and nothing downstream would show it.
    #[test]
    fn the_server_recoil_window_is_exactly_impact_ticks_long() {
        let mut combat = Combat {
            shield: Some(SHIELD),
            ..Default::default()
        };
        combat.impacts.push((ME, 100));
        assert_eq!(combat.impact_of(ME, 100), Some(0.0));
        assert!(combat.impact_of(ME, 100 + IMPACT_TICKS - 1).is_some());
        assert_eq!(combat.impact_of(ME, 100 + IMPACT_TICKS), None);
        assert_eq!(combat.impact_of(PlayerId(4), 100), None, "somebody else");
    }

    /// The client measures the SAME window off its frame clock and lets go when
    /// it runs out — a recoil that never expires is a shield stuck in its
    /// impact pose for the rest of the session.
    #[test]
    fn the_client_recoil_clock_runs_the_same_window_and_then_releases() {
        let mut combat = Combat::default();
        assert_eq!(combat.advance_recoil(0.05), None, "nothing to advance");

        combat.recoil = Some(0.0);
        let mut seen = Vec::new();
        for _ in 0..(IMPACT_TICKS + 2) {
            seen.push(combat.advance_recoil(0.05));
        }
        assert!(seen[0].is_some_and(|p| p > 0.0), "starts moving at once");
        assert!(
            seen.iter()
                .take(IMPACT_TICKS as usize - 1)
                .all(Option::is_some),
            "runs the whole window: {seen:?}"
        );
        assert_eq!(seen.last(), Some(&None), "and releases: {seen:?}");
        assert!(combat.recoil.is_none());
    }
}
