//! The terrain nodes: every draw of packed column geometry, over the plan
//! `draw_plan` built. Whole-column draws go first; the sections that could
//! not join one draw for themselves.

use super::*;

impl TerrainPass {
    /// OPAQUE terrain, near → far for early-Z. Two binds serve the whole
    /// node: every column draw picks its origin row with `first_instance`,
    /// and every draw's triangulation comes from the shared quad index buffer
    /// with the section's first vertex as `base_vertex`.
    pub(super) fn record_opaque(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        pass.set_pipeline(self.pipes.opaque.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        pass.set_index_buffer(self.quad_index.slice(), wgpu::IndexFormat::Uint32);
        for &(_, _, slot, far) in &self.plan.opaque_columns {
            let col = self.columns.at(slot);
            // Far LOD draws the column's leading far region; detailed draws
            // the whole stream. Both are one contiguous range.
            let quads = if far {
                col.opaque_far_quads
            } else {
                col.opaque_quads
            };
            if quads == 0 {
                continue;
            }
            if let Some(vb) = &col.opaque_vbuf {
                pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
                stats.opaque_draws += 1;
                stats.opaque_indices += quads as u64 * 6;
                let slot = col.origin_slot.index();
                pass.draw_indexed(0..quads * 6, 0, slot..slot + 1);
            }
        }
        for item in &self.plan.sections {
            if item.opaque_batched {
                continue;
            }
            let col = self.columns.at(item.column_slot);
            // The section's far region always draws; its leaf tail joins only
            // at detailed LOD. The tail is empty for every section without
            // leaves, so this is one draw in the common case.
            let Some(vb) = &col.opaque_vbuf else {
                continue;
            };
            let mut ranges = [
                (item.opaque_vertex_start, item.opaque_quads),
                (item.opaque_tail_start, item.opaque_tail_quads),
            ];
            if item.use_far_leaf_lod {
                ranges[1].1 = 0;
            }
            let mut bound = false;
            for (vertex_start, quads) in ranges {
                if quads == 0 {
                    continue;
                }
                if !bound {
                    pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
                    bound = true;
                }
                stats.opaque_draws += 1;
                stats.opaque_indices += quads as u64 * 6;
                let slot = col.origin_slot.index();
                pass.draw_indexed(0..quads * 6, vertex_start as i32, slot..slot + 1);
            }
        }
    }

    /// CONTACT SHADOWS: the models' soft floor stamps, multiplied over the
    /// opaque terrain (depth read-only, LessEqual + its own coplanar bias
    /// against the supporting top face). One whole-buffer draw per visible
    /// contact-bearing column — the stream is sparse and needs no per-section
    /// ranges.
    pub(super) fn record_contact(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.pipes.contact.get(ctx.samples));
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        for &(_, _, slot) in &self.plan.contact_columns {
            let col = self.columns.at(slot);
            if col.contact_vertex_count == 0 {
                continue;
            }
            if let Some(vb) = &col.contact_vbuf {
                pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
                let slot = col.origin_slot.index();
                pass.draw(0..col.contact_vertex_count, slot..slot + 1);
            }
        }
    }

