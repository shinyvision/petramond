//! The Layer-3 custom-shape SIM bake cache: per-cell collision boxes a pack's
//! WASM baked, read by the shape's collision facet. A cache MISS (never baked,
//! or a trapped/timed-out bake) falls back to the block row's static collision
//! boxes — the failure policy that keeps placed world data intact while only the
//! bake logic is suspended.
//!
//! Boxes are CONTENT-INTERNED to `'static`, so `World::collision_boxes_at` keeps
//! its `&'static [Aabb]` return without leaking per cell: a gate has two
//! configurations (open / closed), so at most two box sets are ever interned no
//! matter how many gates exist. The intern set is bounded by the shapes'
//! distinct geometries, not by the world.

use std::sync::Mutex;

use crate::block::{Aabb, Block, ShapeFamily};
use crate::chunk::{ChunkPos, SectionPos};
use crate::mathh::IVec3;

use super::store::World;

/// Interned `'static` box sets, deduped by content. Small (a handful per custom
/// shape), so a linear scan is cheaper than hashing float boxes.
static INTERN: Mutex<Vec<&'static [Aabb]>> = Mutex::new(Vec::new());

/// Hard cap on distinct interned box sets. A well-behaved shape has a handful of
/// configurations, but a bake keyed on `world_pos` could leak one slice PER CELL
/// forever (the leak is `'static`). Past the cap we refuse to cache new
/// geometry: those cells fall back to their static boxes (the failure policy),
/// which bounds the leak to a fixed, small amount regardless of a hostile bake.
const INTERN_CAP: usize = 512;

/// Intern `boxes` to a `'static` slice, reusing an equal set if one exists, or
/// `None` once the intern set is full (the caller then falls back to static
/// boxes rather than leaking without bound).
fn intern_boxes(boxes: &[Aabb]) -> Option<&'static [Aabb]> {
    let mut intern = INTERN.lock().expect("bake intern lock");
    if let Some(&existing) = intern.iter().find(|&&b| b == boxes) {
        return Some(existing);
    }
    if intern.len() >= INTERN_CAP {
        return None;
    }
    let leaked: &'static [Aabb] = Box::leak(boxes.to_vec().into_boxed_slice());
    intern.push(leaked);
    Some(leaked)
}

