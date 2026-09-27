//! The capture format: what the engine writes into a mod's files when it
//! captures a presented world (`ClientWorldStateWrite`, `ClientWorldEventsBegin`)
//! and reads back when it presents one (`ClientPresentationApply`).
//!
//! A capture file is whatever the mod makes of it. The engine appends
//! self-delimiting RECORDS where the mod asks, and never reads outside the
//! ranges a mod hands it back. This module is the ONE implementation of the
//! container's layout: the engine's writer and reader call it, and mods read
//! with it too (`mod_sdk::capture`). It is pure: no host calls, and no
//! allocation beyond the decoded value.
//!
//! All integers are little-endian.
//!
//! A RECORD is a 40-byte head, then its pieces and its envelope:
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `PMCR` |
//! | 4 | 2 | `format` ([`CAPTURE_FORMAT`]) |
//! | 6 | 1 | `kind`: 1 State, 2 Frame |
//! | 7 | 1 | `flags`: bit 0 = complete |
//! | 8 | 8 | `len`: the whole record, head included (0 while incomplete) |
//! | 16 | 8 | `envelope_at`: the envelope's offset from the record's start |
//! | 24 | 8 | `envelope_len` |
//! | 32 | 4 | `envelope_crc`: CRC-32 of the envelope bytes |
//! | 36 | 4 | reserved, 0 |
//!
//! A Frame record is head · envelope · pieces, written in one write. A State
//! record is head · pieces · envelope; it is streamed, so its head is written
//! incomplete first and rewritten in place once the envelope has landed.
//!
//! A PIECE is a 40-byte head and one body, addressable and checkable on its
//! own: `[piece offset, 40 + body_len]` is a range a mod may keep and hand
//! back to a presentation.
//!
//! | Offset | Size | Field |
//! | --- | --- | --- |
//! | 0 | 4 | magic `PMCP` |
//! | 4 | 2 | `format` |
//! | 6 | 2 | `protocol`: the opaque bodies' encoding |
//! | 8 | 1 | `kind` ([`ClientPieceKind`]) |
//! | 9 | 1 | `flags`: bit 0 = provisional |
//! | 10 | 2 | reserved, 0 |
//! | 12 | 4 | `body_len` |
//! | 16 | 4 | `body_crc`: CRC-32 of the body bytes as stored |
//! | 20 | 12 | key: a section's `x, y, z` (i32), a column's `x, z`, an entity's id (u64), a player's id (u8), a batch piece's tick (u64); zeros otherwise |
//! | 32 | 8 | `vocabulary`: [`fnv1a64`] of the postcard encoding of the id tables |
//!
//! A body is exactly one protocol frame, `[u32 len][u8 flags, bit 0 zlib][bytes]`.
//! DOCUMENTED bodies ([`ClientPieceKind::documented`]) are never compressed and
//! decode with [`decode_documented`]. The other bodies are the engine's own
//! replication encodings: opaque to mods by design, so no engine change to a
//! section ever becomes a mod-ABI break.
//!
//! A reader stops at a TORN record: bit 0 clear, `len` past the end of the
//! bytes, an envelope failing its CRC, or a listed piece failing its own.
//! Writes are not fsynced one by one, so a complete head may reach the disk
//! before its body; the CRCs are what catch that.

use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::PlayerId;

/// The container version: heads, envelopes and documented bodies.
pub const CAPTURE_FORMAT: u16 = 1;
pub const CAPTURE_RECORD_MAGIC: [u8; 4] = *b"PMCR";
pub const CAPTURE_PIECE_MAGIC: [u8; 4] = *b"PMCP";
pub const CAPTURE_RECORD_HEAD_LEN: usize = 40;
pub const CAPTURE_PIECE_HEAD_LEN: usize = 40;

/// Bytes of a protocol frame's own header (`[u32 len][u8 flags]`).
const FRAME_HEADER_LEN: usize = 5;
const FRAME_FLAG_ZLIB: u8 = 1;
/// Bytes of an envelope entry's header (`[u64 len][u32 crc]`).
const ENTRY_HEADER_LEN: usize = 12;

/// CRC-32 (ISO-HDLC, zlib's own check).
pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

/// FNV-1a, 64-bit: how a piece names the id vocabulary it was written under.
pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

