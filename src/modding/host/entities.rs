//! Entity calls: mob spawn/query/damage/despawn, keyed particle emitters,
//! and deterministic dropped-item spawns.

use mod_api::{HostCall, HostRet, MobSnapshot};

use crate::entity::DroppedItem;
use crate::events::{ModAction, PostEvent, SimCtx};
use crate::item::{ItemStack, ItemType};
use crate::mathh::Vec3;

use super::guards::{finite3, item_by_key, sim_call, sim_query};
use super::intern_mod_id;

/// Phase 3b: entities (mob spawn/query/hurt/despawn, item drops).
pub(super) fn handle_entity_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SpawnMob { key, pos, yaw } => match finite3(pos, "SpawnMob.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                let Some(kind) = crate::mob::defs()
                    .iter()
                    .position(|d| d.key == key)
                    .map(|i| crate::mob::Mob(i as u8))
                else {
                    log::warn!("[mod {mod_id}] SpawnMob: unknown species '{key}'");
                    return HostRet::Bool(false);
                };
                let spawned = ctx.world.spawn_mob(kind, pos, yaw);
                if spawned {
                    ctx.queue.emit(PostEvent::MobSpawned { kind, pos });
                }
                HostRet::Bool(spawned)
            }),
        },
        HostCall::MobsInRadius { pos, radius } => match finite3(pos, "MobsInRadius.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                if !radius.is_finite() {
                    return HostRet::Error("MobsInRadius: non-finite radius".into());
                }
                let r2 = radius * radius;
                HostRet::Mobs(
                    ctx.world
                        .mobs()
                        .instances()
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| !m.is_dead())
                        .filter(|(_, m)| (m.pos - pos).length_squared() <= r2)
                        .map(|(i, m)| MobSnapshot {
                            index: i as u32,
                            key: crate::mob::def(m.kind).key.to_owned(),
                            pos: [m.pos.x, m.pos.y, m.pos.z],
                            health: m.health(),
                            id: m.id(),
                        })
                        .collect(),
                )
            }),
        },
        HostCall::DamageMob {
            index,
            amount,
            origin,
        } => match origin.map(|p| finite3(p, "DamageMob.origin")).transpose() {
            Err(e) => e,
            Ok(origin) => {
                let mod_id = intern_mod_id(mod_id);
                sim_call(|ctx| {
                    ctx.queue.push_action(ModAction::DamageMob {
                        index: index as usize,
                        amount,
                        mod_id,
                        origin,
                    })
                })
            }
        },
        HostCall::DespawnMob { index } => {
            sim_query(|ctx| HostRet::Bool(ctx.world.mobs_mut().remove(index as usize)))
        }
        // Presentation-only mob state (no bus funnel), so unlike DamageMob it
        // applies immediately instead of queueing a ModAction.
        HostCall::MobEmitterSet { index, key, active } => sim_query(|ctx| {
            HostRet::Bool(
                ctx.world
                    .mobs_mut()
                    .set_mob_emitter(index as usize, &key, active),
            )
        }),
        HostCall::SpawnItem {
            item_key,
            count,
            pos,
        } => match finite3(pos, "SpawnItem.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                let Some(item) = item_by_key(&item_key) else {
                    log::warn!("[mod {mod_id}] SpawnItem: unknown item '{item_key}'");
                    return HostRet::Bool(false);
                };
                if count == 0 {
                    return HostRet::Bool(false);
                }
                spawn_item_stacks(ctx, item, count, pos);
                HostRet::Bool(true)
            }),
        },
        other => HostRet::Error(format!(
            "non-entity call {other:?} mis-routed to handle_entity_call (host bug)"
        )),
    }
}

