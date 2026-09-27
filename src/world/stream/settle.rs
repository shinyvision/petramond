use crate::world::store::for_each_column_cy;
use crate::world::ServerWorld;
use petramond_world::chunk::{ChunkPos, SectionPos};

use crate::world::store::LoadTarget;

impl ServerWorld {
    fn gen_neighborhood_settled(&self, sp: SectionPos, target: LoadTarget) -> bool {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    if !SectionPos::cy_in_range(n.cy) || self.data.sections.contains_key(&n) {
                        continue;
                    }
                    if self.side.gen.pending_sections.contains(&n) {
                        return false;
                    }
                    let cp = n.chunk_pos();
                    if self.side.gen.column_gen.contains_key(&cp) {
                        continue;
                    }
                    if self.side.gen.pending.contains_key(&cp) || Self::column_wanted(target, cp) {
                        return false;
                    }
                }
            }
        }
        true
    }

    pub(super) fn queue_deferred_rechecks_around(&mut self, pos: SectionPos) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    self.data.deferred_rechecks.insert(SectionPos::new(
                        pos.cx + dx,
                        pos.cy + dy,
                        pos.cz + dz,
                    ));
                }
            }
        }
    }

    pub(super) fn queue_deferred_rechecks_around_column(&mut self, pos: ChunkPos) {
        for cz in pos.cz - 1..=pos.cz + 1 {
            for cx in pos.cx - 1..=pos.cx + 1 {
                let cp = ChunkPos::new(cx, cz);
                let bits = self.data.section_column_cys.get(&cp).copied().unwrap_or(0);
                for_each_column_cy(bits, |cy| {
                    self.data
                        .deferred_rechecks
                        .insert(SectionPos::new(cx, cy, cz));
                });
            }
        }
    }

    pub(super) fn flush_settled_deferred_if_needed(&mut self, target: LoadTarget) {
        let check: Vec<SectionPos> = if self.data.deferred_recheck_needed {
            self.data.deferred_recheck_needed = false;
            self.data.deferred_rechecks.clear();
            self.data.light_deferred.iter().copied().collect()
        } else {
            std::mem::take(&mut self.data.deferred_rechecks)
                .into_iter()
                .filter(|sp| self.data.light_deferred.contains(sp))
                .collect()
        };
        if check.is_empty() {
            return;
        }
        self.flush_settled_deferred_positions(target, check);
    }

    #[cfg(test)]
    pub(super) fn flush_settled_deferred(&mut self, target: LoadTarget) {
        let check = self.data.light_deferred.iter().copied().collect();
        self.flush_settled_deferred_positions(target, check);
    }

    fn flush_settled_deferred_positions(&mut self, target: LoadTarget, check: Vec<SectionPos>) {
        let ready: Vec<SectionPos> = check
            .into_iter()
            .filter(|sp| {
                !self.side.gen.pending_overlays.contains_key(sp)
                    && self.gen_neighborhood_settled(*sp, target)
            })
            .collect();
        let mut bakes: Vec<SectionPos> = Vec::new();
        for sp in ready {
            let Some(section) = self.data.sections.get(&sp) else {
                self.data.light_deferred.remove(&sp);
                continue;
            };
            let needs_bake = section.light_dirty && !section.all_opaque();
            if needs_bake && self.section_sealed_by_loaded_neighbors(sp) {
                continue;
            }
            self.data.light_deferred.remove(&sp);
            if needs_bake {
                bakes.push(sp);
            }
        }
        for (base, members) in crate::world::light::group_positions(&bakes) {
            if members.len() >= 3 {
                let key = members
                    .iter()
                    .map(|&sp| target.section_priority_key(sp))
                    .min()
                    .unwrap_or(0);
                self.light_bakes.request_batch(
                    key,
                    base,
                    &members,
                    &self.data.sections,
                    &self.data.columns,
                );
            } else {
                for sp in members {
                    let key = target.section_priority_key(sp);
                    self.light_bakes
                        .request(key, sp, &self.data.sections, &self.data.columns);
                }
            }
        }
    }
}
