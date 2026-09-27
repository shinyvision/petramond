use mod_sdk::*;

pub fn kv_counter_bump(pos: [i32; 3], key: &str) -> u8 {
    section_kv_get(pos, key)
        .and_then(|b| b.first().copied())
        .unwrap_or(0)
        + 1
}
