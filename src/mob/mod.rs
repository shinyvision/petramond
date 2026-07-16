//! Mobs: a data-driven creature registry plus the live entity manager + AI.
//!
//! Each species is an opaque [`Mob`] id indexing the runtime def table loaded from
//! `assets/mobs.json` (a layered catalog like `blocks.json`): engine species own the
//! low ids in the frozen [`ENGINE_MOB_NAMES`] order, and mod packs register more
//! through namespaced (`mod_id:name`) rows in load order. A row carries the species'
//! model asset path, render scale, body size, movement stats, spawn pack size, and a
//! `brain` list of `{node, priority, params}` rows resolved through the string-keyed
//! AI-node registry (see [`behavior`]). So **adding an animal is a `mobs.json` row**
//! — no engine edit, no change to the game loop, the scene, or the renderer (which
//! iterate the table generically).
//!
//! Layering: `load` (the catalog loader), `path` (pure A*), `brain` + `behavior`
//! (composable per-tick AI), `nav` (path following + jumps), `instance` (shared
//! kinematics), `manager` (the live set). Nothing here depends on `crate::render`:
//! each species' `.bbmodel` (read through the pack overlay) is precached into a
//! compiled [`Model`](crate::bbmodel::Model) (via [`model`]) that the renderer and
//! the simulation both read — the renderer also reads `scale` off the table.

mod behavior;
mod body_geometry;
mod brain;
mod instance;
mod load;
mod loot;
mod manager;
mod model_meta;
mod nav;
mod noise;
mod path;
mod populate;
mod ragdoll;
pub mod riding;
mod spawn;

pub(crate) use body_geometry::{
    append_body_supports, body_boxes, body_has_peer_support, body_overlaps_block_boxes,
    body_pose_fits, body_separation, body_separation_from_body, clamp_body_yaw,
    closest_body_ray_hit, resolve_body_motion, terrain_safe_motion_prefix, BodyMotion,
    SolidMotionSolver,
};
pub use brain::Brain;
pub use instance::{hurt_flash01, Instance};
pub use loot::{load_loot, LootTables};
pub use manager::{DeathDrop, MobAttack, MobFall, MobTickEvents, Mobs, PlayerAnchor, ShearDrop};
pub use noise::{player_steps_are_audible, Noise, NoiseKind};
pub(crate) use spawn::{
    body_fits_at as spawn_body_fits_at, hostile_attempt_sites, hostile_kind_has_room,
    hostile_spawn_plan, HOSTILE_SPAWN_ATTEMPTS, PASSIVE_SPAWN_INTERVAL_TICKS,
};

use std::sync::LazyLock;

use crate::bbmodel::Model;
use crate::biome::Biome;
use crate::block::Block;
use crate::item::ItemType;
use crate::mathh::Vec3;

use brain::AiBehavior;

/// A registered mob species, identified by its opaque runtime id (the row index in
/// the loaded def table). Engine species own the low ids in a compiled, frozen order
/// (the named consts below — the save palette identifies species by those ids/names);
/// mod packs register additional ids at load through namespaced `mobs.json` rows.
/// Serde carries a species as its registered NAME string
/// (`"petramond:owl"`, `"mod:zombie"`).
#[derive(Copy, Clone, PartialEq, Eq, Hash)]
pub struct Mob(pub u8);

/// Engine species consts, named like the enum variants they replaced so every
/// existing `Mob::Owl` expression and match pattern keeps compiling.
#[allow(non_upper_case_globals)]
impl Mob {
    pub const Owl: Mob = Mob(0);
    pub const Sheep: Mob = Mob(1);
}

/// A reference to a combat-capable entity: a connected player (by session id)
/// or a live mob (by its STABLE session id — never a storage index, which
/// `swap_remove` renumbers between the tick that captured it and the tick that
/// resolves it). This is the shared identity vocabulary for AI targeting
/// (`BehaviorOutput::target`), noise attribution ([`Noise::source`]), and
/// attacker memory (retaliation) — one type, so a perception node can lock
/// onto exactly what a noise or a hit named.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum EntityRef {
    Player(crate::server::player::PlayerId),
    Mob(u64),
}

/// Engine mob names in frozen id order (`ENGINE_MOB_NAMES[id]` names `Mob(id)`).
/// Append-only: save palettes identify mobs by these ids/names. Must stay in
/// lockstep with the consts above; the shipped `mobs.json` covering every name
/// keeps a typo here from going unnoticed.
pub(crate) const ENGINE_MOB_NAMES: &[&str] = &["petramond:owl", "petramond:sheep"];

