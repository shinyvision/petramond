//! Sim-scoped entity calls: mob spawn/query/damage/despawn, keyed particle
//! emitters, riding, kinematic drive, named animations, and dropped-item
//! spawns.

use mod_api::{MobAnimStateData, MobRidersData, MobSnapshot};

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
    /// Spawn a mob by species registry name at `pos` (feet) facing `yaw`. `false`
    /// = unknown species or the mob cap is reached.
    pub fn spawn_mob(key: &str, pos: [f32; 3], yaw: f32) -> bool
        => SpawnMob { key: key.into(), pos, yaw } => Bool
}

host_fn! {
    /// Atomically spawn a mob only if its whole declared body fits loaded,
    /// stream-final world state at `pos`/`yaw`, including exact terrain collision
    /// shapes and other live solid mobs. `false` also covers unknown terrain or
    /// species and the mob cap.
    /// Use this for player-placed vehicles and other solid entities; a failed call
    /// mutates nothing, so the caller can safely retain or refund its item.
    pub fn spawn_mob_checked(key: &str, pos: [f32; 3], yaw: f32) -> bool
        => SpawnMobChecked { key: key.into(), pos, yaw } => Bool
}

host_fn! {
    /// Snapshot the live mobs within `radius` of `pos` (3-D, feet positions), in
    /// the deterministic live-set storage order. Indices are valid this tick only.
    pub fn mobs_in_radius(pos: [f32; 3], radius: f32) -> Vec<MobSnapshot>
        => MobsInRadius { pos, radius } => Mobs
}

host_fn! {
    /// Damage a mob through its global engine-owned i-frames and the
    /// `mob_damage_pre` pipeline (applied at the next in-tick drain point). Mod
    /// damage is not an attack, so the default engine knockback is not applied;
    /// `origin` is spatial context for handlers/feedback.
    pub fn damage_mob(index: u32, amount: f32, origin: Option<[f32; 3]>)
        => DamageMob { index, amount, origin }
}

host_fn! {
    /// Toggle one KEYED particle-emitter bundle (a `particle_emitters.json` catalog
    /// row: particle rows + optional body tint; engine `petramond:*` and pack keys
    /// alike) on a live mob. Presentation-only, replicated, survives death, not
    /// persisted — re-derive it from your own per-mob state. `false` = bad index,
    /// unregistered key, or the mob's active set (4) is full.
    pub fn mob_emitter_set(index: u32, key: &str, active: bool) -> bool
        => MobEmitterSet { index, key: key.into(), active } => Bool
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
    /// in a `mob_interact` handler. Every detach path (your [`mob_dismount`], the
    /// engine's sneak gesture, death, despawn, leave) announces the
    /// `player_dismounted` event.
    pub fn mob_mount(mob_id: u64, player_id: u8, seat: u8) -> bool
        => MobMount { mob_id, player_id, seat } => Bool
}

host_fn! {
    /// Unseat a player from whatever they ride. `false` = they were not mounted.
    pub fn mob_dismount(player_id: u8) -> bool => MobDismount { player_id } => Bool
}

host_fn! {
    /// Read a live mob's declared seat capacity and current riders (in player-id
    /// order). `None` means the mob is missing/dead; a present zero capacity is a
    /// live non-rideable mob.
    pub fn mob_riders(mob_id: u64) -> Option<MobRidersData> => MobRiders { mob_id } => Riders
}

host_fn! {
    /// Remove a mob from the live world immediately (no death, no loot, not
    /// saved). Renumbers later indices — re-query after use.
    pub fn despawn_mob(index: u32) -> bool => DespawnMob { index } => Bool
}

host_fn! {
    /// Spawn `count` of an item (registry key) as a dropped-item entity at `pos`.
    /// `false` = unknown key or zero count.
    pub fn spawn_item(item_key: &str, count: u8, pos: [f32; 3]) -> bool
        => SpawnItem { item_key: item_key.into(), count, pos } => Bool
}
