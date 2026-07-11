//! Scheduling vocabulary: where mod tick systems and worldgen hooks attach.

use serde::{Deserialize, Serialize};

/// The engine's fixed-tick stages, in execution order (mirrors the engine's
/// stage list).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum Stage {
    Mining,
    Placement,
    Attack,
    Drops,
    Menu,
    PlayerDamage,
    WorldScheduled,
    NaturalBreaks,
    Pickup,
    Mobs,
    ItemPhysics,
    Spawning,
}

/// Which side of a [`Stage`] a tick system attaches to. At the boundary between
/// stage N and N+1, `After(N)` systems run before `Before(N+1)` systems.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum AttachSide {
    Before,
    After,
}

/// The worldgen pipeline's addressable stages, in execution order.
///
/// `Climate` assigns the per-column biome map; `Terrain` is the block fill plus
/// cave carve; `Underground` scatters ores/blobs; `Vegetation` places
/// single-block ground plants; `Trees` places the tree features. Features
/// ([`HostCall::RegisterWorldgenFeature`]) attach AFTER a stage (`Climate` is
/// not a valid feature attach point — it is column-level, before any blocks
/// exist); replacements ([`HostCall::RegisterStageReplacement`]) substitute the
/// engine stage itself.
///
/// [`HostCall::RegisterWorldgenFeature`]: crate::HostCall::RegisterWorldgenFeature
/// [`HostCall::RegisterStageReplacement`]: crate::HostCall::RegisterStageReplacement
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
pub enum WorldgenStage {
    Climate,
    Terrain,
    Underground,
    Vegetation,
    Trees,
}
