//! Mobs: a data-driven creature registry, plus the live entity manager and AI.
//!
//! Adding an animal is a `mobs.json` row. You don't touch the engine, the game loop, the scene or
//! the renderer; they all iterate the table.
//!
//! Species live in `assets/mobs.json`, layered like `blocks.json`, and each is an opaque [`Mob`] id
//! into that table. Engine species have the low ids, in the frozen [`ENGINE_MOB_NAMES`] order. Mod
//! packs add theirs as namespaced `mod_id:name` rows in load order. A row gives the model path,
//! render scale, body size, movement stats, spawn pack size and `brain` nodes (see `behavior`).
//!
//! Layering: `load` (catalog loader), `path` (pure A*), `brain` + `behavior` (per-tick AI), `nav`
//! (path following + jumps), `instance` (shared kinematics), `manager` (the live set). Nothing here
//! depends on `crate::render`. Each species' `.bbmodel` is precached into a compiled [`Model`] (via
//! [`model`]) that the renderer and simulation both read.

mod anim;
pub(crate) mod behavior;
mod body_geometry;
mod brain;
mod confined;
mod damage;
mod instance;
mod kinematics;
mod load;
mod manager;
mod model_meta;
mod nav;
mod noise;
mod path;
pub use path::CLIMB_CELLS;
mod populate;
mod ragdoll;
pub mod riding;
mod spatial;
mod spawn;
pub mod tags;

pub use body_geometry::{
    append_body_supports, body_boxes, body_has_peer_support, body_overlaps_block_boxes,
    body_pose_fits, body_separation, body_separation_from_body, clamp_body_yaw,
    closest_body_ray_hit, resolve_body_motion, terrain_safe_motion_prefix, BodyMotion,
    SolidMotionSolver,
};
pub use brain::Brain;
pub use instance::{hurt_flash01, DigStep, Instance};
pub use manager::{
    DeathDrop, MobAttack, MobExposureDamage, MobFall, MobSpill, MobTickEvents, Mobs, PlayerAnchor,
    ShearDrop, SimDistance,
};
pub use nav::mob_can_reach;
#[cfg(any(test, feature = "test-support"))]
pub use nav::route_path;
pub use nav::site_open;
pub use nav::ReachBudget;
pub use nav::{
    footholds, route_probe, walk_region, FloodAsk, KeptBoxes, ROUTE_PROBE_MAX_NODES,
    ROUTE_PROBE_TICK_BUDGET,
};
pub use noise::{player_steps_are_audible, Noise, NoiseField, NoiseKind};
pub use petramond_world::ai_vocab::validate_brain_extensions;
pub use spawn::{
    body_fits_at as spawn_body_fits_at, hostile_attempt_sites, hostile_kind_has_room,
    hostile_spawn_plan, HostileSpawnCache, HOSTILE_SPAWN_ATTEMPTS, PASSIVE_SPAWN_INTERVAL_TICKS,
};

use petramond_world::bbmodel::Model;
use petramond_world::biome::Biome;
use petramond_world::block::Block;
use petramond_world::fluid::Buoyancy;
use petramond_world::item::ItemType;

use brain::AiBehavior;

/// A mob species. The id is opaque at runtime, just the row in the loaded def table. Engine species
/// take the low ids in a frozen order (the consts below), and saves identify species by those ids
/// and names. Mod packs get more ids at load from namespaced `mobs.json` rows. Serde writes a
/// species as its name, like `"petramond:owl"`.
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Mob(pub u8);

