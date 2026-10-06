//! The day/night clock, as the pack reads it.

use mod_sdk::{world_kv_get, ByteReader};

use crate::daylight::daylight;

const TIME_KEY: &str = "petramond:time";

/// How much daylight there is now: 0 at night, 1 by day. `None` without a readable clock.
pub fn daylight_now() -> Option<f32> {
    let bytes = world_kv_get(TIME_KEY)?;
    let t = ByteReader::new(&bytes).f32()?;
    if !t.is_finite() || !(0.0..=1.0).contains(&t) {
        return None;
    }
    Some(daylight(t.rem_euclid(1.0)))
}
