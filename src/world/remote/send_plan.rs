use crate::world::ServerWorld;
use rustc_hash::{FxHashMap, FxHashSet};

use crate::world::store::{for_each_column_cy, LoadAnchor, LoadTarget};
use crate::world::SentSections;
use petramond_world::chunk::{ChunkPos, SectionPos};

impl ServerWorld {
    pub fn section_light_final(&self, sp: SectionPos) -> bool {
        self.data
            .sections
            .get(&sp)
            .is_some_and(|s| s.has_baked_light() || s.all_opaque())
    }

    pub fn take_light_ship_log(&mut self) -> Vec<SectionPos> {
        self.side.replication.light_ship_log.drain().collect()
    }

    fn send_target(&self, anchor: LoadAnchor) -> LoadTarget {
        LoadTarget::new(
            anchor.cx,
            anchor.cy,
            anchor.cz,
            anchor.radius.clamp(1, self.data.render_dist),
        )
    }

    pub fn terrain_send_key(&self, anchor: LoadAnchor) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (
            self.terrain_target_key(anchor),
            self.side.replication.terrain_revision,
        )
            .hash(&mut h);
        h.finish()
    }

    pub fn terrain_target_key(&self, anchor: LoadAnchor) -> u64 {
        let t = self.send_target(anchor);
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (t.center.cx, t.center.cz, t.center_cy, t.render_dist).hash(&mut h);
        h.finish()
    }

    /// Diffs wanted terrain shape against what's already sent: ship nearest-first up to budget,
    /// unload what left the keep shape or server. Just planning. The caller owns the sent sets and
    /// emits messages, column before sections.
    ///
    /// Wanted/keep shapes match the streamer's `column_wanted`/`column_kept`, so the client gets
    /// what the server actually streams for its anchor.
    pub fn plan_terrain_send(
        &self,
        anchor: LoadAnchor,
        sent_columns: &FxHashSet<ChunkPos>,
        sent: &SentSections,
        budget: usize,
    ) -> TerrainSendPlan {
        let target = self.send_target(anchor);
        let underground = self.anchor_underground(target);

        // Ship order mirrors the streamer's gen order: surface shell first for
        // an above-ground anchor, pure 3D nearest-first for a caving one. The
        // band floor is per column; memoize the lookup across the scan.
        // Scan wanted columns × vertical stack instead of every loaded section:
        // the keep-hysteresis ring and far unloaded-but-still-resident columns
        // never enter the unsent candidate list.
        let mut band_los: FxHashMap<ChunkPos, i32> = FxHashMap::default();
        let mut band_lo_of = |world: &Self, cp: ChunkPos| {
            *band_los.entry(cp).or_insert_with(|| {
                world
                    .side
                    .gen
                    .column_gen
                    .get(&cp)
                    .map_or(petramond_world::chunk::SECTION_MIN_CY, |col| {
                        *Self::surface_window_for_column(col, 0).start()
                    })
            })
        };
        let vwin = Self::vertical_window(target.center_cy, 0);
        let mut sections: Vec<(i64, SectionPos)> = Vec::new();
        for (&cp, &bits) in &self.data.section_column_cys {
            if bits == 0 || !Self::column_wanted(target, cp) {
                continue;
            }
            let band_lo = band_lo_of(self, cp);
            let near_xz =
                (cp.cx - target.center.cx).abs() <= 2 && (cp.cz - target.center.cz).abs() <= 2;
            let mut b = bits & !sent.column_bits(cp);
            while b != 0 {
                let cy = petramond_world::chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                b &= b - 1;
                if cy < band_lo {
                    let near = near_xz && (cy - target.center_cy).abs() <= 2;
                    if !vwin.contains(&cy) && !near {
                        continue;
                    }
                }
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                if !self.data.stream_writable(sp) || !self.section_light_final(sp) {
                    continue;
                }
                sections.push((
                    target.surface_biased_section_key(sp, band_lo, underground),
                    sp,
                ));
            }
        }
        sections.sort_unstable_by_key(|(key, _)| *key);
        sections.truncate(budget);
        let sections: Vec<SectionPos> = sections.into_iter().map(|(_, sp)| sp).collect();

        let drop_columns: Vec<ChunkPos> = sent_columns
            .iter()
            .filter(|cp| !Self::column_kept(target, **cp) || !self.data.columns.contains_key(cp))
            .copied()
            .collect();
        let dropped_cols: FxHashSet<ChunkPos> = drop_columns.iter().copied().collect();
        let mut drop_sections = Vec::new();
        for (cp, held) in sent.columns() {
            if dropped_cols.contains(&cp) {
                continue;
            }
            let gone = if Self::column_kept(target, cp) {
                held & !self.data.section_column_cys.get(&cp).copied().unwrap_or(0)
            } else {
                held
            };
            for_each_column_cy(gone, |cy| {
                drop_sections.push(SectionPos::new(cp.cx, cy, cp.cz))
            });
        }

        TerrainSendPlan {
            sections,
            drop_sections,
            drop_columns,
        }
    }
}

pub struct TerrainSendPlan {
    pub sections: Vec<SectionPos>,
    pub drop_sections: Vec<SectionPos>,
    pub drop_columns: Vec<ChunkPos>,
}
