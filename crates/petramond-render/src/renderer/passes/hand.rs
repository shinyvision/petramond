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
        // HAND PASS: the first-person rig and its held items, drawn over the
        // world. Color Load; the world colour is already composited, so we
        // attach the main depth buffer with LoadOp::Clear(1.0) — clearing
        // depth gives the hand its own isolated depth space (it stays on top
        // of the world and never clips terrain) while still letting the held
        // geometry SELF-SORT. Held blocks go through the depth-enabled
        // model3d_hand pipeline; the arms, sprites and bbmodels through the
        // depth-tested item3d pipeline. Every stream draws through one MVP.
        if self.hand.index_count > 0
            || self.hand.item3d_vertex_count > 0
            || self.hand.off_item3d_count > 0
            || self.hand.arm_count > 0
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
            // Both hands' held blocks (model3d, depth-enabled hand variant).
            if self.hand.index_count > 0 {
                pass.set_pipeline(self.hand.model3d_pipe.get(samples));
                pass.set_bind_group(0, &self.hand.model3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.atlas_bind, &[]);
                pass.set_vertex_buffer(0, self.hand.model3d_vbuf.slice(..));
                pass.set_index_buffer(self.hand.model3d_ibuf.slice(..), wgpu::IndexFormat::Uint32);
                pass.draw_indexed(0..self.hand.index_count, 0, 0..1);
            }
            // The first-person rig's arms, in the player's skin.
            if self.hand.arm_count > 0 {
                pass.set_pipeline(self.hand.item3d_pipe.get(samples));
                pass.set_bind_group(0, &self.hand.item3d_mvp_bind, &[0]);
                pass.set_bind_group(1, &self.actor.player_gpu.bind, &[]);
                pass.set_vertex_buffer(0, self.hand.item3d_vbuf.slice(..));
                pass.draw(
                    self.hand.arm_start..self.hand.arm_start + self.hand.arm_count,
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
