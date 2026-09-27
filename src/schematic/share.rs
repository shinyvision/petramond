use super::store::Digest;
pub use crate::net::blob::{BlobPacket, BlobReceiver, BlobSender};
use serde::{Deserialize, Serialize};

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostPlacement {
    pub digest: Digest,
    pub origin: [i32; 3],
    pub turns: u8,
    pub yields_to_positioning: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchematicRequest {
    Chosen {
        tag: String,
        digest: Digest,
    },
    Positioned {
        tag: String,
        digest: Digest,
        origin: [i32; 3],
        turns: u8,
    },
    Fetch {
        digest: Digest,
    },
    Blob(BlobPacket),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchematicNotice {
    Choose {
        tag: String,
    },
    Position {
        tag: String,
        digest: Digest,
        origin: Option<[i32; 3]>,
        turns: u8,
    },
    Want {
        digest: Digest,
    },
    Ghost {
        key: String,
        placement: Option<GhostPlacement>,
    },
    Refused {
        message: String,
    },
    Blob(BlobPacket),
}
