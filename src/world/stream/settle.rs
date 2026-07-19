use crate::chunk::{ChunkPos, SectionPos};

use crate::world::store::{LoadTarget, World};

impl World {
    /// Whether everything the FIRST light/mesh of `sp` could read has landed: each
    /// 3×3×3 neighbour is loaded, or is provably not coming under `target` — outside
    /// the wanted shape, deliberately skipped by its landed column (sky / outside the
    /// vertical+surface window), or out of world range — so absent-as-air is its final
    /// state. A neighbour still pending (or whose column is pending or wanted but not
    /// yet landed) means a bake now would just be redone when it arrives.
    fn gen_neighborhood_settled(&self, sp: SectionPos, target: LoadTarget) -> bool {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    let n = SectionPos::new(sp.cx + dx, sp.cy + dy, sp.cz + dz);
                    if !SectionPos::cy_in_range(n.cy) || self.sections.contains_key(&n) {
                        continue;
                    }
                    if self.pending_sections.contains(&n) {
                        return false;
                    }
                    let cp = n.chunk_pos();
                    if self.column_gen.contains_key(&cp) {
                        continue;
                    }
                    if self.pending.contains_key(&cp) || Self::column_wanted(target, cp) {
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
                    self.deferred_rechecks.insert(SectionPos::new(
                        pos.cx + dx,
                        pos.cy + dy,
                        pos.cz + dz,
                    ));
                }
            }
        }
    }

    pub(super) fn queue_deferred_rechecks_around_column(&mut self, pos: ChunkPos) {
        // Only loaded sections can be in `light_deferred`; phantoms across the
        // full vertical range just churn the recheck set and die in the filter.
        for cz in pos.cz - 1..=pos.cz + 1 {
            for cx in pos.cx - 1..=pos.cx + 1 {
                let cp = ChunkPos::new(cx, cz);
                let bits = self.section_column_cys.get(&cp).copied().unwrap_or(0);
                Self::for_each_column_cy(bits, |cy| {
                    self.deferred_rechecks.insert(SectionPos::new(cx, cy, cz));
                });
            }
        }
    }

    /// Flush deferred sections whose generation neighbourhood has settled:
    /// request the single light bake (skipped when the section landed with
    /// clean persisted light) and queue the single first mesh. Sections whose
    /// saved overlay is still buffered stay parked so the bake reads the saved
    /// blocks, not the generated base it is about to replace.
    pub(super) fn flush_settled_deferred_if_needed(&mut self, target: LoadTarget) {
        let check: Vec<SectionPos> = if self.deferred_recheck_needed {
            self.deferred_recheck_needed = false;
            self.deferred_rechecks.clear();
            self.light_deferred.iter().copied().collect()
        } else {
            std::mem::take(&mut self.deferred_rechecks)
                .into_iter()
                .filter(|sp| self.light_deferred.contains(sp))
                .collect()
        };
        if check.is_empty() {
            return;
        }
        self.flush_settled_deferred_positions(target, check);
    }

    #[cfg(test)]
    pub(super) fn flush_settled_deferred(&mut self, target: LoadTarget) {
        let check = self.light_deferred.iter().copied().collect();
        self.flush_settled_deferred_positions(target, check);
    }

    fn flush_settled_deferred_positions(&mut self, target: LoadTarget, check: Vec<SectionPos>) {
        let ready: Vec<SectionPos> = check
            .into_iter()
            .filter(|sp| {
                !self.pending_overlays.contains_key(sp)
                    && self.gen_neighborhood_settled(*sp, target)
            })
            .collect();
        let mut bakes: Vec<SectionPos> = Vec::new();
        for sp in ready {
            let Some(section) = self.sections.get(&sp) else {
                self.light_deferred.remove(&sp);
                continue;
            };
            let needs_bake = section.light_dirty && !section.all_opaque();
            // Keep an enclosed mixed section parked, rather than forgetting its
            // first bake. A target move rechecks the set; once a player is close
            // enough to already be inside, proximity defeats the sealed skip.
            if needs_bake && self.section_sealed_by_loaded_neighbors(sp) {
                continue;
            }
            self.light_deferred.remove(&sp);
            // Clean light (persisted, loaded from disk) stands as-is.
            // Fully-opaque sections skip baking on both sides of the mesh pump's
            // light gate (their faces cull against solid cells and never sample light).
            if needs_bake {
                bakes.push(sp);
            }
            self.queue_dirty_mesh(sp);
        }
        // Streaming first-bakes coalesce into 2×2×2 batch bakes (one shared 64³
        // flood, ~2× less light worker CPU). Below three members the shared
        // 64³ cube costs more cells than separate 48³ floods, so small groups
        // keep the per-section path.
        for (base, members) in crate::world::light::group_positions(&bakes) {
            if members.len() >= 3 {
                let key = members
                    .iter()
                    .map(|&sp| target.section_priority_key(sp))
                    .min()
                    .unwrap_or(0);
                self.light_bakes
                    .request_batch(key, base, &members, &self.sections, &self.columns);
            } else {
                for sp in members {
                    let key = target.section_priority_key(sp);
                    self.light_bakes
                        .request(key, sp, &self.sections, &self.columns);
                }
            }
        }
    }
}
