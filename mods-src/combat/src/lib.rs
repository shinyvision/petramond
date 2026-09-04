//! combat — the shield, the bow, and the tools' hands.
//!
//! A shield, crafted at the pack's weapons workbench (4 planks + 4 iron
//! ingots), raised by holding the use button. While it is up, monster melee
//! and arrows coming at your FRONT are stopped, the body moves at half
//! speed, and the hands are barred from attacking, mining and interacting.
//! A hit it absorbs knocks it aside for [`IMPACT_TICKS`], during which the
//! next attacker gets through.
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
//! ## The bow
//!
//! A bow in the main hand takes the use press and DRAWS while it is held
//! (the law in [`bow`]): the row's ticks to full, shown through the pull
//! frames on the generic held-DISPLAY seam, the arms in the archer's
//! stance, the body slowed and its hands committed. Letting go takes one
//! arrow from the pack and LAUNCHES it through the engine's flying-item
//! primitive; the `projectile_hit` handler here lands the strike on what
//! it arrives at, harder the faster it arrived — or into a raised shield
//! it arrives at from the front, which stops it — and spends the arrow in
//! the wound; an arrow that met a block keeps the engine's own fate,
//! lodged there to be pulled out again.
//!
//! ## The rules are a list
//!
//! Every use-press rule (the bow, the guard, the next one) is one
//! [`claims::Rule`] in ONE ordered list — precedence is list order. The
//! raise handler asks the list who takes a press; the tick, the frame and
//! the damage handlers fold the rules' claims over it ([`claims::compose`]),
//! and [`body::run`] writes each seam once. Adding a rule is one entry.
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
//! [`guard`], the bow's in [`bow`], the swing law in [`swing`], the strike
//! law in [`strike`], and [`body`] merges every claim into ONE write per
//! body seam. This file only routes:
//!
//! - The **server** tick system runs every player's body per tick, and
//!   lands the strike of any swing whose impact played this tick.
//! - The **client** frame hook runs the local player a round trip
//!   earlier. Both halves, never one: a raised shield that gets to the
//!   screen before the arm holding it is the shield detached from its own
//!   fist, and a swing that plays in first person only is a fist holding a
//!   still tool.
//! - **Blocking** re-runs the rules against the victim's live snapshot and
//!   cancels a frontal `MobAttack` hit (`player_damage_pre`) or drops a
//!   frontal arrow (`projectile_hit`). Falls, PvP melee and other mods'
//!   damage pass, and a cancelled hit applies no knockback either.
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
mod bow;
mod claims;
mod guard;
mod strike;
mod swing;

use body::{BodyClocks, Tools, TICK_SECONDS};
use claims::Rule;
use guard::BLOCK_SOUND;
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

#[derive(Default)]
struct Combat {
    /// The pack's use-press rules in precedence order: the bow (a bow in
    /// the MAIN hand draws over a shield carried in the off hand), then the
    /// guard. A rule whose rows this build lacks is simply not in the list.
    rules: Vec<Box<dyn Rule>>,
    /// The bow's rows, shared with its rule: the arrow half of the law
    /// (what a hit is, what it does) is the projectile handler's, not a
    /// body rule's.
    bow: Option<Rc<bow::Rows>>,
    /// The tool table.
    tools: Tools,
    /// The [`COMBO_MOBS`] this build's registry actually carries — a
    /// species from a pack that is not installed is one row of policy that
    /// never applies, resolved once at init.
    combo_mobs: Vec<MobId>,
    /// This instance is the server: it acts on edges and makes the
    /// simulation claims; a client only shows.
    authority: bool,
    /// SERVER: one set of clocks per body, pruned against the roster each
    /// tick, so a leaver's slot dies with their session.
    clocks: HashMap<PlayerId, BodyClocks>,
    /// CLIENT: the same clocks, local player only.
    local: BodyClocks,
}

impl Combat {
    /// The clocks stepping `player`'s body on this instance.
    fn clocks_of(&mut self, player: PlayerId) -> &mut BodyClocks {
        if self.authority {
            self.clocks.entry(player).or_default()
        } else {
            &mut self.local
        }
    }

    /// SERVER: does `victim`'s guard stop a hit arriving from `origin`? The
    /// rules re-run on their live snapshot decide — no cached flag to go
    /// stale or hit the wrong body. A block is heard, knocks the shield
    /// aside, and tells the wielder's client so.
    fn block(
        &mut self,
        victim: PlayerId,
        state: &PlayerSnapshot,
        origin: Option<[f32; 3]>,
    ) -> bool {
        let clocks = self.clocks.entry(victim).or_default();
        if !claims::compose(&self.rules, state, clocks).covers(state, origin) {
            return false;
        }
        // Spatial: a block is something bystanders hear too.
        emit_sound(BLOCK_SOUND, Some(state.pos));
        clocks.recoil.start();
        // Only the wielder needs telling: every other screen picks the recoil
        // up from the replicated pose the tick system publishes anyway.
        emit_event_to(victim, IMPACT_EVENT, &[]);
        true
    }

