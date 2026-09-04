//! Entity calls: mob spawn/query/damage/despawn, keyed particle emitters,
//! and deterministic dropped-item spawns.

use mod_api::{
    HostCall, HostRet, MobAnimStateData, MobRiderData, MobRidersData, MobSnapshot,
    MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE,
};

use crate::entity::DroppedItem;
use crate::events::{DamageSource, DeferredAction, PostEvent, SimCtx};
use petramond_math::math::{Tilt, Vec3};
use petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE;
use petramond_world::item::{ItemStack, ItemType};

use super::guards::{finite3, item_by_name, live_mob, sim_mutate, sim_mutating_query, sim_query};
use super::intern_mod_id;

/// Maximum horizontal speed accepted from `MobDrive`, derived from the
/// collision resolver's bounded external sweep and the fixed simulation tick.
const MAX_MOB_DRIVE_SPEED: f32 = MAX_SAFE_EXTERNAL_SWEEP_DISTANCE / crate::events::tick::TICK_DT;

fn anim_name_guard(call: &str, anim: &str) -> Result<(), HostRet> {
    if anim.len() <= MAX_MOB_ANIM_NAME_BYTES {
        Ok(())
    } else {
        Err(HostRet::Error(format!(
            "{call}: animation name is {} bytes; the limit is {MAX_MOB_ANIM_NAME_BYTES}",
            anim.len()
        )))
    }
}

fn magnitude_guard(call: &str, field: &str, value: f32, max: f32) -> Result<(), HostRet> {
    if value.is_finite() && value.abs() <= max {
        Ok(())
    } else {
        Err(HostRet::Error(format!(
            "{call}: {field} must be finite with magnitude <= {max}"
        )))
    }
}

/// The ABI snapshot of the live mob at list `index` — the one construction
/// shared by `MobsInRadius` and `MobsWithTag`.
pub(super) fn mob_snapshot(index: usize, m: &crate::mob::Instance) -> MobSnapshot {
    let size = crate::mob::def(m.kind).size;
    MobSnapshot {
        index: index as u32,
        kind: mod_api::MobId(m.kind.0),
        pos: m.pos.to_array(),
        health: m.health(),
        id: m.id(),
        yaw: m.yaw,
        pitch: m.tilt.pitch,
        roll: m.tilt.roll,
        vel: m.vel().to_array(),
        on_ground: m.on_ground(),
        moving: m.moving,
        half_width: size.half_width,
        height: size.height,
        half_length: size.half_length.unwrap_or(size.half_width),
    }
}

/// The damage source a call's named `attacker` resolves to — the ONE rule
/// `DamageMob` and `DamagePlayer` share. A player attacker must be a
/// connected session (a mod bug otherwise: the id came from the roster or
/// an event payload); a mob attacker no longer alive degrades to the mod's
/// own damage, since the strike still happened and there is simply nobody
/// left to remember.
pub(super) fn attack_source(
    ctx: &SimCtx<'_>,
    mod_id: &'static str,
    attacker: Option<mod_api::EntityRef>,
    call: &str,
) -> Result<DamageSource, HostRet> {
    Ok(match attacker {
        None => DamageSource::Mod(mod_id),
        Some(mod_api::EntityRef::Player(id)) => {
            if !ctx.world.player_roster().iter().any(|r| r.id == id.0) {
                return Err(HostRet::Error(format!(
                    "{call}: attacker player {} is not a connected session",
                    id.0
                )));
            }
            DamageSource::PlayerAttack(crate::player::PlayerId(id.0))
        }
        Some(mod_api::EntityRef::Mob(id)) => match live_mob(ctx, id) {
            Some(index) => DamageSource::MobAttack {
                kind: ctx.world.mobs().instances()[index].kind,
                id,
            },
            None => DamageSource::Mod(mod_id),
        },
    })
}

/// One item entity as the ABI snapshots it.
fn item_entity_data(it: &DroppedItem) -> mod_api::ItemEntityData {
    use crate::entity::Motion;
    let (owner, motion) = match it.motion {
        Motion::Loose => (None, mod_api::ItemMotion::Loose),
        Motion::Flight(f) => (f.owner, mod_api::ItemMotion::Flight),
        Motion::Stuck(s) => (
            None,
            mod_api::ItemMotion::Stuck {
                cell: s.anchor.to_array(),
            },
        ),
    };
    mod_api::ItemEntityData {
        id: it.id,
        stack: super::guards::item_stack_data(it.stack),
        owner: owner.map(crate::modding::convert::entity_ref),
        pos: it.pos.to_array(),
        vel: it.vel.to_array(),
        motion,
    }
}

