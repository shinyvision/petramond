//! Complete-cell snapshots and the one applicator that installs them.
//!
//! A [`CellEdit`] is validated whole, then written in budgeted slices so an
//! edit of any size spreads over ticks while staying ONE edit: one receipt,
//! one set of overwritten cells. Compound blocks (doors, multi-cell models)
//! never straddle a slice — a slice pulls in every member of each compound it
//! touches, both the ones it writes and the ones it overwrites.

use super::World;
use crate::schematic::ResolvedCell;
use petramond_math::math::IVec3;
use petramond_world::{
    block::{Block, CellView, ShapeFamily, ShapeState},
    chunk::section_idx,
};
use std::collections::{BTreeMap, HashMap, HashSet};

pub type Cells = Vec<(IVec3, ResolvedCell)>;

#[derive(Clone, Copy, Debug, Default)]
pub struct CellPolicy {
    /// Keep the overwritten cells for the receipt.
    pub record: bool,
    /// Once the edit completes, every non-air block inside these inclusive
    /// bounds re-runs its block update: cells the edit skipped still lose
    /// support to the ones it wrote.
    pub update_bounds: Option<[IVec3; 2]>,
}

/// What a slice did to the block population, keyed at each block's anchor so
/// a multi-cell block is announced once.
pub struct CellHooks<'a> {
    pub removed: &'a [(IVec3, Block)],
    pub placed: &'a [(IVec3, Block)],
}

/// The outcome of an edit, complete or interrupted.
pub struct EditReceipt {
    /// The requested cells, in the caller's order.
    pub target: Cells,
    /// How many `target` cells were written (all of them unless interrupted).
    pub written: usize,
    /// Air written over the rest of each overwritten compound block.
    pub cleared: Cells,
    /// The overwritten cells, when the policy records them.
    pub before: Cells,
    /// Inclusive bounds of `target`.
    pub bounds: [IVec3; 2],
}

/// An edit in progress. Dropping it abandons the unwritten remainder.
pub struct CellEdit {
    target: Cells,
    index: HashMap<IVec3, u32>,
    done: Vec<bool>,
    cursor: usize,
    written: usize,
    cleared: Cells,
    cleared_at: HashSet<IVec3>,
    before: Cells,
    bounds: [IVec3; 2],
    policy: CellPolicy,
}

impl CellEdit {
    pub fn bounds(&self) -> [IVec3; 2] {
        self.bounds
    }

    pub fn len(&self) -> usize {
        self.target.len()
    }

    pub fn is_empty(&self) -> bool {
        self.target.is_empty()
    }

    pub fn is_complete(&self) -> bool {
        self.written == self.target.len()
    }

    /// Close the edit. An interrupted edit moves its unwritten cells to the
    /// tail so `target[..written]` is exactly what reached the world.
    pub fn finish(mut self) -> EditReceipt {
        if !self.is_complete() {
            let mut kept = Vec::with_capacity(self.written);
            let mut rest = Vec::new();
            for (cell, done) in self.target.into_iter().zip(&self.done) {
                if *done {
                    kept.push(cell);
                } else {
                    rest.push(cell);
                }
            }
            kept.append(&mut rest);
            self.target = kept;
        }
        EditReceipt {
            target: self.target,
            written: self.written,
            cleared: self.cleared,
            before: self.before,
            bounds: self.bounds,
        }
    }
}

const AIR: ResolvedCell = ResolvedCell {
    block: Block::Air,
    state: ShapeState::NONE,
    fluid: 0,
    kv: BTreeMap::new(),
    container: None,
    furnace: None,
};

