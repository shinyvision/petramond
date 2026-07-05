//! Player health: fall damage on the tick, the damage funnel, and the HUD read model.
//!
//! Physics only *measures* a fall (per-frame, for local feel — see
//! [`crate::player::Player::track_fall`]); the health *mutation* happens here, on the
//! deterministic game tick, so it stays multiplayer-safe (no wall-clock, no RNG).
//! Every damage source — falling is today's only one — must route through the single
//! [`damage_player`](Game::damage_player) funnel so the `player_damage_pre` /
//! `player_damaged` / `player_died` events fire consistently.

use crate::events::{DamageSource, Outcome, PlayerDamagePre, PostEvent};
use crate::gui::HealthView;
use crate::player::MAX_HEALTH;

use super::tick::TickEvents;
use super::Game;

/// Blocks you can fall with no damage. Beyond this, each further whole block costs one
/// half-heart, so a 4-block fall (one past the safe 3) is the first to hurt — half a
/// heart (1 health point) — matching "a fall from 4 blocks hurts for 0.5 hearts".
const SAFE_FALL_BLOCKS: f32 = 3.0;

/// Float slack (blocks) absorbing the ~1-ULP rounding the collision sweep leaves in a
/// landing's `y`: without it a clean N-block fall can measure N − ε and fall short of
/// the whole-block floor boundary, dealing no damage. Far smaller than any real
/// fractional fall (slab/jump geometry), so it never lifts an honestly sub-threshold
/// fall over the line.
const FALL_EPS: f32 = 1e-3;

/// Half-hearts of fall damage for a landing that fell `distance` blocks: the whole
/// blocks past the safe distance, never negative. `3 → 0`, `4 → 1`, `5 → 2`, ….
pub(super) fn fall_damage_health(distance: f32) -> i32 {
    (distance - SAFE_FALL_BLOCKS + FALL_EPS).floor().max(0.0) as i32
}

impl Game {
    /// Consume the landing the player's physics latched and apply its fall damage on
    /// the tick. Spectators float, so their (absent) fall is drained without harm.
    pub(super) fn tick_fall_damage(&mut self, events: &mut TickEvents) {
        let distance = self.player.take_fall_distance();
        if self.player.is_spectator() {
            return;
        }
        self.damage_player(fall_damage_health(distance), DamageSource::Fall, events);
    }

    /// The single player-damage funnel: dispatch `player_damage_pre` (mutable
    /// amount, cancellable — i-frames live here), apply what survives, queue
    /// `player_damaged`, and fire `player_died` exactly once per >0 → 0 health
    /// transition. There is NO default death consequence — the event just fires;
    /// a mod (or future core content) decides what death means.
    ///
    /// Returns whether damage was actually applied, so a caller can gate the
    /// side effects that must die with a cancelled hit (a mob strike's knockback).
    pub(super) fn damage_player(
        &mut self,
        amount: i32,
        source: DamageSource,
        events: &mut TickEvents,
    ) -> bool {
        // Non-positive damage is a non-event (matching Player::apply_damage's
        // no-op); the fall drain calls this every tick, so dispatching zeros
        // would spam handlers 20×/s.
        if amount <= 0 {
            return false;
        }
        // A dead player takes no further hits: without this, mobs pounding the
        // corpse behind the death screen would re-fire `player_damaged` (hurt
        // sound + shake) every strike and knock the body around.
        if self.player.health() == 0 {
            return false;
        }
        let mut pre = PlayerDamagePre { amount, source };
        if self
            .bus
            .player_damage_pre(&mut self.world, &mut self.player, events, &mut pre)
            == Outcome::Cancel
        {
            return false;
        }
        if pre.amount <= 0 {
            return false;
        }
        let was_alive = self.player.health() > 0;
        self.player.apply_damage(pre.amount);
        let new_health = self.player.health();
        events.player_damaged = true;
        // Being hurt in bed ends the sleep immediately — it never continues
        // through a fight (and a lethal hit hands straight over to death).
        self.interrupt_sleep(events);
        self.bus.emit(PostEvent::PlayerDamaged {
            amount: pre.amount,
            new_health,
        });
        // The transition check keeps this a one-shot: further damage at 0 health
        // (or the zero-damage fall drain) can never re-fire it.
        if was_alive && new_health == 0 {
            events.player_died = true;
            self.spill_inventory_on_death();
            self.bus.emit(PostEvent::PlayerDied);
        }
        true
    }

    /// Death spills everything the player carried as item entities at the
    /// body — the classic corpse pile, waiting where they died.
    fn spill_inventory_on_death(&mut self) {
        // An open container session first returns its transient contents
        // (craft grid, cursor stack) to the inventory, so they spill too
        // instead of quietly surviving in a menu the app closes a frame later.
        self.close_open_menu();
        let centre = self.player.body_center();
        let mut stacks: Vec<crate::item::ItemStack> = Vec::new();
        for i in 0..crate::inventory::TOTAL_SLOTS {
            if let Some(slot) = self.player.inventory.slot_mut(i) {
                if let Some(stack) = slot.take() {
                    stacks.push(stack);
                }
            }
        }
        if let Some(stack) = self.player.inventory.take_cursor() {
            stacks.push(stack);
        }
        let cell = (
            centre.x.floor() as i32,
            centre.y.floor() as i32,
            centre.z.floor() as i32,
        );
        let (sky, blk, _) = self.world.dynamic_light_at_world(cell.0, cell.1, cell.2);
        for stack in stacks {
            self.spawn_counter = self.spawn_counter.wrapping_add(1);
            let mut drop = crate::entity::DroppedItem::new(centre, stack, self.spawn_counter);
            drop.skylight = sky;
            drop.blocklight = blk;
            self.world.spawn_item(drop);
        }
    }

    /// The player's health for the HUD hearts, or `None` when there is no survival
    /// bar to draw (a floating spectator). `(current, max)` in half-heart points.
    pub fn player_health(&self) -> Option<HealthView> {
        if self.player.is_spectator() {
            return None;
        }
        Some(HealthView {
            current: self.player.health(),
            max: MAX_HEALTH,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_fall_is_free_and_four_blocks_is_half_a_heart() {
        // Nothing up to and including the safe distance; the first damage is at 4 blocks.
        assert_eq!(fall_damage_health(0.0), 0);
        assert_eq!(fall_damage_health(3.0), 0, "3-block fall is safe");
        assert_eq!(fall_damage_health(3.9), 0, "under 4 blocks: no damage");
        assert_eq!(fall_damage_health(4.0), 1, "4 blocks = 0.5 hearts");
    }

    #[test]
    fn damage_scales_one_half_heart_per_block_past_the_safe_distance() {
        assert_eq!(fall_damage_health(5.0), 2);
        assert_eq!(fall_damage_health(12.0), 9);
        // A huge fall just returns a large amount; the clamp to 0 lives in apply_damage.
        assert_eq!(fall_damage_health(103.0), 100);
    }

    #[test]
    fn a_clean_four_block_fall_still_hurts_despite_landing_rounding() {
        // The collision sweep can leave the landing a hair high, so a nominal 4.0 fall
        // arrives as 4 − ~1 ULP. FALL_EPS must keep it a half-heart, not silently zero.
        assert_eq!(fall_damage_health(4.0 - 8e-6), 1);
    }
}
