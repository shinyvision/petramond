//! Load mob definitions from `assets/mobs.json` (serde).
//!
//! Mirror of `item::load`: every species' data row (model path, sizes, speeds,
//! health, category, despawn radius, spawn rule, wander tuning, habitat, shear spec,
//! brain) lives on disk, editable — and moddable — without a rebuild. Rows are keyed by registry
//! name: an ENGINE mob name overrides that species' row, a NAMESPACED key
//! (`mod_id:name`) REGISTERS a new dynamic species (same rules as
//! [`crate::registry`]); a new bare name is an error. The table is load-bearing
//! (instances index by id, loot resolves by key), so the loader validates the file
//! covers EVERY registered species exactly once — with unique keys — and fails
//! loudly otherwise.
//!
//! Brains are data here: each row's `brain` list of `{node, priority, params}` is
//! resolved against the engine AI-node registry ([`super::behavior::factory`]) and
//! every node's params are validated by actually running its factory once, so a bad
//! brain fails the load, never a spawn. A namespaced node key resolves to the
//! scripted (WASM) AI node (see `behavior::wasm`).

use std::collections::HashSet;

use serde::Deserialize;

use crate::biome::Biome;
use crate::block::Block;
use crate::registry::NameTable;

use super::brain::AiBehavior;
use super::{
    behavior, BrainNode, Buoyancy, Habitat, Mob, MobCategory, MobCollision, MobDamageFeedback,
    MobDamageFeedbackComponent, MobDamageSound, MobDef, MobSize, MobSoundCategory, MobSoundSpec,
    ShearSpec, SpawnGroup, SpawnRule, WanderCohesion, WanderTuning, DEFAULT_DAMAGE_FLASH_SECS,
    DEFAULT_DAMAGE_KNOCKBACK_SECS, ENGINE_MOB_NAMES,
};

/// Constructs one AI node from its row key + brain-row params + the owning
/// species row. Factories run once at load for validation and once per
/// spawned mob. The key matters only to the scripted (WASM) node, which
/// routes its dispatch on it; engine factories ignore it. The trailing slice
/// is the FULL def table (the in-flight leaked slice during load validation)
/// for factories that resolve cross-species references (`chase_sound`'s
/// `mob_targets`) — a factory must NEVER call `defs()` itself: validation runs
/// inside that LazyLock's initializer and the re-entry deadlocks the load.
pub(super) type NodeFactory = fn(
    &'static str,
    &serde_json::Value,
    &'static MobDef,
    &[MobDef],
) -> Result<Box<dyn AiBehavior>, String>;

#[derive(Deserialize)]
struct RawFile {
    mobs: Vec<RawMobDef>,
}