/// Entity calls (mob spawn/query/hurt/despawn, item drops).
pub(super) fn handle_entity_call(mod_id: &str, call: HostCall) -> HostRet {
    match call {
        HostCall::SpawnMob {
            key,
            pos,
            yaw,
            checked,
        } => match finite3(pos, "SpawnMob.pos") {
            Err(e) => e,
            Ok(_) if !yaw.is_finite() => HostRet::Error("SpawnMob.yaw must be finite".into()),
            Ok(pos) => sim_mutating_query(|ctx| {
                let Some(kind) = crate::mob::by_key(&key) else {
                    log::warn!("[mod {mod_id}] SpawnMob: unknown species '{key}'");
                    return HostRet::SpawnedMob(None);
                };
                let spawned = if checked {
                    ctx.world.spawn_mob_checked(kind, pos, yaw)
                } else {
                    ctx.world.spawn_mob(kind, pos, yaw)
                };
                if let Some(id) = spawned {
                    ctx.queue.emit(PostEvent::MobSpawned { id, kind, pos });
                }
                HostRet::SpawnedMob(spawned)
            }),
        },
        HostCall::MobInfo { mob_id } => sim_query(|ctx| {
            HostRet::Mob(
                live_mob(ctx, mob_id).map(|i| mob_snapshot(i, &ctx.world.mobs().instances()[i])),
            )
        }),
        HostCall::MobCanReach { mob_id, cell } => sim_query(|ctx| {
            HostRet::Bool(live_mob(ctx, mob_id).is_some_and(|i| {
                crate::mob::mob_can_reach(
                    ctx.world,
                    &ctx.world.mobs().instances()[i],
                    petramond_math::math::IVec3::new(cell[0], cell[1], cell[2]),
                )
            }))
        }),
        HostCall::SiteOpen { key, cell } => sim_query(|ctx| {
            let Some(kind) = crate::mob::by_key(&key) else {
                log::warn!("[mod {mod_id}] SiteOpen: unknown species '{key}'");
                return HostRet::Bool(false);
            };
            HostRet::Bool(crate::mob::site_open(
                ctx.world,
                kind,
                petramond_math::math::IVec3::new(cell[0], cell[1], cell[2]),
            ))
        }),
        HostCall::MobsInRadius { pos, radius } => match finite3(pos, "MobsInRadius.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                if !radius.is_finite() {
                    return HostRet::Error("MobsInRadius: non-finite radius".into());
                }
                let r2 = radius * radius;
                let out = ctx
                    .world
                    .mobs()
                    .instances()
                    .iter()
                    .enumerate()
                    .filter(|(_, m)| !m.is_dead())
                    .filter(|(_, m)| (m.pos - pos).length_squared() <= r2)
                    .map(|(i, m)| mob_snapshot(i, m))
                    .collect();
                HostRet::Mobs(out)
            }),
        },
        HostCall::DamageMob {
            mob_id,
            amount,
            origin,
            feedback,
            attacker,
        } => match origin.map(|p| finite3(p, "DamageMob.origin")).transpose() {
            Err(e) => e,
            Ok(origin) => {
                let mod_id = intern_mod_id(mod_id);
                let feedback = feedback.map(crate::modding::mob_damage_feedback);
                sim_mutate(|ctx| {
                    let source = attack_source(ctx, mod_id, attacker, "DamageMob")?;
                    ctx.queue.push_action(DeferredAction::DamageMob {
                        mob_id,
                        amount,
                        source,
                        origin,
                        feedback,
                    });
                    Ok(())
                })
            }
        },
        HostCall::DespawnMob { mob_id } => sim_mutating_query(|ctx| {
            let Some(index) = live_mob(ctx, mob_id) else {
                return HostRet::Bool(false);
            };
            HostRet::Bool(ctx.world.mobs_mut().remove(index))
        }),
        // Presentation-only mob state (no bus funnel), so unlike DamageMob it
        // applies immediately instead of queueing a DeferredAction.
        HostCall::MobEmitterSet {
            mob_id,
            key,
            active,
        } => sim_mutating_query(|ctx| {
            let Some(index) = live_mob(ctx, mob_id) else {
                return HostRet::Bool(false);
            };
            HostRet::Bool(ctx.world.mobs_mut().set_mob_emitter(index, &key, active))
        }),
        // The animation sibling of MobEmitterSet.
        HostCall::MobAnimSet {
            mob_id,
            anim,
            active,
        } => match anim_name_guard("MobAnimSet", &anim) {
            Err(e) => e,
            Ok(()) => sim_mutating_query(|ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                HostRet::Bool(ctx.world.mobs_mut().set_mob_anim(index, &anim, active))
            }),
        },
        HostCall::MobAnimRate { mob_id, anim, rate } => {
            if let Err(e) = anim_name_guard("MobAnimRate", &anim) {
                return e;
            }
            if let Err(e) =
                magnitude_guard("MobAnimRate", "rate", rate, MAX_MOB_ANIM_RATE_MAGNITUDE)
            {
                return e;
            }
            sim_mutating_query(move |ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                HostRet::Bool(ctx.world.mobs_mut().set_mob_anim_rate(index, &anim, rate))
            })
        }
        HostCall::MobAnimSeek {
            mob_id,
            anim,
            phase,
            rate,
        } => {
            if let Err(e) = anim_name_guard("MobAnimSeek", &anim) {
                return e;
            }
            if let Err(e) =
                magnitude_guard("MobAnimSeek", "phase", phase, MAX_MOB_ANIM_PHASE_MAGNITUDE)
            {
                return e;
            }
            if let Err(e) =
                magnitude_guard("MobAnimSeek", "rate", rate, MAX_MOB_ANIM_RATE_MAGNITUDE)
            {
                return e;
            }
            sim_mutating_query(move |ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                HostRet::Bool(
                    ctx.world
                        .mobs_mut()
                        .set_mob_anim_seek(index, &anim, phase, rate),
                )
            })
        }
        // Kinematic drive intent for this tick (see `Instance::set_drive`);
        // immediate like every presentation/locomotion primitive.
        HostCall::MobDrive {
            mob_id,
            horizontal,
            vertical,
            yaw,
            while_walking,
        } => {
            if horizontal.is_some_and(|v| !v.iter().all(|c| c.is_finite()))
                || vertical.is_some_and(|v| !v.is_finite())
                || yaw.is_some_and(|y| !y.is_finite())
            {
                return HostRet::Error("MobDrive: non-finite velocity/yaw".into());
            }
            if while_walking && horizontal.is_some() {
                return HostRet::Error(
                    "MobDrive: a walking-gated intent cannot carry horizontal velocity — \
                     walking IS the horizontal locomotion"
                        .into(),
                );
            }
            if horizontal.is_some_and(|v| v[0].hypot(v[1]) > MAX_MOB_DRIVE_SPEED) {
                return HostRet::Error(format!(
                    "MobDrive: horizontal speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
                ));
            }
            if vertical.is_some_and(|v| v.abs() > MAX_MOB_DRIVE_SPEED) {
                return HostRet::Error(format!(
                    "MobDrive: vertical speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
                ));
            }
            sim_mutating_query(move |ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                HostRet::Bool(ctx.world.mobs_mut().set_mob_drive(
                    index,
                    horizontal,
                    vertical,
                    yaw,
                    while_walking,
                ))
            })
        }
        // Kinematic placement for this tick (see `Instance::set_kinematic`):
        // the mod authored the pose, the engine presents it.
        HostCall::MobKinematic {
            mob_id,
            pos,
            yaw,
            pitch,
            roll,
        } => {
            let tilt = Tilt::new(pitch, roll);
            if !pos.iter().all(|c| c.is_finite()) || !yaw.is_finite() || !tilt.is_finite() {
                return HostRet::Error("MobKinematic: non-finite pose".into());
            }
            if pitch.abs() > std::f32::consts::FRAC_PI_2 {
                return HostRet::Error("MobKinematic: pitch outside ±π/2".into());
            }
            if roll.abs() > std::f32::consts::PI {
                return HostRet::Error("MobKinematic: roll outside ±π".into());
            }
            sim_mutating_query(move |ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::Bool(false);
                };
                let pos = Vec3::from(pos);
                match ctx
                    .world
                    .mobs_mut()
                    .set_mob_kinematic(index, pos, yaw, tilt)
                {
                    Ok(placed) => HostRet::Bool(placed),
                    Err(distance) => HostRet::Error(format!(
                        "MobKinematic: placement {distance} blocks away exceeds the \
                         {MAX_SAFE_EXTERNAL_SWEEP_DISTANCE}-block sweep bound"
                    )),
                }
            })
        }
        HostCall::MobMount {
            mob_id,
            player_id,
            seat,
        } => sim_mutating_query(|ctx| {
            HostRet::Bool(ctx.world.try_mount_player(player_id.0, mob_id, seat))
        }),
        HostCall::PlayerPoseSet {
            player_id,
            anchor,
            yaw,
            pose,
        } => {
            let anchor = match finite3(anchor, "PlayerPoseSet.anchor") {
                Ok(a) => a,
                Err(e) => return e,
            };
            if !yaw.is_finite() {
                return HostRet::Error("PlayerPoseSet: non-finite yaw".into());
            }
            if pose == 0 {
                return HostRet::Bool(false); // reserved "no pose" value
            }
            sim_mutating_query(move |ctx| {
                HostRet::Bool(ctx.world.try_mount_anchor(
                    player_id.0,
                    crate::mob::riding::PoseAnchor {
                        pos: anchor,
                        yaw,
                        pose,
                    },
                ))
            })
        }
        HostCall::MobDismount { player_id } => sim_mutating_query(|ctx| {
            HostRet::Bool(ctx.world.riding_mut().dismount(player_id.0).is_some())
        }),
        HostCall::MobRiders { mob_id } => sim_query(|ctx| {
            let Some(index) = live_mob(ctx, mob_id) else {
                return HostRet::Riders(None);
            };
            let mob = &ctx.world.mobs().instances()[index];
            let capacity = crate::mob::def(mob.kind).seats.len() as u8;
            let riders = ctx
                .world
                .riding()
                .riders_of(crate::mob::riding::MountTarget::Mob(mob_id))
                .into_iter()
                .map(|(seat, player_id)| MobRiderData {
                    seat,
                    player_id: mod_api::PlayerId(player_id),
                })
                .collect();
            HostRet::Riders(Some(MobRidersData { capacity, riders }))
        }),
        HostCall::BlockModelGroup { pos } => sim_query(|ctx| {
            let p = petramond_math::math::IVec3::new(pos[0], pos[1], pos[2]);
            HostRet::ModelGroup(ctx.world.model_group(p).map(|(_, base, _)| {
                mod_api::ModelGroupData {
                    base: [base.x, base.y, base.z],
                    facing: match ctx.world.model_facing_at(base.x, base.y, base.z) {
                        petramond_math::facing::Facing::North => mod_api::Facing::North,
                        petramond_math::facing::Facing::South => mod_api::Facing::South,
                        petramond_math::facing::Facing::West => mod_api::Facing::West,
                        petramond_math::facing::Facing::East => mod_api::Facing::East,
                    },
                }
            }))
        }),
        HostCall::MobAnimState { mob_id, anim } => {
            if let Err(e) = anim_name_guard("MobAnimState", &anim) {
                return e;
            }
            sim_query(move |ctx| {
                let Some(index) = live_mob(ctx, mob_id) else {
                    return HostRet::MobAnimState(None);
                };
                HostRet::MobAnimState(ctx.world.mobs().mob_anim_state(index, &anim).map(|state| {
                    MobAnimStateData {
                        phase: state.phase,
                        rate: state.rate,
                        seek: state.seek,
                    }
                }))
            })
        }
        HostCall::SpawnItem {
            item,
            count,
            pos,
            data,
        } => match finite3(pos, "SpawnItem.pos") {
            Err(e) => e,
            Ok(pos) => {
                let variant = match super::guards::intern_abi_data("SpawnItem", &data) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                sim_mutating_query(|ctx| {
                    let Some(item) = item_by_name(&item) else {
                        log::warn!("[mod {mod_id}] SpawnItem: unknown item '{item}'");
                        return HostRet::Bool(false);
                    };
                    if count == 0 {
                        return HostRet::Bool(false);
                    }
                    spawn_item_stacks(ctx, item, count, pos, variant);
                    HostRet::Bool(true)
                })
            }
        },
        HostCall::LaunchItem {
            item,
            pos,
            vel,
            owner,
            data,
        } => match (
            finite3(pos, "LaunchItem.pos"),
            finite3(vel, "LaunchItem.vel"),
        ) {
            (Err(e), _) | (_, Err(e)) => e,
            (Ok(pos), Ok(vel)) => {
                let variant = match super::guards::intern_abi_data("LaunchItem", &data) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                sim_mutating_query(|ctx| {
                    let Some(item) = item_by_name(&item) else {
                        log::warn!("[mod {mod_id}] LaunchItem: unknown item '{item}'");
                        return HostRet::U64(0);
                    };
                    let owner = owner.map(crate::modding::convert::entity_ref_in);
                    let stack = petramond_world::item::ItemStack::with_variant(item, 1, variant);
                    let mut entity = crate::entity::DroppedItem::launched(pos, stack, vel, owner);
                    let (sky, block) = crate::server::entities::light_at_pos(ctx.world, pos);
                    entity.skylight = sky;
                    entity.blocklight = block;
                    HostRet::U64(ctx.world.spawn_item(entity))
                })
            }
        },
        HostCall::ItemEntity { entity } => sim_query(|ctx| {
            HostRet::ItemEntity(ctx.world.dropped_items().get(entity).map(item_entity_data))
        }),
        other => HostRet::Error(format!(
            "non-entity call {other:?} mis-routed to handle_entity_call (host bug)"
        )),
    }
}

