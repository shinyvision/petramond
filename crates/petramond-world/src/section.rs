pub use crate::block_state::CellMap;
use std::sync::Arc;

use crate::block::Block;
use crate::block_state::BlockStates;
use crate::chunk::{section_idx, SECTION_SIZE, SECTION_VOLUME, SKY_FULL};
use crate::container::Container;
use crate::furnace::Furnace;
use crate::light::LightRgb;

pub use cube::BlockCube;
pub(crate) use metrics::METRICS;

mod block_entities;
mod cell_states;
mod cube;
mod metrics;
mod restore;

#[cfg(test)]
mod tests;

crate::wire_enum::wire_enum! {
    pub enum SectionSummary: u8 {
        Unknown = 0,
        Empty = 1,
        FullOpaque = 2,
        FullWater = 3,
        Mixed = 4,
    }
    default Unknown
}

impl SectionSummary {
    #[inline]
    pub fn virtual_block(self) -> Block {
        match self {
            SectionSummary::FullOpaque => Block::Stone,
            SectionSummary::FullWater => Block::Water,
            _ => Block::Air,
        }
    }
}

#[derive(Copy, Clone, Debug, Default, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct SectionMetrics {
    pub random_tick_count: u32,
    pub opaque_count: u32,
    pub plane_opaque: [u16; 6],
    pub non_air_count: u32,
    pub water_count: u32,
    pub fluid_count: u32,
    pub quench_count: u32,
    pub quencher_count: u32,
    pub biome_tint_count: u32,
    pub particle_emitter_count: u32,
    pub light_emitter_count: u32,
}

impl SectionMetrics {
    pub fn valid(self) -> bool {
        let volume = SECTION_VOLUME as u32;
        self.random_tick_count <= volume
            && self.opaque_count <= volume
            && self.non_air_count <= volume
            && self.water_count <= volume
            && self.fluid_count <= volume
            && self.quench_count <= volume
            && self.quencher_count <= volume
            && self.biome_tint_count <= volume
            && self.particle_emitter_count <= volume
            && self.light_emitter_count <= volume
            && self.plane_opaque.iter().all(|&n| n <= 256)
    }
}

#[derive(Clone)]
pub struct Section {
    pub cx: i32,
    pub cy: i32,
    pub cz: i32,
    blocks: BlockCube,
    states: BlockStates,
    entities: Option<Box<BlockEntities>>,
    pub dirty: bool,
    pub modified: bool,
    skylight: Option<Arc<[u8]>>,
    blocklight: Option<Arc<[LightRgb]>>,
    pub light_dirty: bool,
    pub light_from_persist: bool,
    pub light_revision: u64,
    pub mesh_revision: u64,
    random_tick_count: u32,
    opaque_count: u32,
    plane_opaque: [u16; 6],
    non_air_count: u32,
    water_count: u32,
    fluid_count: u32,
    quench_count: u32,
    quencher_count: u32,
    biome_tint_count: u32,
    particle_emitter_cells: Vec<u16>,
    light_emitter_count: u32,
    shape_render: Option<Arc<std::collections::HashMap<u16, Box<[crate::block::ShapeRenderBox]>>>>,
    light_apertures: Option<Arc<CellMap<bool>>>,
}

#[derive(Clone, Default, PartialEq)]
struct BlockEntities {
    furnaces: CellMap<Furnace>,
    containers: CellMap<Container>,
}

impl BlockEntities {
    fn is_empty(&self) -> bool {
        self.furnaces.is_empty() && self.containers.is_empty()
    }

    fn memory_bytes(&self) -> u64 {
        std::mem::size_of::<Self>() as u64
            + (self.furnaces.len() * (2 + std::mem::size_of::<Furnace>() + 1)) as u64
            + (self.containers.len() * (2 + std::mem::size_of::<Container>() + 1)) as u64
    }
}

