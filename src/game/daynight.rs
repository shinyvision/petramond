//! Core day/night cycle.
//!
//! This is intentionally built through the same tick-stage and shader-param
//! surfaces mods use. The `llama:*` keys are engine-owned public surface.

use crate::events::{Attach, Stage, TickSystems};
use crate::world::World;

/// Full day-night cycle length in game ticks (10 minutes at 20 TPS).
pub(crate) const CYCLE_TICKS: u64 = 12_000;
/// Clock offset of "early morning" within a day (fraction 0.05, just after
/// sunrise) — both the fresh-world start and where sleeping skips to.
const FRESH_CLOCK: u64 = 600;
const TRANSITION: f32 = 0.04;
const NIGHT_SKY_SCALE: f32 = 0.04;
const NIGHT_SKY_COLOR: [f32; 3] = [0.52, 0.62, 1.0];
const MOON_PHASES: u64 = 8;

pub(crate) const CLOCK_KEY: &str = "llama:clock";
pub(crate) const TIME_KEY: &str = "llama:time";
pub(crate) const NIGHT_KEY: &str = "llama:is_night";
pub(crate) const SKY_TIME_PARAM: &str = "llama:time";
pub(crate) const SKY_LIGHT_PARAM: &str = "llama:light";

pub(super) fn install_core(world: &mut World, systems: &mut TickSystems) {
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
    clock: u64,
    last_tick: u64,
    published_clock: Option<u64>,
    published_time: Option<[u8; 4]>,
}

impl DayNightCycle {
    fn from_world(world: &World) -> Self {
        Self {
            clock: read_clock(world)
                .or_else(|| read_time(world).map(|t| clock_from_fraction(t, FRESH_CLOCK)))
                .unwrap_or(FRESH_CLOCK),
            last_tick: world.current_tick(),
            published_clock: None,
            published_time: None,
        }
    }

    fn sync_external(&mut self, world: &World) {
        if let Some(clock) = read_clock(world) {
            if Some(clock) != self.published_clock {
                self.clock = clock;
                return;
            }
        }
        if let Some(raw) = world.mod_kv_get(TIME_KEY).and_then(read_time_bytes) {
            if Some(raw) != self.published_time {
                self.clock = clock_from_fraction(f32::from_le_bytes(raw), self.clock);
            }
        }
    }

    fn advance_to(&mut self, tick: u64) {
        self.clock = self
            .clock
            .saturating_add(tick.saturating_sub(self.last_tick));
        self.last_tick = tick;
    }

    fn publish(&mut self, world: &mut World) {
        let t = day_fraction(self.clock);
        let t_bytes = t.to_le_bytes();
        let day = daylight(t);
        let phase = ((self.clock / CYCLE_TICKS) % MOON_PHASES) as f32;
        let sky_scale = NIGHT_SKY_SCALE + (1.0 - NIGHT_SKY_SCALE) * day;
        let sky_color = [
            lerp(NIGHT_SKY_COLOR[0], 1.0, day),
            lerp(NIGHT_SKY_COLOR[1], 1.0, day),
            lerp(NIGHT_SKY_COLOR[2], 1.0, day),
        ];

        world.mod_kv_set(CLOCK_KEY.into(), self.clock.to_le_bytes().to_vec());
        world.mod_kv_set(TIME_KEY.into(), t_bytes.to_vec());
        world.mod_kv_set(NIGHT_KEY.into(), vec![u8::from(t >= 0.5)]);
        self.published_clock = Some(self.clock);
        self.published_time = Some(t_bytes);

        world.set_shader_param(SKY_TIME_PARAM.into(), [t, day, phase, 0.0]);
        world.set_shader_param(
            SKY_LIGHT_PARAM.into(),
            [sky_scale, sky_color[0], sky_color[1], sky_color[2]],
        );
    }
}

