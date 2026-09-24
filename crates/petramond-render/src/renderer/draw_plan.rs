//! Frustum cull + depth sort: which sections and whole columns each pass
//! draws this frame, and in what order. `encode_passes` consumes the plan
//! without re-deciding any of it.

use super::*;

impl Renderer {
    /// Is this render-local bounding box inside the current view frustum?
    /// `cam_pos` is render-local too. `enclosed` means an ancestor box was
    /// proved wholly inside the frustum, so only the fog range can reject
    /// this one.
    #[inline]
    fn aabb_visible(
        min: glam::Vec3,
        max: glam::Vec3,
        frustum: &Frustum,
        cam_pos: glam::Vec3,
        fog: f32,
        enclosed: bool,
    ) -> bool {
        // Cutout leaf sprays extend beyond their owning voxel/section.
        let margin = glam::Vec3::splat(petramond_mesh::FOLIAGE_OVERHANG);
        let min = min - margin;
        let max = max + margin;
        if !enclosed && !frustum.aabb_visible(min, max) {
            return false;
        }
        aabb_distance_sq(cam_pos, min, max) <= fog * fog
    }

    #[inline]
    fn section_visible(
        section: &GpuSectionMesh,
        frustum: &Frustum,
        render_origin: glam::IVec3,
        cam_pos: glam::Vec3,
        fog: f32,
        enclosed: bool,
    ) -> bool {
        let (ox, oy, oz) = section.origin;
        let min = (glam::IVec3::new(ox, oy, oz) - render_origin).as_vec3();
        let max = min + glam::Vec3::splat(16.0);
        Self::aabb_visible(min, max, frustum, cam_pos, fog, enclosed)
    }

    /// Whole-column AABB covering every installed section. Rejecting here is
    /// visibility-identical to rejecting every section: a section outside the
    /// column stack cannot exist, and a column that fails frustum/fog has no
    /// section that can pass.
    #[inline]
    fn column_visible(
        entry: &ColumnCull,
        frustum: &Frustum,
        render_origin: glam::IVec3,
        cam_pos: glam::Vec3,
        fog: f32,
        enclosed: bool,
    ) -> bool {
        let ColumnCull {
            pos,
            min_cy,
            max_cy,
            ..
        } = *entry;
        if min_cy > max_cy {
            return false;
        }
        let corner = glam::IVec3::new(pos.cx * 16, min_cy * 16, pos.cz * 16);
        let min = (corner - render_origin).as_vec3();
        let max = min + glam::Vec3::new(16.0, ((max_cy - min_cy + 1) * 16) as f32, 16.0);
        Self::aabb_visible(min, max, frustum, cam_pos, fog, enclosed)
    }

    /// Refresh the dense cull mirror when the column set changed: the columns
    /// grouped into [`CULL_REGION_COLUMNS`]-square regions, each region's
    /// entries contiguous, and a bounding record per region.
    ///
    /// Walking the column map is exactly the cost this mirror exists to keep
    /// off the per-frame path, so it happens once per change. The grouping is
    /// what makes the per-frame scan sublinear: at render distance 32 a view
    /// rejects most of ~3 200 columns, and one region test rejects up to 64 of
    /// them at once.
    fn refresh_cull_index(&mut self) {
        if self.terrain.cull_index_revision == self.terrain.gpu_revision {
            return;
        }
        let index = &mut self.terrain.cull_index;
        let regions = &mut self.terrain.cull_regions;
        index.clear();
        regions.clear();
        index.reserve(self.terrain.columns.len());
        index.extend(
            self.terrain
                .columns
                .iter()
                .map(|(pos, slot, column)| ColumnCull {
                    pos,
                    slot,
                    min_cy: column.cy_span.0,
                    max_cy: column.cy_span.1,
                }),
        );
        // Group by region, then by position inside it, so a region's columns
        // are one contiguous run and the run's order is a function of the
        // scene rather than of the column map's iteration order.
        index.sort_unstable_by_key(|e| {
            (
                e.pos.cx >> CULL_REGION_SHIFT,
                e.pos.cz >> CULL_REGION_SHIFT,
                e.pos.cx,
                e.pos.cz,
            )
        });
        let mut start = 0usize;
        while start < index.len() {
            let key = (
                index[start].pos.cx >> CULL_REGION_SHIFT,
                index[start].pos.cz >> CULL_REGION_SHIFT,
            );
            let mut end = start;
            let mut min_cy = i32::MAX;
            let mut max_cy = i32::MIN;
            while end < index.len()
                && (
                    index[end].pos.cx >> CULL_REGION_SHIFT,
                    index[end].pos.cz >> CULL_REGION_SHIFT,
                ) == key
            {
                min_cy = min_cy.min(index[end].min_cy);
                max_cy = max_cy.max(index[end].max_cy);
                end += 1;
            }
            regions.push(CullRegion {
                cx: key.0 << CULL_REGION_SHIFT,
                cz: key.1 << CULL_REGION_SHIFT,
                min_cy,
                max_cy,
                first: start as u32,
                last: end as u32,
            });
            start = end;
        }
        self.terrain.cull_index_revision = self.terrain.gpu_revision;
    }

