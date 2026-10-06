//! Sunburn: zombies under open sky catch fire by day, burn harder the longer they stay in it, and
//! rain puts them out. Rain comes from the weather mod's field when one is loaded; without it the
//! sky is always clear.

use mod_sdk::*;
use weather_core::feed::FieldFeed;
use weather_core::FieldParams;

use crate::day_clock::daylight_now;
use crate::keys;
use crate::routes;

const SUNBURN_RADIUS: f32 = 160.0;
const SUNBURN_SKY_THRESHOLD: f32 = weather_core::DIRECT_SKY_MIN as f32;
const MAX_SKY_LIGHT: f32 = 63.0;
const SUNBURN_CHANCE_PER_100: u64 = 5;
const LIGHT_FIRE_TICKS: u32 = 100;
const DARK_COOL_TICKS: u32 = 60;
const RAIN_COOL_BOOST: f32 = 3.0;

#[derive(Default)]
pub struct Sunburn {
    burning: Option<Burning>,
    zombie: Option<MobId>,
    weather: FieldFeed,
}

#[derive(Copy, Clone)]
struct Burning {
    id: ConditionId,
    light: u8,
    great: u8,
}

impl Sunburn {
    pub fn init(zombie: Option<MobId>) -> Sunburn {
        register_tick_system(
            Stage::Spawning,
            AttachSide::After,
            20,
            routes::TickSystem::Sunburn.id(),
        );
        weather_core::feed::subscribe(routes::Handler::WeatherFeed.id());
        let burning = resolve_condition_logged(keys::BURNING).and_then(|info| {
            Some(Burning {
                id: info.id,
                light: info.stage("light")?,
                great: info.stage("great")?,
            })
        });
        Sunburn {
            burning,
            zombie,
            weather: FieldFeed::default(),
        }
    }

    /// Hears the weather mod's field.
    pub fn observe(&mut self, payload: &EventPayload) {
        self.weather.observe(payload);
    }

    pub fn tick(&mut self) {
        self.weather.tick();
        let Some(daylight) = daylight_now() else {
            return;
        };
        let field = self.weather_field();
        let anchors: Vec<[f64; 3]> = players().iter().map(|p| p.state.pos).collect();
        let kinds: Vec<MobId> = self.zombie.into_iter().collect();
        let near = mobs_near_any_of(&anchors, SUNBURN_RADIUS, &kinds);
        self.burn(daylight, field.as_ref(), &near);
    }

    fn weather_field(&self) -> Option<FieldParams> {
        match world_kv_get(weather_core::CLOCK_KEY).and_then(|b| weather_core::decode_clock(&b)) {
            Some(clock) => self.weather.params_at(clock),
            None => self.weather.params(),
        }
    }

    fn burn(&mut self, daylight: f32, field: Option<&FieldParams>, near: &[MobSnapshot]) {
        let Some(fire) = self.burning else {
            return;
        };
        let sun_can_burn = MAX_SKY_LIGHT * daylight >= SUNBURN_SKY_THRESHOLD;
        let roll = rng_u64("sunburn");
        for mob in near.iter().filter(|m| Some(m.kind) == self.zombie) {
            let burning = mob.conditions.iter().find(|c| c.condition == fire.id);
            if burning.is_none() && splitmix64_mix(roll ^ mob.id) % 100 >= SUNBURN_CHANCE_PER_100 {
                continue;
            }
            let rain = rain_at(field, mob.pos);
            if rain == 0.0 && !sun_can_burn {
                continue;
            }
            let entity = EntityRef::Mob(mob.id);
            let cell = cell_of(mob.pos);
            let Some(sky) = sky_light(cell) else {
                continue;
            };
            let rained_on = rain > 0.0 && sky >= SUNBURN_SKY_THRESHOLD;
            if rained_on {
                entity_condition_cool(entity, fire.id, (rain * RAIN_COOL_BOOST) as u32);
            } else if rain == 0.0 && sky * daylight >= SUNBURN_SKY_THRESHOLD {
                let (stage, ticks) = if burning.is_some_and(|b| b.elapsed >= LIGHT_FIRE_TICKS) {
                    (fire.great, DARK_COOL_TICKS * 2)
                } else {
                    (fire.light, DARK_COOL_TICKS)
                };
                entity_condition_apply(entity, fire.id, stage, ticks);
            }
        }
    }
}

fn sky_light(cell: [i32; 3]) -> Option<f32> {
    light_at(cell).map(|l| l.sky as f32)
}

fn rain_at(field: Option<&FieldParams>, pos: [f64; 3]) -> f32 {
    field.map_or(0.0, |p| weather_core::rain(pos[0], pos[2], p))
}
