use crate::block::Block;
use crate::chunk::{section_idx, SectionPos, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::column::NO_SURFACE;
use crate::world::data::WorldData;

impl WorldData {
    pub fn raise_column_heightmaps_from_section(
        &mut self,
        pos: SectionPos,
    ) -> Option<SkyCoverChange> {
        let cpos = pos.chunk_pos();
        let oy = pos.cy * SECTION_SIZE as i32;
        let mut raised_surface = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let mut raised_sky = [NO_SURFACE; SECTION_SIZE * SECTION_SIZE];
        let section = self.sections.get(&pos)?;
        if section.is_empty_air() {
            return None;
        }
        let column = self.columns.get(&cpos)?;
        let blocks = section.blocks();
        let light_cells = crate::block::light_cells();
        let covers_sky = |id: u16| {
            light_cells
                .get(id as usize)
                .copied()
                .unwrap_or(light_cells[0])
                & crate::block::LIGHT_CELL_DIRECT_SKY
                == 0
        };
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
                        if covers_sky(blocks.get(section_idx(lx, ly, lz))) {
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
        let column =
            std::sync::Arc::make_mut(self.columns.get_mut(&cpos).expect("column checked above"));
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

    pub fn escapes_section_neighborhood(self, changed: SectionPos) -> bool {
        (SECTION_MIN_CY..=SECTION_MAX_CY).any(|cy| {
            (cy - changed.cy).abs() > 1 && self.affects(SectionPos::new(changed.cx, cy, changed.cz))
        })
    }
}
