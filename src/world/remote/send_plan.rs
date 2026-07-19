use rustc_hash::{FxHashMap, FxHashSet};

use crate::chunk::{ChunkPos, SectionPos, SECTION_MIN_CY};
use crate::world::store::{LoadAnchor, LoadTarget, World};

impl World {
    /// Whether `sp`'s light is presentable: baked (possibly stale — a pending
    /// rebake follows as `LightData`) or fully opaque (never bakes; neighbour
    /// meshes cull against it and sample nothing). The terrain sender holds a
    /// section back until this holds, so every install lands light-complete
    /// and the replica performs NO light work of its own.
    pub(crate) fn section_light_final(&self, sp: SectionPos) -> bool {
        self.sections
            .get(&sp)
            .is_some_and(|s| s.has_baked_light() || s.all_opaque())
    }

    /// Drain the sections whose server bake landed since the last streaming
    /// pump (ServerHeadless fills it in `pump_light_bakes`).
    pub(crate) fn take_light_ship_log(&mut self) -> Vec<SectionPos> {
        self.light_ship_log.drain().collect()
    }

    /// Opaque key over everything the per-connection wanted-vs-sent diff
    /// depends on: the anchor's load target (chunk/section centre and render
    /// distance) and the world's terrain-content revision.
    /// While the key is unchanged, a rescan cannot find new work — the sender
    /// skips it (mirroring how `update_load_target` gates its scans).
    /// The wanted-terrain shape for one connection: its anchor at the
    /// anchor's own radius (the connection's view distance), clamped by this
    /// world's `render_dist` budget.
    fn send_target(&self, anchor: LoadAnchor) -> LoadTarget {
        LoadTarget::new(
            anchor.cx,
            anchor.cy,
            anchor.cz,
            anchor.radius.clamp(1, self.render_dist),
        )
    }

    pub(crate) fn terrain_send_key(&self, anchor: LoadAnchor) -> u64 {
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (self.terrain_target_key(anchor), self.terrain_revision).hash(&mut h);
        h.finish()
    }

    /// Anchor-only part of [`terrain_send_key`](Self::terrain_send_key). A
    /// connection consumes its current plan across content revisions, but an
    /// anchor move invalidates that plan immediately.
    pub(crate) fn terrain_target_key(&self, anchor: LoadAnchor) -> u64 {
        let t = self.send_target(anchor);
        use std::hash::{Hash, Hasher};
        let mut h = rustc_hash::FxHasher::default();
        (t.center.cx, t.center.cz, t.center_cy, t.render_dist).hash(&mut h);
        h.finish()
    }

    /// Diff one connection's WANTED terrain shape against what it was already
    /// sent: which loaded, stream-final sections to ship now (nearest-first,
    /// budgeted), and which sent sections/columns left the keep shape (or the
    /// server) and must unload client-side. Pure planning — the caller owns
    /// the sent sets and the message emission (column before its sections).
    ///
    /// The wanted/keep shapes are exactly the streamer's own
    /// (`column_wanted`/`column_kept` over the anchor's target), so a
    /// client is offered precisely what the server streams for its anchor.
    pub(crate) fn plan_terrain_send(
        &self,
        anchor: LoadAnchor,
        sent_columns: &FxHashSet<ChunkPos>,
        sent_sections: &FxHashSet<SectionPos>,
        // Per-column index over `sent_sections` (the cys sent for each column).
        // Drop planning walks this instead of every sent section.
        sent_by_column: &FxHashMap<ChunkPos, Vec<i32>>,
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
                    .column_gen
                    .get(&cp)
                    .map_or(crate::chunk::SECTION_MIN_CY, |col| {
                        *Self::surface_window_for_column(col, 0).start()
                    })
            })
        };
        // Deep sections (below the column surface band) are deferred until the
        // connection's vertical window or 5×5×5 near ring needs them — the
        // replica would park them without meshing anyway, so shipping early
        // only burns bandwidth + client install. Cave BFS visibility still
        // works once the player approaches (window moves → sections enter
        // the plan). Already-sent deep sections are not yanked here.
        let vwin = Self::vertical_window(target.center_cy, 0);
        let mut sections: Vec<(i64, SectionPos)> = Vec::new();
        for (&cp, &bits) in &self.section_column_cys {
            if bits == 0 || !Self::column_wanted(target, cp) {
                continue;
            }
            let band_lo = band_lo_of(self, cp);
            let near_xz = (cp.cx - target.center.cx).abs() <= 2
                && (cp.cz - target.center.cz).abs() <= 2;
            let mut b = bits;
            while b != 0 {
                let cy = crate::chunk::SECTION_MIN_CY + b.trailing_zeros() as i32;
                b &= b - 1;
                if cy < band_lo {
                    let near = near_xz && (cy - target.center_cy).abs() <= 2;
                    if !vwin.contains(&cy) && !near {
                        continue;
                    }
                }
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                if sent_sections.contains(&sp)
                    || !self.stream_writable(sp)
                    || !self.section_light_final(sp)
                {
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

        // Keep test mirrors `unload_far`'s column hysteresis; a section the
        // server itself evicted (vertical window exit) is gone from `sections`
        // and unloads client-side through the same message.
        let drop_columns: Vec<ChunkPos> = sent_columns
            .iter()
            .filter(|cp| !Self::column_kept(target, **cp) || !self.columns.contains_key(cp))
            .copied()
            .collect();
        let dropped_cols: FxHashSet<ChunkPos> = drop_columns.iter().copied().collect();
        let mut drop_sections = Vec::new();
        for (&cp, cys) in sent_by_column {
            if dropped_cols.contains(&cp) {
                continue;
            }
            let column_gone = !Self::column_kept(target, cp);
            for &cy in cys {
                let sp = SectionPos::new(cp.cx, cy, cp.cz);
                debug_assert!(
                    (SECTION_MIN_CY..=crate::chunk::SECTION_MAX_CY).contains(&cy),
                    "sent_by_column cy out of world range"
                );
                if column_gone || !self.sections.contains_key(&sp) {
                    drop_sections.push(sp);
                }
            }
        }

        TerrainSendPlan {
            sections,
            drop_sections,
            drop_columns,
        }
    }
}

/// Output of [`World::plan_terrain_send`].
pub(crate) struct TerrainSendPlan {
    /// Loaded, stream-final, wanted, unsent sections — nearest-first, budgeted.
    pub(crate) sections: Vec<SectionPos>,
    /// Sent sections that left the keep shape or the server world.
    pub(crate) drop_sections: Vec<SectionPos>,
    /// Sent columns that left the keep shape (their sections drop with them).
    pub(crate) drop_columns: Vec<ChunkPos>,
}
