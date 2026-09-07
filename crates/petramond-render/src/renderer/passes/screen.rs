//! From the finished world image to the swapchain: the MSAA resolve, the
//! post-process pass, and the screen chrome (crosshair, UI, drag overlay)
//! drawn ungraded over it.

use super::*;
use crate::renderer::post_process::SceneRoute;

impl Renderer {
    pub(super) fn encode_screen(
        &self,
        enc: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        swapchain: &wgpu::TextureView,
        samples: u32,
        route: SceneRoute,
    ) {
        if samples > 1 {
            // With no grade the resolve IS the final image and lands on the
            // swapchain; otherwise it feeds the post-process pass.
            let resolve_target = if route == SceneRoute::ResolveToSwapchain {
                swapchain
            } else {
                &self.targets.scene_color
            };
            let _resolve = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("MSAA resolve"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view,
                    depth_slice: None,
                    resolve_target: Some(resolve_target),
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Discard,
                    },
                })],
                timestamp_writes: self.gpu_timer.as_ref().and_then(|t| t.pass("MSAA resolve")),
                ..Default::default()
            });
        }
        // POST-PROCESS PASS: supersample reduction + colour grade (or
        // low-resolution upscale) of the finished world image, scene texture →
        // swapchain (see grade.wgsl). Everything after this draws ungraded over
        // the graded world. Skipped when the world already reached the swapchain.
        if route == SceneRoute::PostProcess {
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
