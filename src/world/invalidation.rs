//! Dirty-mark fan-out: the light and mesh invalidation choke points edits,
//! ingest, and sky-cover moves route through.

use crate::world::{ServerWorld, World, WorldSide};
use crate::world::WorldData;
use rustc_hash::FxHashSet;

use petramond_world::chunk::{self, ChunkPos, SectionPos, SECTION_MIN_CY, SECTION_SIZE};

use super::store::SkyCoverChange;

impl<S: WorldSide> World<S> {
    pub(super) fn mark_light_dirty_pos(&mut self, pos: SectionPos) {
        if let Some(s) = self.data.section_mut(pos) {
            s.mark_light_dirty();
            // Without a mesh pump (the server), rebakes are demanded from the
            // mark itself. A replica's section-grained marks (ingest, unload,
            // topology) stay mesh-demanded, so far edges keep their
            // dormant-until-visible behaviour; EDIT marks demand explicitly
            // via `mark_light_dirty_demanded`.
            if self.side.replica().is_none() {
                self.data.relight_demand.insert(pos);
            }
        }
        if self.data.light_deferred.contains(&pos) {
            self.data.deferred_rechecks.insert(pos);
        }
    }

    /// [`mark_light_dirty_pos`](Self::mark_light_dirty_pos) plus a direct
    /// bake demand: edit-driven invalidation must rebake even when no queued
    /// mesh demands the section (a distant sky-cover segment pre-marks no
    /// meshes — the landed bake's diff requeues them if anything changed).
    pub(super) fn mark_light_dirty_demanded(&mut self, pos: SectionPos) {
        self.mark_light_dirty_pos(pos);
        if self.data.sections.contains_key(&pos) {
            self.data.relight_demand.insert(pos);
        }
    }

