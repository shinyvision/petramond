use serde::{Deserialize, Serialize};

pub use super::host::HostCall;
use crate::client::{ClientCanvasEvent, ClientFrameData, ClientUiEvent};
use crate::data::{AiNodeCtx, AiNodeDecision, BlockHookKind, HostileSpawnCandidate};
use crate::events::{EventPayload, Outcome};
use crate::ids::BlockId;
use crate::sched::WorldgenStage;
use crate::shape::{
    BakedItemGeometry, BakedRenderCell, BakedSimCell, CellInput, PlaceInputsView,
    ShapePlacementResult,
};

pub type GenWrite = ([i32; 3], BlockId);

/// Serializes a vector of fixed-size records as one byte string, and refuses bytes that are not
/// whole records.
macro_rules! serde_records {
    ($ty:ident) => {
        impl Serialize for $ty {
            fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
                serde_bytes::Bytes::new(self.0.as_flattened()).serialize(s)
            }
        }

        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let bytes = serde_bytes::ByteBuf::deserialize(d)?;
                let (records, rest) = bytes.as_chunks::<{ $ty::RECORD_BYTES }>();
                if !rest.is_empty() {
                    return Err(serde::de::Error::custom(concat!(
                        stringify!($ty),
                        " is not whole records"
                    )));
                }
                Ok($ty(records.to_vec()))
            }
        }
    };
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct StructurePlacement {
    pub template: String,
    pub origin: [i32; 3],
    pub turn: u8,
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct FeaturePlacement {
    pub feature: String,
    pub origins: Vec<[i32; 3]>,
    pub salt: u64,
}

/// Materials authored cells name by index, packed as little-endian entries: `block: u16`,
/// `pairs: u16`, then per state property `key_len: u16`, key, `value_len: u16`, value. State is
/// in the vocabulary of a structure template's palette entry (`facing`, `half`, `axis`, `open`,
/// `mount`), laid out by the block's shape family. Packed, a palette crosses the ABI as one copy,
/// and whatever the host derives from an entry can be keyed on the entry's bytes.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthoredPalette {
    entries: u32,
    #[serde(with = "serde_bytes")]
    bytes: Vec<u8>,
}

impl AuthoredPalette {
    /// Appends a material; returns its index.
    pub fn push<'a>(
        &mut self,
        block: BlockId,
        state: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> u32 {
        self.push_encoded(&Self::encode(block, state))
    }

    /// One entry's bytes, for a writer that puts the same material in many palettes.
    pub fn encode<'a>(
        block: BlockId,
        state: impl IntoIterator<Item = (&'a str, &'a str)>,
    ) -> Vec<u8> {
        let mut out = Vec::with_capacity(16);
        out.extend_from_slice(&block.0.to_le_bytes());
        out.extend_from_slice(&[0, 0]);
        let mut pairs = 0u16;
        for (key, value) in state {
            for text in [key, value] {
                out.extend_from_slice(&(text.len() as u16).to_le_bytes());
                out.extend_from_slice(text.as_bytes());
            }
            pairs += 1;
        }
        out[2..4].copy_from_slice(&pairs.to_le_bytes());
        out
    }

    /// Appends an entry made by [`encode`](AuthoredPalette::encode); returns its index.
    #[inline]
    pub fn push_encoded(&mut self, entry: &[u8]) -> u32 {
        self.bytes.extend_from_slice(entry);
        self.entries += 1;
        self.entries - 1
    }

    pub fn len(&self) -> usize {
        self.entries as usize
    }

    pub fn is_empty(&self) -> bool {
        self.entries == 0
    }

    pub fn get(&self, index: usize) -> Option<AuthoredEntry<'_>> {
        self.iter().nth(index)?.ok()
    }

    /// The entries in order, an `Err` where the bytes stop being a palette.
    pub fn iter(&self) -> impl Iterator<Item = Result<AuthoredEntry<'_>, ()>> + '_ {
        let mut rest = self.bytes.as_slice();
        let mut left = self.entries;
        std::iter::from_fn(move || {
            if left == 0 {
                return (!rest.is_empty()).then_some(Err(()));
            }
            left -= 1;
            let parsed = AuthoredEntry::parse(rest);
            rest = parsed.map_or(&[], |(_, tail)| tail);
            Some(parsed.map(|(entry, _)| entry).ok_or(()))
        })
    }
}

