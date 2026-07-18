use crate::entity::DroppedItem;
use crate::events::{DamageSource, MobDamagePre, Outcome, PostEvent};
use crate::item::ItemStack;
use crate::mathh::{voxel_at, Vec3};
use crate::mob::{def as mob_def, DeathDrop, MobAttack, MobDamageSound, MobFall, MobSoundCategory};

/// Falls shorter than this into water make no splash — walking or a one-block
/// step-down stays quiet; a real fall throws the burst.
pub(crate) const WATER_SPLASH_MIN_FALL: f32 = 1.5;
/// Falls at least this deep play the BIG splash sound instead of the small one.
const WATER_SPLASH_BIG_FALL: f32 = 5.0;
use crate::player;
use crate::world::World;

use super::game::{ServerGame, ATTACK_COOLDOWN_TICKS};
use super::player::PlayerId;
use crate::game::tick::TickEvents;
use crate::server::health::fall_damage_health;

/// Upward pop of a mob strike's knockback, as a fraction of its horizontal strength —
/// mirrors the mob-side knockback feel (`KNOCKBACK_UP / KNOCKBACK_SPEED` ≈ 0.65 in
/// `mob::instance`), so the player is launched like a mob is when hit.
const MOB_ATTACK_UP_RATIO: f32 = 0.65;

/// Horizontal knockback speed of a player-vs-player melee hit (m/s), with the
/// same [`MOB_ATTACK_UP_RATIO`] upward pop — tuned to read like a mob strike
/// of ordinary strength.
const PVP_ATTACK_KNOCKBACK: f32 = 5.0;

impl ServerGame {
    /// Attack, on the tick: resolve a buffered primary-button press (consumed once, so a
    /// press never lands more than one hit). The damage lands the tick *after* the click —
    /// `pending_attack` is latched per frame and consumed here. Rate-limited by
    /// [`ATTACK_COOLDOWN_TICKS`]: the cooldown counts down one tick at a time and an attack
    /// is refused (no swing, no damage) while it's running, so mashing the button can't
    /// land a hit every tick — only one swing per cooldown connects, so an owl can't be
    /// spam-clicked to death. A swing that connects (a mob hit or a punch at the air) arms
    /// the cooldown and reports `swung_hand`; a click on a block (mining) does neither.
    pub(crate) fn tick_attack(&mut self, s: usize, events: &mut TickEvents) {
        let sess = &mut self.sessions[s];
        sess.attack_cooldown = sess.attack_cooldown.saturating_sub(1);
        // Consume the press AND its targets whether or not it lands (no
        // queuing past one tick); it only resolves once the cooldown elapsed.
        let mob_target = std::mem::take(&mut sess.pending_attack_mob);
        let player_target = std::mem::take(&mut sess.pending_attack_player);
        if !std::mem::take(&mut sess.pending_attack) || sess.attack_cooldown != 0 {
            return;
        }
        if self.resolve_attack(s, mob_target, player_target, events) {
            self.sessions[s].attack_cooldown = ATTACK_COOLDOWN_TICKS;
            events.player(s).swung_hand = true;
        }
    }

