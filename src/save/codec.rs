//! Binary codec for save data: little-endian primitives + compressed section
//! records.
//!
//! A section record stores only what generation can't reproduce for one 16³ cube —
//! block ids and (when present) per-cell fluid metadata — then zlib-compresses
//! the lot
//! (flate2 / miniz_oxide, pure Rust). Biome and surface heightmap are per-column,
//! cheaply regenerated, and so are never written here. Baked light IS persisted
//! (when clean at snapshot time), so a reload samples the saved cubes instead of
//! re-baking the whole explored area; the cubes are mostly uniform and deflate
//! to almost nothing.

mod cell_state;
#[cfg(test)]
mod fixtures;
mod item_slot;
mod kept;
#[cfg(test)]
mod tests;
mod v19;
mod v20;

pub use item_slot::DiskSlot;
pub use petramond_persist::bytecodec::{deflate, inflate, Reader};
pub use petramond_persist::bytecodec::{
    get_indexed, get_kv_map, put_indexed, put_kv_map, put_u16, put_u32, put_u64, put_u8,
};

use std::collections::BTreeMap;
use std::sync::Arc;

use crate::entity::DroppedItem;
use crate::mob::SavedMob;
use petramond_world::block::ShapeState;
use petramond_world::chunk::{SectionPos, SECTION_VOLUME};
use petramond_world::container::Container;
use petramond_world::furnace::Furnace;
use petramond_world::section::{CellMap, Section};

use super::entities::EntityRecord;
use super::format::{Format, RecordError};
use super::mobs::DiskMob;
use super::palette;
use kept::{KeptBlock, KeptCells};

const SECTION_REC_VERSION: u8 = 21;

pub(super) const SECTION: Format = Format::new(
    "section record",
    SECTION_REC_VERSION as u32,
    &[v19::upgrade, v20::upgrade],
);

const FLAG_HAS_FLUID: u8 = 0x01;
const FLAG_HAS_ENTITIES: u8 = 0x02;
const FLAG_HAS_FURNACES: u8 = 0x04;
const FLAG_HAS_CELL_STATES: u8 = 0x08;
const FLAG_HAS_MOBS: u8 = 0x20;
const FLAG2_HAS_CELL_KV: u8 = 0x08;
const FLAG2_HAS_CONTAINERS: u8 = 0x20;
const FLAG3_HAS_SKYLIGHT: u8 = 0x02;
const FLAG3_HAS_BLOCKLIGHT: u8 = 0x04;
const KNOWN_FLAGS: [u8; 3] = [
    FLAG_HAS_FLUID | FLAG_HAS_ENTITIES | FLAG_HAS_FURNACES | FLAG_HAS_CELL_STATES | FLAG_HAS_MOBS,
    FLAG2_HAS_CELL_KV | FLAG2_HAS_CONTAINERS,
    FLAG3_HAS_SKYLIGHT | FLAG3_HAS_BLOCKLIGHT,
];

pub struct SectionSnapshot {
    pub pos: SectionPos,
    pub cache_only: bool,
    pub blocks: petramond_world::section::BlockCube,
    pub fluid: Option<Arc<[u8]>>,
    pub entities: Vec<DroppedItem>,
    pub furnaces: CellMap<Furnace>,
    pub containers: CellMap<Container>,
    pub cell_states: CellMap<ShapeState>,
    pub skylight: Option<Arc<[u8]>>,
    pub blocklight: Option<Arc<[petramond_world::light::LightRgb]>>,
    pub cell_kv: CellMap<BTreeMap<String, Vec<u8>>>,
    pub mobs: Vec<SavedMob>,
    pub kept: KeptContent,
}

impl SectionSnapshot {
    pub fn from_section(s: &Section) -> Self {
        Self {
            pos: SectionPos::new(s.cx, s.cy, s.cz),
            cache_only: false,
            blocks: s.block_cube(),
            fluid: s.fluid_arc(),
            entities: Vec::new(),
            furnaces: s.furnaces().clone(),
            containers: s.containers().clone(),
            cell_states: s.cell_states().clone(),
            skylight: (!s.light_dirty).then(|| s.skylight_arc()).flatten(),
            blocklight: (!s.light_dirty).then(|| s.blocklight_arc()).flatten(),
            cell_kv: s.cell_kv().clone(),
            mobs: Vec::new(),
            kept: KeptContent::default(),
        }
    }
}

