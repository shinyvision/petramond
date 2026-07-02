//! Mobs: a data-driven creature registry plus the live entity manager + AI.
//!
//! Each species is a `#[repr(u8)]` [`Mob`] key indexing a [`MOB_DEFS`] row. A row
//! carries the species' model asset, render scale, body size, movement stats,
//! spawn pack size, and a `make_brain` constructor for its composable AI. So
//! **adding an animal is: add a `Mob` variant, add a `MobDef` row, write its
//! behavior** -- no edits to the game loop, the scene, or the renderer (which both
//! iterate this table generically).
//!
//! Layering: `path` (pure A*), `brain` + `behavior` (composable per-tick AI), `nav`
//! (path following + jumps), `instance` (shared kinematics), `manager` (the live
//! set). Nothing here depends on `crate::render`: each species' `.bbmodel` is precached
//! into a compiled [`Model`](crate::bbmodel::Model) (via [`model`]) that the renderer and
//! the simulation both read — the renderer also reads `scale` off the table.

mod behavior;
mod brain;
mod instance;
mod loot;
mod manager;
mod model_meta;
mod nav;
mod path;
mod push;
mod ragdoll;
mod spawn;

pub use brain::Brain;
pub use instance::Instance;
pub use loot::{load_loot, LootTables};
pub use manager::{DeathDrop, Mobs, ShearDrop};
pub use push::Body;

use std::sync::LazyLock;

use crate::bbmodel::Model;
use crate::biome::Biome;
use crate::block::Block;
use crate::item::ItemType;
use crate::mathh::Vec3;

/// A mob species — the stable key into [`MOB_DEFS`] (like `Block` into the block
/// table). `#[repr(u8)]`; the discriminant is the table index.
#[repr(u8)]
#[derive(Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum Mob {
    Owl,
    Sheep,
}

/// The population group a species belongs to. Natural spawning caps each group
/// independently across the loaded area (so the world can't fill with one kind),
/// alongside the per-species [`MobDef::cap`]. A [`Passive`] mob persists when far from
/// the player — it leaves the live set only by being saved into its unloading chunk,
/// rather than distance-despawning.
///
/// [`Passive`]: MobCategory::Passive
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum MobCategory {
    Passive,
}

impl MobCategory {
    /// The most individuals of this category that may exist in the loaded area at
    /// once; natural spawning stops the category here.
    pub fn cap(self) -> u32 {
        match self {
            MobCategory::Passive => 25,
        }
    }

    /// The distance (blocks) past which this category's mobs are culled from the live
    /// world when no player is near, or `None` for categories that persist while loaded.
    /// Passive mobs always persist — they leave the live set only by being saved into an
    /// unloading chunk. The per-mob despawn timer this would gate lives on [`Instance`].
    pub fn despawn_radius(self) -> Option<f32> {
        match self {
            MobCategory::Passive => None,
        }
    }
}

/// A species' natural-spawn site criteria. The spawner runs its universal checks
/// first (player distance, footing, headroom for the body) and only then asks the
/// species' rule, so a rule only describes what's *species-specific*: the biomes it
/// settles in and the blocks it will stand on. Declarative on purpose — adding a
/// species is data, not a new branch in the spawner.
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
}

/// How many individuals a successful natural spawn attempt tries to place near the
/// first valid site. Singleton species use `1..=1`; herd animals can request a
/// larger bounded group.
#[derive(Copy, Clone, Debug)]
pub struct SpawnGroup {
    pub min: u8,
    pub max: u8,
}

impl SpawnGroup {
    pub const fn singleton() -> Self {
        Self { min: 1, max: 1 }
    }

