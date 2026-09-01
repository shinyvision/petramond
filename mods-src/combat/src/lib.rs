//! combat — the shield, and the tools' hands.
//!
//! A shield, crafted at the pack's weapons workbench (4 planks + 4 iron
//! ingots), raised by holding the use button. While it is up, monster melee
//! coming at your FRONT is cancelled, the body moves at half speed, and the
//! hands are barred from attacking, mining and interacting. A hit it absorbs
//! knocks it aside for [`IMPACT_TICKS`], during which the next attacker gets
//! through.
//!
//! ## The tools' swings (the body seams' second tenant)
//!
//! The pack also owns the MAIN hand while it works a pickaxe or an axe: the
//! swing law in [`swing`] animates the hand per phase (the item in first
//! person, Composed arm bones in third), claims the hand's swing so the
//! engine's vanilla punch stands down, and releases both the moment no tool
//! is held. Quick consecutive ATTACKS chain through the tool's combo of
//! authored curves (each follow-up plays the next swing); mining repeats the
//! first. Same shape as the guard — one pure law in [`swing`], its clock
//! run by the server tick system for every body and by the client frame hook
//! for the local player, a round trip earlier — so this pack exercises both
//! halves of every body seam the shield dogfooded: stances AND whole-hand
//! swing animation.
//!
//! While a claimed tool paces a body, the pack owns the ATTACK RATE
//! outright: the engine cooldown is claimed to zero
//! (`set_player_attribute`) and the in-flight arc bars the next attack (a
//! denial) until its recovery, so the animation and the pace are one clock
//! that cannot disagree. On [`COMBO_MOBS`] a paced hit also drops the
//! engine i-frame from the damage pipeline (`mob_damage_pre` edits the
//! feedback components): the swing clock already limits hits to one per
//! arc, so chained combos land exactly as they read.
//!
//! ## The tools land their own hits
//!
//! A paced tool whose exports mark their IMPACT key takes the player's
//! primary press outright (`attack_attempt` claimed — the engine's
//! crosshair melee stands down for it), and the hit lands when the swing's
//! impact plays: the strike law in [`strike`] judges, from where the
//! attacker is looking at that instant, every body the family's window
//! reaches — closer and more dead-on lands harder, an axe sweeps every body
//! in its arc, a pickaxe plunges into one — and lands the verdicts through
//! the engine's funnel with the player named as the attacker. A press at a
//! block is mining's and is left alone.
//!
//! The pack is laws, a merger, and this wiring: the guard law lives in
//! [`guard`], the swing law in [`swing`], the strike law in [`strike`], and
//! [`body`] merges the body claims into ONE write per body seam
//! (`set_player_attribute`, `set_player_denied_actions`,
//! `set_player_held_pose`, `set_player_bone_pose`,
//! `set_player_hand_motions`). This file only routes:
//!
//! - The **server** tick system publishes every player's body per tick,
//!   and lands the strike of any swing whose impact played this tick.
//! - The **client** frame hook publishes the local player a round trip
//!   earlier. Both halves, never one: a raised shield that gets to the
//!   screen before the arm holding it is the shield detached from its own
//!   fist, and a swing that plays in first person only is a fist holding a
//!   still tool.
//! - **Blocking** is a `player_damage_pre` handler that re-runs the guard
//!   law against the victim's live snapshot and cancels only frontal
//!   `MobAttack` hits. Falls, PvP and other mods' damage pass, and a
//!   cancelled hit applies no knockback either.
//!
//! ## The recoil clock is server state, and the client is TOLD
//!
//! Everything else here derives from local input, which is why it predicts
//! for free. A hit landing does not: nothing the client can see implies it.
//! So the server owns the window and sends the EDGE through `emit_event_to`,
//! and each side runs the same envelope off its own clock — ticks on the
//! server, frame seconds on the client.
//!
//! [`IMPACT_TICKS`]: guard::IMPACT_TICKS

mod body;
mod guard;
mod strike;
mod swing;

use body::Bodies;
use guard::{covers, guard_of, BLOCK_SOUND, IMPACT_SECONDS, IMPACT_TICKS, SHIELD_ITEM};
use mod_sdk::*;

const BODY_SYSTEM: u32 = 1;
const DAMAGE_HANDLER: u32 = 1;
const IMPACT_HANDLER: u32 = 2;
const RAISE_HANDLER: u32 = 3;
const COMBO_HANDLER: u32 = 4;
const ATTACK_HANDLER: u32 = 5;

/// Mobs that take every PACED hit: the engine i-frame is stripped from a
/// hit whose attacker's swings this pack already paces, because the clock
/// does the i-frame's job — one hit per arc — and the window would only
/// swallow chained combos. Species policy; hits from unpaced hands (bare
/// fists, another pack's weapon) keep the engine window.
const COMBO_MOBS: &[&str] = &["monsters:zombie", "monsters:hushjaw"];

/// The cue the server sends the wielder's client when their shield takes a
/// hit. No payload: the client already knows the rule, and the only thing it
/// could not know is that this instant happened.
const IMPACT_EVENT: &str = "combat:shield_impact";

/// One fixed tick's seconds — the SERVER swing clock's step (20 TPS). The
/// client's clock steps on the frame clock; both run the same law, and the
/// engine's eased pose lane makes the two rates one motion.
const TICK_SECONDS: f32 = 1.0 / 20.0;

