use super::{ResolvedCell, Schematic};
use crate::world::{World, WorldRole};
use petramond_math::math::IVec3;
use petramond_world::{
    chunk::{SectionPos, SECTION_VOLUME, SKY_FULL},
    section::Section,
};
use std::{collections::HashMap, sync::Arc};

/// An isolated section snapshot, including the active pack's baked shapes.
pub struct Scene {
    pub size: [i32; 3],
    pub origin: IVec3,
    pub cells: Vec<(IVec3, ResolvedCell)>,
    pub sections: HashMap<SectionPos, Arc<Section>>,
}

impl Scene {
    /// A scene of explicit cells at scene-local positions (`origin` + the
    /// cell's local position), baked like a whole-design preview. Used for one
    /// piece of a larger ghost: the cells a piece shows plus its bordering
    /// cells, so shared faces cull.
    pub fn from_cells(
        size: [i32; 3],
        cells: Vec<(IVec3, ResolvedCell)>,
        bake: impl FnOnce(&mut World),
    ) -> Result<Self, String> {
        let origin = IVec3::new(0, petramond_world::chunk::WORLD_MIN_Y, 0);
        let cells = cells
            .into_iter()
            .map(|(p, data)| (origin + p, data))
            .collect();
        Self::install(size, origin, cells, bake)
    }

    pub fn prepare(
        schematic: &Schematic,
        turns: u8,
        bake: impl FnOnce(&mut World),
    ) -> Result<Self, String> {
        let size = schematic.rotated_size(turns);
        let origin = IVec3::new(
            -size[0] / 2,
            petramond_world::chunk::WORLD_MIN_Y,
            -size[2] / 2,
        );
        let cells = schematic.placed_cells(origin, turns)?;
        Self::install(schematic.rotated_size(turns), origin, cells, bake)
    }

    fn install(
        size: [i32; 3],
        origin: IVec3,
        cells: Vec<(IVec3, ResolvedCell)>,
        bake: impl FnOnce(&mut World),
    ) -> Result<Self, String> {
        let mut sections = HashMap::new();
        for (p, data) in &cells {
            let sp = SectionPos::from_world(p.x, p.y, p.z).ok_or("Invalid preview position")?;
            let s = sections
                .entry(sp)
                .or_insert_with(|| Section::new(sp.cx, sp.cy, sp.cz));
            let (x, y, z) = (
                (p.x & 15) as usize,
                (p.y & 15) as usize,
                (p.z & 15) as usize,
            );
            s.set_fluid(x, y, z, data.block, data.fluid);
            s.set_cell_state(x, y, z, data.state);
            s.cell_kv_restore(x, y, z, data.kv.clone());
        }
        let mut world = World::new_with_pool(
            0,
            1,
            WorldRole::ClientReplica,
            Arc::new(crate::worker::JobPool::inline()),
        );
        let sky: Arc<[u8]> = vec![SKY_FULL; SECTION_VOLUME].into();
        for (sp, mut section) in sections {
            section.set_skylight(sky.clone());
            world.install_cached_section(sp, Arc::new(section));
        }
        for (p, data) in &cells {
            world.mark_custom_bake_edit(p.x, p.y, p.z, data.block);
        }
        bake(&mut world);
        Ok(Self {
            origin,
            size,
            cells,
            sections: world
                .sections
                .iter()
                .map(|(p, s)| (*p, s.clone()))
                .collect(),
        })
    }
}
