mod intents;

use mod_api::{
    EntityCall, HostRet, MobAnimStateData, MobRiderData, MobRidersData, MobSnapshot,
    MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE,
};

use crate::entity::DroppedItem;
use crate::events::{DamageSource, DeferredAction, PostEvent, SimCtx};
use petramond_math::math::{IVec3, Tilt};
use petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE;
use petramond_world::item::{ItemStack, ItemType};

use super::guards::{
    batch_guard, finite3, finite_pos, item_by_name, live_mob, sim_mutate, sim_query,
};
use super::intern_mod_id;

const MAX_MOB_DRIVE_SPEED: f32 = MAX_SAFE_EXTERNAL_SWEEP_DISTANCE / crate::events::tick::TICK_DT;

fn anim_name_guard(call: &str, anim: &str) -> Result<(), HostRet> {
    if anim.len() <= MAX_MOB_ANIM_NAME_BYTES {
        Ok(())
    } else {
        Err(HostRet::invalid(format!(
            "{call}: animation name is {} bytes; the limit is {MAX_MOB_ANIM_NAME_BYTES}",
            anim.len()
        )))
    }
}

fn magnitude_guard(call: &str, field: &str, value: f32, max: f32) -> Result<(), HostRet> {
    if value.is_finite() && value.abs() <= max {
        Ok(())
    } else {
        Err(HostRet::invalid(format!(
            "{call}: {field} must be finite with magnitude <= {max}"
        )))
    }
}

pub(super) fn mob_snapshot(position: usize, m: &crate::mob::Instance) -> MobSnapshot {
    let size = crate::mob::def(m.kind).size;
    MobSnapshot {
        index: position as u32,
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
        entombed: m.entombed(),
        conditions: crate::exposure::condition_data(m.exposure().conditions()),
        target: m.target().map(crate::modding::convert::entity_ref),
    }
}

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
                return Err(HostRet::invalid(format!(
                    "{call}: attacker player {} is not a connected session",
                    id.0
                )));
            }
            DamageSource::PlayerAttack(crate::player::PlayerId(id.0))
        }
        Some(mod_api::EntityRef::Mob(id)) => match live_mob(ctx, id) {
            Some(mob) => DamageSource::MobAttack { kind: mob.kind, id },
            None => DamageSource::Mod(mod_id),
        },
    })
}

