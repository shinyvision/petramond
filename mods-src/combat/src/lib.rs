//! combat: shield, bow, and the tools' hands.
//!
//! Shield: made at the pack's weapons workbench from 4 planks + 4 iron ingots. Hold use to raise
//! it. While it's up, monster melee and arrows from the front are stopped, you move at half
//! speed, and you can't attack, mine or interact. A hit it absorbs knocks it aside for
//! [`IMPACT_TICKS`], and the next attacker gets through.
//!
//! ## Tool swings (second tenant of the body seams)
//!
//! The pack owns the main hand while a pickaxe, axe or sword is out. [`swing`] claims the
//! engine's player clips, one first-person and one body clip per swing, and runs them on its own
//! clock. The vanilla punch stands down until no tool is held. Quick attacks chain through the
//! family's combo and mining loops the work clip. Like the guard, the law is ticked by the server
//! for every body and by the client frame hook a round trip earlier for the local player.
//!
//! A paced tool owns the attack rate. The engine cooldown is pinned to zero and the swing in
//! flight denies the next attack until its recovery, so animation and pace are one clock. On a
//! [`PACED_COMBO_DATA`] species the engine i-frame is dropped too, since the swing clock already
//! allows one hit per arc.
//!
//! ## The bow
//!
//! Holding use with a bow draws it ([`bow`]), with pull frames on the generic held-display seam.
//! Letting go launches one arrow through the engine's flying-item primitive. `projectile_hit`
//! lands the strike, harder the faster it arrived, unless a raised shield faces it. The arrow is
//! spent in the wound; one that hits a block lodges there to be pulled out later.
//!
//! ## The rules are a list
//!
//! Every use-press rule (bow, guard, whatever comes next) is one [`claims::Rule`] in a single
//! ordered list, and list order is precedence. Handlers fold the claims through
//! [`claims::compose`], and [`body::run`] writes each seam once.
//!
//! ## Tools land their own hits
//!
//! A paced tool whose clips mark their impact claims `attack_attempt`, so the engine's crosshair
//! melee stands down and the hit lands when the impact plays. [`strike`] judges every body in the
//! family's window from where the attacker is looking right then: closer and more dead-on hits
//! harder, an axe sweeps its arc, a pickaxe plunges into one target. A press at a block is left
//! to mining.
//!
//! Wiring: guard in [`guard`], bow in [`bow`], swing in [`swing`] over the rows in [`families`],
//! strike in [`strike`], and [`body`] merges the claims. This file only routes:
//!
//! - The server tick runs every player's body and lands any swing whose impact played.
//! - The client frame hook runs the local player a round trip earlier. We need both, or the
//!   shield shows up before the arm holding it and first-person swings hold a still tool.
//! - Blocking re-runs the rules against the victim's live snapshot and cancels a frontal
//!   `MobAttack` hit or deflects a frontal arrow. Falls, PvP melee and other mods' damage pass.
//!
//! ## The recoil clock is server state, and the client is told
//!
//! Everything else here comes from local input and predicts for free. A landed hit doesn't, since
//! nothing the client sees implies it. The server owns the window and sends the edge through
//! `emit_event_to`; each side runs the same envelope off its own clock.
//!
//! [`IMPACT_TICKS`]: guard::IMPACT_TICKS

mod body;
mod bow;
mod claims;
mod families;
mod guard;
mod keys;
mod strike;
mod swing;

#[cfg(test)]
mod rig_clips;

use body::{BodyClocks, Tools, TICK_SECONDS};
use claims::Rule;
use keys::{BLOCK_SOUND, PACED_COMBO_DATA};
use mod_sdk::*;
use std::collections::HashMap;
use std::rc::Rc;

const BODY_SYSTEM: u32 = 1;
const DAMAGE_HANDLER: u32 = 1;
const IMPACT_HANDLER: u32 = 2;
const RAISE_HANDLER: u32 = 3;
const COMBO_HANDLER: u32 = 4;
const ATTACK_HANDLER: u32 = 5;
const PROJECTILE_HANDLER: u32 = 6;

