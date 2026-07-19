//! Binary codec for save data: little-endian primitives + compressed section
//! records.
//!
//! A section record stores only what generation can't reproduce for one 16³ cube —
//! block ids and (when present) water-flow metadata — then zlib-compresses the lot
//! (flate2 / miniz_oxide, pure Rust). Biome and surface heightmap are per-column,
//! cheaply regenerated, and so are never written here. Baked light IS persisted
//! (when clean at snapshot time), so a reload samples the saved cubes instead of
//! re-baking the whole explored area; the cubes are mostly uniform and deflate
//! to almost nothing.

mod primitives;
#[cfg(test)]
mod tests;

pub use primitives::{deflate, get_item_slot, inflate, put_item_slot, Reader};
pub(crate) use primitives::{
    get_indexed, get_kv_map, put_f32, put_f64, put_i64, put_indexed, put_kv_map, put_u16, put_u32,
    put_u64, put_u8, read_u16, read_u32, write_u16, write_u32,
};

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block_state::{LogAxis, SlabState, StairState};
use crate::chunk::{SectionPos, SECTION_VOLUME};
use crate::container::Container;
use crate::door::DoorState;
use crate::entity::DroppedItem;
use crate::facing::Facing;
use crate::furnace::Furnace;
use crate::mob::SavedMob;
use crate::section::Section;
use crate::torch::TorchPlacement;

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
const SECTION_REC_VERSION: u8 = 8;
/// Oldest section-record version this build can still read.
const SECTION_REC_MIN_VERSION: u8 = 8;
const FLAG_HAS_WATER: u8 = 0x01;
const FLAG_HAS_ENTITIES: u8 = 0x02;
const FLAG_HAS_FURNACES: u8 = 0x04;
const FLAG_HAS_ENTITY_FACINGS: u8 = 0x08;
const FLAG_HAS_TORCHES: u8 = 0x10;
const FLAG_HAS_MOBS: u8 = 0x20;
const FLAG_HAS_MODEL_CELLS: u8 = 0x40;
const FLAG_HAS_MODEL_FACINGS: u8 = 0x80;
/// Second flags byte (chunk-record v3+). `0` for a v2 record (no such byte).
/// Bit 0x01 is RESERVED: it carried the retired v5 sapling-stage map (stages
/// are block rows since v6).
const FLAG2_HAS_DOORS: u8 = 0x02;
const FLAG2_HAS_STAIRS: u8 = 0x04;
const FLAG2_HAS_CELL_KV: u8 = 0x08;
const FLAG2_HAS_LOG_AXES: u8 = 0x10;
const FLAG2_HAS_CONTAINERS: u8 = 0x20;
/// Third flags byte (section-record v4+). `0` for older records.
const FLAG3_HAS_SLABS: u8 = 0x01;
/// Persisted baked light (skylight / block-light cubes, appended in that
/// order). Written only when the section's light was CLEAN at snapshot time;
/// an absent cube simply re-bakes on load, so no version bump is needed.
const FLAG3_HAS_SKYLIGHT: u8 = 0x02;
const FLAG3_HAS_BLOCKLIGHT: u8 = 0x04;