    /// Apply one attack swing: damage the targeted mob or PLAYER (rolling the held
    /// weapon's damage; a mob kill spawns loot), or — looking at nothing — punch the air.
    /// Returns whether the hand swung (a mob/player hit or an air punch); a click on a
    /// block doesn't swing (mining is the held action). `mob_target` is only the STABLE
    /// id the click claimed: the authoritative view-ray validator must resolve it to the
    /// nearest live, unoccluded body now. A forged or vanished mob degrades to an air
    /// punch. A click carries at most one of mob/player; a player target that fails
    /// validation (gone, dead, spectator, out of reach) degrades to an air punch the same
    /// way.
    fn resolve_attack(
        &mut self,
        s: usize,
        mob_target: Option<u64>,
        player_target: Option<u8>,
        events: &mut TickEvents,
    ) -> bool {
        if let Some(target) = player_target {
            // The swing happened whether or not the hit validates — mirror
            // the vanished-mob air punch (and arm the cooldown either way).
            self.resolve_player_attack(s, PlayerId(target), events);
            true
        } else if let Some(target) = mob_target {
            if let Some(idx) = self.authoritative_mob_target(s, Some(target)) {
                let damage = self.roll_attack_damage(s);
                let from = self.sessions[s].player.body_center();
                // The pipeline may cancel the damage; the swing still happened and
                // still arms the cooldown.
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
            // A claimed target always swings, even when authority rejects it:
            // the click becomes the same air punch as a vanished target.
            true
        } else {
            // No mob: a punch swing only when looking at nothing.
            self.sessions[s].look.is_none()
        }
    }

    /// The held weapon's damage roll for session `s` (the same roll a mob hit
    /// uses; deterministic off the spawn counter).
    fn roll_attack_damage(&mut self, s: usize) -> f32 {
        let (lo, hi) = crate::item::attack_damage(self.sessions[s].selected_item());
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        lo + crate::entity::hash01(self.spawn_counter as u64) * (hi - lo)
    }

    /// PvP: one validated melee hit on another session, through the single
    /// [`damage_player`](ServerGame::damage_player) funnel with
    /// [`DamageSource::PlayerAttack`]. Validation (any failure = silent no-op — the
    /// swing already happened): the target session exists, is not the
    /// attacker, both ends are alive non-spectators, and the target's body
    /// AABB is within `player::REACH + 1.0` of the attacker's EYE measured to
    /// the AABB's closest point — the same closest-point-plus-slack rule the
    /// block-target reach check uses (`apply_player_update`). An applied hit
    /// shoves the victim horizontally away from the attacker with the mob
    /// strike's upward pop ratio; engine immunity or a cancelled
    /// `player_damage_pre` suppresses damage AND knockback — the same contract
    /// as mob strikes.
    fn resolve_player_attack(&mut self, s: usize, target: PlayerId, events: &mut TickEvents) {
        let Some(t) = self.sessions.iter().position(|sess| sess.id == target) else {
            return; // the clicked player left before the tick
        };
        if t == s {
            return; // self-attack impossible (targeting skips own id; belt and braces)
        }
        let attacker = &self.sessions[s].player;
        if attacker.is_spectator() || attacker.health() == 0 {
            return; // spectators and the dead can't attack
        }
        let victim = &self.sessions[t].player;
        if victim.is_spectator() || victim.health() == 0 {
            return; // spectators and the dead can't be attacked
        }
        let eye = attacker.eye();
        let lo = victim.pos - Vec3::new(player::HALF_W, 0.0, player::HALF_W);
        let hi = victim.pos + Vec3::new(player::HALF_W, player::HEIGHT, player::HALF_W);
        let closest = eye.clamp(lo, hi);
        if (closest - eye).length() > player::REACH + 1.0 {
            return; // out of reach (with the client-camera slack)
        }
        let from = attacker.body_center();
        let victim_center = victim.body_center();
        let damage = self.roll_attack_damage(s);
        let amount = damage.max(0.0).round() as i32;
        let attacker_id = self.sessions[s].id;
        let source = DamageSource::PlayerAttack(attacker_id);
        if self.damage_player(t, amount, source, Some(from), events) {
            let away = victim_center - from;
            let dir = Vec3::new(away.x, 0.0, away.z).normalize_or_zero();
            let impulse = dir * PVP_ATTACK_KNOCKBACK
                + Vec3::new(0.0, PVP_ATTACK_KNOCKBACK * MOB_ATTACK_UP_RATIO, 0.0);
            self.sessions[t].player.apply_knockback(impulse);
        }
    }

    /// THE mob-damage pipeline, shared by every source: reject the victim's
    /// engine-owned immunity, dispatch `mob_damage_pre` (mutable amount,
    /// cancellable), apply what survives through
    /// [`Mobs::damage_mob`](crate::mob::Mobs::damage_mob), and on a kill queue
    /// `mob_died` + roll the loot. Returns whether the request was applied.
    ///
    /// `feedback` composes THIS request's damage pipeline; `None` = the
    /// species' resolved `damage_feedback`. A pipeline without the `Immunity`
    /// component is DoT (burn ticks): neither blocked by an active i-frame
    /// window nor granting one.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn damage_mob_through_pipeline(
        &mut self,
        s: usize,
        idx: usize,
        amount: f32,
        source: DamageSource,
        origin: Option<Vec3>,
        feedback: Option<crate::mob::MobDamageFeedback>,
        events: &mut TickEvents,
    ) -> bool {
        let Some(snapshot) = self
            .world
            .mobs()
            .instances()
            .get(idx)
            .map(|m| (m.kind, m.id(), m.pos, m.is_dead(), m.is_damage_immune()))
        else {
            return false;
        };
        let (kind, mob_id, pos, was_dead, damage_immune) = snapshot;
        let feedback = feedback.unwrap_or_else(|| mob_def(kind).damage_feedback.clone());
        // The i-frame window is itself a pipeline component: only requests
        // whose pipeline participates (`petramond:immunity`) are blocked by
        // an active window. Blocking happens before `mob_damage_pre` — a
        // blocked attempt stays a complete non-event.
        if was_dead || (damage_immune && feedback.has_immunity()) {
            return false;
        }
        let mut pre = MobDamagePre {
            mob_id,
            kind,
            amount,
            source,
            origin,
            feedback,
        };
        let cancelled = {
            let Self {
                world,
                sessions,
                bus,
                ..
            } = self;
            let sess = &mut sessions[s];
            bus.mob_damage_pre(
                world,
                &mut sess.player,
                &mut sess.gui_state,
                events,
                &mut pre,
            ) == Outcome::Cancel
        };
        if cancelled {
            return false;
        }
        if !pre.feedback.has_any_component() {
            return false;
        }
        let soundable_hit = pre.feedback.plays_sound(MobDamageSound::Hurt) && pre.amount > 0.0;
        if let Some(death) = self.world.mobs_mut().damage_mob(
            idx,
            pre.amount,
            pre.origin,
            pre.source.is_attack(),
            pre.source.attacker(),
            &pre.feedback,
        ) {
            if pre.feedback.plays_sound(MobDamageSound::Death) {
                queue_mob_sound(events, mob_id, kind, MobSoundCategory::Death, death.pos);
            }
            self.bus.emit(PostEvent::MobDied {
                id: mob_id,
                kind: death.kind,
                pos: death.pos,
            });
            self.spawn_mob_loot(death);
        } else if soundable_hit {
            queue_mob_sound(events, mob_id, kind, MobSoundCategory::Hurt, pos);
        }
        true
    }

