use super::*;

impl ShadowPass {
    pub(super) fn active(&self) -> bool {
        self.draw.vertex_count > 0
    }

    pub(super) fn record(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.draw.pipeline.get(ctx.samples));
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_vertex_buffer(0, self.draw.vbuf.slice(..));
        pass.set_index_buffer(self.draw.ibuf.slice(..), wgpu::IndexFormat::Uint32);
        let quads =
            self.draw.vertex_count as usize / crate::entity_shadow::VERTS_PER_SHADOW as usize;
        pass.draw_indexed(
            0..crate::entity_shadow::quad_index_count(quads) as u32,
            0,
            0..1,
        );
    }
}

impl ItemEntityPass {
    pub(super) fn models_active(&self) -> bool {
        self.model_draw.index_count > 0
    }

    pub(super) fn items_active(&self) -> bool {
        self.draw.index_count > 0 || self.sprite_draw.index_count > 0
    }

    pub(super) fn record_models(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        self.model_draw.draw(pass, ctx.samples);
    }

    pub(super) fn record_items(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        self.draw.draw(pass, ctx.samples);
        if self.sprite_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            self.sprite_draw.draw(pass, ctx.samples);
        }
    }
}

impl BlockEntityPass {
    pub(super) fn active(&self) -> bool {
        self.draw.index_count > 0
    }

    pub(super) fn record(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
        self.draw.draw(pass, ctx.samples);
    }
}

impl ActorPass {
    pub(super) fn active(&self) -> bool {
        self.mob_gpu.iter().any(|g| !g.drawn.is_empty())
            || !self.player_gpu.drawn.is_empty()
            || self.item_draw.index_count > 0
            || self.model_item_draw.index_count > 0
            || self.block_item_draw.index_count > 0
    }

    pub(super) fn record(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        for g in &self.mob_gpu {
            if g.drawn.is_empty() {
                continue;
            }
            pass.set_bind_group(1, &g.bind, &[]);
            self.skin.draw(pass, ctx.samples, &g.mesh, g.drawn.clone());
        }
        let bodies = &self.player_gpu;
        if !bodies.drawn.is_empty() {
            pass.set_bind_group(1, &bodies.bind, &[]);
            self.skin
                .draw(pass, ctx.samples, &bodies.mesh, bodies.drawn.clone());
        }
        if self.item_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            self.item_draw.draw(pass, ctx.samples);
        }
        if self.model_item_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
            self.model_item_draw.draw(pass, ctx.samples);
        }
        if self.block_item_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.atlas_array, &[]);
            self.block_item_draw.draw(pass, ctx.samples);
        }
    }
}

impl HandPass {
    pub(super) fn break_overlay_active(&self) -> bool {
        self.break_draw.index_count > 0
    }

    /// BREAK OVERLAY: the destroy crack over the targeted block. MULTIPLY
    /// blend; depth LessEqual / no-write over a cube built COINCIDENT with
    /// the block faces (no inflation, so the decal never misaligns), with a
    /// small polygon offset toward the camera so it wins the depth tie.
    pub(super) fn record_break_overlay(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas, &[]);
        self.break_draw.draw(pass, ctx.samples);
    }
}

impl ParticlePass {
    pub(super) fn cutout_active(&self) -> bool {
        self.draw.instance_count > 0
    }

    pub(super) fn emitters_active(&self) -> bool {
        self.emitter_draw.instance_count > 0
    }

    pub(super) fn record_cutout(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        let total = self.draw.instance_count;
        let block = self.block_count;
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        if block > 0 {
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            self.draw.draw(pass, ctx.samples, 0..block);
        }
        if total > block {
            pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
            self.draw.draw(pass, ctx.samples, block..total);
        }
    }

    pub(super) fn record_emitters(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas, &[]);
        self.emitter_draw
            .draw(pass, ctx.samples, 0..self.emitter_draw.instance_count);
    }
}