/// Owned, send-able copy of one 16³ section's save data. The game thread builds one
/// of these (a cheap array clone) and hands it to the I/O thread, which does the
/// expensive compression off the game loop. Biome/heightmap are per-column and
/// regenerated, so they are not part of a section record.
pub struct SectionSnapshot {
    pub pos: SectionPos,
    /// Derived explored-terrain cache, not authoritative player/entity state.
    /// Routing metadata only; it is not encoded inside the section record.
    pub(crate) cache_only: bool,
    pub blocks: Arc<[u8]>,
    pub water: Option<Arc<[u8]>>,
    /// Item entities resting in this section, captured at save time so their
    /// lifetime timers persist with it. Empty for the common case.
    pub entities: Vec<DroppedItem>,
    /// Furnace machine state (burn/cook counters) in this section, keyed by
    /// section-local block index. The slots live in [`containers`](Self::containers).
    /// Empty for the common section.
    pub furnaces: HashMap<u16, Furnace>,
    /// Generic item-slot containers (chests, furnaces, mod container blocks),
    /// keyed by section-local block index. Empty for the common section.
    pub containers: HashMap<u16, Container>,
    /// Facing block-entity orientations (chests, furnaces), keyed by
    /// section-local block index. Empty for the common section.
    pub entity_facings: HashMap<u16, Facing>,
    /// Torch orientations in this section, keyed by section-local block index, so a
    /// wall vs floor torch reloads the way it was placed. Empty for the common section.
    pub torches: HashMap<u16, TorchPlacement>,
    /// Multi-cell bbmodel occupancy: each non-zero authored footprint offset, so a
    /// placed multi-block (the workbench) reloads as one object. Empty for the common
    /// section. See `Section::model_cells`.
    pub model_cells: HashMap<u16, [u8; 3]>,
    /// Per-cell facing for oriented bbmodel blocks, keyed like `model_cells`. Empty for
    /// old/non-directional model placements. See `Section::model_facings`.
    pub model_facings: HashMap<u16, Facing>,
    /// Door state (facing + open + which-half), keyed by section-local index, so a
    /// placed door reloads on the same edge and in the same open/closed pose. Empty for
    /// the common section. See `Section::doors` / [`crate::door`].
    pub doors: HashMap<u16, DoorState>,
    /// State of placed stairs, keyed by section-local index, so a stair reloads with
    /// the same low/open side and top/bottom half.
    pub stair_states: HashMap<u16, StairState>,
    /// State of placed slabs, keyed by section-local index. Stores split axis plus
    /// the material block in each occupied half so mixed slab stacks reload exactly.
    pub slab_states: HashMap<u16, SlabState>,
    /// Non-default log axes, keyed by section-local index. Missing logs are vertical.
    pub log_axes: HashMap<u16, LogAxis>,
    /// Baked skylight cube, captured only when the section's light was CLEAN
    /// (baked and not since invalidated) so a reload can skip the bake
    /// entirely. `None` re-bakes on load, exactly like the pre-persistence
    /// behaviour.
    pub skylight: Option<Arc<[u8]>>,
    /// Baked block-light cube; independent of `skylight` presence on the wire
    /// but only ever written alongside it (absent = no emitter in range).
    pub blocklight: Option<Arc<[u8]>>,
    /// Per-cell mod KV entries (`mod_id:key` → bytes), keyed by section-local
    /// index. Opaque to the engine and PRESERVED byte-exact through load/save —
    /// unknown keys are never dropped, so an absent mod's data survives. See
    /// `Section::cell_kv`.
    pub cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
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
            blocks: s.blocks_arc(),
            water: s.water_arc(),
            entities: Vec::new(),
            furnaces: s.furnaces().clone(),
            containers: s.containers().clone(),
            entity_facings: s.entity_facings().clone(),
            torches: s.torches().clone(),
            model_cells: s.model_cells().clone(),
            model_facings: s.model_facings().clone(),
            doors: s.doors().clone(),
            stair_states: s.stair_states().clone(),
            slab_states: s.slab_states().clone(),
            log_axes: s.log_axes().clone(),
            skylight: (!s.light_dirty).then(|| s.skylight_arc()).flatten(),
            blocklight: (!s.light_dirty).then(|| s.blocklight_arc()).flatten(),
            cell_kv: s.cell_kv().clone(),
            mobs: Vec::new(),
        }
    }
}

