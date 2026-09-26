//! The per-frame dynamic nodes: blob shadows, dropped items, animated
//! blocks, bodies, the block crack and particles — each recorded by the pass
//! struct whose buffers the frame's bake filled.

use super::*;

impl ShadowPass {
    pub(super) fn active(&self) -> bool {
        self.draw.vertex_count > 0
    }

    /// ENTITY SHADOWS: the blob decals under mobs, dropped items and bodies —
    /// the contact stamps' contract (MULTIPLY over opaque terrain, depth
    /// read-only with the coplanar bias, before the sky). One whole-batch
    /// draw; the gather already culled the rows against this frame's view.
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

    /// Dropped bbmodel items: world-space `ItemVertex` over the model atlas,
    /// with per-frame CPU-baked light (so the mob-layout pipeline, not the
    /// world-model one).
    pub(super) fn record_models(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, ctx.world_bind, &[]);
        pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
        self.model_draw.draw(pass, ctx.samples);
    }

    /// ITEM ENTITIES: dropped items as spinning cubes (the opaque dynamic
    /// pipeline over the terrain atlas array) plus extruded sprite slabs (the
    /// mob-layout pipeline over the 2D block atlas — their per-texel wall UVs
    /// need explicit UVs), depth-tested and -written against terrain.
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

    /// BLOCK ENTITIES: every placed animated block (chest lids, door and
    /// trapdoor swings, a pack's own) as full opaque geometry with the
    /// terrain binds, occluding and occluded by terrain.
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

    /// BODIES: animated entity models, one instanced draw per visible
    /// species, each binding its OWN texture at group(1); the skinned
    /// pipeline skins each species' static explicit-UV mesh by the frame's
    /// bone palette, so a model's arbitrary sub-rect UVs sample its own
    /// sheet. Then every player body and the items in their hands.
    pub(super) fn record(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        for g in &self.mob_gpu {
            if g.drawn.is_empty() {
                continue;
            }
            pass.set_bind_group(1, &g.bind, &[]);
            self.skin.draw(pass, ctx.samples, &g.mesh, g.drawn.clone());
        }
        // Player bodies — the local third-person body and every remote
        // player, one instanced draw (shared skin texture and mesh)…
        let bodies = &self.player_gpu;
        if !bodies.drawn.is_empty() {
            pass.set_bind_group(1, &bodies.bind, &[]);
            self.skin
                .draw(pass, ctx.samples, &bodies.mesh, bodies.drawn.clone());
        }
        // …their extruded-sprite held items (2D atlas)…
        if self.item_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            self.item_draw.draw(pass, ctx.samples);
        }
        // …their bbmodel held items (model atlas)…
        if self.model_item_draw.index_count > 0 {
            pass.set_bind_group(1, &ctx.binds.model_atlas, &[]);
            self.model_item_draw.draw(pass, ctx.samples);
        }
        // …and their held block mini-cubes (opaque pipeline + terrain atlas
        // array).
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

    /// PARTICLES: tiny alpha-CUTOUT terrain particle cubes that depth-test
    /// and depth-write, one instance row per particle (cube or oriented
    /// quad). Block flecks occupy the leading rows and draw with the block
    /// atlas; model flecks (bbmodel blocks) the trailing ones, with the model
    /// atlas.
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

    /// EMITTER PARTICLES: solid-colour cubes from block rows (torch flames,
    /// mod emitters) — alpha-blended, depth test without writes, back faces
    /// culled so transparency never exposes the whole cube shell.
    pub(super) fn record_emitters(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_bind_group(0, &ctx.binds.uniform, &[]);
        pass.set_bind_group(1, &ctx.binds.atlas, &[]);
        self.emitter_draw
            .draw(pass, ctx.samples, 0..self.emitter_draw.instance_count);
    }
}