impl std::fmt::Debug for Mob {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Engine names come from the compiled table only, so Debug works
        // mid-bootstrap; dynamic ids print numerically.
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
    /// The raw registry id.
    #[inline]
    pub fn id(self) -> u8 {
        self.0
    }

    /// Every registered species in id order (engine + pack-registered).
    pub fn all() -> &'static [Mob] {
        static ALL: LazyLock<Vec<Mob>> =
            LazyLock::new(|| (0..defs().len()).map(|id| Mob(id as u8)).collect());
        &ALL
    }
}

/// Compatibility default for hostile rows that omit `despawn_radius`.
pub(crate) const DEFAULT_HOSTILE_DESPAWN_RADIUS: f32 = 128.0;

/// The population group a species belongs to. Natural spawning caps each group
/// independently across the loaded area (so the world can't fill with one kind),
/// alongside the per-species [`MobDef::cap`]. A [`Passive`] mob defaults to persisting
/// when far from the player — it leaves the live set only by being saved into its
/// unloading chunk — while a [`Hostile`] one defaults to being culled immediately
/// at its row-resolved [`MobDef::despawn_radius`].
///
/// [`Passive`]: MobCategory::Passive
/// [`Hostile`]: MobCategory::Hostile
#[derive(Copy, Clone, Debug, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobCategory {
    Passive,
    Hostile,
}

impl MobCategory {
    /// The most individuals of this category that may exist in the loaded area at
    /// once; natural spawning stops the category here.
    pub fn cap(self) -> u32 {
        match self {
            MobCategory::Passive => 25,
            MobCategory::Hostile => 70,
        }
    }

    /// Compatibility default for rows that omit `despawn_radius`.
    pub(crate) fn default_despawn_radius(self) -> Option<f32> {
        match self {
            MobCategory::Passive => None,
            MobCategory::Hostile => Some(DEFAULT_HOSTILE_DESPAWN_RADIUS),
        }
    }
}

/// A species' natural-spawn site criteria. The spawner runs its universal checks
/// first (player distance, footing, headroom for the body) and only then asks the
/// species' rule, so a rule only describes what's *species-specific*: the biomes it
/// settles in and the blocks it will stand on. Declarative on purpose — adding a
/// species is data, not a new branch in the spawner. A rule matching nothing (empty
/// biome or ground list) makes the species programmatic-spawn-only: the natural
/// spawner skips it entirely (how a mod's tick system owns its own spawning).
pub struct SpawnRule {
    /// Biomes the species may spawn in.
    pub biomes: &'static [Biome],
    /// Blocks the species accepts as the ground under its feet (the cell it rests on).
    pub ground: &'static [Block],
}

impl SpawnRule {
    /// Whether a site in `biome`, standing on `ground`, satisfies this rule.
    pub fn admits(&self, biome: Biome, ground: Block) -> bool {
        self.biomes.contains(&biome) && self.ground.contains(&ground)
    }

    /// Whether this rule can admit any site at all — `false` marks a species the
    /// natural spawner never attempts (spawnable only programmatically).
    pub fn is_spawnable(&self) -> bool {
        !self.biomes.is_empty() && !self.ground.is_empty()
    }
}

/// How many individuals a successful natural spawn attempt tries to place near the
/// first valid site. Singleton species use `1..=1`; herd animals can request a
/// larger bounded group.
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

/// A species' biome affinity while idly wandering (distinct from where it *spawns*).
/// The wander AI never targets an `avoid` biome — save a bounded escape hatch so a
/// mob hemmed in by avoided terrain still moves — and, among the rest, leans toward
/// `prefer` biomes. The two lists should be disjoint (a biome isn't both).
pub struct Habitat {
    /// Biomes the wander AI refuses to walk into (until the escape hatch lifts it).
    pub avoid: &'static [Biome],
    /// Biomes the wander AI is drawn to when one is in reach.
    pub prefer: &'static [Biome],
}

/// Optional group preference for idle wandering. When present, a mob that has a
/// companion of `companion` inside the configured search radius will only choose
/// destinations that also keep one within its wander radius.
#[derive(Copy, Clone, Debug)]
pub struct WanderCohesion {
    pub companion: Mob,
    /// How many wander radii out to search before treating this mob as already
    /// lonely. Destinations still need a companion within one wander radius.
    pub search_radius_multiplier: u8,
}