/// One [`AuthoredPalette`] entry.
#[derive(Clone, Copy, Debug)]
pub struct AuthoredEntry<'a> {
    pub block: BlockId,
    /// The whole entry, as packed.
    pub bytes: &'a [u8],
    pairs: u16,
}

impl<'a> AuthoredEntry<'a> {
    fn parse(bytes: &'a [u8]) -> Option<(AuthoredEntry<'a>, &'a [u8])> {
        let word = |at: usize| Some(u16::from_le_bytes([*bytes.get(at)?, *bytes.get(at + 1)?]));
        let (block, pairs) = (word(0)?, word(2)?);
        let mut at = 4;
        for _ in 0..2 * pairs as usize {
            let len = word(at)? as usize;
            std::str::from_utf8(bytes.get(at + 2..at + 2 + len)?).ok()?;
            at += 2 + len;
        }
        let entry = AuthoredEntry {
            block: BlockId(block),
            bytes: &bytes[..at],
            pairs,
        };
        Some((entry, &bytes[at..]))
    }

    /// The authored state properties, `(key, value)`.
    pub fn state(&self) -> impl Iterator<Item = (&'a str, &'a str)> + 'a {
        let bytes = self.bytes;
        let mut at = 4;
        let mut text = move || {
            let len = u16::from_le_bytes([bytes[at], bytes[at + 1]]) as usize;
            at += 2 + len;
            std::str::from_utf8(&bytes[at - len..at]).unwrap_or_default()
        };
        (0..self.pairs).map(move |_| (text(), text()))
    }
}

/// Namespaced cell data attached to a cell this output's authored writes place.
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct AuthoredData {
    pub pos: [i32; 3],
    pub key: String,
    pub value: Vec<u8>,
}

/// Palette materials anchored at absolute positions. An anchor may lie outside the dispatching
/// section: its footprint is expanded first and then clipped like every other write, so every
/// section a multi-cell object touches must send its anchor.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthoredWrites {
    pub palette: AuthoredPalette,
    pub cells: AuthoredCells,
    pub data: Vec<AuthoredData>,
}

/// Authored cells as packed little-endian `(x: i32, y: i32, z: i32, material: u16)` records: an
/// output carries thousands of them, and a packed buffer crosses the ABI as one copy instead of
/// a varint per coordinate.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct AuthoredCells(Vec<[u8; AuthoredCells::RECORD_BYTES]>);

impl AuthoredCells {
    pub const RECORD_BYTES: usize = 14;

    pub fn with_capacity(cells: usize) -> Self {
        Self(Vec::with_capacity(cells))
    }

    #[inline]
    pub fn push(&mut self, [x, y, z]: [i32; 3], material: u16) {
        let mut record = [0u8; Self::RECORD_BYTES];
        record[0..4].copy_from_slice(&x.to_le_bytes());
        record[4..8].copy_from_slice(&y.to_le_bytes());
        record[8..12].copy_from_slice(&z.to_le_bytes());
        record[12..14].copy_from_slice(&material.to_le_bytes());
        self.0.push(record);
    }

    pub fn reserve(&mut self, cells: usize) {
        self.0.reserve(cells);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = ([i32; 3], u16)> + '_ {
        self.0.iter().map(|r| {
            let word = |i: usize| i32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
            (
                [word(0), word(4), word(8)],
                u16::from_le_bytes([r[12], r[13]]),
            )
        })
    }
}

serde_records!(AuthoredCells);

impl FromIterator<([i32; 3], u16)> for AuthoredCells {
    fn from_iter<I: IntoIterator<Item = ([i32; 3], u16)>>(cells: I) -> Self {
        let mut out = Self::default();
        for (pos, material) in cells {
            out.push(pos, material);
        }
        out
    }
}