/// Whether it is night per the published `llama:is_night` KV (day fraction in
/// [0.5, 1.0) — sunset through sunrise). False on a world where the cycle has
/// not published yet.
pub(super) fn is_night(world: &World) -> bool {
    world.mod_kv_get(NIGHT_KEY).map(|b| b.first().copied()) == Some(Some(1))
}

/// Skip the clock to early morning of the NEXT day (sleeping through the
/// night — or the day). Written as a `llama:clock` KV like any external write;
/// the core cycle adopts it on its next tick (clock writes win exactly).
pub(super) fn skip_to_morning(world: &mut World) {
    let clock = read_clock(world).unwrap_or(FRESH_CLOCK);
    let next = morning_after(clock);
    world.mod_kv_set(CLOCK_KEY.into(), next.to_le_bytes().to_vec());
}

/// The first early-morning clock strictly after `clock`.
fn morning_after(clock: u64) -> u64 {
    (clock / CYCLE_TICKS + 1) * CYCLE_TICKS + FRESH_CLOCK
}

fn read_clock(world: &World) -> Option<u64> {
    let raw: [u8; 8] = world.mod_kv_get(CLOCK_KEY)?.try_into().ok()?;
    Some(u64::from_le_bytes(raw))
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

fn clock_from_fraction(t: f32, current_clock: u64) -> u64 {
    let day = current_clock / CYCLE_TICKS;
    let tick = (t.rem_euclid(1.0) * CYCLE_TICKS as f32).round() as u64 % CYCLE_TICKS;
    day * CYCLE_TICKS + tick
}

fn day_fraction(clock: u64) -> f32 {
    (clock % CYCLE_TICKS) as f32 / CYCLE_TICKS as f32
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

    #[test]
    fn sleeping_skips_to_the_next_early_morning() {
        // Mid-night (t = 0.75 of day 0) → morning of day 1; already-morning
        // still skips a whole day forward (strictly after).
        assert_eq!(morning_after(9_000), CYCLE_TICKS + FRESH_CLOCK);
        assert_eq!(morning_after(FRESH_CLOCK), CYCLE_TICKS + FRESH_CLOCK);
        // The target is always "early morning": same day fraction as fresh.
        assert!((day_fraction(morning_after(123_456)) - day_fraction(FRESH_CLOCK)).abs() < 1e-6);

        let mut world = World::new(1, 1);
        world.mod_kv_set(CLOCK_KEY.into(), 9_000u64.to_le_bytes().to_vec());
        skip_to_morning(&mut world);
        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&(CYCLE_TICKS + FRESH_CLOCK).to_le_bytes()[..]),
            "the skip writes the adopted llama:clock format"
        );
    }
    use crate::events::EventBus;
    use crate::game::TickEvents;
    use crate::mathh::Vec3;
    use crate::player::Player;

    fn published_time(world: &World) -> f32 {
        let bytes = world.mod_kv_get(TIME_KEY).expect("llama time");
        f32::from_le_bytes(bytes.try_into().expect("4-byte LE f32"))
    }

    #[test]
    fn core_daynight_restores_publishes_and_advances_on_tick_stage() {
        let mut world = World::new(1, 1);
        world.mod_kv_set(CLOCK_KEY.into(), 9000u64.to_le_bytes().to_vec());
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
        let mut feed = TickEvents::default();
        let mut bus = EventBus::default();
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut feed,
            bus.queue_mut(),
        );

        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&9001u64.to_le_bytes()[..]),
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
            &mut feed,
            bus.queue_mut(),
        );
        assert!(
            (published_time(&world) - (3001.0 / CYCLE_TICKS as f32)).abs() < 1e-6,
            "external llama:time write is adopted on the next core tick"
        );

        world.mod_kv_set(CLOCK_KEY.into(), 6000u64.to_le_bytes().to_vec());
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut player,
            &mut feed,
            bus.queue_mut(),
        );
        assert_eq!(
            world.mod_kv_get(CLOCK_KEY),
            Some(&6001u64.to_le_bytes()[..]),
            "external llama:clock write wins exactly"
        );
    }
}
