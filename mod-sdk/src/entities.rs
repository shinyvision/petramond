//! Sim-scoped entity calls: mob spawn/query/damage/despawn, keyed particle
//! emitters, riding, kinematic drive, named animations, and dropped-item
//! spawns.

use mod_api::{Facing, MobAnimStateData, MobRidersData, MobSnapshot, PlayerId};

use crate::__rt::host_fn;

/// The horizontal direction a mob yaw faces — MOB convention: yaw `0` faces
/// `-Z`. The frame [`mob_drive`] velocities/yaws and `mobs.json` seat offsets
/// speak. (Player yaw is π apart: yaw `0` faces `+Z` — see
/// [`crate::player_facing_xz`].)
pub fn mob_facing_xz(yaw: f32) -> [f32; 2] {
    let (s, c) = yaw.sin_cos();
    [-s, -c]
}

host_fn! {
    /// Spawn a mob by species key at `pos` (feet) facing `yaw`, unconditionally
    /// (site fitness is your business — see [`spawn_mob_checked`]). Returns the
    /// newborn's STABLE id — tag/configure it immediately through the ordinary
    /// mob calls. `None` = unknown species or the mob cap is reached.
    pub fn spawn_mob(key: &str, pos: [f32; 3], yaw: f32) -> Option<u64>
        => SpawnMob { key: key.into(), pos, yaw, checked: false } => SpawnedMob
}

host_fn! {
    /// [`spawn_mob`] that spawns only if the whole declared body fits loaded,
    /// stream-final world state at `pos`/`yaw`, including exact terrain collision
    /// shapes and other live solid mobs. `None` also covers unknown terrain or
    /// species and the mob cap.
    /// Use this for player-placed vehicles and other solid entities; a failed call
    /// mutates nothing, so the caller can safely retain or refund its item.
    pub fn spawn_mob_checked(key: &str, pos: [f32; 3], yaw: f32) -> Option<u64>
        => SpawnMob { key: key.into(), pos, yaw, checked: true } => SpawnedMob
}

host_fn! {
    /// Snapshot the live mobs within `radius` of `pos` (3-D, feet positions), in
    /// the deterministic live-set storage order. Address a mob by its stable
    /// `id`; the snapshot `index` is only an intra-tick join key.
    pub fn mobs_in_radius(pos: [f32; 3], radius: f32) -> Vec<MobSnapshot>
        => MobsInRadius { pos, radius } => Mobs
}

host_fn! {
    /// Snapshot ONE live mob by its stable id — for a handler that already
    /// holds an id (an event payload, a stored tag) and needs the mob's
    /// current pose/species. `None` = no such live mob.
    pub fn mob_info(mob_id: u64) -> Option<MobSnapshot> => MobInfo { mob_id } => Mob
}

host_fn! {
    /// Whether the live mob `mob_id` can genuinely NAVIGATE from where it
    /// stands to `cell` (a bounded engine pathfinding probe with the mob's
    /// real body). Ask this before committing the mob to any PICKED
    /// walk-target cell — an unreachable goal walks the mob into the
    /// obstacle between them and parks it there (the pathfinder crowds
    /// partial routes on purpose, for chases). `false` = unreachable, no
    /// such live mob, or the mob is airborne (retry later).
    pub fn mob_can_reach(mob_id: u64, cell: [i32; 3]) -> bool
        => MobCanReach { mob_id, cell } => Bool
}

host_fn! {
    /// Damage a live mob (STABLE id) through the `mob_damage_pre` pipeline with
    /// the species' resolved `damage_feedback` (applied at the next in-tick
    /// drain point; a mob gone by then is a silent no-op). Mod damage is not an
    /// attack, so the default engine knockback is not applied; `origin` is
    /// spatial context for handlers/feedback.
    pub fn damage_mob(mob_id: u64, amount: f32, origin: Option<[f32; 3]>)
        => DamageMob { mob_id, amount, origin, feedback: None }
}

host_fn! {
    /// [`damage_mob`] with an explicitly composed damage pipeline for THIS
    /// request. Compose from [`crate::MobDamageFeedbackComponent`]; a pipeline
    /// without the `Immunity` component is damage-over-time (burn ticks):
    /// neither blocked by the victim's active i-frame window nor granting one.
    pub fn damage_mob_with_feedback(
        mob_id: u64,
        amount: f32,
        origin: Option<[f32; 3]>,
        feedback: crate::MobDamageFeedback,
    )
        => DamageMob { mob_id, amount, origin, feedback: Some(feedback) }
}

