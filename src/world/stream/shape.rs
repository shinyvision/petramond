use crate::world::{World, WorldSide};
use petramond_world::chunk::{
    ChunkPos, SectionPos, SEA_LEVEL, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE,
};
use petramond_worldgen::ColumnGen;

use crate::world::store::{LoadTarget, VERTICAL_LOAD_RADIUS};

const SURFACE_WINDOW_BELOW: i32 = 2;
const SURFACE_WINDOW_ABOVE: i32 = 1;
const HORIZONTAL_KEEP_SLACK: i32 = 2;

impl<S: WorldSide> World<S> {
    pub(in crate::world) fn column_wanted_by_any_target(&self, cp: ChunkPos) -> bool {
        self.data
            .last_load_target
            .is_some_and(|t| Self::column_wanted(t, cp))
            || self
                .data
                .extra_load_targets
                .iter()
                .any(|t| Self::column_wanted(*t, cp))
    }

    pub(in crate::world) fn anchor_underground(&self, target: LoadTarget) -> bool {
        let band_lo = self
            .column_gen(target.center)
            .map(|col| *Self::surface_window_for_column(col, 0).start())
            .or_else(|| self.data.column_deep_band_los.get(&target.center).copied());
        band_lo.is_some_and(|lo| target.center_cy < lo)
    }

    pub(in crate::world) fn vertical_window(
        center_cy: i32,
        slack: i32,
    ) -> std::ops::RangeInclusive<i32> {
        let center_cy = center_cy.clamp(SECTION_MIN_CY, SECTION_MAX_CY);
        let r = VERTICAL_LOAD_RADIUS + slack;
        (center_cy - r).max(SECTION_MIN_CY)..=(center_cy + r).min(SECTION_MAX_CY)
    }

    pub(in crate::world) fn surface_window_for_column(
        col: &ColumnGen,
        slack: i32,
    ) -> std::ops::RangeInclusive<i32> {
        let (surf_min, _) = col.surf_range();
        let bottom_y = surf_min.max(SEA_LEVEL);
        let top_y = col.content_top().max(SEA_LEVEL);
        let lo = bottom_y.div_euclid(SECTION_SIZE as i32) - SURFACE_WINDOW_BELOW - slack;
        let hi = top_y.div_euclid(SECTION_SIZE as i32) + SURFACE_WINDOW_ABOVE + slack;
        lo.max(SECTION_MIN_CY)..=hi.min(SECTION_MAX_CY)
    }

    pub(super) fn wanted_section_cys(col: &ColumnGen, center_cy: i32, slack: i32) -> Vec<i32> {
        let mut out: Vec<i32> = Self::vertical_window(center_cy, slack).collect();
        for cy in Self::surface_window_for_column(col, slack) {
            if !out.contains(&cy) {
                out.push(cy);
            }
        }
        out
    }

    pub(super) fn wanted_section_cys_for_column(
        &self,
        pos: ChunkPos,
        col: &ColumnGen,
        center_cy: i32,
        slack: i32,
    ) -> Vec<i32> {
        let mut out = Self::wanted_section_cys(col, center_cy, slack);
        crate::world::store::for_each_column_cy(self.sky_cavern_bits(pos), |cy| {
            if !out.contains(&cy) {
                out.push(cy);
            }
        });
        for &cy in self.data.saved.sections_in_column(pos) {
            if !out.contains(&cy) {
                out.push(cy);
            }
        }
        out
    }

    fn column_shape_key(target: LoadTarget, pos: ChunkPos) -> (i32, i32, i32) {
        (
            pos.cx - target.center.cx,
            pos.cz - target.center.cz,
            target.render_dist.max(0),
        )
    }

    fn column_in_shape(target: LoadTarget, pos: ChunkPos, slack: i32) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let radius = (r + slack).max(0);
        dx * dx + dz * dz <= radius * radius
    }

    pub(in crate::world) fn column_wanted(target: LoadTarget, pos: ChunkPos) -> bool {
        Self::column_in_shape(target, pos, 0)
    }

    pub(in crate::world) fn column_kept(target: LoadTarget, pos: ChunkPos) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let keep = r + HORIZONTAL_KEEP_SLACK;
        dx * dx + dz * dz <= keep * keep
    }

    pub(super) fn skip_empty_sky_section(&self, sp: SectionPos, content_top: i32) -> bool {
        (sp.cy * SECTION_SIZE as i32) > content_top && !self.data.saved.authoritative_contains(sp)
    }

    pub(super) fn within_current_keep_radius(&self, pos: ChunkPos) -> bool {
        let Some(target) = self.data.last_load_target else {
            return true;
        };
        Self::column_kept(target, pos)
            || self
                .data
                .extra_load_targets
                .iter()
                .any(|t| Self::column_kept(*t, pos))
    }
}