/// Why capture bytes did not read.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CaptureFormatError {
    /// Fewer bytes than the structure needs.
    Short { need: u64, have: u64 },
    /// Not a capture record, piece or entry: foreign bytes.
    Magic,
    /// Another container version; this code reads [`CAPTURE_FORMAT`] only.
    Format { found: u16 },
    /// A record or piece kind this format does not define.
    Kind { found: u8 },
    /// A record whose head was never completed.
    Incomplete,
    /// A length reaching past the bytes given.
    PastEnd { len: u64, have: u64 },
    /// Bytes that fail their CRC.
    Crc { stored: u32, computed: u32 },
    /// A body that is not one well-formed protocol frame.
    Frame,
    /// A documented body stored compressed.
    Compressed,
    /// A value that does not decode.
    Decode(String),
    /// A walker stopped here: the absolute offset of the record or entry, and why.
    At {
        offset: u64,
        error: Box<CaptureFormatError>,
    },
}

impl core::fmt::Display for CaptureFormatError {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        match self {
            Self::Short { need, have } => write!(f, "needs {need} bytes, has {have}"),
            Self::Magic => f.write_str("not a capture record or piece"),
            Self::Format { found } => write!(
                f,
                "capture format {found}; this build reads format {CAPTURE_FORMAT}"
            ),
            Self::Kind { found } => write!(f, "unknown kind {found}"),
            Self::Incomplete => f.write_str("an incomplete record"),
            Self::PastEnd { len, have } => {
                write!(f, "a length of {len} reaches past the {have} bytes there")
            }
            Self::Crc { stored, computed } => {
                write!(f, "CRC {computed:08x} where {stored:08x} was stored")
            }
            Self::Frame => f.write_str("a body that is not one protocol frame"),
            Self::Compressed => f.write_str("a documented body stored compressed"),
            Self::Decode(why) => write!(f, "does not decode: {why}"),
            Self::At { offset, error } => write!(f, "@{offset}: {error}"),
        }
    }
}

impl std::error::Error for CaptureFormatError {}

/// What a record holds.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum CaptureRecordKind {
    State = 1,
    Frame = 2,
}

impl CaptureRecordKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        match v {
            1 => Some(Self::State),
            2 => Some(Self::Frame),
            _ => None,
        }
    }
}

/// A record's head, as [`parse_record_head`] reads it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CaptureRecordHead {
    pub kind: CaptureRecordKind,
    pub complete: bool,
    /// The whole record, head included; 0 while incomplete.
    pub len: u64,
    /// The envelope's offset from the record's start.
    pub envelope_at: u64,
    pub envelope_len: u64,
    pub envelope_crc: u32,
}

/// A piece's kind. The values are the head's `kind` byte. Kinds 1–14 are
/// STATE pieces, one [`ClientStateKey`] each; kinds 20–25 occur only in
/// Frame records.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ClientPieceKind {
    /// Opaque: the engine's section payload.
    Section = 1,
    /// Opaque: the column's own state.
    Column = 2,
    /// Documented: [`ClientPresence`].
    Presence = 3,
    /// Documented: [`ClientPopulation`].
    Population = 4,
    /// Documented: [`ClientCapturedSession`].
    Session = 5,
    /// Opaque: the id vocabulary.
    Tables = 6,
    /// Documented: [`ClientCapturedClock`].
    Clock = 7,
    /// Opaque: the mob's row.
    Mob = 8,
    /// Opaque: the dropped item's row.
    Item = 9,
    /// Opaque: the player's row.
    Player = 10,
    /// Documented: `Vec<`[`ClientRosterEntry`]`>`.
    Roster = 11,
    /// Documented: [`ClientCapturedEnv`].
    Environment = 12,
    /// Opaque: the spatial loops sounding, the local dig cell, the open containers.
    Activity = 13,
    /// Opaque: the local player's own state.
    Viewer = 14,
    /// Opaque: one applied message that is neither a tick batch nor a full arrival.
    Message = 20,
    /// Opaque: a tick batch's block, cell-KV and draw deltas.
    BatchWorld = 21,
    /// Opaque: a tick batch's mob, item and player rows.
    BatchRows = 22,
    /// Opaque: the rest of a tick batch's world content.
    BatchRest = 23,
    /// Opaque: the local predictions presented that frame.
    Cues = 24,
    /// Opaque: the locally presented view in full.
    View = 25,
}

impl ClientPieceKind {
    pub fn from_u8(v: u8) -> Option<Self> {
        use ClientPieceKind::*;
        Some(match v {
            1 => Section,
            2 => Column,
            3 => Presence,
            4 => Population,
            5 => Session,
            6 => Tables,
            7 => Clock,
            8 => Mob,
            9 => Item,
            10 => Player,
            11 => Roster,
            12 => Environment,
            13 => Activity,
            14 => Viewer,
            20 => Message,
            21 => BatchWorld,
            22 => BatchRows,
            23 => BatchRest,
            24 => Cues,
            25 => View,
            _ => return None,
        })
    }

    /// Whether this kind states one key's content.
    pub fn is_state(self) -> bool {
        (self as u8) < 20
    }

