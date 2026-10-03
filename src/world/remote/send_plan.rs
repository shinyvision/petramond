use crate::world::ServerWorld;
use rustc_hash::FxHashSet;

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

    /// The sections and columns whose sendability may have changed since the last pump.
    pub fn take_send_events(&mut self) -> SendEvents {
        let log = &mut self.side.replication;
        SendEvents {
            sections: std::mem::take(&mut log.send_events),
            columns: std::mem::take(&mut log.column_events),
        }
    }

    /// The target a connection at `anchor` streams against, and whether it is underground
    /// (which flips the ship order from surface-first to pure nearest-first).
    pub(crate) fn send_frame(&self, anchor: LoadAnchor) -> (LoadTarget, bool) {
        let target = self.send_target(anchor);
        (target, self.anchor_underground(target))
    }

    /// The drop rule for a SENT column: it left the keep shape or the server.
    #[inline]
    pub(crate) fn column_drop_due(&self, target: LoadTarget, cp: ChunkPos) -> bool {
        !Self::column_kept(target, cp) || !self.data.columns.contains_key(&cp)
    }

    /// The drop rule for a SENT section whose column stays: it left the keep shape or the
    /// server.
    #[inline]
    pub(crate) fn section_drop_due(&self, target: LoadTarget, sp: SectionPos) -> bool {
        !Self::column_kept(target, sp.chunk_pos()) || !self.data.sections.contains_key(&sp)
    }

    #[inline]
    pub fn plan_epoch(&self) -> u64 {
        self.side.replication.plan_epoch
    }

    /// The band floor of a column's surface shell (what the deep-section deferral and the
    /// surface-biased ship order key on).
    fn column_band_lo(&self, cp: ChunkPos) -> i32 {
        self.side
            .gen
            .column_gen
            .get(&cp)
            .map_or(petramond_world::chunk::SECTION_MIN_CY, |col| {
                *Self::surface_window_for_column(col, 0).start()
            })
    }

    /// THE ship rule, shared by the full plan and the incremental re-evaluation: whether a
    /// LOADED section ships to a connection whose target is `target`, and under what
    /// nearest-first key. Deep sections (below the column's band floor) wait until the
    /// connection's vertical window or 5x5x5 near ring reaches them — the replica's
    /// park-without-mesh rule — unless the open sky sees them; nothing ships before its light
    /// is final.
    pub(crate) fn section_send_key(
        &self,
        target: LoadTarget,
        underground: bool,
        sp: SectionPos,
    ) -> Option<i64> {
        let cp = sp.chunk_pos();
        if !Self::column_wanted(target, cp) {
            return None;
        }
        let band_lo = self.column_band_lo(cp);
        if sp.cy < band_lo && !self.sky_cavern_contains(sp) {
            let near_xz =
                (cp.cx - target.center.cx).abs() <= 2 && (cp.cz - target.center.cz).abs() <= 2;
            let near = near_xz && (sp.cy - target.center_cy).abs() <= 2;
            if !Self::vertical_window(target.center_cy, 0).contains(&sp.cy) && !near {
                return None;
            }
        }
        if !self.data.stream_writable(sp) || !self.section_light_final(sp) {
            return None;
        }
        Some(target.surface_biased_section_key(sp, band_lo, underground))
    }

    fn send_target(&self, anchor: LoadAnchor) -> LoadTarget {
        LoadTarget::new(
            anchor.cx,
            anchor.cy,
            anchor.cz,
            anchor.radius.clamp(1, self.data.render_dist),
        )
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
    /// what the server actually streams for its anchor. This is the FULL diff, O(loaded
    /// columns): a connection runs it when its target moves and otherwise keeps the result
    /// live from send events (`TerrainSync::sync_plan`); it is also the oracle the live plan is
    /// checked against.
    pub fn plan_terrain_send(
        &self,
        anchor: LoadAnchor,
        sent_columns: &FxHashSet<ChunkPos>,
        sent: &SentSections,
        budget: usize,
    ) -> TerrainSendPlan {
        let (target, underground) = self.send_frame(anchor);

        let mut section_keys: Vec<(i64, SectionPos)> = Vec::new();
        for (&cp, &bits) in &self.data.section_column_cys {
            if bits == 0 || !Self::column_wanted(target, cp) {
                continue;
            }
            let mut b = bits & !sent.column_bits(cp);
            while b != 0 {
                let cy = petramond_world::chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                b &= b - 1;
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                if let Some(key) = self.section_send_key(target, underground, sp) {
                    section_keys.push((key, sp));
                }
            }
        }
        section_keys.sort_unstable_by_key(|(key, sp)| (*key, sp.cx, sp.cy, sp.cz));
        section_keys.truncate(budget);
        let sections: Vec<SectionPos> = section_keys.iter().map(|(_, sp)| *sp).collect();

        let drop_columns: Vec<ChunkPos> = sent_columns
            .iter()
            .filter(|cp| self.column_drop_due(target, **cp))
            .copied()
            .collect();
        let dropped_cols: FxHashSet<ChunkPos> = drop_columns.iter().copied().collect();
        let mut drop_sections = Vec::new();
        for (cp, held) in sent.columns() {
            if dropped_cols.contains(&cp) {
                continue;
            }
            for_each_column_cy(held, |cy| {
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                if self.section_drop_due(target, sp) {
                    drop_sections.push(sp);
                }
            });
        }

        TerrainSendPlan {
            sections,
            section_keys,
            drop_sections,
            drop_columns,
        }
    }
}

/// What the world logged since the last streaming pump: sections and columns whose
/// sendability to a connection may have changed.
#[derive(Default)]
pub struct SendEvents {
    pub sections: Vec<SectionPos>,
    pub columns: Vec<ChunkPos>,
}

pub struct TerrainSendPlan {
    pub sections: Vec<SectionPos>,
    /// `sections` with their nearest-first keys, in ship order.
    pub section_keys: Vec<(i64, SectionPos)>,
    pub drop_sections: Vec<SectionPos>,
    pub drop_columns: Vec<ChunkPos>,
}
