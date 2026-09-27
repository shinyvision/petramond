//! From the finished world image to the frame: the post-process node, the
//! world-space outline, and the scene's chrome (crosshair, the scene's UI
//! layer, drag overlay) drawn ungraded over the final image. The MSAA
//! resolve is no node of its own: the graph resolves at the end of the last
//! world pass. The same UI-layer recording also draws the window's layer,
//! after the frame's capture point (`frame.rs`).

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

impl UiLayer {
    pub(super) fn base_active(&self) -> bool {
        self.hud_layers.iter().any(|l| l.vertex_count > 0)
            || self.icon_quad_vertex_count > 0
            || !self.doc_ui.base().is_empty()
            || !self.client_overlays.batches.is_empty()
    }

    pub(super) fn overlay_active(&self) -> bool {
        self.count_vertex_count > 0
            || self.drag_icon_quad_vertex_count > 0
            || self.drag_count_vertex_count > 0
            || self.overlay_icon_quad_vertex_count > 0
            || self.overlay_count_vertex_count > 0
            || !self.doc_ui.overlay().is_empty()
    }
}

impl UiPass {
    /// UI: under-chrome HUD layers (hurt vignette) → the GUI-document draw
    /// list (all screen chrome, including its own dim backdrop) → the
    /// over-chrome HUD layers (hearts, status effects, …) → client overlays →
    /// per-slot item icons, all through the UI pipeline (own alpha blend, no
    /// depth). Each layer binds its own texture; solid quads bind the icon
    /// atlas (the solid sentinel skips the sampler, so any layout-compatible
    /// texture works).
    pub(super) fn record_base(&self, pass: &mut wgpu::RenderPass<'_>, layer: &UiLayer) {
        let theme = self.theme.as_ref();
        let solid = &self.icon_atlas.bind;
        let screen = layer.prepared_viewport.size;
        pass.set_pipeline(&self.pipe);
        self.draw_hud_layers(pass, layer, true);
        // The GUI-document draw list: every panel, slot face, hover, gauge,
        // text and dim quad of the frame's screen.
        layer
            .doc_ui
            .draw(pass, layer.doc_ui.base(), theme, solid, screen);
        self.draw_hud_layers(pass, layer, false);
        layer.client_overlays.draw(pass, theme, solid, screen);
        // Per-slot item icons (icon atlas), one bind + one draw.
        if layer.icon_quad_vertex_count > 0 {
            self.draw_icons(pass, layer, 0..layer.icon_quad_vertex_count);
        }
    }

    /// UI OVERLAY / DRAG: stack counts, then the document's overlay tier
    /// (floating tooltip chrome) with its own icons and counts over the base
    /// tier's, then the cursor-held icon and its count — keeping the whole
    /// dragged stack front-most. Icons and counts of each tier sit back to
    /// back in the shared icon and solid buffers: normal, tooltip, drag.
    pub(super) fn record_overlay(&self, pass: &mut wgpu::RenderPass<'_>, layer: &UiLayer) {
        pass.set_pipeline(&self.pipe);
        let counts = layer.count_vertex_count;
        let overlay_counts = layer.overlay_count_vertex_count;
        let icons = layer.icon_quad_vertex_count;
        let overlay_icons = layer.overlay_icon_quad_vertex_count;
        if counts > 0 {
            self.draw_solid(pass, layer, 0..counts);
        }
        // Floating tooltip chrome, over every base-tier icon and count.
        layer.doc_ui.draw(
            pass,
            layer.doc_ui.overlay(),
            self.theme.as_ref(),
            &self.icon_atlas.bind,
            layer.prepared_viewport.size,
        );
        if overlay_icons > 0 {
            self.draw_icons(pass, layer, icons..icons + overlay_icons);
        }
        if overlay_counts > 0 {
            self.draw_solid(pass, layer, counts..counts + overlay_counts);
        }
        if layer.drag_icon_quad_vertex_count > 0 {
            let start = icons + overlay_icons;
            self.draw_icons(
                pass,
                layer,
                start..start + layer.drag_icon_quad_vertex_count,
            );
        }
        if layer.drag_count_vertex_count > 0 {
            let start = counts + overlay_counts;
            self.draw_solid(pass, layer, start..start + layer.drag_count_vertex_count);
        }
    }

    /// The HUD layers on one side of the document chrome, in list order.
    fn draw_hud_layers(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        layer: &UiLayer,
        under_chrome: bool,
    ) {
        for hud in layer
            .hud_layers
            .iter()
            .filter(|l| l.under_chrome == under_chrome)
        {
            if hud.vertex_count == 0 {
                continue;
            }
            let bind = match &hud.texture {
                HudLayerTexture::Solid => Some(&self.icon_atlas.bind),
                HudLayerTexture::Texture(b) => b.as_ref(),
            };
            let Some(bind) = bind else {
                continue; // the layer's art is missing — draw nothing
            };
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, hud.vbuf.slice(..));
            pass.draw(0..hud.vertex_count, 0..1);
        }
    }

    /// A vertex range of the layer's icon-quad buffer, sampling the icon atlas.
    fn draw_icons(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        layer: &UiLayer,
        vertices: std::ops::Range<u32>,
    ) {
        pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
        pass.set_vertex_buffer(0, layer.icon_quad_vbuf.slice(..));
        pass.draw(vertices, 0..1);
    }

    /// A vertex range of the layer's solid-quad buffer (stack counts), drawn
    /// with the icon-atlas bind the solid sentinel ignores.
    fn draw_solid(
        &self,
        pass: &mut wgpu::RenderPass<'_>,
        layer: &UiLayer,
        vertices: std::ops::Range<u32>,
    ) {
        pass.set_bind_group(0, &self.icon_atlas.bind, &[]);
        pass.set_vertex_buffer(0, layer.solid_vbuf.slice(..));
        pass.draw(vertices, 0..1);
    }
}

impl Renderer {
    /// One UI layer over `target`, outside the frame graph: the window's
    /// layer, drawn after the frame's capture point. The same recording as
    /// the graph's `Ui` and `UiOverlay` nodes, in one pass.
    pub(in crate::renderer) fn encode_ui_layer(
        &self,
        enc: &mut wgpu::CommandEncoder,
        target: &wgpu::TextureView,
        layer: &UiLayer,
    ) {
        let (base, overlay) = (layer.base_active(), layer.overlay_active());
        if !base && !overlay {
            return;
        }
        let mut pass = overlay_pass(enc, target, "window ui pass", self.gpu_timer.as_ref());
        if base {
            self.ui.record_base(&mut pass, layer);
        }
        if overlay {
            self.ui.record_overlay(&mut pass, layer);
        }
    }
}

/// A pass drawing over what `target` already holds, with no depth: the
/// window's layers over the finished frame.
pub(in crate::renderer) fn overlay_pass<'e>(
    enc: &'e mut wgpu::CommandEncoder,
    target: &wgpu::TextureView,
    label: &'static str,
    timer: Option<&gpu_timer::GpuTimer>,
) -> wgpu::RenderPass<'e> {
    enc.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some(label),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: target,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Load,
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: timer.and_then(|t| t.pass(label)),
        occlusion_query_set: None,
    })
}
