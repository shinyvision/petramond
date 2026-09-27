use serde::{Deserialize, Serialize};

use petramond_world::chunk::SectionPos;

pub use crate::world::replication::{
    BlockDrawEntry, CellKvEntry, ColumnPayload, LightPayload, SectionBlocks, SectionBytes,
    SectionLight, SectionPayload, SectionStatesPayload,
};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SectionCacheClaim {
    pub pos: SectionPos,
    pub hash: u64,
}

/// Shared cap for the client section cache and the server's belief map for that connection. Both
/// sides insert in the same order and drop the oldest first, so neither has to tell the other
/// what it evicted. Anything that slips gets fixed by `SectionCacheMiss`. About 4k sections is a
/// roomy ring at RD32 and a few hundred MB of replica at worst.
pub const SECTION_CACHE_CAP: usize = 4096;