#[allow(non_upper_case_globals)]
impl Mob {
    pub const Owl: Mob = Mob(0);
    pub const Sheep: Mob = Mob(1);
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntityRef {
    Player(crate::player::PlayerId),
    Mob(MobId),
}

pub type MobId = u64;
impl EntityRef {
    #[inline]
    pub fn player(self) -> Option<crate::player::PlayerId> {
        match self {
            Self::Player(id) => Some(id),
            Self::Mob(_) => None,
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub enum MobTagValue {
    Bool(bool),
    Int(i64),
    Float(f64),
    String(String),
}

impl From<mod_api::MobTagValue> for MobTagValue {
    fn from(v: mod_api::MobTagValue) -> Self {
        match v {
            mod_api::MobTagValue::Bool(b) => Self::Bool(b),
            mod_api::MobTagValue::I64(i) => Self::Int(i),
            mod_api::MobTagValue::F64(f) => Self::Float(f),
            mod_api::MobTagValue::Str(s) => Self::String(s),
        }
    }
}

impl From<&MobTagValue> for mod_api::MobTagValue {
    fn from(v: &MobTagValue) -> Self {
        match v {
            MobTagValue::Bool(b) => Self::Bool(*b),
            MobTagValue::Int(i) => Self::I64(*i),
            MobTagValue::Float(f) => Self::F64(*f),
            MobTagValue::String(s) => Self::Str(s.clone()),
        }
    }
}

impl MobTagValue {
    pub fn as_bool(&self) -> Option<bool> {
        match self {
            Self::Bool(b) => Some(*b),
            _ => None,
        }
    }

    pub fn as_int(&self) -> Option<i64> {
        match self {
            Self::Int(i) => Some(*i),
            _ => None,
        }
    }

    pub fn as_float(&self) -> Option<f64> {
        match self {
            Self::Float(f) => Some(*f),
            _ => None,
        }
    }

    pub fn as_str(&self) -> Option<&str> {
        match self {
            Self::String(s) => Some(s),
            _ => None,
        }
    }
}

pub const ENGINE_MOB_NAMES: &[&str] = &["petramond:owl", "petramond:sheep"];

impl std::fmt::Debug for Mob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match ENGINE_MOB_NAMES.get(self.0 as usize) {
            Some(name) => write!(f, "Mob({name})"),
            None => write!(f, "Mob(#{})", self.0),
        }
    }
}

impl serde::Serialize for Mob {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match defs().get(self.0 as usize) {
            Some(d) => s.serialize_str(d.name),
            None => Err(serde::ser::Error::custom(format!(
                "mob id {} is not registered",
                self.0
            ))),
        }
    }
}

impl<'de> serde::Deserialize<'de> for Mob {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let name = std::borrow::Cow::<str>::deserialize(d)?;
        defs()
            .iter()
            .position(|def| def.name == name)
            .map(|i| Mob(i as u8))
            .ok_or_else(|| serde::de::Error::custom(format!("unknown mob '{name}'")))
    }
}

impl Mob {
    #[inline]
    pub fn id(self) -> u8 {
        self.0
    }

    pub fn all() -> &'static [Mob] {
        &catalog().all
    }
}

pub const DEFAULT_HOSTILE_DESPAWN_RADIUS: f32 = 128.0;

/// When a mob far from every player leaves the world: always past `radius`, and, unless the row
/// opts out, at random once it is beyond [`PLAYER_REACTIVE_RANGE`].
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct Despawn {
    pub radius: f32,
    pub random: bool,
}

pub const PLAYER_REACTIVE_RANGE: f32 = 32.0;

#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobCategory {
    Passive,
    Hostile,
}

impl MobCategory {
    pub fn cap(self) -> u32 {
        match self {
            MobCategory::Passive => 25,
            MobCategory::Hostile => 70,
        }
    }

    pub fn default_despawn_radius(self) -> Option<f32> {
        match self {
            MobCategory::Passive => None,
            MobCategory::Hostile => Some(DEFAULT_HOSTILE_DESPAWN_RADIUS),
        }
    }
}

pub struct SpawnRule {
    pub biomes: &'static [Biome],
    pub underground: &'static [u8],
    pub y: Option<[i32; 2]>,
    pub space: Option<&'static [Block]>,
    pub chance: f32,
    pub chances: &'static [f32],
    pub ground: &'static [Block],
}

impl SpawnRule {
    pub fn admits(&self, biome: Biome, ground: Block) -> bool {
        (self.biomes.contains(&biome) || (self.biomes.is_empty() && !self.underground.is_empty()))
            && (self.ground.contains(&ground) || (self.ground.is_empty() && self.space.is_some()))
    }

    pub fn chance_in(&self, biome: Biome) -> f32 {
        if self.biomes.is_empty() && !self.underground.is_empty() {
            return self.chance;
        }
        match self.biomes.iter().position(|&b| b == biome) {
            Some(i) => self.chance * self.chances.get(i).copied().unwrap_or(1.0),
            None => 0.0,
        }
    }

