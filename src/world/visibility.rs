//! Deep-section visibility: skip meshing (and thereby lighting) for below-surface
//! sections no sightline can reach.
//!
//! Sections in or above their column's surface retention band always mesh — they can
//! face the open sky. Sections wholly BELOW that band ("deep") are only visible
//! through cave openings, so they mesh only when a breadth-first search from the
//! visible region reaches them:
//!
//! - Seeds: a deep section bordering a LOADED non-deep section whose facing plane is
//!   open, and every deep section in the player's 3×3×3 ring (the player may be
//!   mining inside sealed rock). Absent neighbours count as closed: below the window
//!   floor or outside the disc there is nothing to look from, and a still-pending
//!   neighbour re-raises `vis_dirty` when it lands.
//! - A reached section's interior is treated as fully connected (a conservative
//!   over-approximation: it can only over-mesh, never hide something visible), so
//!   sight exits through any open plane of a reached section.
//! - Crossing a seam into a further deep section marks that section visible (its
//!   boundary faces are the cave walls seen from this side) and traverses into it
//!   only if its own facing plane is open too.
//!
//! Hidden deep sections park in `hidden_parked` (out of the hot dirty queue, like
//! `light_blocked_meshes`). Because light bakes are only ever requested as mesh
//! dependencies, parking the mesh parks the light for free. Any edit re-dirties the
//! 3×3×3 AND flags `vis_dirty`, and every refresh re-queues parked sections that
//! became visible, so re-exposure needs no extra bookkeeping.

use std::collections::VecDeque;

use crate::chunk::SectionPos;

use super::store::World;

const FACES: [(i32, i32, i32); 6] = [
    (1, 0, 0),
    (-1, 0, 0),
    (0, 1, 0),
    (0, -1, 0),
    (0, 0, 1),
    (0, 0, -1),
];

impl World {
    /// Classify a freshly-installed section. Deep = wholly below the column's
    /// surface retention band (it cannot see the sky from any loaded position).
    /// Sections without column data stay non-deep — non-deep always meshes, so
    /// misclassification can only cost work, never visibility.
    pub(super) fn classify_deep_on_install(&mut self, pos: SectionPos) {
        let Some(col) = self.column_gen.get(&pos.chunk_pos()) else {
            return;
        };
        let band_lo = *Self::surface_window_for_column(col, 0).start();
        if pos.cy < band_lo {
            self.deep_sections.insert(pos);
        }
        self.vis_dirty = true;
    }

    /// Whether `pos` is inside the player's 3×3×3 section ring — always meshed, so
    /// the view is never missing walls while the visibility refresh lags a pump.
    pub(super) fn near_load_center(&self, pos: SectionPos) -> bool {
        let Some(t) = self.last_load_target else {
            return true;
        };
        (pos.cx - t.center.cx).abs() <= 1
            && (pos.cy - t.center_cy).abs() <= 1
            && (pos.cz - t.center.cz).abs() <= 1
    }

    /// Whether the mesh pump should park `pos` instead of meshing it.
    pub(super) fn section_hidden(&self, pos: SectionPos) -> bool {
        self.deep_sections.contains(&pos)
            && !self.visible_deep.contains(&pos)
            && !self.near_load_center(pos)
    }

    /// Recompute the visible-deep set and re-queue parked sections that became
    /// visible. Runs on the mesh pump whenever `vis_dirty` was raised (ingest,
    /// edit, crossing); cost is O(deep sections) hash probes plus the BFS over
    /// actually-reachable cave sections, so it is bounded and main-thread safe.
    pub(super) fn refresh_deep_visibility(&mut self) {
        self.vis_dirty = false;

        let mut visible: std::collections::HashSet<SectionPos> = std::collections::HashSet::new();
        let mut queue: VecDeque<SectionPos> = VecDeque::new();
        let mut entered: std::collections::HashSet<SectionPos> = std::collections::HashSet::new();

        // Seeds: deep sections bordering the visible region (non-deep or absent
        // positions), plus the player ring.
        for &pos in &self.deep_sections {
            let Some(s) = self.sections.get(&pos) else {
                continue;
            };
            if self.near_load_center(pos) {
                visible.insert(pos);
                if entered.insert(pos) {
                    queue.push_back(pos);
                }
                continue;
            }
            for &(dx, dy, dz) in &FACES {
                let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if self.deep_sections.contains(&n) {
                    continue;
                }
                // The neighbour's air region is visible by definition (non-deep),
                // and sight against this section's wall exists if the neighbour
                // side of the seam is open. Absent neighbours count as CLOSED:
                // below the window floor / outside the disc there is nothing to
                // look from, and a still-pending neighbour re-raises `vis_dirty`
                // the moment it lands, re-running this refresh.
                let n_side_open = self
                    .sections
                    .get(&n)
                    .is_some_and(|ns| ns.face_plane_open(-dx, -dy, -dz));
                if !n_side_open {
                    continue;
                }
                visible.insert(pos);
                // Sight passes INTO this section only through its own open plane.
                if s.face_plane_open(dx, dy, dz) && entered.insert(pos) {
                    queue.push_back(pos);
                }
            }
        }

        // Traverse the cave interior: sight inside an entered section exits through
        // any of its open planes, exposing the neighbouring deep section's walls and
        // continuing wherever that neighbour's facing plane is open too.
        while let Some(pos) = queue.pop_front() {
            let Some(s) = self.sections.get(&pos) else {
                continue;
            };
            for &(dx, dy, dz) in &FACES {
                if !s.face_plane_open(dx, dy, dz) {
                    continue;
                }
                let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if !self.deep_sections.contains(&n) {
                    continue;
                }
                visible.insert(n);
                let Some(ns) = self.sections.get(&n) else {
                    continue;
                };
                if ns.face_plane_open(-dx, -dy, -dz) && entered.insert(n) {
                    queue.push_back(n);
                }
            }
        }

        // Re-queue parked sections that just became visible (or entered the ring).
        let unpark: Vec<SectionPos> = self
            .hidden_parked
            .iter()
            .filter(|p| visible.contains(p) || self.near_load_center(**p))
            .copied()
            .collect();
        for pos in unpark {
            self.hidden_parked.remove(&pos);
            self.dirty_meshes.push(pos);
        }

        self.visible_deep = visible;
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::block::Block;
    use crate::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
    use crate::section::Section;
    use crate::world::store::LoadTarget;
    use crate::worldgen::driver::ChunkGenerator;

    use super::World;

    fn solid_section(pos: SectionPos) -> Section {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_slice_mut().fill(Block::Stone.id());
        section.recompute_random_tick_count();
        section.recompute_opaque_count();
        section
    }

    fn install(world: &mut World, section: Section) {
        let pos = SectionPos::new(section.cx, section.cy, section.cz);
        world.ensure_column(pos.chunk_pos());
        world.sections.insert(pos, Arc::new(section));
        world.classify_deep_on_install(pos);
        world.queue_dirty_mesh(pos);
    }

    fn pump(world: &mut World) {
        for _ in 0..200 {
            world.tick_mesh_budget(8);
            if !world.has_dirty_meshes() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(2));
        }
    }

