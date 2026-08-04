use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::chunk::{ChunkPos, SectionPos};

/// A shared byte buffer on the wire: refcount-bumped over the local
/// connection, serialized as plain bytes over TCP (deserialization allocates a
/// fresh `Arc`, which the remap then rewrites in place — no extra copies).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionBytes(pub Arc<[u8]>);

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

/// A section's BLOCK-ID cube on the wire. Block ids are two bytes, so the
/// naive encoding would double every section frame; this ships the same
/// per-section palette the save record uses — `[distinct: u16][ids: u16 x
/// distinct][one index per cell]`, index width one byte while the section
/// holds ≤ 256 distinct blocks — which keeps a section payload the size it was
/// when ids were bytes. Local connections still ship a refcount bump.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionBlocks(pub Arc<[u16]>);

/// Palette-encode a block cube into the wire/save byte form.
fn pack_blocks(blocks: &[u16]) -> Vec<u8> {
    let mut ids: Vec<u16> = Vec::new();
    let mut index: Vec<u16> = Vec::with_capacity(blocks.len());
    for &b in blocks {
        let at = match ids.iter().position(|&p| p == b) {
            Some(i) => i,
            None => {
                ids.push(b);
                ids.len() - 1
            }
        };
        index.push(at as u16);
    }
    let wide = ids.len() > u8::MAX as usize + 1;
    let mut out = Vec::with_capacity(6 + ids.len() * 2 + index.len() * (1 + wide as usize));
    out.extend_from_slice(&(blocks.len() as u32).to_le_bytes());
    out.extend_from_slice(&(ids.len() as u16).to_le_bytes());
    for id in &ids {
        out.extend_from_slice(&id.to_le_bytes());
    }
    if wide {
        for i in &index {
            out.extend_from_slice(&i.to_le_bytes());
        }
    } else {
        out.extend(index.iter().map(|&i| i as u8));
    }
    out
}

/// Inverse of [`pack_blocks`]; `None` on a malformed buffer.
fn unpack_blocks(v: &[u8]) -> Option<Arc<[u16]>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = v.get(at..at + n)?;
        at += n;
        Some(s)
    };
    let cells = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
    if cells > crate::chunk::SECTION_VOLUME {
        return None;
    }
    let distinct = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
    let mut ids = Vec::with_capacity(distinct);
    for _ in 0..distinct {
        ids.push(u16::from_le_bytes(take(2)?.try_into().ok()?));
    }
    let mut out = Vec::with_capacity(cells);
    if distinct > u8::MAX as usize + 1 {
        for _ in 0..cells {
            let i = u16::from_le_bytes(take(2)?.try_into().ok()?) as usize;
            out.push(*ids.get(i)?);
        }
    } else {
        for &i in take(cells)? {
            out.push(*ids.get(i as usize)?);
        }
    }
    Some(Arc::from(out.into_boxed_slice()))
}

impl Serialize for SectionBlocks {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&pack_blocks(&self.0))
    }
}

impl<'de> Deserialize<'de> for SectionBlocks {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'a> serde::de::Visitor<'a> for V {
            type Value = SectionBlocks;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a palette-packed block cube")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<SectionBlocks, E> {
                unpack_blocks(v)
                    .map(SectionBlocks)
                    .ok_or_else(|| E::custom("malformed block cube"))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<SectionBlocks, E> {
                unpack_blocks(&v)
                    .map(SectionBlocks)
                    .ok_or_else(|| E::custom("malformed block cube"))
            }
            fn visit_seq<A: serde::de::SeqAccess<'a>>(
                self,
                mut seq: A,
            ) -> Result<SectionBlocks, A::Error> {
                let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    v.push(b);
                }
                unpack_blocks(&v)
                    .map(SectionBlocks)
                    .ok_or_else(|| serde::de::Error::custom("malformed block cube"))
            }
        }
        d.deserialize_bytes(V)
    }
}

/// A shared BLOCK-LIGHT cube on the wire: the sibling of [`SectionBytes`] for
/// the packed RGB cell. Same deal — the local connection ships a refcount
/// bump; TCP pays one little-endian byte pass in each direction. Decode forces
/// canonical cells (see [`LightRgb::from_bits`]) so a mangled frame cannot
/// introduce a second spelling of black and desync the region diff.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionLight(pub Arc<[crate::light::LightRgb]>);