pub(super) fn item_entity_data(it: &DroppedItem) -> mod_api::ItemEntityData {
    use crate::entity::Motion;
    let (owner, motion) = match it.motion {
        Motion::Loose => (None, mod_api::ItemMotion::Loose),
        Motion::Flight(f) => (f.owner, mod_api::ItemMotion::Flight),
        Motion::Stuck(s) => (
            s.flight.owner,
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

pub(super) fn handle_entity_call(mod_id: &str, call: EntityCall) -> HostRet {
    match call {
        EntityCall::SpawnMob {
            key,
            pos,
            yaw,
            checked,
        } => match finite_pos(pos, "SpawnMob.pos") {
            Err(e) => e,
            Ok(_) if !yaw.is_finite() => HostRet::invalid("SpawnMob.yaw must be finite".into()),
            Ok(pos) => sim_query(|ctx| {
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
        EntityCall::MobInfo { mob_id } => sim_query(|ctx| {
            let position = ctx.world.mobs().position_of(mob_id);
            HostRet::Mob(
                live_mob(ctx, mob_id)
                    .zip(position)
                    .map(|(mob, position)| mob_snapshot(position, mob)),
            )
        }),
        EntityCall::MobWalkProbe {
            mob_id,
            offsets,
            max_drop,
        } => {
            if offsets.len() > mod_api::MOB_WALK_PROBE_MAX_OFFSETS {
                return HostRet::error(
                    mod_api::ErrorCode::LimitExceeded,
                    format!(
                        "MobWalkProbe: at most {} offsets",
                        mod_api::MOB_WALK_PROBE_MAX_OFFSETS
                    ),
                );
            }
            if !max_drop.is_finite()
                || !(0.0..=3.0).contains(&max_drop)
                || offsets
                    .iter()
                    .any(|v| !v.iter().all(|c| c.is_finite()) || v[0].hypot(v[1]) > 2.0)
            {
                return HostRet::invalid(
                    "MobWalkProbe: offsets must be finite and <= 2 blocks; max_drop must be 0..=3"
                        .into(),
                );
            }
            sim_query(|ctx| {
                HostRet::Bools(match live_mob(ctx, mob_id) {
                    Some(mob) => crate::mob::walk_probe::probe(ctx.world, mob, &offsets, max_drop),
                    None => vec![false; offsets.len()],
                })
            })
        }
        EntityCall::MobCanReach { mob_id, cell } => sim_query(|ctx| {
            HostRet::Bool(live_mob(ctx, mob_id).is_some_and(|mob| {
                crate::mob::mob_can_reach(
                    ctx.world,
                    mob,
                    petramond_math::math::IVec3::new(cell[0], cell[1], cell[2]),
                )
            }))
        }),
        EntityCall::PathProbe {
            key,
            from,
            to,
            blocked,
            max_nodes,
        } => {
            if let Some(err) = batch_guard("PathProbe blocked cell", blocked.len()) {
                return err;
            }
            sim_query(|ctx| {
                let Some(kind) = crate::mob::by_key(&key) else {
                    log::warn!("[mod {mod_id}] PathProbe: unknown species '{key}'");
                    return HostRet::Route(None);
                };
                let blocked: Vec<_> = blocked.iter().copied().map(IVec3::from_array).collect();
                HostRet::Route(crate::mob::route_probe(
                    ctx.world,
                    kind,
                    IVec3::from_array(from),
                    IVec3::from_array(to),
                    &blocked,
                    max_nodes as usize,
                ))
            })
        }
        EntityCall::WalkRegion {
            key,
            from,
            min,
            max,
            blocked,
            toward,
            max_nodes,
        } => {
            if let Some(err) = batch_guard("WalkRegion blocked cell", blocked.len()) {
                return err;
            }
            sim_query(|ctx| {
                let Some(kind) = crate::mob::by_key(&key) else {
                    log::warn!("[mod {mod_id}] WalkRegion: unknown species '{key}'");
                    return HostRet::Flood(mod_api::Flood::Exceeded);
                };
                let blocked: Vec<_> = blocked.iter().copied().map(IVec3::from_array).collect();
                HostRet::Flood(crate::mob::walk_region(
                    ctx.world,
                    kind,
                    crate::mob::FloodAsk {
                        from: IVec3::from_array(from),
                        span: (IVec3::from_array(min), IVec3::from_array(max)),
                        toward,
                        blocked: &blocked,
                        max_nodes: max_nodes as usize,
                    },
                ))
            })
        }
        EntityCall::Footholds { key, cells } => {
            if let Some(err) = batch_guard("Footholds cell", cells.len()) {
                return err;
            }
            sim_query(|ctx| {
                let Some(kind) = crate::mob::by_key(&key) else {
                    return HostRet::Bools(vec![false; cells.len()]);
                };
                let cells: Vec<_> = cells.iter().copied().map(IVec3::from_array).collect();
                HostRet::Bools(crate::mob::footholds(ctx.world, kind, &cells))
            })
        }
        EntityCall::MobHeldDisplay { mob_id, main, off } => {
            let resolve = |name: &Option<String>| match name {
                None => Ok(None),
                Some(name) => item_by_name(name).map(Some).ok_or(()),
            };
            let (Ok(main), Ok(off)) = (resolve(&main), resolve(&off)) else {
                return HostRet::Bool(false);
            };
            sim_query(|ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                ctx.world.mobs_mut().set_held(mob_id, [main, off]);
                HostRet::Bool(true)
            })
        }
        EntityCall::SetMobDraw {
            mob_id,
            frame,
            prims,
        } => {
            if let Some(err) = super::blocks::check_draw_set("SetMobDraw", &prims) {
                return err;
            }
            sim_query(|ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                ctx.world.mobs_mut().set_draw(
                    mob_id,
                    crate::world::draw::BodyDraw {
                        prims: prims.into(),
                        turns: frame == mod_api::DrawFrame::Body,
                    },
                );
                HostRet::Bool(true)
            })
        }
        EntityCall::SiteOpen { key, cell } => sim_query(|ctx| {
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
        EntityCall::MobsInRadius { pos, radius } => match finite_pos(pos, "MobsInRadius.pos") {
            Err(e) => e,
            Ok(pos) => sim_query(|ctx| {
                if !radius.is_finite() {
                    return HostRet::invalid("MobsInRadius: non-finite radius".into());
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
        EntityCall::MobsInRadiusOf { pos, radius, kinds } => {
            match finite_pos(pos, "MobsInRadiusOf.pos") {
                Err(e) => e,
                Ok(pos) => sim_query(|ctx| {
                    if !radius.is_finite() {
                        return HostRet::invalid("MobsInRadiusOf: non-finite radius".into());
                    }
                    let r2 = radius * radius;
                    let out = ctx
                        .world
                        .mobs()
                        .instances()
                        .iter()
                        .enumerate()
                        .filter(|(_, m)| !m.is_dead())
                        .filter(|(_, m)| kinds.contains(&mod_api::MobId(m.kind.0)))
                        .filter(|(_, m)| (m.pos - pos).length_squared() <= r2)
                        .map(|(i, m)| mob_snapshot(i, m))
                        .collect();
                    HostRet::Mobs(out)
                }),
            }
        }
        EntityCall::DamageMob {
            mob_id,
            amount,
            origin,
            feedback,
            attacker,
        } => match origin
            .map(|p| finite_pos(p, "DamageMob.origin"))
            .transpose()
        {
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
        EntityCall::DespawnMob { mob_id } => sim_query(|ctx| {
            if live_mob(ctx, mob_id).is_none() {
                return HostRet::Bool(false);
            }
            HostRet::Bool(ctx.world.mobs_mut().remove(mob_id))
        }),
        EntityCall::MobEmitterSet {
            mob_id,
            key,
            active,
        } => sim_query(|ctx| {
            if live_mob(ctx, mob_id).is_none() {
                return HostRet::Bool(false);
            }
            HostRet::Bool(ctx.world.mobs_mut().set_mob_emitter(mob_id, &key, active))
        }),
        EntityCall::MobAnimSet {
            mob_id,
            anim,
            active,
        } => match anim_name_guard("MobAnimSet", &anim) {
            Err(e) => e,
            Ok(()) => sim_query(|ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                HostRet::Bool(ctx.world.mobs_mut().set_mob_anim(mob_id, &anim, active))
            }),
        },
        EntityCall::MobAnimRate { mob_id, anim, rate } => {
            if let Err(e) = anim_name_guard("MobAnimRate", &anim) {
                return e;
            }
            if let Err(e) =
                magnitude_guard("MobAnimRate", "rate", rate, MAX_MOB_ANIM_RATE_MAGNITUDE)
            {
                return e;
            }
            sim_query(move |ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                HostRet::Bool(ctx.world.mobs_mut().set_mob_anim_rate(mob_id, &anim, rate))
            })
        }
        EntityCall::MobAnimSeek {
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
            sim_query(move |ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                HostRet::Bool(
                    ctx.world
                        .mobs_mut()
                        .set_mob_anim_seek(mob_id, &anim, phase, rate),
                )
            })
        }
        EntityCall::MobDrive {
            mob_id,
            horizontal,
            vertical,
            yaw,
            while_walking,
            gait,
        } => {
            if horizontal.is_some_and(|v| !v.iter().all(|c| c.is_finite()))
                || vertical.is_some_and(|v| !v.is_finite())
                || yaw.is_some_and(|y| !y.is_finite())
            {
                return HostRet::invalid("MobDrive: non-finite velocity/yaw".into());
            }
            if while_walking && horizontal.is_some() {
                return HostRet::invalid(
                    "MobDrive: a walking-gated intent cannot carry horizontal velocity — \
                     walking IS the horizontal locomotion"
                        .into(),
                );
            }
            if horizontal.is_some_and(|v| v[0].hypot(v[1]) > MAX_MOB_DRIVE_SPEED) {
                return HostRet::invalid(format!(
                    "MobDrive: horizontal speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
                ));
            }
            if vertical.is_some_and(|v| v.abs() > MAX_MOB_DRIVE_SPEED) {
                return HostRet::invalid(format!(
                    "MobDrive: vertical speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
                ));
            }
            sim_query(move |ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                HostRet::Bool(ctx.world.mobs_mut().set_mob_drive(
                    mob_id,
                    horizontal,
                    vertical,
                    yaw,
                    while_walking,
                    gait,
                ))
            })
        }
        EntityCall::MobKinematic {
            mob_id,
            pos,
            yaw,
            pitch,
            roll,
        } => {
            let tilt = Tilt::new(pitch, roll);
            if !pos.iter().all(|c| c.is_finite()) || !yaw.is_finite() || !tilt.is_finite() {
                return HostRet::invalid("MobKinematic: non-finite pose".into());
            }
            if pitch.abs() > std::f32::consts::FRAC_PI_2 {
                return HostRet::invalid("MobKinematic: pitch outside ±π/2".into());
            }
            if roll.abs() > std::f32::consts::PI {
                return HostRet::invalid("MobKinematic: roll outside ±π".into());
            }
            sim_query(move |ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::Bool(false);
                }
                let pos = petramond_math::world_pos::WorldPos::from_array(pos);
                match ctx
                    .world
                    .mobs_mut()
                    .set_mob_kinematic(mob_id, pos, yaw, tilt)
                {
                    Ok(placed) => HostRet::Bool(placed),
                    Err(distance) => HostRet::invalid(format!(
                        "MobKinematic: placement {distance} blocks away exceeds the \
                         {MAX_SAFE_EXTERNAL_SWEEP_DISTANCE}-block sweep bound"
                    )),
                }
            })
        }
        EntityCall::MobMount {
            mob_id,
            player_id,
            seat,
        } => sim_query(|ctx| HostRet::Bool(ctx.world.try_mount_player(player_id.0, mob_id, seat))),
        EntityCall::PlayerPoseSet {
            player_id,
            anchor,
            yaw,
            pose,
        } => {
            let anchor = match finite_pos(anchor, "PlayerPoseSet.anchor") {
                Ok(a) => a,
                Err(e) => return e,
            };
            if !yaw.is_finite() {
                return HostRet::invalid("PlayerPoseSet: non-finite yaw".into());
            }
            if pose == 0 {
                return HostRet::Bool(false);
            }
            sim_query(move |ctx| {
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
        EntityCall::MobDismount { player_id } => {
            sim_query(|ctx| HostRet::Bool(ctx.world.riding_mut().dismount(player_id.0).is_some()))
        }
        EntityCall::MobRiders { mob_id } => sim_query(|ctx| {
            let Some(mob) = live_mob(ctx, mob_id) else {
                return HostRet::Riders(None);
            };
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
        EntityCall::MobDriveMany { drives } => {
            if let Some(err) = batch_guard("MobDriveMany drive", drives.len()) {
                return err;
            }
            for drive in &drives {
                if let Err(err) = intents::drive_guard(drive) {
                    return err;
                }
            }
            sim_query(|ctx| {
                HostRet::Bools(
                    drives
                        .into_iter()
                        .map(|d| intents::apply_drive(ctx, d))
                        .collect(),
                )
            })
        }
        EntityCall::MobKinematicMany { poses } => {
            if let Some(err) = batch_guard("MobKinematicMany pose", poses.len()) {
                return err;
            }
            let tilts = match poses
                .iter()
                .map(intents::kinematic_guard)
                .collect::<Result<Vec<_>, _>>()
            {
                Ok(tilts) => tilts,
                Err(err) => return err,
            };
            sim_query(|ctx| {
                let mut accepted = Vec::with_capacity(poses.len());
                for (pose, tilt) in poses.into_iter().zip(tilts) {
                    match intents::apply_kinematic(ctx, pose, tilt) {
                        Ok(ok) => accepted.push(ok),
                        Err(err) => return err,
                    }
                }
                HostRet::Bools(accepted)
            })
        }
        EntityCall::MobAnimMany { ops } => {
            if let Some(err) = batch_guard("MobAnimMany command", ops.len()) {
                return err;
            }
            for op in &ops {
                if let Err(err) = intents::anim_guard(op) {
                    return err;
                }
            }
            sim_query(|ctx| {
                HostRet::Bools(ops.iter().map(|op| intents::apply_anim(ctx, op)).collect())
            })
        }
        EntityCall::MobRidersMany { mob_ids } => {
            if let Some(err) = batch_guard("MobRidersMany mob", mob_ids.len()) {
                return err;
            }
            sim_query(|ctx| {
                HostRet::RidersMany(
                    mob_ids
                        .into_iter()
                        .map(|id| intents::riders(ctx, id))
                        .collect(),
                )
            })
        }
        EntityCall::BlockModelGroup { pos } => sim_query(|ctx| {
            let p = petramond_math::math::IVec3::new(pos[0], pos[1], pos[2]);
            HostRet::ModelGroup(ctx.world.model_group(p).map(|(_, base, _)| {
                mod_api::ModelGroupData {
                    base: [base.x, base.y, base.z],
                    facing: match ctx.world.data().model_facing_at(base.x, base.y, base.z) {
                        petramond_math::facing::Facing::North => mod_api::Facing::North,
                        petramond_math::facing::Facing::South => mod_api::Facing::South,
                        petramond_math::facing::Facing::West => mod_api::Facing::West,
                        petramond_math::facing::Facing::East => mod_api::Facing::East,
                    },
                }
            }))
        }),
        EntityCall::MobAnimState { mob_id, anim } => {
            if let Err(e) = anim_name_guard("MobAnimState", &anim) {
                return e;
            }
            sim_query(move |ctx| {
                if live_mob(ctx, mob_id).is_none() {
                    return HostRet::MobAnimState(None);
                }
                HostRet::MobAnimState(ctx.world.mobs().mob_anim_state(mob_id, &anim).map(|state| {
                    MobAnimStateData {
                        phase: state.phase,
                        rate: state.rate,
                        seek: state.seek,
                    }
                }))
            })
        }
        EntityCall::SpawnItem {
            item,
            count,
            pos,
            data,
        } => match finite_pos(pos, "SpawnItem.pos") {
            Err(e) => e,
            Ok(pos) => {
                let variant = match super::guards::intern_abi_data("SpawnItem", &data) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                sim_query(|ctx| {
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
        EntityCall::LaunchItem {
            item,
            pos,
            vel,
            owner,
            data,
        } => match (
            finite_pos(pos, "LaunchItem.pos"),
            finite3(vel, "LaunchItem.vel"),
        ) {
            (Err(e), _) | (_, Err(e)) => e,
            (Ok(pos), Ok(vel)) => {
                let variant = match super::guards::intern_abi_data("LaunchItem", &data) {
                    Ok(v) => v,
                    Err(e) => return e,
                };
                sim_query(|ctx| {
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
        EntityCall::ItemEntity { entity } => sim_query(|ctx| {
            HostRet::ItemEntity(
                ctx.world
                    .dropped_items()
                    .get(entity)
                    .map(|item| Box::new(item_entity_data(item))),
            )
        }),
    }
}

fn spawn_item_stacks(
    ctx: &mut SimCtx<'_>,
    item: ItemType,
    count: u8,
    pos: petramond_math::world_pos::WorldPos,
    variant: petramond_world::item::VariantId,
) {
    let cell = pos.block();
    let sky = ctx.world.data().skylight6_at_world(cell.x, cell.y, cell.z);
    let block = petramond_world::light::BlockLight6::from_x2(
        ctx.world
            .data()
            .blocklight_rgb_at_world(cell.x, cell.y, cell.z),
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

fn drop_seed(tick: u64, pos: petramond_math::world_pos::WorldPos, i: u32) -> u32 {
    let mut z = tick
        ^ pos.x.to_bits().rotate_left(32)
        ^ pos.z.to_bits()
        ^ pos.y.to_bits().rotate_left(16)
        ^ ((i as u64) << 1);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    (z ^ (z >> 31)) as u32
}

fn fill_inventory(
    player: &mut crate::player::Player,
    item: ItemType,
    count: u8,
    variant: petramond_world::item::VariantId,
) -> (Vec<ItemStack>, petramond_math::world_pos::WorldPos) {
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

fn drop_leftovers(
    world: &mut crate::world::ServerWorld,
    at: petramond_math::world_pos::WorldPos,
    leftovers: Vec<ItemStack>,
) {
    for (i, leftover) in leftovers.into_iter().enumerate() {
        let seed = drop_seed(world.current_tick(), at, i as u32);
        let cell = at.block();
        let mut drop = DroppedItem::new(at, leftover, seed);
        drop.skylight = world.data().skylight6_at_world(cell.x, cell.y, cell.z);
        drop.blocklight = petramond_world::light::BlockLight6::from_x2(
            world.data().blocklight_rgb_at_world(cell.x, cell.y, cell.z),
        );
        world.spawn_item(drop);
    }
}

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
        calls, HostCall, HostRet, MobAnimStateData, MobRidersData, MAX_MOB_ANIM_NAME_BYTES,
        MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE,
    };
    use petramond_math::world_pos::WorldPos;

    use crate::events::tick::TickEvents;
    use crate::events::{PostQueue, RosterRefs, SimCtx};
    use crate::modding::host::{handle_host_call, ModStoreData};
    use crate::modding::scope;
    use crate::world::ServerWorld;
    use petramond_world::chunk::{ChunkPos, SECTION_VOLUME};

    #[test]
    fn spawn_mob_initializes_cached_light_before_first_render_snapshot() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        let section = world
            .section_at_world_mut_for_test(8, 64, 8)
            .expect("fixture loads the spawn section");
        section.set_skylight(vec![0; SECTION_VOLUME].into());
        section.set_blocklight(vec![petramond_world::light::LightRgb::ZERO; SECTION_VOLUME].into());

        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert!(matches!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                        checked: false,
                    }),
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
        let mut world = ServerWorld::new(1, 1);
        world.insert_empty_column_for_test(ChunkPos::new(0, 0));
        world.set_block_world(8, 64, 8, petramond_world::block::Block::Stone);
        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            let checked = |pos| {
                HostCall::from(calls::SpawnMob {
                    key: "petramond:owl".into(),
                    pos,
                    yaw: 0.0,
                    checked: true,
                })
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
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };
        scope::enter(&mut ctx, || {
            assert!(matches!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::SpawnMob {
                        key: "petramond:owl".into(),
                        pos: [8.5, 64.0, 8.5],
                        yaw: 0.0,
                        checked: true,
                    }),
                ),
                HostRet::SpawnedMob(Some(_))
            ));
        });
        assert_eq!(world.mobs().instances().len(), 1);
    }

    #[test]
    fn mob_snapshot_id_survives_unrelated_despawn_index_shift() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, WorldPos::new(1.0, 80.0, 1.0), 0.0));
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, WorldPos::new(2.0, 80.0, 2.0), 0.0));
        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            let before = match handle_host_call(
                &mut data,
                HostCall::from(calls::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                }),
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
                    HostCall::from(calls::DespawnMob {
                        mob_id: before[0].id
                    })
                ),
                HostRet::Bool(true)
            );

            let after = match handle_host_call(
                &mut data,
                HostCall::from(calls::MobsInRadius {
                    pos: [0.0, 80.0, 0.0],
                    radius: 10.0,
                }),
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
                matches!(super::handle_entity_call("alpha", call), HostRet::Err(_)),
                "out-of-envelope call must be a protocol error"
            );
        };

        for (offsets, max_drop) in [
            (vec![[f32::NAN, 0.0]], 1.0),
            (vec![[3.0, 0.0]], 1.0),
            (vec![[0.5, 0.0]], f32::INFINITY),
            (vec![[0.5, 0.0]], -1.0),
            (
                vec![[0.0, 0.0]; mod_api::MOB_WALK_PROBE_MAX_OFFSETS + 1],
                1.0,
            ),
        ] {
            rejected(calls::MobWalkProbe {
                mob_id: 1,
                offsets,
                max_drop,
            });
        }
        rejected(calls::MobAnimSet {
            mob_id: 1,
            anim: "a".repeat(MAX_MOB_ANIM_NAME_BYTES + 1),
            active: true,
        });
        rejected(calls::MobAnimRate {
            mob_id: 1,
            anim: "row".into(),
            rate: MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0,
        });
        rejected(calls::MobAnimSeek {
            mob_id: 1,
            anim: "row".into(),
            phase: MAX_MOB_ANIM_PHASE_MAGNITUDE * 2.0,
            rate: 1.0,
        });
        rejected(calls::MobAnimSeek {
            mob_id: 1,
            anim: "row".into(),
            phase: 0.0,
            rate: MAX_MOB_ANIM_RATE_MAGNITUDE * 2.0,
        });
        rejected(calls::MobDrive {
            mob_id: 1,
            horizontal: Some([super::MAX_MOB_DRIVE_SPEED * 2.0, 0.0]),
            vertical: None,
            yaw: None,
            while_walking: false,
            gait: false,
        });
        rejected(calls::MobDrive {
            mob_id: 1,
            horizontal: None,
            vertical: Some(super::MAX_MOB_DRIVE_SPEED * 2.0),
            yaw: None,
            while_walking: false,
            gait: false,
        });
        rejected(calls::MobDrive {
            mob_id: 1,
            horizontal: Some([1.0, 0.0]),
            vertical: Some(1.0),
            yaw: None,
            while_walking: true,
            gait: false,
        });
    }

    #[test]
    fn mob_queries_distinguish_missing_mobs_and_expose_authoritative_anim_state() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, WorldPos::new(1.0, 80.0, 1.0), 0.0));
        let mob_id = world.mobs().instances()[0].id();
        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
            feed: &mut feed,
            queue: &mut queue,
        };

        scope::enter(&mut ctx, || {
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobRiders { mob_id: u64::MAX })
                ),
                HostRet::Riders(None)
            );
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::MobRiders { mob_id })),
                HostRet::Riders(Some(MobRidersData {
                    capacity: crate::mob::def(crate::mob::Mob::Owl).seats.len() as u8,
                    riders: Vec::new(),
                }))
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobAnimState {
                        mob_id,
                        anim: "row".into(),
                    })
                ),
                HostRet::MobAnimState(None)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobAnimSet {
                        mob_id,
                        anim: "row".into(),
                        active: true,
                    })
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobAnimSeek {
                        mob_id,
                        anim: "row".into(),
                        phase: 1.5,
                        rate: -0.75,
                    })
                ),
                HostRet::Bool(true)
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobAnimState {
                        mob_id,
                        anim: "row".into(),
                    })
                ),
                HostRet::MobAnimState(Some(MobAnimStateData {
                    phase: 0.0,
                    rate: 0.75,
                    seek: Some(1.5),
                }))
            );
        });
    }

    #[test]
    fn a_dead_mob_is_gone_to_every_id_addressed_call() {
        let mut data = ModStoreData::new("alpha", 1);
        let mut world = ServerWorld::new(1, 1);
        assert!(world
            .mobs_mut()
            .spawn(crate::mob::Mob::Owl, WorldPos::new(1.0, 80.0, 1.0), 0.0));
        let mob_id = world.mobs().instances()[0].id();
        assert!(world
            .mobs_mut()
            .damage_mob(
                mob_id,
                1000.0,
                None,
                true,
                None,
                &crate::mob::MobDamageFeedback::default(),
            )
            .is_some());
        assert!(world.mobs().instances()[0].is_dead());

        let mut nobody = RosterRefs::empty();
        let mut feed = TickEvents::default();
        let mut queue = PostQueue::default();
        let mut ctx = SimCtx {
            world: &mut world,
            actor: None,
            players: &mut nobody,
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
                HostCall::from(calls::MobAnimSet {
                    mob_id,
                    anim: "row".into(),
                    active: true,
                }),
            );
            refused(
                &mut data,
                HostCall::from(calls::MobDrive {
                    mob_id,
                    horizontal: Some([1.0, 0.0]),
                    vertical: None,
                    yaw: None,
                    while_walking: false,
                    gait: false,
                }),
            );
            refused(
                &mut data,
                HostCall::from(calls::MobEmitterSet {
                    mob_id,
                    key: "petramond:burn_light".into(),
                    active: true,
                }),
            );
            refused(
                &mut data,
                HostCall::from(calls::MobTagSet {
                    mob_id,
                    key: "alpha:x".into(),
                    value: mod_api::MobTagValue::I64(1),
                }),
            );
            assert_eq!(
                handle_host_call(
                    &mut data,
                    HostCall::from(calls::MobTagGet {
                        mob_id,
                        key: "alpha:x".into(),
                    })
                ),
                HostRet::MobTag(mod_api::MobTagLookup::MissingMob)
            );
            assert_eq!(
                handle_host_call(&mut data, HostCall::from(calls::MobRiders { mob_id })),
                HostRet::Riders(None)
            );
        });
    }
}
