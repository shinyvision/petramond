//! Day/night cycle arithmetic and the engine-owned sky surface.
//!
//! The server's cycle system (`server::daynight`) advances the clock and
//! publishes it; anything that draws a sky from a clock — the client scene, an
//! offscreen capture — derives the same shader params from the same fraction
//! through [`sky_params`]. The `petramond:*` keys are engine-owned public
//! surface shared with mods.

/// Full day-night cycle ticks for the DEFAULT day length (15-minute day +
/// 15-minute night at 20 TPS). The actual cycle is per-world: see
/// [`cycle_ticks_for_day_minutes`] and `World::day_cycle_ticks`.
pub const DEFAULT_CYCLE_TICKS: u64 =
    cycle_ticks_for_day_minutes(crate::save::settings::DEFAULT_DAY_MINUTES);

/// Moon phases per lunar cycle (one phase per day).
pub const MOON_PHASES: u64 = 8;

pub const CLOCK_KEY: &str = "petramond:clock";
pub const TIME_KEY: &str = "petramond:time";
pub const NIGHT_KEY: &str = "petramond:is_night";
pub const FROZEN_KEY: &str = "petramond:time_frozen";
pub const SKY_TIME_PARAM: &str = "petramond:time";
pub const SKY_LIGHT_PARAM: &str = "petramond:light";

const TRANSITION: f32 = 0.04;
const NIGHT_SKY_SCALE: f32 = 0.04;
const NIGHT_SKY_COLOR: [f32; 3] = [0.52, 0.62, 1.0];

/// The world's full cycle ticks for a "day length" setting in real minutes:
/// the night lasts as long as the day, so a 15-minute day is 18 000 day ticks
/// + 18 000 night ticks at 20 TPS. Clamps to the slider range (10..=30 min).
pub const fn cycle_ticks_for_day_minutes(minutes: u32) -> u64 {
    let m = if minutes < 10 {
        10
    } else if minutes > 30 {
        30
    } else {
        minutes
    };
    m as u64 * 60 * 20 * 2
}

/// Clock offset of "early morning" within a day (fraction 0.05, just after
/// sunrise) — both the fresh-world start and where sleeping skips to.
pub const fn fresh_clock(cycle: u64) -> u64 {
    cycle / 20
}

/// The first early-morning clock strictly after `clock`.
pub fn morning_after(clock: u64, cycle: u64) -> u64 {
    (clock / cycle + 1) * cycle + fresh_clock(cycle)
}

/// The clock at day fraction `t` within the day `current_clock` falls in.
pub fn clock_from_fraction(t: f32, current_clock: u64, cycle: u64) -> u64 {
    let day = current_clock / cycle;
    let tick = (t.rem_euclid(1.0) * cycle as f32).round() as u64 % cycle;
    day * cycle + tick
}

/// Position within the current day, `0..1` (`0.25` = noon, `0.75` = midnight).
pub fn day_fraction(clock: u64, cycle: u64) -> f32 {
    (clock % cycle) as f32 / cycle as f32
}

/// The moon phase (`0..MOON_PHASES`) the clock's day shows.
pub fn moon_phase(clock: u64, cycle: u64) -> f32 {
    ((clock / cycle) % MOON_PHASES) as f32
}

/// The two sky shader params for a point in the cycle: `petramond:time`
/// (`[fraction, daylight, moon phase, 0]`) and `petramond:light`
/// (`[sky scale, r, g, b]`). Anything that drives the sky from a clock — the
/// live cycle, an offscreen capture — gets the same sky for the same fraction.
pub fn sky_params(day_fraction: f32, moon_phase: f32) -> ([f32; 4], [f32; 4]) {
    let t = day_fraction.rem_euclid(1.0);
    let day = daylight(t);
    let scale = NIGHT_SKY_SCALE + (1.0 - NIGHT_SKY_SCALE) * day;
    (
        [t, day, moon_phase, 0.0],
        [
            scale,
            lerp(NIGHT_SKY_COLOR[0], 1.0, day),
            lerp(NIGHT_SKY_COLOR[1], 1.0, day),
            lerp(NIGHT_SKY_COLOR[2], 1.0, day),
        ],
    )
}