/// One species row as written in `mobs.json`: a mirror of [`MobDef`] with owned
/// strings/Vecs. Biome and companion references ride as name strings (resolved
/// against this catalog's own tables); block/item references use their registry
/// serde directly.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMobDef {
    /// Registry name: an engine mob name (override) or a namespaced `mod_id:name`
    /// key (dynamic registration).
    mob: String,
    key: String,
    /// Asset-relative `.bbmodel` path, resolved through the pack overlay.
    model: String,
    scale: f64,
    size: MobSize,
    max_health: f64,
    walk_speed: f64,
    jump_speed: f64,
    turn_rate: f64,
    walk_anim_rate: f64,
    category: MobCategory,
    #[serde(default)]
    despawn_radius: Option<f64>,
    cap: u32,
    spawn: RawSpawn,
    spawn_group: SpawnGroup,
    wander: RawWander,
    habitat: RawHabitat,
    avoid_water: bool,
    /// Water behavior (see [`Buoyancy`]); omitted = `swim`.
    #[serde(default)]
    buoyancy: Buoyancy,
    /// Body collision role (see [`MobCollision`]); omitted = `soft`.
    #[serde(default)]
    collision: MobCollision,
    #[serde(default)]
    shear: Option<ShearSpec>,
    #[serde(default)]
    damage_feedback: Vec<RawMobDamageFeedback>,
    #[serde(default)]
    sounds: Vec<RawMobSound>,
    brain: Vec<RawBrainNode>,
    /// Rider seat offsets in mob-local blocks (`+z` = facing, `+x` = right,
    /// `y` = up from the feet). Empty/omitted = not rideable.
    #[serde(default)]
    seats: Vec<[f64; 3]>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawSpawn {
    /// Biome names (see [`Biome::from_name`]). Empty = never natural-spawned.
    biomes: Vec<String>,
    ground: Vec<Block>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawWander {
    chance_per_tick: f64,
    radius: i32,
    #[serde(default)]
    cohesion: Option<RawCohesion>,
}

/// Companion rides as a NAME string (not `Mob` serde): resolving it through the
/// in-flight name table avoids the loader recursing into the very def table it is
/// building.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawCohesion {
    companion: String,
    search_radius_multiplier: u8,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawHabitat {
    avoid: Vec<String>,
    prefer: Vec<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawMobSound {
    /// Registry key from `sounds.json`.
    sound: String,
    /// Semantic mob sound slot (`idle`, `hurt`, or `death`).
    category: MobSoundCategory,
    /// Required for `idle`, rejected for one-shot categories.
    #[serde(default)]
    tick_interval: Option<u32>,
    /// Symmetric variance around `tick_interval`; only meaningful for `idle`.
    #[serde(default)]
    tick_interval_variance: Option<u32>,
}

#[derive(Clone, Debug, Deserialize)]
#[serde(tag = "component", deny_unknown_fields)]
enum RawMobDamageFeedback {
    #[serde(rename = "petramond:decrease_health")]
    DecreaseHealth,
    #[serde(rename = "petramond:flash")]
    Flash {
        #[serde(default = "default_flash_duration")]
        duration: f64,
    },
    #[serde(rename = "petramond:knockback")]
    Knockback {
        #[serde(default = "default_knockback_scale")]
        scale: f64,
        #[serde(default = "default_knockback_duration")]
        duration: f64,
    },
    #[serde(rename = "petramond:sound")]
    Sound { when: RawMobDamageSound },
    #[serde(rename = "petramond:ragdoll")]
    Ragdoll,
}

#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawMobDamageSound {
    Hurt,
    Death,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrainNode {
    node: String,
    /// Omitted = the node's canonical slot (wander lowest, expression above it,
    /// chase above wander, attack on top — see `brain::PRIORITY_*`).
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    params: serde_json::Value,
}

/// Load the mob table from every `mobs.json` layer (base + mod packs, later packs
/// replacing rows by mob), panicking with a precise message if the table is missing
/// or inconsistent.
pub(super) fn table() -> &'static [MobDef] {
    crate::registry::read_catalog("mobs.json", "mob", |texts| parse_layers(texts))
}

pub(super) fn parse_layers(texts: &[&str]) -> Result<&'static [MobDef], String> {
    let mut keys = HashSet::new();
    let defs = crate::registry::load_catalog(
        texts,
        |text| serde_json::from_str::<RawFile>(text).map(|f| f.mobs),
        |r| &r.mob,
        ENGINE_MOB_NAMES,
        "mob",
        |r, id, names| {
            if !keys.insert(r.key.clone()) {
                return Err(format!(
                    "mob '{}': duplicate key '{}' — loot tables resolve by key, so keys must be unique",
                    r.mob, r.key
                ));
            }
            let name = r.mob.clone();
            convert(r, Mob(id), names).map_err(|e| format!("mob '{name}': {e}"))
        },
    )?
    .rows();

    // Brain validation needs the leaked `&'static MobDef` rows (node factories read
    // row data), so it runs last: every factory must accept its params NOW, failing
    // the load rather than the first spawn.
    for d in defs {
        for node in d.brain {
            node.validate(d, defs)
                .map_err(|e| format!("mob '{}': brain node '{}': {e}", d.name, node.node))?;
        }
    }
    Ok(defs)
}

