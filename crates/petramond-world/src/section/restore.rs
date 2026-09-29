use std::collections::BTreeMap;
use std::sync::Arc;

use crate::block::ShapeState;
use crate::block_state::BlockStates;
use crate::chunk::SECTION_VOLUME;
use crate::container::Container;
use crate::furnace::Furnace;

use super::{BlockEntities, CellMap, Section, SectionMetrics, SectionSummary};

impl Section {
    #[allow(clippy::too_many_arguments)]
    pub fn from_saved(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: &[u16],
        fluid: Option<Box<[u8]>>,
        furnaces: CellMap<Furnace>,
        containers: CellMap<Container>,
        cell_states: CellMap<ShapeState>,
        cell_kv: CellMap<BTreeMap<String, Vec<u8>>>,
    ) -> Self {
        Self::from_shared(
            cx,
            cy,
            cz,
            super::BlockCube::from_ids(blocks),
            fluid.map(Arc::from),
            furnaces,
            containers,
            cell_states,
            cell_kv,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn from_replica(
        cx: i32,
        cy: i32,
        cz: i32,
        blocks: super::BlockCube,
        fluid: Option<Arc<[u8]>>,
        furnaces: CellMap<Furnace>,
        containers: CellMap<Container>,
        cell_states: CellMap<ShapeState>,
        cell_kv: CellMap<BTreeMap<String, Vec<u8>>>,
        metrics: SectionMetrics,
    ) -> Self {
        debug_assert!(metrics.valid());
        Self::from_shared(
            cx,
            cy,
            cz,
            blocks,
            fluid,
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
        fluid: Option<Arc<[u8]>>,
        furnaces: CellMap<Furnace>,
        containers: CellMap<Container>,
        cell_states: CellMap<ShapeState>,
        cell_kv: CellMap<BTreeMap<String, Vec<u8>>>,
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
            states: BlockStates::from_shared(fluid, cell_states, cell_kv),
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
            fluid_count: 0,
            quench_count: 0,
            quencher_count: 0,
            biome_tint_count: 0,
            particle_emitter_cells: Vec::new(),
            light_emitter_count: 0,
            shape_render: None,
            light_apertures: None,
            present: super::IdSet::EMPTY,
        };
        if let Some(metrics) = metrics {
            s.install_metrics(metrics);
            if s.non_air_count == 0 {
                s.blocks.fill(0);
            } else if s.water_count as usize == SECTION_VOLUME {
                s.blocks
                    .fill(SectionSummary::FullWater.virtual_block().id());
            }
            s.present = super::IdSet::of_cube(&s.blocks);
        } else {
            s.recompute_opaque_count();
        }
        s
    }

    pub fn same_content(&self, other: &Section) -> bool {
        let Section {
            cx,
            cy,
            cz,
            blocks,
            states,
            entities,
            dirty: _,
            modified: _,
            skylight,
            blocklight,
            light_dirty: _,
            light_from_persist: _,
            light_revision: _,
            mesh_revision: _,
            random_tick_count,
            opaque_count,
            plane_opaque,
            non_air_count,
            water_count,
            fluid_count,
            quench_count,
            quencher_count,
            biome_tint_count,
            particle_emitter_cells,
            light_emitter_count,
            shape_render: _,
            light_apertures: _,
            present: _,
        } = self;
        (*cx, *cy, *cz) == (other.cx, other.cy, other.cz)
            && *random_tick_count == other.random_tick_count
            && *opaque_count == other.opaque_count
            && *plane_opaque == other.plane_opaque
            && *non_air_count == other.non_air_count
            && *water_count == other.water_count
            && *fluid_count == other.fluid_count
            && *quench_count == other.quench_count
            && *quencher_count == other.quencher_count
            && *biome_tint_count == other.biome_tint_count
            && *light_emitter_count == other.light_emitter_count
            && *particle_emitter_cells == other.particle_emitter_cells
            && skylight.as_deref() == other.skylight.as_deref()
            && blocklight.as_deref() == other.blocklight.as_deref()
            && *entities == other.entities
            && *states == other.states
            && *blocks == other.blocks
    }
}
