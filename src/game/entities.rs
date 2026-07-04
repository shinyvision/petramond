use crate::entity::DroppedItem;
use crate::events::{DamageSource, MobHurtPre, Outcome, PostEvent};
use crate::item::ItemStack;
use crate::mathh::{voxel_at, Vec3};
use crate::mob::{DeathDrop, MobAttack, MobSoundCategory};
use crate::world::World;

use super::{tick::TickEvents, Game, ATTACK_COOLDOWN_TICKS};

/// Upward pop of a mob strike's knockback, as a fraction of its horizontal strength —
/// mirrors the mob-side knockback feel (`KNOCKBACK_UP / KNOCKBACK_SPEED` ≈ 0.65 in
/// `mob::instance`), so the player is launched like a mob is when hit.
const MOB_ATTACK_UP_RATIO: f32 = 0.65;

impl Game {
    /// Attack, on the tick: resolve a buffered primary-button press (consumed once, so a
    /// press never lands more than one hit). The damage lands the tick *after* the click —
    /// `pending_attack` is latched per frame and consumed here. Rate-limited by
    /// [`ATTACK_COOLDOWN_TICKS`]: the cooldown counts down one tick at a time and an attack
    /// is refused (no swing, no damage) while it's running, so mashing the button can't
    /// land a hit every tick — only one swing per cooldown connects, so an owl can't be
    /// spam-clicked to death. A swing that connects (a mob hit or a punch at the air) arms
    /// the cooldown and reports `swung_hand`; a click on a block (mining) does neither.
    pub(super) fn tick_attack(&mut self, events: &mut TickEvents) {
        self.attack_cooldown = self.attack_cooldown.saturating_sub(1);
        // Consume the press whether or not it lands (no queuing past one tick); it only
        // resolves once the cooldown has elapsed.
        if !std::mem::take(&mut self.pending_attack) || self.attack_cooldown != 0 {
            return;
        }
        if self.resolve_attack(events) {
            self.attack_cooldown = ATTACK_COOLDOWN_TICKS;
            events.swung_hand = true;
        }
    }

    /// Apply one attack swing: damage the targeted mob (rolling the held weapon's damage
    /// and spawning loot if the hit kills it), or — looking at nothing — punch the air.
    /// Returns whether the hand swung (a mob hit or an air punch); a click on a block
    /// doesn't swing (mining is the held action). Reads the `targeted_mob` / `look`
    /// sampled this frame, before any mob tick has shifted indices.
    fn resolve_attack(&mut self, events: &mut TickEvents) -> bool {
        if let Some(idx) = self.targeted_mob {
            let (lo, hi) = crate::item::attack_damage(self.selected_item());
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let damage = lo + crate::entity::hash01(self.spawn_counter as u64) * (hi - lo);
            let from = self.player.body_center();
            // The pipeline may cancel the damage; the swing still happened and
            // still arms the cooldown.
            self.hurt_mob_through_pipeline(idx, damage, from, events);
            true
        } else {
            // No mob: a punch swing only when looking at nothing.
            self.look.is_none()
        }
    }