    #[test]
    fn hidden_cave_parks_unmeshed_and_opens_when_dug_into() {
        let mut world = World::new(1, 4);
        let generator = ChunkGenerator::new(1);
        let cpos = ChunkPos::new(0, 0);
        world.ensure_column(cpos);
        world
            .column_gen
            .insert(cpos, Arc::new(generator.generate_column_gen(0, 0)));

        let band_lo = *World::surface_window_for_column(&world.column_gen[&cpos], 0).start();
        // Keep the player ring far above the cave.
        world.last_load_target = Some(LoadTarget::new(0, band_lo + 5, 0, 4));

        // A solid "surface" section at the band floor over two deep sections that
        // share an internal air shaft — a cave sealed from the visible region.
        let surface_pos = SectionPos::new(0, band_lo, 0);
        let deep_hi = SectionPos::new(0, band_lo - 1, 0);
        let deep_lo = SectionPos::new(0, band_lo - 2, 0);
        install(&mut world, solid_section(surface_pos));
        for pos in [deep_hi, deep_lo] {
            let mut s = solid_section(pos);
            for y in 0..SECTION_SIZE {
                s.set_block(8, y, 8, Block::Air);
            }
            install(&mut world, s);
        }

        pump(&mut world);
        assert!(
            world.iter_meshes().any(|(p, _)| p == surface_pos),
            "the band-floor section is always visible and must mesh"
        );
        for pos in [deep_hi, deep_lo] {
            assert!(
                world.iter_meshes().all(|(p, _)| p != pos),
                "a sealed deep cave section must not mesh"
            );
            assert!(
                world.hidden_parked.contains(&pos),
                "a sealed deep cave section parks for later re-exposure"
            );
        }

        // Dig through the band floor into the shaft: the cave becomes reachable
        // and both cave sections must come back and mesh.
        let wy = band_lo * SECTION_SIZE as i32;
        assert!(world.set_block_world(8, wy, 8, Block::Air));
        pump(&mut world);
        for pos in [deep_hi, deep_lo] {
            assert!(
                world.iter_meshes().any(|(p, _)| p == pos),
                "digging in must re-expose and mesh the cave section {pos:?}"
            );
        }
    }

    #[test]
    fn player_ring_overrides_hidden_parking() {
        let mut world = World::new(1, 4);
        let generator = ChunkGenerator::new(1);
        let cpos = ChunkPos::new(0, 0);
        world.ensure_column(cpos);
        world
            .column_gen
            .insert(cpos, Arc::new(generator.generate_column_gen(0, 0)));
        let band_lo = *World::surface_window_for_column(&world.column_gen[&cpos], 0).start();
        world.last_load_target = Some(LoadTarget::new(0, band_lo + 5, 0, 4));

        let deep = SectionPos::new(0, band_lo - 2, 0);
        let mut s = solid_section(deep);
        s.set_block(8, 8, 8, Block::Air);
        install(&mut world, s);

        pump(&mut world);
        assert!(
            world.hidden_parked.contains(&deep),
            "an isolated deep pocket parks while the player is far away"
        );

        // The player descends next to it: the ring must pull it back in.
        world.last_load_target = Some(LoadTarget::new(0, deep.cy, 0, 4));
        world.vis_dirty = true;
        pump(&mut world);
        assert!(
            world.iter_meshes().any(|(p, _)| p == deep),
            "a deep section inside the player ring must mesh"
        );
    }
}
