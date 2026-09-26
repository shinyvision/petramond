//! The day/night daylight curve, mirrored from core day/night.
//!
//! The engine publishes only the day FRACTION to mods (`petramond:time`), and
//! the sunburn and spawn-light rules need the daylight factor the sky itself
//! is lit by. This file is the mod's copy of that curve — and it is
//! dependency-free on purpose: the engine's day/night tests compile it
//! verbatim and assert it against the engine's own curve across the whole
//! cycle, so an engine retune fails an engine test that names this mod
//! instead of making zombies burn at a different dusk than the sky shows.

/// Half-width of the dawn/dusk ramp, as a fraction of the cycle.
const TRANSITION: f32 = 0.04;

/// Daylight factor in [0, 1] for a day fraction `t` in [0, 1): 1 through the
/// day, 0 through the night, smoothstepped across each horizon crossing.
pub fn daylight(t: f32) -> f32 {
    let h = (core::f32::consts::PI * TRANSITION).sin();
    smoothstep(-h, h, (core::f32::consts::TAU * t).sin())
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}
