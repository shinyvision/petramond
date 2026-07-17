use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkPos, SectionPos};

/// A shared byte buffer on the wire: refcount-bumped over the local
/// connection, serialized as plain bytes over TCP (deserialization allocates a
/// fresh `Arc`, which the remap then rewrites in place — no extra copies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SectionBytes(pub Arc<[u8]>);

impl Serialize for SectionBytes {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&self.0)
    }
}

impl<'de> Deserialize<'de> for SectionBytes {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'a> serde::de::Visitor<'a> for V {
            type Value = SectionBytes;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a byte buffer")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<SectionBytes, E> {
                Ok(SectionBytes(Arc::from(v)))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<SectionBytes, E> {
                Ok(SectionBytes(Arc::from(v.into_boxed_slice())))
            }
            fn visit_seq<A: serde::de::SeqAccess<'a>>(
                self,
                mut seq: A,
            ) -> Result<SectionBytes, A::Error> {
                let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    v.push(b);
                }
                Ok(SectionBytes(Arc::from(v.into_boxed_slice())))
            }
        }
        d.deserialize_bytes(V)
    }
}

/// A column's client-relevant facts: the biome skin, visible surface,
/// direct-sky cover, and a per-cy section summary so replica physics can answer
/// for ABSENT sections without running worldgen. Sent before the column's first
/// section.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct ColumnPayload {
    pub pos: ChunkPos,
    /// 16×16 biome ids, row-major (z * 16 + x).
    pub biomes: SectionBytes,
    /// 20x20 biome tint halo (two cells beyond each column edge), captured by
    /// column generation and reused by every section mesh in this column.
    pub mesh_biomes: SectionBytes,
    /// 16×16 visible surface heights, same order.
    pub surface_heightmap: Vec<i32>,
    /// 16×16 highest direct-skylight blockers. Differs from
    /// `surface_heightmap` when clear blocks such as glass sit above the real
    /// sky cover.
    pub sky_cover: Vec<i32>,
    /// `SectionSummary` discriminants for every cy in world order — lets the
    /// replica treat absent `FullOpaque`/`FullWater` sections truthfully.
    pub summaries: Vec<u8>,
    /// Lowest section in the surface retention band. Sections below it are
    /// eligible for replica deep-visibility parking.
    pub deep_band_lo: i32,
}

/// One 16³ section's full streamed content — the wire sibling of the save's
/// `SectionSnapshot`, Arc-backed so the local connection ships refcount bumps.
/// One cached section a joining client claims to still hold, by the
/// server-domain content hash the server vouched at unload time.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub(crate) struct SectionCacheClaim {
    pub pos: SectionPos,
    pub hash: u64,
}

/// Entry cap for the client section cache AND the server's per-connection
/// belief map. Both sides insert in the same order (unloads ride the ordered
/// stream) and evict oldest-first, so the two stay aligned without eviction
/// chatter; any residual drift heals through `SectionCacheMiss`. ~4k sections
/// ≈ a generous re-explorable ring at RD32 while bounding worst-case replica
/// memory to a few hundred MB.
pub(crate) const SECTION_CACHE_CAP: usize = 4096;

/// Container SLOT contents, mobs, and dropped items are deliberately absent:
/// they replicate through menu sync and entity batches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct SectionPayload {
    pub pos: SectionPos,
    /// 4096 wire block ids.
    pub blocks: SectionBytes,
    /// Block-derived counters and boundary planes. The replica adopts these
    /// with the shared buffers instead of rescanning the section on its frame.
    pub metrics: crate::section::SectionMetrics,
    /// 4096 water meta bytes, present when any cell holds water.
    pub water: Option<SectionBytes>,
    /// Server-baked light. The ship gate (`plan_terrain_send`) holds a section
    /// back until its light is final, so this is `None` ONLY for sections that
    /// never bake (fully opaque). Replica ingest does no light work of its own;
    /// local predicted edits may compute disposable presentation light.
    /// Post-install rebakes arrive as [`LightData`](ServerToClient::LightData).
    pub skylight: Option<SectionBytes>,
    pub blocklight: Option<SectionBytes>,
    /// Sparse per-cell block states (doors, stairs, slabs, log axes, torches,
    /// saplings, model cells, facings, lit furnaces, cell KV).
    pub states: SectionStatesPayload,
}