    /// Apply the melee strikes the mobs landed this tick (drained from
    /// `World::tick_mobs`), routing each by its target:
    ///
    /// - a PLAYER target runs through the single [`damage_player`] funnel — so
    ///   engine immunity and `player_damage_pre` cancellation both drop the
    ///   damage and knockback — and an applied strike shoves the player away
    ///   from the attacker with an upward pop. Spectators have no body to hit:
    ///   those strikes are dropped whole.
    /// - a MOB target runs through the shared mob damage pipeline
    ///   (`mob_damage_pre`, the row's feedback bundle, loot, ragdoll) with the
    ///   striking mob as source and origin — mob-vs-mob combat is the same
    ///   funnel as every other mob hit, so the victim's knockback comes from
    ///   its own `petramond:knockback` feedback component and its retaliation
    ///   memory records the biter.
    ///
    /// [`damage_player`]: ServerGame::damage_player
    pub(crate) fn apply_mob_attacks(&mut self, attacks: Vec<MobAttack>, events: &mut TickEvents) {
        for a in attacks {
            match a.target {
                crate::mob::EntityRef::Player(pid) => {
                    // A session gone mid-tick can't happen — the session list
                    // only changes between ticks.
                    let Some(s) = self.sessions.iter().position(|sess| sess.id == pid) else {
                        continue;
                    };
                    if self.sessions[s].player.is_spectator() {
                        continue;
                    }
                    let amount = a.damage.max(0.0).round() as i32;
                    let source = DamageSource::MobAttack {
                        kind: a.mob,
                        id: a.mob_id,
                    };
                    if self.damage_player(s, amount, source, Some(a.origin), events) {
                        let impulse = a.knockback_dir * a.knockback
                            + Vec3::new(0.0, a.knockback * MOB_ATTACK_UP_RATIO, 0.0);
                        self.sessions[s].player.apply_knockback(impulse);
                    }
                }
                crate::mob::EntityRef::Mob(target_id) => {
                    // Resolve the STABLE id only now: earlier strikes this tick
                    // may have killed mobs and shifted indices.
                    let Some(idx) = self.world.mobs().index_of_id(target_id) else {
                        continue;
                    };
                    self.damage_mob_through_pipeline(
                        0,
                        idx,
                        a.damage.max(0.0),
                        DamageSource::MobAttack {
                            kind: a.mob,
                            id: a.mob_id,
                        },
                        Some(a.origin),
                        None,
                        events,
                    );
                }
            }
        }
    }