    /// Whether this kind's body is documented ([`decode_documented`]).
    pub fn documented(self) -> bool {
        use ClientPieceKind::*;
        matches!(
            self,
            Presence | Population | Session | Clock | Roster | Environment
        )
    }

    /// Whether this kind's key bytes hold a batch tick.
    pub fn is_batch(self) -> bool {
        use ClientPieceKind::*;
        matches!(self, BatchWorld | BatchRows | BatchRest)
    }
}

/// One stated thing in a presented world.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ClientStateKey {
    Section([i32; 3]),
    Column([i32; 2]),
    /// Which terrain exists.
    Presence,
    /// Which entities exist.
    Population,
    Session,
    Tables,
    /// The newest applied batch's tick and the day clock.
    Clock,
    Mob(u64),
    Item(u64),
    Player(PlayerId),
    Roster,
    Environment,
    Activity,
    Viewer,
}

impl ClientStateKey {
    /// The piece kind that states this key.
    pub fn kind(self) -> ClientPieceKind {
        match self {
            Self::Section(_) => ClientPieceKind::Section,
            Self::Column(_) => ClientPieceKind::Column,
            Self::Presence => ClientPieceKind::Presence,
            Self::Population => ClientPieceKind::Population,
            Self::Session => ClientPieceKind::Session,
            Self::Tables => ClientPieceKind::Tables,
            Self::Clock => ClientPieceKind::Clock,
            Self::Mob(_) => ClientPieceKind::Mob,
            Self::Item(_) => ClientPieceKind::Item,
            Self::Player(_) => ClientPieceKind::Player,
            Self::Roster => ClientPieceKind::Roster,
            Self::Environment => ClientPieceKind::Environment,
            Self::Activity => ClientPieceKind::Activity,
            Self::Viewer => ClientPieceKind::Viewer,
        }
    }

    /// The 12 key bytes of this key's piece head.
    pub fn key_bytes(self) -> [u8; 12] {
        let mut out = [0u8; 12];
        match self {
            Self::Section([x, y, z]) => {
                out[0..4].copy_from_slice(&x.to_le_bytes());
                out[4..8].copy_from_slice(&y.to_le_bytes());
                out[8..12].copy_from_slice(&z.to_le_bytes());
            }
            Self::Column([x, z]) => {
                out[0..4].copy_from_slice(&x.to_le_bytes());
                out[4..8].copy_from_slice(&z.to_le_bytes());
            }
            Self::Mob(id) | Self::Item(id) => out[0..8].copy_from_slice(&id.to_le_bytes()),
            Self::Player(id) => out[0] = id.0,
            _ => {}
        }
        out
    }

    /// The key a state piece's head names; `None` for a frame-only kind.
    pub fn from_head(kind: ClientPieceKind, key: [u8; 12]) -> Option<Self> {
        let i32_at = |at: usize| i32::from_le_bytes(key[at..at + 4].try_into().expect("4 bytes"));
        let u64_at = || u64::from_le_bytes(key[0..8].try_into().expect("8 bytes"));
        Some(match kind {
            ClientPieceKind::Section => Self::Section([i32_at(0), i32_at(4), i32_at(8)]),
            ClientPieceKind::Column => Self::Column([i32_at(0), i32_at(4)]),
            ClientPieceKind::Presence => Self::Presence,
            ClientPieceKind::Population => Self::Population,
            ClientPieceKind::Session => Self::Session,
            ClientPieceKind::Tables => Self::Tables,
            ClientPieceKind::Clock => Self::Clock,
            ClientPieceKind::Mob => Self::Mob(u64_at()),
            ClientPieceKind::Item => Self::Item(u64_at()),
            ClientPieceKind::Player => Self::Player(PlayerId(key[0])),
            ClientPieceKind::Roster => Self::Roster,
            ClientPieceKind::Environment => Self::Environment,
            ClientPieceKind::Activity => Self::Activity,
            ClientPieceKind::Viewer => Self::Viewer,
            _ => return None,
        })
    }
}

/// A piece's head, as [`parse_piece_head`] reads it.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CapturePieceHead {
    /// The opaque bodies' encoding (the engine's protocol version).
    pub protocol: u16,
    pub kind: ClientPieceKind,
    /// The piece holds an unconfirmed local prediction.
    pub provisional: bool,
    pub body_len: u32,
    pub body_crc: u32,
    pub key: [u8; 12],
    pub vocabulary: u64,
}

impl CapturePieceHead {
    /// The key a state piece states; `None` for a frame-only kind.
    pub fn state_key(&self) -> Option<ClientStateKey> {
        ClientStateKey::from_head(self.kind, self.key)
    }