fn convert(r: RawMobDef, mob: Mob, names: &NameTable) -> Result<MobDef, String> {
    r.size.validate()?;
    let cohesion = match r.wander.cohesion {
        Some(c) => Some(WanderCohesion {
            companion: names
                .id(&c.companion)
                .map(Mob)
                .ok_or_else(|| format!("unknown companion mob '{}'", c.companion))?,
            search_radius_multiplier: c.search_radius_multiplier,
        }),
        None => None,
    };
    let despawn_radius = match r.despawn_radius {
        Some(radius) => {
            if !radius.is_finite() || radius <= 0.0 {
                return Err(format!(
                    "despawn_radius must be a positive finite number, got {radius}"
                ));
            }
            Some(radius as f32)
        }
        None => r.category.default_despawn_radius(),
    };
    if r.seats.len() > super::MAX_MOB_SEATS {
        return Err(format!(
            "at most {} seats per species, got {}",
            super::MAX_MOB_SEATS,
            r.seats.len()
        ));
    }
    let mut seats = Vec::with_capacity(r.seats.len());
    for (i, s) in r.seats.iter().enumerate() {
        let seat = [s[0] as f32, s[1] as f32, s[2] as f32];
        if !seat
            .iter()
            .all(|c| c.is_finite() && c.abs() <= super::MAX_MOB_SEAT_OFFSET)
        {
            return Err(format!(
                "seat #{i} offsets must remain finite f32 values within ±{}, got {s:?}",
                super::MAX_MOB_SEAT_OFFSET
            ));
        }
        seats.push(seat);
    }

    Ok(MobDef {
        mob,
        name: Box::leak(r.mob.into_boxed_str()),
        key: Box::leak(r.key.into_boxed_str()),
        model: Box::leak(r.model.into_boxed_str()),
        scale: r.scale as f32,
        size: r.size,
        max_health: r.max_health as f32,
        walk_speed: r.walk_speed as f32,
        jump_speed: r.jump_speed as f32,
        turn_rate: r.turn_rate as f32,
        walk_anim_rate: r.walk_anim_rate as f32,
        category: r.category,
        despawn_radius,
        cap: r.cap,
        spawn: SpawnRule {
            biomes: resolve_biomes(r.spawn.biomes)?,
            ground: Box::leak(r.spawn.ground.into_boxed_slice()),
        },
        spawn_group: r.spawn_group,
        wander: WanderTuning {
            chance_per_tick: r.wander.chance_per_tick as f32,
            radius: r.wander.radius,
            cohesion,
        },
        habitat: Habitat {
            avoid: resolve_biomes(r.habitat.avoid)?,
            prefer: resolve_biomes(r.habitat.prefer)?,
        },
        avoid_water: r.avoid_water,
        buoyancy: r.buoyancy,
        collision: r.collision,
        shear: r.shear,
        damage_feedback: convert_damage_feedback(r.damage_feedback)?,
        sounds: convert_sounds(r.sounds)?,
        brain: convert_brain(r.brain)?,
        seats: Box::leak(seats.into_boxed_slice()),
    })
}

fn default_flash_duration() -> f64 {
    DEFAULT_DAMAGE_FLASH_SECS as f64
}

fn default_knockback_scale() -> f64 {
    1.0
}

fn default_knockback_duration() -> f64 {
    DEFAULT_DAMAGE_KNOCKBACK_SECS as f64
}