/// Spawn `count` of `item` as dropped entities at `pos`, splitting oversized
/// counts into max-stack-size drops. Pop seeds derive from (tick, pos, i) so
/// the spawn is deterministic without any Game-side counter.
fn spawn_item_stacks(ctx: &mut SimCtx<'_>, item: ItemType, count: u8, pos: Vec3) {
    let cell = crate::mathh::voxel_at(pos);
    let sky = ctx.world.skylight6_at_world(cell.x, cell.y, cell.z);
    let block = ctx.world.blocklight6_at_world(cell.x, cell.y, cell.z);
    let mut remaining = count;
    let mut i = 0u32;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        let seed = drop_seed(ctx.world.current_tick(), pos, i);
        let mut drop = DroppedItem::new(pos, ItemStack::new(item, put), seed);
        drop.skylight = sky;
        drop.blocklight = block;
        ctx.world.spawn_item(drop);
        i += 1;
    }
}

/// Deterministic per-drop pop seed: a SplitMix64 finalizer over the tick, the
/// spawn position bits, and the in-call index.
fn drop_seed(tick: u64, pos: Vec3, i: u32) -> u32 {
    let mut z = tick
        ^ ((pos.x.to_bits() as u64) << 32 | pos.z.to_bits() as u64)
        ^ ((pos.y.to_bits() as u64) << 16)
        ^ ((i as u64) << 1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

/// Give the player `count` of `item` through the normal inventory fill;
/// whatever does not fit drops at the player's feet like any other overflow.
pub(super) fn give_item(ctx: &mut SimCtx<'_>, item: ItemType, count: u8) {
    let mut remaining = count;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        if let Some(leftover) = ctx.player.inventory.add(ItemStack::new(item, put)) {
            let at = ctx.player.body_center();
            let seed = drop_seed(ctx.world.current_tick(), at, remaining as u32);
            let cell = crate::mathh::voxel_at(at);
            let mut drop = DroppedItem::new(at, leftover, seed);
            drop.skylight = ctx.world.skylight6_at_world(cell.x, cell.y, cell.z);
            drop.blocklight = ctx.world.blocklight6_at_world(cell.x, cell.y, cell.z);
            ctx.world.spawn_item(drop);
        }
    }
}

#[cfg(test)]
mod tests {
    use mod_api::{HostCall, HostRet};

    use crate::chunk::{ChunkPos, SECTION_VOLUME};
    use crate::events::{PostQueue, SimCtx};
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;

    #[test]
    fn spawn_mob_initializes_cached_light_before_first_render_snapshot() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        let section = world
            .section_at_world_mut_for_test(8, 64, 8)
            .expect("fixture loads the spawn section");
        section.set_skylight(vec![0; SECTION_VOLUME].into());
        section.set_blocklight(vec![0; SECTION_VOLUME].into());

        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                    },
                ),
                HostRet::Bool(true)
            );
        });

        let mob = &world.mobs().instances()[0];
        assert_eq!(mob.skylight, 0);
        assert_eq!(mob.blocklight, 0);
    }

    #[test]
    fn mob_snapshot_id_survives_unrelated_despawn_index_shift() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(1.0, 80.0, 1.0), 0.0));
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(2.0, 80.0, 2.0), 0.0));
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = crate::gui::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            let before = match handle_host_call(
                &mut data,
                HostCall::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                },
            ) {
                HostRet::Mobs(mobs) => mobs,
                other => panic!("MobsInRadius returned {other:?}"),
            };
            assert_eq!(before.len(), 2);
            let shifted_id = before[1].id;
            assert_ne!(before[0].id, shifted_id);

            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::DespawnMob {
                        index: before[0].index
                    }
                ),
                HostRet::Bool(true)
            );

            let after = match handle_host_call(
                &mut data,
                HostCall::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                },
            ) {
                HostRet::Mobs(mobs) => mobs,
                other => panic!("MobsInRadius returned {other:?}"),
            };
            assert_eq!(after.len(), 1);
            assert_eq!(after[0].index, 0, "swap_remove shifted the remaining mob");
            assert_eq!(after[0].id, shifted_id, "stable id survived the shift");
            assert_eq!(after[0].pos, [2.0, 80.0, 2.0]);
        });
    }
}