    /// The tick of the batch a `Batch*` piece belongs to.
    pub fn batch(&self) -> Option<u64> {
        self.kind
            .is_batch()
            .then(|| u64::from_le_bytes(self.key[0..8].try_into().expect("8 bytes")))
    }

    /// The whole piece's length, head included.
    pub fn piece_len(&self) -> u64 {
        CAPTURE_PIECE_HEAD_LEN as u64 + u64::from(self.body_len)
    }
}

fn short(need: usize, have: usize) -> CaptureFormatError {
    CaptureFormatError::Short {
        need: need as u64,
        have: have as u64,
    }
}

fn u16_at(b: &[u8], at: usize) -> u16 {
    u16::from_le_bytes(b[at..at + 2].try_into().expect("2 bytes"))
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    u32::from_le_bytes(b[at..at + 4].try_into().expect("4 bytes"))
}

fn u64_at(b: &[u8], at: usize) -> u64 {
    u64::from_le_bytes(b[at..at + 8].try_into().expect("8 bytes"))
}

fn check_format(found: u16) -> Result<(), CaptureFormatError> {
    if found == CAPTURE_FORMAT {
        Ok(())
    } else {
        Err(CaptureFormatError::Format { found })
    }
}

fn check_crc(bytes: &[u8], stored: u32) -> Result<(), CaptureFormatError> {
    let computed = crc32(bytes);
    if computed == stored {
        Ok(())
    } else {
        Err(CaptureFormatError::Crc { stored, computed })
    }
}

/// Read a record head from the first 40 bytes. An incomplete head reads, with
/// `complete: false`; a walker treats it as torn.
pub fn parse_record_head(bytes: &[u8]) -> Result<CaptureRecordHead, CaptureFormatError> {
    if bytes.len() < CAPTURE_RECORD_HEAD_LEN {
        return Err(short(CAPTURE_RECORD_HEAD_LEN, bytes.len()));
    }
    if bytes[0..4] != CAPTURE_RECORD_MAGIC {
        return Err(CaptureFormatError::Magic);
    }
    check_format(u16_at(bytes, 4))?;
    let kind =
        CaptureRecordKind::from_u8(bytes[6]).ok_or(CaptureFormatError::Kind { found: bytes[6] })?;
    Ok(CaptureRecordHead {
        kind,
        complete: bytes[7] & 1 != 0,
        len: u64_at(bytes, 8),
        envelope_at: u64_at(bytes, 16),
        envelope_len: u64_at(bytes, 24),
        envelope_crc: u32_at(bytes, 32),
    })
}

/// Read a piece head from the first 40 bytes.
pub fn parse_piece_head(bytes: &[u8]) -> Result<CapturePieceHead, CaptureFormatError> {
    if bytes.len() < CAPTURE_PIECE_HEAD_LEN {
        return Err(short(CAPTURE_PIECE_HEAD_LEN, bytes.len()));
    }
    if bytes[0..4] != CAPTURE_PIECE_MAGIC {
        return Err(CaptureFormatError::Magic);
    }
    check_format(u16_at(bytes, 4))?;
    let kind =
        ClientPieceKind::from_u8(bytes[8]).ok_or(CaptureFormatError::Kind { found: bytes[8] })?;
    Ok(CapturePieceHead {
        protocol: u16_at(bytes, 6),
        kind,
        provisional: bytes[9] & 1 != 0,
        body_len: u32_at(bytes, 12),
        body_crc: u32_at(bytes, 16),
        key: bytes[20..32].try_into().expect("12 bytes"),
        vocabulary: u64_at(bytes, 32),
    })
}

pub fn write_record_head(head: &CaptureRecordHead) -> [u8; 40] {
    let mut out = [0u8; 40];
    out[0..4].copy_from_slice(&CAPTURE_RECORD_MAGIC);
    out[4..6].copy_from_slice(&CAPTURE_FORMAT.to_le_bytes());
    out[6] = head.kind as u8;
    out[7] = u8::from(head.complete);
    out[8..16].copy_from_slice(&head.len.to_le_bytes());
    out[16..24].copy_from_slice(&head.envelope_at.to_le_bytes());
    out[24..32].copy_from_slice(&head.envelope_len.to_le_bytes());
    out[32..36].copy_from_slice(&head.envelope_crc.to_le_bytes());
    out
}

pub fn write_piece_head(head: &CapturePieceHead) -> [u8; 40] {
    let mut out = [0u8; 40];
    out[0..4].copy_from_slice(&CAPTURE_PIECE_MAGIC);
    out[4..6].copy_from_slice(&CAPTURE_FORMAT.to_le_bytes());
    out[6..8].copy_from_slice(&head.protocol.to_le_bytes());
    out[8] = head.kind as u8;
    out[9] = u8::from(head.provisional);
    out[12..16].copy_from_slice(&head.body_len.to_le_bytes());
    out[16..20].copy_from_slice(&head.body_crc.to_le_bytes());
    out[20..32].copy_from_slice(&head.key);
    out[32..40].copy_from_slice(&head.vocabulary.to_le_bytes());
    out
}

