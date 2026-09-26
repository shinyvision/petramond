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

#[cfg(test)]
mod golden;
mod item_slot;
#[cfg(test)]
mod tests;
mod v19;

pub use item_slot::{get_item_slot, put_item_slot};
pub use petramond_util::bytecodec::{deflate, inflate, Reader};
pub use petramond_util::bytecodec::{
    get_indexed, get_kv_map, put_f32, put_f64, put_i64, put_indexed, put_kv_map, put_u16, put_u32,
    put_u64, put_u8,
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

use super::format::{Format, RecordError};
use super::palette;

/// Current section-record version. Flag-gated payloads are appended at the end, so a
/// new one that fits a free flag bit needs no version bump. The cubic format starts
/// fresh at `1` (the column-era chunk records are not migrated — saves regenerate).
/// v2 widens the per-mob record with the shear-regrow counter (see `save::mobs`);
/// v3 widens it again with the per-mob mod KV map (default-empty for older records).
/// v4 appends a third flags byte for slab layer state.
/// v5 unifies all slot storage into one generic container list (chests,
/// furnaces, mod containers), splits the furnace record into pure machine
/// state, and adds the shared entity-facing list. v5 is a CLEAN BREAK
/// (`SECTION_REC_MIN_VERSION` = 5): pre-v5 records do not load — the game is
/// unreleased, dev worlds regenerate.
/// v6 RETIRES the sapling-stage map (flags2 bit 0x01, now reserved): growth
/// stages became distinct block rows riding the ordinary block-id array.
/// Another clean break — a v5 record could carry a stage payload this build
/// has no store for.
/// v6 (same version, uncommitted same-day change) also retires the LADDER's
/// entity-facing record: ladder facing became four block rows, so the
/// entity-facing list holds genuine directional block-entity fronts
/// (chest/furnace) only. No bump — v6 never shipped; decode drops any facing
/// entry whose cell is not a directional-view block, so a same-day v6 record
/// with a ladder facing loads clean (the ladder id itself decodes as the
/// north-facing row).
/// v7 widens the per-mob record with the confined flag.
/// v8 replaces the confined boolean with a general mob tag map.
/// v9 stores the mob tag map's keys in a per-record string table: every tag
/// carries a u16 table index instead of repeating the full key string (see
/// `save::mobs`).
/// v10 retires the per-mob fixed shear-regrow counter AND the per-mob mod KV
/// map: both moved into the tag map (`petramond:shear_regrow`; the KV seam
/// was deleted outright), which also newly carries `petramond:health`.
/// Another clean break — a v9 record's shear/KV bytes have no store to land
/// in; dev worlds regenerate.
/// v11 retires the bed_frame block/item: every block id above 94 and item id
/// above 119 shifted down by one, so older sections would decode to the
/// wrong blocks/items. Clean break; dev worlds regenerate.
/// v13 UNIFIES per-cell block state: the seven typed lists (entity facings,
/// torches, model cells/facings, doors, stairs, slabs, log axes — their flag
/// bits are now RESERVED) collapse into ONE opaque cell-state list, each
/// record `[header: len<<4 | id_mask][len bytes]` with id-masked bytes
/// palette-translated. A new stateful block kind needs NO codec change.
/// Clean break; dev worlds regenerate.
/// v14 widens the persisted BLOCK-LIGHT cube from one byte per cell to one
/// packed RGB `u16` (little-endian, `petramond_world::light::LightRgb`): block light
/// carries colour now. Skylight is untouched. Clean break — a v13 record's
/// 4096-byte block-light blob is half the length this build reads; dev worlds
/// regenerate.
/// v15 WIDENS the block id to two bytes and palette-compresses the block
/// array: a record now stores `[distinct: u16][ids: u16 x distinct][index per
/// cell]`, where the per-cell index is one byte while the section holds ≤ 256
/// distinct blocks (every real section) and two bytes otherwise. A section's
/// on-disk block payload therefore stays ~4 KiB even though ids doubled. The
/// unified cell-state record's header also splits into `[len][id_mask]` and
/// its id-masked entries are two bytes each. Clean break; dev worlds
/// regenerate.
/// v16 (2026-09-03): item entities carry their MOTION (loose / in flight /
/// lodged in a block, with the heading and anchor of a lodged one) after
/// the spin. Clean break; dev worlds regenerate.
/// v17 (2026-09-04): a flight persists its velocity only — its heading is
/// derived from that on the first step — and the motion tag is a declared
/// wire enum (`save::entities::MotionKind`). Clean break; dev worlds
/// regenerate.
/// v18 (2026-09-13): item entity and mob positions are `f64` world
/// positions, exact anywhere inside the world border. Clean break; dev
/// worlds regenerate.
/// v19 (2026-09-16): saved mobs carry their container slots, and string tags
/// a u32 length. The oldest version this build reads.
/// v20 (2026-09-26): every flag-gated payload is framed as `[len: u32][bytes]`,
/// so a decoder checks each payload consumes exactly its bytes and a future
/// upgrade step can rewrite one payload and copy the rest untouched. The
/// first MIGRATED bump: v19 records upgrade through `v19::upgrade` on read.
/// From here on a layout change adds an upgrade step (see `save::format`);
/// it is never a clean break.
const SECTION_REC_VERSION: u8 = 20;

/// The section-record format: v19 and newer decode, older is retired.
pub(super) const SECTION: Format = Format::new(
    "section record",
    SECTION_REC_VERSION as u32,
    &[v19::upgrade],
);

const FLAG_HAS_FLUID: u8 = 0x01;
const FLAG_HAS_ENTITIES: u8 = 0x02;
const FLAG_HAS_FURNACES: u8 = 0x04;
/// The unified per-cell state list (v13+; the bit carried entity facings
/// before the unification).
const FLAG_HAS_CELL_STATES: u8 = 0x08;
// 0x10 (torches), 0x40 (model cells), 0x80 (model facings): RESERVED — their
// payloads ride the unified cell-state list since v13.
const FLAG_HAS_MOBS: u8 = 0x20;
/// Second flags byte (chunk-record v3+). `0` for a v2 record (no such byte).
/// Bits 0x01 (v5 sapling stages), 0x02 (doors), 0x04 (stairs), 0x10 (log
/// axes): RESERVED.
const FLAG2_HAS_CELL_KV: u8 = 0x08;
const FLAG2_HAS_CONTAINERS: u8 = 0x20;
/// Third flags byte (section-record v4+). `0` for older records. Bit 0x01
/// (slabs): RESERVED.
/// Persisted baked light (skylight / block-light cubes, appended in that
/// order). Written only when the section's light was CLEAN at snapshot time;
/// an absent cube simply re-bakes on load, so no version bump is needed.
const FLAG3_HAS_SKYLIGHT: u8 = 0x02;
const FLAG3_HAS_BLOCKLIGHT: u8 = 0x04;
/// Every flag bit this build decodes, per flags byte. A set bit outside these
/// is a payload a newer build appended: the record is refused, not
/// half-read, so saving cannot drop it.
const KNOWN_FLAGS: [u8; 3] = [
    FLAG_HAS_FLUID | FLAG_HAS_ENTITIES | FLAG_HAS_FURNACES | FLAG_HAS_CELL_STATES | FLAG_HAS_MOBS,
    FLAG2_HAS_CELL_KV | FLAG2_HAS_CONTAINERS,
    FLAG3_HAS_SKYLIGHT | FLAG3_HAS_BLOCKLIGHT,
];

/// Owned, send-able copy of one 16³ section's save data. The game thread builds one
/// of these (a cheap array clone) and hands it to the I/O thread, which does the
/// expensive compression off the game loop. Biome/heightmap are per-column and
/// regenerated, so they are not part of a section record.
pub struct SectionSnapshot {
    pub pos: SectionPos,
    /// Derived explored-terrain cache, not authoritative player/entity state.
    /// Routing metadata only; it is not encoded inside the section record.
    pub cache_only: bool,
    pub blocks: petramond_world::section::BlockCube,
    pub fluid: Option<Arc<[u8]>>,
    /// Item entities resting in this section, captured at save time so their
    /// lifetime timers persist with it. Empty for the common case.
    pub entities: Vec<DroppedItem>,
    /// Furnace machine state (burn/cook counters) in this section, keyed by
    /// section-local block index. The slots live in [`containers`](Self::containers).
    /// Empty for the common section.
    pub furnaces: CellMap<Furnace>,
    /// Generic item-slot containers (chests, furnaces, mod container blocks),
    /// keyed by section-local block index. Empty for the common section.
    pub containers: CellMap<Container>,
    /// The UNIFIED per-cell block state (stair facing, slab layers, door
    /// pose, torch mount, log axis, model offset+facing, chest/furnace
    /// front), keyed by section-local block index — opaque bytes owned by
    /// each block's codec; the id-masked bytes are the only ones this codec
    /// touches (palette translation). Empty for the common section.
    pub cell_states: CellMap<ShapeState>,
    /// Baked skylight cube, captured only when the section's light was CLEAN
    /// (baked and not since invalidated) so a reload can skip the bake
    /// entirely. `None` re-bakes on load, exactly like the pre-persistence
    /// behaviour.
    pub skylight: Option<Arc<[u8]>>,
    /// Baked block-light cube (packed RGB cells); independent of `skylight`
    /// presence on the wire but only ever written alongside it (absent = no
    /// emitter in range).
    pub blocklight: Option<Arc<[petramond_world::light::LightRgb]>>,
    /// Per-cell mod KV entries (`mod_id:key` → bytes), keyed by section-local
    /// index. Opaque to the engine and PRESERVED byte-exact through load/save —
    /// unknown keys are never dropped, so an absent mod's data survives. See
    /// `Section::cell_kv`.
    pub cell_kv: CellMap<BTreeMap<String, Vec<u8>>>,
    /// Mobs resting in this section, captured at save time so a passive owl reloads
    /// where it was left. Like [`entities`](Self::entities) these don't live in the
    /// `Section`, so the world save paths set this from the live mob set. Empty for the
    /// common section.
    pub mobs: Vec<SavedMob>,
}

impl SectionSnapshot {
    /// Snapshot a section's terrain with no entities or mobs attached. The world save
    /// paths set [`entities`](Self::entities) / [`mobs`](Self::mobs) afterwards from the
    /// active item and mob sets.
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
        }
    }
}

