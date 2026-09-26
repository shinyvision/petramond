//! Frustum cull + depth sort: which sections and whole columns each terrain
//! node draws this frame, and in what order. The terrain nodes consume the
//! plan without re-deciding any of it.
//!
//! The decisions themselves — the region grouping of the cull index, the
//! region test, a column's whole-column draws, which sections still draw for
//! themselves, the deterministic sort — are free functions over plain data,
//! so the tests pin them without a GPU; `plan_draw_order` only walks the
//! columns and applies them.

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

    /// Refresh the dense cull mirror when the column set changed.
    ///
    /// Walking the column map is exactly the cost this mirror exists to keep
    /// off the per-frame path, so it happens once per change. The grouping is
    /// what makes the per-frame scan sublinear: at render distance 32 a view
    /// rejects most of ~3 200 columns, and one region test rejects up to 64 of
    /// them at once.
    fn refresh_cull_index(&mut self) {
        let terrain = &mut self.terrain;
        if terrain.cull_index_revision == terrain.gpu_revision {
            return;
        }
        terrain.cull_index.clear();
        terrain.cull_index.reserve(terrain.columns.len());
        terrain
            .cull_index
            .extend(terrain.columns.iter().map(|(pos, slot, column)| ColumnCull {
                pos,
                slot,
                min_cy: column.cy_span.0,
                max_cy: column.cy_span.1,
            }));
        group_cull_regions(&mut terrain.cull_index, &mut terrain.cull_regions);
        terrain.cull_index_revision = terrain.gpu_revision;
    }

    /// Frustum-cull + depth-sort the visible terrain into
    /// [`TerrainPass::plan`]. Reuses last frame's plan outright when neither
    /// the view nor the column set changed.
    pub(super) fn plan_draw_order(&mut self) {
        if self.terrain.planned_gpu_revision == self.terrain.gpu_revision
            && self.terrain.planned_view_key.as_ref() == Some(&self.terrain.view_key)
        {
            return;
        }
        self.refresh_cull_index();
        // Cull + depth-sort the visible sections once. The opaque node draws
        // nearest-first so the GPU's early-Z rejects occluded fragments before
        // the fragment shader runs; the fluid node draws farthest-first for
        // correct back-to-front alpha.
        let frustum = &self.view.frustum;
        let render_origin = self.view.render_origin;
        // Cull and sort in render-local space, like the GPU draws.
        let cam = self.view.cam_pos.relative_to(render_origin);
        let fog = self.terrain_cull_dist();
        let Self { terrain, .. } = self;
        let plan = &mut terrain.plan;
        let cull_index = &terrain.cull_index;
        let terrain_columns = &mut terrain.columns;
        plan.clear();
        for region in &terrain.cull_regions {
            let Some(enclosed) = region_visible(region, frustum, render_origin, cam, fog) else {
                continue;
            };
            for entry in &cull_index[region.first as usize..region.last as usize] {
                if !Self::column_visible(entry, frustum, render_origin, cam, fog, enclosed) {
                    continue;
                }
                let column_pos = entry.pos;
                let column = terrain_columns.at_mut(entry.slot);
                let first_section = plan.sections.len();
                let mut column_dist_sq = f32::INFINITY;
                let mut visible = VisibleColumn::default();
                for (_, section) in column.sections.iter_mut() {
                    if !Self::section_visible(section, frustum, render_origin, cam, fog, enclosed) {
                        continue;
                    }
                    let (ox, oy, oz) = section.origin;
                    let c = (glam::IVec3::new(ox, oy, oz) - render_origin).as_vec3()
                        + glam::Vec3::splat(8.0);
                    let dist_sq = (cam - c).length_squared();
                    column_dist_sq = column_dist_sq.min(dist_sq);
                    let has_model =
                        section.model_idx_count > 0 || section.model_blend_idx_count > 0;
                    visible.has_opaque |= section.opaque_vertex_count > 0;
                    visible.has_model |= has_model;
                    // Contact visibility is its OWN presence bit: a multi-cell
                    // model's contact triangles can sit in a section whose model
                    // index range is empty.
                    visible.has_contact |= section.contact_vertex_count > 0;
                    plan.any_model |= has_model;
                    plan.any_transparent |= section.transparent_vertex_count > 0
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
                    visible.any_far_lod |= use_far_leaf_lod;
                    visible.all_far_capable_are_far &= !section.has_far_lod || use_far_leaf_lod;
                    plan.sections.push(VisibleSection {
                        dist_sq,
                        column_pos,
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
                let batch = batch_column(
                    visible,
                    ColumnStreams {
                        opaque_quads: column.opaque_quads,
                        opaque_far_quads: column.opaque_far_quads,
                        model_idx_count: column.model_idx_count,
                        contact_vertex_count: column.contact_vertex_count,
                    },
                );
                if let Some(far) = batch.opaque {
                    plan.opaque_columns.push((column_dist_sq, column_pos, entry.slot, far));
                }
                if batch.model {
                    plan.model_columns.push((column_dist_sq, column_pos, entry.slot));
                }
                if batch.contact {
                    plan.contact_columns.push((column_dist_sq, column_pos, entry.slot));
                }
                retain_uncovered(&mut plan.sections, first_section, batch);
            }
        }
        sort_sections(
            &mut plan.sections,
            &mut terrain.sort_scratch,
            &mut terrain.sorted_scratch,
        );
        sort_columns(plan);
        terrain.planned_gpu_revision = terrain.gpu_revision;
        terrain.planned_view_key = Some(terrain.view_key);
    }
}

/// Sort the cull index by region, then by position inside it, and rebuild
/// `regions` over it: each region a contiguous run of `index` with its
/// bounding section span. The order is a function of the column set alone,
/// never of the column map's iteration order.
fn group_cull_regions(index: &mut [ColumnCull], regions: &mut Vec<CullRegion>) {
    let region_of = |e: &ColumnCull| (e.pos.cx >> CULL_REGION_SHIFT, e.pos.cz >> CULL_REGION_SHIFT);
    index.sort_unstable_by_key(|e| (region_of(e), e.pos.cx, e.pos.cz));
    regions.clear();
    let mut start = 0usize;
    while start < index.len() {
        let key = region_of(&index[start]);
        let mut end = start;
        let mut min_cy = i32::MAX;
        let mut max_cy = i32::MIN;
        while end < index.len() && region_of(&index[end]) == key {
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
}

/// The region test: `None` when the whole region is outside the frustum or
/// the fog range, else whether it is wholly INSIDE the frustum — in which
/// case it encloses its columns and their sections, and below it only the
/// fog range can reject.
fn region_visible(
    region: &CullRegion,
    frustum: &Frustum,
    render_origin: glam::IVec3,
    cam: glam::Vec3,
    fog: f32,
) -> Option<bool> {
    if region.min_cy > region.max_cy {
        return None;
    }
    let corner = glam::IVec3::new(region.cx * 16, region.min_cy * 16, region.cz * 16);
    let span = (CULL_REGION_COLUMNS * 16) as f32;
    let margin = glam::Vec3::splat(petramond_mesh::FOLIAGE_OVERHANG);
    let min = (corner - render_origin).as_vec3() - margin;
    let max = min
        + glam::Vec3::new(span, ((region.max_cy - region.min_cy + 1) * 16) as f32, span)
        + margin * 2.0;
    let containment = frustum.aabb_containment(min, max);
    if containment == Containment::Outside || aabb_distance_sq(cam, min, max) > fog * fog {
        return None;
    }
    Some(containment == Containment::Inside)
}

/// What a column's VISIBLE sections add up to: the inputs of its
/// whole-column draw decisions.
#[derive(Copy, Clone, Debug)]
struct VisibleColumn {
    has_opaque: bool,
    has_model: bool,
    has_contact: bool,
    /// Some visible section draws its far LOD.
    any_far_lod: bool,
    /// Every far-capable visible section draws its far LOD.
    all_far_capable_are_far: bool,
}

impl Default for VisibleColumn {
    fn default() -> Self {
        Self {
            has_opaque: false,
            has_model: false,
            has_contact: false,
            any_far_lod: false,
            all_far_capable_are_far: true,
        }
    }
}

/// A packed column's whole-stream sizes.
#[derive(Copy, Clone, Debug)]
struct ColumnStreams {
    opaque_quads: u32,
    opaque_far_quads: u32,
    model_idx_count: u32,
    contact_vertex_count: u32,
}

/// The whole-column draws a column makes this frame.
#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ColumnBatch {
    /// One opaque draw: `Some(far)` — its leading far region (`true`) or its
    /// whole stream (`false`); `None` leaves every section to draw itself.
    opaque: Option<bool>,
    /// One draw of the whole model stream.
    model: bool,
    /// One draw of the whole contact-shadow stream.
    contact: bool,
}

/// Three ways a column's opaque geometry can be one draw: all visible
/// sections detailed (the whole buffer), all far-capable ones far (its
/// leading far region), or — mixed — none, and each section draws for
/// itself. The far case exists because a distant leafy column is the common
/// case at range, and before the two-region packing it cost a draw per
/// section.
fn batch_column(visible: VisibleColumn, streams: ColumnStreams) -> ColumnBatch {
    let detailed = visible.has_opaque && !visible.any_far_lod && streams.opaque_quads > 0;
    let far = visible.has_opaque
        && visible.any_far_lod
        && visible.all_far_capable_are_far
        && streams.opaque_far_quads > 0;
    ColumnBatch {
        opaque: (detailed || far).then_some(far),
        model: visible.has_model && streams.model_idx_count > 0,
        contact: visible.has_contact && streams.contact_vertex_count > 0,
    }
}

/// Whether a section still draws anything for itself once its column's
/// whole-column draws are decided.
fn draws_for_itself(section: &VisibleSection) -> bool {
    let opaque_left = !section.opaque_batched
        && (section.opaque_quads > 0
            || (!section.use_far_leaf_lod && section.opaque_tail_quads > 0));
    let model_left = !section.model_batched
        && (section.model_idx_count > 0 || section.model_blend_idx_count > 0);
    opaque_left
        || model_left
        || section.transparent_quads > 0
        || section.transparent_ts_quads > 0
        || section.translucent_quads > 0
}

/// Stamp one column's batch onto its sections (`sections[first..]`) and drop
/// the ones whose every layer is empty or covered by a whole-column draw:
/// the per-section loops would skip them, and they are the great majority —
/// carrying them costs a move in the sort and a rejected branch in four
/// terrain nodes.
fn retain_uncovered(sections: &mut Vec<VisibleSection>, first: usize, batch: ColumnBatch) {
    let mut w = first;
    for r in first..sections.len() {
        let mut item = sections[r];
        item.opaque_batched = batch.opaque.is_some();
        item.model_batched = batch.model;
        if draws_for_itself(&item) {
            sections[w] = item;
            w += 1;
        }
    }
    sections.truncate(w);
}

/// Sort the sections near → far. Distance alone is not a total order:
/// equidistant columns are common (a symmetric view), so ties break on the
/// column position and then on the section's place in the build order. The
/// result is a function of the scene alone, which the back-to-front fluid
/// node needs — an order that varies run to run blends differently.
///
/// Sorted as KEYS, not as records: a `VisibleSection` is far wider than the
/// three fields the comparison reads, and a comparison sort moves its
/// elements many times. Sorting `(key, index)` pairs and gathering once moves
/// each record exactly once.
fn sort_sections(
    sections: &mut Vec<VisibleSection>,
    keys: &mut Vec<(f32, ChunkPos, u32)>,
    sorted: &mut Vec<VisibleSection>,
) {
    keys.clear();
    keys.extend(
        sections
            .iter()
            .enumerate()
            .map(|(i, s)| (s.dist_sq, s.column_pos, i as u32)),
    );
    keys.sort_unstable_by(|a, b| {
        a.0.total_cmp(&b.0)
            .then_with(|| a.1.cmp(&b.1))
            .then_with(|| a.2.cmp(&b.2))
    });
    sorted.clear();
    sorted.extend(keys.iter().map(|&(_, _, i)| sections[i as usize]));
    std::mem::swap(sections, sorted);
}

/// Sort the whole-column draw lists near → far. `(distance, column)` is a
/// total order — no two columns share a position — so these need no
/// stability guarantee.
fn sort_columns(plan: &mut TerrainPlan) {
    let by_dist_then_pos = |a: &(f32, ChunkPos, ColumnSlot), b: &(f32, ChunkPos, ColumnSlot)| {
        a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1))
    };
    plan.opaque_columns.sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    plan.model_columns.sort_unstable_by(by_dist_then_pos);
    plan.contact_columns.sort_unstable_by(by_dist_then_pos);
}

#[cfg(test)]
mod tests;
