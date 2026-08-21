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
    label: &'static str,
    color_load: wgpu::LoadOp<wgpu::Color>,
    depth_load: Option<wgpu::LoadOp<f32>>,
    timer: Option<&'a gpu_timer::GpuTimer>,
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
        timestamp_writes: timer.and_then(|t| t.pass(label)),
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
                column_has_model |= section.model_idx_count > 0 || section.model_blend_idx_count > 0;
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
        let view = if direct {
            swapchain
        } else {
            &self.targets.scene_color
        };
        let cc = self.sky.clear_color;
        // OPAQUE PASS: the visible chunk terrain, near→far for early-Z. The first
        // pass of the frame: CLEARS color (to the fog colour) and depth.
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "opaque pass",
                wgpu::LoadOp::Clear(wgpu::Color {
                    r: cc[0] as f64,
                    g: cc[1] as f64,
                    b: cc[2] as f64,
                    a: 1.0,
                }),
                Some(wgpu::LoadOp::Clear(1.0)),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            pass.set_pipeline(&self.opaque_pipe);
            // Two binds for the whole pass: every column draw picks its origin
            // row with `first_instance`, and every draw's triangulation comes
            // from the shared quad index buffer with the section's first vertex
            // as `base_vertex`.
            pass.set_vertex_buffer(1, self.terrain.column_origins.buffer().slice(..));
            pass.set_index_buffer(self.terrain.quad_index.slice(), wgpu::IndexFormat::Uint32);
            for (_, pos) in opaque_columns {
                let Some(col) = self.terrain.columns.get(pos) else {
                    continue;
                };
                if col.opaque_quads == 0 {
                    continue;
                }
                if let Some(vb) = &col.opaque_vbuf {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    stats.opaque_draws += 1;
                    stats.opaque_indices += col.opaque_quads as u64 * 6;
                    let slot = col.origin_slot.index();
                    pass.draw_indexed(0..col.opaque_quads * 6, 0, slot..slot + 1);
                }
            }
            for item in order.iter() {
                if item.opaque_batched {
                    continue;
                }
                let Some(col) = self.terrain.columns.get(&item.column_pos) else {
                    continue;
                };
                // near -> far (early-Z)
                let (vbuf, vertex_start, quads) = if item.use_far_leaf_lod {
                    (
                        &col.far_opaque_vbuf,
                        item.far_opaque_vertex_start,
                        item.far_opaque_quads,
                    )
                } else {
                    (
                        &col.opaque_vbuf,
                        item.opaque_vertex_start,
                        item.opaque_quads,
                    )
                };
                if quads == 0 {
                    continue;
                }
                if let Some(vb) = vbuf {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    stats.opaque_draws += 1;
                    stats.opaque_indices += quads as u64 * 6;
                    let slot = col.origin_slot.index();
                    pass.draw_indexed(0..quads * 6, vertex_start as i32, slot..slot + 1);
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
                &self.targets.depth,
                "contact shadow pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.contact_pipe);
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            for (_, pos) in contact_columns {
                let Some(col) = self.terrain.columns.get(pos) else {
                    continue;
                };
                if col.contact_vertex_count == 0 {
                    continue;
                }
                if let Some(vb) = &col.contact_vbuf {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
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
                &self.targets.depth,
                "sky pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.sky.pipe);
            pass.set_bind_group(0, &self.sky.bind, &[]);
            pass.set_bind_group(1, &self.sky.texture_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // MODEL PASS: bbmodel-block geometry (explicit-UV, sampling the model atlas),
        // drawn per visible chunk with the mob pipeline (own texture + the same
        // underwater/fog the world uses) over depth from the opaque pass — so a placed
        // model occludes and is occluded by terrain like any block. Most chunks have no
        // model geometry, so this is usually a no-op loop.
        if any_model_visible || self.item_entity.model_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "model pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.model_atlas_bind, &[]);
            // Chunk model geometry draws with the world-model pipeline: its
            // vertices carry (sky, block) light so the shader applies the
            // day/night sky scale (meshes don't rebake at sunset).
            pass.set_pipeline(&self.world_model_pipe);
            for (_, pos) in model_columns {
                let Some(col) = self.terrain.columns.get(pos) else {
                    continue;
                };
                if col.model_idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    pass.set_index_buffer(
                        self.terrain.geometry.slice(&ib.alloc, ib.len),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(0..col.model_idx_count, 0, 0..1);
                }
            }
            for item in order.iter() {
                if item.model_batched || item.model_idx_count == 0 {
                    continue;
                }
                let Some(col) = self.terrain.columns.get(&item.column_pos) else {
                    continue;
                };
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    pass.set_index_buffer(
                        self.terrain.geometry.slice(&ib.alloc, ib.len),
                        wgpu::IndexFormat::Uint32,
                    );
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
            self.item_entity.model_draw.draw(&mut pass);
        }
        // ITEM-ENTITY PASS (§8 2b): dropped items as spinning cubes (the EXISTING
        // opaque pipeline, terrain atlas array) plus extruded sprite slabs (the
        // mob-layout pipeline over the 2D block atlas — their per-texel wall UVs
        // need explicit UVs). Load color + depth, depth test + write so items
        // occlude and are occluded by terrain.
        if self.item_entity.draw.index_count > 0 || self.item_entity.sprite_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "item entity pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            self.item_entity.draw.draw(&mut pass);
            if self.item_entity.sprite_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.item_entity.sprite_draw.draw(&mut pass);
            }
        }
        // CHEST + DOOR PASS: placed chests (inset body + hinged lid) and doors (2-tall
        // hinged slab) drawn as full opaque geometry by the EXISTING opaque pipeline
        // with the same uniform + atlas binds, loading color + depth so they occlude and
        // are occluded by terrain — exactly like the item-entity pass above.
        if self.block_entity.chest_draw.index_count > 0
            || self.block_entity.door_draw.index_count > 0
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "chest+door pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            self.block_entity.chest_draw.draw(&mut pass);
            self.block_entity.door_draw.draw(&mut pass);
        }
        // MOB PASS: animated entity models, one draw per visible species. Loads color
        // + depth (test + WRITE) so mobs occlude and are occluded by terrain — like
        // the item-entity / chest passes — but binds each species' OWN texture at
        // group(1) (not the block atlas); the mob pipeline (set by each DynamicDraw)
        // uses explicit-UV vertices so a model's arbitrary sub-rect UVs sample its
        // own sheet.
        if self.actor.mob_gpu.iter().any(|g| g.draw.index_count > 0)
            || self.actor.player_gpu.draw.index_count > 0
            || self.actor.item_draw.index_count > 0
            || self.actor.model_item_draw.index_count > 0
            || self.actor.block_item_draw.index_count > 0
        {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "mob pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            for g in &self.actor.mob_gpu {
                if g.draw.index_count == 0 {
                    continue;
                }
                pass.set_bind_group(1, &g.bind, &[]);
                g.draw.draw(&mut pass);
            }
            // Player bodies — the local third-person body and every remote
            // player, one combined stream (shared skin texture, mob pipeline)…
            if self.actor.player_gpu.draw.index_count > 0 {
                pass.set_bind_group(1, &self.actor.player_gpu.bind, &[]);
                self.actor.player_gpu.draw.draw(&mut pass);
            }
            // …their extruded-sprite held items (2D atlas)…
            if self.actor.item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.actor.item_draw.draw(&mut pass);
            }
            // …their bbmodel held items (model atlas)…
            if self.actor.model_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                self.actor.model_item_draw.draw(&mut pass);
            }
            // …and their held block mini-cubes (opaque pipeline + terrain
            // atlas array).
            if self.actor.block_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_array_bind, &[]);
                self.actor.block_item_draw.draw(&mut pass);
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
                &self.targets.depth,
                "translucent block pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            pass.set_pipeline(&self.translucent_pipe);
            // One bind for the whole pass: every column draw picks its
            // origin row with `first_instance`.
            pass.set_vertex_buffer(1, self.terrain.column_origins.buffer().slice(..));
            pass.set_index_buffer(self.terrain.quad_index.slice(), wgpu::IndexFormat::Uint32);
            for item in order.iter() {
                if item.translucent_quads == 0 {
                    continue;
                }
                let Some(col) = self.terrain.columns.get(&item.column_pos) else {
                    continue;
                };
                // near -> far: depth-writing, so early-Z applies like opaque.
                if let Some(vb) = &col.translucent_vbuf {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    stats.transparent_draws += 1;
                    stats.transparent_indices += item.translucent_quads as u64 * 6;
                    let slot = col.origin_slot.index();
                    pass.draw_indexed(
                        0..item.translucent_quads * 6,
                        item.translucent_vertex_start as i32,
                        slot..slot + 1,
                    );
                }
            }
        }
        // MODEL-BLEND PASS: the chunk's semi-transparent bbmodel faces (the
        // `model_blend_idx` ranges of the same model vertex/index buffers) —
        // alpha-blended but depth-WRITING, the ice precedent: overlapping
        // blended faces of one model resolve their order through the depth
        // buffer. Same ordering contract with the break overlay as ice (the
        // crack decal draws on top of a mined model's glass). Drawn over the
        // model pass's opaque depth, so blended glass correctly occludes and
        // is occluded by the model's own solid parts.
        if any_model_visible {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "model blend pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.model_atlas_bind, &[]);
            pass.set_pipeline(&self.world_model_blend_pipe);
            for (_, pos) in model_columns {
                let Some(col) = self.terrain.columns.get(pos) else {
                    continue;
                };
                if col.model_blend_idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    pass.set_index_buffer(
                        self.terrain.geometry.slice(&ib.alloc, ib.len),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(
                        col.model_idx_count..col.model_idx_count + col.model_blend_idx_count,
                        0,
                        0..1,
                    );
                }
            }
            for item in order.iter() {
                if item.model_batched || item.model_blend_idx_count == 0 {
                    continue;
                }
                let Some(col) = self.terrain.columns.get(&item.column_pos) else {
                    continue;
                };
                if let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) {
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    pass.set_index_buffer(
                        self.terrain.geometry.slice(&ib.alloc, ib.len),
                        wgpu::IndexFormat::Uint32,
                    );
                    pass.draw_indexed(
                        item.model_blend_index_start
                            ..item.model_blend_index_start + item.model_blend_idx_count,
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
        if self.hand.break_draw.index_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "break overlay pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.hand.break_draw.draw(&mut pass);
        }
        // PARTICLE PASS (§8 3b): tiny 3D terrain particle cubes. Drawn BEFORE the
        // transparent water pass (but after the break overlay, so they sit in front
        // of the crack): they are alpha-CUTOUT solids that DEPTH-TEST + DEPTH-WRITE,
        // so water blends over the ones behind it (underwater dust reads as
        // submerged) while ones in front of the water still occlude it. Reuses
        // uniform_bind + atlas_bind. 24 verts / 36 indices per cube.
        if self.particle.draw.vertex_count > 0 {
            let verts_per_cube = crate::particles::VERTS_PER_CUBE as u32;
            let idx_per_cube = crate::particles::INDICES_PER_CUBE as u32;
            // Cube boundaries: block flecks occupy [0..block_cubes), model flecks the rest.
            let total_cubes = self.particle.draw.vertex_count / verts_per_cube;
            let block_cubes = self.particle.block_vertex_count / verts_per_cube;
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "particle pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            // Block-atlas flecks: the leading index range via the standard draw.
            if block_cubes > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.particle
                    .draw
                    .draw(&mut pass, block_cubes * idx_per_cube);
            }
            // Model-atlas flecks (bbmodel blocks): the trailing index range, same vbuf with
            // the model atlas bound. Indices are absolute into the shared vbuf, so no base-
            // vertex offset is needed.
            if total_cubes > block_cubes {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                pass.set_pipeline(&self.particle.draw.pipeline);
                pass.set_vertex_buffer(0, self.particle.draw.vbuf.slice(..));
                pass.set_index_buffer(self.particle.draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
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
                &self.targets.depth,
                "transparent pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_array_bind, &[]);
            // One bind for the whole pass: every column draw picks its
            // origin row with `first_instance`.
            pass.set_vertex_buffer(1, self.terrain.column_origins.buffer().slice(..));
            pass.set_index_buffer(self.terrain.quad_index.slice(), wgpu::IndexFormat::Uint32);
            // Water side faces cull their backs, water TOPS do not (they must
            // stay visible from underneath). Sections almost never carry both,
            // so tracking the bound pipeline keeps this at one switch per pass
            // in practice. `None` until the first draw binds one: a render pass
            // starts with NO pipeline, so seeding this with a side is a draw
            // without a pipeline whenever that side happens to come first.
            let mut two_sided_bound: Option<bool> = None;
            for item in order.iter().rev() {
                if item.transparent_quads == 0 && item.transparent_ts_quads == 0 {
                    continue;
                }
                let Some(col) = self.terrain.columns.get(&item.column_pos) else {
                    continue;
                };
                let slot = col.origin_slot.index();
                // far -> near (alpha order)
                for (vbuf, start, quads, two_sided) in [
                    (
                        &col.transparent_vbuf,
                        item.transparent_vertex_start,
                        item.transparent_quads,
                        false,
                    ),
                    (
                        &col.transparent_ts_vbuf,
                        item.transparent_ts_vertex_start,
                        item.transparent_ts_quads,
                        true,
                    ),
                ] {
                    if quads == 0 {
                        continue;
                    }
                    let Some(vb) = vbuf else { continue };
                    if two_sided_bound != Some(two_sided) {
                        pass.set_pipeline(if two_sided {
                            &self.transparent_two_sided_pipe
                        } else {
                            &self.transparent_pipe
                        });
                        two_sided_bound = Some(two_sided);
                    }
                    pass.set_vertex_buffer(0, self.terrain.geometry.slice(&vb.alloc, vb.len));
                    stats.transparent_draws += 1;
                    stats.transparent_indices += quads as u64 * 6;
                    pass.draw_indexed(0..quads * 6, start as i32, slot..slot + 1);
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
        if self.sky.env_passes.iter().any(|env| !env.dormant) {
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("env depth downsample"),
                    color_attachments: &[],
                    depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                        view: &self.sky.env_depth,
                        depth_ops: Some(wgpu::Operations {
                            load: wgpu::LoadOp::Clear(1.0),
                            store: wgpu::StoreOp::Store,
                        }),
                        stencil_ops: None,
                    }),
                    timestamp_writes: self
                        .gpu_timer
                        .as_ref()
                        .and_then(|t| t.pass("env depth downsample")),
                    ..Default::default()
                });
                pass.set_pipeline(&self.sky.env_scaler.down_pipe);
                pass.set_bind_group(0, &self.sky.env_down_bind, &[]);
                pass.draw(0..3, 0..1);
            }
            {
                let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("environment pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: &self.sky.env_color,
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
                    timestamp_writes: self
                        .gpu_timer
                        .as_ref()
                        .and_then(|t| t.pass("environment pass")),
                    ..Default::default()
                });
                for env in self.sky.env_passes.iter().filter(|env| !env.dormant) {
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
                    &self.targets.depth,
                    "env composite pass",
                    wgpu::LoadOp::Load,
                    None,
                    self.gpu_timer.as_ref(),
                );
                pass.set_pipeline(&self.sky.env_scaler.comp_pipe);
                pass.set_bind_group(0, &self.sky.env_comp_bind, &[]);
                pass.draw(0..3, 0..1);
            }
        }
        // TRANSLUCENT BLOCK-EMITTER PARTICLES: solid-color cube particles from block
        // rows (torch flame cubes and mod emitters). They draw after water with alpha
        // blending, depth test but no write, and back-face culling in the pipeline so
        // transparency never exposes the whole cube shell.
        if self.particle.emitter_draw.vertex_count > 0 {
            let verts_per_cube = crate::particles::VERTS_PER_CUBE as u32;
            let idx_per_cube = crate::particles::INDICES_PER_CUBE as u32;
            let cubes = self.particle.emitter_draw.vertex_count / verts_per_cube;
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "emitter particle pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            self.particle
                .emitter_draw
                .draw(&mut pass, cubes * idx_per_cube);
        }
        // Selection outline, after particles: load color + depth, depth-test (no
        // write) so it draws over terrain/water at the targeted block but stays
        // occluded behind nearer geometry.
        if self.chrome.selection.is_some() && self.chrome.outline_vertex_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "outline pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.chrome.outline_pipe);
            pass.set_bind_group(0, &self.chrome.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.chrome.outline_vbuf.slice(..));
            pass.draw(0..self.chrome.outline_vertex_count, 0..1);
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
        if self.hand.index_count > 0
            || self.hand.item3d_vertex_count > 0
            || self.hand.off_index_count > 0
            || self.hand.off_item3d_count > 0
        {
            // NB: depth load-op is CLEAR(1.0) — this pass intentionally resets the
            // depth buffer so the hand self-sorts in isolation from the world.
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "hand pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Clear(1.0)),
                self.gpu_timer.as_ref(),
            );
            // Bare arm / held block (model3d, depth-enabled hand variant).
            if self.hand.index_count > 0 {
                pass.set_pipeline(&self.hand.model3d_pipe);
                pass.set_bind_group(0, &self.hand.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.hand.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.hand.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand.index_count, 0, 0..1);
            }
            // The OFF hand's held block: its geometry appends after the main
            // hand's in the shared buffers; MVP slot 1.
            if self.hand.off_index_count > 0 {
                pass.set_pipeline(&self.hand.model3d_pipe);
                pass.set_bind_group(0, &self.hand.model3d_mvp_bind, &[256]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.hand.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.hand.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    self.hand.index_count..self.hand.index_count + self.hand.off_index_count,
                    self.hand.vertex_count as i32,
                    0..1,
                );
            }
            // Extruded held sprite (block atlas) OR a held bbmodel block (model atlas) —
            // both ride the item3d pipeline (non-indexed triangle list, depth-tested).
            if self.hand.item3d_vertex_count > 0 {
                pass.set_pipeline(&self.hand.item3d_pipe);
                pass.set_bind_group(0, &self.hand.item3d_mvp_bind, &[0]);
                let atlas = if self.hand.held_is_model {
                    &self.model_atlas_bind
                } else {
                    &self.atlas_bind
                };
                pass.set_bind_group(1, atlas, &[]);
                pass.set_vertex_buffer(0, self.hand.item3d_vbuf.slice(..));
                pass.draw(0..self.hand.item3d_vertex_count, 0..1);
            }
            // The OFF hand's item3d stream (appended range, MVP slot 1).
            if self.hand.off_item3d_count > 0 {
                pass.set_pipeline(&self.hand.item3d_pipe);
                pass.set_bind_group(0, &self.hand.item3d_mvp_bind, &[256]);
                let atlas = if self.hand.off_is_model {
                    &self.model_atlas_bind
                } else {
                    &self.atlas_bind
                };
                pass.set_bind_group(1, atlas, &[]);
                pass.set_vertex_buffer(0, self.hand.item3d_vbuf.slice(..));
                pass.draw(
                    self.hand.off_item3d_start
                        ..self.hand.off_item3d_start + self.hand.off_item3d_count,
                    0..1,
                );
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
                &self.targets.depth,
                "grade pass",
                wgpu::LoadOp::Clear(wgpu::Color::BLACK),
                None,
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.targets.grade_pipe);
            pass.set_bind_group(0, &self.targets.grade_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        // CROSSHAIR PASS: the center invert-blend crosshair. Color Load, NO depth.
        if self.chrome.crosshair_vertex_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.targets.depth,
                "crosshair pass",
                wgpu::LoadOp::Load,
                None,
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.chrome.crosshair_pipe);
            pass.set_vertex_buffer(0, self.chrome.crosshair_vbuf.slice(..));
            pass.draw(0..self.chrome.crosshair_vertex_count, 0..1);
        }
        // UI PASS: under-chrome HUD layers (hurt vignette) → the GUI-document
        // draw list (all screen chrome, including its own dim backdrop) → the
        // over-chrome HUD layers (hearts, status effects, …) → per-slot item
        // icons, all via `ui_pipe` (own alpha blend, NO depth). Each layer
        // binds its own texture; solid quads bind the icon atlas (the solid
        // sentinel skips the sampler, so any layout-compatible texture works).
        if self.ui.hud_layers.iter().any(|l| l.vertex_count > 0)
            || self.ui.icon_quad_vertex_count > 0
            || !self.ui.doc_ui.batches.is_empty()
            || !self.ui.client_overlays.batches.is_empty()
        {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.targets.depth,
                "ui pass",
                wgpu::LoadOp::Load,
                None,
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.ui.pipe);
            let draw_layers = |pass: &mut wgpu::RenderPass<'_>, under: bool| {
                for layer in self
                    .ui
                    .hud_layers
                    .iter()
                    .filter(|l| l.under_chrome == under)
                {
                    if layer.vertex_count == 0 {
                        continue;
                    }
                    let bind = match &layer.texture {
                        super::HudLayerTexture::Solid => Some(&self.ui.icon_atlas.bind),
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
            if self.ui.icon_quad_vertex_count > 0 {
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.icon_quad_vbuf.slice(..));
                pass.draw(0..self.ui.icon_quad_vertex_count, 0..1);
            }
        }
        // UI OVERLAY / DRAG PASS: stack counts, then the document's overlay
        // tier (floating tooltip chrome) with its own icons and counts over
        // the base tier's, then the cursor-held icon and its count — keeping
        // the whole dragged stack front-most.
        if self.ui.count_vertex_count > 0
            || self.ui.drag_icon_quad_vertex_count > 0
            || self.ui.drag_count_vertex_count > 0
            || self.ui.overlay_icon_quad_vertex_count > 0
            || self.ui.overlay_count_vertex_count > 0
            || self.has_doc_overlay()
        {
            let mut pass = color_depth_pass(
                enc,
                swapchain,
                &self.targets.depth,
                "ui overlay / drag pass",
                wgpu::LoadOp::Load,
                None,
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(&self.ui.pipe);
            // Normal stack counts (solid), at the head of the solid buffer.
            if self.ui.count_vertex_count > 0 {
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.solid_vbuf.slice(..));
                pass.draw(0..self.ui.count_vertex_count, 0..1);
            }
            // Floating tooltip chrome, over every base-tier icon and count.
            self.draw_doc_ui_overlay(&mut pass);
            // Its icons, appended after the normal icons.
            if self.ui.overlay_icon_quad_vertex_count > 0 {
                let start = self.ui.icon_quad_vertex_count;
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.icon_quad_vbuf.slice(..));
                pass.draw(start..start + self.ui.overlay_icon_quad_vertex_count, 0..1);
            }
            // Its counts (solid), packed after the normal counts.
            if self.ui.overlay_count_vertex_count > 0 {
                let start = self.ui.count_vertex_count;
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.solid_vbuf.slice(..));
                pass.draw(start..start + self.ui.overlay_count_vertex_count, 0..1);
            }
            // Cursor-held icon, appended after the tooltip icons.
            if self.ui.drag_icon_quad_vertex_count > 0 {
                let start = self.ui.icon_quad_vertex_count + self.ui.overlay_icon_quad_vertex_count;
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.icon_quad_vbuf.slice(..));
                pass.draw(start..start + self.ui.drag_icon_quad_vertex_count, 0..1);
            }
            // Cursor-held count (solid), packed after the tooltip counts.
            if self.ui.drag_count_vertex_count > 0 {
                let start = self.ui.count_vertex_count + self.ui.overlay_count_vertex_count;
                pass.set_bind_group(0, &self.ui.icon_atlas.bind, &[]);
                pass.set_vertex_buffer(0, self.ui.solid_vbuf.slice(..));
                pass.draw(start..start + self.ui.drag_count_vertex_count, 0..1);
            }
        }
    }
}
