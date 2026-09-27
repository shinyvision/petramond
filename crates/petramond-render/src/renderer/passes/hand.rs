use super::*;

impl HandPass {
    pub(super) fn active(&self) -> bool {
        self.index_count > 0
            || self.item3d_vertex_count > 0
            || self.off_item3d_count > 0
            || self.arm_count > 0
    }

    pub(super) fn record(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        ctx: &PassCtx<'_>,
        skin: &wgpu::BindGroup,
    ) {
        if self.index_count > 0 {
            pass.set_pipeline(self.model3d_pipe.get(ctx.samples));
            pass.set_bind_group(0, &self.model3d_mvp_bind, &[0]);
            pass.set_bind_group(1, &ctx.binds.atlas, &[]);
            pass.set_vertex_buffer(0, self.model3d_vbuf.slice(..));
            pass.set_index_buffer(self.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..self.index_count, 0, 0..1);
        }
        if self.arm_count > 0 {
            pass.set_pipeline(self.item3d_pipe.get(ctx.samples));
            pass.set_bind_group(0, &self.item3d_mvp_bind, &[0]);
            pass.set_bind_group(1, skin, &[]);
            pass.set_vertex_buffer(0, self.item3d_vbuf.slice(..));
            pass.draw(self.arm_start..self.arm_start + self.arm_count, 0..1);
        }
        if self.item3d_vertex_count > 0 {
            self.draw_item3d(
                pass,
                ctx,
                0,
                self.held_is_model,
                0..self.item3d_vertex_count,
            );
        }
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
