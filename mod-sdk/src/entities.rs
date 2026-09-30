use mod_api::{
    ConditionOp, EntityRef, Facing, MobAnimOp, MobAnimStateData, MobDriveData, MobId,
    MobKinematicData, MobRidersData, MobSnapshot, PlayerId,
};

use crate::__rt::host_fn;
use crate::__rt::try_host_fn;

try_host_fn! {
    pub fn try_mob_drive_many(drives: Vec<MobDriveData>) -> Vec<bool>
        => MobDriveMany { drives } => Bools
}

try_host_fn! {
    pub fn try_mob_kinematic_many(poses: Vec<MobKinematicData>) -> Vec<bool>
        => MobKinematicMany { poses } => Bools
}

try_host_fn! {
    pub fn try_mob_anim_many(ops: Vec<MobAnimOp>) -> Vec<bool>
        => MobAnimMany { ops } => Bools
}

try_host_fn! {
    pub fn try_mob_riders_many(mob_ids: Vec<u64>) -> Vec<Option<MobRidersData>>
        => MobRidersMany { mob_ids } => RidersMany
}

try_host_fn! {
    pub fn try_entity_conditions_many(ops: Vec<ConditionOp>) -> Vec<bool>
        => EntityConditionsMany { ops } => Bools
}

host_fn! {
    pub fn mob_drive_many(drives: Vec<MobDriveData>) -> Vec<bool>
        => MobDriveMany { drives } => Bools
}

host_fn! {
    pub fn mob_kinematic_many(poses: Vec<MobKinematicData>) -> Vec<bool>
        => MobKinematicMany { poses } => Bools
}

host_fn! {
    pub fn mob_anim_many(ops: Vec<MobAnimOp>) -> Vec<bool>
        => MobAnimMany { ops } => Bools
}

host_fn! {
    pub fn mob_riders_many(mob_ids: Vec<u64>) -> Vec<Option<MobRidersData>>
        => MobRidersMany { mob_ids } => RidersMany
}

host_fn! {
    pub fn entity_conditions_many(ops: Vec<ConditionOp>) -> Vec<bool>
        => EntityConditionsMany { ops } => Bools
}

pub fn mob_facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [-s, -c]
}

host_fn! {
    pub fn item_entities_in_radius(pos: [f64; 3], radius: f32, limit: u32) -> Vec<mod_api::ItemEntityData>
        => ItemEntitiesInRadius { pos, radius, limit } => ItemEntities
}

host_fn! {
    pub fn item_impulses(impulses: Vec<(u64, [f32; 3])>) -> Vec<bool>
        => ItemImpulses { impulses } => Bools
}

host_fn! {
    pub fn spawn_mob(key: &str, pos: [f64; 3], yaw: f32) -> Option<u64>
        => SpawnMob { key: key.into(), pos, yaw, checked: false } => SpawnedMob
}

host_fn! {
    pub fn spawn_mob_checked(key: &str, pos: [f64; 3], yaw: f32) -> Option<u64>
        => SpawnMob { key: key.into(), pos, yaw, checked: true } => SpawnedMob
}

host_fn! {
    pub fn mobs_in_radius(pos: [f64; 3], radius: f32) -> Vec<MobSnapshot>
        => MobsInRadius { pos, radius } => Mobs
}

host_fn! {
    /// [`mobs_in_radius`] for the species `kinds` only: the host snapshots nothing else.
    pub fn mobs_in_radius_of(pos: [f64; 3], radius: f32, kinds: Vec<MobId>) -> Vec<MobSnapshot>
        => MobsInRadiusOf { pos, radius, kinds } => Mobs
}

host_fn! {
    pub fn mob_info(mob_id: u64) -> Option<MobSnapshot> => MobInfo { mob_id } => Mob
}

host_fn! {
    pub fn mob_can_reach(mob_id: u64, cell: [i32; 3]) -> bool
        => MobCanReach { mob_id, cell } => Bool
}

host_fn! {
    pub fn site_open(key: &str, cell: [i32; 3]) -> bool
        => SiteOpen { key: key.into(), cell } => Bool
}

host_fn! {
    /// Damages a live mob (STABLE id) via `mob_damage_pre`, using the species' resolved
    /// `damage_feedback`. Applied at the next in-tick drain, so a mob that's already gone just
    /// no-ops.
    ///
    /// `attacker` decides credit. `None` is the mod's own damage, so no knockback and no
    /// retaliation memory. A player makes it their melee strike: the victim remembers them and
    /// `origin` drives the knockback direction. Forward a hit owed from `attack_attempt` this way.
    /// The id has to be a connected session or it's a mod bug. A mob makes it that mob's strike.
    pub fn damage_mob(
        mob_id: u64,
        amount: f32,
        origin: Option<[f64; 3]>,
        attacker: Option<EntityRef>,
    )
        => DamageMob { mob_id, amount, origin, feedback: None, attacker }
}