    pub fn is_spawnable(&self) -> bool {
        (!self.biomes.is_empty() || !self.underground.is_empty())
            && (!self.ground.is_empty() || self.space.is_some())
    }
}

#[derive(Copy, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SpawnGroup {
    pub min: u8,
    pub max: u8,
}

impl SpawnGroup {
    pub fn min_count(self) -> u32 {
        self.min.min(self.max).max(1) as u32
    }

    pub fn roll(self, rng: &mut MobRng) -> u32 {
        let lo = self.min.min(self.max).max(1) as i32;
        let hi = self.min.max(self.max).max(1) as i32;
        rng.next_range(lo, hi) as u32
    }
}

pub struct Habitat {
    pub avoid: &'static [Biome],
    pub prefer: &'static [Biome],
}

#[derive(Copy, Clone, Debug)]
pub struct WanderCohesion {
    pub companion: Mob,
    pub search_radius_multiplier: u8,
}

impl WanderCohesion {
    pub fn search_radius(self, wander_radius: i32) -> i32 {
        let multiplier = i32::from(self.search_radius_multiplier.max(1));
        wander_radius.saturating_mul(multiplier)
    }
}

#[derive(Copy, Clone, Debug)]
pub struct WanderTuning {
    pub chance_per_tick: f32,
    pub radius: i32,
    pub avoid_ground: &'static [Block],
    pub cohesion: Option<WanderCohesion>,
}

petramond_math::wire_enum::wire_enum! {
    #[derive(Hash, serde::Deserialize)]
    #[serde(rename_all = "snake_case")]
    pub enum MobSoundCategory: u8 {
        Idle = 0,
        Hurt = 1,
        Death = 2,
    }
    default Idle
}

pub const DEFAULT_DAMAGE_FLASH_SECS: f32 = 0.3;
pub const DEFAULT_DAMAGE_KNOCKBACK_SECS: f32 = 0.3;

#[derive(Clone, Debug, PartialEq)]
pub struct MobDamageFeedback {
    pub components: Vec<MobDamageFeedbackComponent>,
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub enum MobDamageFeedbackComponent {
    DecreaseHealth,
    Flash { duration: f32 },
    Knockback { scale: f32, duration: f32 },
    Sound { category: MobDamageSound },
    Ragdoll,
    Immunity { ticks: u32 },
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MobDamageSound {
    Hurt,
    Death,
}

impl MobDamageFeedback {
    pub fn none() -> Self {
        Self {
            components: Vec::new(),
        }
    }

    #[inline]
    pub fn has_any_component(&self) -> bool {
        !self.components.is_empty()
    }

    #[inline]
    pub fn has_immunity(&self) -> bool {
        self.components
            .iter()
            .any(|c| matches!(c, MobDamageFeedbackComponent::Immunity { .. }))
    }

    #[inline]
    pub fn plays_sound(&self, sound: MobDamageSound) -> bool {
        self.components.iter().any(|c| {
            matches!(
                c,
                MobDamageFeedbackComponent::Sound { category } if *category == sound
            )
        })
    }
}

impl Default for MobDamageFeedback {
    fn default() -> Self {
        Self {
            components: vec![
                MobDamageFeedbackComponent::DecreaseHealth,
                MobDamageFeedbackComponent::Flash {
                    duration: DEFAULT_DAMAGE_FLASH_SECS,
                },
                MobDamageFeedbackComponent::Knockback {
                    scale: 1.0,
                    duration: DEFAULT_DAMAGE_KNOCKBACK_SECS,
                },
                MobDamageFeedbackComponent::Sound {
                    category: MobDamageSound::Hurt,
                },
                MobDamageFeedbackComponent::Sound {
                    category: MobDamageSound::Death,
                },
                MobDamageFeedbackComponent::Ragdoll,
                MobDamageFeedbackComponent::Immunity {
                    ticks: petramond_world::damage::MOB_DAMAGE_IFRAME_TICKS,
                },
            ],
        }
    }
}

#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundSpec {
    pub category: MobSoundCategory,
    pub sound: petramond_world::sound_registry::Sound,
    pub tick_interval: Option<u32>,
    pub tick_interval_variance: u32,
}

pub const MAX_MOB_BODY_HALF_EXTENT: f32 = 32.0;
pub const MAX_MOB_BODY_HEIGHT: f32 = 32.0;
pub const MAX_MOB_BODY_SEGMENTS: usize = 64;
pub const MAX_MOB_SEAT_OFFSET: f32 = 32.0;
pub const MAX_MOB_REACH: f32 = 16.0;

#[derive(Copy, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobSize {
    pub half_width: f32,
    pub height: f32,
    #[serde(default)]
    pub half_length: Option<f32>,
}

