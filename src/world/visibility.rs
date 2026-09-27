//! Deep-section visibility: skip meshing (and thereby lighting) for below-surface
//! sections no sightline can reach.
//!
//! Sections in or above their column's surface retention band always mesh — they can
//! face the open sky. Sections wholly BELOW that band ("deep") are only visible
//! through cave openings, so they mesh only when a breadth-first search from the
//! visible region reaches them:
//!
//! - Seeds: a deep section bordering a LOADED non-deep section whose facing plane is
//!   open, and every deep section in the player's 5×5×5 ring (the player may be
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

use crate::world::{ReplicaWorld, World, WorldSide};
use std::collections::VecDeque;

use petramond_math::math::FACE_NEIGHBORS;
use petramond_world::chunk::SectionPos;

const NEAR_LOAD_RADIUS: i32 = 2;

impl<S: WorldSide> World<S> {
    pub(super) fn near_load_center(&self, pos: SectionPos) -> bool {
        let Some(t) = self.data.last_load_target else {
            return true;
        };
        let near = |target: super::store::LoadTarget| {
            (pos.cx - target.center.cx).abs() <= NEAR_LOAD_RADIUS
                && (pos.cy - target.center_cy).abs() <= NEAR_LOAD_RADIUS
                && (pos.cz - target.center.cz).abs() <= NEAR_LOAD_RADIUS
        };
        near(t) || self.data.extra_load_targets.iter().copied().any(near)
    }

    pub(super) fn column_near_load_center(&self, pos: petramond_world::chunk::ChunkPos) -> bool {
        let near = |target: super::store::LoadTarget| {
            (pos.cx - target.center.cx).abs() <= NEAR_LOAD_RADIUS
                && (pos.cz - target.center.cz).abs() <= NEAR_LOAD_RADIUS
        };
        self.data.last_load_target.is_some_and(near)
            || self.data.extra_load_targets.iter().copied().any(near)
    }
}

impl ReplicaWorld {
    pub(super) fn classify_deep_on_install(&mut self, pos: SectionPos) {
        let Some(&band_lo) = self.data.column_deep_band_los.get(&pos.chunk_pos()) else {
            return;
        };
        if pos.cy < band_lo {
            self.side.terrain.deep_sections.insert(pos);
        }
        self.side.terrain.vis_dirty = true;
    }

    pub(super) fn section_hidden(&self, pos: SectionPos) -> bool {
        self.side.terrain.deep_sections.contains(&pos)
            && !self.side.terrain.visible_deep.contains(&pos)
            && !self.near_load_center(pos)
    }

    pub(super) fn refresh_deep_visibility(&mut self) {
        self.side.terrain.vis_dirty = false;

        let mut visible: rustc_hash::FxHashSet<SectionPos> = rustc_hash::FxHashSet::default();
        let mut queue: VecDeque<SectionPos> = VecDeque::new();
        let mut entered: rustc_hash::FxHashSet<SectionPos> = rustc_hash::FxHashSet::default();

        for &pos in &self.side.terrain.deep_sections {
            let Some(s) = self.data.sections.get(&pos) else {
                continue;
            };
            if self.near_load_center(pos) {
                visible.insert(pos);
                if entered.insert(pos) {
                    queue.push_back(pos);
                }
                continue;
            }
            for d in FACE_NEIGHBORS {
                let (dx, dy, dz) = (d.x, d.y, d.z);
                let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if self.side.terrain.deep_sections.contains(&n) {
                    continue;
                }
                let n_side_open = self
                    .data
                    .sections
                    .get(&n)
                    .is_some_and(|ns| ns.face_plane_open(-dx, -dy, -dz));
                if !n_side_open {
                    continue;
                }
                visible.insert(pos);
                if s.face_plane_open(dx, dy, dz) && entered.insert(pos) {
                    queue.push_back(pos);
                }
            }
        }

        while let Some(pos) = queue.pop_front() {
            let Some(s) = self.data.sections.get(&pos) else {
                continue;
            };
            for d in FACE_NEIGHBORS {
                let (dx, dy, dz) = (d.x, d.y, d.z);
                if !s.face_plane_open(dx, dy, dz) {
                    continue;
                }
                let n = SectionPos::new(pos.cx + dx, pos.cy + dy, pos.cz + dz);
                if !self.side.terrain.deep_sections.contains(&n) {
                    continue;
                }
                visible.insert(n);
                let Some(ns) = self.data.sections.get(&n) else {
                    continue;
                };
                if ns.face_plane_open(-dx, -dy, -dz) && entered.insert(n) {
                    queue.push_back(n);
                }
            }
        }

        let unpark: Vec<SectionPos> = self
            .side
            .terrain
            .hidden_parked
            .iter()
            .filter(|p| visible.contains(p) || self.near_load_center(**p))
            .copied()
            .collect();
        for pos in unpark {
            self.side.terrain.hidden_parked.remove(&pos);
            self.side.terrain.dirty_meshes.push(pos);
        }

        let unseal_near: Vec<SectionPos> = self
            .side
            .terrain
            .sealed_parked
            .iter()
            .filter(|p| self.near_load_center(**p))
            .copied()
            .collect();
        for pos in unseal_near {
            self.queue_dirty_mesh(pos);
        }

        self.side.terrain.visible_deep = visible;
    }
}