/// Check a whole piece (head and body in `piece`, nothing after it required)
/// against its CRC, and answer its head and its body: one protocol frame.
pub fn check_piece(piece: &[u8]) -> Result<(CapturePieceHead, &[u8]), CaptureFormatError> {
    let head = parse_piece_head(piece)?;
    let end = CAPTURE_PIECE_HEAD_LEN + head.body_len as usize;
    if piece.len() < end {
        return Err(CaptureFormatError::PastEnd {
            len: head.piece_len(),
            have: piece.len() as u64,
        });
    }
    let body = &piece[CAPTURE_PIECE_HEAD_LEN..end];
    check_crc(body, head.body_crc)?;
    Ok((head, body))
}

/// Split one protocol frame into its flags and its bytes.
pub fn split_frame(frame: &[u8]) -> Result<(u8, &[u8]), CaptureFormatError> {
    if frame.len() < FRAME_HEADER_LEN {
        return Err(CaptureFormatError::Frame);
    }
    let len = u32_at(frame, 0) as usize;
    if frame.len() != FRAME_HEADER_LEN + len {
        return Err(CaptureFormatError::Frame);
    }
    Ok((frame[4], &frame[FRAME_HEADER_LEN..]))
}

/// A documented body's frame: `value` postcard-encoded, never compressed.
pub fn encode_documented<T: Serialize>(value: &T) -> Result<Vec<u8>, CaptureFormatError> {
    let body =
        postcard::to_allocvec(value).map_err(|e| CaptureFormatError::Decode(format!("{e}")))?;
    let len = u32::try_from(body.len()).map_err(|_| CaptureFormatError::Frame)?;
    let mut frame = Vec::with_capacity(FRAME_HEADER_LEN + body.len());
    frame.extend_from_slice(&len.to_le_bytes());
    frame.push(0);
    frame.extend_from_slice(&body);
    Ok(frame)
}

/// A documented piece's body (Presence, Population, Session, Clock, Roster,
/// Environment): the whole piece, head and body, in.
pub fn decode_documented<T: DeserializeOwned>(piece: &[u8]) -> Result<T, CaptureFormatError> {
    let (head, body) = check_piece(piece)?;
    if !head.kind.documented() {
        return Err(CaptureFormatError::Kind {
            found: head.kind as u8,
        });
    }
    let (flags, bytes) = split_frame(body)?;
    if flags & FRAME_FLAG_ZLIB != 0 {
        return Err(CaptureFormatError::Compressed);
    }
    postcard::from_bytes(bytes).map_err(|e| CaptureFormatError::Decode(format!("{e}")))
}

/// Check the envelope against its head's CRC, then decode it.
pub fn decode_envelope(
    head: &CaptureRecordHead,
    envelope: &[u8],
) -> Result<ClientEnvelope, CaptureFormatError> {
    check_crc(envelope, head.envelope_crc)?;
    let decode = |e: postcard::Error| CaptureFormatError::Decode(format!("{e}"));
    Ok(match head.kind {
        CaptureRecordKind::State => {
            ClientEnvelope::State(postcard::from_bytes(envelope).map_err(decode)?)
        }
        CaptureRecordKind::Frame => {
            ClientEnvelope::Frame(postcard::from_bytes(envelope).map_err(decode)?)
        }
    })
}

/// Where a record's envelope lies inside `record` (the record's bytes from its
/// head on), checked against the record's length.
fn envelope_span(
    head: &CaptureRecordHead,
    have: usize,
) -> Result<core::ops::Range<usize>, CaptureFormatError> {
    let past = CaptureFormatError::PastEnd {
        len: head.envelope_at.saturating_add(head.envelope_len),
        have: head.len,
    };
    let end = head
        .envelope_at
        .checked_add(head.envelope_len)
        .filter(|&end| head.envelope_at >= CAPTURE_RECORD_HEAD_LEN as u64 && end <= head.len)
        .ok_or(past)?;
    let (start, end) = (head.envelope_at as usize, end as usize);
    if end > have {
        return Err(short(end, have));
    }
    Ok(start..end)
}