/// Section content this build cannot bring to life — mobs and item entities
/// it cannot represent (see `save::mobs`, `save::entities`) — kept in disk
/// form. The save holds it beside the section while the section is loaded
/// and writes it back with every save of the section, so it returns when
/// its mod does. (Kept blocks and container slots ride the section's own
/// cell KV instead: they belong to one cell, and a write to that cell must
/// discard them — see `kept`.)
#[derive(Clone, Debug, Default, PartialEq)]
pub struct KeptContent {
    pub mobs: Vec<DiskMob>,
    pub entities: Vec<EntityRecord>,
}

impl KeptContent {
    pub fn is_empty(&self) -> bool {
        self.mobs.is_empty() && self.entities.is_empty()
    }
}

pub fn encode_snapshot(s: &SectionSnapshot, pal: &palette::Palette) -> Vec<u8> {
    let kept = KeptCells::of(s);
    let cell_states = kept.cell_states(&s.cell_states);
    let cell_kv = kept::stored_kv(&s.cell_kv);
    let light = kept.blocks.is_empty();
    let has_entities = !s.entities.is_empty() || !s.kept.entities.is_empty();
    let has_mobs = !s.mobs.is_empty() || !s.kept.mobs.is_empty();

    let extra = s.fluid.as_ref().map_or(0, |w| w.len());
    let mut payload = Vec::with_capacity(4 + s.blocks.len() + extra);
    put_u8(&mut payload, SECTION_REC_VERSION);
    let mut flags = 0u8;
    if s.fluid.is_some() {
        flags |= FLAG_HAS_FLUID;
    }
    if has_entities {
        flags |= FLAG_HAS_ENTITIES;
    }
    if !s.furnaces.is_empty() {
        flags |= FLAG_HAS_FURNACES;
    }
    if !cell_states.is_empty() {
        flags |= FLAG_HAS_CELL_STATES;
    }
    if has_mobs {
        flags |= FLAG_HAS_MOBS;
    }
    let mut flags2 = 0u8;
    if !cell_kv.is_empty() {
        flags2 |= FLAG2_HAS_CELL_KV;
    }
    if !s.containers.is_empty() {
        flags2 |= FLAG2_HAS_CONTAINERS;
    }
    let skylight = s.skylight.as_ref().filter(|_| light);
    let blocklight = s.blocklight.as_ref().filter(|_| light);
    let mut flags3 = 0u8;
    if skylight.is_some() {
        flags3 |= FLAG3_HAS_SKYLIGHT;
    }
    if blocklight.is_some() {
        flags3 |= FLAG3_HAS_BLOCKLIGHT;
    }
    put_u8(&mut payload, flags);
    put_u8(&mut payload, flags2);
    put_u8(&mut payload, flags3);
    put_block_cube(&mut payload, &s.blocks, pal, &kept.blocks);
    if let Some(w) = &s.fluid {
        put_framed(&mut payload, |buf| buf.extend_from_slice(w));
    }
    if has_entities {
        put_framed(&mut payload, |buf| {
            super::entities::put_entities(buf, &s.entities, &s.kept.entities, pal)
        });
    }
    if !s.furnaces.is_empty() {
        put_framed(&mut payload, |buf| {
            super::furnace::put_furnaces(buf, &s.furnaces)
        });
    }
    if !cell_states.is_empty() {
        put_framed(&mut payload, |buf| {
            put_indexed(buf, &cell_states, 8, |buf, state| state.put(buf, pal))
        });
    }
    if has_mobs {
        put_framed(&mut payload, |buf| {
            super::mobs::put_mobs(buf, &s.mobs, &s.kept.mobs, pal)
        });
    }
    if !cell_kv.is_empty() {
        put_framed(&mut payload, |buf| {
            put_indexed(buf, &*cell_kv, 16, |buf, map| {
                put_kv_map(buf, map);
            })
        });
    }
    if !s.containers.is_empty() {
        put_framed(&mut payload, |buf| {
            super::container::put_containers(buf, &s.containers, &kept.slots, pal)
        });
    }
    if let Some(sky) = skylight {
        put_framed(&mut payload, |buf| buf.extend_from_slice(sky));
    }
    if let Some(bl) = blocklight {
        put_framed(&mut payload, |buf| {
            buf.extend_from_slice(&petramond_world::light::to_le_bytes(bl))
        });
    }
    deflate(&payload)
}