const IMPACT_EVENT: &str = "combat:shield_impact";
const DEFLECT_SPEED_SCALE: f32 = 0.1;

#[derive(Default)]
struct Combat {
    rules: Vec<Box<dyn Rule>>,
    bow: Option<Rc<bow::Rows>>,
    tools: Tools,
    combo_mobs: Vec<MobId>,
    authority: bool,
    clocks: HashMap<PlayerId, BodyClocks>,
    local: BodyClocks,
}

impl Combat {
    fn clocks_of(&mut self, player: PlayerId) -> &mut BodyClocks {
        if self.authority {
            self.clocks.entry(player).or_default()
        } else {
            &mut self.local
        }
    }

    fn block(
        &mut self,
        victim: PlayerId,
        state: &PlayerSnapshot,
        origin: Option<[f64; 3]>,
    ) -> bool {
        let clocks = self.clocks.entry(victim).or_default();
        if !claims::compose(&self.rules, state, clocks).covers(state, origin) {
            return false;
        }
        emit_sound(BLOCK_SOUND, Some(state.pos));
        clocks.recoil.start();
        emit_event_to(victim, IMPACT_EVENT, &[]);
        true
    }

    fn on_damage(&mut self, payload: &EventPayload) -> Outcome {
        let EventPayload::PlayerDamagePre {
            source: DamageSource::MobAttack { .. },
            origin,
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        let state = player_state();
        let Some(me) = state.id else {
            return Outcome::Continue;
        };
        if self.block(me, &state, *origin) {
            Outcome::Cancel
        } else {
            Outcome::Continue
        }
    }

    /// SERVER: an arrow from this pack landed somewhere.
    ///
    /// Hits a body: damage scales with arrival speed off the arrow row's rungs, archer named as
    /// attacker so knockback/retaliation/`mob_damage_pre` see a real hit, arrow fate becomes
    /// `Consume`.
    /// Hits a player with guard raised toward the flight: bounces back at reduced speed.
    /// Hits a block: keeps engine fate, sticks if the row says so.
    /// Always returns `Continue`, since another rule (poison on the arrow, say) might still act on
    /// this hit.
    fn on_projectile_hit(&mut self, payload: &mut EventPayload) -> Outcome {
        let EventPayload::ProjectileHit {
            entity,
            target,
            pos,
            vel,
            fate,
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        let Some(rows) = self.bow.clone() else {
            return Outcome::Continue;
        };
        let Some(item) = item_entity(*entity) else {
            return Outcome::Continue;
        };
        let Some(arrow) = rows.arrow_named(&item.stack.item) else {
            return Outcome::Continue;
        };
        let speed = (vel[0] * vel[0] + vel[1] * vel[1] + vel[2] * vel[2]).sqrt();
        let roll = || strike::roll(arrow.damage_at(speed), rng_u64("arrow"));
        match target {
            ProjectileTarget::Mob(mob) => {
                damage_mob(*mob, roll(), Some(*pos), item.owner);
                *fate = ProjectileFate::Consume;
            }
            ProjectileTarget::Player(victim) => {
                let came_from = [
                    pos[0] - f64::from(vel[0]),
                    pos[1] - f64::from(vel[1]),
                    pos[2] - f64::from(vel[2]),
                ];
                let blocked = players()
                    .into_iter()
                    .find(|entry| entry.id == *victim)
                    .is_some_and(|entry| self.block(*victim, &entry.state, Some(came_from)));
                if blocked {
                    *fate = ProjectileFate::Deflect {
                        vel: vel.map(|v| -v * DEFLECT_SPEED_SCALE),
                    };
                } else {
                    damage_player(*victim, roll().round() as i32, Some(*pos), item.owner);
                    *fate = ProjectileFate::Consume;
                }
            }
            ProjectileTarget::Block { .. } => {}
        }
        Outcome::Continue
    }

    fn on_attack_attempt(&self, payload: &EventPayload) -> Outcome {
        let EventPayload::AttackAttempt { block, player, .. } = payload else {
            return Outcome::Continue;
        };
        if block.is_some() {
            return Outcome::Continue;
        }
        let state = player_state();
        if state.id != Some(*player) || !self.tools.lands(state.held) {
            return Outcome::Continue;
        }
        Outcome::Cancel
    }

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
            .is_some_and(|entry| self.tools.paces(entry.state.held));
        if paced {
            feedback
                .components
                .retain(|c| !matches!(c, MobDamageFeedbackComponent::Immunity { .. }));
        }
        Outcome::Continue
    }

    fn on_raise(&mut self) -> Outcome {
        let state = player_state();
        let Some(me) = state.id else {
            return Outcome::Continue;
        };
        let Some(owner) = claims::taker(&self.rules, &state) else {
            return Outcome::Continue;
        };
        self.clocks_of(me).press_owner = Some(owner);
        hold_use(me);
        Outcome::Cancel
    }
}

impl Mod for Combat {
    fn init(&mut self) {
        self.tools = Tools::resolve();
        self.combo_mobs = mobs_with_data(PACED_COMBO_DATA)
            .into_iter()
            .map(|(kind, _)| kind)
            .collect();
        if let Some(rows) = bow::Rows::load() {
            let rows = Rc::new(rows);
            self.rules.push(Box::new(bow::BowRule::new(rows.clone())));
            self.bow = Some(rows);
        }
        if let Some(shield) = guard::ShieldRule::resolve() {
            self.rules.push(Box::new(shield));
        }
        if self.rules.is_empty() && self.tools.is_empty() {
            return;
        }
        match runtime_side() {
            RuntimeSide::Server => {
                self.authority = true;
                register_tick_system(Stage::Mining, AttachSide::Before, 0, BODY_SYSTEM);
                register_event_handler(EventKind::PlayerDamagePre, 0, DAMAGE_HANDLER);
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
                register_event_handler(EventKind::MobDamagePre, 0, COMBO_HANDLER);
                register_event_handler(EventKind::AttackAttempt, 0, ATTACK_HANDLER);
                register_event_handler(EventKind::ProjectileHit, 0, PROJECTILE_HANDLER);
            }
            RuntimeSide::Client => {
                register_event_handler(EventKind::ModEvent, 0, IMPACT_HANDLER);
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
            }
            RuntimeSide::Worldgen => {}
        }
    }

