//! Per-column surface / direct-sky-cover map maintenance, plus the change
//! envelope ([`SkyCoverChange`]) streaming and edits use to bound skylight
//! invalidation.
//! (Data-half queries; the mutation/orchestration half stays in the engine crate.)

use crate::world::data::WorldData;
use crate::block::Block;
use crate::chunk::{
    section_idx, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use crate::column::NO_SURFACE;

impl WorldData {

    /// Merge one deterministic generated/cache section into the analytical bare
    /// surface and sky-cover maps. It can only add feature blocks above those
    /// baselines; authoritative saved terrain uses
    /// `recompute_column_heightmaps` because
    /// it may also remove them. Returns the changed cover envelope.
    pub fn raise_column_heightmaps_from_section(
        &mut self,
        pos: SectionPos,
    ) -> Option<SkyCoverChange> {
        let cpos = pos.chunk_pos();
        let oy = pos.cy * SECTION_SIZE as i32;
        let mut raised_surface = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut raised_sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let section = self.sections.get(&pos)?;
        let column = self.columns.get(&cpos)?;
        let blocks = section.blocks();
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
                        if blocks.get(section_idx(lx, ly, lz)) != Block::Air.id() {
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
                        let block = Block::from_id(blocks.get(section_idx(lx, ly, lz)));
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

/// Vertical envelope of one column's direct-sky-cover changes. Skylight can
/// only differ between the lower endpoint's seep reach and the upper endpoint,
/// so streaming invalidation need not touch the rest of the world stack.
#[derive(Copy, Clone, Debug)]
pub struct SkyCoverChange {
    min_cover: i32,
    max_cover: i32,
}

impl SkyCoverChange {
    pub fn between(old: i32, new: i32) -> Option<Self> {
        (old != new).then_some(Self {
            min_cover: old.min(new),
            max_cover: old.max(new),
        })
    }

    pub fn merge(&mut self, other: Self) {
        self.min_cover = self.min_cover.min(other.min_cover);
        self.max_cover = self.max_cover.max(other.max_cover);
    }

    pub fn affects(self, pos: SectionPos) -> bool {
        super::light::cover_change_affects_section(pos, self.min_cover, self.max_cover)
    }

    /// L1 gap from `pos`'s cell box to the changed direct-sky segment of the
    /// world column `(wx, wz)` — the cells between the two cover endpoints,
    /// whose direct-sky status flipped. Light can only change within the
    /// flood reach of that segment, so a single-column cover move needs no
    /// blanket 3×3-column invalidation.
    pub fn segment_gap(self, pos: SectionPos, wx: i32, wz: i32) -> i32 {
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
    pub fn escapes_section_neighborhood(self, changed: SectionPos) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).any(|cy| {
            (cy - changed.cy).abs() > 1 && self.affects(SectionPos::new(changed.cx, cy, changed.cz))
        })
    }
}
