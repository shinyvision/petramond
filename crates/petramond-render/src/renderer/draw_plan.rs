//! Frustum cull + depth sort: which sections and whole columns each pass
//! draws this frame, and in what order. `encode_passes` consumes the plan
//! without re-deciding any of it.

use super::*;

impl Renderer {
    /// Is this section mesh's bounding box inside the current view frustum?
    #[inline]
    fn aabb_visible(
        min: glam::Vec3,
        max: glam::Vec3,
        frustum: Frustum,
        render_origin: glam::Vec3,
        cam_pos: glam::Vec3,
        fog: f32,
    ) -> bool {
        // Cutout leaf sprays extend beyond their owning voxel/section.
        let margin = glam::Vec3::splat(petramond_mesh::FOLIAGE_OVERHANG);
        let min = min - margin;
        let max = max + margin;
        if !frustum.aabb_visible(min - render_origin, max - render_origin) {
            return false;
        }
        aabb_distance_sq(cam_pos, min, max) <= fog * fog
    }

    #[inline]
    fn section_visible(
        section: &GpuSectionMesh,
        frustum: Frustum,
        render_origin: glam::Vec3,
        cam_pos: glam::Vec3,
        fog: f32,
    ) -> bool {
        let (ox, oy, oz) = section.origin;
        let min = glam::Vec3::new(ox as f32, oy as f32, oz as f32);
        let max = glam::Vec3::new((ox + 16) as f32, (oy + 16) as f32, (oz + 16) as f32);
        Self::aabb_visible(min, max, frustum, render_origin, cam_pos, fog)
    }

    /// Whole-column AABB covering every installed section. Rejecting here is
    /// visibility-identical to rejecting every section: a section outside the
    /// column stack cannot exist, and a column that fails frustum/fog has no
    /// section that can pass.
    #[inline]
    fn column_visible(
        column: &GpuColumnMesh,
        column_pos: ChunkPos,
        frustum: Frustum,
        render_origin: glam::Vec3,
        cam_pos: glam::Vec3,
        fog: f32,
    ) -> bool {
        let (min_cy, max_cy) = column.cy_span;
        if min_cy > max_cy {
            return false;
        }
        let ox = column_pos.cx * 16;
        let oz = column_pos.cz * 16;
        let min = glam::Vec3::new(ox as f32, (min_cy * 16) as f32, oz as f32);
        let max = glam::Vec3::new(
            (ox + 16) as f32,
            ((max_cy + 1) * 16) as f32,
            (oz + 16) as f32,
        );
        Self::aabb_visible(min, max, frustum, render_origin, cam_pos, fog)
    }