impl SectionPayload {
    /// The SERVER-DOMAIN content fingerprint behind the section cache: a hash
    /// of the payload's postcard encoding, so every current and future field
    /// is covered without a parallel hash implementation to keep in sync.
    /// `to_payload` emits every sparse list cell-sorted, so identical content
    /// hashes identically. Raw session ids make this meaningless outside the
    /// process runs that share this server's registries — the in-memory
    /// session cache is its only valid consumer; NEVER persist these hashes.
    pub(crate) fn content_hash(&self) -> u64 {
        use std::hash::Hasher;
        let bytes = postcard::to_allocvec(self).expect("section payload postcard-encodes");
        let mut h = rustc_hash::FxHasher::default();
        h.write(&bytes);
        h.finish()
    }
}

/// One section's freshly baked light cubes — shipped whenever a server bake
/// lands for a section in the recipient's sent set (rebakes after edits and
/// after a neighbour's landing invalidated a seam). Arc-backed like
/// [`SectionPayload`]: the local pipe ships refcount bumps.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub(crate) struct LightPayload {
    pub pos: SectionPos,
    /// 4096 skylight bytes (x2 scale).
    pub skylight: SectionBytes,
    /// 4096 block-light bytes; `None` when no emitter reaches the section
    /// (reads as all-zero, mirroring `Section::set_blocklight`'s compaction).
    pub blocklight: Option<SectionBytes>,
}

/// The sparse per-cell state maps a section carries beyond raw block ids.
/// Cell keys are the section-local u16 cell index; every entry list is sorted
/// by cell so identical state encodes identically. Encodings are EXACTLY the
/// save codec's per-entry bytes (`save::codec::encode_snapshot`) — the wire
/// delegates to the same `encode`/`to_u8` state packers, so replication is as
/// lossless as a save/load roundtrip. Built/consumed by `world::remote`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub(crate) struct SectionStatesPayload {
    /// (cell, `DoorState::encode` byte)
    pub doors: Vec<(u16, u8)>,
    /// (cell, `StairState::encode` byte)
    pub stairs: Vec<(u16, u8)>,
    /// (cell, [`SlabState::encode_meta`, layer 0 block id, layer 1 block id])
    /// — the save codec's 3-byte record, with RAW session block ids.
    pub slabs: Vec<(u16, [u8; 3])>,
    /// (cell, `LogAxis::to_u8` byte)
    pub log_axes: Vec<(u16, u8)>,
    /// (cell, `TorchPlacement::to_u8` byte)
    pub torches: Vec<(u16, u8)>,
    /// (cell, sapling growth stage)
    pub saplings: Vec<(u16, u8)>,
    /// (cell, `Facing::to_u8` byte) — chest/furnace block-entity fronts.
    pub entity_facings: Vec<(u16, u8)>,
    /// (cell, `Facing::to_u8` byte) — oriented bbmodel blocks.
    pub model_facings: Vec<(u16, u8)>,
    /// (cell, authored footprint offset) for multi-cell model blocks.
    pub model_cells: Vec<(u16, [u8; 3])>,
    /// Cells whose furnace is LIT. Machine state (burn/cook counters) is sim
    /// state and stays server-side; the replica only needs the lit face.
    pub furnaces_lit: Vec<u16>,
    /// Per-cell mod KV, preserved opaquely (entries sorted by key — the map
    /// is a `BTreeMap` section-side).
    pub cell_kv: Vec<CellKvEntry>,
}

/// One cell's opaque mod KV: `(cell, sorted (key, value-bytes) entries)` —
/// the wire mirror of the section's per-cell `BTreeMap`.
pub(crate) type CellKvEntry = (u16, Vec<(String, Vec<u8>)>);
