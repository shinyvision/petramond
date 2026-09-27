use std::sync::Arc;

use serde::{Deserialize, Serialize};

use petramond_math::math::IVec3;
use petramond_world::chunk::{ChunkPos, SectionPos};

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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionBlocks(pub Arc<[u16]>);

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

fn unpack_blocks(v: &[u8]) -> Option<Arc<[u16]>> {
    let mut at = 0usize;
    let mut take = |n: usize| -> Option<&[u8]> {
        let s = v.get(at..at + n)?;
        at += n;
        Some(s)
    };
    let cells = u32::from_le_bytes(take(4)?.try_into().ok()?) as usize;
    if cells > petramond_world::chunk::SECTION_VOLUME {
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

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SectionLight(pub Arc<[petramond_world::light::LightRgb]>);

impl Serialize for SectionLight {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_bytes(&petramond_world::light::to_le_bytes(&self.0))
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
                let cells = petramond_world::light::from_le_bytes(v)
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

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct ColumnPayload {
    pub pos: ChunkPos,
    pub biomes: SectionBytes,
    pub mesh_biomes: SectionBytes,
    pub surface_heightmap: Vec<i32>,
    pub sky_cover: Vec<i32>,
    pub summaries: Vec<u8>,
    pub deep_band_lo: i32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SectionPayload {
    pub pos: SectionPos,
    pub blocks: SectionBlocks,
    pub metrics: petramond_world::section::SectionMetrics,
    pub fluid: Option<SectionBytes>,
    pub skylight: Option<SectionBytes>,
    pub blocklight: Option<SectionLight>,
    pub states: SectionStatesPayload,
}

impl SectionPayload {
    pub fn content_hash(&self) -> u64 {
        use std::hash::Hasher;
        let bytes = postcard::to_allocvec(self).expect("section payload postcard-encodes");
        let mut h = rustc_hash::FxHasher::default();
        h.write(&bytes);
        h.finish()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LightPayload {
    pub pos: SectionPos,
    pub skylight: SectionBytes,
    pub blocklight: Option<SectionLight>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct SectionStatesPayload {
    pub cell_states: Vec<(u16, petramond_world::block::ShapeState)>,
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

pub type BlockDrawEntry = (u16, super::draw::DrawPrims);

pub type CellKvEntry = (u16, Vec<(String, Vec<u8>)>);

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct BlockDelta {
    pub pos: IVec3,
    pub block_id: u16,
    pub fluid: Option<u8>,
    pub state: Option<petramond_world::block::ShapeState>,
    /// The cell's mod KV map after the change (empty for the common cell).
    /// A delta ALWAYS carries the cell's current KV because the replica's
    /// apply wipes the cell's KV exactly like a server-side write — without
    /// this, a CORRECTIVE delta (a snapshot of an UNCHANGED cell) would
    /// erase replica KV the server still holds. Sorted (BTreeMap iteration), so the wire is
    /// deterministic.
    pub cell_kv: Vec<(String, Vec<u8>)>,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct CellKvDelta {
    pub pos: IVec3,
    pub key: String,
    pub value: Option<Vec<u8>>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BlockDrawDelta {
    pub pos: IVec3,
    pub prims: super::draw::DrawPrims,
}