    /// Apply fall landings reported by `World::tick_mobs` through the mob damage
    /// pipeline. Mobs use the same distance curve as players, but fall damage is not an
    /// attack and carries no origin, so default knockback does not run.
    pub(crate) fn apply_mob_fall_damage(&mut self, falls: Vec<MobFall>, events: &mut TickEvents) {
        for fall in falls {
            let amount = fall_damage_health(fall.distance) as f32;
            if amount <= 0.0 {
                continue;
            }
            let Some(idx) = self.world.mobs().index_of_id(fall.mob_id) else {
                continue;
            };
            self.damage_mob_through_pipeline(0, idx, amount, DamageSource::Fall, None, None, events);
        }
    }

    /// Queue the core `petramond:water_splash` burst at the water surface above
    /// `feet` — a one-shot every client presents. `fall` (blocks) is the burst
    /// intensity: harder falls throw more droplets (the bundle's
    /// `count_per_intensity` scales the count). Falls below
    /// [`WATER_SPLASH_MIN_FALL`] stay quiet, so walking or stepping into water
    /// never splashes.
    pub(crate) fn push_water_splash(&mut self, feet: Vec3, fall: f32, events: &mut TickEvents) {
        if fall < WATER_SPLASH_MIN_FALL {
            return;
        }
        let Some(bundle) =
            crate::particle_emitters::by_key(crate::particle_emitters::WATER_SPLASH_KEY)
        else {
            return;
        };
        let c = crate::mathh::voxel_at(feet);
        // The entry cell (or the one below, when the feet sit at the boundary).
        let mut top = if self.world.water_cell_at(c.x, c.y, c.z) {
            c.y
        } else if self.world.water_cell_at(c.x, c.y - 1, c.z) {
            c.y - 1
        } else {
            return;
        };
        // The splash throws from the TOP of the water: climb to the surface
        // cell (bounded — a plunge never starts more than a few cells deep).
        while top - c.y < 8 && self.world.water_cell_at(c.x, top + 1, c.z) {
            top += 1;
        }
        let pos = Vec3::new(feet.x, top as f32 + 1.02, feet.z);
        events.world.emitter_bursts.push((bundle.id, pos, fall));
        // The splash SOUND rides the ordinary one-shot sound channel (the
        // emitter catalog is particles-only); the fall depth picks the clip.
        let sound = if fall >= WATER_SPLASH_BIG_FALL {
            crate::audio::Sound::WaterSplashBig
        } else {
            crate::audio::Sound::WaterSplashSmall
        };
        events.world.sounds.push(crate::game::ModSound {
            sound,
            pos: Some(pos),
        });
    }

    /// Record the gameplay noise of session `s` acting on the block at `pos`
    /// (place/break), for hearing-based mob AI. The noise sounds at the BLOCK's
    /// centre and names the acting player — that's what a listener locks onto.
    /// Natural (sim-caused) breaks stay silent: they have no actor.
    pub(crate) fn push_block_noise(
        &mut self,
        s: usize,
        pos: crate::mathh::IVec3,
        kind: crate::mob::NoiseKind,
    ) {
        self.world.push_noise(crate::mob::Noise {
            pos: Vec3::new(pos.x as f32, pos.y as f32, pos.z as f32) + Vec3::splat(0.5),
            kind,
            source: crate::mob::EntityRef::Player(self.sessions[s].id),
        });
    }

