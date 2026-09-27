use crate::world::ServerWorld;
use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::construction::{self, Plan, Record, Status};
use petramond_world::item::ItemStack;
use petramond_world::world::placement::PlacementPlan;

#[cfg(test)]
mod tests;

#[derive(Clone, Debug, PartialEq)]
pub enum CellStatus {
    Unloaded,
    Satisfied,
    Place {
        missing: Vec<ItemStack>,
        writes: PlacementPlan,
    },
    Clear {
        at: IVec3,
        block: Block,
        footprint: Vec<IVec3>,
        holds_items: bool,
    },
    Pending(IVec3),
    Unsupported(String),
}

impl ServerWorld {
    pub fn construction_record(&self, pos: IVec3) -> Option<Record> {
        self.physics_cell_final_at(pos.x, pos.y, pos.z)
            .then(|| Record::at(&self.data, pos))
    }

    pub fn construction_status(&self, pos: IVec3, record: &Record) -> CellStatus {
        let cells = match construction::plan(record, pos) {
            Plan::Unit { writes, .. } => writes.writes.iter().map(|w| w.cell).collect(),
            _ => vec![pos],
        };
        if cells.iter().any(|c| {
            !petramond_world::border::contains_column(c.x, c.z)
                || petramond_world::chunk::SectionPos::from_world(c.x, c.y, c.z).is_none()
        }) {
            return CellStatus::Unsupported("outside the world".into());
        }
        if cells
            .iter()
            .any(|c| !self.physics_cell_final_at(c.x, c.y, c.z))
        {
            return CellStatus::Unloaded;
        }
        match construction::status(&self.data, pos, record) {
            Status::Satisfied => CellStatus::Satisfied,
            Status::Place { missing, writes } => CellStatus::Place { missing, writes },
            Status::Pending(anchor) => CellStatus::Pending(anchor),
            Status::Unsupported(reason) => CellStatus::Unsupported(reason),
            Status::Clear { at, block } => {
                let footprint = self.break_footprint_cells(at);
                let holds_items = self
                    .container_at(self.container_anchor(at))
                    .is_some_and(|c| c.slots.iter().any(Option::is_some));
                CellStatus::Clear {
                    at,
                    block,
                    footprint,
                    holds_items,
                }
            }
        }
    }
}
