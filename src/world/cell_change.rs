use std::collections::HashMap;

use petramond_math::math::IVec3;
use petramond_world::block::Block;
use petramond_world::chunk::SectionPos;
use rustc_hash::FxHashSet;

use super::store::SkyCoverChange;
use crate::world::{World, WorldSide};

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::world) enum ChangeKind {
    Write,
    Place,
    Replace,
    Costume,
    Remote,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
pub(in crate::world) struct CellChange {
    pub pos: IVec3,
    pub old: Block,
    pub kind: ChangeKind,
}

impl CellChange {
    pub(in crate::world) fn new(pos: IVec3, old: Block, kind: ChangeKind) -> Self {
        Self { pos, old, kind }
    }
}

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn apply_cell_changes(&mut self, changes: &[CellChange]) {
        if changes.is_empty() {
            return;
        }
        let news: Vec<Block> = changes
            .iter()
            .map(|c| Block::from_id(self.data.chunk_block(c.pos.x, c.pos.y, c.pos.z)))
            .collect();

        let mut sections = FxHashSet::default();
        for c in changes {
            if let Some(sp) = SectionPos::from_world(c.pos.x, c.pos.y, c.pos.z) {
                if sections.insert(sp) {
                    self.refresh_presented_index(sp);
                    self.refresh_block_entity_index(sp);
                }
            }
        }

        for (c, &new) in changes.iter().zip(&news) {
            let keeps_owner = match c.kind {
                ChangeKind::Costume => true,
                ChangeKind::Replace => false,
                ChangeKind::Write | ChangeKind::Place => false,
                ChangeKind::Remote => c.old == new,
            };
            if keeps_owner {
                self.refresh_block_draw_placement(c.pos);
            } else {
                self.forget_block_draw(c.pos);
            }
        }

        let mut sky_changed: HashMap<(i32, i32), SkyCoverChange> = HashMap::new();
        for (c, &new) in changes.iter().zip(&news) {
            if let Some(change) =
                self.update_column_heights_after_set(c.pos.x, c.pos.y, c.pos.z, new)
            {
                if c.kind == ChangeKind::Remote {
                    continue;
                }
                sky_changed
                    .entry((c.pos.x, c.pos.z))
                    .and_modify(|all| all.merge(change))
                    .or_insert(change);
            }
        }
        for ((wx, wz), change) in sky_changed {
            self.mark_sky_cover_edited_at(wx, wz, change);
        }

        for (c, &new) in changes.iter().zip(&news) {
            let IVec3 { x, y, z } = c.pos;
            self.queue_dirty_meshes_sampling_cell(x, y, z);
            self.mark_custom_bake_edit(x, y, z, new);
            if c.kind != ChangeKind::Remote {
                self.refine_shape_states_around(x, y, z);
                self.announce_cell_change(c, new);
            }
        }

        self.mark_visibility_dirty();
    }

    fn announce_cell_change(&mut self, c: &CellChange, new: Block) {
        let IVec3 { x, y, z } = c.pos;
        if c.kind != ChangeKind::Write {
            self.notify_block_and_neighbors(x, y, z);
            return;
        }
        let nav_relevant = !super::tick::edit_nav_equivalent(c.old, new);
        if c.old.has_same_light_behavior(new) {
            self.notify_light_equivalent_change_nav(x, y, z, nav_relevant);
        } else {
            let radius = self.edit_light_reach(x, y, z, c.old, new);
            self.notify_block_change_with_light_radius_nav(x, y, z, radius, nav_relevant);
        }
    }
}
