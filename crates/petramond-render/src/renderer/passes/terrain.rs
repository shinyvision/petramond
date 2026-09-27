use super::*;
use crate::resources::{ColumnBuffer, SectionStream, Span};

impl TerrainPass {
    pub(super) fn record_opaque(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        pass.set_index_buffer(self.quad_index.slice(), wgpu::IndexFormat::Uint32);
        let pipe = self.pipes.opaque.get(ctx.samples);
        self.draws
            .encode(pass, QuadPass::Opaque, &self.geometry, &|_| pipe);
        let list = self.draws.list(QuadPass::Opaque);
        stats.opaque_draws += list.len();
        stats.opaque_indices += list.indices();
    }

    pub(super) fn record_contact(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.pipes.contact.get(ctx.samples));
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        for &(_, _, slot) in &self.plan.contact_columns {
            let col = self.columns.at(slot);
            let region = col.region(SectionStream::Contact);
            if region.is_empty() {
                continue;
            }
            if let Some(vb) = col.buffer(ColumnBuffer::Contact) {
                pass.set_vertex_buffer(0, self.geometry.slice(ColumnBuffer::Contact, vb));
                let slot = col.origin_slot.index();
                pass.draw(region.start..region.end(), slot..slot + 1);
            }
        }
    }

    pub(super) fn record_models(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        pass.set_pipeline(self.pipes.world_model.get(ctx.samples));
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        self.draw_model_stream(pass, SectionStream::ModelIndices);
    }

    pub(super) fn record_translucent(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        stats: &mut RenderStats,
    ) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        pass.set_vertex_buffer(1, self.column_origins.buffer().slice(..));
        pass.set_index_buffer(self.quad_index.slice(), wgpu::IndexFormat::Uint32);
        let pipe = self.pipes.translucent.get(ctx.samples);
        self.draws
            .encode(pass, QuadPass::Translucent, &self.geometry, &|_| pipe);
        let list = self.draws.list(QuadPass::Translucent);
        stats.transparent_draws += list.len();
        stats.transparent_indices += list.indices();
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
        self.draw_model_stream(pass, SectionStream::ModelBlendIndices);
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
            let total = col.region(SectionStream::ModelIndices).count
                + col.region(SectionStream::ModelBlendIndices).count;
            self.draw_model_range(
                pass,
                col,
                Span {
                    start: 0,
                    count: total,
                },
            );
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
        let side = self.pipes.transparent.get(ctx.samples);
        let top = self.pipes.transparent_two_sided.get(ctx.samples);
        self.draws
            .encode(pass, QuadPass::Transparent, &self.geometry, &|layer| {
                if layer == petramond_mesh::QuadLayer::TransparentTwoSided {
                    top
                } else {
                    side
                }
            });
        let list = self.draws.list(QuadPass::Transparent);
        stats.transparent_draws += list.len();
        stats.transparent_indices += list.indices();
    }

    fn draw_model_stream(&self, pass: &mut wgpu::RenderPass<'_>, stream: SectionStream) {
        for &(_, _, slot) in &self.plan.model_columns {
            let col = self.columns.at(slot);
            self.draw_model_range(pass, col, col.region(stream));
        }
        for item in self.plan.sections.iter().filter(|item| !item.model_batched) {
            self.draw_model_range(pass, self.columns.at(item.column_slot), item.span(stream));
        }
    }

    fn draw_model_range(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        col: &crate::resources::GpuColumnMesh,
        indices: Span,
    ) {
        if indices.is_empty() || !self.geometry.bind_model_streams(pass, col) {
            return;
        }
        let slot = col.origin_slot.index();
        pass.draw_indexed(indices.start..indices.end(), 0, slot..slot + 1);
    }
}