/// Compress a section snapshot into a record: `[version, flags, flags2, flags3, blocks,
/// fluid meta?, entities?, …]`, zlib-deflated. Each flag-gated payload is appended only
/// when present, framed with its length, so a terrain-only section pays for just its
/// block array.
pub fn encode_snapshot(s: &SectionSnapshot) -> Vec<u8> {
    encode_snapshot_with(s, &super::palette::active())
}

/// [`encode_snapshot`] against an explicit palette — the world's is a
/// process-wide handle, and a test that means to state something about the
/// FORMAT must not depend on which world happens to be open.
pub fn encode_snapshot_with(s: &SectionSnapshot, pal: &palette::Palette) -> Vec<u8> {
    let extra = s.fluid.as_ref().map_or(0, |w| w.len());
    let mut payload = Vec::with_capacity(4 + s.blocks.len() + extra);
    put_u8(&mut payload, SECTION_REC_VERSION);
    let mut flags = 0u8;
    if s.fluid.is_some() {
        flags |= FLAG_HAS_FLUID;
    }
    if !s.entities.is_empty() {
        flags |= FLAG_HAS_ENTITIES;
    }
    if !s.furnaces.is_empty() {
        flags |= FLAG_HAS_FURNACES;
    }
    if !s.cell_states.is_empty() {
        flags |= FLAG_HAS_CELL_STATES;
    }
    if !s.mobs.is_empty() {
        flags |= FLAG_HAS_MOBS;
    }
    let mut flags2 = 0u8;
    if !s.cell_kv.is_empty() {
        flags2 |= FLAG2_HAS_CELL_KV;
    }
    if !s.containers.is_empty() {
        flags2 |= FLAG2_HAS_CONTAINERS;
    }
    let mut flags3 = 0u8;
    if s.skylight.is_some() {
        flags3 |= FLAG3_HAS_SKYLIGHT;
    }
    if s.blocklight.is_some() {
        flags3 |= FLAG3_HAS_BLOCKLIGHT;
    }
    put_u8(&mut payload, flags);
    put_u8(&mut payload, flags2);
    put_u8(&mut payload, flags3);
    // Block ids are stored as the SAVE's ids (see `super::palette`), so a
    // future registry renumbering can't corrupt old worlds.
    put_block_cube(&mut payload, &s.blocks, pal);
    // Every flag-gated payload below is framed (`put_framed`), in flag order.
    if let Some(w) = &s.fluid {
        put_framed(&mut payload, |buf| buf.extend_from_slice(w));
    }
    if !s.entities.is_empty() {
        put_framed(&mut payload, |buf| {
            super::entities::put_entities(buf, &s.entities)
        });
    }
    if !s.furnaces.is_empty() {
        put_framed(&mut payload, |buf| {
            super::furnace::put_furnaces(buf, &s.furnaces)
        });
    }
    if !s.cell_states.is_empty() {
        // Each record is `[len][id_mask][len bytes]`; the id-masked bytes
        // are BLOCK IDS and go through the palette like the block array —
        // the ONLY interpretation this codec ever applies to state bytes.
        put_framed(&mut payload, |buf| {
            put_indexed(buf, &s.cell_states, 8, |buf, state| {
                let bytes = state.bytes();
                put_u8(buf, bytes.len() as u8);
                put_u8(buf, state.id_mask());
                let mut i = 0;
                while i < bytes.len() {
                    if state.id_mask() & (1 << i) != 0 && i + 1 < bytes.len() {
                        put_u16(buf, pal.block_to_disk(state.id_at(i)));
                        i += 2;
                    } else {
                        put_u8(buf, bytes[i]);
                        i += 1;
                    }
                }
            })
        });
    }
    if !s.mobs.is_empty() {
        put_framed(&mut payload, |buf| super::mobs::put_mobs(buf, &s.mobs));
    }
    if !s.cell_kv.is_empty() {
        // Each record is the cell's KV map (idx written by put_indexed);
        // rec_bytes is a reserve hint only — the record body is variable.
        put_framed(&mut payload, |buf| {
            put_indexed(buf, &s.cell_kv, 16, |buf, map| {
                put_kv_map(buf, map);
            })
        });
    }
    if !s.containers.is_empty() {
        put_framed(&mut payload, |buf| {
            super::container::put_containers(buf, &s.containers)
        });
    }
    if let Some(sky) = &s.skylight {
        put_framed(&mut payload, |buf| buf.extend_from_slice(sky));
    }
    if let Some(bl) = &s.blocklight {
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
/// also gives deflate a much shorter dictionary to chew on.
fn put_block_cube(
    buf: &mut Vec<u8>,
    blocks: &petramond_world::section::BlockCube,
    pal: &palette::Palette,
) {
    let mut ids: Vec<u16> = Vec::new();
    let mut index: Vec<u16> = Vec::with_capacity(blocks.len());
    for b in blocks.iter() {
        let disk = pal.block_to_disk(b);
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

/// Inverse of [`put_block_cube`], mapping each palette entry back through the
/// save palette on the way out.
fn get_block_cube(r: &mut Reader, pal: &palette::Palette) -> Option<Vec<u16>> {
    let distinct = r.u16()? as usize;
    if distinct == 0 || distinct > petramond_world::registry::WIDE_ID_CAP {
        return None;
    }
    let mut ids = Vec::with_capacity(distinct);
    for _ in 0..distinct {
        ids.push(pal.block_from_disk(r.u16()?));
    }
    let mut out = Vec::with_capacity(SECTION_VOLUME);
    if distinct <= u8::MAX as usize + 1 {
        for &i in r.bytes(SECTION_VOLUME)?.iter() {
            out.push(*ids.get(i as usize)?);
        }
    } else {
        for _ in 0..SECTION_VOLUME {
            out.push(*ids.get(r.u16()? as usize)?);
        }
    }
    Some(out)
}

/// Append one flag-gated payload as `[len: u32][bytes]`, `body` writing the
/// bytes.
fn put_framed(buf: &mut Vec<u8>, body: impl FnOnce(&mut Vec<u8>)) {
    let at = buf.len();
    put_u32(buf, 0);
    body(buf);
    let len = (buf.len() - at - 4) as u32;
    buf[at..at + 4].copy_from_slice(&len.to_le_bytes());
}

/// A section record's contents: the section plus the item entities and
/// mobs stored with it.
pub type DecodedSection = (Section, Vec<DroppedItem>, Vec<SavedMob>);

/// Decode a compressed section record into a `Section` at `pos` plus any item
/// entities and mobs stored with it. An older record is migrated first; a
/// newer, corrupt or unknown one is a typed error — never "no record".
pub fn decode_section(pos: SectionPos, blob: &[u8]) -> Result<DecodedSection, RecordError> {
    decode_section_with(pos, blob, &super::palette::active())
}

/// [`decode_section`] against an explicit palette (see [`encode_snapshot_with`]).
pub fn decode_section_with(
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

/// Sequential reader over a current-version record body that hands out its
/// framed payloads. Error offsets count from the start of the decompressed
/// record (the version byte precedes the body).
struct Frames<'a> {
    r: Reader<'a>,
}

/// One framed payload and where it starts in the record.
struct Frame<'a> {
    bytes: &'a [u8],
    at: usize,
    what: &'static str,
}

impl<'a> Frames<'a> {
    fn corrupt(&self, what: &'static str) -> RecordError {
        RecordError::corrupt(SECTION.name, what, self.r.offset() + 1)
    }

    /// The next payload when `present`, else `None` (nothing consumed).
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
    /// Decode the payload; the decoder must consume exactly the frame.
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

    /// A fixed-size payload (a per-cell cube).
    fn exact(self, len: usize) -> Result<&'a [u8], RecordError> {
        self.decode(|r| r.bytes(len))
    }
}

/// Decode a current-version record body (after the version byte).
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
    let blocks = get_block_cube(&mut frames.r, pal).ok_or_else(|| frames.corrupt("block cube"))?;
    let fluid = frames
        .payload(flags & FLAG_HAS_FLUID != 0, "fluid")?
        .map(|f| f.exact(SECTION_VOLUME))
        .transpose()?;
    let entities = frames
        .payload(flags & FLAG_HAS_ENTITIES != 0, "entities")?
        .map(|f| f.decode(super::entities::get_entities))
        .transpose()?;
    let furnaces = frames
        .payload(flags & FLAG_HAS_FURNACES != 0, "furnaces")?
        .map(|f| f.decode(super::furnace::get_furnaces))
        .transpose()?;
    let cell_states = frames
        .payload(flags & FLAG_HAS_CELL_STATES != 0, "cell states")?
        .map(|f| f.decode(|r| get_cell_states(r, pal)))
        .transpose()?;
    let mobs = frames
        .payload(flags & FLAG_HAS_MOBS != 0, "mobs")?
        .map(|f| f.decode(super::mobs::get_mobs))
        .transpose()?;
    let cell_kv = frames
        .payload(flags2 & FLAG2_HAS_CELL_KV != 0, "cell kv")?
        .map(|f| f.decode(|r| get_indexed(r, get_kv_map)))
        .transpose()?;
    let containers = frames
        .payload(flags2 & FLAG2_HAS_CONTAINERS != 0, "containers")?
        .map(|f| f.decode(super::container::get_containers))
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
    let mut section = Section::from_saved(
        pos.cx,
        pos.cy,
        pos.cz,
        &blocks,
        fluid.map(|w| w.to_vec().into_boxed_slice()),
        furnaces.unwrap_or_default(),
        containers.unwrap_or_default(),
        cell_states.unwrap_or_default(),
        cell_kv.unwrap_or_default(),
    );
    // Persisted clean light: seed the cache and clear `light_dirty`, so the
    // streamer's settle flush skips the bake for this section entirely. The
    // `light_from_persist` flag records that these cubes are the settled
    // persisted bake — the streamer's cover-change invalidation spares them
    // when the change's source is itself persisted content.
    if let Some(sky) = skylight {
        section.set_skylight(Arc::from(sky));
        if let Some(bl) = blocklight {
            section.set_blocklight(Arc::from(bl));
        }
        section.light_from_persist = true;
    }
    Ok((
        section,
        entities.unwrap_or_default(),
        mobs.unwrap_or_default(),
    ))
}

/// The unified cell-state list: per cell `[len][id_mask][len bytes]`, the
/// id-masked pairs mapped back through the save palette.
fn get_cell_states(r: &mut Reader, pal: &palette::Palette) -> Option<CellMap<ShapeState>> {
    get_indexed(r, |r| {
        let len = r.u8()? as usize;
        let id_mask = r.u8()?;
        if len > petramond_world::block::SHAPE_STATE_MAX {
            return None;
        }
        let mut bytes = [0u8; petramond_world::block::SHAPE_STATE_MAX];
        let mut i = 0;
        while i < len {
            if id_mask & (1 << i) != 0 && i + 1 < len {
                let [lo, hi] = ShapeState::id_bytes(pal.block_from_disk(r.u16()?));
                bytes[i] = lo;
                bytes[i + 1] = hi;
                i += 2;
            } else {
                bytes[i] = r.u8()?;
                i += 1;
            }
        }
        Some(ShapeState::with_ids(&bytes[..len], id_mask))
    })
}