/// Every cell of the inclusive box `min..=max` set to `block`, clipped to the section like any
/// other write. A cell that already holds exactly `block` with no state is left untouched, so
/// clearing to air where the terrain is already air costs a read, not a write.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct GenFill {
    pub min: [i32; 3],
    pub max: [i32; 3],
    pub block: BlockId,
}

/// [`GenFill`]s as packed little-endian `(min: [i32; 3], max: [i32; 3], block: u16)` records.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct GenFills(Vec<[u8; GenFills::RECORD_BYTES]>);

impl GenFills {
    pub const RECORD_BYTES: usize = 26;

    pub fn reserve(&mut self, fills: usize) {
        self.0.reserve(fills);
    }

    #[inline]
    pub fn push(&mut self, GenFill { min, max, block }: GenFill) {
        let mut record = [0u8; Self::RECORD_BYTES];
        record[0..4].copy_from_slice(&min[0].to_le_bytes());
        record[4..8].copy_from_slice(&min[1].to_le_bytes());
        record[8..12].copy_from_slice(&min[2].to_le_bytes());
        record[12..16].copy_from_slice(&max[0].to_le_bytes());
        record[16..20].copy_from_slice(&max[1].to_le_bytes());
        record[20..24].copy_from_slice(&max[2].to_le_bytes());
        record[24..26].copy_from_slice(&block.0.to_le_bytes());
        self.0.push(record);
    }

    pub fn len(&self) -> usize {
        self.0.len()
    }

    pub fn is_empty(&self) -> bool {
        self.0.is_empty()
    }

    pub fn iter(&self) -> impl Iterator<Item = GenFill> + '_ {
        self.0.iter().map(|r| {
            let word = |i: usize| i32::from_le_bytes([r[i], r[i + 1], r[i + 2], r[i + 3]]);
            GenFill {
                min: [word(0), word(4), word(8)],
                max: [word(12), word(16), word(20)],
                block: BlockId(u16::from_le_bytes([r[24], r[25]])),
            }
        })
    }
}

serde_records!(GenFills);

impl FromIterator<GenFill> for GenFills {
    fn from_iter<I: IntoIterator<Item = GenFill>>(fills: I) -> Self {
        let mut out = Self::default();
        for fill in fills {
            out.push(fill);
        }
        out
    }
}

/// An inclusive box of SECTION coordinates `[cx, cy, cz]`.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct SectionBox {
    pub min: [i32; 3],
    pub max: [i32; 3],
}

impl SectionBox {
    pub fn contains(&self, section: [i32; 3]) -> bool {
        (0..3).all(|a| self.min[a] <= section[a] && section[a] <= self.max[a])
    }
}

/// Applied in order: `fills`, `blocks`, `authored`, `features`, `structures`; later writes win.
///
/// `nothing_in` are sections this feature writes nothing in, in this session: like a memo value
/// it must be a pure function of the world seed. The host stops dispatching the feature there,
/// so a feature that knows a whole region is empty says so once instead of answering every
/// section in it.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GenOutput {
    pub nothing_in: Vec<SectionBox>,
    pub fills: GenFills,
    pub blocks: Vec<GenWrite>,
    pub authored: AuthoredWrites,
    pub structures: Vec<StructurePlacement>,
    pub features: Vec<FeaturePlacement>,
    pub deferred: bool,
    /// What this feature writes in other sections, handed over ahead of their dispatch.
    pub ahead: Vec<SectionOutput>,
}

/// A feature's output for another section, handed over ahead of that section's dispatch: the
/// host applies it when the section generates instead of dispatching the feature there. It must
/// be what the feature would answer for that section, and carries no further `ahead` itself.
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct SectionOutput {
    pub section: [i32; 3],
    pub output: GenOutput,
}

impl GenOutput {
    pub fn deferred() -> Self {
        Self {
            deferred: true,
            ..Self::default()
        }
    }
}