#[cfg(test)]
mod tests {
    use crate::world::store::LoadTarget;
    use crate::world::ReplicaWorld;
    use petramond_world::block::Block;
    use petramond_world::chunk::{ChunkPos, SectionPos, SECTION_SIZE};
    use petramond_world::section::Section;
    use petramond_worldgen::ChunkGenerator;
    use std::sync::Arc;

    fn solid_section(pos: SectionPos) -> Section {
        let mut section = Section::new(pos.cx, pos.cy, pos.cz);
        section.blocks_mut().fill(Block::Stone.id());
        section.recompute_opaque_count();
        section
    }

    fn install(world: &mut ReplicaWorld, section: Section) {
        let pos = SectionPos::new(section.cx, section.cy, section.cz);
        world.data.ensure_column(pos.chunk_pos());
        world.data.sections.insert(pos, Arc::new(section));
        world.note_section_loaded(pos);
        world.classify_deep_on_install(pos);
        world.queue_dirty_mesh(pos);
    }

    fn pump(world: &mut ReplicaWorld) {
        for _ in 0..200 {
            world.tick_mesh_budget(8);
            if !world.has_dirty_meshes() {
                break;
            }
        }
    }

    #[test]
    fn hidden_cave_parks_unmeshed_and_opens_when_dug_into() {
        let mut world = ReplicaWorld::new(1, 4);
        let generator = ChunkGenerator::new(1);
        let cpos = ChunkPos::new(0, 0);
        world.data.ensure_column(cpos);
        let column = generator.generate_column_gen(0, 0);
        let band_lo = *ReplicaWorld::surface_window_for_column(&column, 0).start();
        world.data.column_deep_band_los.insert(cpos, band_lo);
        world.data.last_load_target = Some(LoadTarget::new(0, band_lo + 5, 0, 4));

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
                world.side.terrain.hidden_parked.contains(&pos),
                "a sealed deep cave section parks for later re-exposure"
            );
        }

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
        let mut world = ReplicaWorld::new(1, 4);
        let generator = ChunkGenerator::new(1);
        let cpos = ChunkPos::new(0, 0);
        world.data.ensure_column(cpos);
        let column = generator.generate_column_gen(0, 0);
        let band_lo = *ReplicaWorld::surface_window_for_column(&column, 0).start();
        world.data.column_deep_band_los.insert(cpos, band_lo);
        world.data.last_load_target = Some(LoadTarget::new(0, band_lo + 5, 0, 4));

        let deep = SectionPos::new(0, band_lo - 2, 0);
        let mut s = solid_section(deep);
        s.set_block(8, 8, 8, Block::Air);
        install(&mut world, s);

        pump(&mut world);
        assert!(
            world.side.terrain.hidden_parked.contains(&deep),
            "an isolated deep pocket parks while the player is far away"
        );

        world.data.last_load_target = Some(LoadTarget::new(deep.cx - 2, deep.cy, deep.cz, 4));
        world.side.terrain.vis_dirty = true;
        pump(&mut world);
        assert!(
            world.iter_meshes().any(|(p, _)| p == deep),
            "a deep section inside the player ring must mesh"
        );
    }
}