    /// Record every audibly-moving player's footstep noise for this tick's mob
    /// AI batch — called once per tick right before the mob stage. Sneaking
    /// players are silent (the whole point of sneaking near a listener);
    /// airborne players are silent until they land.
    pub(crate) fn push_player_step_noises(&mut self) {
        for s in 0..self.sessions.len() {
            let p = &self.sessions[s].player;
            let horizontal_sq = p.vel.x * p.vel.x + p.vel.z * p.vel.z;
            if crate::mob::player_steps_are_audible(
                horizontal_sq,
                p.on_ground,
                self.sessions[s].sneaking(),
                p.is_spectator(),
            ) {
                let noise = crate::mob::Noise {
                    pos: p.pos,
                    kind: crate::mob::NoiseKind::Step,
                    source: crate::mob::EntityRef::Player(self.sessions[s].id),
                };
                self.world.push_noise(noise);
            }
        }
    }

    /// Roll a dead mob's loot table and scatter the drops at its body. Called the
    /// instant a mob dies (from the attack that killed it), so loot appears "when
    /// killed" while the corpse ragdolls. No-op for a species with no table.
    pub(crate) fn spawn_mob_loot(&mut self, death: DeathDrop) {
        let Some(table) = self.loot.get(crate::mob::def(death.kind).key) else {
            return;
        };
        self.spawn_counter = self.spawn_counter.wrapping_add(1);
        let stacks = table.roll(self.spawn_counter as u64);
        // Pop from roughly the mob's body centre so drops don't clip into the floor.
        let centre = death.pos + Vec3::new(0.0, 0.3, 0.0);
        for stack in stacks {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let mut drop = DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = death.skylight;
            drop.blocklight = death.blocklight;
            self.world.spawn_item(drop);
        }
    }

    /// Per game-tick (20 TPS) pickup for player `s`: pull any eligible drop
    /// within their pickup radius into their inventory. Item lifetime advances
    /// once per tick in the stage driver, not here. Returns whether at least
    /// one item was collected this tick, so the client can play the pickup sound.
    pub(crate) fn item_pickup_tick(&mut self, s: usize) -> bool {
        // A dead body vacuums nothing: without this the corpse standing at the
        // death spot would re-collect its own spilled inventory behind the
        // death screen.
        if self.sessions[s].player.health() == 0 {
            return false;
        }
        let requester = self.sessions[s].id;
        let player_pos = self.sessions[s].player.body_center();
        // Plan first against a cloned inventory, reserving capacity without
        // mutating the real slots. Only drops requested BY this player are
        // allowed to magnet toward (and be collected by) them.
        let mut planned = self.sessions[s].player.inventory.clone();
        self.world
            .dropped_items_mut()
            .request_pickups(requester, player_pos, |stack| {
                let count = planned.fits_count(stack);
                if count > 0 {
                    let leftover = planned.add(ItemStack::new(stack.item, count));
                    debug_assert!(
                        leftover.is_none(),
                        "fits_count overestimated pickup capacity"
                    );
                }
                count
            });

        // Borrow-split: `dropped_items_mut()` borrows the drops, the session
        // owns the inventory — disjoint `ServerGame` fields, so this type-checks without
        // aliasing. Actual inventory mutation only happens after a requested drop
        // reaches the absorb radius.
        let inventory = &mut self.sessions[s].player.inventory;
        let mut collected = false;
        self.world
            .dropped_items_mut()
            .collect_requested_pickups(requester, player_pos, |stack| {
                collected = true;
                inventory.add(stack)
            });
        collected
    }
}

fn queue_mob_sound(
    events: &mut TickEvents,
    mob_id: u64,
    kind: crate::mob::Mob,
    category: MobSoundCategory,
    pos: Vec3,
) {
    if crate::mob::def(kind).sound_for(category).is_some() {
        events.world.mob_sounds.push(crate::game::MobSoundEvent {
            mob_id,
            kind,
            category,
            pos,
        });
    }
}

/// The two 6-bit light channels `(sky6, block6)` for dynamic geometry at a world
/// position, so the held item, particles, and dropped items are lit by torches
/// just like the static blocks around them (and torch light survives the night).
pub(crate) fn light_at_pos(world: &World, pos: Vec3) -> (u8, u8) {
    let c = voxel_at(pos);
    (
        world.skylight6_at_world(c.x, c.y, c.z),
        world.blocklight6_at_world(c.x, c.y, c.z),
    )
}
