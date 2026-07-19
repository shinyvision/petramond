//! Dirty-mark fan-out: the light and mesh invalidation choke points edits,
//! ingest, and sky-cover moves route through.

use rustc_hash::FxHashSet;

use crate::chunk::{self, ChunkPos, SectionPos, SECTION_MIN_CY, SECTION_SIZE};

use super::store::{SkyCoverChange, World, WorldRole};

impl World {
    pub(super) fn mark_light_dirty_pos(&mut self, pos: SectionPos) {
        if let Some(s) = self.section_mut(pos) {
            s.mark_light_dirty();
            // Headless rebakes are demanded from the mark itself (no mesh
            // pump to demand them). Client/combined section-grained marks
            // (ingest, unload, topology) stay mesh-demanded, so far edges
            // keep their dormant-until-visible behaviour; EDIT marks demand
            // explicitly via `mark_light_dirty_demanded`.
            if self.role == WorldRole::ServerHeadless {
                self.relight_demand.insert(pos);
            }
        }
        if self.light_deferred.contains(&pos) {
            self.deferred_rechecks.insert(pos);
        }
    }

    /// [`mark_light_dirty_pos`](Self::mark_light_dirty_pos) plus a direct
    /// bake demand: edit-driven invalidation must rebake even when no queued
    /// mesh demands the section (a distant sky-cover segment pre-marks no
    /// meshes — the landed bake's diff requeues them if anything changed).
    pub(super) fn mark_light_dirty_demanded(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        if self.sections.contains_key(&pos) {
            self.relight_demand.insert(pos);
        }
    }

    pub(super) fn queue_dirty_mesh(&mut self, pos: SectionPos) {
        // A headless server never meshes: with nobody pumping `tick_mesh_budget`,
        // anything queued here would only accumulate.
        if self.role == WorldRole::ServerHeadless {
            return;
        }
        if let Some(job) = self.mesh_job_cancels.get(&pos) {
            job.cancel();
        }
        if let Some(s) = self.section_mut(pos) {
            s.dirty = true;
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
            self.light_blocked_meshes.remove(&pos);
            self.hidden_parked.remove(&pos);
            self.sealed_parked.remove(&pos);
            self.dirty_meshes.push(pos);
        }
    }

    pub(super) fn mark_light_and_mesh_dirty_pos(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        self.queue_dirty_mesh(pos);
    }

    /// A column sky-cover cell moved, changing which cells are considered open sky
    /// for every skylight bake whose 3×3 XZ seed grid includes this column. Dirty only
    /// sections already in memory; absent generated/sky sections will bake from the new
    /// cover map when they stream in or materialize.
    /// Streaming cover changes arrive in contiguous batches, so each loaded
    /// section is invalidated only once even when several changed columns'
    /// dependent footprints overlap.
    ///
    /// A change whose `from_persist` flag is set was raised purely by PERSISTED
    /// record content landing (disk-primary / overlay, no fresh generation in
    /// the column). Records only persist light captured in a globally settled
    /// state, so every section still holding its untouched persisted bake
    /// (`Section::light_from_persist`) already saw that content — such sections
    /// are spared, which is what keeps a reload of explored terrain bake-free.
    /// Sections baked live this session may have read the pre-landing cover and
    /// are marked regardless.
    pub(super) fn mark_sky_cover_light_dirty_around_many(
        &mut self,
        changes: impl IntoIterator<Item = (ChunkPos, (SkyCoverChange, bool))>,
    ) {
        self.mark_sky_cover_light_dirty_around_impl(changes, false);
    }