    pub const fn new(min: u8, max: u8) -> Self {
        Self { min, max }
    }

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

/// A mob's collision/render footprint: a centred AABB `half_width` across and
/// `height` tall, with the feet at the mob position.
#[derive(Copy, Clone, Debug)]
pub struct MobSize {
    pub half_width: f32,
    pub height: f32,
}

impl MobSize {
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
#[derive(Copy, Clone, Debug)]
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
/// stands, which way it faces, and how many ticks of coat regrowth remain (`0` = fully
/// coated). A live [`Instance`] projects to this when its chunk unloads (so it rides
/// that chunk's save record, like a dropped item) and is rebuilt from it on reload with
/// a fresh brain. Transient AI/physics state (velocity, health, animation, the despawn
/// timer) is deliberately *not* saved: a reloaded mob simply resumes wandering. The
/// shear-regrow counter IS saved — a shorn sheep must not reload with its wool back.
#[derive(Copy, Clone, Debug, PartialEq)]
pub struct SavedMob {
    pub kind: Mob,
    pub pos: Vec3,
    pub yaw: f32,
    pub shear_regrow: u32,
}

impl SavedMob {
    /// Capture a live mob's persisted fields.
    pub fn of(inst: &Instance) -> Self {
        Self {
            kind: inst.kind,
            pos: inst.pos,
            yaw: inst.yaw,
            shear_regrow: inst.shear_regrow(),
        }
    }
}

/// One row of the mob registry: everything that makes a species what it is. `model_src`
/// and `scale` feed the renderer; the rest drives the simulation. (`model_src` is compiled
/// once into the shared [`Model`](crate::bbmodel::Model) — see [`model`].)
pub struct MobDef {
    pub mob: Mob,
    /// Stable snake_case identity (e.g. `"owl"`), independent of any display name —
    /// the key a loot table is looked up by. Mirrors [`crate::item::ItemType::key`].
    pub key: &'static str,
    /// The embedded `.bbmodel` source (geometry + walk animation + texture). Read only at
    /// precache time (see [`model`]); at runtime the compiled
    /// [`Model`](crate::bbmodel::Model) is authoritative.
    pub model_src: &'static str,
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
    /// What shearing this species yields, or `None` for species that can't be shorn.
    pub shear: Option<ShearSpec>,
    /// Constructs this species' AI brain on spawn (its composed behaviors), reading
    /// whatever it needs (e.g. its [`Habitat`]) off the row.
    pub make_brain: fn(&'static MobDef) -> Brain,
}

/// The id-ordered registry table (one row per [`Mob`], indexed by `Mob as u8`).
pub static MOB_DEFS: &[MobDef] = &[
    MobDef {
        mob: Mob::Owl,
        key: "owl",
        model_src: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/owl.bbmodel"
        )),
        scale: 0.25,
        size: MobSize {
            half_width: 0.22,
            height: 0.7,
        },
        max_health: 4.0,
        walk_speed: 2.4,
        jump_speed: 7.2,
        turn_rate: 7.0,
        walk_anim_rate: 1.2,
        category: MobCategory::Passive,
        cap: 8,
        spawn: SpawnRule {
            // Owls settle in wooded country, perched on the canopy or down on grass.
            biomes: &[Biome::Forest, Biome::RedwoodForest],
            ground: &[Block::OakLeaves, Block::BirchLeaves, Block::Grass],
        },
        spawn_group: SpawnGroup::singleton(),
        wander: WanderTuning {
            // A new stroll every few seconds of standing around.
            chance_per_tick: 1.0 / 80.0,
            radius: 8,
            cohesion: None,
        },
        habitat: Habitat {
            // Open / arid / watery country an owl won't wander into.
            avoid: &[
                Biome::Savanna,
                Biome::Ocean,
                Biome::DeepOcean,
                Biome::Beach,
                Biome::Plains,
                Biome::Desert,
            ],
            // Drawn back to forest, so a strayed owl drifts home.
            prefer: &[Biome::Forest, Biome::RedwoodForest],
        },
        avoid_water: true,
        shear: None,
        make_brain: behavior::owl_brain,
    },
    MobDef {
        mob: Mob::Sheep,
        key: "sheep",
        model_src: include_str!(concat!(
            env!("CARGO_MANIFEST_DIR"),
            "/assets/models/sheep.bbmodel"
        )),
        scale: 0.0625,
        size: MobSize {
            half_width: 0.45,
            height: 1.3,
        },
        max_health: 8.0,
        walk_speed: 1.5,
        jump_speed: 7.2,
        turn_rate: 5.5,
        walk_anim_rate: 1.25,
        category: MobCategory::Passive,
        cap: 18,
        spawn: SpawnRule {
            biomes: SHEEP_TEMPERATE_BIOMES,
            ground: &[Block::Grass, Block::Podzol, Block::Dirt],
        },
        spawn_group: SpawnGroup::new(2, 5),
        wander: WanderTuning {
            // Sheep browse less restlessly than owls, but range a little farther as a herd.
            chance_per_tick: 1.0 / 100.0,
            radius: 10,
            cohesion: Some(WanderCohesion {
                companion: Mob::Sheep,
                search_radius_multiplier: 2,
            }),
        },
        habitat: Habitat {
            avoid: SHEEP_AVOID_BIOMES,
            prefer: SHEEP_TEMPERATE_BIOMES,
        },
        avoid_water: true,
        shear: Some(ShearSpec {
            drop: ItemType::Wool,
            min: 1,
            max: 3,
            regrow_min: 1200,
            regrow_max: 6000,
        }),
        make_brain: behavior::sheep_brain,
    },
];

const SHEEP_TEMPERATE_BIOMES: &[Biome] = &[
    Biome::Plains,
    Biome::Savanna,
    Biome::Forest,
    Biome::Swamp,
    Biome::Taiga,
    Biome::Foothills,
    Biome::Wetland,
    Biome::RedwoodForest,
    Biome::OldGrowthTaiga,
    Biome::CherryGrove,
    Biome::Meadow,
    Biome::WindsweptHills,
    Biome::WoodedHills,
    Biome::MountainEdge,
];

const SHEEP_AVOID_BIOMES: &[Biome] = &[
    Biome::Ocean,
    Biome::DeepOcean,
    Biome::Beach,
    Biome::River,
    Biome::Desert,
    Biome::DesertLakes,
    Biome::SnowyTundra,
    Biome::SnowyTaiga,
    Biome::SnowyPeaks,
    Biome::Grove,
    Biome::SnowySlopes,
    Biome::StonyPeaks,
];

#[inline]
pub fn from_id(id: u8) -> Mob {
    MOB_DEFS.get(id as usize).map_or(Mob::Owl, |d| d.mob)
}

#[inline]
pub fn def(mob: Mob) -> &'static MobDef {
    &MOB_DEFS[mob as usize]
}