    /// The blocking half of `player_damage_pre`: cancel a frontal monster
    /// strike, and knock the shield aside for doing it.
    fn on_damage(&mut self, payload: &EventPayload) -> Outcome {
        // Falls, PvP and other mods' damage are not the shield's job.
        let EventPayload::PlayerDamagePre {
            source: DamageSource::MobAttack { .. },
            origin,
            ..
        } = payload
        else {
            return Outcome::Continue;
        };
        // The dispatch names its victim.
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

    /// SERVER: an arrow of this pack's arrived somewhere. A BODY takes the
    /// strike — damage by arrival speed off the arrow row's own rungs, the
    /// archer named as the attacker so knockback, retaliation and every
    /// `mob_damage_pre` handler see a real hit — and the arrow's fate
    /// becomes `Consume`, spent in the wound; a PLAYER whose raised guard
    /// faces the flight stops it instead, and it drops at their feet. A
    /// BLOCK keeps the engine's fate: the row says it sticks, so it lodges
    /// there. Always `Continue`: another pack's rule (a poison on the
    /// arrow's data) may still act on the same hit.
    fn on_projectile_hit(&mut self, payload: &mut EventPayload) -> Outcome {
        let EventPayload::ProjectileHit {
            entity,
            target,
            pos,
            vel,
            fate,
        } = payload
        else {
            return Outcome::Continue;
        };
        let Some(rows) = self.bow.clone() else {
            return Outcome::Continue;
        };
        // The entity is live for the whole dispatch: its stack says whether
        // this is one of the pack's arrows, and which.
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
                // The arrow came from back along its flight: that is the
                // direction the guard has to be facing.
                let came_from = [pos[0] - vel[0], pos[1] - vel[1], pos[2] - vel[2]];
                let blocked = players()
                    .into_iter()
                    .find(|entry| entry.id == *victim)
                    .is_some_and(|entry| self.block(*victim, &entry.state, Some(came_from)));
                if blocked {
                    *fate = ProjectileFate::Drop;
                } else {
                    damage_player(*victim, roll().round() as i32, Some(*pos), item.owner);
                    *fate = ProjectileFate::Consume;
                }
            }
            ProjectileTarget::Block { .. } => {}
        }
        Outcome::Continue
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
        if state.id != Some(*player) || !self.tools.lands(state.held) {
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
            .is_some_and(|entry| self.tools.paces(entry.state.held));
        if paced {
            feedback
                .components
                .retain(|c| !matches!(c, MobDamageFeedbackComponent::Immunity { .. }));
        }
        Outcome::Continue
    }

    /// The press nothing else wanted: the first rule in the list that takes
    /// it holds the gesture until the button comes up, and is remembered as
    /// its owner on this instance's clocks. Taking it is not an interaction,
    /// so nothing jabs; `Cancel` only stops later handlers seeing it.
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
        // Registry-only, and legal on every instance (server, worldgen,
        // client) — the client half needs the rows just as much.
        self.tools = Tools::resolve();
        self.combo_mobs = COMBO_MOBS
            .iter()
            .filter_map(|name| resolve_mob(name))
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
                // The tick's earliest seam, so a guard raised this tick is up
                // before the Mobs stage swings at it — and the swings'
                // answers publish within the same pass.
                register_tick_system(Stage::Mining, AttachSide::Before, 0, BODY_SYSTEM);
                register_event_handler(EventKind::PlayerDamagePre, 0, DAMAGE_HANDLER);
                register_event_handler(EventKind::UseUnclaimed, 0, RAISE_HANDLER);
                register_event_handler(EventKind::MobDamagePre, 0, COMBO_HANDLER);
                register_event_handler(EventKind::AttackAttempt, 0, ATTACK_HANDLER);
                register_event_handler(EventKind::ProjectileHit, 0, PROJECTILE_HANDLER);
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

    /// Run every player's body, and land the strike of any swing whose
    /// impact played this tick. Idempotent every tick — no edge to miss; a
    /// released claim is just the neutral write.
    fn tick_system(&mut self, system: u32) {
        debug_assert_eq!(system, BODY_SYSTEM);
        // The roster is the snapshot of truth: prune the clocks of anyone
        // gone, then one lookup per body.
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
                strike::land(entry.id, style, &entry.state);
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
            // The wielder's client hearing that its shield just took a hit.
            // Starting the clock is the whole handler; what the recoil looks
            // like is the shared rule's business on both sides.
            IMPACT_HANDLER => {
                if matches!(payload, EventPayload::ModEvent { key, .. } if key == IMPACT_EVENT) {
                    self.local.recoil.start();
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