/// A section's block cube on disk: `[distinct: u16][ids: u16 x distinct]` then
/// one index per cell — a BYTE while the section holds ≤ 256 distinct blocks
/// (every section a world ever produces) and a `u16` otherwise. The palette is
/// what keeps the record ~4 KiB after block ids widened to two bytes, and it
/// also gives deflate a much shorter dictionary to chew on. A cell holding a
/// kept block writes the kept disk id.
fn put_block_cube(
    buf: &mut Vec<u8>,
    blocks: &petramond_world::section::BlockCube,
    pal: &palette::Palette,
    kept: &CellMap<KeptBlock>,
) {
    let mut ids: Vec<u16> = Vec::new();
    let mut index: Vec<u16> = Vec::with_capacity(blocks.len());
    for (cell, b) in blocks.iter().enumerate() {
        let disk = if kept.is_empty() {
            pal.block_to_disk(b)
        } else {
            kept.get(&(cell as u16))
                .map_or_else(|| pal.block_to_disk(b), |block| block.disk)
        };
        let at = match ids.iter().position(|&p| p == disk) {
            Some(i) => i,
            None => {
                ids.push(disk);
                ids.len() - 1
            }
        };
        index.push(at as u16);
    }
    put_u16(buf, ids.len() as u16);
    for id in &ids {
        put_u16(buf, *id);
    }
    if ids.len() <= u8::MAX as usize + 1 {
        buf.extend(index.iter().map(|&i| i as u8));
    } else {
        for i in &index {
            put_u16(buf, *i);
        }
    }
}

type BlockCube = (Vec<u16>, Vec<(u16, u16)>);

fn get_block_cube(r: &mut Reader, pal: &palette::Palette) -> Option<BlockCube> {
    let distinct = r.u16()? as usize;
    if distinct == 0 || distinct > petramond_world::registry::WIDE_ID_CAP {
        return None;
    }
    let mut disk = Vec::with_capacity(distinct);
    for _ in 0..distinct {
        disk.push(r.u16()?);
    }
    let runtime: Vec<Option<u16>> = disk.iter().map(|&d| pal.block_from_disk_known(d)).collect();
    let mut out = Vec::with_capacity(SECTION_VOLUME);
    let mut unknown = Vec::new();
    let mut place = |cell: usize, i: usize| -> Option<()> {
        match *runtime.get(i)? {
            Some(id) => out.push(id),
            None => {
                out.push(0);
                unknown.push((cell as u16, disk[i]));
            }
        }
        Some(())
    };
    if distinct <= u8::MAX as usize + 1 {
        for (cell, &i) in r.bytes(SECTION_VOLUME)?.iter().enumerate() {
            place(cell, i as usize)?;
        }
    } else {
        for cell in 0..SECTION_VOLUME {
            let i = r.u16()? as usize;
            place(cell, i)?;
        }
    }
    Some((out, unknown))
}

fn put_framed(buf: &mut Vec<u8>, body: impl FnOnce(&mut Vec<u8>)) {
    let at = buf.len();
    put_u32(buf, 0);
    body(buf);
    let len = (buf.len() - at - 4) as u32;
    buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
}

pub struct DecodedSection {
    pub section: Section,
    pub entities: Vec<DroppedItem>,
    pub mobs: Vec<SavedMob>,
    pub kept: KeptContent,
}

pub fn decode_section(
    pos: SectionPos,
    blob: &[u8],
    pal: &palette::Palette,
) -> Result<DecodedSection, RecordError> {
    let payload = inflate(blob).ok_or(RecordError::corrupt(SECTION.name, "zlib stream", 0))?;
    let (&version, body) =
        payload
            .split_first()
            .ok_or(RecordError::corrupt(SECTION.name, "version", 0))?;
    let body = SECTION.upgrade(u32::from(version), body)?;
    decode_current(pos, &body, pal)
}

struct Frames<'a> {
    r: Reader<'a>,
}

struct Frame<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Frames<'a> {
    fn corrupt(&self, what: &'static str) -> RecordError {
        RecordError::corrupt(SECTION.name, what, self.r.offset() + 1)
    }

    fn payload(
        &mut self,
        present: bool,
        what: &'static str,
    ) -> Result<Option<Frame<'a>>, RecordError> {
        if !present {
            return Ok(None);
        }
        let len = self.r.u32().ok_or_else(|| self.corrupt(what))? as usize;
        let at = self.r.offset() + 1;
        let bytes = self.r.bytes(len).ok_or_else(|| self.corrupt(what))?;
        Ok(Some(Frame { bytes, at, what }))
    }
}

impl<'a> Frame<'a> {
    fn decode<T>(self, f: impl FnOnce(&mut Reader<'a>) -> Option<T>) -> Result<T, RecordError> {
        let mut r = Reader::new(self.bytes);
        match f(&mut r) {
            Some(value) if r.is_at_end() => Ok(value),
            _ => Err(RecordError::corrupt(
                SECTION.name,
                self.what,
                self.at + r.offset(),
            )),
        }
    }

    fn exact(self, len: usize) -> Result<&'a [u8], RecordError> {
        self.decode(|r| r.bytes(len))
    }
}

