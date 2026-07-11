//! Sound playback routed through the tick→presentation channel: one-shots
//! and handle-addressed spatial sounds.

// `MobSnapshot` is imported for intra-doc links only.
#[allow(unused_imports)]
use mod_api::{HostCall, HostRet, MobSnapshot};

use crate::__rt;

/// Play a sound by `sounds.json` key. `pos` attenuates by the sound row's
/// `attenuation_distance`; `None` plays at full volume. `false` = unknown key.
pub fn emit_sound(key: &str, pos: Option<[f32; 3]>) -> bool {
    match __rt::host_call(&HostCall::EmitSound {
        key: key.into(),
        pos,
    }) {
        HostRet::Bool(ok) => ok,
        other => panic!("EmitSound returned {other:?}"),
    }
}

/// Start a spatial sound at a fixed world position. Returns a deterministic
/// session handle, or `0` if the key/parameters were rejected. Travel distance
/// comes from the sound row's `attenuation_distance`.
pub fn sound_play_at(key: &str, pos: [f32; 3], volume: f32, pitch: f32) -> u64 {
    match __rt::host_call(&HostCall::SoundPlayAt {
        key: key.into(),
        pos,
        volume,
        pitch,
    }) {
        HostRet::U64(handle) => handle,
        other => panic!("SoundPlayAt returned {other:?}"),
    }
}

/// Start a spatial sound pinned to a stable mob id from [`MobSnapshot::id`].
/// If that mob despawns, the engine lets the sound finish at its last known
/// position. Returns `0` if the key/mob/parameters were rejected. Travel
/// distance comes from the sound row's `attenuation_distance`.
pub fn sound_play_on_mob(mob_id: u64, key: &str, volume: f32, pitch: f32) -> u64 {
    match __rt::host_call(&HostCall::SoundPlayOnMob {
        mob_id,
        key: key.into(),
        volume,
        pitch,
    }) {
        HostRet::U64(handle) => handle,
        other => panic!("SoundPlayOnMob returned {other:?}"),
    }
}

/// Stop a spatial sound handle. Unknown handles are ignored.
pub fn sound_stop(handle: u64) {
    __rt::expect_unit(
        "SoundStop",
        __rt::host_call(&HostCall::SoundStop { handle }),
    );
}