    /// Frustum-cull + depth-sort the visible chunks into `order`, returning this
    /// frame's initial [`RenderStats`] and terrain-pass gates.
    pub(super) fn plan_draw_order(
        &mut self,
        order: &mut Vec<VisibleSection>,
        opaque_columns: &mut Vec<(f32, ChunkPos)>,
        model_columns: &mut Vec<(f32, ChunkPos)>,
        contact_columns: &mut Vec<(f32, ChunkPos)>,
    ) -> (RenderStats, bool, bool) {
        if self.terrain.planned_gpu_revision == self.terrain.gpu_revision
            && self.terrain.planned_view_key.as_ref() == Some(&self.terrain.view_key)
        {
            return (
                RenderStats::default(),
                self.terrain.plan_any_model,
                self.terrain.plan_any_transparent,
            );
        }
        // Cull + depth-sort the visible sections once. The opaque pass draws nearest-first
        // so the GPU's early-Z rejects occluded fragments before the fragment shader runs;
        // the transparent pass draws farthest-first for correct back-to-front alpha.
        let cam = self.view.cam_pos;
        let frustum = self.view.frustum;
        let render_origin = self.view.render_origin;
        let fog = self.terrain_cull_dist();
        let terrain_columns = &self.terrain.columns;
        let far_leaf_lod_state = &mut self.terrain.far_leaf_lod_state;
        order.clear();
        opaque_columns.clear();
        model_columns.clear();
        contact_columns.clear();
        let mut any_model_visible = false;
        let mut any_transparent_visible = false;
        for (column_pos, column) in terrain_columns {
            if !Self::column_visible(column, *column_pos, frustum, render_origin, cam, fog) {
                continue;
            }
            let first_section = order.len();
            let mut column_dist_sq = f32::INFINITY;
            let mut column_has_opaque = false;
            let mut column_has_model = false;
            let mut column_has_contact = false;
            let mut any_far_lod_active = false;
            for &(sp, ref section) in &column.sections {
                if !Self::section_visible(section, frustum, render_origin, cam, fog) {
                    continue;
                }
                let (ox, oy, oz) = section.origin;
                let c = glam::Vec3::new(ox as f32 + 8.0, oy as f32 + 8.0, oz as f32 + 8.0);
                let dist_sq = (cam - c).length_squared();
                column_dist_sq = column_dist_sq.min(dist_sq);
                column_has_opaque |= section.opaque_vertex_count > 0;
                column_has_model |=
                    section.model_idx_count > 0 || section.model_blend_idx_count > 0;
                // Contact visibility is its OWN presence bit: a multi-cell
                // model's contact triangles can sit in a section whose model
                // index range is empty.
                column_has_contact |= section.contact_vertex_count > 0;
                any_model_visible |=
                    section.model_idx_count > 0 || section.model_blend_idx_count > 0;
                any_transparent_visible |= section.transparent_vertex_count > 0
                    || section.transparent_ts_vertex_count > 0
                    || section.translucent_vertex_count > 0;
                // Only a section that OWNS a far mesh can be in the LOD state
                // map, so a section without one never touches the hash table —
                // which is nearly all of them, every frame.
                let use_far_leaf_lod = section.far_opaque_vertex_count > 0 && {
                    let was_active = far_leaf_lod_state.get(&sp).copied().unwrap_or(false);
                    let now_active = far_leaf_lod_active(dist_sq, (ox, oz), true, was_active);
                    if now_active {
                        far_leaf_lod_state.insert(sp, true);
                    } else if was_active {
                        far_leaf_lod_state.remove(&sp);
                    }
                    now_active
                };
                any_far_lod_active |= use_far_leaf_lod;
                order.push(VisibleSection {
                    dist_sq,
                    column_pos: *column_pos,
                    opaque_batched: false,
                    model_batched: false,
                    use_far_leaf_lod,
                    opaque_vertex_start: section.opaque_vertex_start,
                    opaque_quads: section.opaque_vertex_count / 4,
                    far_opaque_vertex_start: section.far_opaque_vertex_start,
                    far_opaque_quads: section.far_opaque_vertex_count / 4,
                    transparent_vertex_start: section.transparent_vertex_start,
                    transparent_quads: section.transparent_vertex_count / 4,
                    transparent_ts_vertex_start: section.transparent_ts_vertex_start,
                    transparent_ts_quads: section.transparent_ts_vertex_count / 4,
                    translucent_vertex_start: section.translucent_vertex_start,
                    translucent_quads: section.translucent_vertex_count / 4,
                    model_index_start: section.model_index_start,
                    model_idx_count: section.model_idx_count,
                    model_blend_index_start: section.model_blend_index_start,
                    model_blend_idx_count: section.model_blend_idx_count,
                });
            }
            let opaque_batched =
                column_has_opaque && !any_far_lod_active && column.opaque_quads > 0;
            let model_batched = column_has_model && column.model_idx_count > 0;
            if opaque_batched {
                opaque_columns.push((column_dist_sq, *column_pos));
            }
            if model_batched {
                model_columns.push((column_dist_sq, *column_pos));
            }
            if column_has_contact && column.contact_vertex_count > 0 {
                contact_columns.push((column_dist_sq, *column_pos));
            }
            // Drop the sections whose every layer is either empty or covered by
            // a whole-column draw: the per-section pass loops would skip them,
            // and they are the great majority — carrying them costs a 60-byte
            // move in the sort and a rejected branch in four encode loops.
            let mut w = first_section;
            for r in first_section..order.len() {
                let mut item = order[r];
                item.opaque_batched = opaque_batched;
                item.model_batched = model_batched;
                let opaque_left = !opaque_batched
                    && if item.use_far_leaf_lod {
                        item.far_opaque_quads
                    } else {
                        item.opaque_quads
                    } > 0;
                let model_left =
                    !model_batched && (item.model_idx_count > 0 || item.model_blend_idx_count > 0);
                if opaque_left
                    || model_left
                    || item.transparent_quads > 0
                    || item.transparent_ts_quads > 0
                    || item.translucent_quads > 0
                {
                    order[w] = item;
                    w += 1;
                }
            }
            order.truncate(w);
        }
        // Distance alone is not a total order: equidistant columns are common
        // (a symmetric view), and `terrain_columns` is a HashMap, so a stable
        // sort would leave ties in per-process hash order. That makes the
        // back-to-front transparent pass blend equidistant columns in a
        // different order run to run. Break ties on the column position so the
        // draw order is a function of the scene, not of the hash seed.
        order.sort_by(|a, b| {
            a.dist_sq
                .total_cmp(&b.dist_sq)
                .then_with(|| a.column_pos.cmp(&b.column_pos))
        });
        let by_dist_then_pos = |a: &(f32, ChunkPos), b: &(f32, ChunkPos)| {
            a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1))
        };
        opaque_columns.sort_by(by_dist_then_pos);
        model_columns.sort_by(by_dist_then_pos);
        contact_columns.sort_by(by_dist_then_pos);
        self.terrain.planned_gpu_revision = self.terrain.gpu_revision;
        self.terrain.planned_view_key = Some(self.terrain.view_key.clone());
        self.terrain.plan_any_model = any_model_visible;
        self.terrain.plan_any_transparent = any_transparent_visible;
        (
            RenderStats::default(),
            any_model_visible,
            any_transparent_visible,
        )
    }
}