impl MobSize {
    pub fn validate(self) -> Result<(), String> {
        if !self.half_width.is_finite()
            || self.half_width <= 0.0
            || self.half_width > MAX_MOB_BODY_HALF_EXTENT
        {
            return Err(format!(
                "size.half_width must be finite and in (0, {MAX_MOB_BODY_HALF_EXTENT}], got {}",
                self.half_width
            ));
        }
        if !self.height.is_finite() || self.height <= 0.0 || self.height > MAX_MOB_BODY_HEIGHT {
            return Err(format!(
                "size.height must be finite and in (0, {MAX_MOB_BODY_HEIGHT}], got {}",
                self.height
            ));
        }
        if let Some(half_length) = self.half_length {
            if !half_length.is_finite()
                || half_length < self.half_width
                || half_length > MAX_MOB_BODY_HALF_EXTENT
            {
                return Err(format!(
                    "size.half_length must be finite and in [{}, {MAX_MOB_BODY_HALF_EXTENT}], got {half_length}",
                    self.half_width
                ));
            }
            let segments = (half_length / self.half_width).ceil() as usize;
            if segments > MAX_MOB_BODY_SEGMENTS {
                return Err(format!(
                    "long body requires {segments} segments; maximum is {MAX_MOB_BODY_SEGMENTS}"
                ));
            }
        }
        Ok(())
    }

    pub fn body_segments(self) -> usize {
        if self.validate().is_err() {
            return 0;
        }
        let half_length = self.half_length.unwrap_or(self.half_width);
        (half_length / self.half_width).ceil().max(1.0) as usize
    }

    #[inline]
    pub fn head_cells(self) -> i32 {
        (self.height.ceil() as i32).max(1)
    }
}

