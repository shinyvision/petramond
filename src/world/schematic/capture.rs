use super::World;
use crate::schematic::{CellData, ResolvedCell, Schematic, SchematicBuilder, SelectionBox};
use petramond_math::math::IVec3;
use petramond_world::{
    block::{Block, CellView, ShapeFamily, ShapeState},
    chunk::{section_idx, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY},
    section::Section,
};
use rustc_hash::{FxHashMap, FxHashSet};
use std::sync::Arc;

#[derive(Clone)]
enum SnapshotSection {
    Loaded(Arc<Section>),
    Uniform(Block),
}

/// Immutable section handles taken at one tick; cell reads happen on the worker.
pub(crate) struct Capture {
    name: String,
    regions: Vec<SelectionBox>,
    include_air: bool,
    /// Keep only the public construction design: no stored inventories,
    /// machine state, or cell data an item does not carry.
    public_only: bool,
    sections: FxHashMap<SectionPos, SnapshotSection>,
}

impl Capture {
    pub(crate) fn new(
        world: &World,
        name: String,
        regions: Vec<SelectionBox>,
        include_air: bool,
        public_only: bool,
    ) -> Result<Self, String> {
        if name.trim().is_empty() || name.len() > 128 {
            return Err("Use a name of 1–128 bytes".into());
        }
        if regions.is_empty() {
            return Err("Select blocks before saving".into());
        }
        for r in &regions {
            r.volume().ok_or("Invalid selection bounds")?;
            for p in [r.lo, r.hi.map(|v| v - 1)] {
                if !petramond_world::border::contains_column(p[0], p[2])
                    || SectionPos::from_world(p[0], p[1], p[2]).is_none()
                {
                    return Err("Selection is outside the world bounds".into());
                }
            }
        }
        let margin = petramond_world::block_model::all()
            .iter()
            .flat_map(|kind| petramond_world::block_model::footprint(*kind))
            .map(i32::from)
            .max()
            .unwrap_or(1)
            .max(1);
        let nearby: Vec<_> = regions
            .iter()
            .map(|r| SelectionBox {
                lo: r.lo.map(|p| p.saturating_sub(margin)),
                hi: r.hi.map(|p| p.saturating_add(margin)),
            })
            .collect();
        let needed = |sp: SectionPos| {
            let lo = [sp.cx * 16, sp.cy * 16, sp.cz * 16];
            let bounds = SelectionBox {
                lo,
                hi: lo.map(|p| p + 16),
            };
            nearby.iter().any(|r| r.intersection(bounds).is_some())
        };
        // Retain final sections for compound members too. Cloning these handles
        // does not copy blocks, inventories or mod data; writes use copy-on-write.
        let mut sections = FxHashMap::default();
        for (sp, section) in &world.sections {
            if needed(*sp) && world.physics_cell_final_at(sp.cx * 16, sp.cy * 16, sp.cz * 16) {
                sections.insert(*sp, SnapshotSection::Loaded(section.clone()));
            }
        }
        for cp in world.columns.keys() {
            for cy in SECTION_MIN_CY..=SECTION_MAX_CY {
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                if needed(sp)
                    && !sections.contains_key(&sp)
                    && world.physics_cell_final_at(sp.cx * 16, sp.cy * 16, sp.cz * 16)
                {
                    sections.insert(
                        sp,
                        SnapshotSection::Uniform(world.physics_block(
                            sp.cx * 16,
                            sp.cy * 16,
                            sp.cz * 16,
                        )),
                    );
                }
            }
        }
        Ok(Self {
            name,
            regions,
            include_air,
            public_only,
            sections,
        })
    }

    fn section(&self, p: [i32; 3]) -> Result<&SnapshotSection, String> {
        let sp =
            SectionPos::from_world(p[0], p[1], p[2]).ok_or("Cell is outside the world bounds")?;
        self.sections
            .get(&sp)
            .ok_or_else(|| "Wait for the selected terrain to load".into())
    }