host_fn! {
    /// Toggle one KEYED particle-emitter bundle (a `particle_emitters.json` catalog
    /// row: particle rows + optional body tint; engine `petramond:*` and pack keys
    /// alike) on a live mob (STABLE id). Presentation-only, replicated, already-
    /// active sets survive death, not persisted — re-derive it from your own
    /// per-mob state. `false` = unknown/dead mob, unregistered key, or the mob's
    /// active set (4) is full.
    pub fn mob_emitter_set(mob_id: u64, key: &str, active: bool) -> bool
        => MobEmitterSet { mob_id, key: key.into(), active } => Bool
}

host_fn! {
    /// Fire a ONE-SHOT particle burst at `pos`: `key` names a
    /// `particle_emitters.json` BURST bundle (the core `petramond:water_splash`
    /// included). `intensity` scales the particle count through the bundle's
    /// `count_per_intensity`. Fire-and-forget presentation for every client, like
    /// `emit_sound`. `false` = unknown key or not a burst bundle.
    pub fn emitter_burst(key: &str, pos: [f32; 3], intensity: f32) -> bool
        => EmitterBurst { key: key.into(), pos, intensity } => Bool
}

host_fn! {
    /// Toggle a NAMED model animation on a live mob (STABLE id) — the animation
    /// sibling of [`mob_emitter_set`]: presentation-only, ≤ 4 active per mob,
    /// replicated, never persisted. Each active animation LAYERS over the
    /// walk/idle/rest base pose with its OWN phase (activation starts at phase
    /// 0, rate 1 — drive playback with [`mob_anim_rate`]); names the model
    /// doesn't have draw nothing. `false` = unknown mob or full active set.
    pub fn mob_anim_set(mob_id: u64, anim: &str, active: bool) -> bool
        => MobAnimSet { mob_id, anim: anim.into(), active } => Bool
}

host_fn! {
    /// Set an ACTIVE named animation's playback rate: `1.0` plays, `0.0` FREEZES
    /// mid-stroke exactly where it is (an oar pauses in place, never snaps
    /// home), negative reverses — code-driven playback over an authored clip.
    /// Cancels an in-flight [`mob_anim_seek`]. `false` = unknown mob or the anim
    /// is not active.
    pub fn mob_anim_rate(mob_id: u64, anim: &str, rate: f32) -> bool
        => MobAnimRate { mob_id, anim: anim.into(), rate } => Bool
}

host_fn! {
    /// SEEK an active named animation to the absolute `phase` at `|rate|`
    /// anim-seconds per second: the phase approaches the target DIRECTLY (no
    /// modulo — pick the nearest-cycle target yourself for a shortest-path
    /// return), lands on it EXACTLY, then holds at rate 0. How an oar settles
    /// gently back onto its authored pose from wherever the stroke stopped. A
    /// [`mob_anim_rate`] call cancels the seek. `false` = unknown mob or the
    /// anim is not active.
    pub fn mob_anim_seek(mob_id: u64, anim: &str, phase: f32, rate: f32) -> bool
        => MobAnimSeek { mob_id, anim: anim.into(), phase, rate } => Bool
}

host_fn! {
    /// Read the engine's authoritative playback state for an ACTIVE named
    /// animation. `None` = the mob is missing/dead or the animation is inactive.
    /// Use this phase when choosing absolute seek targets; do not mirror the
    /// engine's fixed-tick stepping in guest state.
    pub fn mob_anim_state(mob_id: u64, anim: &str) -> Option<MobAnimStateData>
        => MobAnimState { mob_id, anim: anim.into() } => MobAnimState
}

host_fn! {
    /// Drive a live mob (STABLE id) kinematically for THIS tick: `vel` is a
    /// horizontal world-space velocity `[x, z]` (m/s) replacing the brain's wish
    /// locomotion; `yaw`, when present, sets the absolute facing (mob convention —
    /// see [`mob_facing_xz`]). Vertical physics (gravity, water buoyancy) and
    /// collision stay engine-owned. An INTENT, not a state: re-issue every tick;
    /// friction and steering feel are your policy. `false` = unknown or dead mob.
    pub fn mob_drive(mob_id: u64, vel: [f32; 2], yaw: Option<f32>) -> bool
        => MobDrive { mob_id, vel, yaw } => Bool
}