impl From<Vec<GenWrite>> for GenOutput {
    fn from(blocks: Vec<GenWrite>) -> Self {
        Self {
            blocks,
            ..Self::default()
        }
    }
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestCall {
    TickSystem {
        id: u32,
    },
    HandleEvent {
        id: u32,
        payload: EventPayload,
    },

    GenFeature {
        feature_id: u32,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u16>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },
    GenStage {
        callback_id: u32,
        stage: WorldgenStage,
        section_pos: [i32; 3],
        seed: u32,
        blocks: Vec<u16>,
        surface_heights: Vec<i32>,
        biomes: Vec<u8>,
        sea_level: i32,
    },

    GuiClick {
        kind_key: String,
        widget_id: String,
        at: Option<crate::ContainerAddress>,
    },

    HostileSpawnCandidate {
        callback_id: u32,
        candidate: HostileSpawnCandidate,
    },

    BlockBehavior {
        callback_id: u32,
        kind: BlockHookKind,
        pos: [i32; 3],
    },

    /// One AI decision for one mob this tick, for the node registered via
    /// [`CoreCall::RegisterAiNode`](crate::CoreCall::RegisterAiNode).
    /// Decision-only, no sim scope. World edits, spawns and player state error here; `CurrentTick`,
    /// RNG and log work fine, and `ctx.tick` already has the tick.
    /// Return desires via [`GuestRet::AiDecision`], brain arbitration merges by priority.
    /// On ABI 2.1+ the engine batches via [`GuestCall::AiNodeBatch`] instead; this is just the
    /// fallback for older guests.
    AiNode {
        callback_id: u32,
        ctx: AiNodeCtx,
    },

    ClientFrame {
        frame: ClientFrameData,
    },
    ClientKey {
        action_id: u32,
        pressed: bool,
    },
    ClientUi {
        kind_key: String,
        event: ClientUiEvent,
    },
    ClientCanvas {
        canvas_key: String,
        event: ClientCanvasEvent,
    },
    ClientCanvasScroll {
        canvas_key: String,
        x: f32,
        y: f32,
        delta: f32,
    },

    BakeShapeSim {
        shape_kind: u16,
        cells: Vec<CellInput>,
    },
    BakeShapeRender {
        shape_kind: u16,
        cells: Vec<CellInput>,
    },
    BakeShapeItem {
        shape_kind: u16,
        block_id: BlockId,
    },
    ShapePlacementPlan {
        shape_kind: u16,
        block_id: BlockId,
        inputs: PlaceInputsView,
    },

    AiNodeBatch {
        callback_id: u32,
        ctxs: Vec<AiNodeCtx>,
    },

    /// Every claim touching the columns `min..=max`, from a feature registered
    /// [`with_claims`](crate::GenFeatureFilter): columns it keeps for itself, which the engine's
    /// trees stay out of. Answered with [`GuestRet::GenClaims`]; like a memo value the answer
    /// must be a pure function of the world seed.
    GenClaims {
        feature_id: u32,
        seed: u32,
        sea_level: i32,
        min: [i32; 2],
        max: [i32; 2],
    },
}

#[derive(Serialize, Deserialize, Clone, Debug, PartialEq)]
pub enum GuestRet {
    Unit,
    Event {
        outcome: Outcome,
        payload: Option<EventPayload>,
    },
    GenOutput(GenOutput),
    GenBlocks(Vec<u16>),
    GenBiomes(#[serde(with = "serde_bytes")] Vec<u8>),
    HostileSpawn(Option<String>),
    AiDecision(Option<AiNodeDecision>),

    BakedSim(Vec<BakedSimCell>),
    BakedRender(Vec<BakedRenderCell>),
    BakedItem(BakedItemGeometry),
    ShapePlacement(ShapePlacementResult),
    Unsupported,
    AiDecisions(Vec<Option<AiNodeDecision>>),
    GenClaims(GenClaims),
}

/// An inclusive rectangle of columns.
#[derive(Serialize, Deserialize, Clone, Copy, Debug, PartialEq, Eq)]
pub struct ColumnBox {
    pub min: [i32; 2],
    pub max: [i32; 2],
}

impl ColumnBox {
    pub fn contains(&self, [x, z]: [i32; 2]) -> bool {
        self.min[0] <= x && x <= self.max[0] && self.min[1] <= z && z <= self.max[1]
    }

