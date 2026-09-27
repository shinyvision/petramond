#[allow(unused_imports)]
use mod_api::MobSnapshot;

use crate::__rt::host_fn;

host_fn! {
    pub fn emit_sound(key: &str, pos: Option<[f64; 3]>) -> bool
        => EmitSound { key: key.into(), pos } => Bool
}

host_fn! {
    pub fn sound_play_at(key: &str, pos: [f64; 3], volume: f32, pitch: f32) -> u64
        => SoundPlayAt { key: key.into(), pos, volume, pitch } => U64
}

host_fn! {
    pub fn sound_play_on_mob(mob_id: u64, key: &str, volume: f32, pitch: f32) -> u64
        => SoundPlayOnMob { mob_id, key: key.into(), volume, pitch } => U64
}

host_fn! {
    pub fn sound_stop(handle: u64) => SoundStop { handle }
}

host_fn! {
    pub fn sound_set(handle: u64, volume: f32, pitch: f32)
        => SoundSet { handle, volume, pitch }
}
