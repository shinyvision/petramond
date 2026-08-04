//! Per-column surface / direct-sky-cover map maintenance, plus the change
//! envelope ([`SkyCoverChange`]) streaming and edits use to bound skylight
//! invalidation.

use crate::world::WorldData;
use crate::block::Block;
use crate::chunk::{
    section_idx, ChunkPos, SectionPos, SECTION_SIZE,
};
use crate::column::NO_SURFACE;

use petramond_world::world::column_heightmaps::SkyCoverChange;
use super::store::World;

    /// Recompute a column's visible surface and direct-sky cover from its
    /// currently-loaded sections. Used after overlaying saved terrain, whose
    /// blocks can differ from generation. Returns the changed cover envelope.
    impl World {
    pub(super) fn recompute_column_heightmaps(&mut self, cpos: ChunkPos) -> Option<SkyCoverChange> {
        // Gather both maps under immutable section borrows, then write the
        // column once (the section and column maps are distinct fields).
        let mut surf = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut surface_remaining = surf.len();
        let mut sky_remaining = sky.len();
        for cy in WorldData::column_section_range().rev() {
            if surface_remaining == 0 && sky_remaining == 0 {
                break;
            }
            let Some(section) = self.sections.get(&SectionPos::new(cpos.cx, cy, cpos.cz)) else {
                continue;
            };
            let oy = cy * SECTION_SIZE as i32;
            let blocks = section.blocks();
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let col = lz * SECTION_SIZE + lx;
                    if surf[col] != NO_SURFACE && sky[col] != NO_SURFACE {
                        continue;
                    }
                    for ly in (0..SECTION_SIZE).rev() {
                        let id = blocks.get(section_idx(lx, ly, lz));
                        if surf[col] == NO_SURFACE && id != Block::Air.id() {
                            surf[col] = oy + ly as i32;
                            surface_remaining -= 1;
                        }
                        if sky[col] == NO_SURFACE && !Block::from_id(id).transmits_direct_skylight()
                        {
                            sky[col] = oy + ly as i32;
                            sky_remaining -= 1;
                        }
                        if surf[col] != NO_SURFACE && sky[col] != NO_SURFACE {
                            break;
                        }
                    }
                }
            }
        }
        // Floor the scan at the generated surface only while that surface section is
        // absent. Once loaded, its blocks are authoritative; otherwise a streaming
        // recompute can "restore" ground over a player-dug sky shaft.
        let bare = self.gen.column_gen.get(&cpos).cloned();
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                let ground = bare
                    .as_ref()
                    .map(|c| c.heightmap_surface_y(lx, lz))
                    .unwrap_or(NO_SURFACE);
                let ground_loaded = SectionPos::from_world(
                    cpos.cx * SECTION_SIZE as i32 + lx as i32,
                    ground,
                    cpos.cz * SECTION_SIZE as i32 + lz as i32,
                )
                .is_some_and(|sp| self.sections.contains_key(&sp));
                if !ground_loaded && ground != NO_SURFACE {
                    surf[i] = surf[i].max(ground);
                    sky[i] = sky[i].max(ground);
                }
            }
        }
        let col = self.ensure_column(cpos);
        let mut payload_changed = false;
        let mut sky_change: Option<SkyCoverChange> = None;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                if col.surface_y(lx, lz) != surf[i] {
                    col.set_surface_y(lx, lz, surf[i]);
                    payload_changed = true;
                }
                if col.sky_cover_y(lx, lz) != sky[i] {
                    let change = SkyCoverChange::between(col.sky_cover_y(lx, lz), sky[i])
                        .expect("different cover heights");
                    if let Some(all) = sky_change.as_mut() {
                        all.merge(change);
                    } else {
                        sky_change = Some(change);
                    }
                    col.set_sky_cover_y(lx, lz, sky[i]);
                    payload_changed = true;
                }
            }
        }
        if payload_changed {
            self.bump_column_payload_revision(cpos);
        }
        sky_change
    }
}

