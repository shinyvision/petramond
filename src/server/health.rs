use crate::events::{DamageSource, Outcome, PlayerDamagePre, PostEvent};
use petramond_world::damage::Immunity;

use super::game::ServerGame;
use crate::events::tick::TickEvents;

const SAFE_FALL_BLOCKS: f32 = 3.0;

const FALL_EPS: f32 = 1e-3;

pub fn fall_damage_health(distance: f32) -> i32 {
    (distance - SAFE_FALL_BLOCKS + FALL_EPS).floor().max(0.0) as i32
}

impl ServerGame {
    pub fn tick_damage_immunity(&mut self) {
        for session in &mut self.sessions {
            session.player.tick_damage_immunity();
        }
        self.world.mobs_mut().tick_damage_immunity();
    }

    pub fn tick_fall_damage(&mut self, s: usize, events: &mut TickEvents) {
        let distance = std::mem::replace(&mut self.sessions[s].sim.pending_fall, 0.0);
        if self.sessions[s].player.is_spectator() {
            return;
        }
        self.damage_player(
            s,
            fall_damage_health(distance),
            DamageSource::Fall,
            None,
            events,
        );
    }

    pub fn tick_fluid_splash(&mut self, s: usize, events: &mut TickEvents) {
        let fall = std::mem::replace(&mut self.sessions[s].sim.pending_splash, 0.0);
        if self.sessions[s].player.is_spectator() {
            return;
        }
        let feet = self.sessions[s].player.pos;
        self.push_fluid_splash(feet, fall, events);
    }

    pub fn damage_player(
        &mut self,
        s: usize,
        amount: i32,
        source: DamageSource,
        origin: Option<petramond_math::world_pos::WorldPos>,
        events: &mut TickEvents,
    ) -> bool {
        self.damage_player_through_funnel(s, amount, source, origin, Immunity::PLAYER, events)
    }

    pub fn damage_player_through_funnel(
        &mut self,
        s: usize,
        amount: i32,
        source: DamageSource,
        origin: Option<petramond_math::world_pos::WorldPos>,
        immunity: Immunity,
        events: &mut TickEvents,
    ) -> bool {
        if amount <= 0 || self.sessions[s].player.is_invulnerable() {
            return false;
        }
        if self.sessions[s].player.health() == 0 {
            return false;
        }
        if immunity.blocks(self.sessions[s].player.damage_immunity()) {
            return false;
        }
        let mut pre = PlayerDamagePre {
            amount,
            source,
            origin,
        };
        let cancelled = {
            let Self {
                world,
                sessions,
                mods,
                ..
            } = self;
            let actor = Some(sessions[s].id);
            let bus = mods.bus_mut();
            bus.player_damage_pre(world, sessions, actor, events, &mut pre) == Outcome::Cancel
        };
        if cancelled {
            return false;
        }
        if pre.amount <= 0 {
            return false;
        }
        let was_alive = self.sessions[s].player.health() > 0;
        let applied = self.sessions[s].player.apply_damage(pre.amount, immunity);
        if !applied {
            return false;
        }
        let new_health = self.sessions[s].player.health();
        events.player(s).player_damaged = true;
        self.interrupt_sleep(s, events);
        self.mods.emit(PostEvent::PlayerDamaged {
            player: self.sessions[s].id,
            amount: pre.amount,
            new_health,
        });
        if was_alive && new_health == 0 {
            events.player(s).player_died = true;
            if self.world.keep_inventory() {
                self.close_open_menu_for(s, events);
            } else {
                self.spill_inventory_on_death(s, events);
            }
            self.mods.emit(PostEvent::PlayerDied {
                player: self.sessions[s].id,
            });
        }
        true
    }

    fn spill_inventory_on_death(&mut self, s: usize, events: &mut TickEvents) {
        self.close_open_menu_for(s, events);
        let centre = self.sessions[s].player.body_center();
        let mut stacks: Vec<petramond_world::item::ItemStack> = Vec::new();
        for i in 0..petramond_world::inventory::TOTAL_SLOTS {
            if let Some(slot) = self.sessions[s].player.inventory.slot_mut(i) {
                if let Some(stack) = slot.take() {
                    stacks.push(stack);
                }
            }
        }
        if let Some(stack) = self.sessions[s].player.inventory.take_cursor() {
            stacks.push(stack);
        }
        if let Some(stack) = self.sessions[s].player.inventory.take_off_hand() {
            stacks.push(stack);
        }
        let cell = (
            centre.x.floor() as i32,
            centre.y.floor() as i32,
            centre.z.floor() as i32,
        );
        let (sky, blk) = self
            .world
            .data()
            .dynamic_light_at_world(cell.0, cell.1, cell.2);
        for stack in stacks {
            let mut drop = crate::entity::DroppedItem::new(centre, stack, self.seeds.draw());
            drop.skylight = sky;
            drop.blocklight = blk;
            self.world.spawn_item(drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn safe_fall_is_free_and_four_blocks_is_half_a_heart() {
        assert_eq!(fall_damage_health(0.0), 0);
        assert_eq!(fall_damage_health(3.0), 0, "3-block fall is safe");
        assert_eq!(fall_damage_health(3.9), 0, "under 4 blocks: no damage");
        assert_eq!(fall_damage_health(4.0), 1, "4 blocks = 0.5 hearts");
    }

    #[test]
    fn damage_scales_one_half_heart_per_block_past_the_safe_distance() {
        assert_eq!(fall_damage_health(5.0), 2);
        assert_eq!(fall_damage_health(12.0), 9);
        assert_eq!(fall_damage_health(103.0), 100);
    }

    #[test]
    fn a_clean_four_block_fall_still_hurts_despite_landing_rounding() {
        assert_eq!(fall_damage_health(4.0 - 8e-6), 1);
    }

    #[test]
    fn the_pre_damage_dispatch_names_its_victim_and_carries_their_intents() {
        use std::sync::{Arc, Mutex};

        let mut server = crate::server::session_build::build_server_inline("", 1, 2);
        let other = crate::server::session_build::spawn_player(server.world.data().seed);
        let victim_s = server.add_session_for_test(other);
        let victim_id = server.sessions[victim_s].id;
        assert_ne!(victim_s, 0, "the victim must not be the host session");

        server.sessions[victim_s].input.intent_use_held = true;
        server.sessions[victim_s].input.intent_gameplay = true;
        server.publish_player_inputs();

        type Seen = Vec<(Option<crate::player::PlayerId>, bool)>;
        let seen: Arc<Mutex<Seen>> = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::clone(&seen);
        server
            .mods
            .bus_mut()
            .on_player_damage_pre(0, move |ctx, _ev| {
                let id = ctx.actor;
                let use_held = id
                    .and_then(|id| {
                        ctx.world
                            .player_roster()
                            .iter()
                            .find(|r| r.id == id.0)
                            .map(|r| r.use_held)
                    })
                    .unwrap_or(false);
                sink.lock().unwrap().push((id, use_held));
                crate::events::Outcome::Continue
            });

        let mut events = crate::events::tick::TickEvents::default();
        server.damage_player(
            victim_s,
            3,
            crate::events::DamageSource::Fall,
            None,
            &mut events,
        );

        let seen = seen.lock().unwrap();
        assert_eq!(seen.len(), 1, "the handler ran once");
        assert_eq!(
            seen[0].0,
            Some(victim_id),
            "the ACTING player is the victim"
        );
        assert!(seen[0].1, "the victim's own session intents are readable");
    }
}