    pub(super) fn queue_dirty_mesh(&mut self, pos: SectionPos) {
        // The server never meshes: it has no presentation to queue into.
        let Some(replica) = self.side.replica_mut() else {
            return;
        };
        let terrain = &mut replica.terrain;
        if let Some(job) = terrain.mesh_job_cancels.get(&pos) {
            job.cancel();
        }
        if let Some(s) = self.data.section_mut(pos) {
            s.dirty = true;
            s.mesh_revision = s.mesh_revision.wrapping_add(1);
            terrain.light_blocked_meshes.remove(&pos);
            terrain.hidden_parked.remove(&pos);
            terrain.sealed_parked.remove(&pos);
            terrain.dirty_meshes.push(pos);
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
        let note_persist = self.persisting();
        for dz in -1..=1 {
            for dx in -1..=1 {
                let cp = ChunkPos::new(center.cx + dx, center.cz + dz);
                let bits = self.data.section_column_cys.get(&cp).copied().unwrap_or(0);
                let mut b = bits;
                while b != 0 {
                    let cy = SECTION_MIN_CY + b.trailing_zeros() as i32;
                    b &= b - 1;
                    let pos = SectionPos::new(cp.cx, cy, cp.cz);
                    if change.segment_gap(pos, wx, wz) > LIGHT_REACH {
                        continue;
                    }
                    if note_persist {
                        self.data.light_edited_since_persist.insert(pos);
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
                    let bits = self.data.section_column_cys.get(&cp).copied().unwrap_or(0);
                    let mut b = bits;
                    while b != 0 {
                        let cy = SECTION_MIN_CY + b.trailing_zeros() as i32;
                        b &= b - 1;
                        let pos = SectionPos::new(cp.cx, cy, cp.cz);
                        if change.affects(pos)
                            && self
                                .data.sections
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
            if edited && self.persisting() {
                self.data.light_edited_since_persist.insert(pos);
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
        let Some((center, lx, ly, lz)) = WorldData::split_world(wx, wy, wz) else {
            return;
        };
        let note_persist = self.persisting();
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
                    if note_persist && self.data.sections.contains_key(&pos) {
                        self.data.light_edited_since_persist.insert(pos);
                    }
                }
            }
        }
    }

    /// Route one changed cell's relight. An authoritative world queues it for
    /// the incremental relight (`apply_light_edits`) when its region's stored
    /// light can be trusted, and otherwise marks the full rebake at once
    /// (`radius` as [`mark_light_dirty_around_cell_radius`]). The replica
    /// always marks: its light is server-owned, and its predicted edits
    /// relight through the prediction bundle's own bakes.
    ///
    /// A negative `radius` means the caller proved the change light-neutral
    /// — possibly by reading light that queued edits have not updated yet
    /// ("dark here" may be stale). With edits pending, the cell is queued
    /// anyway so the batch seeds it; on its own it needs nothing.
    ///
    /// [`mark_light_dirty_around_cell_radius`]: Self::mark_light_dirty_around_cell_radius
    pub(super) fn relight_cell(&mut self, wx: i32, wy: i32, wz: i32, radius: i32) {
        if self.side.server().is_none() {
            if radius >= 0 {
                self.mark_light_dirty_around_cell_radius(wx, wy, wz, radius);
            }
            return;
        }
        if radius < 0 && self.data.light_edits.is_empty() {
            return;
        }
        let cell = petramond_math::math::IVec3::new(wx, wy, wz);
        if radius < 0 || petramond_world::world::light::edit_relightable(&self.data.sections, cell) {
            self.data.light_edits.push((cell, radius));
        } else {
            self.mark_light_dirty_around_cell_radius(wx, wy, wz, radius);
        }
    }

    /// Queue one changed cell for the incremental relight when this world can
    /// take it there (authoritative, region trusted). Returns `false`
    /// otherwise, leaving the caller's own invalidation in charge — which must
    /// dirty the cell's section, so no pending batch whose region holds the
    /// cell can relight around it unseeded.
    pub(super) fn queue_incremental_relight(&mut self, cell: petramond_math::math::IVec3) -> bool {
        if self.side.server().is_none()
            || !petramond_world::world::light::edit_relightable(&self.data.sections, cell)
        {
            return false;
        }
        self.data.light_edits.push((cell, Self::LIGHT_REACH));
        true
    }

    /// Queue a remesh of every section whose mesh samples world cell
    /// `(wx, wy, wz)`: the owning section plus every bordering neighbour whose
    /// sampling halo (`petramond_mesh::SAMPLING_HALO`: the one-cell pad for
    /// culling, AO and smooth light, plus the snow-cover and bedding lookups
    /// transition donors make past it) includes the cell — up to 8 sections
    /// for a corner cell, 1 for an interior cell. Sections whose *light* the
    /// edit changes are requeued when their rebake lands (see
    /// `pump_light_bakes`), so they need no blanket pre-mark here.
    pub(super) fn queue_dirty_meshes_sampling_cell(&mut self, wx: i32, wy: i32, wz: i32) {
        /// The neighbour offsets along one axis whose halo reaches `local`:
        /// the low neighbour samples `below` cells up into this section, the
        /// high neighbour samples `above` cells down into it.
        #[inline]
        fn deltas(local: usize, below: usize, above: usize) -> &'static [i32] {
            if local < above {
                &[0, -1]
            } else if local >= SECTION_SIZE - below {
                &[0, 1]
            } else {
                &[0]
            }
        }
        let Some((center, lx, ly, lz)) = WorldData::split_world(wx, wy, wz) else {
            return;
        };
        let halo = &petramond_mesh::SAMPLING_HALO;
        for &dy in deltas(ly, halo.down, halo.up) {
            for &dz in deltas(lz, halo.horizontal, halo.horizontal) {
                for &dx in deltas(lx, halo.horizontal, halo.horizontal) {
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

impl ServerWorld {
    /// Drain the queued incremental relights (only the server queues them —
    /// see [`relight_cell`](World::relight_cell)): one BFS pass over the stored
    /// cubes for the whole batch, installing the sections whose light moved.
    /// When any queued cell's region cannot be trusted any more (a neighbour
    /// evicted or dirtied since it was queued), EVERY queued cell falls back
    /// to its full-rebake mark — a partial batch would relight against cells
    /// whose own change it never seeded.
    pub(in crate::world) fn apply_light_edits(&mut self) {
        if self.data.light_edits.is_empty() {
            return;
        }
        let edits = std::mem::take(&mut self.data.light_edits);
        let cells: Vec<_> = edits.iter().map(|&(cell, _)| cell).collect();
        match petramond_world::world::light::relight_edits(
            &self.data.sections,
            &self.data.columns,
            &cells,
        ) {
            Some(relit) => {
                for r in relit {
                    self.install_light_cubes(r.pos, r.skylight, r.blocklight);
                    // Changed light is new shippable content, exactly like a
                    // landed bake (see `pump_light_bakes`).
                    self.side.replication.light_ship_log.insert(r.pos);
                    self.side.replication.bump_terrain_revision();
                }
            }
            None => {
                for (cell, radius) in edits {
                    if radius >= 0 {
                        self.mark_light_dirty_around_cell_radius(cell.x, cell.y, cell.z, radius);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use petramond_world::block::Block;
    use petramond_world::section::Section;
    use petramond_world::world::light::{bake_section, SectionBakeJob};

    /// A settled 3×3×3 block of sections: a solid stone floor layer (fully
    /// opaque, so it never bakes), an air layer under a one-cell stone roof at
    /// y = 15, and open sky above — every bakeable section's light landed.
    fn settled_room() -> ServerWorld {
        let mut w = ServerWorld::new(0, 4);
        for cy in -1..=1 {
            for cz in -1..=1 {
                for cx in -1..=1 {
                    let mut s = Section::new(cx, cy, cz);
                    if cy == -1 {
                        s.blocks_mut().fill(Block::Stone.id());
                        s.recompute_opaque_count();
                    } else if cy == 0 {
                        for z in 0..SECTION_SIZE {
                            for x in 0..SECTION_SIZE {
                                s.set_block(x, SECTION_SIZE - 1, z, Block::Stone);
                            }
                        }
                    }
                    w.insert_section_for_test(SectionPos::new(cx, cy, cz), s);
                }
            }
        }
        for column in w.data.columns.values_mut() {
            for z in 0..SECTION_SIZE {
                for x in 0..SECTION_SIZE {
                    column.set_surface_y(x, z, SECTION_SIZE as i32 - 1);
                    column.set_sky_cover_y(x, z, SECTION_SIZE as i32 - 1);
                }
            }
        }
        for _ in 0..2500 {
            w.pump_light_bakes();
            if w.data.sections.values().all(|s| !s.light_dirty || s.all_opaque()) {
                return w;
            }
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
        panic!("fixture light never settled");
    }

    /// An edit whose whole region holds settled light relights incrementally:
    /// nothing goes light-dirty, no bake is requested, and the next pump
    /// installs cubes equal to a fresh full rebake of every section.
    #[test]
    fn a_settled_edit_relights_incrementally_and_exactly() {
        let mut w = settled_room();
        let torch = petramond_math::math::IVec3::new(8, 4, 8);
        assert!(w.set_block_world(torch.x, torch.y, torch.z, Block::Torch));

        assert!(
            w.data.sections.values().all(|s| !s.light_dirty || s.all_opaque()),
            "an incremental edit marks nothing for a full rebake"
        );
        assert!(w.data.relight_demand.is_empty());
        assert_eq!(w.data.light_edits.len(), 1, "the edit waits for the next drain");

        w.pump_light_bakes();
        assert!(w.data.light_edits.is_empty());
        assert!(!w.data.blocklight_rgb_at_world(torch.x + 1, torch.y, torch.z).is_dark());
        for (&pos, section) in w.data.sections.iter() {
            if section.all_opaque() {
                continue;
            }
            let want = bake_section(
                SectionBakeJob::snapshot_unchecked(pos, &w.data.sections, &w.data.columns)
                    .expect("loaded"),
            );
            assert_eq!(section.skylight_arc().as_deref(), Some(&want.skylight[..]), "{pos:?}");
            let block = section.blocklight_arc();
            let block = block.as_deref().unwrap_or(&petramond_world::world::light::ZERO_CUBE[..]);
            assert_eq!(block, &want.blocklight[..], "{pos:?}");
        }
    }

    /// The same edit beside an absent section cannot trust its region: it
    /// marks the full rebake on the spot, exactly as before incremental light.
    #[test]
    fn an_edit_beside_an_absent_section_marks_the_full_rebake() {
        let mut w = settled_room();
        w.data.sections.remove(&SectionPos::new(1, 0, 0));
        let torch = petramond_math::math::IVec3::new(12, 4, 8);
        assert!(w.set_block_world(torch.x, torch.y, torch.z, Block::Torch));
        assert!(w.data.light_edits.is_empty());
        assert!(w.data.sections[&SectionPos::new(0, 0, 0)].light_dirty);
        assert!(w.data.relight_demand.contains(&SectionPos::new(0, 0, 0)));
    }
}
