//! GPU render-pass encoding for [`Renderer`].
//!
//! `encode_passes` records the world passes in order over the plan
//! `draw_plan` built, then hands off to the environment chain, the hand pass
//! and the screen tail (resolve, post-process, chrome) in the sibling
//! modules. `render` stays the thin orchestrator (one encoder, one submit,
//! one present). The shared pass helper `color_depth_pass` lives here.

use super::post_process::SceneRoute;
use super::*;

mod environment;
mod hand;
mod screen;

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
pub(super) fn color_depth_pass<'a>(
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
        // target; the post-process pass then reads it and writes the swapchain,
        // and screen chrome (crosshair, UI) draws over the graded image so its
        // colours stay exact. See [`SceneRoute`] for the two cases that skip
        // the round-trip.
        let samples = self.targets.anti_aliasing.sample_count();
        let route = self.scene_route();
        let view = if route == SceneRoute::Direct {
            swapchain
        } else {
            self.targets
                .multisample_color
                .as_ref()
                .unwrap_or(&self.targets.scene_color)
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
            pass.set_pipeline(self.opaque_pipe.get(samples));
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
            pass.set_pipeline(self.contact_pipe.get(samples));
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
        // ENTITY SHADOW PASS: the blob-shadow decals under mobs / dropped items
        // / bodies — same contract as the contact stamps above (MULTIPLY over
        // opaque terrain, depth read-only with the coplanar bias, drawn before
        // the sky so a culled support section can't leave smudges on the
        // background). One whole-batch draw; the gather already culled the
        // rows against this frame's view volume.
        if self.shadow.draw.vertex_count > 0 {
            let mut pass = color_depth_pass(
                enc,
                view,
                &self.targets.depth,
                "entity shadow pass",
                wgpu::LoadOp::Load,
                Some(wgpu::LoadOp::Load),
                self.gpu_timer.as_ref(),
            );
            pass.set_pipeline(self.shadow.draw.pipeline.get(samples));
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_vertex_buffer(0, self.shadow.draw.vbuf.slice(..));
            pass.set_index_buffer(self.shadow.draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
            let quads = self.shadow.draw.vertex_count as usize
                / crate::entity_shadow::VERTS_PER_SHADOW as usize;
            pass.draw_indexed(
                0..crate::entity_shadow::quad_index_count(quads) as u32,
                0,
                0..1,
            );
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
            pass.set_pipeline(self.sky.pipe.get(samples));
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
            pass.set_pipeline(self.world_model_pipe.get(samples));
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
            pass.set_pipeline(self.model_pipe.get(samples));
            self.item_entity.model_draw.draw(&mut pass, samples);
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
            self.item_entity.draw.draw(&mut pass, samples);
            if self.item_entity.sprite_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.item_entity.sprite_draw.draw(&mut pass, samples);
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
            self.block_entity.chest_draw.draw(&mut pass, samples);
            self.block_entity.door_draw.draw(&mut pass, samples);
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
                g.draw.draw(&mut pass, samples);
            }
            // Player bodies — the local third-person body and every remote
            // player, one combined stream (shared skin texture, mob pipeline)…
            if self.actor.player_gpu.draw.index_count > 0 {
                pass.set_bind_group(1, &self.actor.player_gpu.bind, &[]);
                self.actor.player_gpu.draw.draw(&mut pass, samples);
            }
            // …their extruded-sprite held items (2D atlas)…
            if self.actor.item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.actor.item_draw.draw(&mut pass, samples);
            }
            // …their bbmodel held items (model atlas)…
            if self.actor.model_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                self.actor.model_item_draw.draw(&mut pass, samples);
            }
            // …and their held block mini-cubes (opaque pipeline + terrain
            // atlas array).
            if self.actor.block_item_draw.index_count > 0 {
                pass.set_bind_group(1, &self.atlas_array_bind, &[]);
                self.actor.block_item_draw.draw(&mut pass, samples);
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
            pass.set_pipeline(self.translucent_pipe.get(samples));
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
            pass.set_pipeline(self.world_model_blend_pipe.get(samples));
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
            self.hand.break_draw.draw(&mut pass, samples);
        }
        // PARTICLE PASS (§8 3b): tiny 3D terrain particle cubes. Drawn BEFORE the
        // transparent water pass (but after the break overlay, so they sit in front
        // of the crack): they are alpha-CUTOUT solids that DEPTH-TEST + DEPTH-WRITE,
        // so water blends over the ones behind it (underwater dust reads as
        // submerged) while ones in front of the water still occlude it. Reuses
        // uniform_bind + atlas_bind. Cube faces and oriented sprites share quad indices.
        if self.particle.draw.vertex_count > 0 {
            let verts_per_quad = 4;
            let idx_per_quad = 6;
            // Quad boundaries: block flecks occupy [0..block_quads), model flecks the rest.
            let total_quads = self.particle.draw.vertex_count / verts_per_quad;
            let block_quads = self.particle.block_vertex_count / verts_per_quad;
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
            if block_quads > 0 {
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                self.particle
                    .draw
                    .draw(&mut pass, block_quads * idx_per_quad, samples);
            }
            // Model-atlas flecks (bbmodel blocks): the trailing index range, same vbuf with
            // the model atlas bound. Indices are absolute into the shared vbuf, so no base-
            // vertex offset is needed.
            if total_quads > block_quads {
                pass.set_bind_group(1, &self.model_atlas_bind, &[]);
                pass.set_pipeline(self.particle.draw.pipeline.get(samples));
                pass.set_vertex_buffer(0, self.particle.draw.vbuf.slice(..));
                pass.set_index_buffer(self.particle.draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(
                    block_quads * idx_per_quad..total_quads * idx_per_quad,
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
                            self.transparent_two_sided_pipe.get(samples)
                        } else {
                            self.transparent_pipe.get(samples)
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
        self.encode_environment(enc, view, samples);
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
                .draw(&mut pass, cubes * idx_per_cube, samples);
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
            pass.set_pipeline(self.chrome.outline_pipe.get(samples));
            pass.set_bind_group(0, &self.chrome.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.chrome.outline_vbuf.slice(..));
            pass.draw(0..self.chrome.outline_vertex_count, 0..1);
        }
        self.encode_hand(enc, view, samples);
        self.encode_screen(enc, view, swapchain, samples, route);
    }
}
