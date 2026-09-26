//! One read surface over "the world this instance sees", so a handler's
//! GATE is written once and runs on both instances of a mod.
//!
//! The server instance reads the authoritative world ([`get_block`],
//! [`section_kv_get`]); the client instance predicts against the replica
//! ([`client_blocks_at`], [`client_cell_kv_at`]). A gate generic over
//! [`WorldView`] is the SAME code on both: the server handler runs it and
//! then mutates, the client predictor runs it and answers the claim — the
//! prediction cannot drift from the authority because there is nothing
//! left to keep in sync. Both reads answer `None` for a cell the instance
//! cannot inspect (unloaded, not stream-final), which a gate must treat as
//! "not actionable", never as evidence.
//!
//! [`get_block`]: crate::get_block
//! [`section_kv_get`]: crate::section_kv_get
//! [`client_blocks_at`]: crate::client_blocks_at
//! [`client_cell_kv_at`]: crate::client_cell_kv_at

use crate::{BlockId, RuntimeSide};

/// The world reads a handler gate makes.
pub trait WorldView {
    /// The block at a world cell; `None` = not inspectable right now.
    fn block(&self, pos: [i32; 3]) -> Option<BlockId>;
    /// One per-cell KV entry; `None` = absent or not inspectable.
    fn cell_kv(&self, pos: [i32; 3], key: &str) -> Option<Vec<u8>>;
}

/// The world as the executing instance sees it: the authoritative world on
/// the server, the replica on the client. Resolve it once per dispatch with
/// [`SideWorld::current`] (or from a side flag the mod already holds).
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideWorld {
    /// The server instance: authoritative reads.
    Server,
    /// The client instance: replica reads (prediction).
    Replica,
}

impl SideWorld {
    /// The view for the runtime executing this code.
    pub fn current() -> Self {
        Self::for_client(crate::runtime_side() == RuntimeSide::Client)
    }

    /// The view for an instance that already knows its side.
    pub fn for_client(client: bool) -> Self {
        if client {
            SideWorld::Replica
        } else {
            SideWorld::Server
        }
    }
}

impl WorldView for SideWorld {
    fn block(&self, pos: [i32; 3]) -> Option<BlockId> {
        match self {
            SideWorld::Server => crate::get_block(pos),
            SideWorld::Replica => crate::client_blocks_at(vec![pos])
                .into_iter()
                .next()
                .flatten(),
        }
    }

    fn cell_kv(&self, pos: [i32; 3], key: &str) -> Option<Vec<u8>> {
        match self {
            SideWorld::Server => crate::section_kv_get(pos, key),
            SideWorld::Replica => crate::client_cell_kv_at(key, vec![pos])
                .into_iter()
                .next()
                .flatten(),
        }
    }
}
