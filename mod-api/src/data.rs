//! Payload structs and small vocabularies shared by calls and replies.

use serde::{Deserialize, Serialize};

/// One value of the open GUI session's state map (Phase 5). Written by mods
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

/// A live mob's snapshot for [`HostCall::MobsInRadius`]. `index` addresses the
/// mob in later calls ([`HostCall::DamageMob`], the mob KV calls) and is valid
/// THIS TICK ONLY — any engine mob removal (deaths finishing, despawns, section
/// unloads, [`HostCall::DespawnMob`]) renumbers; re-query, never store indices.
///
/// [`HostCall::MobsInRadius`]: crate::HostCall::MobsInRadius
/// [`HostCall::DamageMob`]: crate::HostCall::DamageMob
/// [`HostCall::DespawnMob`]: crate::HostCall::DespawnMob
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct MobSnapshot {
    pub index: u32,
    /// The species' key (`"petramond:owl"`, `"zombies:zombie"`).
    pub key: String,
    /// Feet position.
    pub pos: [f32; 3],
    pub health: f32,
    /// Stable session id for this live mob. Unlike `index`, this survives
    /// unrelated `swap_remove` renumbering; it is not a species id and is not
    /// promised stable across save/load.
    pub id: u64,
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

/// One item stack crossing the ABI: the item's stable registry key + count.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemStackData {
    pub key: String,
    pub count: u8,
}

/// One item's registry data (see [`HostCall::ItemInfo`]).
///
/// [`HostCall::ItemInfo`]: crate::HostCall::ItemInfo
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ItemInfoData {
    pub max_stack: u8,
    /// Furnace-fuel burn duration in game ticks; `0` = not a fuel.
    pub fuel_burn_ticks: u32,
    /// The item's tag names (engine tags bare, pack tags namespaced).
    pub tags: Vec<String>,
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

/// The read-only mob snapshot an [`GuestCall::AiNode`] decision sees.
///
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
    /// Player body-centre (world space).
    pub player_pos: [f32; 3],
    /// True when the navigator has no active path ("the mob is idle").
    pub nav_idle: bool,
    /// True when the mob's body is in water.
    pub in_water: bool,
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
