//! Payload structs and small vocabularies shared by calls and replies.

use serde::{Deserialize, Serialize};

use crate::ids::{BlockId, ItemId, MobId, PlayerId};

/// Maximum UTF-8 byte length of a named mob animation crossing the mod API.
/// The simulation stores and replicates active names, so the mechanism bounds
/// them independently of whether the mob's model recognizes the name.
pub const MAX_MOB_ANIM_NAME_BYTES: usize = 64;

/// Largest absolute named-animation phase accepted from a mod, in authored
/// animation seconds.
pub const MAX_MOB_ANIM_PHASE_MAGNITUDE: f32 = 1_000_000.0;

/// Largest absolute named-animation playback/seek rate accepted from a mod,
/// in authored animation seconds per real second.
pub const MAX_MOB_ANIM_RATE_MAGNITUDE: f32 = 1_000.0;

/// One value of the open GUI session's state map. Written by mods
/// on the tick ([`HostCall::GuiStateSet`]); read per frame by the renderer to
/// drive `label` text, `rotimage` angles (radians, `F32`), and mod overlay
/// fractions. Keys are mod-local: the map belongs to one GUI session (cleared
/// on open/close), so no namespace prefix is enforced.
///
/// [`HostCall::GuiStateSet`]: crate::HostCall::GuiStateSet
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuiValue {
    F32(f32),
    I32(i32),
    Str(String),
}

/// One value in a live mob's tag map. Engine tags use the `petramond:`
/// namespace (e.g., `petramond:confined`); mods may invent `mod_id:` keys.
/// Tags persist with the mob and are visible to AI.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobTagValue {
    Bool(bool),
    I64(i64),
    F64(f64),
    Str(String),
}

/// The outcome of [`HostCall::MobTagGet`](crate::HostCall::MobTagGet): a mob
/// that is GONE (dead, unloaded, never spawned) is told apart from a live mob
/// simply not carrying the key — the two mean different things to a mod
/// (retry vs. store), so they are never conflated.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum MobTagLookup {
    /// No such LIVE mob (dead, unloaded, or never existed).
    MissingMob,
    /// The mob is live but carries nothing under the key.
    Absent,
    /// The mob carries this value under the key.
    Value(MobTagValue),
}

/// A live mob's snapshot for [`HostCall::MobsInRadius`] /
/// [`HostCall::MobsWithTag`]. The mob's ADDRESS is the stable
/// [`id`](Self::id) — every mob call and event payload speaks it
/// (see the mob-addressing note on [`HostCall`](crate::HostCall)). `index` is
/// only an intra-tick JOIN key against other snapshots taken this tick; it is
/// never accepted by a call and renumbers on any removal.
///
/// [`HostCall::MobsInRadius`]: crate::HostCall::MobsInRadius
/// [`HostCall::MobsWithTag`]: crate::HostCall::MobsWithTag
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobSnapshot {
    /// Live-set list position THIS TICK — an intra-tick join key only, never
    /// an address (calls take [`id`](Self::id)).
    pub index: u32,
    /// The species' key (`"petramond:owl"`, `"zombies:zombie"`).
    pub key: String,
    /// The species' session id — the compact form of `key`, matching the
    /// `kind` in event payloads ([`EventPayload::MobDied`] etc.); bridge with
    /// [`HostCall::ResolveMob`] / [`HostCall::MobNames`].
    ///
    /// [`EventPayload::MobDied`]: crate::EventPayload::MobDied
    /// [`HostCall::ResolveMob`]: crate::HostCall::ResolveMob
    /// [`HostCall::MobNames`]: crate::HostCall::MobNames
    pub kind: MobId,
    /// Feet position.
    pub pos: [f32; 3],
    pub health: f32,
    /// Stable session id for this live mob — THE mob address, held across
    /// ticks. It survives unrelated removals; it is not a species id and is
    /// not promised stable across save/load.
    pub id: u64,
    /// Body facing, radians about +Y. MOB convention: yaw `0` faces `-Z`,
    /// so the facing direction is `(-sin yaw, 0, -cos yaw)` — the same frame
    /// [`HostCall::MobDrive`] yaws speak.
    ///
    /// [`HostCall::MobDrive`]: crate::HostCall::MobDrive
    pub yaw: f32,
    /// Current velocity (m/s). Read-only; steer through
    /// [`HostCall::MobDrive`].
    ///
    /// [`HostCall::MobDrive`]: crate::HostCall::MobDrive
    pub vel: [f32; 3],
}