/// Walk the records in `bytes`, which start at absolute file offset `base`.
/// Each item is a record's absolute offset and head, its envelope checked.
/// The walk stops at the first torn or foreign record and says where
/// ([`CaptureFormatError::At`]); it never reads a record's pieces.
pub fn records(
    bytes: &[u8],
    base: u64,
) -> impl Iterator<Item = Result<(u64, CaptureRecordHead), CaptureFormatError>> + '_ {
    let mut pos = 0usize;
    let mut stopped = false;
    core::iter::from_fn(move || {
        if stopped || pos == bytes.len() {
            return None;
        }
        let at = base + pos as u64;
        let rest = &bytes[pos..];
        let step = (|| {
            let head = parse_record_head(rest)?;
            if !head.complete {
                return Err(CaptureFormatError::Incomplete);
            }
            if head.len < CAPTURE_RECORD_HEAD_LEN as u64 || head.len > rest.len() as u64 {
                return Err(CaptureFormatError::PastEnd {
                    len: head.len,
                    have: rest.len() as u64,
                });
            }
            let span = envelope_span(&head, rest.len())?;
            check_crc(&rest[span], head.envelope_crc)?;
            Ok(head)
        })();
        match step {
            Ok(head) => {
                pos += head.len as usize;
                Some(Ok((at, head)))
            }
            Err(error) => {
                stopped = true;
                Some(Err(CaptureFormatError::At {
                    offset: at,
                    error: Box::new(error),
                }))
            }
        }
    })
}

/// The envelope of a whole record (`record` = its bytes from the head on),
/// checked and decoded.
pub fn record_envelope(record: &[u8]) -> Result<ClientEnvelope, CaptureFormatError> {
    let head = parse_record_head(record)?;
    if !head.complete {
        return Err(CaptureFormatError::Incomplete);
    }
    let span = envelope_span(&head, record.len())?;
    decode_envelope(&head, &record[span])
}

/// One envelope-file entry's bytes: `[u64 len][u32 CRC-32 of the entry
/// bytes][postcard ClientEnvelopeEntry]`, `len` counting the postcard bytes.
pub fn write_envelope_entry(entry: &ClientEnvelopeEntry) -> Result<Vec<u8>, CaptureFormatError> {
    let body =
        postcard::to_allocvec(entry).map_err(|e| CaptureFormatError::Decode(format!("{e}")))?;
    let mut out = Vec::with_capacity(ENTRY_HEADER_LEN + body.len());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

/// Walk an envelope file's entries in `bytes`, which start at absolute file
/// offset `base`: each item is an entry's absolute offset and the entry. The
/// walk stops at the first torn entry and says where.
pub fn envelope_entries(
    bytes: &[u8],
    base: u64,
) -> impl Iterator<Item = Result<(u64, ClientEnvelopeEntry), CaptureFormatError>> + '_ {
    let mut pos = 0usize;
    let mut stopped = false;
    core::iter::from_fn(move || {
        if stopped || pos == bytes.len() {
            return None;
        }
        let at = base + pos as u64;
        let rest = &bytes[pos..];
        let step = (|| {
            if rest.len() < ENTRY_HEADER_LEN {
                return Err(short(ENTRY_HEADER_LEN, rest.len()));
            }
            let len = u64_at(rest, 0);
            let available = (rest.len() - ENTRY_HEADER_LEN) as u64;
            if len > available {
                return Err(CaptureFormatError::PastEnd {
                    len,
                    have: available,
                });
            }
            let body = &rest[ENTRY_HEADER_LEN..ENTRY_HEADER_LEN + len as usize];
            check_crc(body, u32_at(rest, 8))?;
            let entry: ClientEnvelopeEntry = postcard::from_bytes(body)
                .map_err(|e| CaptureFormatError::Decode(format!("{e}")))?;
            Ok((ENTRY_HEADER_LEN + len as usize, entry))
        })();
        match step {
            Ok((used, entry)) => {
                pos += used;
                Some(Ok((at, entry)))
            }
            Err(error) => {
                stopped = true;
                Some(Err(CaptureFormatError::At {
                    offset: at,
                    error: Box::new(error),
                }))
            }
        }
    })
}

// --- Envelopes ------------------------------------------------------------

/// A pose in the presented world: `pos` is the EYE.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientPose {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

/// One piece as an envelope lists it. `range` is RELATIVE to its record's
/// start, so a record copied byte for byte into another file stays valid.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPieceInfo {
    pub kind: ClientPieceKind,
    /// State pieces.
    pub key: Option<ClientStateKey>,
    /// `Batch*` pieces.
    pub batch: Option<u64>,
    /// `[offset, len]`, relative to its record.
    pub range: [u64; 2],
    /// The piece's `body_crc`, so a mod checking a record needs no piece heads.
    pub crc: u32,
    pub provisional: bool,
}

