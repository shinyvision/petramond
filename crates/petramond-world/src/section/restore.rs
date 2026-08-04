use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block::{Block, ShapeState};
use crate::block_state::BlockStates;
use crate::chunk::SECTION_VOLUME;
use crate::container::Container;
use crate::furnace::Furnace;

use super::{BlockEntities, Section, SectionMetrics};

impl Section {
    /// Rebuild a section from saved arrays. `modified` starts false — it already
    /// matches what's on disk. The light cube is left for the async bake.
    #[allow(clippy::too_many_arguments)]
    pub fn from_saved(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: &[u16],
        water: Option<Box<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        cell_states: HashMap<u16, ShapeState>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        Self::from_shared(
            cx,
            cy,
            cz,
            super::BlockCube::from_ids(blocks),
            water.map(Arc::from),
            furnaces,
            containers,
            cell_states,
            cell_kv,
            None,
        )
    }

    /// Rebuild a replica section from immutable wire buffers and server-derived
    /// counters. No voxel buffer is copied or scanned on the render thread.
    #[allow(clippy::too_many_arguments)]
    pub fn from_replica(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: super::BlockCube,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        cell_states: HashMap<u16, ShapeState>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
        metrics: SectionMetrics,
    ) -> Self {
        debug_assert!(metrics.valid());
        Self::from_shared(
            cx,
            cy,
            cz,
            blocks,
            water,
            furnaces,
            containers,
            cell_states,
            cell_kv,
            Some(metrics),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_shared(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: super::BlockCube,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        cell_states: HashMap<u16, ShapeState>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
        metrics: Option<SectionMetrics>,
    ) -> Self {
        let entities = BlockEntities {
            furnaces,
            containers,
        };
        let mut s = Self {
            cx,
            cy,
            cz,
            blocks,
            states: BlockStates::from_shared(water, cell_states, cell_kv),
            entities: (!entities.is_empty()).then(|| Box::new(entities)),
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
            biome_tint_count: 0,
            particle_emitter_cells: Vec::new(),
            light_emitter_count: 0,
            shape_render: None,
            light_apertures: None,
        };
        if let Some(metrics) = metrics {
            s.install_metrics(metrics);
            if s.non_air_count == 0 {
                s.blocks.fill(0);
            } else if s.water_count as usize == SECTION_VOLUME {
                s.blocks.fill(Block::Water.id());
            }
        } else {
            s.recompute_opaque_count();
        }
        s
    }
}
