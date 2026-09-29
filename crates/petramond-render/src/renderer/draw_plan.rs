use super::*;
use crate::resources::SectionStream;
use petramond_mesh::QuadLayer;

mod draws;
mod occlusion;
pub(crate) use draws::{wanted_features as terrain_draw_features, QuadPass, TerrainDraws};
pub(crate) use occlusion::SectionOcclusion;

impl Renderer {
    #[inline]
    fn aabb_visible(
        min: glam::Vec3,
        max: glam::Vec3,
        frustum: &Frustum,
        cam_pos: glam::Vec3,
        fog: f32,
        enclosed: bool,
    ) -> bool {
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

    fn refresh_cull_index(&mut self) {
        let terrain = &mut self.terrain;
        if terrain.cull_index_revision == terrain.gpu_revision {
            return;
        }
        terrain.cull_index.clear();
        terrain.cull_index.reserve(terrain.columns.len());
        terrain.cull_index.extend(
            terrain
                .columns
                .iter()
                .map(|(pos, slot, column)| ColumnCull {
                    pos,
                    slot,
                    min_cy: column.cy_span.0,
                    max_cy: column.cy_span.1,
                }),
        );
        group_cull_regions(&mut terrain.cull_index, &mut terrain.cull_regions);
        terrain.occlusion.clear();
        for column in terrain.columns.values() {
            terrain.occlusion.insert_column(
                column.column_pos(),
                column.cy_span,
                column
                    .sections
                    .iter()
                    .map(|(pos, section)| (pos.cy, section.visibility)),
            );
        }
        terrain.cull_index_revision = terrain.gpu_revision;
    }

    pub(super) fn plan_draw_order(&mut self) {
        if self.terrain.planned_gpu_revision == self.terrain.gpu_revision
            && self.terrain.planned_view_key.as_ref() == Some(&self.terrain.view_key)
        {
            return;
        }
        self.refresh_cull_index();
        let frustum = &self.view.frustum;
        let render_origin = self.view.render_origin;
        let cam = self.view.cam_pos.relative_to(render_origin);
        let fog = self.terrain_cull_dist();
        let Self {
            terrain,
            device,
            queue,
            ..
        } = self;
        let camera_block = render_origin + cam.floor().as_ivec3();
        let camera_section = petramond_world::chunk::SectionPos::new(
            camera_block.x.div_euclid(16),
            camera_block.y.div_euclid(16),
            camera_block.z.div_euclid(16),
        );
        // A section's box reaches the fog only within this many columns of the camera's.
        let reach = ((fog + petramond_mesh::FOLIAGE_OVERHANG) / 16.0).ceil() as i32 + 2;
        let occluding = terrain.occlusion.flood(camera_section, reach, |pos| {
            let min = (glam::IVec3::new(pos.cx, pos.cy, pos.cz) * 16 - render_origin).as_vec3();
            Self::aabb_visible(min, min + glam::Vec3::splat(16.0), frustum, cam, fog, false)
        });
        let occlusion = &terrain.occlusion;
        let batch_hidden = terrain.draws.draws_directly();
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
                for (pos, section) in column.sections.iter_mut() {
                    if !Self::section_visible(section, frustum, render_origin, cam, fog, enclosed)
                        || (occluding && !occlusion.is_visible(*pos))
                    {
                        visible.hidden_opaque |= !section.span(SectionStream::OpaqueFar).is_empty();
                        continue;
                    }
                    let (ox, oy, oz) = section.origin;
                    let c = (glam::IVec3::new(ox, oy, oz) - render_origin).as_vec3()
                        + glam::Vec3::splat(8.0);
                    let dist_sq = (cam - c).length_squared();
                    column_dist_sq = column_dist_sq.min(dist_sq);
                    let has_model = section.has_model();
                    visible.has_opaque |= !section.span(SectionStream::OpaqueFar).is_empty();
                    visible.has_model |= has_model;
                    visible.has_contact |= !section.span(SectionStream::Contact).is_empty();
                    plan.any_model |= has_model;
                    plan.any_transparent |= section.has_alpha();
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
                        spans: section.spans,
                    });
                }
                let batch = batch_column(
                    visible,
                    ColumnStreams {
                        opaque_quads: column.opaque_quads(),
                        opaque_far_quads: column.opaque_far_quads(),
                        model_idx_count: column.region(SectionStream::ModelIndices).count,
                        contact_vertex_count: column.region(SectionStream::Contact).count,
                    },
                    batch_hidden,
                );
                if let Some(far) = batch.opaque {
                    plan.opaque_columns
                        .push((column_dist_sq, column_pos, entry.slot, far));
                }
                if batch.model {
                    plan.model_columns
                        .push((column_dist_sq, column_pos, entry.slot));
                }
                if batch.contact {
                    plan.contact_columns
                        .push((column_dist_sq, column_pos, entry.slot));
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
        build_terrain_draws(
            &mut terrain.draws,
            &terrain.geometry,
            &terrain.columns,
            plan,
        );
        terrain.draws.finish(device, queue);
        terrain.planned_gpu_revision = terrain.gpu_revision;
        terrain.planned_view_key = Some(terrain.view_key);
    }
}

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
    let max =
        min + glam::Vec3::new(
            span,
            ((region.max_cy - region.min_cy + 1) * 16) as f32,
            span,
        ) + margin * 2.0;
    let containment = frustum.aabb_containment(min, max);
    if containment == Containment::Outside || aabb_distance_sq(cam, min, max) > fog * fog {
        return None;
    }
    Some(containment == Containment::Inside)
}

