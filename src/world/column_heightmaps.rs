//! Per-column surface / direct-sky-cover map maintenance, plus the change
//! envelope ([`SkyCoverChange`]) streaming and edits use to bound skylight
//! invalidation.

use crate::block::Block;
use crate::chunk::{
    section_idx, ChunkPos, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::column::NO_SURFACE;

use super::store::World;

/// Vertical envelope of one column's direct-sky-cover changes. Skylight can
/// only differ between the lower endpoint's seep reach and the upper endpoint,
/// so streaming invalidation need not touch the rest of the world stack.
#[derive(Copy, Clone, Debug)]
pub(super) struct SkyCoverChange {
    min_cover: i32,
    max_cover: i32,
}

impl SkyCoverChange {
    pub(super) fn between(old: i32, new: i32) -> Option<Self> {
        (old != new).then_some(Self {
            min_cover: old.min(new),
            max_cover: old.max(new),
        })
    }

    pub(super) fn merge(&mut self, other: Self) {
        self.min_cover = self.min_cover.min(other.min_cover);
        self.max_cover = self.max_cover.max(other.max_cover);
    }

    pub(super) fn affects(self, pos: SectionPos) -> bool {
        super::light::cover_change_affects_section(pos, self.min_cover, self.max_cover)
    }

    /// L1 gap from `pos`'s cell box to the changed direct-sky segment of the
    /// world column `(wx, wz)` — the cells between the two cover endpoints,
    /// whose direct-sky status flipped. Light can only change within the
    /// flood reach of that segment, so a single-column cover move needs no
    /// blanket 3×3-column invalidation.
    pub(super) fn segment_gap(self, pos: SectionPos, wx: i32, wz: i32) -> i32 {
        let (ox, oy, oz) = pos.origin_world();
        let side = SECTION_SIZE as i32 - 1;
        let gx = (ox - wx).max(wx - (ox + side)).max(0);
        let gz = (oz - wz).max(wz - (oz + side)).max(0);
        let seg_lo = self.min_cover.saturating_add(1);
        let seg_hi = self.max_cover;
        let gy = (oy - seg_hi).max(seg_lo - (oy + side)).max(0);
        gx + gz + gy
    }

    /// Generated-section ingest already invalidates that section's 3x3x3. Only
    /// an unusual cover jump spanning farther vertically needs the additional
    /// column-map invalidation pass.
    pub(super) fn escapes_section_neighborhood(self, changed: SectionPos) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).any(|cy| {
            (cy - changed.cy).abs() > 1 && self.affects(SectionPos::new(changed.cx, cy, changed.cz))
        })
    }
}

impl World {
    /// Recompute a column's visible surface and direct-sky cover from its
    /// currently-loaded sections. Used after overlaying saved terrain, whose
    /// blocks can differ from generation. Returns the changed cover envelope.
    pub(super) fn recompute_column_heightmaps(&mut self, cpos: ChunkPos) -> Option<SkyCoverChange> {
        // Gather both maps under immutable section borrows, then write the
        // column once (the section and column maps are distinct fields).
        let mut surf = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut surface_remaining = surf.len();
        let mut sky_remaining = sky.len();
        for cy in Self::column_section_range().rev() {
            if surface_remaining == 0 && sky_remaining == 0 {
                break;
            }
            let Some(section) = self.sections.get(&SectionPos::new(cpos.cx, cy, cpos.cz)) else {
                continue;
            };
            let oy = cy * SECTION_SIZE as i32;
            let blocks = section.blocks_slice();
            for lz in 0..SECTION_SIZE {
                for lx in 0..SECTION_SIZE {
                    let col = lz * SECTION_SIZE + lx;
                    if surf[col] != NO_SURFACE && sky[col] != NO_SURFACE {
                        continue;
                    }
                    for ly in (0..SECTION_SIZE).rev() {
                        let id = blocks[section_idx(lx, ly, lz)];
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
        let bare = self.column_gen.get(&cpos).cloned();
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

    /// Merge one deterministic generated/cache section into the analytical bare
    /// surface and sky-cover maps. It can only add feature blocks above those
    /// baselines; authoritative saved terrain uses
    /// [`recompute_column_heightmaps`](Self::recompute_column_heightmaps) because
    /// it may also remove them. Returns the changed cover envelope.
    pub(super) fn raise_column_heightmaps_from_section(
        &mut self,
        pos: SectionPos,
    ) -> Option<SkyCoverChange> {
        let cpos = pos.chunk_pos();
        let oy = pos.cy * SECTION_SIZE as i32;
        let mut raised_surface = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut raised_sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let Some(section) = self.sections.get(&pos) else {
            return None;
        };
        let Some(column) = self.columns.get(&cpos) else {
            return None;
        };
        let blocks = section.blocks_slice();
        let mut any = false;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                let surface = column.surface_y(lx, lz);
                if oy + SECTION_SIZE as i32 - 1 > surface {
                    for ly in (0..SECTION_SIZE).rev() {
                        let wy = oy + ly as i32;
                        if wy <= surface {
                            break;
                        }
                        if blocks[section_idx(lx, ly, lz)] != Block::Air.id() {
                            raised_surface[i] = wy;
                            any = true;
                            break;
                        }
                    }
                }

                let sky_cover = column.sky_cover_y(lx, lz);
                if oy + SECTION_SIZE as i32 - 1 > sky_cover {
                    for ly in (0..SECTION_SIZE).rev() {
                        let wy = oy + ly as i32;
                        if wy <= sky_cover {
                            break;
                        }
                        let block = Block::from_id(blocks[section_idx(lx, ly, lz)]);
                        if !block.transmits_direct_skylight() {
                            raised_sky[i] = wy;
                            any = true;
                            break;
                        }
                    }
                }
            }
        }
        if !any {
            return None;
        }
        let column = self.columns.get_mut(&cpos).expect("column checked above");
        let mut payload_changed = false;
        let mut sky_change: Option<SkyCoverChange> = None;
        for lz in 0..SECTION_SIZE {
            for lx in 0..SECTION_SIZE {
                let i = lz * SECTION_SIZE + lx;
                if raised_surface[i] > column.surface_y(lx, lz) {
                    column.set_surface_y(lx, lz, raised_surface[i]);
                    payload_changed = true;
                }
                if raised_sky[i] > column.sky_cover_y(lx, lz) {
                    let change = SkyCoverChange::between(column.sky_cover_y(lx, lz), raised_sky[i])
                        .expect("raised cover height");
                    if let Some(all) = sky_change.as_mut() {
                        all.merge(change);
                    } else {
                        sky_change = Some(change);
                    }
                    column.set_sky_cover_y(lx, lz, raised_sky[i]);
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
