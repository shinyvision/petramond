//! Core day/night cycle system.
//!
//! This is intentionally built through the same tick-stage and shader-param
//! surfaces mods use. The cycle arithmetic, the sky derivation and the
//! engine-owned `petramond:*` keys live in `crate::rules::daynight`, shared
//! with the client.

use crate::events::{Attach, Stage, TickSystems};
use crate::rules::daynight::{
    clock_from_fraction, day_fraction, fresh_clock, moon_phase, morning_after, sky_params,
    CLOCK_KEY, FROZEN_KEY, NIGHT_KEY, SKY_LIGHT_PARAM, SKY_TIME_PARAM, TIME_KEY,
};
use crate::world::ServerWorld;

pub fn install_core(world: &mut ServerWorld, systems: &mut TickSystems) {
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
    fn from_world(world: &ServerWorld) -> Self {
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

    fn sync_external(&mut self, world: &ServerWorld) {
        self.frozen = read_frozen(world);
        if let Some(clock) = read_clock(world) {
            if Some(clock) != self.published_clock {
                self.clock = clock;
                return;
            }
        }
        if let Some(raw) = world
            .data()
            .world_kv_get(TIME_KEY)
            .and_then(read_time_bytes)
        {
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

    fn publish(&mut self, world: &mut ServerWorld) {
        let t = day_fraction(self.clock, self.cycle);
        let t_bytes = t.to_le_bytes();
        let phase = moon_phase(self.clock, self.cycle);
        let (time_param, light_param) = sky_params(t, phase);

        world.world_kv_set(CLOCK_KEY.into(), self.clock.to_le_bytes().to_vec());
        world.world_kv_set(TIME_KEY.into(), t_bytes.to_vec());
        world.world_kv_set(NIGHT_KEY.into(), vec![u8::from(t >= 0.5)]);
        world.world_kv_set(FROZEN_KEY.into(), vec![u8::from(self.frozen)]);
        self.published_clock = Some(self.clock);
        self.published_time = Some(t_bytes);

        world.set_shader_param(SKY_TIME_PARAM.into(), time_param);
        world.set_shader_param(SKY_LIGHT_PARAM.into(), light_param);
    }
}

/// Whether it is night per the published `petramond:is_night` KV (day fraction in
/// [0.5, 1.0) — sunset through sunrise). False on a world where the cycle has
/// not published yet.
pub(super) fn is_night(world: &ServerWorld) -> bool {
    world
        .data()
        .world_kv_get(NIGHT_KEY)
        .map(|b| b.first().copied())
        == Some(Some(1))
}

/// The published day clock (`petramond:clock`), or 0 on a world whose cycle has
/// not published yet — stamped on every `TickUpdate` so a client's sky follows
/// the server's.
pub(super) fn current_clock(world: &ServerWorld) -> u64 {
    read_clock(world).unwrap_or(0)
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum TimePreset {
    Day,
    Noon,
    Night,
    Midnight,
}

/// Set the named point within the current absolute day. The core cycle adopts
/// this ordinary clock write on its next deterministic tick.
pub fn set_time(world: &mut ServerWorld, preset: TimePreset) {
    let cycle = world.day_cycle_ticks();
    let within_day = match preset {
        TimePreset::Day => fresh_clock(cycle),
        TimePreset::Noon => cycle / 4,
        TimePreset::Night => cycle / 2,
        TimePreset::Midnight => cycle * 3 / 4,
    };
    let current = read_clock(world).unwrap_or(fresh_clock(cycle));
    let clock = current / cycle * cycle + within_day;
    world.world_kv_set(CLOCK_KEY.into(), clock.to_le_bytes().to_vec());
}

/// Freeze/unfreeze the deterministic cycle at its current clock. The flag is
/// world KV, so save-all/autosave and reload preserve it.
pub fn set_frozen(world: &mut ServerWorld, frozen: bool) {
    world.world_kv_set(FROZEN_KEY.into(), vec![u8::from(frozen)]);
}

/// Skip the clock to early morning of the NEXT day (sleeping through the
/// night — or the day). Written as a `petramond:clock` KV like any external write;
/// the core cycle adopts it on its next tick (clock writes win exactly).
pub(super) fn skip_to_morning(world: &mut ServerWorld) {
    let cycle = world.day_cycle_ticks();
    let clock = read_clock(world).unwrap_or(fresh_clock(cycle));
    let next = morning_after(clock, cycle);
    world.world_kv_set(CLOCK_KEY.into(), next.to_le_bytes().to_vec());
}

fn read_clock(world: &ServerWorld) -> Option<u64> {
    let raw: [u8; 8] = world.data().world_kv_get(CLOCK_KEY)?.try_into().ok()?;
    Some(u64::from_le_bytes(raw))
}

fn read_frozen(world: &ServerWorld) -> bool {
    world
        .data()
        .world_kv_get(FROZEN_KEY)
        .and_then(|b| b.first())
        .copied()
        == Some(1)
}

fn read_time(world: &ServerWorld) -> Option<f32> {
    world
        .data()
        .world_kv_get(TIME_KEY)
        .and_then(read_time_bytes)
        .map(f32::from_le_bytes)
        .filter(|t| t.is_finite())
}

fn read_time_bytes(bytes: &[u8]) -> Option<[u8; 4]> {
    let raw: [u8; 4] = bytes.try_into().ok()?;
    f32::from_le_bytes(raw).is_finite().then_some(raw)
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::crafting::Recipes;

    use crate::rules::daynight::DEFAULT_CYCLE_TICKS;

    const C: u64 = DEFAULT_CYCLE_TICKS;

    #[test]
    fn sleeping_skips_to_the_next_early_morning() {
        let mut world = ServerWorld::new(1, 1);
        world.world_kv_set(CLOCK_KEY.into(), (C * 3 / 4).to_le_bytes().to_vec());
        skip_to_morning(&mut world);
        assert_eq!(
            world.data().world_kv_get(CLOCK_KEY),
            Some(&(C + fresh_clock(C)).to_le_bytes()[..]),
            "the skip writes the adopted petramond:clock format"
        );

        // A shorter per-world day skips by ITS cycle, not the default.
        let mut world = ServerWorld::new(1, 1);
        world.set_day_cycle_ticks(crate::rules::daynight::cycle_ticks_for_day_minutes(10));
        let c10 = world.day_cycle_ticks();
        world.world_kv_set(CLOCK_KEY.into(), (c10 * 3 / 4).to_le_bytes().to_vec());
        skip_to_morning(&mut world);
        assert_eq!(
            world.data().world_kv_get(CLOCK_KEY),
            Some(&(c10 + fresh_clock(c10)).to_le_bytes()[..])
        );
    }

    use crate::events::tick::TickEvents;
    use crate::events::{EventBus, RosterRefs};

    fn published_time(world: &ServerWorld) -> f32 {
        let bytes = world.data().world_kv_get(TIME_KEY).expect("petramond time");
        f32::from_le_bytes(bytes.try_into().expect("4-byte LE f32"))
    }

    #[test]
    fn core_daynight_restores_publishes_and_advances_on_tick_stage() {
        let mut world = ServerWorld::new(1, 1);
        world.world_kv_set(CLOCK_KEY.into(), (C * 3 / 4).to_le_bytes().to_vec());
        let mut systems = TickSystems::default();

        install_core(&mut world, &mut systems);

        let t0 = published_time(&world);
        assert!(
            (t0 - 0.75).abs() < 1e-6,
            "restored clock publishes midnight fraction, got {t0}"
        );
        assert_eq!(world.data().world_kv_get(NIGHT_KEY), Some(&[1u8][..]));

        let params = world.data().environment().shader_params();
        let light = params.get(SKY_LIGHT_PARAM).expect("sky light param");
        assert!(light[0] < 0.5, "midnight sky light is dark: {light:?}");
        assert!(
            light[1] < light[3] && light[2] < light[3],
            "midnight sky light keeps the blue-dominant tint: {light:?}"
        );
        assert_eq!(params.get(SKY_TIME_PARAM).expect("sky time param")[0], t0);

        world.game_tick(&Recipes::default());
        let mut feed = TickEvents::default();
        let mut bus = EventBus::default();
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut RosterRefs::empty(),
            &mut feed,
            bus.queue_mut(),
        );

        assert_eq!(
            world.data().world_kv_get(CLOCK_KEY),
            Some(&(C * 3 / 4 + 1).to_le_bytes()[..]),
            "clock advances by the elapsed engine tick"
        );
        assert!(
            published_time(&world) > t0,
            "published time advances with the clock"
        );

        world.world_kv_set(TIME_KEY.into(), 0.25f32.to_le_bytes().to_vec());
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut RosterRefs::empty(),
            &mut feed,
            bus.queue_mut(),
        );
        assert!(
            (published_time(&world) - ((C / 4 + 1) as f32 / C as f32)).abs() < 1e-6,
            "external petramond:time write is adopted on the next core tick"
        );

        world.world_kv_set(CLOCK_KEY.into(), (C / 2).to_le_bytes().to_vec());
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut RosterRefs::empty(),
            &mut feed,
            bus.queue_mut(),
        );
        assert_eq!(
            world.data().world_kv_get(CLOCK_KEY),
            Some(&(C / 2 + 1).to_le_bytes()[..]),
            "external petramond:clock write wins exactly"
        );
    }

    #[test]
    fn named_times_and_frozen_cycle_are_deterministic() {
        let mut world = ServerWorld::new(1, 1);
        world.world_kv_set(CLOCK_KEY.into(), (C + 123).to_le_bytes().to_vec());

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
        let mut feed = TickEvents::default();
        let mut bus = EventBus::default();
        for _ in 0..3 {
            world.game_tick(&Recipes::default());
            systems.run(
                Attach::After(Stage::Spawning),
                &mut world,
                &mut RosterRefs::empty(),
                &mut feed,
                bus.queue_mut(),
            );
        }
        assert_eq!(read_clock(&world), Some(frozen_at));
        assert_eq!(world.data().world_kv_get(FROZEN_KEY), Some(&[1][..]));

        set_frozen(&mut world, false);
        world.game_tick(&Recipes::default());
        systems.run(
            Attach::After(Stage::Spawning),
            &mut world,
            &mut RosterRefs::empty(),
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