    /// Gameplay-edit variant of the sky-cover invalidation, exact to the
    /// changed world column: only sections within light-flood reach of the
    /// flipped direct-sky segment relight. No meshes are pre-marked — a
    /// landed rebake requeues its own mesh and the samplers of whatever
    /// regions actually changed (`pump_light_bakes`), so an envelope section
    /// whose cubes prove untouched publishes nothing. Also records that any
    /// persisted cubes are stale so an eviction racing the rebake rewrites
    /// them lightless.
    pub(super) fn mark_sky_cover_edited_at(&mut self, wx: i32, wz: i32, change: SkyCoverChange) {
        // Exact flood reach (14, see `LIGHT_REACH`); the streaming batch path
        // keeps its column-level `SKY_SEEP_REACH` bound.
        const LIGHT_REACH: i32 = chunk::SKY_FULL as i32 / 2 - 1;
        let center = ChunkPos::new(
            wx.div_euclid(SECTION_SIZE as i32),
            wz.div_euclid(SECTION_SIZE as i32),
        );
        let note_persist = self.save.is_some();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let cp = ChunkPos::new(center.cx + dx, center.cz + dz);
                let bits = self.section_column_cys.get(&cp).copied().unwrap_or(0);
                let mut b = bits;
                while b != 0 {
                    let cy = SECTION_MIN_CY + b.trailing_zeros() as i32;
                    b &= b - 1;
                    let pos = SectionPos::new(cp.cx, cy, cp.cz);
                    if change.segment_gap(pos, wx, wz) > LIGHT_REACH {
                        continue;
                    }
                    if note_persist {
                        self.light_edited_since_persist.insert(pos);
                    }
                    self.mark_light_dirty_demanded(pos);
                }
            }
        }
    }

    fn mark_sky_cover_light_dirty_around_impl(
        &mut self,
        changes: impl IntoIterator<Item = (ChunkPos, (SkyCoverChange, bool))>,
        edited: bool,
    ) {
        let mut affected = Vec::new();
        let mut seen = FxHashSet::default();
        for (center, (change, from_persist)) in changes {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let cp = ChunkPos::new(center.cx + dx, center.cz + dz);
                    let bits = self.section_column_cys.get(&cp).copied().unwrap_or(0);
                    let mut b = bits;
                    while b != 0 {
                        let cy = SECTION_MIN_CY + b.trailing_zeros() as i32;
                        b &= b - 1;
                        let pos = SectionPos::new(cp.cx, cy, cp.cz);
                        if change.affects(pos)
                            && self
                                .sections
                                .get(&pos)
                                .is_some_and(|s| !(from_persist && s.light_from_persist))
                            && seen.insert(pos)
                        {
                            affected.push(pos);
                        }
                    }
                }
            }
        }
        for pos in affected {
            if edited && self.save.is_some() {
                self.light_edited_since_persist.insert(pos);
            }
            self.mark_light_and_mesh_dirty_pos(pos);
        }
    }

    /// Mark the 3×3×3 section neighbourhood around `center` dirty for remesh, so
    /// border face-culling / AO / light sampling stay correct across section seams.
    pub(super) fn mark_dirty_neighborhood(&mut self, center: SectionPos, include_center: bool) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if !include_center && dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    self.queue_dirty_mesh(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
        }
    }

    /// L1 distance from a section-local cell to the nearest cell of the
    /// neighbouring section at axis delta `d` (0 within this section).
    #[inline]
    pub(super) fn axis_gap(local: usize, d: i32) -> i32 {
        match d {
            -1 => local as i32 + 1,
            1 => SECTION_SIZE as i32 - local as i32,
            _ => 0,
        }
    }

    /// The exact flood reach of one changed cell, in cells: the flood loses 2
    /// per step from at most `SKY_FULL` (30), so no single-cell change
    /// survives past L1 distance 14.
    pub(super) const LIGHT_REACH: i32 = chunk::SKY_FULL as i32 / 2 - 1;

    /// Mark light dirty for exactly the sections one changed cell can
    /// influence within `radius` (at most
    /// [`LIGHT_REACH`](Self::LIGHT_REACH), so a mid-section edit invalidates
    /// 7 sections, not 27; smaller when the caller bounded the reach by the
    /// light actually present at the cell — an edit in the dark cannot
    /// brighten or darken anything far away). Direct-sky cover moves reach
    /// farther vertically and are invalidated separately via
    /// [`SkyCoverChange`] segments.
    pub(super) fn mark_light_dirty_around_cell_radius(
        &mut self,
        wx: i32,
        wy: i32,
        wz: i32,
        radius: i32,
    ) {
        let Some((center, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return;
        };
        let note_persist = self.save.is_some();
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    let gap =
                        Self::axis_gap(lx, dx) + Self::axis_gap(ly, dy) + Self::axis_gap(lz, dz);
                    if gap > radius {
                        continue;
                    }
                    let pos = SectionPos::new(center.cx + dx, center.cy + dy, center.cz + dz);
                    self.mark_light_dirty_demanded(pos);
                    if note_persist && self.sections.contains_key(&pos) {
                        self.light_edited_since_persist.insert(pos);
                    }
                }
            }
        }
    }

    /// Queue a remesh of every section whose mesh samples world cell
    /// `(wx, wy, wz)`: the owning section plus the bordering neighbours whose
    /// one-cell 18³ pad (face culling, AO, smooth light) includes it — up to 8
    /// sections for a corner cell, 1 for an interior cell. Sections whose
    /// *light* the edit changes are requeued when their rebake lands (see
    /// `pump_light_bakes`), so they need no blanket pre-mark here.
    pub(super) fn queue_dirty_meshes_sampling_cell(&mut self, wx: i32, wy: i32, wz: i32) {
        #[inline]
        fn deltas(local: usize) -> &'static [i32] {
            if local == 0 {
                &[0, -1]
            } else if local == SECTION_SIZE - 1 {
                &[0, 1]
            } else {
                &[0]
            }
        }
        let Some((center, lx, ly, lz)) = Self::split_world(wx, wy, wz) else {
            return;
        };
        for &dy in deltas(ly) {
            for &dz in deltas(lz) {
                for &dx in deltas(lx) {
                    self.queue_dirty_mesh(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
        }
    }

    pub(super) fn mark_light_dirty_neighborhood(
        &mut self,
        center: SectionPos,
        include_center: bool,
    ) {
        for dy in -1..=1 {
            for dz in -1..=1 {
                for dx in -1..=1 {
                    if !include_center && dx == 0 && dy == 0 && dz == 0 {
                        continue;
                    }
                    self.mark_light_dirty_pos(SectionPos::new(
                        center.cx + dx,
                        center.cy + dy,
                        center.cz + dz,
                    ));
                }
            }
        }
    }
}