host_fn! {
    pub fn damage_mob_with_feedback(
        mob_id: u64,
        amount: f32,
        origin: Option<[f64; 3]>,
        feedback: crate::MobDamageFeedback,
        attacker: Option<EntityRef>,
    )
        => DamageMob { mob_id, amount, origin, feedback: Some(feedback), attacker }
}

host_fn! {
    pub fn mob_emitter_set(mob_id: u64, key: &str, active: bool) -> bool
        => MobEmitterSet { mob_id, key: key.into(), active } => Bool
}

host_fn! {
    pub fn emitter_burst(key: &str, pos: [f64; 3], intensity: f32) -> bool
        => EmitterBurst { key: key.into(), pos, intensity, direction: None, texture: None } => Bool
}

host_fn! {
    pub fn emitter_burst_of(
        key: &str,
        pos: [f64; 3],
        intensity: f32,
        direction: Option<[f32; 3]>,
        texture: Option<mod_api::ParticleTexture>
    ) -> bool
        => EmitterBurst { key: key.into(), pos, intensity, direction, texture } => Bool
}

host_fn! {
    pub fn mob_anim_set(mob_id: u64, anim: &str, active: bool) -> bool
        => MobAnimSet { mob_id, anim: anim.into(), active } => Bool
}

host_fn! {
    pub fn mob_anim_rate(mob_id: u64, anim: &str, rate: f32) -> bool
        => MobAnimRate { mob_id, anim: anim.into(), rate } => Bool
}

host_fn! {
    pub fn mob_anim_seek(mob_id: u64, anim: &str, phase: f32, rate: f32) -> bool
        => MobAnimSeek { mob_id, anim: anim.into(), phase, rate } => Bool
}

host_fn! {
    pub fn mob_anim_state(mob_id: u64, anim: &str) -> Option<MobAnimStateData>
        => MobAnimState { mob_id, anim: anim.into() } => MobAnimState
}

host_fn! {
    pub fn mob_drive(mob_id: u64, vel: [f32; 2], yaw: Option<f32>) -> bool
        => MobDrive { mob_id, horizontal: Some(vel), vertical: None, yaw, while_walking: false, gait: false } => Bool
}

host_fn! {
    pub fn mob_step(mob_id: u64, vel: [f32; 2]) -> bool
        => MobDrive { mob_id, horizontal: Some(vel), vertical: None, yaw: None, while_walking: false, gait: true } => Bool
}

host_fn! {
    pub fn mob_drive_velocity(mob_id: u64, vel: [f32; 3], yaw: Option<f32>) -> bool
        => MobDrive { mob_id, horizontal: Some([vel[0], vel[2]]), vertical: Some(vel[1]), yaw, while_walking: false, gait: false } => Bool
}

host_fn! {
    pub fn mob_kinematic(mob_id: u64, pos: [f64; 3], yaw: f32, pitch: f32, roll: f32) -> bool
        => MobKinematic { mob_id, pos, yaw, pitch, roll } => Bool
}

host_fn! {
    /// Sets a live mob's vertical velocity (m/s) for this tick only, without touching the
    /// brain's own walking. Gravity resumes next tick, and water buoyancy is still engine-owned.
    /// An upward value from the ground counts as a launch - the walk gait carries through the arc
    /// and the clip re-phases onto a cycle boundary. The engine's own nav-step jump wins if it
    /// fires the same tick. This is how a pack builds gaits (hop, pounce, lunge) on top of raw
    /// velocity. Check the snapshot's `moving` and `on_ground` to time it.
    ///
    /// Pass `true` for `while_walking` on a gait launch, so the engine drops it if the walk ended
    /// before the intent got consumed. Latched intents go off last tick's state, so an arrival in
    /// between would cause a stale bounce at the destination. Pass `false` for an unconditional
    /// launch, like a startle jump.
    ///
    /// Re-issue per launch, like [`mob_drive`]. Returns `false` if the mob's unknown or dead.
    pub fn mob_drive_vertical(mob_id: u64, vel: f32, while_walking: bool) -> bool
        => MobDrive { mob_id, horizontal: None, vertical: Some(vel), yaw: None, while_walking, gait: false } => Bool
}

host_fn! {
    pub fn mob_mount(mob_id: u64, player_id: PlayerId, seat: u8) -> bool
        => MobMount { mob_id, player_id, seat } => Bool
}

host_fn! {
    pub fn player_pose_set(player_id: PlayerId, anchor: [f64; 3], yaw: f32, pose: u8) -> bool
        => PlayerPoseSet { player_id, anchor, yaw, pose } => Bool
}

host_fn! {
    pub fn mob_dismount(player_id: PlayerId) -> bool => MobDismount { player_id } => Bool
}

host_fn! {
    pub fn mob_riders(mob_id: u64) -> Option<MobRidersData> => MobRiders { mob_id } => Riders
}