    fn tick_system(&mut self, system: u32) {
        debug_assert_eq!(system, BODY_SYSTEM);
        let roster = players();
        self.clocks
            .retain(|id, _| roster.iter().any(|entry| entry.id == *id));
        for entry in &roster {
            let clocks = self.clocks.entry(entry.id).or_default();
            let landed = body::run(
                &self.tools,
                &self.rules,
                entry.id,
                clocks,
                &entry.state,
                true,
                TICK_SECONDS,
            );
            if let Some(style) = landed {
                strike::land(entry.id, self.tools.profile(style), &entry.state);
            }
        }
    }

    fn handle_event(&mut self, handler: u32, payload: &mut EventPayload) -> Outcome {
        match handler {
            COMBO_HANDLER => self.on_mob_damage(payload),
            ATTACK_HANDLER => self.on_attack_attempt(payload),
            PROJECTILE_HANDLER => self.on_projectile_hit(payload),
            DAMAGE_HANDLER => self.on_damage(payload),
            RAISE_HANDLER => self.on_raise(),
            IMPACT_HANDLER => {
                if matches!(payload, EventPayload::ModEvent { key, .. } if key == IMPACT_EVENT) {
                    self.local.recoil.start();
                }
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn client_frame(&mut self, frame: &ClientFrameData) {
        let state = player_state();
        let Some(me) = state.id else {
            return;
        };
        let dt = frame.dt.max(0.0);
        body::run(
            &self.tools,
            &self.rules,
            me,
            &mut self.local,
            &state,
            false,
            dt,
        );
    }
}

register_mod!(Combat);
