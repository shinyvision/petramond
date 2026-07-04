//! daynight — the day/night cycle proof-of-concept mod (WIKI/modding.md).
//!
//! One tick system (After(Spawning), the last seam of the tick) advances a
//! persisted day clock, publishes interop KV, and feeds render shader params
//! consumed by the pack's sky shader. Everything derives from
//! the persisted clock plus engine-tick deltas — no wall clock, no RNG, so
//! the cycle is deterministic and survives save/load (world KV is restored
//! before `mod_init`, so `init` picks the clock back up).
//!
//! # Published world-KV contract (other mods READ these; formats are frozen)
//!
//! - `daynight:time` — 4 bytes, little-endian IEEE-754 f32: the day fraction
//!   in `[0, 1)`. 0.0 = sunrise midpoint, 0.25 = noon, 0.5 = sunset midpoint,
//!   0.75 = midnight. One full cycle = 12000 ticks (10 min at 20 TPS).
//! - `daynight:is_night` — 1 byte: `1` exactly while the day fraction is in
//!   `[0.5, 1.0)` — night runs from mid-dusk (the sun's centre crossing the
//!   horizon) to mid-dawn — else `0`.
//! - `daynight:clock` — 8 bytes, little-endian u64: absolute elapsed cycle
//!   ticks (day count = clock / 12000). This is the mod's OWN persisted state;
//!   other mods should read the derived keys above, not this one.

use mod_sdk::*;

const CLOCK_SYSTEM: u32 = 1;

/// Full day-night cycle length in game ticks (10 minutes at 20 TPS).
const CYCLE_TICKS: u64 = 12_000;
/// A fresh world starts here: early morning (day fraction 0.05), well clear
/// of the dawn transition, so new worlds open in full daylight.
const FRESH_CLOCK: u64 = 600;
/// Width of each dawn/dusk transition window, in day fraction (~24 s).
const TRANSITION: f32 = 0.04;
/// Sky-light floor at deep night (1.0 = the engine's noon rendering). The
/// terrain/entity shaders consume this through the active sky shader's
/// `sky_light_param`, not through direct environment host calls.
const NIGHT_SKY_SCALE: f32 = 0.04;
/// Deep-night sky LIGHT colour. Kept subtle: blue stays at identity while red
/// and green ease down a little, then the daylight curve lerps back to white.
const NIGHT_SKY_COLOR: [f32; 3] = [0.52, 0.62, 1.0];
/// The shader still receives a phase number so a future textured/procedural
/// moon can preserve the existing once-per-day cadence.
const MOON_PHASES: u64 = 8;

const CLOCK_KEY: &str = "daynight:clock";
const TIME_KEY: &str = "daynight:time";
const NIGHT_KEY: &str = "daynight:is_night";
const SKY_TIME_PARAM: &str = "daynight:time";
const SKY_LIGHT_PARAM: &str = "daynight:light";

#[derive(Default)]
struct DayNight {
    /// Absolute elapsed cycle ticks — the ONE piece of persisted state.
    clock: u64,
    /// Engine tick at the last update; the clock advances by engine-tick
    /// deltas so it is robust to whatever the session's tick counter starts at.
    last_tick: u64,
}

impl Mod for DayNight {
    fn init(&mut self) {
        register_tick_system(Stage::Spawning, AttachSide::After, 0, CLOCK_SYSTEM);
        // Sim-scoped calls are safe in init here: this mod registers no
        // worldgen hooks, so no detached per-thread instance ever runs it.
        self.clock = match world_kv_get(CLOCK_KEY) {
            Some(bytes) => match bytes.try_into() {
                Ok(raw) => u64::from_le_bytes(raw),
                Err(_) => FRESH_CLOCK, // malformed record: start a fresh day
            },
            None => FRESH_CLOCK,
        };
        self.last_tick = current_tick();
        // Publish immediately so a loaded world never flashes noon before the
        // first tick.
        self.publish();
        log(&format!("initialized at clock {}", self.clock));
    }

    fn tick_system(&mut self, _system_id: u32) {
        let now = current_tick();
        self.clock += now.saturating_sub(self.last_tick);
        self.last_tick = now;
        self.publish();
    }
}

impl DayNight {
    /// Derive everything from the clock and push it out: the interop KV keys
    /// and the visual shader params. Runs every tick so shader-driven sky,
    /// sunlight, and celestial motion stay in lockstep with gameplay time.
    fn publish(&self) {
        let t = (self.clock % CYCLE_TICKS) as f32 / CYCLE_TICKS as f32;
        let day = daylight(t);
        let phase = ((self.clock / CYCLE_TICKS) % MOON_PHASES) as f32;
        let sky_scale = NIGHT_SKY_SCALE + (1.0 - NIGHT_SKY_SCALE) * day;
        let sky_color = [
            lerp(NIGHT_SKY_COLOR[0], 1.0, day),
            lerp(NIGHT_SKY_COLOR[1], 1.0, day),
            lerp(NIGHT_SKY_COLOR[2], 1.0, day),
        ];

        world_kv_set(CLOCK_KEY, self.clock.to_le_bytes().to_vec());
        world_kv_set(TIME_KEY, t.to_le_bytes().to_vec());
        world_kv_set(NIGHT_KEY, vec![u8::from(t >= 0.5)]);

        shader_set_param(SKY_TIME_PARAM, [t, day, phase, 0.0]);
        shader_set_param(
            SKY_LIGHT_PARAM,
            [sky_scale, sky_color[0], sky_color[1], sky_color[2]],
        );
    }
}

/// Daylight factor: 1.0 in full day (fraction 0..0.5), 0.0 in deep night,
/// smoothstepped through the dawn (centred on 0.0) and dusk (centred on 0.5)
/// windows. Formulated on the sun's elevation `sin(2πt)` so both windows and
/// the midnight wrap fall out of one expression: the elevation threshold
/// `sin(π·TRANSITION)` makes each window ~TRANSITION wide in day fraction.
fn daylight(t: f32) -> f32 {
    let h = (core::f32::consts::PI * TRANSITION).sin();
    smoothstep(-h, h, (core::f32::consts::TAU * t).sin())
}

/// The classic Hermite smoothstep: 0 at `e0`, 1 at `e1`, smooth in between.
fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

register_mod!(DayNight);
