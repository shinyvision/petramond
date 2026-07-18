use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;

use crate::block::Block;
use crate::block_state::{BlockStates, LogAxis, SlabState, StairState};
use crate::chunk::SECTION_VOLUME;
use crate::container::Container;
use crate::door::DoorState;
use crate::facing::Facing;
use crate::furnace::Furnace;
use crate::torch::TorchPlacement;

use super::{uniform_cube, BlockEntities, Section, SectionMetrics};

impl Section {
    /// Rebuild a section from saved arrays. `modified` starts false — it already
    /// matches what's on disk. The light cube is left for the async bake.
    #[allow(clippy::too_many_arguments)]
    pub fn from_saved(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Box<[u8]>,
        water: Option<Box<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        Self::from_shared(
            cx,
            cy,
            cz,
            blocks.into(),
            water.map(Arc::from),
            furnaces,
            containers,
            entity_facings,
            torches,
            model_cells,
            model_facings,
            doors,
            stairs,
            slabs,
            log_axes,
            cell_kv,
            None,
        )
    }

    /// Rebuild a replica section from immutable wire buffers and server-derived
    /// counters. No voxel buffer is copied or scanned on the render thread.
    #[allow(clippy::too_many_arguments)]
    pub(crate) fn from_replica(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Arc<[u8]>,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
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
            entity_facings,
            torches,
            model_cells,
            model_facings,
            doors,
            stairs,
            slabs,
            log_axes,
            cell_kv,
            Some(metrics),
        )
    }

    #[allow(clippy::too_many_arguments)]
    fn from_shared(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: Arc<[u8]>,
        water: Option<Arc<[u8]>>,
        furnaces: HashMap<u16, Furnace>,
        containers: HashMap<u16, Container>,
        entity_facings: HashMap<u16, Facing>,
        torches: HashMap<u16, TorchPlacement>,
        model_cells: HashMap<u16, [u8; 3]>,
        model_facings: HashMap<u16, Facing>,
        doors: HashMap<u16, DoorState>,
        stairs: HashMap<u16, StairState>,
        slabs: HashMap<u16, SlabState>,
        log_axes: HashMap<u16, LogAxis>,
        cell_kv: HashMap<u16, BTreeMap<String, Vec<u8>>>,
        metrics: Option<SectionMetrics>,
    ) -> Self {
        let entities = BlockEntities {
            furnaces,
            containers,
            entity_facings,
        };
        let mut s = Self {
            cx,
            cy,
            cz,
            blocks,
            states: BlockStates::from_shared(
                water,
                torches,
                model_cells,
                model_facings,
                doors,
                stairs,
                slabs,
                log_axes,
                cell_kv,
            ),
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
            particle_emitter_count: 0,
            light_emitter_count: 0,
        };
        if let Some(metrics) = metrics {
            s.install_metrics(metrics);
            if s.non_air_count == 0 {
                s.blocks = uniform_cube(0);
            } else if s.water_count as usize == SECTION_VOLUME {
                s.blocks = uniform_cube(Block::Water.id());
            }
        } else {
            s.recompute_opaque_count();
        }
        s
    }
}