/// Spawn `count` of `item` as dropped entities at `pos`, splitting oversized
/// counts into max-stack-size drops. Pop seeds derive from (tick, pos, i) so
/// the spawn is deterministic without any Game-side counter.
fn spawn_item_stacks(
    ctx: &mut SimCtx<'_>,
    item: ItemType,
    count: u8,
    pos: Vec3,
    variant: petramond_world::item::VariantId,
) {
    let cell = petramond_math::math::voxel_at(pos);
    let sky = ctx.world.skylight6_at_world(cell.x, cell.y, cell.z);
    let block = petramond_world::light::BlockLight6::from_x2(
        ctx.world.blocklight_rgb_at_world(cell.x, cell.y, cell.z),
    );
    let mut remaining = count;
    let mut i = 0u32;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        let seed = drop_seed(ctx.world.current_tick(), pos, i);
        let mut drop = DroppedItem::new(pos, ItemStack::with_variant(item, put, variant), seed);
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
pub(super) fn give_item(
    ctx: &mut SimCtx<'_>,
    item: ItemType,
    count: u8,
    variant: petramond_world::item::VariantId,
) {
    let (leftovers, at) = fill_inventory(ctx.player, item, count, variant);
    drop_leftovers(ctx.world, at, leftovers);
}

/// The inventory half of a give: fill `player`'s inventory, returning what
/// did not fit plus where its overflow should drop (the player's body). Split
/// from the drop half so an EXPLICIT-player give (`GiveItemTo`) can run it
/// inside the sessions-view borrow and spawn the drops outside it — one
/// arithmetic for both addressings.
fn fill_inventory(
    player: &mut crate::player::Player,
    item: ItemType,
    count: u8,
    variant: petramond_world::item::VariantId,
) -> (Vec<ItemStack>, petramond_math::math::Vec3) {
    let mut leftovers = Vec::new();
    let mut remaining = count;
    while remaining > 0 {
        let put = remaining.min(item.max_stack_size());
        remaining -= put;
        if let Some(leftover) = player
            .inventory
            .add(ItemStack::with_variant(item, put, variant))
        {
            leftovers.push(leftover);
        }
    }
    (leftovers, player.body_center())
}

/// The world half of a give: drop what the inventory refused, at `at`.
fn drop_leftovers(
    world: &mut crate::world::World,
    at: petramond_math::math::Vec3,
    leftovers: Vec<ItemStack>,
) {
    for (i, leftover) in leftovers.into_iter().enumerate() {
        let seed = drop_seed(world.current_tick(), at, i as u32);
        let cell = petramond_math::math::voxel_at(at);
        let mut drop = DroppedItem::new(at, leftover, seed);
        drop.skylight = world.skylight6_at_world(cell.x, cell.y, cell.z);
        drop.blocklight = petramond_world::light::BlockLight6::from_x2(
            world.blocklight_rgb_at_world(cell.x, cell.y, cell.z),
        );
        world.spawn_item(drop);
    }
}

/// [`give_item`] addressed to a NAMED session through the sessions view:
/// `false` when the id resolves to no connected player.
pub(super) fn give_item_to(
    ctx: &mut SimCtx<'_>,
    player: crate::player::PlayerId,
    item: ItemType,
    count: u8,
    variant: petramond_world::item::VariantId,
) -> bool {
    let Some((leftovers, at)) =
        ctx.with_player(player, |p| fill_inventory(p, item, count, variant))
    else {
        return false;
    };
    drop_leftovers(ctx.world, at, leftovers);
    true
}

#[cfg(test)]
mod tests {
    use mod_api::{
        HostCall, HostRet, MobAnimStateData, MobRidersData, MAX_MOB_ANIM_NAME_BYTES,
        MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE,
    };

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::player::Player;
    use crate::world::World;
    use petramond_math::math::Vec3;
    use petramond_world::chunk::{ChunkPos, SECTION_VOLUME};

    #[test]
    fn spawn_mob_initializes_cached_light_before_first_render_snapshot() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        let section = world
            .section_at_world_mut_for_test(8, 64, 8)
            .expect("fixture loads the spawn section");
        section.set_skylight(vec![0; SECTION_VOLUME].into());
        section.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());

        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert!(matches!(
                handle_host_call(
                    &mut data,
                    HostCall::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                        checked: false,
                    },
                ),
                HostRet::SpawnedMob(Some(_))
            ));
        });

        let mob = &world.mobs().instances()[0];
        assert_eq!(mob.skylight, 0);
        assert_eq!(mob.blocklight, petramond_world::light::BlockLight6::DARK);
    }

    #[test]
    fn checked_spawn_requires_a_loaded_clear_body_pose() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.set_block_world(8, 64, 8, petramond_world::block::Block::Stone);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let checked = |pos| HostCall::SpawnMob {
                key: "petramond:owl".into(),
                pos,
                yaw: 0.0,
                checked: true,
            };
            assert_eq!(
                handle_host_call(&mut data, checked([8.5, 64.0, 8.5])),
                HostRet::SpawnedMob(None),
                "terrain overlap rejects without spawning"
            );
            assert_eq!(
                handle_host_call(&mut data, checked([32.5, 64.0, 8.5])),
                HostRet::SpawnedMob(None),
                "unknown unloaded space is never treated as clear"
            );
        });
        assert!(world.mobs().instances().is_empty());

        world.set_block_world(8, 64, 8, petramond_world::block::Block::Air);
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert!(matches!(
                handle_host_call(
                    &mut data,
                    HostCall::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                        checked: true,
                    },
                ),
                HostRet::SpawnedMob(Some(_))
            ));
        });
        assert_eq!(world.mobs().instances().len(), 1);
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
        let mut gui = petramond_world::gui_state::empty_gui_state();
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
                        mob_id: before[0].id
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

    #[test]
    fn mob_animation_and_drive_calls_reject_unbounded_guest_control_state() {
        let rejected = |call| {
            assert!(
                matches!(super::handle_entity_call("alpha", call), HostRet::Error(_)),
                "out-of-envelope call must be a protocol error"
            );
        };

        rejected(HostCall::MobAnimSet {
            mob_id: 1,
            anim: "a".repeat(MAX_MOB_ANIM_NAME_BYTES + 1),
            active: true,
        });
        rejected(HostCall::MobAnimRate {
            mob_id: 1,
            anim: "row".into(),
            rate: MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0,
        });
        rejected(HostCall::MobAnimSeek {
            mob_id: 1,
            anim: "row".into(),
            phase: MAX_MOB_ANIM_PHASE_MAGNITUDE * 2.0,
            rate: 1.0,
        });
        rejected(HostCall::MobAnimSeek {
            mob_id: 1,
            anim: "row".into(),
            phase: 0.0,
            rate: MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0,
        });
        rejected(HostCall::MobDrive {
            mob_id: 1,
            horizontal: Some([super::MAX_MOB_DRIVE_SPEED * 2.0, 0.0]),
            vertical: None,
            yaw: None,
            while_walking: false,
        });
        rejected(HostCall::MobDrive {
            mob_id: 1,
            horizontal: None,
            vertical: Some(super::MAX_MOB_DRIVE_SPEED * 2.0),
            yaw: None,
            while_walking: false,
        });
        rejected(HostCall::MobDrive {
            mob_id: 1,
            horizontal: Some([1.0, 0.0]),
            vertical: Some(1.0),
            yaw: None,
            while_walking: true,
        });
    }

    #[test]
    fn mob_queries_distinguish_missing_mobs_and_expose_authoritative_anim_state() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(1.0, 80.0, 1.0), 0.0));
        let mob_id = world.mobs().instances()[0].id();
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(&mut data, HostCall::MobRiders { mob_id: u64::MAX }),
                HostRet::Riders(None)
            );
            assert_eq!(
                handle_host_call(&mut data, HostCall::MobRiders { mob_id }),
                HostRet::Riders(Some(MobRidersData {
                    capacity: crate::mob::def(crate::mob::Mob::Owl).seats.len() as u8,
                    riders: Vec::new(),
                }))
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::MobAnimState {
                        mob_id,
                        anim: "row".into(),
                    }
                ),
                HostRet::MobAnimState(None)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::MobAnimSet {
                        mob_id,
                        anim: "row".into(),
                        active: true,
                    }
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::MobAnimSeek {
                        mob_id,
                        anim: "row".into(),
                        phase: 1.5,
                        rate: -0.75,
                    }
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::MobAnimState {
                        mob_id,
                        anim: "row".into(),
                    }
                ),
                HostRet::MobAnimState(Some(MobAnimStateData {
                    phase: 0.0,
                    rate: 0.75,
                    seek: Some(1.5),
                }))
            );
        });
    }

    /// The one dead-mob policy (`live_mob`): a ragdolling corpse is GONE to
    /// every id-addressed call — reads answer `None`/`Bytes(None)` and writes
    /// answer `false`, uniformly, so state can never be written to a mob no
    /// read can see.
    #[test]
    fn a_dead_mob_is_gone_to_every_id_addressed_call() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = World::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, Vec3::new(1.0, 80.0, 1.0), 0.0));
        let mob_id = world.mobs().instances()[0].id();
        assert!(world
            .mobs_mut()
            .damage_mob(
                0,
                1000.0,
                None,
                true,
                None,
                &crate::mob::MobDamageFeedback::default(),
            )
            .is_some());
        assert!(world.mobs().instances()[0].is_dead());

        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut gui = petramond_world::gui_state::empty_gui_state();
        let mut ctx = SimCtx {
            world: &mut world,
            player: &mut player,
            gui_state: &mut gui,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let refused = |data: &mut ModStoreData, call| {
                assert_eq!(
                    handle_host_call(data, call),
                    HostRet::Bool(false),
                    "an id-addressed write must refuse a corpse"
                );
            };
            refused(
                &mut data,
                HostCall::MobAnimSet {
                    mob_id,
                    anim: "row".into(),
                    active: true,
                },
            );
            refused(
                &mut data,
                HostCall::MobDrive {
                    mob_id,
                    horizontal: Some([1.0, 0.0]),
                    vertical: None,
                    yaw: None,
                    while_walking: false,
                },
            );
            refused(
                &mut data,
                HostCall::MobEmitterSet {
                    mob_id,
                    key: "petramond:burn_light".into(),
                    active: true,
                },
            );
            refused(
                &mut data,
                HostCall::MobTagSet {
                    mob_id,
                    key: "alpha:x".into(),
                    value: mod_api::MobTagValue::I64(1),
                },
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::MobTagGet {
                        mob_id,
                        key: "alpha:x".into(),
                    }
                ),
                HostRet::MobTag(mod_api::MobTagLookup::MissingMob)
            );
            assert_eq!(
                handle_host_call(&mut data, HostCall::MobRiders { mob_id }),
                HostRet::Riders(None)
            );
        });
    }
}
