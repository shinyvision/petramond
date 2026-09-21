//! Sharing schematic assets between a client and a server: the messages that
//! choose and position a design for a requester on the other side. The
//! archives themselves cross as blob streams (`net::blob`).

use super::store::Digest;
pub use crate::net::blob::{BlobPacket, BlobReceiver, BlobSender};
use serde::{Deserialize, Serialize};

/// Where an anchored design stands: its asset and whole-block transform.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GhostPlacement {
    pub digest: Digest,
    pub origin: [i32; 3],
    pub turns: u8,
    /// Not drawn while its viewer positions a design under the same key.
    pub yields_to_positioning: bool,
}

/// Client → server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchematicRequest {
    /// The player chose `digest` for the open choice `tag`.
    Chosen {
        tag: String,
        digest: Digest,
    },
    /// The player anchored the design for the open positioning `tag`.
    Positioned {
        tag: String,
        digest: Digest,
        origin: [i32; 3],
        turns: u8,
    },
    /// Send me the archive of `digest`.
    Fetch {
        digest: Digest,
    },
    Blob(BlobPacket),
}

/// Server → client.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SchematicNotice {
    /// Open the schematic library for a choice named `tag`.
    Choose {
        tag: String,
    },
    /// Position `digest` for `tag`, starting at `origin` when given.
    Position {
        tag: String,
        digest: Digest,
        origin: Option<[i32; 3]>,
        turns: u8,
    },
    /// Send the archive of `digest`: the world does not hold it yet.
    Want {
        digest: Digest,
    },
    /// The anchored ghost `key` now shows `placement`, or nothing.
    Ghost {
        key: String,
        placement: Option<GhostPlacement>,
    },
    /// A request could not be served; `message` says why.
    Refused {
        message: String,
    },
    Blob(BlobPacket),
}