impl World {
    /// The baked collision boxes for the custom shape at `pos`, or `None` when
    /// the cell has no bake yet (the collision facet then uses the row's static
    /// boxes).
    #[inline]
    pub(crate) fn custom_shape_boxes(&self, pos: IVec3) -> Option<&'static [Aabb]> {
        self.custom_bake.get(&pos).copied()
    }

    /// Record a custom shape cell's freshly-baked collision boxes. A full intern
    /// set (a runaway per-position bake) drops the cache entry so the cell falls
    /// back to its static boxes instead of leaking an unbounded slice.
    pub(crate) fn set_custom_bake(&mut self, pos: IVec3, boxes: &[Aabb]) {
        match intern_boxes(boxes) {
            Some(interned) => {
                self.custom_bake.insert(pos, interned);
            }
            None => {
                self.custom_bake.remove(&pos);
            }
        }
    }

    /// Record a custom shape cell's freshly-baked RENDER boxes on its section
    /// (a no-op if the section isn't loaded) — the client render-bake pump. The
    /// section keeps it (and bumps its mesh revision) so the next mesh job draws
    /// the baked geometry instead of the cube fallback.
    pub(crate) fn set_custom_render_bake(&mut self, pos: IVec3, boxes: Box<[Aabb]>) {
        if let Some((sp, lx, ly, lz)) = Self::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = crate::chunk::section_idx(lx, ly, lz) as u16;
                section.set_shape_render(idx, boxes);
            }
        }
    }

    /// Record a custom shape cell's baked light aperture on its section (the
    /// deterministic SIM bake). The wire aperture is already a per-cell "opaque to
    /// light" decision — Opaque blocks light, Open passes it. A real opacity
    /// TRANSITION relights the cell's section neighbourhood so the change
    /// propagates; an unchanged bake costs nothing.
    pub(crate) fn set_custom_light_aperture(
        &mut self,
        pos: IVec3,
        aperture: mod_api::LightAperture,
    ) {
        let opaque = match aperture {
            mod_api::LightAperture::Opaque => true,
            mod_api::LightAperture::Open => false,
        };
        if let Some((sp, lx, ly, lz)) = Self::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = crate::chunk::section_idx(lx, ly, lz) as u16;
                if section.set_custom_light_aperture(idx, opaque) {
                    self.mark_light_dirty_neighborhood(sp, true);
                }
            }
        }
    }

    /// Drop a cell's bake so the next read re-bakes (or falls back) — the edit
    /// invalidation the block-write lanes call.
    #[inline]
    pub(crate) fn invalidate_custom_bake(&mut self, pos: IVec3) {
        self.custom_bake.remove(&pos);
    }

    /// Take the custom-shape cells that need a (re)bake, each with the neighbour
    /// context a bake reads — cleared, so the host's bake pump processes each
    /// dirty cell once. Cells whose block is no longer a custom shape (broken
    /// since being dirtied) are dropped.
    pub(crate) fn drain_custom_bake_dirty(&mut self) -> Vec<CustomBakeCell> {
        // Sort by position so the bake dispatch order is DEFINED and identical
        // on the server and every client replica (C1): the dirty set is a hashed
        // set with no stable order, and a bake that touched instance state would
        // otherwise diverge between the two and desync.
        let mut dirty: Vec<IVec3> = self.custom_bake_dirty.drain().collect();
        dirty.sort_by_key(|p| (p.x, p.y, p.z));
        dirty
            .into_iter()
            .filter_map(|pos| {
                let block = crate::block::Block::from_id(self.chunk_block(pos.x, pos.y, pos.z));
                if block.shape_family() != crate::block::ShapeFamily::Custom {
                    return None;
                }
                let n = |dx, dy, dz| {
                    self.physics_block(pos.x + dx, pos.y + dy, pos.z + dz)
                        .id()
                };
                Some(CustomBakeCell {
                    pos,
                    shape_kind: block.shape_kind().0,
                    // The shape's declaration key names the owning pack (namespace).
                    shape_key: block.shape_kind().key(),
                    block_id: block.id(),
                    neighbor_ids: [
                        n(-1, 0, 0),
                        n(1, 0, 0),
                        n(0, -1, 0),
                        n(0, 1, 0),
                        n(0, 0, -1),
                        n(0, 0, 1),
                    ],
                })
            })
            .collect()
    }

    /// Whether any custom-shape cell is awaiting a bake — the cheap gate the
    /// tick's bake step checks before building a mod dispatch scope.
    #[inline]
    pub(crate) fn has_pending_custom_bakes(&self) -> bool {
        !self.custom_bake_dirty.is_empty()
    }

    /// The number of baked RENDER boxes cached for the custom cell at `pos` (0
    /// when none) — the render-bake test probe.
    #[cfg(test)]
    pub(crate) fn custom_render_box_count(&self, pos: IVec3) -> usize {
        let Some((sp, lx, ly, lz)) = Self::split_world(pos.x, pos.y, pos.z) else {
            return 0;
        };
        self.sections
            .get(&sp)
            .and_then(|s| s.shape_render_boxes(crate::chunk::section_idx(lx, ly, lz) as u16))
            .map_or(0, |b| b.len())
    }

    /// Drop the per-cell collision bake cache (as a fresh load would), so the
    /// next read falls back to the static boxes — the reload-simulation probe.
    #[cfg(test)]
    pub(crate) fn clear_custom_bake_for_test(&mut self) {
        self.custom_bake.clear();
    }

    /// Run the section-load custom-shape scan for the section holding `pos` — the
    /// `note_section_loaded` re-bake path, exposed for reload tests.
    #[cfg(test)]
    pub(crate) fn scan_section_custom_bakes_for_test(&mut self, pos: IVec3) {
        if let Some((sp, ..)) = Self::split_world(pos.x, pos.y, pos.z) {
            self.scan_section_custom_bakes(sp);
        }
    }

    /// The baked light-aperture opacity stored for the custom cell at `pos`
    /// (`None` = never baked) — the SIM light-aperture test probe.
    #[cfg(test)]
    pub(crate) fn custom_light_aperture_opaque(&self, pos: IVec3) -> Option<bool> {
        let (sp, lx, ly, lz) = Self::split_world(pos.x, pos.y, pos.z)?;
        self.sections
            .get(&sp)?
            .custom_light_apertures()?
            .get(&(crate::chunk::section_idx(lx, ly, lz) as u16))
            .copied()
    }

    /// Mark every Layer-3 custom-shape cell in a freshly-LOADED section dirty for
    /// baking. A section load (worldgen, streaming, client ingest, save reload)
    /// sets its cells in BULK, bypassing [`mark_custom_bake_edit`], so a chair
    /// restored from disk would never re-bake — it would show the row's static
    /// fallback collision and the cube render forever. This is the load-time
    /// equivalent, called from `note_section_loaded` for every install.
    pub(in crate::world) fn scan_section_custom_bakes(&mut self, pos: crate::chunk::SectionPos) {
        let Some(section) = self.sections.get(&pos) else {
            return;
        };
        // An all-air section (the empty sky band, the common case above the
        // surface) can hold no custom shape — skip the id scan entirely.
        if section.is_empty_air() {
            return;
        }
        // The overwhelmingly common non-empty section still holds no custom
        // shape; the scan is a tight LUT loop over the id buffer.
        let (ox, oy, oz) = pos.origin_world();
        let mut dirty: Vec<IVec3> = Vec::new();
        for (idx, &id) in section.blocks_slice().iter().enumerate() {
            if Block::from_id(id).shape_family() == ShapeFamily::Custom {
                let (lx, ly, lz) = crate::chunk::section_local(idx);
                dirty.push(IVec3::new(ox + lx as i32, oy + ly as i32, oz + lz as i32));
            }
        }
        for p in dirty {
            self.custom_bake_dirty.insert(p);
        }
    }

    /// A block at `(wx, wy, wz)` became `new_block`: drop the cached bake for the
    /// cell and its face neighbours (a custom shape may read them), and re-mark
    /// any custom cell dirty for the next bake pump. The single hook both the
    /// authoritative edit (`set_block_world`) and the replica ingest
    /// (`apply_remote_delta`) call, so client prediction bakes the same cells the
    /// server does.
    pub(crate) fn mark_custom_bake_edit(&mut self, wx: i32, wy: i32, wz: i32, new_block: Block) {
        for (dx, dy, dz) in [
            (0, 0, 0),
            (-1, 0, 0),
            (1, 0, 0),
            (0, -1, 0),
            (0, 1, 0),
            (0, 0, -1),
            (0, 0, 1),
        ] {
            let p = IVec3::new(wx + dx, wy + dy, wz + dz);
            self.invalidate_custom_bake(p);
            let cell = if (dx, dy, dz) == (0, 0, 0) {
                new_block
            } else {
                Block::from_id(self.chunk_block(p.x, p.y, p.z))
            };
            if cell.shape_family() == ShapeFamily::Custom {
                self.custom_bake_dirty.insert(p);
            } else {
                // The cell is no longer a custom shape: drop any stale baked
                // light aperture so a later ungated read can't see it (the
                // render-box cache re-bakes with the cell, but the aperture map
                // has no such rewrite path).
                self.clear_custom_light_aperture(p);
            }
        }
    }

    /// Clear a cell's stored baked light aperture (it stopped being a custom
    /// shape), relighting its section neighbourhood only on a real change.
    fn clear_custom_light_aperture(&mut self, pos: IVec3) {
        if let Some((sp, lx, ly, lz)) = Self::split_world(pos.x, pos.y, pos.z) {
            if let Some(section) = self.section_mut(sp) {
                let idx = crate::chunk::section_idx(lx, ly, lz) as u16;
                if section.clear_custom_light_aperture(idx) {
                    self.mark_light_dirty_neighborhood(sp, true);
                }
            }
        }
    }

    /// Drop every cached custom bake (collision + dirty mark) in a section being
    /// evicted — the render-box and light-aperture caches ride the `Section` and
    /// evict with it, but the world-keyed collision map and the dirty set do not,
    /// so a roamed-away section would leave stale collision and churn
    /// `chunk_block` on unloaded coords every bake pump.
    pub(in crate::world) fn evict_custom_bake_section(&mut self, pos: SectionPos) {
        let in_section = |p: &IVec3| Self::split_world(p.x, p.y, p.z).map(|s| s.0) == Some(pos);
        self.custom_bake.retain(|p, _| !in_section(p));
        self.custom_bake_dirty.retain(|p| !in_section(p));
    }

    /// Drop every cached custom bake in a column being evicted.
    pub(in crate::world) fn evict_custom_bake_column(&mut self, pos: ChunkPos) {
        let in_column = |p: &IVec3| {
            ChunkPos::new(
                p.x.div_euclid(crate::chunk::SECTION_SIZE as i32),
                p.z.div_euclid(crate::chunk::SECTION_SIZE as i32),
            ) == pos
        };
        self.custom_bake.retain(|p, _| !in_column(p));
        self.custom_bake_dirty.retain(|p| !in_column(p));
    }

    /// Drop the whole custom-bake cache (the regen path clears every section).
    pub(in crate::world) fn clear_custom_bake(&mut self) {
        self.custom_bake.clear();
        self.custom_bake_dirty.clear();
    }
}