    fn record(&self, data: ResolvedCell) -> CellData {
        if !self.public_only {
            return CellData::capture(&data);
        }
        let public =
            petramond_world::construction::Record::portable(data.block, data.state, &data.kv);
        CellData::capture(&ResolvedCell {
            block: data.block,
            state: data.state,
            fluid: data.fluid,
            kv: public.data,
            container: None,
            furnace: None,
        })
    }

    pub(crate) fn run(self) -> Result<Schematic, String> {
        let mut cells = SchematicBuilder::default();
        let mut visited = FxHashSet::default();
        for r in &self.regions {
            let section_bounds = SelectionBox {
                lo: r.lo.map(|p| p.div_euclid(16)),
                hi: r.hi.map(|p| (p - 1).div_euclid(16) + 1),
            };
            for chunk in section_bounds.cells() {
                if !visited.insert(chunk) {
                    continue;
                }
                let lo = chunk.map(|v| v * 16);
                let section = self.section(lo)?;
                if !self.include_air && section.empty() {
                    continue;
                }
                let bounds = SelectionBox {
                    lo,
                    hi: lo.map(|v| v + 16),
                };
                for clipped in self.regions.iter().filter_map(|r| r.intersection(bounds)) {
                    for p in clipped.cells() {
                        if cells.contains(p) {
                            continue;
                        }
                        let data = section.read(p);
                        if !self.include_air && data.block == Block::Air {
                            continue;
                        }
                        let footprint = footprint(p, &data);
                        cells.insert(p, self.record(data))?;
                        for peer in footprint {
                            if !cells.contains(peer) {
                                let data = self.section(peer)?.read(peer);
                                cells.insert(peer, self.record(data))?;
                            }
                        }
                    }
                }
            }
        }
        cells.normalized(self.name)
    }
}

impl SnapshotSection {
    fn empty(&self) -> bool {
        match self {
            Self::Loaded(s) => s.is_empty_air(),
            Self::Uniform(b) => *b == Block::Air,
        }
    }
    fn read(&self, p: [i32; 3]) -> ResolvedCell {
        let mut data = ResolvedCell {
            block: Block::Air,
            state: ShapeState::NONE,
            fluid: 0,
            kv: Default::default(),
            container: None,
            furnace: None,
        };
        match self {
            Self::Uniform(b) => data.block = *b,
            Self::Loaded(s) => {
                let [x, y, z] = p.map(|v| (v & 15) as usize);
                data.block = s.block(x, y, z);
                data.state = s.cell_state(x, y, z);
                data.fluid = s.fluid_meta(x, y, z);
                data.kv = s
                    .cell_kv()
                    .get(&(section_idx(x, y, z) as u16))
                    .cloned()
                    .unwrap_or_default();
                data.container = s.container_at(x, y, z).cloned();
                data.furnace = s.furnace_at(x, y, z).copied();
            }
        }
        data
    }
}

fn footprint(p: [i32; 3], data: &ResolvedCell) -> Vec<[i32; 3]> {
    // Compound ownership must follow the captured state, not later live-world edits.
    let pos = IVec3::from_array(p);
    if let Some(kind) = data.block.model_kind() {
        let state = petramond_world::block_model::ModelCellState::from_cell(data.state);
        let base =
            petramond_world::block_model::base_from_cell(pos, kind, state.offset, state.facing);
        petramond_world::block_model::oriented_footprint_cells(base, kind, state.facing)
            .into_iter()
            .map(|(p, _)| p.to_array())
            .collect()
    } else if data.block.shape_family() == ShapeFamily::Door {
        let door = petramond_world::door::DoorState::from_cell(data.state);
        vec![(pos + if door.top { -IVec3::Y } else { IVec3::Y }).to_array()]
    } else {
        Vec::new()
    }
}

#[cfg(test)]
mod tests;