#[derive(Copy, Clone, Debug)]
struct VisibleColumn {
    has_opaque: bool,
    has_model: bool,
    has_contact: bool,
    any_far_lod: bool,
    all_far_capable_are_far: bool,
    hidden_opaque: bool,
}

impl Default for VisibleColumn {
    fn default() -> Self {
        Self {
            has_opaque: false,
            has_model: false,
            has_contact: false,
            any_far_lod: false,
            all_far_capable_are_far: true,
            hidden_opaque: false,
        }
    }
}

#[derive(Copy, Clone, Debug)]
struct ColumnStreams {
    opaque_quads: u32,
    opaque_far_quads: u32,
    model_idx_count: u32,
    contact_vertex_count: u32,
}

#[derive(Copy, Clone, Debug, PartialEq, Eq)]
struct ColumnBatch {
    opaque: Option<bool>,
    model: bool,
    contact: bool,
}

/// Three ways a column's opaque geometry can be one draw: all visible
/// sections detailed (the whole buffer), all far-capable ones far (its
/// leading far region), or — mixed — none, and each section draws for
/// itself. The far case exists because a distant leafy column is the common
/// case at range, and before the two-region packing it cost a draw per
/// section. A whole-column draw also covers culled sections, so it is taken
/// only when none holds opaque geometry — unless `batch_hidden` (every draw
/// is a CPU call on this device, and one column draw beats many).
fn batch_column(visible: VisibleColumn, streams: ColumnStreams, batch_hidden: bool) -> ColumnBatch {
    let batchable = visible.has_opaque && (batch_hidden || !visible.hidden_opaque);
    let detailed = batchable && !visible.any_far_lod && streams.opaque_quads > 0;
    let far = batchable
        && visible.any_far_lod
        && visible.all_far_capable_are_far
        && streams.opaque_far_quads > 0;
    ColumnBatch {
        opaque: (detailed || far).then_some(far),
        model: visible.has_model && streams.model_idx_count > 0,
        contact: visible.has_contact && streams.contact_vertex_count > 0,
    }
}

fn draws_for_itself(section: &VisibleSection) -> bool {
    let has = |stream| !section.span(stream).is_empty();
    let opaque_left = !section.opaque_batched
        && (has(SectionStream::OpaqueFar)
            || (!section.use_far_leaf_lod && has(SectionStream::OpaqueTail)));
    let model_left = !section.model_batched
        && (has(SectionStream::ModelIndices) || has(SectionStream::ModelBlendIndices));
    opaque_left
        || model_left
        || has(SectionStream::Transparent)
        || has(SectionStream::TransparentTwoSided)
        || has(SectionStream::Translucent)
}

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

fn sort_columns(plan: &mut TerrainPlan) {
    let by_dist_then_pos = |a: &(f32, ChunkPos, ColumnSlot), b: &(f32, ChunkPos, ColumnSlot)| {
        a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1))
    };
    plan.opaque_columns
        .sort_unstable_by(|a, b| a.0.total_cmp(&b.0).then_with(|| a.1.cmp(&b.1)));
    plan.model_columns.sort_unstable_by(by_dist_then_pos);
    plan.contact_columns.sort_unstable_by(by_dist_then_pos);
}

/// Fill the terrain quad nodes' draw lists from the sorted plan: the opaque
/// node draws whole columns near→far and then every section no column draw
/// covered (its far range, plus its leaf tail at detailed LOD); translucent
/// blocks draw near→far and fluids far→near, sides and two-sided tops in
/// their section's place.
fn build_terrain_draws(
    draws: &mut TerrainDraws,
    arenas: &crate::resources::TerrainArenas,
    columns: &ColumnStore,
    plan: &TerrainPlan,
) {
    draws.clear();
    let opaque = draws.list_mut(QuadPass::Opaque);
    for &(_, _, slot, far) in &plan.opaque_columns {
        let column = columns.at(slot);
        let quads = if far {
            column.opaque_far_quads()
        } else {
            column.opaque_quads()
        };
        opaque.push(arenas, column, QuadLayer::Opaque, 0, quads);
    }
    for item in plan.sections.iter().filter(|item| !item.opaque_batched) {
        let column = columns.at(item.column_slot);
        opaque.push_span(
            arenas,
            column,
            QuadLayer::Opaque,
            item.span(SectionStream::OpaqueFar),
        );
        if !item.use_far_leaf_lod {
            opaque.push_span(
                arenas,
                column,
                QuadLayer::Opaque,
                item.span(SectionStream::OpaqueTail),
            );
        }
    }
    let translucent = draws.list_mut(QuadPass::Translucent);
    for item in &plan.sections {
        let column = columns.at(item.column_slot);
        translucent.push_span(
            arenas,
            column,
            QuadLayer::Translucent,
            item.span(SectionStream::Translucent),
        );
    }
    let transparent = draws.list_mut(QuadPass::Transparent);
    for item in plan.sections.iter().rev() {
        let column = columns.at(item.column_slot);
        transparent.push_span(
            arenas,
            column,
            QuadLayer::Transparent,
            item.span(SectionStream::Transparent),
        );
        transparent.push_span(
            arenas,
            column,
            QuadLayer::TransparentTwoSided,
            item.span(SectionStream::TransparentTwoSided),
        );
    }
}

#[cfg(test)]
mod tests;