fn convert_damage_feedback(rows: Vec<RawMobDamageFeedback>) -> Result<MobDamageFeedback, String> {
    if rows.is_empty() {
        return Ok(MobDamageFeedback::default());
    }
    let mut components = Vec::with_capacity(rows.len());
    for row in rows {
        components.push(match row {
            RawMobDamageFeedback::DecreaseHealth => MobDamageFeedbackComponent::DecreaseHealth,
            RawMobDamageFeedback::Flash { duration } => {
                if !duration.is_finite() || duration < 0.0 {
                    return Err(format!(
                        "damage_feedback petramond:flash duration must be finite and non-negative, got {duration}"
                    ));
                }
                MobDamageFeedbackComponent::Flash {
                    duration: duration as f32,
                }
            }
            RawMobDamageFeedback::Knockback { scale, duration } => {
                if !scale.is_finite() || scale < 0.0 {
                    return Err(format!(
                        "damage_feedback petramond:knockback scale must be finite and non-negative, got {scale}"
                    ));
                }
                if !duration.is_finite() || duration < 0.0 {
                    return Err(format!(
                        "damage_feedback petramond:knockback duration must be finite and non-negative, got {duration}"
                    ));
                }
                MobDamageFeedbackComponent::Knockback {
                    scale: scale as f32,
                    duration: duration as f32,
                }
            }
            RawMobDamageFeedback::Sound { when } => MobDamageFeedbackComponent::Sound {
                category: match when {
                    RawMobDamageSound::Hurt => MobDamageSound::Hurt,
                    RawMobDamageSound::Death => MobDamageSound::Death,
                },
            },
            RawMobDamageFeedback::Ragdoll => MobDamageFeedbackComponent::Ragdoll,
        });
    }
    Ok(MobDamageFeedback { components })
}

fn resolve_biomes(names: Vec<String>) -> Result<&'static [Biome], String> {
    let mut out = Vec::with_capacity(names.len());
    for n in names {
        out.push(Biome::from_name(&n).ok_or_else(|| format!("unknown biome '{n}'"))?);
    }
    Ok(Box::leak(out.into_boxed_slice()))
}

fn convert_brain(rows: Vec<RawBrainNode>) -> Result<&'static [BrainNode], String> {
    let mut nodes = Vec::with_capacity(rows.len());
    for r in rows {
        let Some(spec) = behavior::node_spec(&r.node) else {
            return Err(format!("unknown AI node '{}'", r.node));
        };
        // A missing `params` reads as JSON null; normalize to the empty object so
        // factories see one shape.
        let params = if r.params.is_null() {
            serde_json::Value::Object(serde_json::Map::new())
        } else {
            r.params
        };
        nodes.push(BrainNode {
            node: Box::leak(r.node.into_boxed_str()),
            priority: r.priority.unwrap_or(spec.default_priority),
            factory: spec.factory,
            params: Box::leak(Box::new(params)),
        });
    }
    Ok(Box::leak(nodes.into_boxed_slice()))
}

fn convert_sounds(rows: Vec<RawMobSound>) -> Result<&'static [MobSoundSpec], String> {
    let mut out = Vec::with_capacity(rows.len());
    let mut categories = HashSet::new();
    for r in rows {
        if !categories.insert(r.category) {
            return Err(format!("duplicate {:?} sound category", r.category));
        }
        let sound = crate::audio::sound_by_name(&r.sound)
            .ok_or_else(|| format!("sound '{}' is not registered in sounds.json", r.sound))?;
        let (tick_interval, tick_interval_variance) = match r.category {
            MobSoundCategory::Idle => {
                let interval = r.tick_interval.ok_or("idle sound requires tick_interval")?;
                if interval == 0 {
                    return Err("idle sound tick_interval must be greater than 0".into());
                }
                (Some(interval), r.tick_interval_variance.unwrap_or(0))
            }
            MobSoundCategory::Hurt | MobSoundCategory::Death => {
                if r.tick_interval.is_some() || r.tick_interval_variance.is_some() {
                    return Err(format!(
                        "{:?} sound must not set tick_interval or tick_interval_variance",
                        r.category
                    ));
                }
                (None, 0)
            }
        };
        out.push(MobSoundSpec {
            category: r.category,
            sound,
            tick_interval,
            tick_interval_variance,
        });
    }
    Ok(Box::leak(out.into_boxed_slice()))
}

#[cfg(test)]
mod tests;