pub fn uniform_cube(value: u8) -> Arc<[u8]> {
    static CACHE: [std::sync::OnceLock<Arc<[u8]>>; 256] =
        [const { std::sync::OnceLock::new() }; 256];
    CACHE[value as usize]
        .get_or_init(|| vec![value; SECTION_VOLUME].into())
        .clone()
}

fn compact_uniform_cube(cube: Arc<[u8]>) -> Arc<[u8]> {
    let first = cube[0];
    if cube.iter().all(|&v| v == first) {
        uniform_cube(first)
    } else {
        cube
    }
}

impl Section {
    pub fn new(cx: i32, cy: i32, cz: i32) -> Self {
        Self {
            cx,
            cy,
            cz,
            blocks: BlockCube::uniform(0),
            states: BlockStates::new(),
            entities: None,
            dirty: true,
            modified: false,
            skylight: None,
            blocklight: None,
            light_dirty: true,
            light_from_persist: false,
            light_revision: 0,
            mesh_revision: 0,
            random_tick_count: 0,
            opaque_count: 0,
            plane_opaque: [0; 6],
            non_air_count: 0,
            water_count: 0,
            fluid_count: 0,
            quench_count: 0,
            quencher_count: 0,
            biome_tint_count: 0,
            particle_emitter_cells: Vec::new(),
            light_emitter_count: 0,
            shape_render: None,
            light_apertures: None,
        }
    }

    #[inline]
    pub fn custom_light_apertures(&self) -> Option<&CellMap<bool>> {
        self.light_apertures.as_deref()
    }

    pub fn set_custom_light_aperture(&mut self, idx: u16, opaque: bool) -> bool {
        let map = Arc::make_mut(self.light_apertures.get_or_insert_with(Default::default));
        map.insert(idx, opaque) != Some(opaque)
    }

    pub fn clear_custom_light_aperture(&mut self, idx: u16) -> bool {
        match self.light_apertures.as_ref() {
            Some(map) if map.contains_key(&idx) => {
                Arc::make_mut(self.light_apertures.as_mut().unwrap()).remove(&idx);
                true
            }
            _ => false,
        }
    }

    #[inline]
    pub fn shape_render_boxes(&self, idx: u16) -> Option<&[crate::block::ShapeRenderBox]> {
        self.shape_render.as_ref()?.get(&idx).map(|b| &b[..])
    }

    pub fn set_shape_render(&mut self, idx: u16, boxes: Box<[crate::block::ShapeRenderBox]>) {
        Arc::make_mut(self.shape_render.get_or_insert_with(Default::default)).insert(idx, boxes);
        self.mesh_revision += 1;
    }

    #[inline]
    pub fn origin_world(&self) -> (i32, i32, i32) {
        (
            self.cx * SECTION_SIZE as i32,
            self.cy * SECTION_SIZE as i32,
            self.cz * SECTION_SIZE as i32,
        )
    }

    #[inline]
    pub fn block(&self, x: usize, y: usize, z: usize) -> Block {
        Block::from_id(self.blocks.get(section_idx(x, y, z)))
    }

    #[inline]
    pub fn block_raw(&self, x: usize, y: usize, z: usize) -> u16 {
        self.blocks.get(section_idx(x, y, z))
    }

    #[inline]
    pub fn block_at_idx(&self, idx: usize) -> u16 {
        self.blocks.get(idx)
    }

    #[inline]
    pub fn blocks(&self) -> &BlockCube {
        &self.blocks
    }

