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
//!
//! A layer may also carry ADDITIVE `brain_extensions`: `{mob, brain}` rows that
//! append nodes to ANOTHER catalog row's brain without restating (or owning) it —
//! how a pack composes behavior onto an engine or foreign species (the farming
//! pack's wheat-lure on the engine sheep). Extending is not registering, so the
//! pack-admission namespace rule does not apply to the target name. Extensions
//! are CROSS-PACK injections, so their failure policy is degradation, never a
//! catalog panic blaming the target row: what is checkable without the def
//! table (shape, node-key resolution, declared inputs) fails PACK ADMISSION
//! ([`validate_brain_extensions`]); what needs the loaded defs (factory param
//! validation, an unloaded target) logs the offending layer's source and skips
//! that extension, mirroring how an unregistered scripted node contributes no
//! opinion. Validated extensions live in a side table beside the def rows
//! ([`LoadedMobs::extensions`]) and are appended per spawn by
//! [`super::build_brain`] — the target row's own `brain` stays its own.

use std::collections::HashSet;

use serde::Deserialize;

use crate::biome::Biome;
use crate::block::Block;
use crate::registry::NameTable;

use super::brain::AiBehavior;
use super::{
    behavior, BrainNode, Buoyancy, Habitat, Mob, MobCategory, MobCollision, MobDamageFeedback,
    MobDamageFeedbackComponent, MobDamageSound, MobDef, MobSize, MobSoundCategory, MobSoundSpec,
    MobTagValue, ShearSpec, SpawnGroup, SpawnRule, WanderCohesion, WanderTuning,
    DEFAULT_DAMAGE_FLASH_SECS, DEFAULT_DAMAGE_KNOCKBACK_SECS, ENGINE_MOB_NAMES,
};

/// Constructs one AI node from its row key + brain-row params + declared
/// scripted inputs + the owning species row. Factories run once at load for
/// validation and once per spawned mob. The key and inputs matter only to the
/// scripted (WASM) node, which routes its dispatch on the key and ships only
/// the declared facts; engine factories ignore the key and reject inputs. The
/// trailing slice is the FULL def table (the in-flight leaked slice during
/// load validation) for factories that resolve cross-species references
/// (`chase_sound`'s `mob_targets`) — a factory must NEVER call `defs()`
/// itself: validation runs inside that LazyLock's initializer and the
/// re-entry deadlocks the load.
pub(super) type NodeFactory = fn(
    &'static str,
    &serde_json::Value,
    behavior::ScriptedInputs,
    &'static MobDef,
    &[MobDef],
) -> Result<Box<dyn AiBehavior>, String>;

#[derive(Deserialize)]
struct RawFile {
    mobs: Vec<RawMobDef>,
    /// Collected during the same parse the catalog frame runs — no extra
    /// per-layer parse pass.
    #[serde(default)]
    brain_extensions: Vec<RawBrainExtension>,
}

/// The extension-only lenient view a pack's `mobs.json` gets at ADMISSION
/// (before any registry exists) — see [`validate_brain_extensions`].
#[derive(Deserialize)]
struct RawExtFile {
    #[serde(default)]
    brain_extensions: Vec<RawBrainExtension>,
}

/// One additive brain extension: nodes appended to `mob`'s merged row.
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrainExtension {
    /// Registry name of the TARGET species — deliberately anyone's.
    mob: String,
    brain: Vec<RawBrainNode>,
}

/// Pack-admission validation of a layer's optional `brain_extensions`,
/// surfaced early (`manifest::registration_keys`) so a bad extension disables
/// the PACK — the admission contract — instead of panicking the whole catalog
/// load. Everything checkable WITHOUT the loaded def table is checked here:
/// the strict shape parse, node-key resolution through the same registry the
/// loader uses, and the declared-inputs vocabulary. Factory param validation
/// needs the target def and runs at catalog load, where a failing extension
/// is skipped with its source named (see the module docs).
pub(crate) fn validate_brain_extensions(text: &str) -> Result<(), String> {
    let file = serde_json::from_str::<RawExtFile>(text)
        .map_err(|e| format!("invalid brain_extensions: {e}"))?;
    for ext in &file.brain_extensions {
        for node in &ext.brain {
            admission_check_node(node).map_err(|e| {
                format!(
                    "invalid brain_extensions: extension for '{}': node '{}': {e}",
                    ext.mob, node.node
                )
            })?;
        }
    }
    Ok(())
}