/// Every species' compiled [`Model`](crate::bbmodel::Model), indexed by `Mob as usize` —
/// the in-memory golden asset, precached once on first use (compiling each `.bbmodel` →
/// `.llmob` on a cache miss, else fast-loading the `.llmob`) and shared by the renderer and
/// the simulation. After this builds, nothing in the running engine reads a `.bbmodel`.
static MODELS: LazyLock<Vec<Model>> = LazyLock::new(|| {
    MOB_DEFS
        .iter()
        .map(|d| {
            let m = d.mob;
            crate::asset_cache::load_or_compile::<Model>(d.key, d.model_src.as_bytes())
                .unwrap_or_else(|e| {
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
    &MODELS[mob as usize]
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
        for d in MOB_DEFS {
            assert_eq!(def(d.mob).mob, d.mob);
            assert_eq!(from_id(d.mob as u8), d.mob);
        }
        assert_eq!(from_id(u8::MAX), Mob::Owl, "out-of-range falls back to Owl");
    }

    #[test]
    fn all_mob_model_sources_parse() {
        for d in MOB_DEFS {
            let model = Model::load(d.model_src)
                .unwrap_or_else(|e| panic!("{} model should parse: {e}", d.key));
            assert!(
                !model.cubes.is_empty(),
                "{} model should have renderable geometry",
                d.key
            );
        }
    }
}