#[derive(Copy, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShearSpec {
    pub drop: ItemType,
    pub min: u8,
    pub max: u8,
    pub regrow_min: u32,
    pub regrow_max: u32,
    pub coat: CubeName,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CubeName(pub &'static str);

impl<'de> serde::Deserialize<'de> for CubeName {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        String::deserialize(d).map(|name| Self(String::leak(name)))
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct SavedMob {
    pub kind: Mob,
    pub pos: petramond_math::world_pos::WorldPos,
    pub yaw: f32,
    pub tags: std::collections::BTreeMap<String, MobTagValue>,
    pub container: petramond_world::container::Container,
}

impl SavedMob {
    pub fn of(inst: &Instance) -> Self {
        Self {
            kind: inst.kind,
            pos: inst.pos,
            yaw: inst.yaw,
            tags: inst.tags().clone(),
            container: inst.container().clone(),
        }
    }
}

pub struct BrainNode {
    pub node: &'static str,
    pub priority: u8,
    factory: load::NodeFactory,
    params: &'static serde_json::Value,
    inputs: behavior::ScriptedInputs,
}

impl BrainNode {
    fn validate(&self, def: &'static MobDef, all: &[MobDef]) -> Result<(), String> {
        (self.factory)(self.node, self.params, self.inputs, def, all).map(|_| ())
    }
}

pub fn build_brain(def: &'static MobDef) -> Brain {
    let mut brain = Brain::new();
    let extension_nodes = loaded()
        .extensions
        .iter()
        .filter(|(target, _)| *target == def.mob)
        .flat_map(|(_, nodes)| nodes.iter());
    for node in def.brain.iter().chain(extension_nodes) {
        let behavior: Box<dyn AiBehavior> =
            (node.factory)(node.node, node.params, node.inputs, def, defs()).unwrap_or_else(|e| {
                panic!(
                    "mob '{}': brain node '{}' failed after load validation: {e}",
                    def.name, node.node
                )
            });
        brain = brain.with_boxed(node.priority, behavior);
    }
    brain
}

pub use petramond_world::exposure::Tolerance;

pub struct MobDef {
    pub mob: Mob,
    pub name: &'static str,
    pub key: &'static str,
    pub model: &'static str,
    pub scale: f32,
    pub size: MobSize,
    pub tags: &'static std::collections::BTreeMap<String, MobTagValue>,
    pub data: &'static [(&'static str, &'static str)],
    pub loot: Option<String>,
    pub walk_speed: f32,
    pub jump_speed: f32,
    pub turn_rate: f32,
    pub walk_anim_rate: f32,
    pub category: MobCategory,
    pub despawn: Option<Despawn>,
    pub cap: u32,
    pub spawn: SpawnRule,
    pub spawn_group: SpawnGroup,
    pub wander: WanderTuning,
    pub habitat: Habitat,
    pub avoid_fluids: bool,
    pub footsteps: bool,
    pub step_noise: bool,
    pub self_ao: f32,
    pub edge_guard: bool,
    pub nav: nav::NavTuning,
    pub buoyancy: Buoyancy,
    pub tolerates: Tolerance,
    pub gravity_scale: f32,
    pub air_control: bool,
    pub collision: MobCollision,
    pub shear: Option<ShearSpec>,
    pub damage_feedback: MobDamageFeedback,
    pub sounds: &'static [MobSoundSpec],
    pub brain: &'static [BrainNode],
    pub seats: &'static [[f32; 3]],
    pub container_slots: usize,
    pub reach: f32,
    pub eye_height: f32,
    pub hands: Option<MobHands>,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MobHands {
    pub roll: f32,
    pub main: (&'static str, [f32; 3]),
    pub off: Option<(&'static str, [f32; 3])>,
}

impl MobDef {
    pub fn data_value(&self, key: &str) -> Option<&'static str> {
        self.data
            .iter()
            .find_map(|(k, value)| (*k == key).then_some(*value))
    }
    #[inline]
    pub fn sound_for(&self, category: MobSoundCategory) -> Option<&MobSoundSpec> {
        self.sounds.iter().find(|s| s.category == category)
    }

    #[inline]
    pub fn spawn_health(&self) -> f32 {
        match self.tags.get(tags::HEALTH) {
            Some(MobTagValue::Float(f)) => *f as f32,
            _ => unreachable!("loader guarantees a Float {} spawn tag", tags::HEALTH),
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobCollision {
    #[default]
    Soft,
    Solid,
}

pub fn solid_boxes(
    id: u64,
    pos: petramond_math::world_pos::WorldPos,
    yaw: f32,
    size: MobSize,
    out: &mut Vec<petramond_world::collision::DynBox>,
) {
    for (min, max) in body_boxes(pos, yaw, size) {
        out.push(petramond_world::collision::DynBox { id, min, max });
    }
}

pub const MAX_ACTIVE_MOB_EMITTERS: usize = 4;

pub const MAX_ACTIVE_MOB_ANIMS: usize = 4;

pub const MAX_MOB_TAGS: usize = 32;

pub const MAX_MOB_SEATS: usize = 8;

pub(crate) struct MobCatalog {
    loaded: load::LoadedMobs,
    all: Box<[Mob]>,
    by_key: rustc_hash::FxHashMap<&'static str, Mob>,
}

pub(crate) static CATALOG: petramond_world::content::Slot<MobCatalog> =
    petramond_world::content::Slot::new(
        "mobs.json",
        &[
            petramond_world::content::stage::ITEMS,
            petramond_world::content::stage::LOOT,
            petramond_world::content::stage::PARTICLE_EMITTERS,
            petramond_world::content::stage::SOUNDS,
            petramond_world::content::stage::BIOMES,
            petramond_world::content::stage::EFFECTS,
        ],
        load_catalog,
    );

fn load_catalog(reg: &petramond_world::content::ContentRegistry) -> Result<MobCatalog, String> {
    let loaded = load::table(reg.packs())?;
    Ok(MobCatalog {
        all: (0..loaded.defs.len()).map(|id| Mob(id as u8)).collect(),
        by_key: loaded
            .defs
            .iter()
            .enumerate()
            .map(|(i, d)| (d.key, Mob(i as u8)))
            .collect(),
        loaded,
    })
}

fn catalog() -> &'static MobCatalog {
    CATALOG.current()
}

fn loaded() -> &'static load::LoadedMobs {
    &catalog().loaded
}

pub fn defs() -> &'static [MobDef] {
    loaded().defs
}

impl MobDef {
    pub fn path_params(&self) -> path::PathParams {
        path::PathParams::for_body(self.size.head_cells(), self.size.half_width)
            .tolerating(self.tolerates.blocks)
    }
}

#[inline]
pub fn def(mob: Mob) -> &'static MobDef {
    &defs()[mob.0 as usize]
}

pub fn by_key(key: &str) -> Option<Mob> {
    catalog().by_key.get(key).copied()
}

static MODELS: petramond_world::content::Slot<Vec<Model>> =
    petramond_world::content::Slot::new("mob models", &["mobs.json"], compile_models);

fn compile_models(_: &petramond_world::content::ContentRegistry) -> Result<Vec<Model>, String> {
    Ok(defs()
        .iter()
        .map(|d| {
            let m = d.mob;
            let Some((src, _)) = petramond_world::assets::read_bytes(d.model) else {
                log::error!("mob model '{}' not found in the asset roots", d.model);
                return Model::empty();
            };
            petramond_world::asset_cache::load_or_compile::<Model>(d.name, &src).unwrap_or_else(
                |e| {
                    log::error!("mob model precache failed for {m:?}: {e}");
                    Model::empty()
                },
            )
        })
        .collect())
}

pub fn model(mob: Mob) -> &'static Model {
    &MODELS.current()[mob.0 as usize]
}

pub struct MobRng {
    seed: u64,
    counter: u64,
}

impl MobRng {
    pub fn new(seed: u64) -> Self {
        MobRng { seed, counter: 0 }
    }

    pub fn next_f32(&mut self) -> f32 {
        self.counter = self.counter.wrapping_add(1);
        crate::entity::hash01(self.seed ^ self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    pub fn next_u64(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        let mut z = (self.seed ^ self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    pub fn next_range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as f32;
        lo + (self.next_f32() * span) as i32
    }

    pub fn next_signed(&mut self) -> f32 {
        self.next_f32() * 2.0 - 1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rng_is_deterministic_and_in_range() {
        let mut a = MobRng::new(7);
        let mut b = MobRng::new(7);
        for _ in 0..1000 {
            let x = a.next_f32();
            assert_eq!(x, b.next_f32(), "same seed -> same stream");
            assert!((0.0..1.0).contains(&x));
            let r = a.next_range(-8, 8);
            let _ = b.next_range(-8, 8);
            assert!((-8..=8).contains(&r), "range inclusive: {r}");
        }
    }

    #[test]
    fn def_round_trips_through_id() {
        for (i, d) in defs().iter().enumerate() {
            assert_eq!(def(d.mob).mob, d.mob);
            assert_eq!(d.mob, Mob(i as u8), "row index == id");
        }
        assert_eq!(Mob::all().len(), defs().len());
    }

    #[test]
    fn serde_speaks_registry_names() {
        for d in defs() {
            let v = serde_json::to_value(d.mob).expect("serializes");
            assert_eq!(v, serde_json::Value::String(d.name.into()));
            assert_eq!(serde_json::from_value::<Mob>(v).unwrap(), d.mob);
        }
        assert!(
            serde_json::from_value::<Mob>(serde_json::Value::String("no_such_mob".into())).is_err(),
            "unknown names error on deserialize"
        );
    }

    #[test]
    fn all_mob_model_sources_parse() {
        for d in defs() {
            let (src, _) = petramond_world::assets::read_bytes(d.model)
                .unwrap_or_else(|| panic!("{} model asset '{}' should exist", d.key, d.model));
            let text = String::from_utf8(src).expect("bbmodel is utf-8 JSON");
            let model =
                Model::load(&text).unwrap_or_else(|e| panic!("{} model should parse: {e}", d.key));
            assert!(
                !model.cubes.is_empty(),
                "{} model should have renderable geometry",
                d.key
            );
        }
    }
}