/// One custom-shape cell awaiting a bake, with the neighbourhood a bake reads.
pub(crate) struct CustomBakeCell {
    pub pos: IVec3,
    pub shape_kind: u8,
    pub shape_key: &'static str,
    pub block_id: u8,
    /// Neighbour block ids in `-x,+x,-y,+y,-z,+z` order.
    pub neighbor_ids: [u8; 6],
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interning_dedups_equal_box_sets() {
        let a = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        let b = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        // Equal content interns to the SAME 'static slice (pointer identity).
        assert!(std::ptr::eq(
            intern_boxes(&a).unwrap(),
            intern_boxes(&b).unwrap()
        ));
    }

    #[test]
    fn cache_stores_reads_and_invalidates() {
        let mut w = World::new(0, 4);
        let pos = IVec3::new(3, 64, -7);
        let half = [Aabb {
            min: [0.0, 0.0, 0.0],
            max: [1.0, 0.5, 1.0],
        }];
        assert_eq!(w.custom_shape_boxes(pos), None, "no bake yet");
        w.set_custom_bake(pos, &half);
        assert_eq!(w.custom_shape_boxes(pos), Some(&half[..]));
        // An edit at the cell drops the bake (the next read falls back / re-bakes).
        w.invalidate_custom_bake(pos);
        assert_eq!(w.custom_shape_boxes(pos), None);
    }
}
