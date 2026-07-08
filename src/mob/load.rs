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
    behavior, BrainNode, Habitat, Mob, MobCategory, MobDef, MobSize, MobSoundCategory,
    MobSoundSpec, ShearSpec, SpawnGroup, SpawnRule, WanderCohesion, WanderTuning, ENGINE_MOB_NAMES,
};

/// Constructs one AI node from its row key + brain-row params + the owning
/// species row. Factories run once at load for validation and once per
/// spawned mob. The key matters only to the scripted (WASM) node, which
/// routes its dispatch on it; engine factories ignore it.
pub(super) type NodeFactory =
    fn(&'static str, &serde_json::Value, &'static MobDef) -> Result<Box<dyn AiBehavior>, String>;

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
    #[serde(default)]
    shear: Option<ShearSpec>,
    #[serde(default)]
    sounds: Vec<RawMobSound>,
    brain: Vec<RawBrainNode>,
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
    let layers = crate::assets::read_layers("mobs.json");
    if layers.is_empty() {
        panic!(
            "mobs.json not found (searched {:?}); the game cannot run without its mob table",
            crate::assets::candidate_paths("mobs.json")
        );
    }
    for (_, path) in &layers {
        log::info!("mob defs layer: {}", path.display());
    }
    let texts: Vec<&str> = layers.iter().map(|(s, _)| s.as_str()).collect();
    parse_layers(&texts).unwrap_or_else(|e| panic!("mobs.json: {e}"))
}

pub(super) fn parse_layers(texts: &[&str]) -> Result<&'static [MobDef], String> {
    // Same catalog contract as blocks/items/sounds/models: merge rows by key,
    // engine names keep their frozen ids, namespaced keys register fresh ids.
    let mut merged: Vec<RawMobDef> = Vec::new();
    let mut layer_keys: Vec<Vec<String>> = Vec::new();
    for (li, text) in texts.iter().enumerate() {
        let raw: RawFile =
            serde_json::from_str(text).map_err(|e| format!("layer #{li}: invalid JSON: {e}"))?;
        layer_keys.push(raw.mobs.iter().map(|r| r.mob.clone()).collect());
        for r in raw.mobs {
            match merged.iter_mut().find(|m| m.mob == r.mob) {
                Some(slot) => *slot = r,
                None => merged.push(r),
            }
        }
    }
    let names = NameTable::build(ENGINE_MOB_NAMES, &layer_keys, "mob")?;

    let mut rows: Vec<Option<MobDef>> = (0..names.len()).map(|_| None).collect();
    let mut keys = HashSet::new();
    for r in merged {
        let id = names
            .id(&r.mob)
            .ok_or_else(|| format!("unregistered mob '{}'", r.mob))?;
        if !keys.insert(r.key.clone()) {
            return Err(format!(
                "mob '{}': duplicate key '{}' — loot tables resolve by key, so keys must be unique",
                r.mob, r.key
            ));
        }
        let name = r.mob.clone();
        rows[id as usize] =
            Some(convert(r, Mob(id), &names).map_err(|e| format!("mob '{name}': {e}"))?);
    }
    let mut defs = Vec::with_capacity(rows.len());
    for (id, row) in rows.into_iter().enumerate() {
        defs.push(row.ok_or_else(|| {
            format!(
                "missing row for mob '{}'",
                names.name(id as u8).unwrap_or("?")
            )
        })?);
    }
    let defs: &'static [MobDef] = Box::leak(defs.into_boxed_slice());

    // Brain validation needs the leaked `&'static MobDef` rows (node factories read
    // row data), so it runs last: every factory must accept its params NOW, failing
    // the load rather than the first spawn.
    for d in defs {
        for node in d.brain {
            node.validate(d)
                .map_err(|e| format!("mob '{}': brain node '{}': {e}", d.name, node.node))?;
        }
    }
    Ok(defs)
}

fn convert(r: RawMobDef, mob: Mob, names: &NameTable) -> Result<MobDef, String> {
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
        shear: r.shear,
        sounds: convert_sounds(r.sounds)?,
        brain: convert_brain(r.brain)?,
    })
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
mod tests {
    use super::*;
    use crate::mathh::Vec3;

    fn base() -> String {
        let (text, _) =
            crate::assets::read_base_text("mobs.json").expect("assets/mobs.json must ship");
        text
    }