host_fn! {
    /// Seat a player in `seat` of a live mob (STABLE id). The engine validates
    /// mechanism (live mob, declared free seat, unmounted player) and slaves the
    /// rider from this tick; WHO may sit WHERE is your policy — usually decided
    /// in an `interact_attempt` handler. Every detach path (your [`mob_dismount`],
    /// the engine's sneak gesture, death, despawn, leave) announces the
    /// `player_dismounted` event.
    pub fn mob_mount(mob_id: u64, player_id: PlayerId, seat: u8) -> bool
        => MobMount { mob_id, player_id, seat } => Bool
}

host_fn! {
    /// Pin a player in a named POSE at the world-space `anchor` (rider feet
    /// origin), body facing `yaw` (player convention: yaw `0` faces `+Z`) —
    /// the static-seat primitive behind chairs/benches/sofas. YOUR policy is
    /// where poses exist (your own seat layout) and who takes one; the engine
    /// owns mechanism: one pose per player, no two players on one exact
    /// anchor, replication and every release valve (sneak gesture, death,
    /// spectator, leave). Read occupancy back from the roster
    /// ([`crate::players`] → `pose_anchor`), never from mirrored mod state.
    /// Poses are transient and not tied to any block — release sitters
    /// yourself when your furniture breaks ([`mob_dismount`]). Pose
    /// vocabulary: [`mod_api::pose`].
    pub fn player_pose_set(player_id: PlayerId, anchor: [f32; 3], yaw: f32, pose: u8) -> bool
        => PlayerPoseSet { player_id, anchor, yaw, pose } => Bool
}

host_fn! {
    /// Unseat a player from whatever holds them — a mob seat or a pose
    /// anchor. `false` = they were not mounted or posed.
    pub fn mob_dismount(player_id: PlayerId) -> bool => MobDismount { player_id } => Bool
}

host_fn! {
    /// Read a live mob's declared seat capacity and current riders (in player-id
    /// order). `None` means the mob is missing/dead; a present zero capacity is a
    /// live non-rideable mob.
    pub fn mob_riders(mob_id: u64) -> Option<MobRidersData> => MobRiders { mob_id } => Riders
}

/// Map a point from a placed model group's unrotated FOOTPRINT space (origin
/// at the footprint min corner, the space `models.json` seats/geometry are
/// authored in) into world space — the exact transform the engine places
/// model geometry with, so a computed pose anchor lands on the authored seat
/// cushion under every facing. `base`/`facing` come from
/// [`block_model_group`]; `footprint` is your model's declared `cells`.
pub fn footprint_local_to_world(
    base: [i32; 3],
    footprint: [u8; 3],
    facing: Facing,
    local: [f32; 3],
) -> [f32; 3] {
    let (sx, sz) = (footprint[0] as f32, footprint[2] as f32);
    let [x, y, z] = local;
    let (rx, rz) = match facing {
        Facing::North => (x, z),
        Facing::South => (sx - x, sz - z),
        Facing::East => (sz - z, x),
        Facing::West => (z, sx - x),
    };
    [
        base[0] as f32 + rx,
        base[1] as f32 + y,
        base[2] as f32 + rz,
    ]
}

/// The PLAYER-convention body yaw (yaw `0` faces `+Z`) that faces the same
/// way as a placed model's `facing` — what a seat computed with
/// [`footprint_local_to_world`] passes to [`player_pose_set`].
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

    /// Pinned against the ENGINE's `placement_transform_fp` convention
    /// (North identity; South mirrors X and Z; East/West swap axes with one
    /// mirror). If this drifts, computed pose anchors leave the authored
    /// seat cushions on rotated placements.
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
    /// Remove a live mob (STABLE id) from the world immediately (no death, no
    /// loot, not saved). `false` = no such live mob.
    pub fn despawn_mob(mob_id: u64) -> bool => DespawnMob { mob_id } => Bool
}

host_fn! {
    /// Spawn `count` of an item (by registry NAME) as a dropped-item entity at
    /// `pos`. `false` = unknown name or zero count.
    pub fn spawn_item(item: &str, count: u8, pos: [f32; 3]) -> bool
        => SpawnItem { item: item.into(), count, pos } => Bool
}
