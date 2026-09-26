//! The per-mob intents a tick system issues for a whole population — drive,
//! kinematic pose, named-animation playback — and the rider read beside
//! them. Each is validated and applied here ONCE, so the single call and its
//! batched twin (`MobDriveMany`, `MobKinematicMany`, `MobAnimMany`,
//! `MobRidersMany`) cannot disagree about what an element means.

use mod_api::{
    HostRet, MobAnimOp, MobDriveData, MobKinematicData, MobRiderData, MobRidersData,
    MAX_MOB_ANIM_NAME_BYTES, MAX_MOB_ANIM_PHASE_MAGNITUDE, MAX_MOB_ANIM_RATE_MAGNITUDE,
};

use crate::events::SimCtx;
use petramond_math::math::Tilt;
use petramond_world::collision::MAX_SAFE_EXTERNAL_SWEEP_DISTANCE;

use super::super::guards::live_mob;

/// Maximum horizontal speed accepted from `MobDrive`, derived from the
/// collision resolver's bounded external sweep and the fixed simulation tick.
pub(super) const MAX_MOB_DRIVE_SPEED: f32 =
    MAX_SAFE_EXTERNAL_SWEEP_DISTANCE / crate::events::tick::TICK_DT;

pub(super) fn anim_name_guard(call: &str, anim: &str) -> Result<(), HostRet> {
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

/// The rules every drive intent obeys.
pub(super) fn drive_guard(d: &MobDriveData) -> Result<(), HostRet> {
    if d.horizontal
        .is_some_and(|v| !v.iter().all(|c| c.is_finite()))
        || d.vertical.is_some_and(|v| !v.is_finite())
        || d.yaw.is_some_and(|y| !y.is_finite())
    {
        return Err(HostRet::invalid("MobDrive: non-finite velocity/yaw".into()));
    }
    if d.while_walking && d.horizontal.is_some() {
        return Err(HostRet::invalid(
            "MobDrive: a walking-gated intent cannot carry horizontal velocity — \
             walking IS the horizontal locomotion"
                .into(),
        ));
    }
    if d.horizontal
        .is_some_and(|v| v[0].hypot(v[1]) > MAX_MOB_DRIVE_SPEED)
    {
        return Err(HostRet::invalid(format!(
            "MobDrive: horizontal speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
        )));
    }
    if d.vertical.is_some_and(|v| v.abs() > MAX_MOB_DRIVE_SPEED) {
        return Err(HostRet::invalid(format!(
            "MobDrive: vertical speed exceeds {MAX_MOB_DRIVE_SPEED} m/s"
        )));
    }
    Ok(())
}

/// Latch one validated drive intent for this tick (see
/// `Instance::set_drive`); `false` = unknown or dead mob.
pub(super) fn apply_drive(ctx: &mut SimCtx<'_>, d: MobDriveData) -> bool {
    let Some(_mob) = live_mob(ctx, d.mob_id) else {
        return false;
    };
    ctx.world.mobs_mut().set_mob_drive(
        d.mob_id,
        d.horizontal,
        d.vertical,
        d.yaw,
        d.while_walking,
        d.gait,
    )
}

/// The rules every kinematic pose obeys; the pose's tilt on success.
pub(super) fn kinematic_guard(k: &MobKinematicData) -> Result<Tilt, HostRet> {
    let tilt = Tilt::new(k.pitch, k.roll);
    if !k.pos.iter().all(|c| c.is_finite()) || !k.yaw.is_finite() || !tilt.is_finite() {
        return Err(HostRet::invalid("MobKinematic: non-finite pose".into()));
    }
    if k.pitch.abs() > std::f32::consts::FRAC_PI_2 {
        return Err(HostRet::invalid("MobKinematic: pitch outside ±π/2".into()));
    }
    if k.roll.abs() > std::f32::consts::PI {
        return Err(HostRet::invalid("MobKinematic: roll outside ±π".into()));
    }
    Ok(tilt)
}

/// Place one validated pose for this tick (see `Instance::set_kinematic`):
/// `Ok(false)` = unknown or dead mob, `Err` = a placement past the sweep
/// bound.
pub(super) fn apply_kinematic(
    ctx: &mut SimCtx<'_>,
    k: MobKinematicData,
    tilt: Tilt,
) -> Result<bool, HostRet> {
    let Some(_mob) = live_mob(ctx, k.mob_id) else {
        return Ok(false);
    };
    let pos = petramond_math::world_pos::WorldPos::from_array(k.pos);
    ctx.world
        .mobs_mut()
        .set_mob_kinematic(k.mob_id, pos, k.yaw, tilt)
        .map_err(|distance| {
            HostRet::invalid(format!(
                "MobKinematic: placement {distance} blocks away exceeds the \
                 {MAX_SAFE_EXTERNAL_SWEEP_DISTANCE}-block sweep bound"
            ))
        })
}

/// The rules every named-animation command obeys.
pub(super) fn anim_guard(op: &MobAnimOp) -> Result<(), HostRet> {
    match op {
        MobAnimOp::Set { anim, .. } => anim_name_guard("MobAnimSet", anim),
        MobAnimOp::Rate { anim, rate, .. } => {
            anim_name_guard("MobAnimRate", anim)?;
            magnitude_guard("MobAnimRate", "rate", *rate, MAX_MOB_ANIM_RATE_MAGNITUDE)
        }
        MobAnimOp::Seek {
            anim, phase, rate, ..
        } => {
            anim_name_guard("MobAnimSeek", anim)?;
            magnitude_guard("MobAnimSeek", "phase", *phase, MAX_MOB_ANIM_PHASE_MAGNITUDE)?;
            magnitude_guard("MobAnimSeek", "rate", *rate, MAX_MOB_ANIM_RATE_MAGNITUDE)
        }
    }
}

/// Apply one validated animation command; `false` = unknown mob, the per-mob
/// cap (a set), or an inactive animation (a rate or seek).
pub(super) fn apply_anim(ctx: &mut SimCtx<'_>, op: &MobAnimOp) -> bool {
    let mob_id = match op {
        MobAnimOp::Set { mob_id, .. }
        | MobAnimOp::Rate { mob_id, .. }
        | MobAnimOp::Seek { mob_id, .. } => *mob_id,
    };
    let Some(_mob) = live_mob(ctx, mob_id) else {
        return false;
    };
    let mobs = ctx.world.mobs_mut();
    match op {
        MobAnimOp::Set { anim, active, .. } => mobs.set_mob_anim(mob_id, anim, *active),
        MobAnimOp::Rate { anim, rate, .. } => mobs.set_mob_anim_rate(mob_id, anim, *rate),
        MobAnimOp::Seek {
            anim, phase, rate, ..
        } => mobs.set_mob_anim_seek(mob_id, anim, *phase, *rate),
    }
}

/// The seat capacity and riders of the live mob `mob_id`; `None` = no such
/// live mob.
pub(super) fn riders(ctx: &SimCtx<'_>, mob_id: u64) -> Option<MobRidersData> {
    let mob = live_mob(ctx, mob_id)?;
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
    Some(MobRidersData { capacity, riders })
}
