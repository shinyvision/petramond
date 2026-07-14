//! Sim-scoped entity calls: mob spawn/query/damage/despawn, keyed particle
//! emitters, and dropped-item spawns.

use mod_api::{HostCall, HostRet, MobSnapshot};

use crate::__rt;

/// Spawn a mob by species registry name at `pos` (feet) facing `yaw`. `false`
/// = unknown species or the mob cap is reached.
pub fn spawn_mob(key: &str, pos: [f32; 3], yaw: f32) -> bool {
    match __rt::host_call(&HostCall::SpawnMob {
        key: key.into(),
        pos,
        yaw,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SpawnMob returned {other:?}"),
    }
}

/// Snapshot the live mobs within `radius` of `pos` (3-D, feet positions), in
/// the deterministic live-set storage order. Indices are valid this tick only.
pub fn mobs_in_radius(pos: [f32; 3], radius: f32) -> Vec<MobSnapshot> {
    match __rt::host_call(&HostCall::MobsInRadius { pos, radius }) {
        HostRet::Mobs(mobs) => mobs,
        other => panic!("MobsInRadius returned {other:?}"),
    }
}

/// Damage a mob through its global engine-owned i-frames and the
/// `mob_damage_pre` pipeline (applied at the next in-tick drain point). Mod
/// damage is not an attack, so the default engine knockback is not applied;
/// `origin` is spatial context for handlers/feedback.
pub fn damage_mob(index: u32, amount: f32, origin: Option<[f32; 3]>) {
    __rt::expect_unit(
        "DamageMob",
        __rt::host_call(&HostCall::DamageMob {
            index,
            amount,
            origin,
        }),
    );
}

/// Toggle one KEYED particle-emitter bundle (a `particle_emitters.json` catalog
/// row: particle rows + optional body tint; engine `petramond:*` and pack keys
/// alike) on a live mob. Presentation-only, replicated, survives death, not
/// persisted — re-derive it from your own per-mob state. `false` = bad index,
/// unregistered key, or the mob's active set (4) is full.
pub fn mob_emitter_set(index: u32, key: &str, active: bool) -> bool {
    match __rt::host_call(&HostCall::MobEmitterSet {
        index,
        key: key.into(),
        active,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("MobEmitterSet returned {other:?}"),
    }
}

/// Fire a ONE-SHOT particle burst at `pos`: `key` names a
/// `particle_emitters.json` BURST bundle (the core `petramond:water_splash`
/// included). `intensity` scales the particle count through the bundle's
/// `count_per_intensity`. Fire-and-forget presentation for every client, like
/// `emit_sound`. `false` = unknown key or not a burst bundle.
pub fn emitter_burst(key: &str, pos: [f32; 3], intensity: f32) -> bool {
    match __rt::host_call(&HostCall::EmitterBurst {
        key: key.into(),
        pos,
        intensity,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("EmitterBurst returned {other:?}"),
    }
}

/// Remove a mob from the live world immediately (no death, no loot, not
/// saved). Renumbers later indices — re-query after use.
pub fn despawn_mob(index: u32) -> bool {
    match __rt::host_call(&HostCall::DespawnMob { index }) {
        HostRet::Bool(ok) => ok,
        other => panic!("DespawnMob returned {other:?}"),
    }
}

/// Spawn `count` of an item (registry key) as a dropped-item entity at `pos`.
/// `false` = unknown key or zero count.
pub fn spawn_item(item_key: &str, count: u8, pos: [f32; 3]) -> bool {
    match __rt::host_call(&HostCall::SpawnItem {
        item_key: item_key.into(),
        count,
        pos,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("SpawnItem returned {other:?}"),
    }
}