/// A State record's envelope.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientStateEnvelope {
    /// The presented world's revision the state describes.
    pub revision: u64,
    /// The `ChangedSince` base actually used; `None` = complete.
    pub since: Option<u64>,
    /// The newest applied batch.
    pub tick: u64,
    /// The fractional tick the last frame presented.
    pub presented_tick: f64,
    pub pieces: Vec<ClientPieceInfo>,
    /// `Keys` asked for these; they are not present.
    pub absent: Vec<ClientStateKey>,
}

/// The local eye as a frame presented it: bob and speed-coupled FOV
/// included. The hurt shake, hands and motion are in the `View` piece.
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientCapturedView {
    pub player: PlayerId,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
}

/// A Frame record's envelope.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientFrameEnvelope {
    /// Records this log wrote before this one.
    pub seq: u64,
    /// The presented world's revision after this frame.
    pub revision: u64,
    pub presented_tick: f64,
    /// Ticks of the tick batches this frame applied.
    pub batches: Vec<u64>,
    pub view: Option<ClientCapturedView>,
    /// Every piece, in apply order.
    pub pieces: Vec<ClientPieceInfo>,
    /// Changed this frame without being stated in full. Never entities: every
    /// `BatchRows` piece states every row of its batch.
    pub touched: Vec<ClientStateKey>,
    /// Sections and columns unloaded this frame.
    pub removed: Vec<ClientStateKey>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientEnvelope {
    State(ClientStateEnvelope),
    Frame(ClientFrameEnvelope),
}

/// One entry of an envelope file: the record's absolute `[offset, len]` and
/// its envelope.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientEnvelopeEntry {
    pub record: [u64; 2],
    pub envelope: ClientEnvelope,
}

// --- Documented bodies ----------------------------------------------------

/// Which terrain exists. For each column, bit `i` of `sections` (u64 words,
/// least significant bit first) means section `cy_min + i` is present. A
/// column that is present with no loaded section has empty `sections`.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPresence {
    pub cy_min: i32,
    pub columns: Vec<ClientColumnPresence>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientColumnPresence {
    pub pos: [i32; 2],
    pub sections: Vec<u64>,
}

impl ClientColumnPresence {
    /// Whether section `cy` of this column is present, given the presence's `cy_min`.
    pub fn has_section(&self, cy_min: i32, cy: i32) -> bool {
        let Ok(i) = usize::try_from(i64::from(cy) - i64::from(cy_min)) else {
            return false;
        };
        self.sections
            .get(i / 64)
            .is_some_and(|word| word >> (i % 64) & 1 != 0)
    }
}

/// Which entities exist.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPopulation {
    pub mobs: Vec<u64>,
    pub items: Vec<u64>,
    pub players: Vec<PlayerId>,
}

/// The session the world was presented in: what decoding and hosting it needs.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientCapturedSession {
    pub seed: u32,
    pub local_player: PlayerId,
    pub mods: Vec<ClientCapturedMod>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientCapturedMod {
    pub id: String,
    pub version: String,
    pub affects_world: bool,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientCapturedClock {
    pub tick: u64,
    pub day_clock: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientRosterEntry {
    pub player: PlayerId,
    pub name: String,
}

/// Every shader param as REPLICATED, before any mod's override.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientCapturedEnv {
    pub params: Vec<(String, [f32; 4])>,
}

// --- Capture and presentation calls ---------------------------------------

/// Which keys a `ClientWorldStateWrite` states.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientStateSelect {
    /// Every present key.
    All,
    /// Every present key whose content changed after this revision, plus
    /// `Presence`/`Population` when their key set moved. A revision this
    /// world never issued is served as `All` (`since: None`).
    ChangedSince(u64),
    /// Exactly these keys, each once; one not present goes to `absent`.
    Keys(Vec<ClientStateKey>),
}

/// An accepted `ClientWorldStateWrite`: the file ticket its record completes
/// on, and the revision the state describes (known at the call).
#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub struct ClientStateTicketData {
    pub write: u64,
    pub revision: u64,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq)]
pub enum ClientEventsPhase {
    Running,
    Ending,
    Ended,
    Failed,
}

/// Where an events log stands. A few numbers: the engine keeps no envelopes
/// for a mod.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientEventsReport {
    pub phase: ClientEventsPhase,
    pub error: Option<String>,
    /// Records queued so far.
    pub frames: u64,
    /// The file offset up to which this log's records have been handed to the OS.
    pub written_through: u64,
    /// Bytes of this log's frames queued but not yet written.
    pub backlog_bytes: u64,
}