/// One rider of a mob, for [`HostCall::MobRiders`].
///
/// [`HostCall::MobRiders`]: crate::HostCall::MobRiders
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct MobRiderData {
    /// Seat index into the species' `seats` row list.
    pub seat: u8,
    /// The riding session.
    pub player_id: PlayerId,
}

/// Seat declaration and current occupants of one live mob, for
/// [`HostCall::MobRiders`].
///
/// [`HostCall::MobRiders`]: crate::HostCall::MobRiders
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct MobRidersData {
    /// Number of seats declared by the mob's species row. Valid seat indices
    /// are `0..capacity`.
    pub capacity: u8,
    /// Current occupants, in player-id order.
    pub riders: Vec<MobRiderData>,
}

/// Authoritative playback state of one active named mob animation, for
/// [`HostCall::MobAnimState`].
///
/// [`HostCall::MobAnimState`]: crate::HostCall::MobAnimState
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct MobAnimStateData {
    /// Absolute authored-animation phase in seconds.
    pub phase: f32,
    /// Current playback rate. While seeking this is the non-negative approach
    /// rate; after landing it is `0`.
    pub rate: f32,
    /// Absolute seek target, or `None` during ordinary rate-driven playback.
    pub seek: Option<f32>,
}

/// One player's movement intent this tick, for [`HostCall::PlayerInput`] —
/// decomposed into the player's own yaw frame so a driving mod never touches
/// the world-space wish plumbing.
///
/// [`HostCall::PlayerInput`]: crate::HostCall::PlayerInput
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct PlayerInputData {
    /// Forward(+)/back(−) along the player's facing, `[-1, 1]`.
    pub forward: f32,
    /// Right(+)/left(−) strafe, `[-1, 1]`.
    pub strafe: f32,
    pub jump: bool,
    pub sneak: bool,
    /// The player's look. PLAYER convention: yaw `0` faces `+Z` (facing
    /// `(sin yaw, 0, cos yaw)`) — π apart from the mob yaw convention; a mod
    /// aligning a mount to its rider adds π.
    pub yaw: f32,
    pub pitch: f32,
}

/// The player's state for [`HostCall::PlayerState`].
///
/// [`HostCall::PlayerState`]: crate::HostCall::PlayerState
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerSnapshot {
    /// Feet position.
    pub pos: [f32; 3],
    pub vel: [f32; 3],
    /// Look direction, radians (yaw about +Y, pitch clamped short of vertical).
    pub yaw: f32,
    pub pitch: f32,
    /// Half-heart points (`0..=20`).
    pub health: i32,
    pub on_ground: bool,
    pub spectator: bool,
}

/// One entry of [`HostCall::Players`]: a connected player's session id plus
/// their state snapshot.
///
/// [`HostCall::Players`]: crate::HostCall::Players
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct PlayerListEntry {
    /// The session's player id — the value per-player calls
    /// (`PlayerInput`, `MobMount`) address.
    pub id: PlayerId,
    pub state: PlayerSnapshot,
}

/// One core-selected candidate for programmatic hostile spawning. The engine
/// owns physical site selection; registered hostile spawners decide whether a
/// specific hostile species admits this site.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct HostileSpawnCandidate {
    /// Feet position, centered in the candidate cell.
    pub pos: [f32; 3],
    /// Feet cell.
    pub cell: [i32; 3],
    /// Cached light channels on the 6-bit `0..=63` scale.
    pub combined_light: u8,
    pub sky_light: u8,
    pub block_light: u8,
    /// Distance (blocks) from this site to the NEAREST connected player — the
    /// multiplayer-correct input for proximity spawn rules (the host-session
    /// `PlayerState` snapshot only sees one player).
    pub nearest_player_dist: f32,
}

