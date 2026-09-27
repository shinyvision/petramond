use crate::{BlockId, RuntimeSide};

pub trait WorldView {
    fn block(&self, pos: [i32; 3]) -> Option<BlockId>;
    fn cell_kv(&self, pos: [i32; 3], key: &str) -> Option<Vec<u8>>;
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub enum SideWorld {
    Server,
    Replica,
}

impl SideWorld {
    pub fn current() -> Self {
        Self::for_client(crate::runtime_side() == RuntimeSide::Client)
    }

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
