use serde::{Deserialize, Serialize};

use petramond_world::chunk::SectionPos;

// The payload value types are world-owned (see `world::replication`); the
// wire protocol carries them as-is.
pub use crate::world::replication::{
    BlockDrawEntry, CellKvEntry, ColumnPayload, LightPayload, SectionBlocks, SectionBytes,
    SectionLight, SectionPayload, SectionStatesPayload,
};

/// One cached section a joining client claims to still hold, by the
/// server-domain content hash the server vouched at unload time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionCacheClaim {
    pub pos: SectionPos,
    pub hash: u64,
}

/// Entry cap for the client section cache AND the server's per-connection
/// belief map. Both sides insert in the same order (unloads ride the ordered
/// stream) and evict oldest-first, so the two stay aligned without eviction
/// chatter; any residual drift heals through `SectionCacheMiss`. ~4k sections
/// ≈ a generous re-explorable ring at RD32 while bounding worst-case replica
/// memory to a few hundred MB.
pub const SECTION_CACHE_CAP: usize = 4096;
