//! Runtime registry ids crossing the ABI.

use serde::{Deserialize, Serialize};

/// A runtime block id — raw `u8` into the engine's registry. Dynamic content is
/// NAME-addressed (`mod_id:name` keys in the pack catalogs assign ids at load),
/// so numeric ids are stable within a session but never across sessions or
/// saves; mods must not persist them. Resolve ids from names at `mod_init` time
/// with [`HostCall::ResolveBlock`].
///
/// [`HostCall::ResolveBlock`]: crate::HostCall::ResolveBlock
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct BlockId(pub u8);

impl BlockId {
    /// Air is engine id 0 — the one numeric id frozen by contract (worldgen
    /// and the save format both rely on it).
    pub const AIR: BlockId = BlockId(0);
}

/// A runtime item id — same contract as [`BlockId`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct ItemId(pub u8);

/// A runtime mob species id — same contract as [`BlockId`].
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub struct MobId(pub u8);