    pub fn blocks_iter(&self) -> impl Iterator<Item = u16> + '_ {
        self.blocks.iter()
    }

    pub fn set_block(&mut self, x: usize, y: usize, z: usize, b: Block) {
        self.set_block_raw(x, y, z, b.id());
    }

    pub fn set_block_raw(&mut self, x: usize, y: usize, z: usize, id: u16) {
        let i = section_idx(x, y, z);
        let old = self.blocks.get(i);
        self.blocks.set(i, id);
        self.adjust_random_tick_count(old, id);
        self.adjust_opaque_count(x, y, z, old, id);
        self.states.clear_on_block_change(i);
        self.dirty = true;
        self.mark_light_dirty();
    }

    pub fn blocks_mut(&mut self) -> &mut BlockCube {
        &mut self.blocks
    }

    pub fn edit_ids_bulk(&mut self, f: impl FnOnce(&mut [u16])) {
        thread_local! {
            static SCRATCH: std::cell::Cell<Vec<u16>> = const { std::cell::Cell::new(Vec::new()) };
        }
        let mut ids = SCRATCH.with(|c| c.take());
        ids.clear();
        ids.extend(self.blocks.iter());
        f(&mut ids);
        self.blocks = BlockCube::from_ids(&ids);
        SCRATCH.with(|c| c.set(ids));
    }

    pub fn block_cube(&self) -> BlockCube {
        self.blocks.clone()
    }

    pub fn memory_parts(&self) -> (Option<usize>, usize, u64, u64, u64) {
        let (fluid_ptr, fluid_len, sparse) = self.states.memory_parts();
        let entities = self.entities.as_ref().map_or(0, |e| e.memory_bytes());
        let emitters = (self.particle_emitter_cells.capacity() * 2) as u64;
        (fluid_ptr, fluid_len, sparse, entities, emitters)
    }

    pub fn block_cube_heap(&self) -> (usize, u64) {
        self.blocks.heap()
    }

    pub fn fluid_arc(&self) -> Option<Arc<[u8]>> {
        self.states.fluid_arc()
    }
    pub fn skylight_arc(&self) -> Option<Arc<[u8]>> {
        self.skylight.clone()
    }
    pub fn blocklight_arc(&self) -> Option<Arc<[LightRgb]>> {
        self.blocklight.clone()
    }

    #[inline]
    pub fn has_baked_light(&self) -> bool {
        self.skylight.is_some()
    }

    #[inline]
    pub fn fluid_meta(&self, x: usize, y: usize, z: usize) -> u8 {
        self.states.fluid_meta(section_idx(x, y, z))
    }

    pub fn set_fluid(&mut self, x: usize, y: usize, z: usize, b: Block, meta: u8) {
        let i = section_idx(x, y, z);
        let id = b.id();
        let old = self.blocks.get(i);
        self.blocks.set(i, id);
        self.adjust_random_tick_count(old, id);
        self.adjust_opaque_count(x, y, z, old, id);
        let meta = if b.is_fluid() { meta } else { 0 };
        self.states.store_fluid_meta(i, meta);
        self.dirty = true;
    }

    pub fn fluid_slice(&self) -> Option<&[u8]> {
        self.states.fluid_slice()
    }

    #[inline]
    pub fn has_flowing_fluid(&self) -> bool {
        self.states.has_flowing()
    }

    #[inline]
    pub fn skylight_at(&self, x: usize, y: usize, z: usize) -> u8 {
        match &self.skylight {
            Some(s) => s[section_idx(x, y, z)],
            None => SKY_FULL,
        }
    }

    pub fn set_skylight(&mut self, cube: Arc<[u8]>) {
        self.skylight = Some(compact_uniform_cube(cube));
        self.light_dirty = false;
    }

    #[inline]
    pub fn blocklight_at(&self, x: usize, y: usize, z: usize) -> LightRgb {
        match &self.blocklight {
            Some(b) => b[section_idx(x, y, z)],
            None => LightRgb::ZERO,
        }
    }

    pub fn set_blocklight(&mut self, cube: Arc<[LightRgb]>) {
        if cube.iter().all(|v| v.is_dark()) {
            self.blocklight = None;
        } else {
            self.blocklight = Some(cube);
        }
    }

    pub fn mark_light_dirty(&mut self) {
        self.light_dirty = true;
        self.light_from_persist = false;
        self.light_revision = self.light_revision.wrapping_add(1);
    }

    pub fn mark_light_clean(&mut self) {
        self.light_dirty = false;
    }

    pub fn clear_blocklight(&mut self) {
        self.blocklight = None;
    }
}