fn decode_current(
    pos: SectionPos,
    body: &[u8],
    pal: &palette::Palette,
) -> Result<DecodedSection, RecordError> {
    let mut frames = Frames {
        r: Reader::new(body),
    };
    let r = &mut frames.r;
    let (Some(flags), Some(flags2), Some(flags3)) = (r.u8(), r.u8(), r.u8()) else {
        return Err(frames.corrupt("flags"));
    };
    let unknown = [flags, flags2, flags3]
        .iter()
        .zip(KNOWN_FLAGS)
        .enumerate()
        .fold(0u32, |acc, (i, (f, known))| {
            acc | (u32::from(f & !known) << (8 * i))
        });
    if unknown != 0 {
        return Err(RecordError::UnknownPayload {
            format: SECTION.name,
            flags: unknown,
        });
    }
    let (blocks, unknown_cells) =
        get_block_cube(&mut frames.r, pal).ok_or_else(|| frames.corrupt("block cube"))?;
    let fluid = frames
        .payload(flags & FLAG_HAS_FLUID != 0, "fluid")?
        .map(|f| f.exact(SECTION_VOLUME))
        .transpose()?;
    let entities = frames
        .payload(flags & FLAG_HAS_ENTITIES != 0, "entities")?
        .map(|f| f.decode(|r| super::entities::get_entities(r, pal)))
        .transpose()?;
    let furnaces = frames
        .payload(flags & FLAG_HAS_FURNACES != 0, "furnaces")?
        .map(|f| f.decode(super::furnace::get_furnaces))
        .transpose()?;
    let stored_states = frames
        .payload(flags & FLAG_HAS_CELL_STATES != 0, "cell states")?
        .map(|f| f.decode(cell_state::get_stored))
        .transpose()?;
    let mobs = frames
        .payload(flags & FLAG_HAS_MOBS != 0, "mobs")?
        .map(|f| f.decode(|r| super::mobs::get_mobs(r, pal)))
        .transpose()?;
    let cell_kv = frames
        .payload(flags2 & FLAG2_HAS_CELL_KV != 0, "cell kv")?
        .map(|f| f.decode(|r| get_indexed(r, get_kv_map)))
        .transpose()?;
    let containers = frames
        .payload(flags2 & FLAG2_HAS_CONTAINERS != 0, "containers")?
        .map(|f| f.decode(|r| super::container::get_containers(r, pal)))
        .transpose()?;
    let skylight = frames
        .payload(flags3 & FLAG3_HAS_SKYLIGHT != 0, "skylight")?
        .map(|f| f.exact(SECTION_VOLUME))
        .transpose()?;
    let blocklight = frames
        .payload(flags3 & FLAG3_HAS_BLOCKLIGHT != 0, "block light")?
        .map(|f| f.decode(|r| petramond_world::light::from_le_bytes(r.bytes(SECTION_VOLUME * 2)?)))
        .transpose()?;
    if !frames.r.is_at_end() {
        return Err(frames.corrupt("trailing bytes"));
    }

    let (containers, kept_slots) = containers.unwrap_or_default();
    let (cell_states, cell_kv) = kept::restore_cells(
        unknown_cells,
        stored_states.unwrap_or_default(),
        cell_kv.unwrap_or_default(),
        kept_slots,
        pal,
    );
    let light_is_current = !cell_kv
        .values()
        .any(|map| map.contains_key(kept::KEPT_BLOCK_KEY));
    let mut section = Section::from_saved(
        pos.cx,
        pos.cy,
        pos.cz,
        &blocks,
        fluid.map(|w| w.to_vec().into_boxed_slice()),
        furnaces.unwrap_or_default(),
        containers,
        cell_states,
        cell_kv,
    );
    // Light came off disk clean, so seed the cache and clear `light_dirty`; the settle flush then
    // skips this section. `light_from_persist` tells cover-change invalidation to spare it when the
    // change is persisted content too. A now-kept block still forces a re-bake, because the light
    // was baked with it in place, not the air standing in for it.
    if let Some(sky) = skylight.filter(|_| light_is_current) {
        section.set_skylight(Arc::from(sky));
        if let Some(bl) = blocklight {
            section.set_blocklight(Arc::from(bl));
        }
        section.light_from_persist = true;
    }
    let (entities, kept_entities) = entities.unwrap_or_default();
    let (mobs, kept_mobs) = mobs.map(|m| (m.live, m.kept)).unwrap_or_default();
    Ok(DecodedSection {
        section,
        entities,
        mobs,
        kept: KeptContent {
            mobs: kept_mobs,
            entities: kept_entities,
        },
    })
}