    /// THE mob-hurt pipeline, shared by player attacks and mod `HurtMob`
    /// actions: dispatch `mob_hurt_pre` (mutable amount, cancellable), apply
    /// what survives through [`Mobs::hurt_mob`](crate::mob::Mobs::hurt_mob),
    /// and on a kill queue `mob_died` + roll the loot. Returns whether damage
    /// was applied (false = no such mob or a handler cancelled).
    pub(super) fn hurt_mob_through_pipeline(
        &mut self,
        idx: usize,
        amount: f32,
        from: Vec3,
        events: &mut TickEvents,
    ) -> bool {
        let Some(snapshot) = self
            .world
            .mobs()
            .instances()
            .get(idx)
            .map(|m| (m.kind, m.id(), m.pos, m.is_dead()))
        else {
            return false;
        };
        let (kind, mob_id, pos, was_dead) = snapshot;
        let mut pre = MobHurtPre {
            mob: idx,
            kind,
            amount,
            source: from,
        };
        if self
            .bus
            .mob_hurt_pre(&mut self.world, &mut self.player, events, &mut pre)
            == Outcome::Cancel
        {
            return false;
        }
        let soundable_hit = pre.amount > 0.0 && !was_dead;
        if let Some(death) = self.world.mobs_mut().hurt_mob(idx, pre.amount, from) {
            queue_mob_sound(events, mob_id, kind, MobSoundCategory::Death, death.pos);
            self.bus.emit(PostEvent::MobDied {
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
    /// `World::tick_mobs`): each runs through the single [`damage_player`] funnel —
    /// so `player_damage_pre` handlers (i-frames) see it and a cancel drops BOTH the
    /// damage and the knockback — and an applied strike shoves the player away from
    /// the attacker with an upward pop, mirroring the mob-side knockback feel.
    /// Spectators have no body to hit: strikes are dropped whole.
    ///
    /// [`damage_player`]: Game::damage_player
    pub(super) fn apply_mob_attacks(&mut self, attacks: Vec<MobAttack>, events: &mut TickEvents) {
        for a in attacks {
            if self.player.is_spectator() {
                continue;
            }
            let amount = a.damage.max(0.0).round() as i32;
            if self.damage_player(amount, DamageSource::Mob(a.mob), events) {
                let impulse = a.knockback_dir * a.knockback
                    + Vec3::new(0.0, a.knockback * MOB_ATTACK_UP_RATIO, 0.0);
                self.player.apply_knockback(impulse);
            }
        }
    }

    /// Roll a dead mob's loot table and scatter the drops at its body. Called the
    /// instant a mob dies (from the attack that killed it), so loot appears "when
    /// killed" while the corpse ragdolls. No-op for a species with no table.
    pub(super) fn spawn_mob_loot(&mut self, death: DeathDrop) {
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

    /// Per-frame presentation update: only particles, which are a purely visual effect
    /// (they don't touch the world). Everything that simulates the world or its entities —
    /// mob AI/physics AND dropped-item physics — runs on the fixed game tick (see
    /// [`game_tick_step`](Self::game_tick_step)); the renderer interpolates between ticks.
    pub(super) fn tick_entities(&mut self, dt: f32) {
        self.particles.tick(dt, &self.world);
    }

    /// Per game-tick (20 TPS) item maintenance: advance every drop's lifetime
    /// timer (despawning those past their 5-minute limit) and pull any eligible
    /// drop within the player's pickup radius into the inventory. Driven from the
    /// fixed-tick loop, so both are paced by the simulation clock. Returns whether at
    /// least one item was collected this tick, so the client can play the pickup sound.
    pub(super) fn item_pickup_tick(&mut self) -> bool {
        self.world.tick_item_lifetime();
        let player_pos = self.player.body_center();
        // Plan first against a cloned inventory, reserving capacity without
        // mutating the real slots. Only requested drops are allowed to magnet.
        let mut planned = self.player.inventory.clone();
        self.world
            .dropped_items_mut()
            .request_pickups(player_pos, |stack| {
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

        // Borrow-split: `dropped_items_mut()` borrows the drops, `self.player`
        // owns the inventory — disjoint `Game` fields, so this type-checks without
        // aliasing. Actual inventory mutation only happens after a requested drop
        // reaches the absorb radius.
        let inventory = &mut self.player.inventory;
        let mut collected = false;
        self.world
            .dropped_items_mut()
            .collect_requested_pickups(player_pos, |stack| {
                collected = true;
                inventory.add(stack)
            });
        collected
    }

    pub(super) fn refresh_dropped_item_lights_after_world_light_update(&mut self) {
        let revision = self.world.lighting_revision();
        if self.dropped_light_revision == revision {
            return;
        }
        self.world.refresh_item_lights();
        self.dropped_light_revision = revision;
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
        events.mob_sounds.push(crate::game::MobSoundEvent {
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
pub(super) fn light_at_pos(world: &World, pos: Vec3) -> (u8, u8) {
    let c = voxel_at(pos);
    (
        world.skylight6_at_world(c.x, c.y, c.z),
        world.blocklight6_at_world(c.x, c.y, c.z),
    )
}
