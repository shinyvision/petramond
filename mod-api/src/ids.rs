//! Runtime registry ids crossing the ABI.

use serde::{Deserialize, Serialize};

/// A runtime block id — raw `u16` into the engine's registry. Dynamic content is
/// NAME-addressed (`mod_id:name` keys in the pack catalogs assign ids at load),
/// so numeric ids are stable within a session but never across sessions or
/// saves; mods must not persist them. Resolve ids from names at `mod_init` time
/// with [`RegistryCall::ResolveBlock`](crate::RegistryCall::ResolveBlock).
///
/// [`RegistryCall::ResolveBlock`]: crate::RegistryCall::ResolveBlock
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u16);

impl BlockId {
    /// Air is engine id 0 — the one numeric id frozen by contract (worldgen
    /// and the save format both rely on it).
    pub const AIR: BlockId = BlockId(0);
}

/// A runtime item id — same contract as [`BlockId`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub u16);

/// A runtime mob SPECIES id — same contract as [`BlockId`]. This identifies a
/// kind (`"petramond:sheep"`), never a live instance; live mobs are addressed
/// by their stable `u64` session id ([`MobSnapshot::id`]). Bridge species key
/// strings and ids with [`RegistryCall::ResolveMob`](crate::RegistryCall::ResolveMob) / [`RegistryCall::MobNames`](crate::RegistryCall::MobNames).
///
/// [`MobSnapshot::id`]: crate::MobSnapshot::id
/// [`RegistryCall::ResolveMob`]: crate::RegistryCall::ResolveMob
/// [`RegistryCall::MobNames`]: crate::RegistryCall::MobNames
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MobId(pub u8);

/// A connected session's player id — the one vocabulary for every ABI field
/// naming a player (per-player calls like [`PlayerCall::PlayerInput`](crate::PlayerCall::PlayerInput) /
/// [`EntityCall::MobMount`](crate::EntityCall::MobMount), rider lists, event payloads, damage sources).
/// Session-scoped like every runtime id: never persist it.
///
/// [`PlayerCall::PlayerInput`]: crate::PlayerCall::PlayerInput
/// [`EntityCall::MobMount`]: crate::EntityCall::MobMount
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct PlayerId(pub u8);

/// A runtime body CONDITION id (`conditions.json`, e.g. `"petramond:burning"`) —
/// same contract as [`BlockId`]. Bridge keys and ids with
/// [`RegistryCall::ResolveCondition`](crate::RegistryCall::ResolveCondition) / [`RegistryCall::ConditionNames`](crate::RegistryCall::ConditionNames); a condition's
/// stages are addressed by their index in the row, named by
/// [`ConditionInfoData::stages`].
///
/// [`RegistryCall::ResolveCondition`]: crate::RegistryCall::ResolveCondition
/// [`RegistryCall::ConditionNames`]: crate::RegistryCall::ConditionNames
/// [`ConditionInfoData::stages`]: crate::ConditionInfoData::stages
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct ConditionId(pub u8);
