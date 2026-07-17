//! Core day/night cycle.
//!
//! This is intentionally built through the same tick-stage and shader-param
//! surfaces mods use. The `petramond:*` keys are engine-owned public surface.

use crate::events::{Attach, Stage, TickSystems};
use crate::world::World;

/// Full day-night cycle ticks for the DEFAULT day length (15-minute day +
/// 15-minute night at 20 TPS). The actual cycle is per-world: see
/// [`cycle_ticks_for_day_minutes`] and `World::day_cycle_ticks`.
pub(crate) const DEFAULT_CYCLE_TICKS: u64 =
    cycle_ticks_for_day_minutes(crate::save::settings::DEFAULT_DAY_MINUTES);

/// The world's full cycle ticks for a "day length" setting in real minutes:
/// the night lasts as long as the day, so a 15-minute day is 18 000 day ticks
/// + 18 000 night ticks at 20 TPS. Clamps to the slider range (10..=30 min).
pub(crate) const fn cycle_ticks_for_day_minutes(minutes: u32) -> u64 {
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
const fn fresh_clock(cycle: u64) -> u64 {
    cycle / 20
}
const TRANSITION: f32 = 0.04;
const NIGHT_SKY_SCALE: f32 = 0.04;
const NIGHT_SKY_COLOR: [f32; 3] = [0.52, 0.62, 1.0];
const MOON_PHASES: u64 = 8;

pub(crate) const CLOCK_KEY: &str = "petramond:clock";
pub(crate) const TIME_KEY: &str = "petramond:time";
pub(crate) const NIGHT_KEY: &str = "petramond:is_night";
pub(crate) const FROZEN_KEY: &str = "petramond:time_frozen";
pub(crate) const SKY_TIME_PARAM: &str = "petramond:time";
pub(crate) const SKY_LIGHT_PARAM: &str = "petramond:light";

pub(crate) fn install_core(world: &mut World, systems: &mut TickSystems) {
    let mut cycle = DayNightCycle::from_world(world);
    cycle.publish(world);

    systems.attach(Attach::After(Stage::Spawning), 0, move |ctx| {
        cycle.sync_external(ctx.world);
        cycle.advance_to(ctx.world.current_tick());
        cycle.publish(ctx.world);
    });
}

#[derive(Debug)]
struct DayNightCycle {
    /// The world's full day+night cycle length in ticks (per-world setting,
    /// fixed for the session — set before core systems install).
    cycle: u64,
    clock: u64,
    last_tick: u64,
    frozen: bool,
    published_clock: Option<u64>,
    published_time: Option<[u8; 4]>,
}

impl DayNightCycle {
    fn from_world(world: &World) -> Self {
        let cycle = world.day_cycle_ticks();
        Self {
            cycle,
            clock: read_clock(world)
                .or_else(|| {
                    read_time(world).map(|t| clock_from_fraction(t, fresh_clock(cycle), cycle))
                })
                .unwrap_or(fresh_clock(cycle)),
            last_tick: world.current_tick(),
            frozen: read_frozen(world),
            published_clock: None,
            published_time: None,
        }
    }

    fn sync_external(&mut self, world: &World) {
        self.frozen = read_frozen(world);
        if let Some(clock) = read_clock(world) {
            if Some(clock) != self.published_clock {
                self.clock = clock;
                return;
            }
        }
        if let Some(raw) = world.mod_kv_get(TIME_KEY).and_then(read_time_bytes) {
            if Some(raw) != self.published_time {
                self.clock = clock_from_fraction(f32::from_le_bytes(raw), self.clock, self.cycle);
            }
        }
    }

    fn advance_to(&mut self, tick: u64) {
        if !self.frozen {
            self.clock = self
                .clock
                .saturating_add(tick.saturating_sub(self.last_tick));
        }
        self.last_tick = tick;
    }

    fn publish(&mut self, world: &mut World) {
        let t = day_fraction(self.clock, self.cycle);
        let t_bytes = t.to_le_bytes();
        let day = daylight(t);
        let phase = ((self.clock / self.cycle) % MOON_PHASES) as f32;
        let sky_scale = NIGHT_SKY_SCALE + (1.0 - NIGHT_SKY_SCALE) * day;
        let sky_color = [
            lerp(NIGHT_SKY_COLOR[0], 1.0, day),
            lerp(NIGHT_SKY_COLOR[1], 1.0, day),
            lerp(NIGHT_SKY_COLOR[2], 1.0, day),
        ];

        world.mod_kv_set(CLOCK_KEY.into(), self.clock.to_le_bytes().to_vec());
        world.mod_kv_set(TIME_KEY.into(), t_bytes.to_vec());
        world.mod_kv_set(NIGHT_KEY.into(), vec![u8::from(t >= 0.5)]);
        world.mod_kv_set(FROZEN_KEY.into(), vec![u8::from(self.frozen)]);
        self.published_clock = Some(self.clock);
        self.published_time = Some(t_bytes);

        world.set_shader_param(SKY_TIME_PARAM.into(), [t, day, phase, 0.0]);
        world.set_shader_param(
            SKY_LIGHT_PARAM.into(),
            [sky_scale, sky_color[0], sky_color[1], sky_color[2]],
        );
    }
}

/// Whether it is night per the published `petramond:is_night` KV (day fraction in
/// [0.5, 1.0) — sunset through sunrise). False on a world where the cycle has
/// not published yet.
pub(super) fn is_night(world: &World) -> bool {
    world.mod_kv_get(NIGHT_KEY).map(|b| b.first().copied()) == Some(Some(1))
}

/// The published day clock (`petramond:clock`), or 0 on a world whose cycle has
/// not published yet — stamped on every `TickUpdate` so a client's sky follows
/// the server's.
pub(super) fn current_clock(world: &World) -> u64 {
    read_clock(world).unwrap_or(0)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(crate) enum TimePreset {
    Day,
    Noon,
    Night,
    Midnight,
}

/// Set the named point within the current absolute day. The core cycle adopts
/// this ordinary clock write on its next deterministic tick.
pub(crate) fn set_time(world: &mut World, preset: TimePreset) {
    let cycle = world.day_cycle_ticks();
    let within_day = match preset {
        TimePreset::Day => fresh_clock(cycle),
        TimePreset::Noon => cycle / 4,
        TimePreset::Night => cycle / 2,
        TimePreset::Midnight => cycle * 3 / 4,
    };
    let current = read_clock(world).unwrap_or(fresh_clock(cycle));
    let clock = current / cycle * cycle + within_day;
    world.mod_kv_set(CLOCK_KEY.into(), clock.to_le_bytes().to_vec());
}

/// Freeze/unfreeze the deterministic cycle at its current clock. The flag is
/// world KV, so save-all/autosave and reload preserve it.
pub(crate) fn set_frozen(world: &mut World, frozen: bool) {
    world.mod_kv_set(FROZEN_KEY.into(), vec![u8::from(frozen)]);
}

/// Skip the clock to early morning of the NEXT day (sleeping through the
/// night — or the day). Written as a `petramond:clock` KV like any external write;
/// the core cycle adopts it on its next tick (clock writes win exactly).
pub(super) fn skip_to_morning(world: &mut World) {
    let cycle = world.day_cycle_ticks();
    let clock = read_clock(world).unwrap_or(fresh_clock(cycle));
    let next = morning_after(clock, cycle);
    world.mod_kv_set(CLOCK_KEY.into(), next.to_le_bytes().to_vec());
}

/// The first early-morning clock strictly after `clock`.
fn morning_after(clock: u64, cycle: u64) -> u64 {
    (clock / cycle + 1) * cycle + fresh_clock(cycle)
}

fn read_clock(world: &World) -> Option<u64> {
    let raw: [u8; 8] = world.mod_kv_get(CLOCK_KEY)?.try_into().ok()?;
    Some(u64::from_le_bytes(raw))
}

fn read_frozen(world: &World) -> bool {
    world
        .mod_kv_get(FROZEN_KEY)
        .and_then(|b| b.first())
        .copied()
        == Some(1)
}

fn read_time(world: &World) -> Option<f32> {
    world
        .mod_kv_get(TIME_KEY)
        .and_then(read_time_bytes)
        .map(f32::from_le_bytes)
        .filter(|t| t.is_finite())
}

fn read_time_bytes(bytes: &[u8]) -> Option<[u8; 4]> {
    let raw: [u8; 4] = bytes.try_into().ok()?;
    f32::from_le_bytes(raw).is_finite().then_some(raw)
}

fn clock_from_fraction(t: f32, current_clock: u64, cycle: u64) -> u64 {
    let day = current_clock / cycle;
    let tick = (t.rem_euclid(1.0) * cycle as f32).round() as u64 % cycle;
    day * cycle + tick
}

fn day_fraction(clock: u64, cycle: u64) -> f32 {
    (clock % cycle) as f32 / cycle as f32
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::crafting::Recipes;

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
    fn sleeping_skips_to_the_next_early_morning() {
        // Mid-night (t = 0.75 of day 0) → morning of day 1; already-morning
        // still skips a whole day forward (strictly after).
        assert_eq!(morning_after(C * 3 / 4, C), C + fresh_clock(C));
        assert_eq!(morning_after(fresh_clock(C), C), C + fresh_clock(C));
        // The target is always "early morning": same day fraction as fresh.
        assert!(
            (day_fraction(morning_after(123_456, C), C) - day_fraction(fresh_clock(C), C)).abs()
                < 1e-6
        );

        let mut world = World::new(1, 1);
        world.mod_kv_set(CLOCK_KEY.into(), (C * 3 / 4).to_le_bytes().to_vec());
        skip_to_morning(&mut world);
        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&(C + fresh_clock(C)).to_le_bytes()[..]),
            "the skip writes the adopted petramond:clock format"
        );

        // A shorter per-world day skips by ITS cycle, not the default.
        let mut world = World::new(1, 1);
        world.set_day_cycle_ticks(cycle_ticks_for_day_minutes(10));
        let c10 = world.day_cycle_ticks();
        world.mod_kv_set(CLOCK_KEY.into(), (c10 * 3 / 4).to_le_bytes().to_vec());
        skip_to_morning(&mut world);
        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&(c10 + fresh_clock(c10)).to_le_bytes()[..])
        );
    }

    use crate::events::EventBus;
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::player::Player;

    fn published_time(world: &World) -> f32 {
        let bytes = world.mod_kv_get(TIME_KEY).expect("petramond time");
        f32::from_le_bytes(bytes.try_into().expect("4-byte LE f32"))
    }

    #[test]
    fn core_daynight_restores_publishes_and_advances_on_tick_stage() {
        let mut world = World::new(1, 1);
        world.mod_kv_set(CLOCK_KEY.into(), (C * 3 / 4).to_le_bytes().to_vec());
        let mut systems = TickSystems::default();

        install_core(&mut world, &mut systems);

        let t0 = published_time(&world);
        assert!(
            (t0 - 0.75).abs() < 1e-6,
            "restored clock publishes midnight fraction, got {t0}"
        );
        assert_eq!(world.mod_kv_get(NIGHT_KEY), Some(&[1u8][..]));

        let params = world.environment().shader_params();
        let light = params.get(SKY_LIGHT_PARAM).expect("sky light param");
        assert!(light[0] < 0.5, "midnight sky light is dark: {light:?}");
        assert!(
            light[1] < light[3] && light[2] < light[3],
            "midnight sky light keeps the blue-dominant tint: {light:?}"
        );
        assert_eq!(params.get(SKY_TIME_PARAM).expect("sky time param")[0], t0);

        world.game_tick(&Recipes::default());
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut gui = crate::gui::empty_gui_state();
        let mut feed = TickEvents::default();
        let mut bus = EventBus::default();
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut gui,
            &mut feed,
            bus.queue_mut(),
        );

        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&(C * 3 / 4 + 1).to_le_bytes()[..]),
            "clock advances by the elapsed engine tick"
        );
        assert!(
            published_time(&world) > t0,
            "published time advances with the clock"
        );

        world.mod_kv_set(TIME_KEY.into(), 0.25f32.to_le_bytes().to_vec());
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut gui,
            &mut feed,
            bus.queue_mut(),
        );
        assert!(
            (published_time(&world) - ((C / 4 + 1) as f32 / C as f32)).abs() < 1e-6,
            "external petramond:time write is adopted on the next core tick"
        );

        world.mod_kv_set(CLOCK_KEY.into(), (C / 2).to_le_bytes().to_vec());
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut gui,
            &mut feed,
            bus.queue_mut(),
        );
        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&(C / 2 + 1).to_le_bytes()[..]),
            "external petramond:clock write wins exactly"
        );
    }

    #[test]
    fn named_times_and_frozen_cycle_are_deterministic() {
        let mut world = World::new(1, 1);
        world.mod_kv_set(CLOCK_KEY.into(), (C + 123).to_le_bytes().to_vec());

        set_time(&mut world, TimePreset::Day);
        assert_eq!(read_clock(&world), Some(C + fresh_clock(C)));
        set_time(&mut world, TimePreset::Noon);
        assert_eq!(read_clock(&world), Some(C + C / 4));
        set_time(&mut world, TimePreset::Night);
        assert_eq!(read_clock(&world), Some(C + C / 2));
        set_time(&mut world, TimePreset::Midnight);
        assert_eq!(read_clock(&world), Some(C + C * 3 / 4));

        set_frozen(&mut world, true);
        let frozen_at = read_clock(&world).unwrap();
        let mut systems = TickSystems::default();
        install_core(&mut world, &mut systems);
        let mut player = Player::new(Vec3::new(0.0, 80.0, 0.0));
        let mut gui = crate::gui::empty_gui_state();
        let mut feed = TickEvents::default();
        let mut bus = EventBus::default();
        for _ in 0..3 {
            world.game_tick(&Recipes::default());
            systems.run(
                Attach::After(Stage::Spawning),
                &mut world,
                &mut player,
                &mut gui,
                &mut feed,
                bus.queue_mut(),
            );
        }
        assert_eq!(read_clock(&world), Some(frozen_at));
        assert_eq!(world.mod_kv_get(FROZEN_KEY), Some(&[1][..]));

        set_frozen(&mut world, false);
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut gui,
            &mut feed,
            bus.queue_mut(),
        );
        assert_eq!(
            read_clock(&world),
            Some(frozen_at + 1),
            "unfreeze resumes without replaying frozen ticks"
        );
    }
}
