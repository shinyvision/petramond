//! GPU render-pass encoding for [`Renderer`], lifted verbatim out of `render`.
//!
//! `plan_draw_order` frustum-culls + depth-sorts the visible chunks; `encode_passes`
//! records every pass in the SAME order with the SAME load/store ops. `render`
//! stays the thin orchestrator (one encoder, one submit, one present). The pass
//! helper `color_depth_pass` and visibility tests live here too.

use super::*;

/// Begin one render pass with a single color attachment over `view` and an
/// optional depth attachment over `depth`. Collapses the near-identical
/// `begin_render_pass` boilerplate every pass used to spell out — only the parts
/// that actually vary are parameters: the debug `label`, the color load-op
/// (`Clear` for the sky, `Load` everywhere after), and `depth_load`:
/// - `Some(load_op)` → attach `depth` with that depth load-op (always store),
///   no stencil — the world / overlay / hand passes.
/// - `None` → no depth attachment — the sky, crosshair, and UI passes.
///
/// The store-ops, `depth_slice`, `resolve_target`, `timestamp_writes`, and
/// `occlusion_query_set` are the same for every pass, so they live here.
fn color_depth_pass<'a>(
    encoder: &'a mut wgpu::CommandEncoder,
    view: &'a wgpu::TextureView,
    depth: &'a wgpu::TextureView,
    label: &str,
    color_load: wgpu::LoadOp<wgpu::Color>,
    depth_load: Option<wgpu::LoadOp<f32>>,
) -> wgpu::RenderPass<'a> {
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: color_load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: depth_load.map(|load| wgpu::RenderPassDepthStencilAttachment {
            view: depth,
            depth_ops: Some(wgpu::Operations {
                load,
                store: wgpu::StoreOp::Store,
            }),
            stencil_ops: None,
        }),
        timestamp_writes: None,
        occlusion_query_set: None,
    })
}

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
        let mut min_cy = i32::MAX;
        let mut max_cy = i32::MIN;
        for &(sp, _) in &column.sections {
            min_cy = min_cy.min(sp.cy);
            max_cy = max_cy.max(sp.cy);
        }
        if min_cy > max_cy {
            return false;
        }
        let ox = column_pos.cx * 16;
        let oz = column_pos.cz * 16;
        let min = glam::Vec3::new(ox as f32, (min_cy * 16) as f32, oz as f32);
        let max = glam::Vec3::new((ox + 16) as f32, ((max_cy + 1) * 16) as f32, (oz + 16) as f32);
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
        if self.terrain_planned_gpu_revision == self.terrain_gpu_revision
            && self.terrain_planned_view_key.as_ref() == Some(&self.terrain_view_key)
        {
            return (
                RenderStats::default(),
                self.terrain_plan_any_model,
                self.terrain_plan_any_transparent,
            );
        }
        // Cull + depth-sort the visible sections once. The opaque pass draws nearest-first
        // so the GPU's early-Z rejects occluded fragments before the fragment shader runs;
        // the transparent pass draws farthest-first for correct back-to-front alpha.
        let cam = self.cam_pos;
        let frustum = self.frustum;
        let render_origin = self.render_origin;
        let fog = self.terrain_cull_dist();
        let terrain_columns = &self.terrain_columns;
        let far_leaf_lod_state = &mut self.far_leaf_lod_state;
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
                column_has_opaque |= section.opaque_idx_count > 0;
                column_has_model |= section.model_idx_count > 0;
                // Contact visibility is its OWN presence bit: a multi-cell
                // model's contact triangles can sit in a section whose model
                // index range is empty.
                column_has_contact |= section.contact_vertex_count > 0;
                any_model_visible |= section.model_idx_count > 0;
                any_transparent_visible |=
                    section.transparent_idx_count > 0 || section.translucent_idx_count > 0;
                let was_far_lod_active = far_leaf_lod_state.get(&sp).copied().unwrap_or(false);
                let use_far_leaf_lod = far_leaf_lod_active(
                    dist_sq,
                    (section.origin.0, section.origin.2),
                    section.far_opaque_idx_count > 0,
                    was_far_lod_active,
                );
                if use_far_leaf_lod {
                    far_leaf_lod_state.insert(sp, true);
                } else {
                    far_leaf_lod_state.remove(&sp);
                }
                any_far_lod_active |= use_far_leaf_lod;
                order.push(VisibleSection {
                    dist_sq,
                    column_pos: *column_pos,
                    opaque_batched: false,
                    model_batched: false,
                    use_far_leaf_lod,
                    opaque_index_start: section.opaque_index_start,
                    opaque_idx_count: section.opaque_idx_count,
                    far_opaque_index_start: section.far_opaque_index_start,
                    far_opaque_idx_count: section.far_opaque_idx_count,
                    transparent_index_start: section.transparent_index_start,
                    transparent_idx_count: section.transparent_idx_count,
                    translucent_index_start: section.translucent_index_start,
                    translucent_idx_count: section.translucent_idx_count,
                    model_index_start: section.model_index_start,
                    model_idx_count: section.model_idx_count,
                });
            }
            if column_has_opaque && !any_far_lod_active && column.opaque_idx_count > 0 {
                for item in &mut order[first_section..] {
                    item.opaque_batched = true;
                }
                opaque_columns.push((column_dist_sq, *column_pos));
            }
            if column_has_model && column.model_idx_count > 0 {
                for item in &mut order[first_section..] {
                    item.model_batched = true;
                }
                model_columns.push((column_dist_sq, *column_pos));
            }
            if column_has_contact && column.contact_vertex_count > 0 {
                contact_columns.push((column_dist_sq, *column_pos));
            }
        }
        order.sort_by(|a, b| a.dist_sq.total_cmp(&b.dist_sq));
        opaque_columns.sort_by(|a, b| a.0.total_cmp(&b.0));
        model_columns.sort_by(|a, b| a.0.total_cmp(&b.0));
        contact_columns.sort_by(|a, b| a.0.total_cmp(&b.0));
        self.terrain_planned_gpu_revision = self.terrain_gpu_revision;
        self.terrain_planned_view_key = Some(self.terrain_view_key.clone());
        self.terrain_plan_any_model = any_model_visible;
        self.terrain_plan_any_transparent = any_transparent_visible;
        (
            RenderStats::default(),
            any_model_visible,
            any_transparent_visible,
        )
    }

    /// Encode every GPU render pass for this frame, in order, with byte-for-byte
    /// identical load/store ops. Reads the baked per-frame buffers off `self`;
    /// mutates only the passed `stats`.
    #[allow(clippy::too_many_arguments)]
    pub(super) fn encode_passes(
        &self,
        enc: &mut wgpu::CommandEncoder,
        swapchain: &wgpu::TextureView,
        order: &[VisibleSection],
        opaque_columns: &[(f32, ChunkPos)],
        model_columns: &[(f32, ChunkPos)],
        contact_columns: &[(f32, ChunkPos)],
        stats: &mut RenderStats,
        any_model_visible: bool,
        any_transparent_visible: bool,
    ) {
        // The world (opaque → sky → … → hand) renders into the offscreen scene
        // target; the grade pass then reads it and writes the swapchain, and
        // screen chrome (crosshair, UI) draws over the graded image so its
        // colours stay exact. With grade off at native scale the world skips
        // the round-trip and renders straight into the swapchain.
        let direct = self.direct_to_swapchain();
        let view = if direct { swapchain } else { &self.scene_color };
        let cc = self.clear_color;
        // OPAQUE PASS: the visible chunk terrain, near→far for early-Z. The first
        // pass of the frame: CLEARS color (to the fog colour) and depth.
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "opaque pass",
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: cc[0] as f64,
                    g: cc[1] as f64,
                    b: cc[2] as f64,
                    a: 1.0,
                }),
                Some(wgpu::LoadOp::Clear(1.0)),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            pass.set_pipeline(&self.opaque_pipe);
            for (_, pos) in opaque_columns {
                let Some(col) = self.terrain_columns.get(pos) else {
                    continue;
                };
                if col.opaque_idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (&col.opaque_vbuf, &col.opaque_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(1, col.origin_vbuf.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    stats.opaque_draws += 1;
                    stats.opaque_indices += col.opaque_idx_count as u64;
                    pass.draw_indexed(0..col.opaque_idx_count, 0, 0..1);
                }
            }
            for item in order.iter() {
                if item.opaque_batched {
                    continue;
                }
                let Some(col) = self.terrain_columns.get(&item.column_pos) else {
                    continue;
                };
                // near -> far (early-Z)
                let (vbuf, ibuf, index_start, idx_count) = if item.use_far_leaf_lod {
                    (
                        &col.far_opaque_vbuf,
                        &col.far_opaque_ibuf,
                        item.far_opaque_index_start,
                        item.far_opaque_idx_count,
                    )
                } else {
                    (
                        &col.opaque_vbuf,
                        &col.opaque_ibuf,
                        item.opaque_index_start,
                        item.opaque_idx_count,
                    )
                };
                if idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (vbuf, ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(1, col.origin_vbuf.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    stats.opaque_draws += 1;
                    stats.opaque_indices += idx_count as u64;
                    pass.draw_indexed(index_start..index_start + idx_count, 0, 0..1);
                }
            }
        }
        // CONTACT-SHADOW PASS: the models' soft floor stamps, multiplied over the
        // opaque terrain just drawn. Depth read-only (LessEqual + its own
        // coplanar bias against the supporting top face). Drawing BEFORE the sky
        // is a safety contract: the stamp writes no depth, so if its supporting
        // terrain section was culled while an adjacent model section stayed
        // visible, the sky's far-plane LessEqual draw replaces the orphaned
        // darkening with sky instead of smudging the background. One whole-buffer
        // draw per visible contact-bearing column — the stream is sparse and
        // needs no per-section ranges.
        if !contact_columns.is_empty() {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "contact shadow pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_pipeline(&self.contact_pipe);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            for (_, pos) in contact_columns {
                let Some(col) = self.terrain_columns.get(pos) else {
                    continue;
                };
                if col.contact_vertex_count == 0 {
                    continue;
                }
                if let Some(vb) = &col.contact_vbuf {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.draw(0..col.contact_vertex_count, 0..1);
                }
            }
        }
        // SKY PASS: full-screen background triangle at exactly the far plane,
        // AFTER opaque so its LessEqual depth test shades only the pixels no
        // terrain covered (the sky fs is the priciest full-screen shader). The
        // sky shader owns celestials and any day/night colour.
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "sky pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_pipeline(&self.sky_pipe);
            pass.set_bind_group(0, &self.sky_bind, &[]);
            pass.set_bind_group(1, &self.sky_texture_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // MODEL PASS: bbmodel-block geometry (explicit-UV, sampling the model atlas),
        // drawn per visible chunk with the mob pipeline (own texture + the same
        // underwater/fog the world uses) over depth from the opaque pass — so a placed
        // model occludes and is occluded by terrain like any block. Most chunks have no
        // model geometry, so this is usually a no-op loop.
        if any_model_visible || self.item_model_entity_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "model pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.model_atlas_bind, &[]);
            // Chunk model geometry draws with the world-model pipeline: its
            // vertices carry (sky, block) light so the shader applies the
            // day/night sky scale (meshes don't rebake at sunset).
            pass.set_pipeline(&self.world_model_pipe);
            for (_, pos) in model_columns {
                let Some(col) = self.terrain_columns.get(pos) else {
                    continue;
                };
                if col.model_idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..col.model_idx_count, 0, 0..1);
                }
            }
            for item in order.iter() {
                if item.model_batched || item.model_idx_count == 0 {
                    continue;
                }
                let Some(col) = self.terrain_columns.get(&item.column_pos) else {
                    continue;
                };
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(
                        item.model_index_start..item.model_index_start + item.model_idx_count,
                        0,
                        0..1,
                    );
                }
            }
            // Dropped bbmodel items (world-space, same model atlas; ItemVertex
            // with per-frame CPU-baked light, so they stay on the mob-layout
            // pipeline).
            pass.set_pipeline(&self.model_pipe);
            self.item_model_entity_draw.draw(&mut pass);
        }
        // ITEM-ENTITY PASS (§8 2b): dropped items as spinning cubes (the EXISTING
        // opaque pipeline, terrain atlas array) plus extruded sprite slabs (the
        // mob-layout pipeline over the 2D block atlas — their per-texel wall UVs
        // need explicit UVs). Load color + depth, depth test + write so items
        // occlude and are occluded by terrain.
        if self.item_entity_draw.index_count > 0 || self.item_sprite_entity_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "item entity pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            self.item_entity_draw.draw(&mut pass);
            if self.item_sprite_entity_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.item_sprite_entity_draw.draw(&mut pass);
            }
        }
        // CHEST + DOOR PASS: placed chests (inset body + hinged lid) and doors (2-tall
        // hinged slab) drawn as full opaque geometry by the EXISTING opaque pipeline
        // with the same uniform + atlas binds, loading color + depth so they occlude and
        // are occluded by terrain — exactly like the item-entity pass above.
        if self.chest_draw.index_count > 0 || self.door_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "chest+door pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            self.chest_draw.draw(&mut pass);
            self.door_draw.draw(&mut pass);
        }
        // MOB PASS: animated entity models, one draw per visible species. Loads color
        // + depth (test + WRITE) so mobs occlude and are occluded by terrain — like
        // the item-entity / chest passes — but binds each species' OWN texture at
        // group(1) (not the block atlas); the mob pipeline (set by each DynamicDraw)
        // uses explicit-UV vertices so a model's arbitrary sub-rect UVs sample its
        // own sheet.
        if self.mob_gpu.iter().any(|g| g.draw.index_count > 0)
            || self.player_gpu.draw.index_count > 0
            || self.player_item_draw.index_count > 0
            || self.player_model_item_draw.index_count > 0
            || self.player_block_item_draw.index_count > 0
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "mob pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            for g in &self.mob_gpu {
                if g.draw.index_count == 0 {
                    continue;
                }
                pass.set_bind_group(1, &g.bind, &[]);
                g.draw.draw(&mut pass);
            }
            // Player bodies — the local third-person body and every remote
            // player, one combined stream (shared skin texture, mob pipeline)…
            if self.player_gpu.draw.index_count > 0 {
                pass.set_bind_group(1, &self.player_gpu.bind, &[]);
                self.player_gpu.draw.draw(&mut pass);
            }
            // …their extruded-sprite held items (2D atlas)…
            if self.player_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.player_item_draw.draw(&mut pass);
            }
            // …their bbmodel held items (model atlas)…
            if self.player_model_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                self.player_model_item_draw.draw(&mut pass);
            }
            // …and their held block mini-cubes (opaque pipeline + terrain
            // atlas array).
            if self.player_block_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_array_bind, &[]);
                self.player_block_item_draw.draw(&mut pass);
            }
        }
        // TRANSLUCENT-BLOCK PASS: ice — alpha-blended but depth-WRITING, so a
        // sheet of translucent cubes resolves its own face order through the
        // depth buffer. Encoded BEFORE the break overlay so a crack decal on a
        // mined ice block draws ON TOP of the ice (the decal's biased
        // LessEqual wins on the depth the ice just wrote) instead of being
        // washed out by the ice blending over it.
        if any_transparent_visible {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "translucent block pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            pass.set_pipeline(&self.translucent_pipe);
            for item in order.iter() {
                if item.translucent_idx_count == 0 {
                    continue;
                }
                let Some(col) = self.terrain_columns.get(&item.column_pos) else {
                    continue;
                };
                // near -> far: depth-writing, so early-Z applies like opaque.
                if let (Some(vb), Some(ib)) = (&col.translucent_vbuf, &col.translucent_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(1, col.origin_vbuf.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    stats.transparent_draws += 1;
                    stats.transparent_indices += item.translucent_idx_count as u64;
                    pass.draw_indexed(
                        item.translucent_index_start
                            ..item.translucent_index_start + item.translucent_idx_count,
                        0,
                        0..1,
                    );
                }
            }
        }
        // BREAK-OVERLAY PASS: the destroy crack over the targeted block. Drawn
        // AFTER translucent blocks (the crack must sit on mined ice) but BEFORE
        // the transparent water pass — it is a decal on the block, so water must
        // be able to blend in front of it (a crack on a submerged block shows
        // THROUGH the water, not over it). MULTIPLY blend; depth LessEqual /
        // no-write over a cube built COINCIDENT with the block faces (no inflation,
        // so the decal never misaligns), with a small polygon offset toward the
        // camera (BREAK_DEPTH_BIAS) so it wins the depth tie cleanly. Reuses
        // uniform_bind (view_proj + uv_rects) + atlas_bind.
        if self.break_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "break overlay pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.break_draw.draw(&mut pass);
        }
        // PARTICLE PASS (§8 3b): tiny 3D terrain particle cubes. Drawn BEFORE the
        // transparent water pass (but after the break overlay, so they sit in front
        // of the crack): they are alpha-CUTOUT solids that DEPTH-TEST + DEPTH-WRITE,
        // so water blends over the ones behind it (underwater dust reads as
        // submerged) while ones in front of the water still occlude it. Reuses
        // uniform_bind + atlas_bind. 24 verts / 36 indices per cube.
        if self.particle_draw.vertex_count > 0 {
            let verts_per_cube = crate::render::particles::VERTS_PER_CUBE as u32;
            let idx_per_cube = crate::render::particles::INDICES_PER_CUBE as u32;
            // Cube boundaries: block flecks occupy [0..block_cubes), model flecks the rest.
            let total_cubes = self.particle_draw.vertex_count / verts_per_cube;
            let block_cubes = self.particle_block_vertex_count / verts_per_cube;
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "particle pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            // Block-atlas flecks: the leading index range via the standard draw.
            if block_cubes > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.particle_draw
                    .draw(&mut pass, block_cubes * idx_per_cube);
            }
            // Model-atlas flecks (bbmodel blocks): the trailing index range, same vbuf with
            // the model atlas bound. Indices are absolute into the shared vbuf, so no base-
            // vertex offset is needed.
            if total_cubes > block_cubes {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                pass.set_pipeline(&self.particle_draw.pipeline);
                pass.set_vertex_buffer(0, self.particle_draw.vbuf.slice(..));
                pass.set_index_buffer(self.particle_draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    block_cubes * idx_per_cube..total_cubes * idx_per_cube,
                    0,
                    0..1,
                );
            }
        }
        // TRANSPARENT (WATER) PASS: far→near back-to-front, depth test only
        // (water must never occlude terrain behind it). Translucent BLOCKS
        // drew earlier (their own depth-writing pass, before the break
        // overlay), so water behind ice depth-fails against the ice's written
        // depth instead of double-blending over it.
        if any_transparent_visible {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "transparent pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            pass.set_pipeline(&self.transparent_pipe);
            for item in order.iter().rev() {
                if item.transparent_idx_count == 0 {
                    continue;
                }
                let Some(col) = self.terrain_columns.get(&item.column_pos) else {
                    continue;
                };
                // far -> near (alpha order)
                if let (Some(vb), Some(ib)) = (&col.transparent_vbuf, &col.transparent_ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_vertex_buffer(1, col.origin_vbuf.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    stats.transparent_draws += 1;
                    stats.transparent_indices += item.transparent_idx_count as u64;
                    pass.draw_indexed(
                        item.transparent_index_start
                            ..item.transparent_index_start + item.transparent_idx_count,
                        0,
                        0..1,
                    );
                }
            }
        }
        // ENVIRONMENT (VOLUMETRIC) PASSES: pack-supplied full-screen shaders
        // (clouds, auroras, fog volumes), composed in pack load order. Drawn
        // after ALL depth-writing world geometry so each shader can occlude
        // itself per-fragment against the frame depth, which it SAMPLES
        // (group 0 binding 2) — the pass attaches no depth, which is what
        // makes sampling it legal. Drawn AFTER the water pass: water writes no
        // depth, so paint order is the only thing keeping a cloud in front of
        // a lake (camera on a peak inside the deck, lake below punched a hole
        // through the cloud when water drew last). The reverse case — a lake
        // in FRONT of a cloudy horizon — needs no paint-order help: the march
        // clamps at the sampled depth, and the lakeBED behind the surface is
        // always nearer than any cloud behind the lake. Drawn BEFORE the
        // emitter particles so rain/snow volumes (no depth write) still streak
        // over the deck.
        //
        // HALF-RES: the passes march into `env_color` (half the scene dims)
        // against `env_depth` — a max-of-2x2 downsample of the frame depth —
        // and a depth-aware composite lifts the premultiplied result onto
        // the scene (crisp at silhouette edges, bilinear elsewhere). A
        // volumetric is soft, so this quarters its fragment cost invisibly;
        // see pipeline::EnvScaler and the two env_*.wgsl builtins.
        if self.env_passes.iter().any(|env| !env.dormant) {
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("env depth downsample"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.env_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    ..Default::default()
                });
                pass.set_pipeline(&self.env_scaler.down_pipe);
                pass.set_bind_group(0, &self.env_down_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.env_color,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            // Transparent black: premultiplied compositing is
                            // associative, so (passes over clear) over scene
                            // equals the old passes-over-scene directly.
                            load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    ..Default::default()
                });
                for env in self.env_passes.iter().filter(|env| !env.dormant) {
                    pass.set_pipeline(&env.res.pipe);
                    pass.set_bind_group(0, &env.bind, &[]);
                    pass.set_bind_group(1, &env.res.texture_bind, &[]);
                    pass.draw(0..3, 0..1);
                }
            }
            {
                let mut pass = color_depth_pass(
                    enc,
                    view,
                    &self.depth,
                    "env composite pass",
                    wgpu::LoadOp::Load,
                    None,
                );
                pass.set_pipeline(&self.env_scaler.comp_pipe);
                pass.set_bind_group(0, &self.env_comp_bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        // TRANSLUCENT BLOCK-EMITTER PARTICLES: solid-color cube particles from block
        // rows (torch flame cubes and mod emitters). They draw after water with alpha
        // blending, depth test but no write, and back-face culling in the pipeline so
        // transparency never exposes the whole cube shell.
        if self.emitter_particle_draw.vertex_count > 0 {
            let verts_per_cube = crate::render::particles::VERTS_PER_CUBE as u32;
            let idx_per_cube = crate::render::particles::INDICES_PER_CUBE as u32;
            let cubes = self.emitter_particle_draw.vertex_count / verts_per_cube;
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "emitter particle pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.emitter_particle_draw
                .draw(&mut pass, cubes * idx_per_cube);
        }
        // Selection outline, after particles: load color + depth, depth-test (no
        // write) so it draws over terrain/water at the targeted block but stays
        // occluded behind nearer geometry.
        if self.selection.is_some() && self.outline_vertex_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "outline pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
            );
            pass.set_pipeline(&self.outline_pipe);
            pass.set_bind_group(0, &self.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
            pass.draw(0..self.outline_vertex_count, 0..1);
        }
        // HAND PASS (§8 4c): the first-person held item / bare hand, drawn over the
        // world. Color Load; the world colour is already composited, so we attach
        // the main depth buffer with LoadOp::Clear(1.0) — clearing depth gives the
        // hand its own isolated depth space (it stays on top of the world and never
        // clips terrain) while still letting the held geometry SELF-SORT. The bare
        // arm + held block go through the depth-enabled model3d_hand pipeline
        // (slot 0 = the hand MVP); a held SPRITE goes through the (now depth-tested)
        // item3d pipeline (extruded, slot 0 = the item MVP — the model3d hand is
        // empty in that case, so slot 0 is free). They are mutually exclusive, but
        // both are drawn here so the pass is correct regardless.
        if self.hand_index_count > 0 || self.item3d_vertex_count > 0 {
            // NB: depth load-op is CLEAR(1.0) — this pass intentionally resets the
            // depth buffer so the hand self-sorts in isolation from the world.
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.depth,
                "hand pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Clear(1.0)),
            );
            // Bare arm / held block (model3d, depth-enabled hand variant).
            if self.hand_index_count > 0 {
                pass.set_pipeline(&self.model3d_hand_pipe);
                pass.set_bind_group(0, &self.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand_index_count, 0, 0..1);
            }
            // Extruded held sprite (block atlas) OR a held bbmodel block (model atlas) —
            // both ride the item3d pipeline (non-indexed triangle list, depth-tested).
            if self.item3d_vertex_count > 0 {
                pass.set_pipeline(&self.item3d_pipe);
                pass.set_bind_group(0, &self.item3d_mvp_bind, &[0]);
                let atlas = if self.held_is_model {
                    &self.model_atlas_bind
                } else {
                    &self.atlas_bind
                };
                pass.set_bind_group(1, atlas, &[]);
                pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
                pass.draw(0..self.item3d_vertex_count, 0..1);
            }
        }
        // GRADE PASS: full-screen colour grade (+ upscale when render_scale < 1)
        // of the finished world image, scene texture → swapchain (see
        // grade.wgsl). Everything after this draws ungraded over the graded
        // world. Skipped entirely when the world already rendered direct.
        if !direct {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.depth,
                "grade pass",
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                None,
            );
            pass.set_pipeline(&self.grade_pipe);
            pass.set_bind_group(0, &self.grade_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // CROSSHAIR PASS: the center invert-blend crosshair. Color Load, NO depth.
        if self.crosshair_vertex_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.depth,
                "crosshair pass",
                wgpu::LoadOp::Load,
                None,
            );
            pass.set_pipeline(&self.crosshair_pipe);
            pass.set_vertex_buffer(0, self.crosshair_vbuf.slice(..));
            pass.draw(0..self.crosshair_vertex_count, 0..1);
        }
        // UI PASS: under-chrome HUD layers (hurt vignette) → the GUI-document
        // draw list (all screen chrome, including its own dim backdrop) → the
        // over-chrome HUD layers (hearts, status effects, …) → per-slot item
        // icons, all via `ui_pipe` (own alpha blend, NO depth). Each layer
        // binds its own texture; solid quads bind the icon atlas (the solid
        // sentinel skips the sampler, so any layout-compatible texture works).
        if self.hud_layers.iter().any(|l| l.vertex_count > 0)
            || self.icon_quad_vertex_count > 0
            || !self.doc_ui.batches.is_empty()
            || !self.client_overlays.batches.is_empty()
        {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.depth,
                "ui pass",
                wgpu::LoadOp::Load,
                None,
            );
            pass.set_pipeline(&self.ui_pipe);
            let draw_layers = |pass: &mut wgpu::RenderPass<'_>, under: bool| {
                for layer in self.hud_layers.iter().filter(|l| l.under_chrome == under) {
                    if layer.vertex_count == 0 {
                        continue;
                    }
                    let bind = match &layer.texture {
                        super::HudLayerTexture::Solid => Some(&self.icon_atlas.bind),
                        super::HudLayerTexture::Texture(b) => b.as_ref(),
                    };
                    let Some(bind) = bind else {
                        continue; // the layer's art is missing — draw nothing
                    };
                    pass.set_bind_group(0, bind, &[]);
                    pass.set_vertex_buffer(0, layer.vbuf.slice(..));
                    pass.draw(0..layer.vertex_count, 0..1);
                }
            };
            draw_layers(&mut pass, true);
            // The GUI-document draw list: every panel, slot face, hover,
            // gauge, text and dim quad of the frame's screen.
            self.draw_doc_ui(&mut pass);
            draw_layers(&mut pass, false);
            self.draw_client_overlays(&mut pass);
            // Per-slot item icons (icon atlas), one bind + one draw.
            if self.icon_quad_vertex_count > 0 {
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.icon_quad_vbuf.slice(..));
                pass.draw(0..self.icon_quad_vertex_count, 0..1);
            }
        }
        // UI OVERLAY / DRAG PASS: stack counts, then the cursor-held icon, then its
        // count — keeping the whole dragged stack front-most.
        if self.ui_count_vertex_count > 0
            || self.drag_icon_quad_vertex_count > 0
            || self.ui_drag_count_vertex_count > 0
        {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.depth,
                "ui overlay / drag pass",
                wgpu::LoadOp::Load,
                None,
            );
            pass.set_pipeline(&self.ui_pipe);
            // Normal stack counts (solid), at the head of the solid buffer.
            if self.ui_count_vertex_count > 0 {
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui_solid_vbuf.slice(..));
                pass.draw(0..self.ui_count_vertex_count, 0..1);
            }
            // Cursor-held icon, appended after the normal icons.
            if self.drag_icon_quad_vertex_count > 0 {
                let start = self.icon_quad_vertex_count;
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.icon_quad_vbuf.slice(..));
                pass.draw(start..start + self.drag_icon_quad_vertex_count, 0..1);
            }
            // Cursor-held count (solid), packed after the normal counts.
            if self.ui_drag_count_vertex_count > 0 {
                let start = self.ui_count_vertex_count;
                pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui_solid_vbuf.slice(..));
                pass.draw(start..start + self.ui_drag_count_vertex_count, 0..1);
            }
        }
    }
}