impl Serialize for SectionLight {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&crate::light::to_le_bytes(&self.0))
    }
}

impl<'de> Deserialize<'de> for SectionLight {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct V;
        impl<'a> serde::de::Visitor<'a> for V {
            type Value = SectionLight;
            fn expecting(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
                f.write_str("a packed RGB light buffer")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<SectionLight, E> {
                let cells = crate::light::from_le_bytes(v)
                    .ok_or_else(|| E::custom("odd-length light buffer"))?;
                Ok(SectionLight(Arc::from(cells)))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<SectionLight, E> {
                self.visit_bytes(&v)
            }
            fn visit_seq<A: serde::de::SeqAccess<'a>>(
                self,
                mut seq: A,
            ) -> Result<SectionLight, A::Error> {
                let mut v = Vec::with_capacity(seq.size_hint().unwrap_or(0));
                while let Some(b) = seq.next_element::<u8>()? {
                    v.push(b);
                }
                self.visit_bytes(&v)
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
pub struct ColumnPayload {
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
pub struct SectionCacheClaim {
    pub pos: SectionPos,
    pub hash: u64,
}

/// Entry cap for the client section cache AND the server's per-connection
/// belief map. Both sides insert in the same order (unloads ride the ordered
/// stream) and evict oldest-first, so the two stay aligned without eviction
/// chatter; any residual drift heals through `SectionCacheMiss`. ~4k sections
/// ≈ a generous re-explorable ring at RD32 while bounding worst-case replica
/// memory to a few hundred MB.
pub const SECTION_CACHE_CAP: usize = 4096;

/// Container SLOT contents, mobs, and dropped items are deliberately absent:
/// they replicate through menu sync and entity batches.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionPayload {
    pub pos: SectionPos,
    /// 4096 wire block ids.
    pub blocks: SectionBlocks,
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
    pub blocklight: Option<SectionLight>,
    /// Sparse per-cell block states (doors, stairs, slabs, log axes, torches,
    /// model cells, facings, cell KV).
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
    pub fn content_hash(&self) -> u64 {
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
pub struct LightPayload {
    pub pos: SectionPos,
    /// 4096 skylight bytes (x2 scale).
    pub skylight: SectionBytes,
    /// 4096 packed RGB block-light cells; `None` when no emitter reaches the
    /// section (reads as all-dark, mirroring `Section::set_blocklight`).
    pub blocklight: Option<SectionLight>,
}

/// The sparse per-cell state maps a section carries beyond raw block ids.
/// Cell keys are the section-local u16 cell index; every entry list is sorted
/// by cell so identical state encodes identically. Encodings are EXACTLY the
/// save codec's per-entry bytes (`save::codec::encode_snapshot`) — the wire
/// delegates to the same `encode`/`to_u8` state packers, so replication is as
/// lossless as a save/load roundtrip. Built/consumed by `world::remote`.
#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionStatesPayload {
    /// The section's UNIFIED per-cell block state, cell-sorted: verbatim
    /// store bytes (opaque to the transport except the id-masked BLOCK-ID
    /// bytes, rewritten through the block LUT at the boundary via
    /// `ShapeState::remap_ids`).
    pub cell_states: Vec<(u16, crate::block::ShapeState)>,
    /// Per-cell mod KV, preserved opaquely (entries sorted by key — the map
    /// is a `BTreeMap` section-side).
    pub cell_kv: Vec<CellKvEntry>,
    /// Mod-submitted per-block DRAW SETS in this section (`world::draw`),
    /// cell-sorted.
    ///
    /// They ride the section rather than only the per-tick delta lane because
    /// a set is retained per-block state: the delta carries CHANGES, so a
    /// machine that last redrew itself an hour ago would be invisible to
    /// everyone who joined since — and a mod cannot force a resend, because
    /// resubmitting an unchanged set logs nothing by design.
    pub draws: Vec<BlockDrawEntry>,
}

/// One cell's draw set on the wire: `(cell, prims)`, in the mod's own
/// submitted form — names, like every other replicated identity.
pub type BlockDrawEntry = (u16, crate::world::draw::DrawPrims);

/// One cell's opaque mod KV: `(cell, sorted (key, value-bytes) entries)` —
/// the wire mirror of the section's per-cell `BTreeMap`.
pub type CellKvEntry = (u16, Vec<(String, Vec<u8>)>);
