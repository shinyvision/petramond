use crate::world::store::for_each_column_cy;
use crate::world::ServerWorld;
use petramond_world::chunk::{ChunkPos, SectionPos};

use crate::world::store::LoadTarget;

impl ServerWorld {
    pub(super) fn unload_far_multi(&mut self, targets: &[LoadTarget]) {
        let drop_columns: Vec<ChunkPos> = self
            .data
            .columns
            .keys()
            .filter(|p| !targets.iter().any(|t| Self::column_kept(*t, **p)))
            .copied()
            .collect();
        let mut drop_sections = Vec::new();
        for (&cp, &bits) in &self.data.section_column_cys {
            if !targets.iter().any(|t| Self::column_kept(*t, cp)) {
                continue;
            }
            let mut b = bits;
            while b != 0 {
                let cy = petramond_world::chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                b &= b - 1;
                if targets
                    .iter()
                    .any(|t| Self::vertical_window(t.center_cy, 2).contains(&cy))
                {
                    continue;
                }
                let in_surface = self
                    .side
                    .gen
                    .column_gen
                    .get(&cp)
                    .is_some_and(|col| Self::surface_window_for_column(col, 2).contains(&cy));
                if !in_surface {
                    drop_sections.push(SectionPos::new(cp.cx, cy, cp.cz));
                }
            }
        }
        self.evict_columns_and_sections(drop_columns, drop_sections);
    }

    pub(super) fn unload_far(&mut self, target: LoadTarget, vertical_moved: bool) {
        let vwindow = Self::vertical_window(target.center_cy, 2);

        let drop_columns: Vec<ChunkPos> = self
            .data
            .columns
            .keys()
            .filter(|p| !Self::column_kept(target, **p))
            .copied()
            .collect();
        let drop_sections: Vec<SectionPos> = if vertical_moved {
            let mut out = Vec::new();
            for (&cp, &bits) in &self.data.section_column_cys {
                if !Self::column_kept(target, cp) {
                    continue;
                }
                let mut b = bits;
                while b != 0 {
                    let cy = petramond_world::chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                    b &= b - 1;
                    if vwindow.contains(&cy) {
                        continue;
                    }
                    let in_surface =
                        self.side.gen.column_gen.get(&cp).is_some_and(|col| {
                            Self::surface_window_for_column(col, 2).contains(&cy)
                        });
                    if !in_surface {
                        out.push(SectionPos::new(cp.cx, cy, cp.cz));
                    }
                }
            }
            out
        } else {
            Vec::new()
        };
        self.evict_columns_and_sections(drop_columns, drop_sections);
    }

    fn evict_columns_and_sections(
        &mut self,
        drop_columns: Vec<ChunkPos>,
        drop_sections: Vec<SectionPos>,
    ) {
        self.apply_light_edits();
        if self.side.save.is_some() {
            let mut snaps = Vec::new();
            for &cpos in &drop_columns {
                let bits = self
                    .data
                    .section_column_cys
                    .get(&cpos)
                    .copied()
                    .unwrap_or(0);
                for_each_column_cy(bits, |cy| {
                    if let Some(snap) =
                        self.harvest_section_snapshot(SectionPos::new(cpos.cx, cy, cpos.cz))
                    {
                        snaps.push(snap);
                    }
                });
            }
            for &sp in &drop_sections {
                if let Some(snap) = self.harvest_section_snapshot(sp) {
                    snaps.push(snap);
                }
            }
            if let Some(save) = self.side.save.as_mut() {
                save.save_sections(&mut self.data.saved, snaps);
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
            self.side.gen.pending_overlays.remove(&sp);
            self.settle_stream_nonfinal(sp);
            self.remove_pending_section(sp);
            if let Some(job) = self.side.gen.pending_section_jobs.remove(&sp) {
                job.cancel();
            }
        }
        if dropped_any {
            self.bump_terrain_revision();
        }
    }

    fn drop_overlays_for_column(&mut self, pos: ChunkPos) {
        self.side
            .gen
            .pending_overlays
            .retain(|sp, _| sp.chunk_pos() != pos);
    }
}