pub fn footprint_local_to_world(
    base: [i32; 3],
    footprint: [u8; 3],
    facing: Facing,
    local: [f32; 3],
) -> [f64; 3] {
    let (sx, sz) = (footprint[0] as f32, footprint[2] as f32);
    let [x, y, z] = local;
    let (rx, rz) = match facing {
        Facing::North => (x, z),
        Facing::South => (sx - x, sz - z),
        Facing::East => (sz - z, x),
        Facing::West => (z, sx - x),
    };
    [
        f64::from(base[0]) + f64::from(rx),
        f64::from(base[1]) + f64::from(y),
        f64::from(base[2]) + f64::from(rz),
    ]
}

pub fn facing_player_yaw(facing: Facing) -> f32 {
    use std::f32::consts::{FRAC_PI_2, PI};
    match facing {
        Facing::North => PI,
        Facing::South => 0.0,
        Facing::East => FRAC_PI_2,
        Facing::West => -FRAC_PI_2,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Same convention as the engine's `placement_transform_fp`. North is identity, South mirrors
    /// X and Z, and East/West swap axes with one mirror. Keep them in sync, or seated poses end up
    /// off the cushions on rotated placements.
    #[test]
    fn footprint_mapping_matches_the_engine_placement_transform() {
        let fp = [1, 2, 1];
        let local = [0.5, -0.25, 0.25];
        let base = [10, 5, 10];
        assert_eq!(
            footprint_local_to_world(base, fp, Facing::North, local),
            [10.5, 4.75, 10.25]
        );
        assert_eq!(
            footprint_local_to_world(base, fp, Facing::South, local),
            [10.5, 4.75, 10.75]
        );
        assert_eq!(
            footprint_local_to_world(base, fp, Facing::East, local),
            [10.75, 4.75, 10.5]
        );
        assert_eq!(
            footprint_local_to_world(base, fp, Facing::West, local),
            [10.25, 4.75, 10.5]
        );
    }
}

host_fn! {
    pub fn despawn_mob(mob_id: u64) -> bool => DespawnMob { mob_id } => Bool
}

host_fn! {
    pub fn spawn_item(item: &str, count: u8, pos: [f64; 3]) -> bool
        => SpawnItem { item: item.into(), count, pos, data: Vec::new() } => Bool
}

host_fn! {
    pub fn launch_item(
        item: &str,
        pos: [f64; 3],
        vel: [f32; 3],
        owner: Option<mod_api::EntityRef>,
        data: &[(&str, &[u8])],
    ) -> u64
        => LaunchItem {
            item: item.into(),
            pos,
            vel,
            owner,
            data: data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
        } => U64
}

host_fn! {
    pub fn item_entity(entity: u64) -> Option<Box<mod_api::ItemEntityData>>
        => ItemEntity { entity } => ItemEntity
}

host_fn! {
    pub fn spawn_item_data(item: &str, count: u8, pos: [f64; 3], data: &[(&str, &[u8])]) -> bool
        => SpawnItem {
            item: item.into(),
            count,
            pos,
            data: data.iter().map(|(k, v)| (k.to_string(), v.to_vec())).collect(),
        } => Bool
}

host_fn! {
    pub fn entity_condition_apply(entity: EntityRef, condition: mod_api::ConditionId, stage: u8, ticks: u32) -> bool
        => EntityConditionApply { entity, condition, stage, ticks } => Bool
}
host_fn! {
    pub fn entity_condition_cool(entity: EntityRef, condition: mod_api::ConditionId, ticks: u32) -> bool
        => EntityConditionCool { entity, condition, ticks } => Bool
}

host_fn! {
    pub fn path_probe(key: &str, from: [i32; 3], to: [i32; 3], blocked: Vec<[i32; 3]>, max_nodes: u32) -> Option<mod_api::Route>
        => PathProbe { key: key.into(), from, to, blocked, max_nodes } => Route
}

host_fn! {
    pub fn walk_region(key: &str, from: [i32; 3], min: [i32; 3], max: [i32; 3], blocked: Vec<[i32; 3]>, toward: bool, max_nodes: u32) -> mod_api::Flood
        => WalkRegion { key: key.into(), from, min, max, blocked, toward, max_nodes } => Flood
}

host_fn! {
    pub fn footholds(key: &str, cells: Vec<[i32; 3]>) -> Vec<bool>
        => Footholds { key: key.into(), cells } => Bools
}

host_fn! {
    pub fn mob_held_display(mob_id: u64, main: Option<String>, off: Option<String>) -> bool
        => MobHeldDisplay { mob_id, main, off } => Bool
}

host_fn! {
    pub fn set_mob_draw(mob_id: u64, frame: mod_api::DrawFrame, prims: Vec<mod_api::DrawPrim>) -> bool
        => SetMobDraw { mob_id, frame, prims } => Bool
}

host_fn! {
    /// Tests short straight walking legs against collision and safe footing.
    pub fn mob_walk_probe(mob_id: u64, offsets: Vec<[f32; 2]>, max_drop: f32) -> Vec<bool>
        => MobWalkProbe { mob_id, offsets, max_drop } => Bools
}