impl World {
    /// Snapshot one stream-final cell, including every persistent block-owned store.
    pub fn snapshot_cell(&self, pos: IVec3) -> Result<ResolvedCell, String> {
        self.cell_readable(pos)?;
        let mut data = ResolvedCell {
            block: Block::from_id(self.chunk_block(pos.x, pos.y, pos.z)),
            ..AIR
        };
        if let Some((s, x, y, z)) = self.chunk_at_world(pos.x, pos.y, pos.z) {
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
        Ok(data)
    }

    /// Whether the cell already holds exactly `data`, without copying it out.
    fn cell_holds(&self, pos: IVec3, data: &ResolvedCell) -> bool {
        if Block::from_id(self.chunk_block(pos.x, pos.y, pos.z)) != data.block {
            return false;
        }
        let Some((s, x, y, z)) = self.chunk_at_world(pos.x, pos.y, pos.z) else {
            return data.state == ShapeState::NONE
                && data.fluid == 0
                && data.kv.is_empty()
                && data.container.is_none()
                && data.furnace.is_none();
        };
        s.cell_state(x, y, z) == data.state
            && s.fluid_meta(x, y, z) == data.fluid
            && s.cell_kv()
                .get(&(section_idx(x, y, z) as u16))
                .map_or(data.kv.is_empty(), |kv| *kv == data.kv)
            && s.container_at(x, y, z) == data.container.as_ref()
            && s.furnace_at(x, y, z) == data.furnace.as_ref()
    }

    fn cell_readable(&self, pos: IVec3) -> Result<(), String> {
        if !petramond_world::border::contains_column(pos.x, pos.z)
            || petramond_world::chunk::SectionPos::from_world(pos.x, pos.y, pos.z).is_none()
        {
            return Err("Cell is outside the world bounds".into());
        }
        if !self.physics_cell_final_at(pos.x, pos.y, pos.z) {
            return Err("Wait for the selected terrain to load".into());
        }
        Ok(())
    }

    fn cell_writable(&self, p: IVec3) -> Result<(), String> {
        if !petramond_world::border::contains_column(p.x, p.z) {
            return Err("Edit crosses the world border".into());
        }
        let sp = petramond_world::chunk::SectionPos::from_world(p.x, p.y, p.z)
            .ok_or("Edit crosses the world height limit")?;
        if !self.stream_writable(sp) || !self.physics_cell_final_at(p.x, p.y, p.z) {
            return Err("Wait for the destination terrain to load".into());
        }
        Ok(())
    }

    /// Validate the complete edit before anything is written. A refusal hands
    /// the cells back untouched.
    pub fn begin_cells(
        &self,
        target: Cells,
        policy: CellPolicy,
    ) -> Result<CellEdit, (Cells, String)> {
        let mut index = HashMap::with_capacity(target.len());
        let mut bounds = [IVec3::MAX, IVec3::MIN];
        let mut checked = if target.is_empty() {
            Err("Empty edit".to_string())
        } else {
            Ok(())
        };
        for (i, (p, _)) in target.iter().enumerate() {
            if index.insert(*p, i as u32).is_some() {
                checked = Err("Duplicate edit cell".to_string());
            }
            bounds = [bounds[0].min(*p), bounds[1].max(*p)];
        }
        let checked = checked
            .and_then(|()| target.iter().try_for_each(|(p, _)| self.cell_writable(*p)))
            .and_then(|()| validate_compounds(&target, &index));
        match checked {
            Ok(()) => Ok(CellEdit {
                done: vec![false; target.len()],
                target,
                index,
                cursor: 0,
                written: 0,
                cleared: Vec::new(),
                cleared_at: HashSet::new(),
                before: Vec::new(),
                bounds,
                policy,
            }),
            Err(message) => Err((target, message)),
        }
    }

    /// Write the next slice of at most roughly `budget` cells (a compound is
    /// never split, so a slice may run slightly over). `hooks` runs between
    /// the write and the final seating of instance state, so a listener that
    /// treats every placement as a fresh one cannot reinitialise a copied
    /// machine. Returns whether the edit is complete.
    pub fn step_cells(
        &mut self,
        edit: &mut CellEdit,
        budget: usize,
        hooks: &mut dyn FnMut(&mut World, CellHooks<'_>),
    ) -> Result<bool, String> {
        let (slice, cleared) = self.next_slice(edit, budget)?;
        if !slice.is_empty() || !cleared.is_empty() {
            let air = AIR;
            let cells: Vec<(IVec3, &ResolvedCell)> = slice
                .iter()
                .map(|&i| {
                    let (p, data) = &edit.target[i as usize];
                    (*p, data)
                })
                .chain(cleared.iter().map(|p| (*p, &air)))
                .collect();
            for (p, _) in &cells {
                self.cell_writable(*p)?;
            }
            for (p, _) in &cells {
                if !self.materialize_section_at(*p) {
                    return Err("Destination terrain is unavailable".into());
                }
            }
            if edit.policy.record {
                let before = cells
                    .iter()
                    .map(|(p, _)| self.snapshot_cell(*p).map(|d| (*p, d)))
                    .collect::<Result<Vec<_>, _>>()?;
                edit.before.extend(before);
            }
            let removed = self.anchored_blocks(
                cells
                    .iter()
                    .map(|(p, _)| (*p, Block::from_id(self.chunk_block(p.x, p.y, p.z)))),
            );
            self.install_cells(&cells);
            let placed = self.anchored_blocks(cells.iter().map(|(p, d)| (*p, d.block)));
            hooks(
                self,
                CellHooks {
                    removed: &removed,
                    placed: &placed,
                },
            );
            let disturbed: Vec<_> = cells
                .iter()
                .filter(|(p, data)| !self.cell_holds(*p, data))
                .copied()
                .collect();
            if !disturbed.is_empty() {
                self.install_cells(&disturbed);
            }
            for i in slice {
                edit.done[i as usize] = true;
                edit.written += 1;
            }
            for p in cleared {
                edit.cleared_at.insert(p);
                edit.cleared.push((p, AIR));
            }
        }
        if !edit.is_complete() {
            return Ok(false);
        }
        if let Some([min, max]) = edit.policy.update_bounds {
            self.queue_non_air_updates_in_box(min, max);
        }
        Ok(true)
    }

    /// Apply a whole edit at once, with no listeners.
    pub fn apply_cells(&mut self, cells: Cells, policy: CellPolicy) -> Result<EditReceipt, String> {
        let mut edit = self.begin_cells(cells, policy).map_err(|(_, e)| e)?;
        self.step_cells(&mut edit, usize::MAX, &mut |_, _| {})?;
        Ok(edit.finish())
    }

    /// The next unwritten target cells, closed over compound membership, plus
    /// the cells outside the target that overwritten compounds leave behind.
    fn next_slice(
        &self,
        edit: &mut CellEdit,
        budget: usize,
    ) -> Result<(Vec<u32>, Vec<IVec3>), String> {
        let mut slice = Vec::new();
        let mut cleared = Vec::new();
        let mut taken = HashSet::new();
        let mut work = Vec::new();
        while slice.len() < budget.max(1) {
            while edit.cursor < edit.target.len()
                && (edit.done[edit.cursor] || taken.contains(&(edit.cursor as u32)))
            {
                edit.cursor += 1;
            }
            if edit.cursor == edit.target.len() {
                break;
            }
            work.push(edit.cursor as u32);
            taken.insert(edit.cursor as u32);
            while let Some(i) = work.pop() {
                slice.push(i);
                let (pos, data) = &edit.target[i as usize];
                self.cell_readable(*pos)?;
                let members = compound_members(*pos, data)
                    .into_iter()
                    .flatten()
                    .chain(self.break_footprint_cells(*pos));
                for p in members {
                    match edit.index.get(&p) {
                        Some(&j) => {
                            if !edit.done[j as usize] && taken.insert(j) {
                                work.push(j);
                            }
                        }
                        None => {
                            if !edit.cleared_at.contains(&p) && !cleared.contains(&p) {
                                cleared.push(p);
                            }
                        }
                    }
                }
            }
        }
        Ok((slice, cleared))
    }

    fn anchored_blocks(&self, cells: impl Iterator<Item = (IVec3, Block)>) -> Vec<(IVec3, Block)> {
        cells
            .filter(|(_, block)| *block != Block::Air)
            .map(|(p, block)| (self.container_anchor(p).to_array(), block))
            .collect::<BTreeMap<_, _>>()
            .into_iter()
            .map(|(p, block)| (IVec3::from_array(p), block))
            .collect()
    }

    /// Install raw records. Callers have validated and materialized the cells.
    fn install_cells(&mut self, cells: &[(IVec3, &ResolvedCell)]) {
        for (p, _) in cells {
            self.forget_block_draw(*p);
        }
        for (p, data) in cells {
            let (s, x, y, z) = self
                .chunk_at_world_mut(p.x, p.y, p.z)
                .expect("materialized edit");
            s.take_container(x, y, z);
            s.take_furnace(x, y, z);
            s.set_block(x, y, z, data.block);
            s.set_fluid(x, y, z, data.block, data.fluid);
            s.set_cell_state(x, y, z, data.state);
            s.cell_kv_restore(x, y, z, data.kv.clone());
            if let Some(c) = &data.container {
                s.insert_container(x, y, z, c.clone());
            }
            if let Some(f) = data.furnace {
                s.insert_furnace(x, y, z, f);
            }
            s.modified = true;
        }
        let positions: Vec<_> = cells.iter().map(|(p, _)| *p).collect();
        for (p, data) in cells {
            self.mark_custom_bake_edit(p.x, p.y, p.z, data.block);
            self.note_block_entity_change(*p);
        }
        self.terrain.vis_dirty = true;
        self.refresh_region(&positions);
        // Neighbours see the completed edit; the copied cells retain their
        // authored connection/corner state until a later world edit changes it.
        for (p, data) in cells {
            let (s, x, y, z) = self
                .chunk_at_world_mut(p.x, p.y, p.z)
                .expect("materialized edit");
            if s.cell_state(x, y, z) != data.state {
                s.set_cell_state(x, y, z, data.state);
                self.notify_block_and_neighbors(p.x, p.y, p.z);
            }
        }
    }
}

/// Every cell of the compound block `data` belongs to at `pos`; `None` for a
/// single-cell block.
fn compound_members(pos: IVec3, data: &ResolvedCell) -> Option<Vec<IVec3>> {
    use petramond_world::block_model::{base_from_cell, oriented_footprint_cells, ModelCellState};
    if let Some(kind) = data.block.model_kind() {
        let model = ModelCellState::from_cell(data.state);
        let base = base_from_cell(pos, kind, model.offset, model.facing);
        Some(
            oriented_footprint_cells(base, kind, model.facing)
                .into_iter()
                .map(|(p, _)| p)
                .collect(),
        )
    } else if data.block.shape_family() == ShapeFamily::Door {
        let door = petramond_world::door::DoorState::from_cell(data.state);
        Some(vec![pos, pos + if door.top { -IVec3::Y } else { IVec3::Y }])
    } else {
        None
    }
}

fn validate_compounds(cells: &Cells, index: &HashMap<IVec3, u32>) -> Result<(), String> {
    use petramond_world::block_model::{base_from_cell, oriented_footprint_cells, ModelCellState};
    let at = |p: &IVec3| index.get(p).map(|&i| &cells[i as usize].1);
    for (pos, data) in cells {
        if let Some(kind) = data.block.model_kind() {
            let model = ModelCellState::from_cell(data.state);
            let base = base_from_cell(*pos, kind, model.offset, model.facing);
            let mut member = false;
            for (p, offset) in oriented_footprint_cells(base, kind, model.facing) {
                member |= p == *pos;
                let expected = ModelCellState {
                    offset,
                    facing: model.facing,
                };
                if !at(&p).is_some_and(|d| {
                    d.block == data.block && ModelCellState::from_cell(d.state) == expected
                }) {
                    return Err("Edit contains an incomplete model block".into());
                }
            }
            if !member {
                return Err("Invalid model offset".into());
            }
        } else if data.block.shape_family() == ShapeFamily::Door {
            let door = petramond_world::door::DoorState::from_cell(data.state);
            let other = *pos + if door.top { -IVec3::Y } else { IVec3::Y };
            if !at(&other).is_some_and(|d| {
                d.block == data.block && {
                    let peer = petramond_world::door::DoorState::from_cell(d.state);
                    peer.top != door.top && peer.facing == door.facing && peer.open == door.open
                }
            }) {
                return Err("Edit contains half a door".into());
            }
        }
    }
    Ok(())
}
