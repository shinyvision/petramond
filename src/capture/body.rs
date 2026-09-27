//! The ONE codec of every piece body: what the capture writers put inside a
//! piece and what a presentation reads back out.
//!
//! A body is one protocol frame (`net::framing`'s `[u32 len][u8 flags][bytes]`,
//! bit 0 of the flags = zlib) around a postcard value. DOCUMENTED bodies are
//! `mod_api::capture`'s own types and are never compressed; the opaque ones
//! are the engine's replication encodings below, opaque to mods by design.
//!
//! | Kind | Body |
//! | --- | --- |
//! | `Section` | [`SectionPayload`] |
//! | `Column` | [`ColumnPayload`] |
//! | `Tables` | [`NameTables`] |
//! | `Mob`, `Item`, `Player` | [`MobStateRow`](crate::net::protocol::MobStateRow), [`ItemStateRow`](crate::net::protocol::ItemStateRow), [`PlayerStateRow`](crate::net::protocol::PlayerStateRow) |
//! | `Activity` | [`CapturedActivity`] |
//! | `Viewer` | [`SelfState`](crate::net::protocol::SelfState) |
//! | `Message` | [`ServerToClient`] |
//! | `BatchWorld`, `BatchRows`, `BatchRest` | [`BatchWorld`], [`BatchRows`], [`BatchRest`] |
//! | `Cues` | [`CapturedCues`] |
//! | `View` | [`ViewCue`] |

use std::io::{Read, Write};

use mod_api::capture::{
    crc32, write_piece_head, CapturePieceHead, ClientPieceKind, ClientStateKey,
    CAPTURE_PIECE_HEAD_LEN,
};
use petramond_math::math::IVec3;
use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::net::framing::MAX_FRAME;
use crate::net::protocol::{
    BlockDelta, BlockDrawDelta, CellKvDelta, ItemLane, MobLane, PlayerLane, SpatialSoundMsg,
    TickSection, TickUpdate, WorldEventMsg,
};

pub use super::view::ViewCue;
pub use crate::net::protocol::{ColumnPayload, NameTables, SectionPayload, ServerToClient};

/// Bodies above this many bytes are zlib-compressed when that shrinks them.
const COMPRESS_MIN: usize = 1024;

/// The spatial loops sounding, the local player's dig cell, the open containers.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapturedActivity {
    /// Each sounding loop as the command that started it.
    pub loops: Vec<SpatialSoundMsg>,
    pub dig: Option<IVec3>,
    pub open_chests: Vec<IVec3>,
}

/// A tick batch's terrain writes.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchWorld {
    pub block_deltas: Vec<BlockDelta>,
    pub cell_kv_deltas: Vec<CellKvDelta>,
    pub block_draws: Vec<BlockDrawDelta>,
}

/// A tick batch's entity lanes, or one slice of them when they would pass
/// one protocol frame. A lane moves the tracked set on as the batch did:
/// despawns, then spawns, then updates.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchRows {
    pub mobs: MobLane,
    pub items: ItemLane,
    pub players: PlayerLane,
}

impl BatchRows {
    /// Despawned ids and rows, over every lane.
    pub fn entries(&self) -> usize {
        let lane = |despawned: usize, rows: usize| despawned + rows;
        lane(self.mobs.despawned.len(), self.mobs.len())
            + lane(self.items.despawned.len(), self.items.len())
            + lane(self.players.despawned.len(), self.players.len())
    }
}

/// The rest of a tick batch's world content: every section a presentation
/// replays that is neither terrain nor an entity lane, in batch order.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct BatchRest {
    pub tick: u64,
    pub clock: u64,
    pub sections: Vec<TickSection>,
}

/// The local predictions a frame presented: the predicted place/break
/// events, and the dig cell as it then stood.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct CapturedCues {
    pub events: Vec<WorldEventMsg>,
    pub dig: Option<IVec3>,
}

/// Why a body did not encode or decode.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BodyError(pub String);

impl std::fmt::Display for BodyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

