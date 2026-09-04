//! vehicles — rideable vehicles over the generic riding, drive and
//! kinematic-placement API: a rowable boat ([`boat`]) and minecarts on
//! rails ([`minecart`], with the rail connection rule in [`rail`], the rail
//! geometry in [`track`] and the cart's motion in [`cart`]).
//!
//! Everything in this pack is POLICY over engine mechanisms; it ships no
//! vehicle-specific engine code. Each vehicle module documents its own
//! seams; this file only routes the shared registrations to them.

use mod_sdk::*;

mod boat;
mod cart;
mod minecart;
mod rail;
mod track;

const TICK_BOATS: u32 = 1;
const TICK_CARTS: u32 = 2;
const ON_ITEM_USE: u32 = 1;
const ON_INTERACT_ATTEMPT: u32 = 2;
const ON_DISMOUNTED: u32 = 3;
const ON_BLOCK_PLACED: u32 = 4;
const ON_MOB_DAMAGE: u32 = 5;

#[derive(Default)]
struct Vehicles {
    boats: boat::Boats,
    carts: minecart::Minecarts,
}

impl Mod for Vehicles {
    fn init(&mut self) {
        self.boats.init();
        self.carts.init();
        register_event_handler(EventKind::ItemUsePre, 0, ON_ITEM_USE);
        register_event_handler(EventKind::InteractAttempt, 0, ON_INTERACT_ATTEMPT);
        register_event_handler(EventKind::PlayerDismounted, 0, ON_DISMOUNTED);
        register_event_handler(EventKind::BlockPlaced, 0, ON_BLOCK_PLACED);
        register_event_handler(EventKind::MobDamagePre, 0, ON_MOB_DAMAGE);
        // Both drives run right before the mob stage, so the intents they
        // issue apply in the same tick.
        register_tick_system(Stage::Mobs, AttachSide::Before, 0, TICK_BOATS);
        register_tick_system(Stage::Mobs, AttachSide::Before, 1, TICK_CARTS);
        log("initialized: boat + minecarts and rails");
    }

    fn handle_event(&mut self, handler_id: u32, payload: &mut EventPayload) -> Outcome {
        match (handler_id, &*payload) {
            (ON_ITEM_USE, EventPayload::ItemUsePre { item, target, .. }) => {
                self.boats.on_item_use(*item, *target)
            }
            (
                ON_INTERACT_ATTEMPT,
                EventPayload::InteractAttempt {
                    block, mob, player, ..
                },
            ) => {
                // Each vehicle self-gates on what was clicked (one species
                // lookup shared by all); a claim ends the walk.
                match mob {
                    Some(id) => {
                        let Some(kind) = mob_info(*id).map(|m| m.kind) else {
                            return Outcome::Continue;
                        };
                        if self.boats.on_interact_mob(*id, kind, *player) == Outcome::Cancel {
                            return Outcome::Cancel;
                        }
                        self.carts.on_interact_mob(*id, kind, *player)
                    }
                    None => self.carts.on_interact_block(*block),
                }
            }
            (ON_DISMOUNTED, EventPayload::PlayerDismounted { player_id, mount }) => {
                self.boats.on_dismounted(*player_id, mount);
                Outcome::Continue
            }
            (ON_BLOCK_PLACED, EventPayload::BlockPlaced { pos, block }) => {
                self.carts.on_block_placed(*pos, *block);
                Outcome::Continue
            }
            (
                ON_MOB_DAMAGE,
                EventPayload::MobDamagePre {
                    mob_id,
                    kind,
                    origin,
                    ..
                },
            ) => {
                self.carts.on_mob_damage(*mob_id, *kind, *origin);
                Outcome::Continue
            }
            _ => Outcome::Continue,
        }
    }

    fn tick_system(&mut self, system_id: u32) {
        match system_id {
            TICK_BOATS => self.boats.tick(),
            TICK_CARTS => self.carts.tick(),
            _ => {}
        }
    }
}

register_mod!(Vehicles);