    /// MODELS: bbmodel-block geometry (explicit-UV, sampling the model
    /// atlas) over the opaque depth, so a placed model occludes and is
    /// occluded by terrain like any block. Its vertices carry (sky, block)
    /// light, so the world-model pipeline applies the day/night sky scale
    /// (meshes don't rebake at sunset).
    pub(super) fn record_models(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        pass.set_pipeline(self.pipes.world_model.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        for &(_, _, slot) in &self.plan.model_columns {
            let col = self.columns.at(slot);
            if col.model_idx_count > 0 {
                self.draw_model_range(pass, col, 0..col.model_idx_count);
            }
        }
        for item in &self.plan.sections {
            if item.model_batched || item.model_idx_count == 0 {
                continue;
            }
            let start = item.model_index_start;
            self.draw_model_range(
                pass,
                self.columns.at(item.column_slot),
                start..start + item.model_idx_count,
            );
        }
    }

    /// TRANSLUCENT BLOCKS (ice): alpha-blended but depth-WRITING, so a sheet
    /// of translucent cubes resolves its own face order through the depth
    /// buffer. Near → far: depth-writing, so early-Z applies like opaque.
    pub(super) fn record_translucent(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        pass.set_pipeline(self.pipes.translucent.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        pass.set_index_buffer(self.quad_index.slice(), wgpu::IndexFormat::Uint32);
        for item in &self.plan.sections {
            if item.translucent_quads == 0 {
                continue;
            }
            let col = self.columns.at(item.column_slot);
            if let Some(vb) = &col.translucent_vbuf {
                pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
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

    /// MODEL BLEND: the chunk's semi-transparent bbmodel faces (the
    /// `model_blend_idx` ranges of the same model buffers) — alpha-blended
    /// but depth-WRITING, the ice precedent. Drawn over the models' opaque
    /// depth, so blended glass occludes and is occluded by the model's own
    /// solid parts.
    pub(super) fn record_model_blend(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        pass.set_pipeline(self.pipes.world_model_blend.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        for &(_, _, slot) in &self.plan.model_columns {
            let col = self.columns.at(slot);
            if col.model_blend_idx_count > 0 {
                let start = col.model_idx_count;
                self.draw_model_range(pass, col, start..start + col.model_blend_idx_count);
            }
        }
        for item in &self.plan.sections {
            if item.model_batched || item.model_blend_idx_count == 0 {
                continue;
            }
            let start = item.model_blend_index_start;
            self.draw_model_range(
                pass,
                self.columns.at(item.column_slot),
                start..start + item.model_blend_idx_count,
            );
        }
    }

    /// MODEL BREAK: the destroy crack over a mined bbmodel block, drawn as a
    /// decal over the model's OWN triangles — the column model stream the
    /// model node drew, re-rasterized with the crack pipeline and masked in
    /// the shader to the cracked model's outline box. Nothing re-derives the
    /// model's form, so the decal is depth-coincident and hugs every cube,
    /// however small or rotated.
    pub(super) fn record_model_break(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        crack: &crate::model_break::ModelBreak,
    ) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        pass.set_bind_group(2, &crack.bind, &[]);
        pass.set_pipeline(crack.pipe.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        for pos in &crack.columns {
            let Some(col) = self.columns.get(pos) else {
                continue;
            };
            // The whole column's model stream, opaque range and blend range
            // together: the mask discards every fragment outside the cracked
            // model, so the node never needs to know which section holds it.
            let total = col.model_idx_count + col.model_blend_idx_count;
            if total > 0 {
                self.draw_model_range(pass, col, 0..total);
            }
        }
    }

    /// FLUID: see-through fluid, far → near back-to-front, depth test only (a
    /// see-through fluid must never occlude terrain behind it; an opaque
    /// fluid drew with the opaque terrain). Translucent BLOCKS drew earlier
    /// with depth writes, so fluid behind ice depth-fails against the ice
    /// instead of double-blending over it.
    pub(super) fn record_fluid(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        pass.set_index_buffer(self.quad_index.slice(), wgpu::IndexFormat::Uint32);
        // Fluid side faces cull their backs, fluid TOPS do not (they must
        // stay visible from underneath). Sections almost never carry both, so
        // tracking the bound pipeline keeps this at one switch per node in
        // practice. `None` until the first draw binds one: the node may open
        // its render pass, which starts with NO pipeline.
        let mut two_sided_bound: Option<bool> = None;
        for item in self.plan.sections.iter().rev() {
            if item.transparent_quads == 0 && item.transparent_ts_quads == 0 {
                continue;
            }
            let col = self.columns.at(item.column_slot);
            let slot = col.origin_slot.index();
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
                        self.pipes.transparent_two_sided.get(ctx.samples)
                    } else {
                        self.pipes.transparent.get(ctx.samples)
                    });
                    two_sided_bound = Some(two_sided);
                }
                pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
                stats.transparent_draws += 1;
                stats.transparent_indices += quads as u64 * 6;
                pass.draw_indexed(0..quads * 6, start as i32, slot..slot + 1);
            }
        }
    }

    /// Draw `indices` of `col`'s model stream: its vertex and index buffers
    /// bound from the arena, its origin row by `first_instance`.
    fn draw_model_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        col: &crate::resources::GpuColumnMesh,
        indices: std::ops::Range<u32>,
    ) {
        let (Some(vb), Some(ib)) = (&col.model_vbuf, &col.model_ibuf) else {
            return;
        };
        let slot = col.origin_slot.index();
        pass.set_vertex_buffer(0, self.geometry.slice(&vb.alloc, vb.len));
        pass.set_index_buffer(
            self.geometry.slice(&ib.alloc, ib.len),
            wgpu::IndexFormat::Uint32,
        );
        pass.draw_indexed(indices, 0, slot..slot + 1);
    }
}
