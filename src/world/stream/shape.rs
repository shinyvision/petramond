use crate::chunk::{ChunkPos, SectionPos, SEA_LEVEL, SECTION_MAX_CY, SECTION_MIN_CY, SECTION_SIZE};
use crate::worldgen::driver::ColumnGen;

use crate::world::store::{LoadTarget, World, VERTICAL_LOAD_RADIUS};

const SURFACE_WINDOW_BELOW: i32 = 2;
const SURFACE_WINDOW_ABOVE: i32 = 1;
const HORIZONTAL_KEEP_SLACK: i32 = 2;

impl World {
    /// Whether `cp` is wanted under ANY current anchor. The multi-anchor form
    /// of `last_load_target + column_wanted`, used wherever "is this column
    /// coming?" must hold for every player (the sim guard's in-flight
    /// classification, keep checks). Identical to the single check while
    /// `extra_load_targets` is empty.
    pub(in crate::world) fn column_wanted_by_any_target(&self, cp: ChunkPos) -> bool {
        self.last_load_target
            .is_some_and(|t| Self::column_wanted(t, cp))
            || self
                .extra_load_targets
                .iter()
                .any(|t| Self::column_wanted(*t, cp))
    }

    /// Whether `target`'s anchor sits below its own column's surface band — the
    /// caving case where the surface-first scheduling bias must stay off (see
    /// [`LoadTarget::surface_biased_section_key`]). An anchor whose column data
    /// hasn't landed yet counts as above ground: the bias only reorders work,
    /// and the anchor's own column is always the nearest-first column job.
    pub(in crate::world) fn anchor_underground(&self, target: LoadTarget) -> bool {
        let band_lo = self
            .column_gen
            .get(&target.center)
            .map(|col| *Self::surface_window_for_column(col, 0).start())
            .or_else(|| self.column_deep_band_los.get(&target.center).copied());
        band_lo.is_some_and(|lo| target.center_cy < lo)
    }

    /// The vertical section-`cy` window around the player, clamped to the world range.
    /// `slack` widens it (used by unload for hysteresis so a section doesn't thrash on
    /// the boundary).
    pub(super) fn vertical_window(center_cy: i32, slack: i32) -> std::ops::RangeInclusive<i32> {
        let center_cy = center_cy.clamp(SECTION_MIN_CY, SECTION_MAX_CY);
        let r = VERTICAL_LOAD_RADIUS + slack;
        (center_cy - r).max(SECTION_MIN_CY)..=(center_cy + r).min(SECTION_MAX_CY)
    }

    /// A surface/content retention band for a generated column. This is intentionally
    /// independent from the player's current section: spectator flight far above the
    /// world should not evict the terrain stack underneath a still-visible column.
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

    /// Player-centred vertical window plus the column's surface/content band.
    /// UNORDERED (duplicates removed in-place): every consumer re-orders by its own
    /// submission priority key, so sorting here was pure per-column waste.
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
        if let Some(save) = self.save.as_ref() {
            for sp in save.manifest_sections_in_column(pos) {
                if !out.contains(&sp.cy) {
                    out.push(sp.cy);
                }
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

    /// `pub(in crate::world)` for the sim guard: an absent column that is wanted under the
    /// current target counts as in-flight, not as never-coming.
    pub(in crate::world) fn column_wanted(target: LoadTarget, pos: ChunkPos) -> bool {
        Self::column_in_shape(target, pos, 0)
    }

    /// `pub(in crate::world)` for the per-connection terrain sender: its client-side
    /// unload mirrors the streamer's own keep hysteresis.
    pub(in crate::world) fn column_kept(target: LoadTarget, pos: ChunkPos) -> bool {
        let (dx, dz, r) = Self::column_shape_key(target, pos);
        let keep = r + HORIZONTAL_KEEP_SLACK;
        dx * dx + dz * dz <= keep * keep
    }

    /// Whether `sp` can be left ungenerated: it sits entirely above its column's content
    /// (provably all-air sky) AND the save holds no player edit there. Absent sky sections
    /// read as air with full skylight, and building into the sky materializes the section
    /// on write — so skipping them costs the common case nothing while still streaming any
    /// sky structure the player saved. Halving the loaded section count this way cuts gen,
    /// meshing, AND lighting, since each scales with the number of loaded sections.
    pub(super) fn skip_empty_sky_section(&self, sp: SectionPos, content_top: i32) -> bool {
        (sp.cy * SECTION_SIZE as i32) > content_top
            && !self
                .save
                .as_ref()
                .is_some_and(|s| s.authoritative_manifest_contains(sp))
    }

    pub(super) fn within_current_keep_radius(&self, pos: ChunkPos) -> bool {
        let Some(target) = self.last_load_target else {
            return true;
        };
        Self::column_kept(target, pos)
            || self
                .extra_load_targets
                .iter()
                .any(|t| Self::column_kept(*t, pos))
    }
}
