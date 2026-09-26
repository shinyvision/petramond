//! From the finished world image to the screen: the post-process node, the
//! world-space outline, and the screen chrome (crosshair, UI, drag overlay)
//! drawn ungraded over the final image. The MSAA resolve is no node of its
//! own: the graph resolves at the end of the last world pass.

use super::*;

impl SceneTargets {
    /// POST-PROCESS: supersample reduction + colour grade (or low-resolution
    /// upscale) of the finished world image, scene texture → swapchain (see
    /// `grade.wgsl`). Everything after it draws ungraded over the result.
    pub(super) fn record_grade(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.grade_pipe);
        pass.set_bind_group(0, &self.grade_bind, &[]);
        pass.draw(0..3, 0..1);
    }
}

impl ChromePass {
    pub(super) fn outline_active(&self) -> bool {
        self.selection.is_some() && self.outline_vertex_count > 0
    }

    pub(super) fn crosshair_active(&self) -> bool {
        self.crosshair_vertex_count > 0
    }

    /// OUTLINE: the targeted block's wireframe, depth-tested without writes
    /// so it draws over terrain and water at the target but stays occluded
    /// behind nearer geometry.
    pub(super) fn record_outline(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.outline_pipe.get(ctx.samples));
        pass.set_bind_group(0, &self.outline_bind, &[]);
        pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
        pass.draw(0..self.outline_vertex_count, 0..1);
    }

    /// CROSSHAIR: the centre invert-blend crosshair.
    pub(super) fn record_crosshair(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.crosshair_pipe);
        pass.set_vertex_buffer(0, self.crosshair_vbuf.slice(..));
        pass.draw(0..self.crosshair_vertex_count, 0..1);
    }
}

impl UiPass {
    pub(super) fn base_active(&self) -> bool {
        self.hud_layers.iter().any(|l| l.vertex_count > 0)
            || self.icon_quad_vertex_count > 0
            || !self.doc_ui.batches.is_empty()
            || !self.client_overlays.batches.is_empty()
    }

    pub(super) fn overlay_active(&self) -> bool {
        self.count_vertex_count > 0
            || self.drag_icon_quad_vertex_count > 0
            || self.drag_count_vertex_count > 0
            || self.overlay_icon_quad_vertex_count > 0
            || self.overlay_count_vertex_count > 0
            || self.has_doc_overlay()
    }

    /// UI: under-chrome HUD layers (hurt vignette) → the GUI-document draw
    /// list (all screen chrome, including its own dim backdrop) → the
    /// over-chrome HUD layers (hearts, status effects, …) → client overlays →
    /// per-slot item icons, all through the UI pipeline (own alpha blend, no
    /// depth). Each layer binds its own texture; solid quads bind the icon
    /// atlas (the solid sentinel skips the sampler, so any layout-compatible
    /// texture works).
    pub(super) fn record_base(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipe);
        self.draw_hud_layers(pass, true);
        // The GUI-document draw list: every panel, slot face, hover, gauge,
        // text and dim quad of the frame's screen.
        self.draw_doc_ui(pass);
        self.draw_hud_layers(pass, false);
        self.draw_client_overlays(pass);
        // Per-slot item icons (icon atlas), one bind + one draw.
        if self.icon_quad_vertex_count > 0 {
            self.draw_icons(pass, 0..self.icon_quad_vertex_count);
        }
    }

    /// UI OVERLAY / DRAG: stack counts, then the document's overlay tier
    /// (floating tooltip chrome) with its own icons and counts over the base
    /// tier's, then the cursor-held icon and its count — keeping the whole
    /// dragged stack front-most. Icons and counts of each tier sit back to
    /// back in the shared icon and solid buffers: normal, tooltip, drag.
    pub(super) fn record_overlay(&self, pass: &mut wgpu::RenderPass<'_>) {
        pass.set_pipeline(&self.pipe);
        let counts = self.count_vertex_count;
        let overlay_counts = self.overlay_count_vertex_count;
        let icons = self.icon_quad_vertex_count;
        let overlay_icons = self.overlay_icon_quad_vertex_count;
        if counts > 0 {
            self.draw_solid(pass, 0..counts);
        }
        // Floating tooltip chrome, over every base-tier icon and count.
        self.draw_doc_ui_overlay(pass);
        if overlay_icons > 0 {
            self.draw_icons(pass, icons..icons + overlay_icons);
        }
        if overlay_counts > 0 {
            self.draw_solid(pass, counts..counts + overlay_counts);
        }
        if self.drag_icon_quad_vertex_count > 0 {
            let start = icons + overlay_icons;
            self.draw_icons(pass, start..start + self.drag_icon_quad_vertex_count);
        }
        if self.drag_count_vertex_count > 0 {
            let start = counts + overlay_counts;
            self.draw_solid(pass, start..start + self.drag_count_vertex_count);
        }
    }

    /// The HUD layers on one side of the document chrome, in list order.
    fn draw_hud_layers(&self, pass: &mut wgpu::RenderPass<'_>, under_chrome: bool) {
        for layer in self
            .hud_layers
            .iter()
            .filter(|l| l.under_chrome == under_chrome)
        {
            if layer.vertex_count == 0 {
                continue;
            }
            let bind = match &layer.texture {
                HudLayerTexture::Solid => Some(&self.icon_atlas.bind),
                HudLayerTexture::Texture(b) => b.as_ref(),
            };
            let Some(bind) = bind else {
                continue; // the layer's art is missing — draw nothing
            };
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, layer.vbuf.slice(..));
            pass.draw(0..layer.vertex_count, 0..1);
        }
    }

    /// A vertex range of the icon-quad buffer, sampling the icon atlas.
    fn draw_icons(&self, pass: &mut wgpu::RenderPass<'_>, vertices: std::ops::Range<u32>) {
        pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
        pass.set_vertex_buffer(0, self.icon_quad_vbuf.slice(..));
        pass.draw(vertices, 0..1);
    }

    /// A vertex range of the solid-quad buffer (stack counts), drawn with the
    /// icon-atlas bind the solid sentinel ignores.
    fn draw_solid(&self, pass: &mut wgpu::RenderPass<'_>, vertices: std::ops::Range<u32>) {
        pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
        pass.set_vertex_buffer(0, self.solid_vbuf.slice(..));
        pass.draw(vertices, 0..1);
    }
}