    /// The shipped `assets/mobs.json` must load fully — the same gate the game
    /// applies at startup, surfaced as a test so a bad edit fails CI, not a launch.
    #[test]
    fn shipped_mobs_json_loads_fully() {
        let defs = parse_layers(&[&base()]).unwrap_or_else(|e| panic!("shipped mobs.json: {e}"));
        assert_eq!(
            defs.len(),
            ENGINE_MOB_NAMES.len(),
            "the base table is exactly the engine set"
        );
        for (i, d) in defs.iter().enumerate() {
            assert_eq!(d.mob, Mob(i as u8));
            assert_eq!(d.name, ENGINE_MOB_NAMES[i]);
        }
    }

    #[test]
    fn pack_layer_overrides_rows_by_mob() {
        let layer = r#"{"mobs": [{
            "mob": "llama:owl", "key": "llama:owl", "model": "models/owl.bbmodel", "scale": 0.5,
            "size": {"half_width": 0.3, "height": 0.9}, "max_health": 6.0,
            "walk_speed": 3.0, "jump_speed": 7.2, "turn_rate": 7.0, "walk_anim_rate": 1.2,
            "category": "passive", "cap": 4,
            "spawn": {"biomes": ["forest"], "ground": ["llama:grass"]},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": ["forest"]},
            "avoid_water": true,
            "brain": [{"node": "wander", "priority": 0}]
        }]}"#;
        let defs = parse_layers(&[&base(), layer]).expect("layered table loads");
        assert_eq!(defs.len(), ENGINE_MOB_NAMES.len(), "an override adds no id");
        let owl = &defs[Mob::Owl.0 as usize];
        assert_eq!(owl.scale, 0.5);
        assert_eq!(owl.cap, 4);
        assert_eq!(owl.brain.len(), 1);
    }

    #[test]
    fn namespaced_pack_row_registers_a_hostile_mob_with_a_data_brain() {
        let layer = r#"{"mobs": [{
            "mob": "mymod:zombling", "key": "mymod:zombling", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.8}, "max_health": 20.0,
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "hostile", "despawn_radius": 64.0, "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_water": false,
            "brain": [
                {"node": "wander", "priority": 0},
                {"node": "chase_player", "priority": 20, "params": {"radius": 12.0, "give_up_radius": 18.0}},
                {"node": "melee_attack", "priority": 30, "params": {"reach": 1.2, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20}}
            ]
        }]}"#;
        let defs = parse_layers(&[&base(), layer]).expect("dynamic row loads");
        let engine = ENGINE_MOB_NAMES.len();
        assert_eq!(defs.len(), engine + 1, "a fresh id past the engine set");
        let z = &defs[engine];
        assert_eq!(z.mob, Mob(engine as u8));
        assert_eq!(z.name, "mymod:zombling");
        assert_eq!(z.category, MobCategory::Hostile);
        assert_eq!(z.despawn_radius, Some(64.0));
        assert!(
            !z.spawn.is_spawnable(),
            "an empty spawn rule = programmatic-spawn-only"
        );

        // The data brain WORKS: chase overrides wander toward a nearby player, and
        // melee emits an attack intent in reach — driven through the real Brain.
        let mut brain = super::super::build_brain(z);
        let world = {
            use crate::block::Block;
            use crate::chunk::{Chunk, ChunkPos, CHUNK_SX, CHUNK_SZ};
            let mut w = crate::world::World::new(0, 1);
            let mut c = Chunk::new(0, 0);
            for zz in 0..CHUNK_SZ {
                for xx in 0..CHUNK_SX {
                    c.set_block(xx, 63, zz, Block::Grass);
                }
            }
            w.insert_chunk_for_test(ChunkPos::new(0, 0), c);
            w
        };
        let mut rng = super::super::MobRng::new(1);
        let mob_pos = Vec3::new(2.5, 64.0, 2.5);
        let player = Vec3::new(3.7, 64.9, 2.5); // 1.2 blocks away: chase + melee range
        let mut ctx = super::super::brain::AiCtx {
            mob_id: 1,
            pos: mob_pos,
            cell: crate::mathh::voxel_at(mob_pos),
            yaw: -std::f32::consts::FRAC_PI_2, // facing +X, toward the player
            head_height: z.size.height,
            half_width: z.size.half_width,
            world: &world,
            player_pos: player,
            nav_idle: true,
            in_water: false,
            head: z.size.head_cells(),
            idle_anims: &[],
            mob_index: 0,
            mobs: &[],
            rng: &mut rng,
        };
        let decision = brain.decide(&mut ctx);
        assert_eq!(
            decision.goal,
            Some(crate::mathh::IVec3::new(3, 64, 2)),
            "chase_player steers navigation at the player's cell"
        );
        let attack = decision
            .attack
            .expect("melee_attack emits an intent in reach");
        assert_eq!(attack.damage, 2.0);
        assert_eq!(attack.knockback, 5.0);
    }

    #[test]
    fn mob_sound_hooks_resolve_registered_sound_keys() {
        let layer = r#"{"mobs": [{
            "mob": "mymod:caller", "key": "mymod:caller", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0,
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_water": false,
            "sounds": [
                {"category": "idle", "sound": "llama:item_pickup", "tick_interval": 40, "tick_interval_variance": 10},
                {"category": "hurt", "sound": "llama:wood_punch"},
                {"category": "death", "sound": "llama:wood_break"}
            ],
            "brain": []
        }]}"#;
        let defs = parse_layers(&[&base(), layer]).expect("sound hooks load");
        let caller = defs
            .iter()
            .find(|d| d.name == "mymod:caller")
            .expect("dynamic mob registered");
        assert_eq!(
            caller
                .sound_for(super::super::MobSoundCategory::Idle)
                .expect("idle hook")
                .tick_interval,
            Some(40)
        );
        assert!(
            caller
                .sound_for(super::super::MobSoundCategory::Hurt)
                .is_some(),
            "hurt hook resolved"
        );
        assert!(
            caller
                .sound_for(super::super::MobSoundCategory::Death)
                .is_some(),
            "death hook resolved"
        );
    }

    #[test]
    fn idle_mob_sound_requires_a_positive_interval() {
        let layer = r#"{"mobs": [{
            "mob": "mymod:caller", "key": "mymod:caller", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0,
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_water": false,
            "sounds": [{"category": "idle", "sound": "llama:item_pickup"}],
            "brain": []
        }]}"#;
        let err = parse_layers(&[&base(), layer])
            .map(|_| ())
            .expect_err("idle cadence is required");
        assert!(err.contains("tick_interval"), "{err}");
    }

    #[test]
    fn unknown_and_reserved_brain_nodes_are_load_errors() {
        let row = |node: &str| {
            format!(
                r#"{{"mobs": [{{
                    "mob": "mymod:thing", "key": "mymod:thing", "model": "models/owl.bbmodel",
                    "scale": 0.25, "size": {{"half_width": 0.3, "height": 1.0}}, "max_health": 4.0,
                    "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
                    "category": "passive", "cap": 8,
                    "spawn": {{"biomes": [], "ground": []}},
                    "spawn_group": {{"min": 1, "max": 1}},
                    "wander": {{"chance_per_tick": 0.0125, "radius": 8}},
                    "habitat": {{"avoid": [], "prefer": []}},
                    "avoid_water": false,
                    "brain": [{{"node": "{node}", "priority": 0}}]
                }}]}}"#
            )
        };
        let err = parse_layers(&[&base(), &row("levitate")])
            .map(|_| ())
            .expect_err("unknown node refused");
        assert!(err.contains("unknown AI node 'levitate'"), "{err}");
        // A namespaced key resolves to the scripted WASM node and loads.
        parse_layers(&[&base(), &row("mymod:levitate")]).expect("scripted node key loads");
        // Params on a params-less node are refused too (typos never load).
        let bad = r#"{"mobs": [{
            "mob": "mymod:thing", "key": "mymod:thing", "model": "models/owl.bbmodel",
            "scale": 0.25, "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0,
            "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
            "category": "passive", "cap": 8,
            "spawn": {"biomes": [], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []},
            "avoid_water": false,
            "brain": [{"node": "head_look", "priority": 10, "params": {"bogus": 1}}]
        }]}"#;
        let err = parse_layers(&[&base(), bad])
            .map(|_| ())
            .expect_err("stray params refused");
        assert!(err.contains("takes no params"), "{err}");
    }

    #[test]
    fn bare_additions_and_bad_references_are_rejected() {
        // A NEW bare mob name is refused at name-table build.
        let bare = r#"{"mobs": [{"mob": "zombling", "key": "zombling", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_water": false, "brain": []}]}"#;
        let err = parse_layers(&[&base(), bare])
            .map(|_| ())
            .expect_err("bare additions refused");
        assert!(
            err.contains("zombling") && err.contains("namespace"),
            "{err}"
        );

        // An unknown biome name in a spawn rule is a load error.
        let bad_biome = r#"{"mobs": [{"mob": "mymod:z", "key": "mymod:z", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": ["atlantis"], "ground": []},
            "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_water": false, "brain": []}]}"#;
        let err = parse_layers(&[&base(), bad_biome])
            .map(|_| ())
            .expect_err("unknown biome refused");
        assert!(err.contains("atlantis"), "{err}");

        // An unknown cohesion companion is a load error.
        let bad_companion = r#"{"mobs": [{"mob": "mymod:z", "key": "mymod:z", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8,
                       "cohesion": {"companion": "ghost", "search_radius_multiplier": 2}},
            "habitat": {"avoid": [], "prefer": []}, "avoid_water": false, "brain": []}]}"#;
        let err = parse_layers(&[&base(), bad_companion])
            .map(|_| ())
            .expect_err("unknown companion refused");
        assert!(err.contains("ghost"), "{err}");
    }

    #[test]
    fn loader_rejects_incomplete_tables_and_duplicate_keys() {
        // A single engine row is not a full table.
        let (owl_only, _) = {
            let full: serde_json::Value = serde_json::from_str(&base()).unwrap();
            let owl = full["mobs"][0].clone();
            (serde_json::json!({ "mobs": [owl] }).to_string(), ())
        };
        let err = parse_layers(&[&owl_only])
            .map(|_| ())
            .expect_err("partial tables refused");
        assert!(err.contains("missing row"), "{err}");

        // Two DIFFERENT mobs sharing one key: rejected (loot resolves by key).
        let clash = r#"{"mobs": [{"mob": "mymod:z", "key": "llama:owl", "model": "m", "scale": 1.0,
            "size": {"half_width": 0.3, "height": 1.0}, "max_health": 4.0, "walk_speed": 2.0,
            "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0, "category": "passive",
            "cap": 8, "spawn": {"biomes": [], "ground": []}, "spawn_group": {"min": 1, "max": 1},
            "wander": {"chance_per_tick": 0.0125, "radius": 8},
            "habitat": {"avoid": [], "prefer": []}, "avoid_water": false, "brain": []}]}"#;
        let err = parse_layers(&[&base(), clash])
            .map(|_| ())
            .expect_err("duplicate keys refused");
        assert!(err.contains("duplicate key"), "{err}");
    }

    /// End-to-end dynamic mob registration through a REAL pack: the namespaced
    /// species registers, spawns, distance-despawns per its hostile category, and
    /// pins into the save palette by name (with unknown disk names skipped).
    ///
    /// The global registries are process-wide LazyLocks, so pack injection must
    /// happen before ANY test touches them — this outer test re-spawns the test
    /// binary with `LLAMACRAFT_MODS` set, running only the `#[ignore]`d inner test
    /// below (the pattern pinned by `registry::tests::dynamic_pack_content_flows_
    /// end_to_end`).
    #[test]
    fn dynamic_pack_mob_flows_end_to_end() {
        let root = std::env::temp_dir().join(format!("llamacraft-mobpack-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let pack = root.join("mods/testmob");
        std::fs::create_dir_all(&pack).unwrap();
        std::fs::write(
            pack.join("pack.json"),
            r#"{ "name": "Test Mob", "id": "testmob", "description": "dynamic mob fixture" }"#,
        )
        .unwrap();
        // A hostile chaser reusing the engine owl model through the overlay
        // fallback; empty spawn rule = programmatic spawns only.
        std::fs::write(
            pack.join("mobs.json"),
            r#"{"mobs": [{
                "mob": "testmob:zombling", "key": "testmob:zombling",
                "model": "models/owl.bbmodel", "scale": 0.25,
                "size": {"half_width": 0.3, "height": 1.8}, "max_health": 20.0,
                "walk_speed": 2.0, "jump_speed": 7.2, "turn_rate": 6.0, "walk_anim_rate": 1.0,
                "category": "hostile", "cap": 8,
                "spawn": {"biomes": [], "ground": []},
                "spawn_group": {"min": 1, "max": 1},
                "wander": {"chance_per_tick": 0.0125, "radius": 8},
                "habitat": {"avoid": [], "prefer": []},
                "avoid_water": false,
                "brain": [
                    {"node": "wander", "priority": 0},
                    {"node": "chase_player", "priority": 20, "params": {"radius": 12.0, "give_up_radius": 18.0}},
                    {"node": "melee_attack", "priority": 30, "params": {"reach": 1.2, "damage": 2.0, "knockback": 5.0, "cooldown_ticks": 20}}
                ]
            }]}"#,
        )
        .unwrap();

        let exe = std::env::current_exe().expect("test binary path");
        let out = std::process::Command::new(exe)
            .arg("mob::load::tests::dynamic_pack_mob_inner")
            .arg("--exact")
            .arg("--ignored")
            .arg("--nocapture")
            .env("LLAMACRAFT_MODS", root.join("mods"))
            .env("LLAMACRAFT_MOBPACK_SAVE", root.join("save"))
            .output()
            .expect("spawn test binary");
        let _ = std::fs::remove_dir_all(&root);
        assert!(
            out.status.success(),
            "inner test failed\n--- stdout ---\n{}\n--- stderr ---\n{}",
            String::from_utf8_lossy(&out.stdout),
            String::from_utf8_lossy(&out.stderr),
        );
    }

    /// Runs ONLY in the child process spawned above (needs `LLAMACRAFT_MODS`
    /// pointing at the fixture pack before first registry touch).
    #[test]
    #[ignore = "spawned by dynamic_pack_mob_flows_end_to_end with a fixture pack env"]
    fn dynamic_pack_mob_inner() {
        use super::super::{def, defs, Mob, Mobs};
        use crate::world::World;

        let engine = ENGINE_MOB_NAMES.len();
        // --- Registration: one fresh id past the engine set, name-addressed. ---
        assert_eq!(defs().len(), engine + 1);
        let z = Mob(engine as u8);
        assert_eq!(def(z).name, "testmob:zombling");
        assert_eq!(
            serde_json::to_value(z).unwrap(),
            serde_json::Value::String("testmob:zombling".into())
        );

        // --- Spawnable programmatically; the data brain builds on spawn. ---
        let world = World::new(0, 1);
        let mut mobs = Mobs::new(0);
        let home = Vec3::new(8.0, 64.0, 8.0);
        assert!(mobs.spawn(z, home, 0.0));

        // --- Hostile despawn contract: culled on the first far tick. ---
        let near = home + Vec3::new(4.0, 0.0, 0.0);
        let far = home + Vec3::new(500.0, 0.0, 0.0); // way past the despawn radius
        for _ in 0..40 {
            mobs.tick(
                0.05,
                &world,
                &[crate::mob::PlayerAnchor {
                    id: Default::default(),
                    pos: near,
                    body: None,
                }],
                false,
            );
        }
        assert_eq!(mobs.len(), 1, "a near player keeps the hostile mob alive");
        mobs.tick(
            0.05,
            &world,
            &[crate::mob::PlayerAnchor {
                id: Default::default(),
                pos: far,
                body: None,
            }],
            false,
        );
        assert!(
            mobs.is_empty(),
            "a far player culls the hostile mob immediately"
        );

        // --- Save palette: the namespaced mob pins by name; strangers skip. ---
        let save = std::path::PathBuf::from(std::env::var_os("LLAMACRAFT_MOBPACK_SAVE").unwrap());
        std::fs::create_dir_all(&save).unwrap();
        // An "old" palette with a stranger between the engine mobs and ours, so
        // disk ids and runtime ids genuinely diverge.
        let blocks: Vec<&str> = crate::block::ENGINE_BLOCK_NAMES.to_vec();
        let items: Vec<&str> = crate::item::ENGINE_ITEM_NAMES.to_vec();
        std::fs::write(
            save.join("palette.json"),
            serde_json::json!({
                "blocks": blocks,
                "items": items,
                "mobs": ["llama:owl", "othermod:phantom", "llama:sheep"],
            })
            .to_string(),
        )
        .unwrap();
        let p = crate::save::palette::load_or_create(&save, &Default::default()).unwrap();
        for &m in Mob::all() {
            let disk = p.mob_to_disk(m.id()).expect("every enabled species pins");
            assert_eq!(
                p.mob_from_disk(disk),
                Some(m.id()),
                "{m:?} round-trips by name"
            );
        }
        assert_eq!(p.mob_to_disk(Mob::Owl.id()), Some(0));
        assert_eq!(
            p.mob_to_disk(Mob::Sheep.id()),
            Some(2),
            "remapped past the stranger"
        );
        assert_eq!(
            p.mob_to_disk(z.id()),
            Some(3),
            "the dynamic mob was appended"
        );
        assert_eq!(
            p.mob_from_disk(1),
            None,
            "the unknown disk name decodes to a skip, never a wrong species"
        );
        let text = std::fs::read_to_string(save.join("palette.json")).unwrap();
        assert!(
            text.contains("testmob:zombling"),
            "the dynamic mob is pinned in palette.json"
        );
    }
}