/// `value` as one protocol frame; `compress` only for opaque bodies.
pub fn encode_body<T: Serialize>(value: &T, compress: bool) -> Result<Vec<u8>, BodyError> {
    let raw = postcard::to_allocvec(value).map_err(|e| BodyError(e.to_string()))?;
    if raw.len() > MAX_FRAME {
        return Err(BodyError(format!(
            "a body of {} bytes passes one protocol frame",
            raw.len()
        )));
    }
    let (flags, bytes) = if compress && raw.len() > COMPRESS_MIN {
        let mut enc = flate2::write::ZlibEncoder::new(Vec::new(), flate2::Compression::fast());
        enc.write_all(&raw).map_err(|e| BodyError(e.to_string()))?;
        let packed = enc.finish().map_err(|e| BodyError(e.to_string()))?;
        if packed.len() < raw.len() {
            (1u8, packed)
        } else {
            (0, raw)
        }
    } else {
        (0, raw)
    };
    let mut frame = Vec::with_capacity(5 + bytes.len());
    frame.extend_from_slice(&(bytes.len() as u32).to_le_bytes());
    frame.push(flags);
    frame.extend_from_slice(&bytes);
    Ok(frame)
}

/// Decode one body frame, inflating at most one protocol frame.
pub fn decode_body<T: DeserializeOwned>(frame: &[u8]) -> Result<T, BodyError> {
    let (flags, bytes) =
        mod_api::capture::split_frame(frame).map_err(|e| BodyError(e.to_string()))?;
    let inflated;
    let bytes = if flags & 1 != 0 {
        let mut out = Vec::new();
        flate2::read::ZlibDecoder::new(bytes)
            .take(MAX_FRAME as u64 + 1)
            .read_to_end(&mut out)
            .map_err(|e| BodyError(e.to_string()))?;
        if out.len() > MAX_FRAME {
            return Err(BodyError("a body inflates past one protocol frame".into()));
        }
        inflated = out;
        &inflated[..]
    } else {
        bytes
    };
    postcard::from_bytes(bytes).map_err(|e| BodyError(e.to_string()))
}

/// The postcard bytes the vocabulary hash is taken over.
pub fn vocabulary_of(tables: &NameTables) -> u64 {
    mod_api::capture::fnv1a64(&postcard::to_allocvec(tables).expect("name tables encode"))
}

/// A whole piece: head and `body` (one frame from [`encode_body`]).
pub fn piece(
    kind: ClientPieceKind,
    key: [u8; 12],
    provisional: bool,
    vocabulary: u64,
    body: &[u8],
) -> Vec<u8> {
    let head = CapturePieceHead {
        protocol: crate::net::PROTOCOL_VERSION,
        kind,
        provisional,
        body_len: body.len() as u32,
        body_crc: crc32(body),
        key,
        vocabulary,
    };
    let mut out = Vec::with_capacity(CAPTURE_PIECE_HEAD_LEN + body.len());
    out.extend_from_slice(&write_piece_head(&head));
    out.extend_from_slice(body);
    out
}

/// A state piece stating `key` with `value` as its body.
pub fn state_piece<T: Serialize>(
    key: ClientStateKey,
    value: &T,
    vocabulary: u64,
) -> Result<Vec<u8>, BodyError> {
    let kind = key.kind();
    let body = encode_body(value, !kind.documented())?;
    Ok(piece(kind, key.key_bytes(), false, vocabulary, &body))
}

/// A tick batch split by role into its three bodies. The recipient's chrome
/// (request answers, creative and schematic replies, menu sync) is in none
/// of them.
pub fn split_batch(t: &TickUpdate) -> (BatchWorld, BatchRows, BatchRest) {
    let mut world = BatchWorld::default();
    let mut rows = BatchRows::default();
    let mut rest = BatchRest {
        tick: t.tick,
        clock: t.clock,
        sections: Vec::new(),
    };
    for section in &t.sections {
        match section {
            TickSection::BlockDeltas(d) => world.block_deltas.extend(d.iter().cloned()),
            TickSection::CellKvDeltas(d) => world.cell_kv_deltas.extend(d.iter().cloned()),
            TickSection::BlockDraws(d) => world.block_draws.extend(d.iter().cloned()),
            TickSection::Mobs(lane) => rows.mobs = lane.clone(),
            TickSection::Items(lane) => rows.items = lane.clone(),
            TickSection::Players(lane) => rows.players = lane.clone(),
            TickSection::PlayerActions(_)
            | TickSection::SleepTally(_)
            | TickSection::SelfState(_)
            | TickSection::Env(_)
            | TickSection::OpenChests(_)
            | TickSection::Events(_)
            | TickSection::SelfEvents(_) => rest.sections.push(section.clone()),
            TickSection::Creative(_)
            | TickSection::Schematics(_)
            | TickSection::ActionOutcomes(_)
            | TickSection::MenuSync(_) => {}
        }
    }
    (world, rows, rest)
}
