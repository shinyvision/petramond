//! The first-person hand pass.

use super::*;

impl Renderer {
    /// The held item / bare hand over the finished world, in its own cleared
    /// depth space.
    pub(super) fn encode_hand(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        samples: u32,
    ) {
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
                pass.set_pipeline(self.hand.model3d_pipe.get(samples));
                pass.set_bind_group(0, &self.hand.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.hand.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.hand.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand.index_count, 0, 0..1);
            }
            // The OFF hand's held block: its geometry appends after the main
            // hand's in the shared buffers; MVP slot 1.
            if self.hand.off_index_count > 0 {
                pass.set_pipeline(self.hand.model3d_pipe.get(samples));
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
                pass.set_pipeline(self.hand.item3d_pipe.get(samples));
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
                pass.set_pipeline(self.hand.item3d_pipe.get(samples));
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
    }
}