impl WanderCohesion {
    pub fn search_radius(self, wander_radius: i32) -> i32 {
        let multiplier = i32::from(self.search_radius_multiplier.max(1));
        wander_radius.saturating_mul(multiplier)
    }
}

/// Data that controls idle wander cadence, range, and optional group preference. The
/// biome/water filters live beside it on [`MobDef`] because they are reused by spawn
/// and habitat-facing code.
#[derive(Copy, Clone, Debug)]
pub struct WanderTuning {
    pub chance_per_tick: f32,
    pub radius: i32,
    pub cohesion: Option<WanderCohesion>,
}

/// The semantic reason a mob sound is played. The sound clip itself stays in
/// `sounds.json`; this category maps a species to a row in that catalog.
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobSoundCategory {
    /// Periodic ambient call while the mob is alive and present to the client.
    Idle,
    /// A non-lethal hit landed on the mob.
    Hurt,
    /// The killing hit landed on the mob.
    Death,
}

pub(crate) const DEFAULT_DAMAGE_FLASH_SECS: f32 = 0.3;
pub(crate) const DEFAULT_DAMAGE_KNOCKBACK_SECS: f32 = 0.3;

/// Damage feedback components a species applies when a damage request survives
/// `mob_damage_pre`. Empty `damage_feedback` rows resolve to [`Default`], so a mob
/// row can opt into the normal engine bundle without enumerating it.
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
            ],
        }
    }
}

impl MobSoundCategory {
    /// Wire discriminant (net protocol `WorldEventMsg::MobSound`).
    pub(crate) fn to_u8(self) -> u8 {
        match self {
            Self::Idle => 0,
            Self::Hurt => 1,
            Self::Death => 2,
        }
    }

    pub(crate) fn from_u8(v: u8) -> Self {
        match v {
            1 => Self::Hurt,
            2 => Self::Death,
            _ => Self::Idle,
        }
    }
}

/// One species sound hook. Idle sounds carry a client-side tick cadence; hurt
/// and death sounds are fired from semantic hit/death events.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct MobSoundSpec {
    pub category: MobSoundCategory,
    pub sound: crate::audio::Sound,
    pub tick_interval: Option<u32>,
    pub tick_interval_variance: u32,
}

pub(crate) const MAX_MOB_BODY_HALF_EXTENT: f32 = 32.0;
pub(crate) const MAX_MOB_BODY_HEIGHT: f32 = 32.0;
pub(crate) const MAX_MOB_BODY_SEGMENTS: usize = 64;
pub(crate) const MAX_MOB_SEAT_OFFSET: f32 = 32.0;

/// A mob's collision/render footprint: a centred AABB `half_width` across and
/// `height` tall, with the feet at the mob position. A LONG body (a hull) may
/// also declare `half_length` along its FACING axis: its terrain movement,
/// SOLID-collision presence (see [`MobCollision::Solid`] and [`solid_boxes`]),
/// and targeting all use the oriented `half_length × half_width` rectangle as
/// a run of overlapping square boxes, so a boat's bow and stern behave like
/// its middle.
#[derive(Copy, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct MobSize {
    pub half_width: f32,
    pub height: f32,
    #[serde(default)]
    pub half_length: Option<f32>,
}

impl MobSize {
    /// Validate the geometry envelope shared by collision, targeting, and
    /// riding. Pack rows are untrusted input, and long-body segment count is
    /// bounded independently of absolute dimensions.
    pub(crate) fn validate(self) -> Result<(), String> {
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

    pub(crate) fn body_segments(self) -> usize {
        if self.validate().is_err() {
            return 0;
        }
        let half_length = self.half_length.unwrap_or(self.half_width);
        (half_length / self.half_width).ceil().max(1.0) as usize
    }

    /// Whole cells of vertical clearance the body needs (for standable/pathfinding
    /// tests): the height rounded up, at least one.
    #[inline]
    pub fn head_cells(self) -> i32 {
        (self.height.ceil() as i32).max(1)
    }
}

/// What shearing a species yields, when it can be shorn at all: the item dropped, the
/// per-shear count range, and how long (game ticks) the coat takes to grow back —
/// during which the mob renders without its coat and can't be shorn again. Row data on
/// [`MobDef`], so a new shearable species is a data edit, not new code.
#[derive(Copy, Clone, Debug, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ShearSpec {
    pub drop: ItemType,
    /// Inclusive drop-count range rolled per shear.
    pub min: u8,
    pub max: u8,
    /// Inclusive regrow-duration range (game ticks) rolled per shear.
    pub regrow_min: u32,
    pub regrow_max: u32,
}

