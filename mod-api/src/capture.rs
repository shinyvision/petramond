use serde::de::DeserializeOwned;
use serde::{Deserialize, Serialize};

use crate::PlayerId;

pub const CAPTURE_FORMAT: u16 = 1;
pub const CAPTURE_RECORD_MAGIC: [u8; 4] = *b"PMCR";
pub const CAPTURE_PIECE_MAGIC: [u8; 4] = *b"PMCP";
pub const CAPTURE_RECORD_HEAD_LEN: usize = 40;
pub const CAPTURE_PIECE_HEAD_LEN: usize = 40;

const FRAME_HEADER_LEN: usize = 5;
const FRAME_FLAG_ZLIB: u8 = 1;
const ENTRY_HEADER_LEN: usize = 12;

pub fn crc32(bytes: &[u8]) -> u32 {
    crc32fast::hash(bytes)
}

pub fn fnv1a64(bytes: &[u8]) -> u64 {
    bytes.iter().fold(0xcbf2_9ce4_8422_2325, |hash, &b| {
        (hash ^ u64::from(b)).wrapping_mul(0x0000_0100_0000_01b3)
    })
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum CaptureFormatError {
    Short {
        need: u64,
        have: u64,
    },
    Magic,
    Format {
        found: u16,
    },
    Kind {
        found: u8,
    },
    Incomplete,
    PastEnd {
        len: u64,
        have: u64,
    },
    Crc {
        stored: u32,
        computed: u32,
    },
    Frame,
    Compressed,
    Decode(String),
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CaptureRecordHead {
    pub kind: CaptureRecordKind,
    pub complete: bool,
    pub len: u64,
    pub envelope_at: u64,
    pub envelope_len: u64,
    pub envelope_crc: u32,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
#[repr(u8)]
pub enum ClientPieceKind {
    Section = 1,
    Column = 2,
    Presence = 3,
    Population = 4,
    Session = 5,
    Tables = 6,
    Clock = 7,
    Mob = 8,
    Item = 9,
    Player = 10,
    Roster = 11,
    Environment = 12,
    Activity = 13,
    Viewer = 14,
    Message = 20,
    BatchWorld = 21,
    BatchRows = 22,
    BatchRest = 23,
    Cues = 24,
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

    pub fn is_state(self) -> bool {
        (self as u8) < 20
    }

    pub fn documented(self) -> bool {
        use ClientPieceKind::*;
        matches!(
            self,
            Presence | Population | Session | Clock | Roster | Environment
        )
    }

    pub fn is_batch(self) -> bool {
        use ClientPieceKind::*;
        matches!(self, BatchWorld | BatchRows | BatchRest)
    }
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ClientStateKey {
    Section([i32; 3]),
    Column([i32; 2]),
    Presence,
    Population,
    Session,
    Tables,
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

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub struct CapturePieceHead {
    pub protocol: u16,
    pub kind: ClientPieceKind,
    pub provisional: bool,
    pub body_len: u32,
    pub body_crc: u32,
    pub key: [u8; 12],
    pub vocabulary: u64,
}

impl CapturePieceHead {
    pub fn state_key(&self) -> Option<ClientStateKey> {
        ClientStateKey::from_head(self.kind, self.key)
    }

    pub fn batch(&self) -> Option<u64> {
        self.kind
            .is_batch()
            .then(|| u64::from_le_bytes(self.key[0..8].try_into().expect("8 bytes")))
    }

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

pub fn record_envelope(record: &[u8]) -> Result<ClientEnvelope, CaptureFormatError> {
    let head = parse_record_head(record)?;
    if !head.complete {
        return Err(CaptureFormatError::Incomplete);
    }
    let span = envelope_span(&head, record.len())?;
    decode_envelope(&head, &record[span])
}

pub fn write_envelope_entry(entry: &ClientEnvelopeEntry) -> Result<Vec<u8>, CaptureFormatError> {
    let body =
        postcard::to_allocvec(entry).map_err(|e| CaptureFormatError::Decode(format!("{e}")))?;
    let mut out = Vec::with_capacity(ENTRY_HEADER_LEN + body.len());
    out.extend_from_slice(&(body.len() as u64).to_le_bytes());
    out.extend_from_slice(&crc32(&body).to_le_bytes());
    out.extend_from_slice(&body);
    Ok(out)
}

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

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientPose {
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPieceInfo {
    pub kind: ClientPieceKind,
    pub key: Option<ClientStateKey>,
    pub batch: Option<u64>,
    pub range: [u64; 2],
    pub crc: u32,
    pub provisional: bool,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientStateEnvelope {
    pub revision: u64,
    pub since: Option<u64>,
    pub tick: u64,
    pub presented_tick: f64,
    pub pieces: Vec<ClientPieceInfo>,
    pub absent: Vec<ClientStateKey>,
}

#[derive(Serialize, Deserialize, Copy, Clone, Debug, PartialEq)]
pub struct ClientCapturedView {
    pub player: PlayerId,
    pub pos: [f64; 3],
    pub yaw: f32,
    pub pitch: f32,
    pub roll: f32,
    pub fov_y: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientFrameEnvelope {
    pub seq: u64,
    pub revision: u64,
    pub presented_tick: f64,
    pub batches: Vec<u64>,
    pub view: Option<ClientCapturedView>,
    pub pieces: Vec<ClientPieceInfo>,
    pub touched: Vec<ClientStateKey>,
    pub removed: Vec<ClientStateKey>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum ClientEnvelope {
    State(ClientStateEnvelope),
    Frame(ClientFrameEnvelope),
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientEnvelopeEntry {
    pub record: [u64; 2],
    pub envelope: ClientEnvelope,
}

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
    pub fn has_section(&self, cy_min: i32, cy: i32) -> bool {
        let Ok(i) = usize::try_from(i64::from(cy) - i64::from(cy_min)) else {
            return false;
        };
        self.sections
            .get(i / 64)
            .is_some_and(|word| word >> (i % 64) & 1 != 0)
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientPopulation {
    pub mobs: Vec<u64>,
    pub items: Vec<u64>,
    pub players: Vec<PlayerId>,
}

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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientCapturedEnv {
    pub params: Vec<(String, [f32; 4])>,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub enum ClientStateSelect {
    All,
    ChangedSince(u64),
    Keys(Vec<ClientStateKey>),
}

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

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ClientEventsReport {
    pub phase: ClientEventsPhase,
    pub error: Option<String>,
    pub frames: u64,
    pub written_through: u64,
    pub backlog_bytes: u64,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub struct ClientPresentationStateData {
    pub opening: bool,
    pub open: bool,
    pub owner: Option<String>,
    pub position: f64,
    pub released_through: Option<u64>,
    pub ready: bool,
    pub pending: Vec<u64>,
    pub applied: u64,
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