    pub fn overlaps(&self, other: &ColumnBox) -> bool {
        (0..2).all(|a| self.min[a] <= other.max[a] && other.min[a] <= self.max[a])
    }
}

/// A set of columns inside `bounds`, a bit per column row by row along x (lowest bit first).
#[derive(Serialize, Deserialize, Clone, Debug, PartialEq, Eq)]
pub struct ColumnMask {
    pub bounds: ColumnBox,
    #[serde(with = "serde_bytes")]
    pub bits: Vec<u8>,
}

impl ColumnMask {
    /// A mask over `bounds` holding no column.
    pub fn empty(bounds: ColumnBox) -> ColumnMask {
        let side = |a: usize| (bounds.max[a] - bounds.min[a] + 1).max(0) as usize;
        ColumnMask {
            bounds,
            bits: vec![0; (side(0) * side(1)).div_ceil(8)],
        }
    }

    /// A mask over `bounds` holding every column `has` accepts.
    pub fn from_fn(bounds: ColumnBox, has: impl Fn([i32; 2]) -> bool) -> ColumnMask {
        let mut mask = ColumnMask::empty(bounds);
        for z in bounds.min[1]..=bounds.max[1] {
            for x in bounds.min[0]..=bounds.max[0] {
                if has([x, z]) {
                    mask.insert_run(z, x, x);
                }
            }
        }
        mask
    }

    /// Adds the columns `x0..=x1` of row `z`, clipped to the bounds.
    pub fn insert_run(&mut self, z: i32, x0: i32, x1: i32) {
        let b = self.bounds;
        let (x0, x1) = (x0.max(b.min[0]), x1.min(b.max[0]));
        if x0 > x1 || z < b.min[1] || z > b.max[1] {
            return;
        }
        let row = (z - b.min[1]) as usize * (b.max[0] - b.min[0] + 1) as usize;
        let (first, last) = (
            row + (x0 - b.min[0]) as usize,
            row + (x1 - b.min[0]) as usize,
        );
        let (head, tail) = (0xffu8 << (first % 8), 0xffu8 >> (7 - last % 8));
        if first / 8 == last / 8 {
            self.bits[first / 8] |= head & tail;
        } else {
            self.bits[first / 8] |= head;
            self.bits[first / 8 + 1..last / 8].fill(0xff);
            self.bits[last / 8] |= tail;
        }
    }

    /// Whether the bits cover the bounds exactly.
    pub fn is_well_formed(&self) -> bool {
        let side = |a: usize| i128::from(self.bounds.max[a]) - i128::from(self.bounds.min[a]) + 1;
        let (w, d) = (side(0), side(1));
        w > 0 && d > 0 && (w * d + 7) / 8 == self.bits.len() as i128
    }

    #[inline]
    pub fn has(&self, c: [i32; 2]) -> bool {
        if !self.bounds.contains(c) {
            return false;
        }
        let width = (self.bounds.max[0] - self.bounds.min[0] + 1) as usize;
        let i = (c[1] - self.bounds.min[1]) as usize * width + (c[0] - self.bounds.min[0]) as usize;
        self.bits[i / 8] >> (i % 8) & 1 != 0
    }
}

/// The reply to [`GuestCall::GenClaims`].
#[derive(Serialize, Deserialize, Clone, Debug, Default, PartialEq, Eq)]
pub struct GenClaims {
    /// Every claim touching the asked-about columns, whole.
    pub claims: Vec<ColumnMask>,
    /// The claims depend on a memo another worker is still computing: ask again once it lands.
    pub deferred: bool,
    /// Section outputs of the structures these claims were worked out from, as in
    /// [`GenOutput::ahead`].
    pub ahead: Vec<SectionOutput>,
}