/// Compress a section snapshot into a record: `[version, flags, flags2, flags3, blocks,
/// water?, entities?, …]`, zlib-deflated. Each flag-gated payload is appended only
/// when present, so a terrain-only section pays for just its block array.
pub fn encode_snapshot(s: &SectionSnapshot) -> Vec<u8> {
    let extra = s.water.as_ref().map_or(0, |w| w.len());
    let mut payload = Vec::with_capacity(4 + s.blocks.len() + extra);
    put_u8(&mut payload, SECTION_REC_VERSION);
    let mut flags = 0u8;
    if s.water.is_some() {
        flags |= FLAG_HAS_WATER;
    }
    if !s.entities.is_empty() {
        flags |= FLAG_HAS_ENTITIES;
    }
    if !s.furnaces.is_empty() {
        flags |= FLAG_HAS_FURNACES;
    }
    if !s.entity_facings.is_empty() {
        flags |= FLAG_HAS_ENTITY_FACINGS;
    }
    if !s.torches.is_empty() {
        flags |= FLAG_HAS_TORCHES;
    }
    if !s.mobs.is_empty() {
        flags |= FLAG_HAS_MOBS;
    }
    if !s.model_cells.is_empty() {
        flags |= FLAG_HAS_MODEL_CELLS;
    }
    if !s.model_facings.is_empty() {
        flags |= FLAG_HAS_MODEL_FACINGS;
    }
    let mut flags2 = 0u8;
    if !s.doors.is_empty() {
        flags2 |= FLAG2_HAS_DOORS;
    }
    if !s.stair_states.is_empty() {
        flags2 |= FLAG2_HAS_STAIRS;
    }
    if !s.cell_kv.is_empty() {
        flags2 |= FLAG2_HAS_CELL_KV;
    }
    if !s.log_axes.is_empty() {
        flags2 |= FLAG2_HAS_LOG_AXES;
    }
    if !s.containers.is_empty() {
        flags2 |= FLAG2_HAS_CONTAINERS;
    }
    let mut flags3 = 0u8;
    if !s.slab_states.is_empty() {
        flags3 |= FLAG3_HAS_SLABS;
    }
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
    let pal = super::palette::active();
    payload.extend(s.blocks.iter().map(|&b| pal.block_to_disk(b)));
    if let Some(w) = &s.water {
        payload.extend_from_slice(w);
    }
    if !s.entities.is_empty() {
        super::entities::put_entities(&mut payload, &s.entities);
    }
    if !s.furnaces.is_empty() {
        super::furnace::put_furnaces(&mut payload, &s.furnaces);
    }
    if !s.entity_facings.is_empty() {
        put_indexed(&mut payload, &s.entity_facings, 1, |buf, facing| {
            put_u8(buf, facing.to_u8());
        });
    }
    if !s.torches.is_empty() {
        super::torch::put_torches(&mut payload, &s.torches);
    }
    if !s.mobs.is_empty() {
        super::mobs::put_mobs(&mut payload, &s.mobs);
    }
    if !s.model_cells.is_empty() {
        // Each record is the cell's 3-byte footprint offset (idx written by put_indexed).
        put_indexed(&mut payload, &s.model_cells, 3, |buf, off| {
            put_u8(buf, off[0]);
            put_u8(buf, off[1]);
            put_u8(buf, off[2]);
        });
    }
    if !s.model_facings.is_empty() {
        put_indexed(&mut payload, &s.model_facings, 1, |buf, facing| {
            put_u8(buf, facing.to_u8());
        });
    }
    if !s.doors.is_empty() {
        // Each record is the cell's 1-byte packed door state (idx written by put_indexed).
        put_indexed(&mut payload, &s.doors, 1, |buf, state| {
            put_u8(buf, state.encode());
        });
    }
    if !s.stair_states.is_empty() {
        put_indexed(&mut payload, &s.stair_states, 1, |buf, state| {
            put_u8(buf, state.encode());
        });
    }
    if !s.cell_kv.is_empty() {
        // Each record is the cell's KV map (idx written by put_indexed);
        // rec_bytes is a reserve hint only — the record body is variable.
        put_indexed(&mut payload, &s.cell_kv, 16, |buf, map| {
            put_kv_map(buf, map);
        });
    }
    if !s.log_axes.is_empty() {
        put_indexed(&mut payload, &s.log_axes, 1, |buf, axis| {
            put_u8(buf, axis.to_u8());
        });
    }
    if !s.containers.is_empty() {
        super::container::put_containers(&mut payload, &s.containers);
    }
    if !s.slab_states.is_empty() {
        put_indexed(&mut payload, &s.slab_states, 3, |buf, state| {
            put_u8(buf, state.encode_meta());
            put_u8(buf, pal.block_to_disk(state.layers[0].id()));
            put_u8(buf, pal.block_to_disk(state.layers[1].id()));
        });
    }
    if let Some(sky) = &s.skylight {
        payload.extend_from_slice(sky);
    }
    if let Some(bl) = &s.blocklight {
        payload.extend_from_slice(bl);
    }
    deflate(&payload)
}