fn daylight(t: f32) -> f32 {
    let h = (std::f32::consts::PI * TRANSITION).sin();
    smoothstep(-h, h, (std::f32::consts::TAU * t).sin())
}

fn smoothstep(e0: f32, e1: f32, x: f32) -> f32 {
    let t = ((x - e0) / (e1 - e0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

fn lerp(a: f32, b: f32, t: f32) -> f32 {
    a + (b - a) * t
}

/// The monsters mod's copy of [`daylight`] (its sunburn and spawn-light
/// rules read only the published day fraction), compiled here verbatim so a
/// retune of the curve fails this crate's tests and names the mirror.
#[cfg(test)]
#[path = "../../mods-src/monsters/src/daylight.rs"]
mod monsters_daylight;

#[cfg(test)]
mod tests {
    use super::*;

    const C: u64 = DEFAULT_CYCLE_TICKS;

    #[test]
    fn day_minutes_map_to_cycle_ticks_and_clamp() {
        // The spec point: a 15-minute day is 18 000 day ticks (36 000 cycle).
        assert_eq!(cycle_ticks_for_day_minutes(15), 36_000);
        assert_eq!(C, 36_000, "default day length is 15 minutes");
        assert_eq!(cycle_ticks_for_day_minutes(10), 24_000);
        assert_eq!(cycle_ticks_for_day_minutes(30), 72_000);
        assert_eq!(cycle_ticks_for_day_minutes(5), 24_000, "clamped low");
        assert_eq!(cycle_ticks_for_day_minutes(99), 72_000, "clamped high");
        // "Early morning" stays the same fraction at every length.
        assert_eq!(fresh_clock(C) as f32 / C as f32, 0.05);
    }

    #[test]
    fn morning_after_is_strictly_the_next_early_morning() {
        // Mid-night (t = 0.75 of day 0) → morning of day 1; already-morning
        // still skips a whole day forward (strictly after).
        assert_eq!(morning_after(C * 3 / 4, C), C + fresh_clock(C));
        assert_eq!(morning_after(fresh_clock(C), C), C + fresh_clock(C));
        // The target is always "early morning": same day fraction as fresh.
        assert!(
            (day_fraction(morning_after(123_456, C), C) - day_fraction(fresh_clock(C), C)).abs()
                < 1e-6
        );
    }

    #[test]
    fn fraction_round_trips_within_the_current_day() {
        let clock = 3 * C + 100;
        assert_eq!(clock_from_fraction(0.25, clock, C), 3 * C + C / 4);
        assert_eq!(
            clock_from_fraction(day_fraction(clock, C), clock, C),
            clock,
            "a fraction read off a clock maps back onto it"
        );
        assert_eq!(moon_phase(clock, C), 3.0);
        assert_eq!(moon_phase(9 * C, C), 1.0, "phases wrap every eight days");
    }

    #[test]
    fn sky_is_bright_at_noon_and_blue_dark_at_midnight() {
        let (noon_time, noon_light) = sky_params(0.25, 0.0);
        assert_eq!(noon_time[0], 0.25);
        assert!(noon_time[1] > 0.99, "full daylight at noon: {noon_time:?}");
        assert!(noon_light[0] > 0.99, "noon sky at full scale: {noon_light:?}");

        let (midnight_time, light) = sky_params(1.75, 4.0);
        assert!(
            (midnight_time[0] - 0.75).abs() < 1e-6,
            "the fraction wraps into 0..1"
        );
        assert_eq!(midnight_time[2], 4.0, "the moon phase rides through");
        assert!(light[0] < 0.5, "midnight sky light is dark: {light:?}");
        assert!(
            light[1] < light[3] && light[2] < light[3],
            "midnight sky light keeps the blue-dominant tint: {light:?}"
        );
    }

    #[test]
    fn the_monsters_mod_mirrors_the_daylight_curve() {
        for i in 0..=2000 {
            let t = i as f32 / 2000.0;
            assert_eq!(
                super::monsters_daylight::daylight(t),
                daylight(t),
                "mods-src/monsters/src/daylight.rs disagrees with the sky at t = {t}"
            );
        }
    }
}