#[derive(Default)]
struct Combat {
    /// The shield's session id, resolved once at init. `None` (a build
    /// without the row) leaves the guard inert; the tools' swings run
    /// regardless.
    shield: Option<ItemId>,
    /// The tool table and the swing clocks — the per-body publisher.
    body: Bodies,
    /// The [`COMBO_MOBS`] this build's registry actually carries — a
    /// species from a pack that is not installed is one row of policy that
    /// never applies, resolved once at init.
    combo_mobs: Vec<MobId>,
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
        if !guard_of(Some(shield), &state, self.impact_of(me, now)).absorbs()
            || !covers(&state, *origin)
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

    /// The press half of the strike: a primary press by a hand holding a
    /// tool that LANDS its own hits is this pack's — claimed here, so the
    /// engine's crosshair melee stands down, and landed by the tick system
    /// when the swing's impact plays. A press at a block is mining's; an
    /// unpaced hand (fists, another pack's weapon, a tool whose exports
    /// mark no impact) keeps the engine's hit on the click.
    fn on_attack_attempt(&self, payload: &EventPayload) -> Outcome {
        let EventPayload::AttackAttempt { block, player, .. } = payload else {
            return Outcome::Continue;
        };
        if block.is_some() {
            return Outcome::Continue;
        }
        // The dispatch names the presser; their live snapshot says what
        // the hand holds.
        let state = player_state();
        if state.id != Some(*player) || !self.body.lands(state.held) {
            return Outcome::Continue;
        }
        Outcome::Cancel
    }

    /// The combo half of `mob_damage_pre`: a PACED attacker's hit on a
    /// [`COMBO_MOBS`] species drops the `Immunity` component from its
    /// feedback pipeline, so the hit neither respects nor grants the engine
    /// i-frame window — the attacker's swing clock is already the rate
    /// limit. Everything else about the hit (health, flash, knockback,
    /// sound) plays exactly as the species authored it.
    fn on_mob_damage(&self, payload: &mut EventPayload) -> Outcome {
        let EventPayload::MobDamagePre {
            kind,
            source,
            feedback,
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        if !self.combo_mobs.contains(kind) {
            return Outcome::Continue;
        }
        let DamageSource::PlayerAttack { id } = source else {
            return Outcome::Continue;
        };
        let attacker = *id;
        let paced = players()
            .iter()
            .find(|entry| entry.id == attacker)
            .is_some_and(|entry| self.body.paces(entry.state.held));
        if paced {
            feedback
                .components
                .retain(|c| !matches!(c, MobDamageFeedbackComponent::Immunity { .. }));
        }
        Outcome::Continue
    }
}

impl Mod for Combat {
    fn init(&mut self) {
        // Registry-only, and legal on every instance (server, worldgen,
        // client) — the client half needs the ids just as much.
        self.body.resolve();
        self.combo_mobs = COMBO_MOBS
            .iter()
            .filter_map(|name| resolve_mob(name))
            .collect();
        self.shield = resolve_item(SHIELD_ITEM);
        if self.shield.is_none() {
            log("[combat] 'combat:shield' did not resolve — the guard stays inert");
        }
        if self.shield.is_none() && self.body.is_empty() {
            return;
        }
        match runtime_side() {
            RuntimeSide::Server => {
                // The tick's earliest seam, so a guard raised this tick is up
                // before the Mobs stage swings at it — and the swings'
                // answers publish within the same pass.
                register_tick_system(Stage::Mining, AttachSide::Before, 0, BODY_SYSTEM);
                register_event_handler(EventKind::PlayerDamagePre, 0, DAMAGE_HANDLER);
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
                register_event_handler(EventKind::MobDamagePre, 0, COMBO_HANDLER);
                register_event_handler(EventKind::AttackAttempt, 0, ATTACK_HANDLER);
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

    /// Publish every player's guard and tool swings, and land the strike
    /// of any swing whose impact played this tick. Idempotent every tick —
    /// no edge to miss; a released claim is just the neutral write.
    fn tick_system(&mut self, system: u32) {
        debug_assert_eq!(system, BODY_SYSTEM);
        let now = current_tick();
        self.impacts
            .retain(|(_, at)| now.saturating_sub(*at) < IMPACT_TICKS);
        // The roster is the snapshot of truth for both rules: prune the
        // clocks of anyone gone, advance the rest, publish per body.
        let roster = players();
        self.body.prune(&roster);
        for entry in &roster {
            let impact = self.impact_of(entry.id, now);
            let guard = guard_of(self.shield, &entry.state, impact);
            let landed = self
                .body
                .publish(entry.id, &guard, &entry.state, true, TICK_SECONDS);
            if let Some(style) = landed {
                strike::land(entry.id, style, &entry.state);
            }
        }
    }

    fn handle_event(&mut self, handler: u32, payload: &mut EventPayload) -> Outcome {
        if handler == COMBO_HANDLER {
            return self.on_mob_damage(payload);
        }
        if handler == ATTACK_HANDLER {
            return self.on_attack_attempt(payload);
        }
        // Everything below is the shield's, inert without its item.
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

    /// The PREDICTED half: the same rules, the same snapshot, one round trip
    /// earlier. Presentation only — a speed scale a batch late is
    /// imperceptible next to a shield that visibly lags the button, and a
    /// swing whose curve lags the click feels mushy on both sides of a
    /// strike.
    fn client_frame(&mut self, frame: &ClientFrameData) {
        let impact = self.advance_recoil(frame.dt);
        let state = player_state();
        let Some(me) = state.id else {
            return;
        };
        let guard = guard_of(self.shield, &state, impact);
        self.body
            .publish(me, &guard, &state, false, frame.dt.max(0.0));
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