/// Decode a compressed section record into a `Section` at `pos` plus any item
/// entities and mobs stored with it. `None` on corrupt / wrong-version /
/// wrong-length data.
pub fn decode_section(
    pos: SectionPos,
    blob: &[u8],
) -> Option<(Section, Vec<DroppedItem>, Vec<SavedMob>)> {
    let payload = inflate(blob)?;
    let mut r = Reader::new(&payload);
    let version = r.u8()?;
    if !(SECTION_REC_MIN_VERSION..=SECTION_REC_VERSION).contains(&version) {
        return None;
    }
    let flags = r.u8()?;
    let flags2 = r.u8()?;
    let flags3 = r.u8()?;
    let pal = super::palette::active();
    let blocks: Box<[u8]> = r
        .bytes(SECTION_VOLUME)?
        .iter()
        .map(|&b| pal.block_from_disk(b))
        .collect();
    let water = if flags & FLAG_HAS_WATER != 0 {
        Some(r.bytes(SECTION_VOLUME)?.to_vec().into_boxed_slice())
    } else {
        None
    };
    let entities = if flags & FLAG_HAS_ENTITIES != 0 {
        super::entities::get_entities(&mut r)?
    } else {
        Vec::new()
    };
    let furnaces = if flags & FLAG_HAS_FURNACES != 0 {
        super::furnace::get_furnaces(&mut r)?
    } else {
        HashMap::new()
    };
    let mut entity_facings = if flags & FLAG_HAS_ENTITY_FACINGS != 0 {
        get_indexed(&mut r, |r| Some(Facing::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    // Entity facings belong to directional-view block entities (chest/furnace
    // fronts) only. Drop anything else: any surviving entry marks the section
    // a block-entity section, and records written while ladders still stored
    // their mount here would re-enter the furnace/chest fan-out for a cell
    // whose facing now lives on its block row.
    entity_facings
        .retain(|&idx, _| crate::block::Block::from_id(blocks[idx as usize]).directional_view());
    let torches = if flags & FLAG_HAS_TORCHES != 0 {
        super::torch::get_torches(&mut r)?
    } else {
        HashMap::new()
    };
    let mobs = if flags & FLAG_HAS_MOBS != 0 {
        super::mobs::get_mobs(&mut r)?
    } else {
        Vec::new()
    };
    let model_cells = if flags & FLAG_HAS_MODEL_CELLS != 0 {
        get_indexed(&mut r, |r| Some([r.u8()?, r.u8()?, r.u8()?]))?
    } else {
        HashMap::new()
    };
    let model_facings = if flags & FLAG_HAS_MODEL_FACINGS != 0 {
        get_indexed(&mut r, |r| Some(Facing::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let doors = if flags2 & FLAG2_HAS_DOORS != 0 {
        get_indexed(&mut r, |r| Some(DoorState::decode(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let stair_states = if flags2 & FLAG2_HAS_STAIRS != 0 {
        get_indexed(&mut r, |r| Some(StairState::decode(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let cell_kv = if flags2 & FLAG2_HAS_CELL_KV != 0 {
        get_indexed(&mut r, get_kv_map)?
    } else {
        HashMap::new()
    };
    let log_axes = if flags2 & FLAG2_HAS_LOG_AXES != 0 {
        get_indexed(&mut r, |r| Some(LogAxis::from_u8(r.u8()?)))?
    } else {
        HashMap::new()
    };
    let containers = if flags2 & FLAG2_HAS_CONTAINERS != 0 {
        super::container::get_containers(&mut r)?
    } else {
        HashMap::new()
    };
    let slab_states = if flags3 & FLAG3_HAS_SLABS != 0 {
        get_indexed(&mut r, |r| {
            let meta = r.u8()?;
            let a = crate::block::Block(pal.block_from_disk(r.u8()?));
            let b = crate::block::Block(pal.block_from_disk(r.u8()?));
            Some(SlabState::decode(meta, a, b))
        })?
    } else {
        HashMap::new()
    };
    let skylight = if flags3 & FLAG3_HAS_SKYLIGHT != 0 {
        Some(r.bytes(SECTION_VOLUME)?)
    } else {
        None
    };
    let blocklight = if flags3 & FLAG3_HAS_BLOCKLIGHT != 0 {
        Some(r.bytes(SECTION_VOLUME)?)
    } else {
        None
    };
    let mut section = Section::from_saved(
        pos.cx,
        pos.cy,
        pos.cz,
        blocks,
        water,
        furnaces,
        containers,
        entity_facings,
        torches,
        model_cells,
        model_facings,
        doors,
        stair_states,
        slab_states,
        log_axes,
        cell_kv,
    );
    // Persisted clean light: seed the cache and clear `light_dirty`, so the
    // streamer's settle flush skips the bake for this section entirely. The
    // `light_from_persist` flag records that these cubes are the settled
    // persisted bake — the streamer's cover-change invalidation spares them
    // when the change's source is itself persisted content.
    if let Some(sky) = skylight {
        section.set_skylight(std::sync::Arc::from(sky));
        if let Some(bl) = blocklight {
            section.set_blocklight(std::sync::Arc::from(bl));
        }
        section.light_from_persist = true;
    }
    Some((section, entities, mobs))
}
