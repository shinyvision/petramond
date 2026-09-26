//! The first-person hand node.

use super::*;

impl HandPass {
    pub(super) fn active(&self) -> bool {
        self.index_count > 0
            || self.item3d_vertex_count > 0
            || self.off_item3d_count > 0
            || self.arm_count > 0
    }

    /// HAND: the first-person rig and its held items over the finished
    /// world, in the depth space the node's cleared attachment gives it (on
    /// top of the world, never clipping terrain, yet self-sorting). Held
    /// blocks go through the depth-enabled model3d hand pipeline; the arms,
    /// sprites and bbmodels through the depth-tested item3d pipeline. Every
    /// stream draws through one MVP. `skin` is the player skin the arms wear.
    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        skin: &wgpu::BindGroup,
    ) {
        // Both hands' held blocks (model3d, depth-enabled hand variant).
        if self.index_count > 0 {
            pass.set_pipeline(self.model3d_pipe.get(ctx.samples));
            pass.set_bind_group(0, &self.model3d_mvp_bind, &[0]);
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
            pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
        // The first-person rig's arms, in the player's skin.
        if self.arm_count > 0 {
            pass.set_pipeline(self.item3d_pipe.get(ctx.samples));
            pass.set_bind_group(0, &self.item3d_mvp_bind, &[0]);
            pass.set_bind_group(1, skin, &[]);
            pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
            pass.draw(self.arm_start..self.arm_start + self.arm_count, 0..1);
        }
        // Extruded held sprite (block atlas) OR a held bbmodel block (model
        // atlas) — both ride the item3d pipeline (non-indexed, depth-tested).
        if self.item3d_vertex_count > 0 {
            self.draw_item3d(
                pass,
                ctx,
                0,
                self.held_is_model,
                0..self.item3d_vertex_count,
            );
        }
        // The OFF hand's item3d stream (appended range, MVP slot 1).
        if self.off_item3d_count > 0 {
            let start = self.off_item3d_start;
            self.draw_item3d(
                pass,
                ctx,
                256,
                self.off_is_model,
                start..start + self.off_item3d_count,
            );
        }
    }

    /// One item3d vertex range through the MVP slot at `mvp_offset`, over
    /// the model atlas for a bbmodel item and the block atlas for a sprite.
    fn draw_item3d(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        mvp_offset: wgpu::DynamicOffset,
        is_model: bool,
        vertices: std::ops::Range<u32>,
    ) {
        pass.set_pipeline(self.item3d_pipe.get(ctx.samples));
        pass.set_bind_group(0, &self.item3d_mvp_bind, &[mvp_offset]);
        let atlas = if is_model {
            &ctx.binds.model_atlas
        } else {
            &ctx.binds.atlas
        };
        pass.set_bind_group(1, atlas, &[]);
        pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
        pass.draw(vertices, 0..1);
    }
}