/// Which isolated runtime instance is executing this module. Server and
/// worldgen instances are deterministic simulation runtimes; `Client` is a
/// presentation-only instance with read-only replica queries and sandboxed
/// client storage.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum RuntimeSide {
    Server,
    Worldgen,
    Client,
}

/// One item stack crossing the ABI: the item's registry NAME (the one
/// mod-facing item identity — see the identity note on
/// [`HostCall`](crate::HostCall)) + count.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemStackData {
    /// Registry name (`"petramond:coal"`, `"kitchen:raw_mutton"`).
    pub item: String,
    pub count: u8,
}

/// One item's registry row (see [`HostCall::ItemInfo`]) — the stable,
/// mod-relevant fields of its `items.json` row, the same data engine
/// mechanics read. Presentation internals (sprite/model/held pose) stay
/// engine-side. Session-stable: cache it mod-side, never re-ask per tick.
///
/// [`HostCall::ItemInfo`]: crate::HostCall::ItemInfo
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemInfoData {
    /// Effective per-slot stack cap (durable items — tools — never stack).
    pub max_stack: u8,
    /// Fuel burn duration in game ticks; `0` = not a fuel. Any machine may
    /// consume it (the furnace reads exactly this field).
    pub fuel_burn_ticks: u32,
    /// The item's tag names (engine tags bare, pack tags namespaced).
    pub tags: Vec<String>,
    /// Human-readable display name (UI text only — never an identity).
    pub display_name: String,
    /// Session id of the block this item places (the row's `block` link), or
    /// `None` for an item-only item (tools, raw drops, ingots). Compare
    /// against `get_block` reads; resolve a name via `BlockNames`.
    pub block: Option<BlockId>,
    /// The mining tool this item acts as, or `None`.
    pub tool: Option<ToolInfoData>,
    /// Edible-item data, or `None` for non-food.
    pub food: Option<FoodInfoData>,
    /// The ENGINE use handler the row declares (`"bucket_fill"`,
    /// `"bucket_pour"`, `"shear"`), or `None`. Mods react to any item's use
    /// through `item_use_pre` — this field only reveals engine-handled uses.
    pub item_use: Option<String>,
}

/// An item's mining-tool row data (see [`ItemInfoData::tool`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ToolInfoData {
    /// Tool family: `"pickaxe"`, `"axe"`, `"shovel"`, or `"shears"`.
    pub kind: String,
    /// Material tier `1..=4` (wooden, stone, iron, diamond).
    pub tier: u8,
}

/// An item's edible row data (see [`ItemInfoData::food`]).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FoodInfoData {
    /// Game ticks of held-button eating before the item is consumed.
    pub eat_ticks: u32,
    /// Status effects granted when the eat completes.
    pub effects: Vec<FoodEffectData>,
}

/// One granted food effect: an `effects.json` registry key + duration.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FoodEffectData {
    pub effect: String,
    pub ticks: u32,
}

/// Which [`BlockBehavior`](crate::GuestCall::BlockBehavior) hook fired — the mod-side
/// mirror of the engine `BlockBehavior` trait's methods.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum BlockHookKind {
    /// The probabilistic per-section random tick (a few cells per section per
    /// game tick). Mod-behavior blocks always receive random ticks.
    RandomTick,
    /// A scheduled tick previously requested via [`HostCall::ScheduleTick`].
    ///
    /// [`HostCall::ScheduleTick`]: crate::HostCall::ScheduleTick
    ScheduledTick,
    /// The cell or one of its 6 neighbours changed (the ANNOUNCE phase).
    NeighborUpdate,
}

/// One active status effect crossing the ABI (see [`HostCall::EffectsActive`]).
///
/// [`HostCall::EffectsActive`]: crate::HostCall::EffectsActive
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct EffectStateData {
    /// The effect's registry key (`"petramond:regeneration"`, `"mod_id:haste"`).
    pub key: String,
    /// Remaining game ticks.
    pub remaining: u32,
}