    /// Frustum-cull + depth-sort the visible chunks into `order`, returning this
    /// frame's initial [`RenderStats`] and terrain-pass gates.
    pub(super) fn plan_draw_order(
        &mut self,
        order: &mut Vec<VisibleSection>,
        opaque_columns: &mut Vec<OpaqueColumnDraw>,
        model_columns: &mut Vec<(f32, ChunkPos, ColumnSlot)>,
        contact_columns: &mut Vec<(f32, ChunkPos, ColumnSlot)>,
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
        self.refresh_cull_index();
        // Cull + depth-sort the visible sections once. The opaque pass draws nearest-first
        // so the GPU's early-Z rejects occluded fragments before the fragment shader runs;
        // the transparent pass draws farthest-first for correct back-to-front alpha.
        let frustum = &self.view.frustum;
        let render_origin = self.view.render_origin;
        // Cull and sort in render-local space, like the GPU draws.
        let cam = self.view.cam_pos.relative_to(render_origin);
        let fog = self.terrain_cull_dist();
        let Self { terrain, .. } = self;
        let sort_scratch = &mut terrain.sort_scratch;
        let sorted = &mut terrain.sorted_scratch;
        let cull_index = &terrain.cull_index;
        let cull_regions = &terrain.cull_regions;
        let terrain_columns = &mut terrain.columns;
        order.clear();
        opaque_columns.clear();
        model_columns.clear();
        contact_columns.clear();
        let mut any_model_visible = false;
        let mut any_transparent_visible = false;
        for region in cull_regions {
            if region.min_cy > region.max_cy {
                continue;
            }
            let corner = glam::IVec3::new(region.cx * 16, region.min_cy * 16, region.cz * 16);
            let span = (CULL_REGION_COLUMNS * 16) as f32;
            let margin = glam::Vec3::splat(petramond_mesh::FOLIAGE_OVERHANG);
            let rmin = (corner - render_origin).as_vec3() - margin;
            let rmax =
                rmin + glam::Vec3::new(
                    span,
                    ((region.max_cy - region.min_cy + 1) * 16) as f32,
                    span,
                ) + margin * 2.0;
            let containment = frustum.aabb_containment(rmin, rmax);
            if containment == Containment::Outside || aabb_distance_sq(cam, rmin, rmax) > fog * fog
            {
                continue;
            }
            // A region wholly inside the frustum encloses its columns and
            // their sections: below here only the fog range can reject.
            let enclosed = containment == Containment::Inside;
            for entry in &cull_index[region.first as usize..region.last as usize] {
                if !Self::column_visible(entry, frustum, render_origin, cam, fog, enclosed) {
                    continue;
                }
                let column_pos = &entry.pos;
                let column = terrain_columns.at_mut(entry.slot);
                let first_section = order.len();
                let mut column_dist_sq = f32::INFINITY;
                let mut column_has_opaque = false;
                let mut column_has_model = false;
                let mut column_has_contact = false;
                let mut any_far_lod_active = false;
                // Every far-capable VISIBLE section agreeing on the far LOD is
                // what lets the whole column draw its far region in one call.
                let mut all_far_capable_are_far = true;
                for (_, section) in column.sections.iter_mut() {
                    if !Self::section_visible(section, frustum, render_origin, cam, fog, enclosed) {
                        continue;
                    }
                    let (ox, oy, oz) = section.origin;
                    let c = (glam::IVec3::new(ox, oy, oz) - render_origin).as_vec3()
                        + glam::Vec3::splat(8.0);
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
                    // The hysteresis state lives on the section record, so a
                    // section without a far mesh — nearly all of them, every frame
                    // — costs one field test, and one with a far mesh costs a
                    // field read and a field write rather than three hash probes.
                    let use_far_leaf_lod = section.has_far_lod && {
                        let now_active =
                            far_leaf_lod_active(dist_sq, (ox, oz), true, section.far_lod_active);
                        section.far_lod_active = now_active;
                        now_active
                    };
                    any_far_lod_active |= use_far_leaf_lod;
                    all_far_capable_are_far &= !section.has_far_lod || use_far_leaf_lod;
                    order.push(VisibleSection {
                        dist_sq,
                        column_pos: *column_pos,
                        column_slot: entry.slot,
                        opaque_batched: false,
                        model_batched: false,
                        use_far_leaf_lod,
                        opaque_vertex_start: section.opaque_vertex_start,
                        opaque_quads: section.opaque_vertex_count / 4,
                        opaque_tail_start: section.opaque_tail_start,
                        opaque_tail_quads: section.opaque_tail_count / 4,
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
                // Three ways a column's opaque geometry can be one draw: all
                // sections detailed (the whole buffer), all far (its leading
                // far region), or — mixed — none, and each section draws for
                // itself. The far case exists because a distant leafy column
                // is the common case at range, and before the two-region
                // packing it cost a draw per section.
                let detailed_batched =
                    column_has_opaque && !any_far_lod_active && column.opaque_quads > 0;
                let far_batched = column_has_opaque
                    && any_far_lod_active
                    && all_far_capable_are_far
                    && column.opaque_far_quads > 0;
                let opaque_batched = detailed_batched || far_batched;
                let model_batched = column_has_model && column.model_idx_count > 0;
                if opaque_batched {
                    opaque_columns.push((column_dist_sq, *column_pos, entry.slot, far_batched));
                }
                if model_batched {
                    model_columns.push((column_dist_sq, *column_pos, entry.slot));
                }
                if column_has_contact && column.contact_vertex_count > 0 {
                    contact_columns.push((column_dist_sq, *column_pos, entry.slot));
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
                        && (item.opaque_quads > 0
                            || (!item.use_far_leaf_lod && item.opaque_tail_quads > 0));
                    let model_left = !model_batched
                        && (item.model_idx_count > 0 || item.model_blend_idx_count > 0);
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
        }
        // Distance alone is not a total order: equidistant columns are common
        // (a symmetric view), so ties are broken on the column position and
        // then on the section's own place in the build order. The result is a
        // function of the scene alone, which the back-to-front transparent
        // pass needs — an order that varies run to run blends differently.
        //
        // Sorted as KEYS, not as records: a `VisibleSection` is far wider than
        // the three fields the comparison reads, and a comparison sort moves
        // its elements many times. Sorting `(key, index)` pairs and gathering
        // once moves each record exactly once.
        sort_scratch.clear();
        sort_scratch.extend(
            order
                .iter()
                .enumerate()
                .map(|(i, s)| (s.dist_sq, s.column_pos, i as u32)),
        );
        sort_scratch.sort_unstable_by(|a, b| {
            a.0.total_cmp(&b.0)
                .then_with(|| a.1.cmp(&b.1))
                .then_with(|| a.2.cmp(&b.2))
        });
        sorted.clear();
        sorted.extend(sort_scratch.iter().map(|&(_, _, i)| order[i as usize]));
        std::mem::swap(order, sorted);
        let by_dist_then_pos = |a: &(f32, ChunkPos, ColumnSlot),
                                b: &(f32, ChunkPos, ColumnSlot)| {
            a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1))
        };
        // `(distance, column)` is a total order — no two columns share a
        // position — so these need no stability guarantee.
        opaque_columns.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
        model_columns.sort_unstable_by(by_dist_then_pos);
        contact_columns.sort_unstable_by(by_dist_then_pos);
        self.terrain.planned_gpu_revision = self.terrain.gpu_revision;
        self.terrain.planned_view_key = Some(self.terrain.view_key);
        self.terrain.plan_any_model = any_model_visible;
        self.terrain.plan_any_transparent = any_transparent_visible;
        (
            RenderStats::default(),
            any_model_visible,
            any_transparent_visible,
        )
    }
}
