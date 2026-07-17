use crate::chunk::{ChunkPos, SectionPos};

use crate::world::store::{LoadTarget, World};

impl World {
    /// [`unload_far`] keeping the UNION of the anchors' keep shapes: a column
    /// (or kept column's section) survives if any anchor still wants it, with
    /// the same hysteresis slack as the single-anchor path.
    pub(super) fn unload_far_multi(&mut self, targets: &[LoadTarget]) {
        let drop_columns: Vec<ChunkPos> = self
            .columns
            .keys()
            .filter(|p| !targets.iter().any(|t| Self::column_kept(*t, **p)))
            .copied()
            .collect();
        let drop_sections: Vec<SectionPos> =
            self.sections
                .keys()
                .filter(|sp| {
                    if targets
                        .iter()
                        .any(|t| Self::vertical_window(t.center_cy, 2).contains(&sp.cy))
                    {
                        return false;
                    }
                    let cp = sp.chunk_pos();
                    targets.iter().any(|t| Self::column_kept(*t, cp))
                        && !self.column_gen.get(&cp).is_some_and(|col| {
                            Self::surface_window_for_column(col, 2).contains(&sp.cy)
                        })
                })
                .copied()
                .collect();
        self.evict_columns_and_sections(drop_columns, drop_sections);
    }

    /// Evict everything no longer wanted: columns that left the horizontal radius (whole
    /// column), and sections of kept columns that left the vertical window. Modified /
    /// entity-bearing sections are harvested + persisted first (same gate as autosave).
    pub(super) fn unload_far(&mut self, target: LoadTarget, vertical_moved: bool) {
        let vwindow = Self::vertical_window(target.center_cy, 2);

        let drop_columns: Vec<ChunkPos> = self
            .columns
            .keys()
            .filter(|p| !Self::column_kept(target, **p))
            .copied()
            .collect();
        let drop_sections: Vec<SectionPos> = if vertical_moved {
            self.sections
                .keys()
                .filter(|sp| {
                    // Cheapest rejection first: almost every section is still inside
                    // the player window, so answer that with two integer compares
                    // before the column-shape test and the per-column surface band.
                    if vwindow.contains(&sp.cy) {
                        return false;
                    }
                    let cp = sp.chunk_pos();
                    Self::column_kept(target, cp)
                        && !self.column_gen.get(&cp).is_some_and(|col| {
                            Self::surface_window_for_column(col, 2).contains(&sp.cy)
                        })
                })
                .copied()
                .collect()
        } else {
            Vec::new()
        };
        self.evict_columns_and_sections(drop_columns, drop_sections);
    }

    /// The persist-then-drop tail of unloading: harvest entities + persist
    /// modified sections (same gate as autosave), then evict. Shared by the
    /// single- and multi-anchor unload selections.
    fn evict_columns_and_sections(
        &mut self,
        drop_columns: Vec<ChunkPos>,
        drop_sections: Vec<SectionPos>,
    ) {
        // Persist (harvesting entities into the record) before anything leaves memory.
        if self.save.is_some() {
            let mut snaps = Vec::new();
            for &cpos in &drop_columns {
                for cy in Self::column_section_range() {
                    if let Some(snap) =
                        self.harvest_section_snapshot(SectionPos::new(cpos.cx, cy, cpos.cz))
                    {
                        snaps.push(snap);
                    }
                }
            }
            for &sp in &drop_sections {
                if let Some(snap) = self.harvest_section_snapshot(sp) {
                    snaps.push(snap);
                }
            }
            if let Some(save) = self.save.as_mut() {
                save.save_sections(snaps);
            }
            self.flush_pending_colgen_records();
        }

        let dropped_any = !drop_columns.is_empty() || !drop_sections.is_empty();
        for pos in drop_columns {
            self.remove_column(pos);
            self.drop_overlays_for_column(pos);
        }
        for sp in drop_sections {
            self.remove_section(sp);
            self.pending_overlays.remove(&sp);
            self.pending_sections.remove(&sp);
            if let Some(job) = self.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        if dropped_any {
            self.bump_terrain_revision();
        }
    }

    /// Drop any buffered disk overlays for a column that is no longer wanted, so a
    /// section whose column was evicted before its overlay could land doesn't linger.
    fn drop_overlays_for_column(&mut self, pos: ChunkPos) {
        self.pending_overlays.retain(|sp, _| sp.chunk_pos() != pos);
    }
}