/// Cached light at a loaded cell (see [`HostCall::LightAt`]), all on the
/// renderer's 6-bit `0..=63` scale; `combined = max(sky, block)`.
///
/// [`HostCall::LightAt`]: crate::HostCall::LightAt
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct LightData {
    pub combined: u8,
    pub sky: u8,
    pub block: u8,
}

/// The collision-shape CLASS of a world cell (see
/// [`HostCall::CollisionShapeAt`]) — generic physics with no gameplay policy
/// baked in. Spawn/placement rules compose on top of it in mod code (e.g.
/// `Full` + not water + not tagged `petramond:leaves`).
///
/// [`HostCall::CollisionShapeAt`]: crate::HostCall::CollisionShapeAt
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum CollisionShape {
    /// No collision boxes: air, water, walk-through cover (tall grass).
    Empty,
    /// Collision boxes that do not amount to one full unit cube: stairs,
    /// slabs, doors, snow layers, model blocks.
    Partial,
    /// Exactly one collision box spanning the whole unit cell.
    Full,
}

/// The read-only mob snapshot an [`GuestCall::AiNode`] decision sees.
///
/// The baseline fields (the mob's own state, the current tick, and the
/// nearest player's id/position) are always present. Fact fields beyond the
/// baseline are DECLARED INPUTS: the brain node row lists the facts its node
/// reads (`"inputs": ["player_held"]` in `mobs.json`), and only declared
/// facts are computed and shipped — an undeclared fact always reads `None`.
/// Every `player_*` fact describes the SAME player, [`player_id`]
/// (the nearest one), mutually consistent within a dispatch.
///
/// [`player_id`]: AiNodeCtx::player_id
/// [`GuestCall::AiNode`]: crate::GuestCall::AiNode
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct AiNodeCtx {
    /// Stable id of the deciding mob — key per-mob guest state off it.
    pub mob_id: u64,
    /// Mob feet position (world space).
    pub pos: [f32; 3],
    /// Mob foothold voxel.
    pub cell: [i32; 3],
    /// Body facing (radians).
    pub yaw: f32,
    /// The current game tick — the same value `current_tick()` returns
    /// (dispatch runs once per owning mob per game tick), carried here so
    /// timekeeping costs no host call.
    pub tick: u64,
    /// Session id of the NEAREST player — the player every `player_*` fact
    /// in this snapshot describes, and the target of an attack decision.
    pub player_id: PlayerId,
    /// That player's body-centre (world space).
    pub player_pos: [f32; 3],
    /// True when the navigator has no active path ("the mob is idle").
    pub nav_idle: bool,
    /// True when the mob's body is in water.
    pub in_water: bool,
    /// DECLARED INPUT `"player_held"`: the nearest player's selected (held)
    /// item — resolve names via `ResolveItem` and compare (a lure, a beg, a
    /// trade gate all read this same fact). `None` when the input is
    /// undeclared, the hand is empty, or the player is a spectator.
    pub player_held: Option<ItemId>,
    /// DECLARED INPUT `"player_foothold"`: the mob-standable navigation
    /// foothold nearest that player (what the engine's `chase_player` paths
    /// toward) — the ready-made `goal` for any follow/approach node. `None`
    /// when the input is undeclared, the player is airborne or has no
    /// reachable foothold, or the player is more than 32 blocks away (the
    /// outer edge of player-reactive mob AI — the scan is skipped past it).
    pub player_foothold: Option<[i32; 3]>,
}

/// One scripted node's contribution to a mob's tick. Every field defaults to
/// "no opinion"; the engine keeps the highest-priority non-`None` value per
/// field across the whole brain (scripted and engine nodes alike).
#[derive(Serialize, Deserialize, Clone, Copy, Debug, Default, PartialEq)]
pub struct AiNodeDecision {
    /// A navigation destination (world voxel) to path toward.
    pub goal: Option<[i32; 3]>,
    /// A desired head orientation `[yaw, pitch]` relative to the body.
    pub head_look: Option<[f32; 2]>,
    /// An `idle_*` animation index to play.
    pub idle_anim: Option<u8>,
    /// A melee strike `[damage, knockback]` to land on the player this tick.
    pub attack: Option<[f32; 2]>,
}