/// A mob in its persisted form: just what survives a save — the species, where it
/// stands, which way it faces, how many ticks of coat regrowth remain (`0` = fully
/// coated), and its mod KV entries. A live [`Instance`] projects to this when its
/// chunk unloads (so it rides that chunk's save record, like a dropped item) and is
/// rebuilt from it on reload with a fresh brain. Transient AI/physics state (velocity,
/// health, animation, the despawn timer) is deliberately *not* saved: a reloaded mob
/// simply resumes wandering. The shear-regrow counter IS saved — a shorn sheep must
/// not reload with its wool back — and so is the mod KV (default-empty for records
/// older than section-record v3).
#[derive(Clone, Debug, PartialEq)]
pub struct SavedMob {
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
    pub shear_regrow: u32,
    /// Per-mob mod KV (`mod_id:key` → bytes), opaque to the engine; BTreeMap
    /// so the save encoding is deterministic.
    pub kv: std::collections::BTreeMap<String, Vec<u8>>,
}

impl SavedMob {
    /// Capture a live mob's persisted fields.
    pub fn of(inst: &Instance) -> Self {
        Self {
            kind: inst.kind,
            pos: inst.pos,
            yaw: inst.yaw,
            shear_regrow: inst.shear_regrow(),
            kv: inst.mod_kv().clone(),
        }
    }
}

/// One resolved row of a species' data-driven brain: an AI-node key, its priority,
/// the engine factory the key resolved to, and the row's (load-validated) params.
/// [`build_brain`] instantiates a fresh behavior per spawned mob from these.
pub struct BrainNode {
    /// The node key as written in the row (`"wander"`, `"chase_player"`, ...).
    pub node: &'static str,
    pub priority: u8,
    factory: load::NodeFactory,
    params: &'static serde_json::Value,
}

impl BrainNode {
    /// Run the factory once, discarding the behavior — the loader's validation pass,
    /// so a bad row fails the catalog load instead of the first spawn. `all` is the
    /// in-flight def table (validation runs inside the `defs()` initializer, so the
    /// factory must never reach for the LazyLock itself).
    fn validate(&self, def: &'static MobDef, all: &[MobDef]) -> Result<(), String> {
        (self.factory)(self.node, self.params, def, all).map(|_| ())
    }
}

/// Compose a species' AI [`Brain`] from its resolved brain rows. Called per spawned
/// mob (behaviors hold per-instance state). Factories were validated at catalog load,
/// so a failure here is a loader bug, not bad data.
pub(crate) fn build_brain(def: &'static MobDef) -> Brain {
    let mut brain = Brain::new();
    for node in def.brain {
        let behavior: Box<dyn AiBehavior> = (node.factory)(node.node, node.params, def, defs())
            .unwrap_or_else(|e| {
                panic!(
                    "mob '{}': brain node '{}' failed after load validation: {e}",
                    def.name, node.node
                )
            });
        brain = brain.with_boxed(node.priority, behavior);
    }
    brain
}