/// Where the presentation stands (`ClientPresentationState`).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientPresentationStateData {
    pub opening: bool,
    pub open: bool,
    pub owner: Option<String>,
    /// The presented position, fractional ticks.
    pub position: f64,
    /// The newest tick batch released into the world.
    pub released_through: Option<u64>,
    /// No apply pending, and every batch the position needs is released, or
    /// the queued events are exhausted.
    pub ready: bool,
    /// Applies not yet landed, in issue order; the first is being prepared.
    pub pending: Vec<u64>,
    /// The last apply that LANDED (0 = none yet).
    pub applied: u64,
    /// Every queued events range has been released.
    pub exhausted: bool,
    pub error: Option<String>,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn piece(kind: ClientPieceKind, key: [u8; 12], frame: &[u8]) -> Vec<u8> {
        let head = CapturePieceHead {
            protocol: 7,
            kind,
            provisional: true,
            body_len: frame.len() as u32,
            body_crc: crc32(frame),
            key,
            vocabulary: fnv1a64(b"tables"),
        };
        let mut out = write_piece_head(&head).to_vec();
        out.extend_from_slice(frame);
        out
    }

    fn frame_record(seq: u64) -> Vec<u8> {
        let envelope = postcard::to_allocvec(&ClientFrameEnvelope {
            seq,
            revision: 3,
            presented_tick: 1.5,
            batches: vec![1],
            view: None,
            pieces: Vec::new(),
            touched: vec![ClientStateKey::Section([1, -2, 3])],
            removed: Vec::new(),
        })
        .unwrap();
        let len = (CAPTURE_RECORD_HEAD_LEN + envelope.len()) as u64;
        let head = CaptureRecordHead {
            kind: CaptureRecordKind::Frame,
            complete: true,
            len,
            envelope_at: CAPTURE_RECORD_HEAD_LEN as u64,
            envelope_len: envelope.len() as u64,
            envelope_crc: crc32(&envelope),
        };
        let mut out = write_record_head(&head).to_vec();
        out.extend_from_slice(&envelope);
        out
    }

    #[test]
    fn a_documented_piece_reads_back_its_key_and_body_and_refuses_damage() {
        let clock = ClientCapturedClock {
            tick: 9,
            day_clock: 4,
        };
        let key = ClientStateKey::Section([-5, 2, 1 << 20]);
        let bytes = piece(
            ClientPieceKind::Clock,
            key.key_bytes(),
            &encode_documented(&clock).unwrap(),
        );
        assert_eq!(decode_documented::<ClientCapturedClock>(&bytes), Ok(clock));
        let head = parse_piece_head(&bytes).unwrap();
        assert_eq!(
            ClientStateKey::from_head(ClientPieceKind::Section, head.key),
            Some(key)
        );
        let mut torn = bytes.clone();
        *torn.last_mut().unwrap() ^= 1;
        assert!(matches!(
            decode_documented::<ClientCapturedClock>(&torn),
            Err(CaptureFormatError::Crc { .. })
        ));
    }

    #[test]
    fn the_record_walk_stops_at_the_first_torn_record_and_says_where() {
        let mut file = frame_record(0);
        let second = file.len() as u64;
        file.extend(frame_record(1));
        let third = file.len() as u64;
        let mut damaged = frame_record(2);
        *damaged.last_mut().unwrap() ^= 1;
        file.extend(damaged);
        file.extend(frame_record(3));
        let walked: Vec<_> = records(&file, 100).collect();
        assert_eq!(walked.len(), 3);
        assert_eq!(walked[1].as_ref().unwrap().0, 100 + second);
        assert!(matches!(
            &walked[2],
            Err(CaptureFormatError::At { offset, .. }) if *offset == 100 + third
        ));
        let first = record_envelope(&file[..second as usize]).unwrap();
        assert!(matches!(first, ClientEnvelope::Frame(f) if f.seq == 0));
    }

    #[test]
    fn envelope_entries_walk_until_a_torn_one() {
        let entry = |at: u64| ClientEnvelopeEntry {
            record: [at, 40],
            envelope: ClientEnvelope::State(ClientStateEnvelope {
                revision: 1,
                since: None,
                tick: 2,
                presented_tick: 2.0,
                pieces: Vec::new(),
                absent: vec![ClientStateKey::Mob(3)],
            }),
        };
        let mut file = write_envelope_entry(&entry(0)).unwrap();
        file.extend(write_envelope_entry(&entry(40)).unwrap());
        let whole = file.len();
        file.extend(&write_envelope_entry(&entry(80)).unwrap()[..5]);
        let walked: Vec<_> = envelope_entries(&file, 0).collect();
        assert_eq!(walked.len(), 3);
        assert_eq!(walked[1].as_ref().unwrap().1, entry(40));
        assert!(matches!(
            &walked[2],
            Err(CaptureFormatError::At { offset, .. }) if *offset == whole as u64
        ));
    }
}