/// The def-free half of extension-node validation (shared vocabulary with the
/// loader: `node_spec` + [`behavior::ScriptedInputs::parse`]).
fn admission_check_node(node: &RawBrainNode) -> Result<(), String> {
    if behavior::node_spec(&node.node).is_none() {
        return Err(format!("unknown AI node '{}'", node.node));
    }
    let inputs = behavior::ScriptedInputs::parse(&node.inputs)?;
    let scripted = crate::registry::namespace(&node.node)
        .is_some_and(|ns| ns != crate::registry::ENGINE_NAMESPACE);
    if !scripted && !inputs.is_empty() {
        return Err("'inputs' are only declarable on scripted (mod_id:name) nodes".into());
    }
    Ok(())
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
    /// Spawn tags: the mob tag map every individual of this species is born
    /// with (JSON bool/int/float/string → the typed [`MobTagValue`]). Must
    /// carry a positive numeric `petramond:health` — health IS a tag.
    tags: serde_json::Map<String, serde_json::Value>,
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
    #[serde(rename = "petramond:immunity")]
    Immunity {
        #[serde(default = "default_immunity_ticks")]
        ticks: u32,
    },
}

fn default_immunity_ticks() -> u32 {
    crate::damage::MOB_DAMAGE_IFRAME_TICKS
}

#[derive(Copy, Clone, Debug, Deserialize)]
#[serde(rename_all = "snake_case")]
enum RawMobDamageSound {
    Hurt,
    Death,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct RawBrainNode {
    node: String,
    /// Omitted = the node's canonical slot (wander lowest, expression above it,
    /// chase above wander, attack on top — see `brain::PRIORITY_*`).
    #[serde(default)]
    priority: Option<u8>,
    #[serde(default)]
    params: serde_json::Value,
    /// Scripted-node perception facts this node DECLARES it reads
    /// (`"inputs": ["player_held"]`) — only declared facts are computed and
    /// shipped per dispatch (see `behavior::ScriptedInputs`). Engine nodes
    /// read the sim context directly and must declare none.
    #[serde(default)]
    inputs: Vec<String>,
}

/// The mob catalog as loaded: the id-ordered def rows plus the validated
/// cross-pack brain extensions (a side table — the target rows' own `brain`
/// lists stay their own; [`super::build_brain`] appends per spawn).
pub(super) struct LoadedMobs {
    pub defs: &'static [MobDef],
    /// `(target, appended nodes)` in extension (layer) order.
    pub extensions: &'static [(Mob, &'static [BrainNode])],
}

/// Load the mob table from every `mobs.json` layer (base + mod packs, later packs
/// replacing rows by mob), panicking with a precise message if the table is missing
/// or inconsistent.
pub(super) fn table() -> LoadedMobs {
    crate::registry::read_catalog_labeled("mobs.json", "mob", |layers| {
        let labeled: Vec<(&str, String)> = layers
            .iter()
            .map(|(text, path)| (*text, path.display().to_string()))
            .collect();
        parse_layers_labeled(&labeled)
    })
}

#[cfg(test)]
pub(super) fn parse_layers(texts: &[&str]) -> Result<LoadedMobs, String> {
    let labeled: Vec<(&str, String)> = texts
        .iter()
        .enumerate()
        .map(|(li, text)| (*text, format!("mobs.json layer #{li}")))
        .collect();
    parse_layers_labeled(&labeled)
}