/// One row of the mob registry: everything that makes a species what it is. `model`
/// and `scale` feed the renderer; the rest drives the simulation. (`model` names the
/// `.bbmodel` asset, compiled once into the shared [`Model`](crate::bbmodel::Model) —
/// see [`model`].)
pub struct MobDef {
    pub mob: Mob,
    /// Registry name — the row key in `mobs.json` (`"petramond:owl"`, or
    /// `"mod_id:name"` for a pack species). The identity serde and the save
    /// palette speak.
    pub name: &'static str,
    /// Stable identity key (e.g. `"petramond:owl"`), independent of any display name
    /// — the key a loot table is looked up by. Mirrors [`crate::item::ItemType::key`].
    pub key: &'static str,
    /// Asset-relative `.bbmodel` path (`models/owl.bbmodel`), resolved through the
    /// pack overlay ([`crate::assets::read_bytes`]) at precache time (see [`model`]);
    /// at runtime the compiled [`Model`](crate::bbmodel::Model) is authoritative.
    pub model: &'static str,
    /// Model-unit → metre scale for rendering.
    pub scale: f32,
    /// Body AABB (collision + pathfinding clearance).
    pub size: MobSize,
    /// Starting (and maximum) health. A hit subtracts its rolled damage; at `0` the
    /// mob dies. Float because weapon damage is rolled from a per-weapon range.
    pub max_health: f32,
    /// Ground walk speed (m/s).
    pub walk_speed: f32,
    /// Upward launch speed of a jump (m/s); sized to clear a one-block step.
    pub jump_speed: f32,
    /// How fast the mob turns to face travel (rad/s).
    pub turn_rate: f32,
    /// Walk-cycle playback rate (animation-seconds per real second) while moving.
    pub walk_anim_rate: f32,
    /// Which population cap this species counts against (with [`MobCategory::cap`]).
    pub category: MobCategory,
    /// Distance (blocks) at or beyond which this species is distance-despawned
    /// immediately, or `None` for species that persist while loaded.
    pub despawn_radius: Option<f32>,
    /// Most individuals of this species allowed in the loaded area; natural spawning
    /// stops here even if the category cap has room.
    pub cap: u32,
    /// Where this species spawns naturally (biome + the block it stands on).
    pub spawn: SpawnRule,
    /// Number of nearby individuals produced by one successful natural spawn attempt.
    pub spawn_group: SpawnGroup,
    /// Idle wander cadence, radius, and optional group preference.
    pub wander: WanderTuning,
    /// Biome affinity for idle wandering (avoid / prefer) — see [`Habitat`].
    pub habitat: Habitat,
    /// Whether the wander AI steers destinations away from water (it still re-rolls a
    /// bounded number of times before settling for a wet spot — see the wander
    /// behavior). Crossing water en route is always allowed; this is only about where
    /// the mob chooses to head.
    pub avoid_water: bool,
    /// How this species behaves in water (`"buoyancy"` row, default `swim`) —
    /// see [`Buoyancy`].
    pub buoyancy: Buoyancy,
    /// This species' body collision role (`"collision"` row, default `soft`) —
    /// see [`MobCollision`].
    pub collision: MobCollision,
    /// What shearing this species yields, or `None` for species that can't be shorn.
    pub shear: Option<ShearSpec>,
    /// Damage feedback components resolved from this species' `damage_feedback` row.
    pub damage_feedback: MobDamageFeedback,
    /// Presentation sound hooks keyed by semantic mob event. The actual clip
    /// variants, gain, pitch jitter, and attenuation live in `sounds.json`.
    pub sounds: &'static [MobSoundSpec],
    /// The species' AI as data: priority-ordered node rows resolved against the
    /// engine AI-node registry at load (see [`behavior`] and [`build_brain`]).
    pub brain: &'static [BrainNode],
    /// Rider seat offsets in mob-local blocks — `+z` toward the mob's facing,
    /// `+x` to its right, `y` up from the feet. Empty = not rideable. The seat
    /// INDEX is the stable seat identity (mount calls and the replicated rider
    /// rows both speak it); the riding mechanism rotates the offset by the
    /// mob's live yaw each tick. Mount/dismount POLICY (who may sit where)
    /// stays with mods — the engine only owns the attachment.
    pub seats: &'static [[f32; 3]],
}

impl MobDef {
    #[inline]
    pub fn sound_for(&self, category: MobSoundCategory) -> Option<&MobSoundSpec> {
        self.sounds.iter().find(|s| s.category == category)
    }
}

/// How a species behaves in water (`mobs.json` `"buoyancy"`, default `swim`).
/// Creatures SWIM: they stroke toward air and bob through the waterline (see
/// the swim constants in [`instance`]). A hull FLOATS: it levels off at the
/// water surface (feet a small draft below it) and holds there — no bob.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Buoyancy {
    #[default]
    Swim,
    Surface,
}

/// A species' body collision role (`mobs.json` `"collision"`, default
/// `soft`). SOFT bodies interact through the overlap push only (every
/// creature — bodies jostle apart but never block). A SOLID body's AABB is
/// ALSO a rigid obstacle in the shared swept resolver: players and mobs walk
/// into it and stop, land on it and stand, and solid peers propose motion for
/// one deterministic pairwise solve before committing together (a hull). A
/// solid body never receives soft push and never soft-pushes a
/// player, because that would fight rigid contact; it still pushes overlapping
/// SOFT mobs through their own separation pass.
#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobCollision {
    #[default]
    Soft,
    Solid,
}

/// Emit the SOLID-collision boxes for one live body into `out`: the square
/// `half_width` box, or — when the species declares `half_length` — a run of
/// overlapping square boxes whose centres march along the FACING axis so the
/// union covers the oriented hull rectangle (axis-aligned staircase under a
/// diagonal yaw; exact on the axes). One shared emitter so the server's live
/// instances and the client's interpolated rows produce identical geometry.
pub fn solid_boxes(
    id: u64,
    pos: Vec3,
    yaw: f32,
    size: MobSize,
    out: &mut Vec<crate::collision::DynBox>,
) {
    for (min, max) in body_boxes(pos, yaw, size) {
        out.push(crate::collision::DynBox {
            id,
            min: min.to_array(),
            max: max.to_array(),
        });
    }
}

/// Most particle-emitter bundles active on one mob at a time — bounds the
/// per-mob replicated id list.
pub const MAX_ACTIVE_MOB_EMITTERS: usize = 4;

/// Most named animations active on one mob at a time — bounds the per-mob
/// replicated name list exactly like [`MAX_ACTIVE_MOB_EMITTERS`] bounds
/// emitters.
pub const MAX_ACTIVE_MOB_ANIMS: usize = 4;

/// Most rider seats one species may declare — bounds the `seats` row list and
/// keeps the wire-facing seat index an honest small integer.
pub const MAX_MOB_SEATS: usize = 8;

/// The loaded, id-ordered mob def table (engine rows first, then pack rows in load
/// order). Loads exactly once, on first access; a missing or inconsistent
/// `mobs.json` fails loudly at startup.
pub fn defs() -> &'static [MobDef] {
    static DEFS: LazyLock<&'static [MobDef]> = LazyLock::new(load::table);
    &DEFS
}

#[inline]
pub fn def(mob: Mob) -> &'static MobDef {
    &defs()[mob.0 as usize]
}

/// Every species' compiled [`Model`](crate::bbmodel::Model), indexed by `Mob` id —
/// the in-memory golden asset, precached once on first use (compiling each `.bbmodel` →
/// `.llmob` on a cache miss, else fast-loading the `.llmob`) and shared by the renderer and
/// the simulation. Sources are read through the pack overlay, so a pack can override a
/// species' art by shipping the same relative path. After this builds, nothing in the
/// running engine reads a `.bbmodel`.
static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    defs()
        .iter()
        .map(|d| {
            let m = d.mob;
            let Some((src, _)) = crate::assets::read_bytes(d.model) else {
                log::error!("mob model '{}' not found in the asset roots", d.model);
                return Model::empty();
            };
            crate::asset_cache::load_or_compile::<Model>(d.name, &src).unwrap_or_else(|e| {
                log::error!("mob model precache failed for {m:?}: {e}");
                Model::empty()
            })
        })
        .collect()
});

/// This species' precached [`Model`](crate::bbmodel::Model), borrowed for the process
/// lifetime: the renderer bakes geometry from it each frame and the simulation derives its
/// skeleton + idle metadata from it (see `model_meta`).
pub fn model(mob: Mob) -> &'static Model {
    &MODELS[mob.0 as usize]
}

/// A deterministic per-mob RNG (a SplitMix64-style finalizer over a seed + counter).
/// Reuses [`crate::entity::hash01`] so mobs need no `rand` crate and their wander is
/// fully reproducible.
pub struct MobRng {
    seed: u64,
    counter: u64,
}

impl MobRng {
    pub fn new(seed: u64) -> Self {
        MobRng { seed, counter: 0 }
    }

    /// Next value in `[0, 1)`.
    pub fn next_f32(&mut self) -> f32 {
        self.counter = self.counter.wrapping_add(1);
        crate::entity::hash01(self.seed ^ self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15))
    }

    /// Next full 64-bit value — a fresh seed for a sub-system (e.g. a death ragdoll's
    /// per-bone fling). A SplitMix64 finalizer over the seed + advanced counter.
    pub fn next_u64(&mut self) -> u64 {
        self.counter = self.counter.wrapping_add(1);
        let mut z = (self.seed ^ self.counter.wrapping_mul(0x9E37_79B9_7F4A_7C15))
            .wrapping_add(0x9E37_79B9_7F4A_7C15);
        z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
        z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
        z ^ (z >> 31)
    }

    /// Next integer in `[lo, hi]` (inclusive). Returns `lo` if the range is empty.
    pub fn next_range(&mut self, lo: i32, hi: i32) -> i32 {
        if hi <= lo {
            return lo;
        }
        let span = (hi - lo + 1) as f32;
        lo + (self.next_f32() * span) as i32
    }

    /// Next value in `[-1, 1)` — a symmetric glance/jitter amount.
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
            let (src, _) = crate::assets::read_bytes(d.model)
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