/// Each layer arrives with a source label (its pack path) so extension
/// diagnostics can name the offending pack.
fn parse_layers_labeled(layers: &[(&str, String)]) -> Result<LoadedMobs, String> {
    let texts: Vec<&str> = layers.iter().map(|(text, _)| *text).collect();
    // One parse per layer: the parse hook feeds the catalog frame its rows
    // and collects that layer's extensions aside, tagged with the layer index.
    let mut extensions: Vec<(usize, RawBrainExtension)> = Vec::new();
    let mut parse_li = 0usize;
    let mut keys = HashSet::new();
    let catalog = crate::registry::load_catalog(
        &texts,
        |text| {
            let file = serde_json::from_str::<RawFile>(text)?;
            let li = parse_li;
            parse_li += 1;
            extensions.extend(file.brain_extensions.into_iter().map(|e| (li, e)));
            Ok(file.mobs)
        },
        |r| &r.mob,
        ENGINE_MOB_NAMES,
        "mob",
        |r: RawMobDef, id, names| {
            if !keys.insert(r.key.clone()) {
                return Err(format!(
                    "mob '{}': duplicate key '{}' — loot tables resolve by key, so keys must be unique",
                    r.mob, r.key
                ));
            }
            let name = r.mob.clone();
            convert(r, Mob(id), names).map_err(|e| format!("mob '{name}': {e}"))
        },
    )?;
    let defs = catalog.rows();

    // Brain validation needs the leaked `&'static MobDef` rows (node factories read
    // row data), so it runs last: every factory must accept its params NOW, failing
    // the load rather than the first spawn.
    for d in defs {
        for node in d.brain {
            node.validate(d, defs)
                .map_err(|e| format!("mob '{}': brain node '{}': {e}", d.name, node.node))?;
        }
    }

    // Extensions are FOREIGN injections, so a bad one degrades (skip + a
    // warning naming its source layer) instead of failing the whole catalog
    // while blaming the target row. Admission already refused everything
    // checkable without the def table.
    let mut applied: Vec<(Mob, &'static [BrainNode])> = Vec::new();
    for (li, ext) in extensions {
        let source = &layers[li].1;
        let validated = match catalog.id(&ext.mob) {
            None => Err(format!("targets '{}', which is not loaded", ext.mob)),
            Some(id) => convert_brain(ext.brain)
                .and_then(|nodes| {
                    let target = &defs[id as usize];
                    for node in nodes {
                        node.validate(target, defs)
                            .map_err(|e| format!("brain node '{}': {e}", node.node))?;
                    }
                    Ok(nodes)
                })
                .map(|nodes| (Mob(id), nodes))
                .map_err(|e| format!("for '{}': {e}", ext.mob)),
        };
        match validated {
            Ok(entry) => applied.push(entry),
            Err(e) => log::warn!("{source}: mobs.json brain extension {e} — extension skipped"),
        }
    }
    Ok(LoadedMobs {
        defs,
        extensions: Box::leak(applied.into_boxed_slice()),
    })
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
        tags: convert_spawn_tags(r.tags)?,
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
            RawMobDamageFeedback::Immunity { ticks } => {
                MobDamageFeedbackComponent::Immunity { ticks }
            }
        });
    }
    Ok(MobDamageFeedback { components })
}

/// Convert a row's spawn-tag map to the typed runtime map: JSON bool → `Bool`,
/// integer → `Int`, other number → `Float`, string → `String` (arrays/objects
/// are rejected — a tag is one value). `petramond:health` is required,
/// positive, and normalized to `Float` however the row wrote it, so the
/// damage pipeline reads one type.
fn convert_spawn_tags(
    raw: serde_json::Map<String, serde_json::Value>,
) -> Result<&'static std::collections::BTreeMap<String, MobTagValue>, String> {
    if raw.len() > super::MAX_MOB_TAGS {
        return Err(format!(
            "at most {} spawn tags per species, got {}",
            super::MAX_MOB_TAGS,
            raw.len()
        ));
    }
    let mut tags = std::collections::BTreeMap::new();
    for (key, value) in raw {
        let tag = match value {
            serde_json::Value::Bool(b) => MobTagValue::Bool(b),
            serde_json::Value::Number(n) => match n.as_i64() {
                Some(i) => MobTagValue::Int(i),
                None => MobTagValue::Float(
                    n.as_f64()
                        .ok_or_else(|| format!("spawn tag '{key}': unrepresentable number"))?,
                ),
            },
            serde_json::Value::String(s) => MobTagValue::String(s),
            other => {
                return Err(format!(
                    "spawn tag '{key}' must be a bool, number, or string, got {other}"
                ));
            }
        };
        tags.insert(key, tag);
    }
    let health = match tags.get(super::tags::HEALTH) {
        Some(MobTagValue::Int(i)) => *i as f64,
        Some(MobTagValue::Float(f)) => *f,
        _ => {
            return Err(format!(
                "spawn tags must carry a numeric '{}' — health is a tag",
                super::tags::HEALTH
            ));
        }
    };
    if !health.is_finite() || health <= 0.0 {
        return Err(format!(
            "spawn tag '{}' must be positive and finite, got {health}",
            super::tags::HEALTH
        ));
    }
    tags.insert(
        super::tags::HEALTH.to_owned(),
        MobTagValue::Float(health),
    );
    Ok(Box::leak(Box::new(tags)))
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
        let inputs = behavior::ScriptedInputs::parse(&r.inputs)
            .map_err(|e| format!("brain node '{}': {e}", r.node))?;
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
            inputs,
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
