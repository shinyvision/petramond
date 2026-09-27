use super::*;

impl SceneTargets {
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

    pub(super) fn record_outline(&self, pass: &mut wgpu::RenderPass<'_>, ctx: &PassCtx<'_>) {
        pass.set_pipeline(self.outline_pipe.get(ctx.samples));
        pass.set_bind_group(0, &self.outline_bind, &[]);
        pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
        pass.draw(0..self.outline_vertex_count, 0..1);
    }

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
    pub(super) fn record_base(&self, pass: &mut wgpu::RenderPass<'_>, layer: &UiLayer) {
        let theme = self.theme.as_ref();
        let solid = &self.icon_atlas.bind;
        let screen = layer.prepared_viewport.size;
        pass.set_pipeline(&self.pipe);
        self.draw_hud_layers(pass, layer, true);
        layer
            .doc_ui
            .draw(pass, layer.doc_ui.base(), theme, solid, screen);
        self.draw_hud_layers(pass, layer, false);
        layer.client_overlays.draw(pass, theme, solid, screen);
        if layer.icon_quad_vertex_count > 0 {
            self.draw_icons(pass, layer, 0..layer.icon_quad_vertex_count);
        }
    }

    pub(super) fn record_overlay(&self, pass: &mut wgpu::RenderPass<'_>, layer: &UiLayer) {
        pass.set_pipeline(&self.pipe);
        let counts = layer.count_vertex_count;
        let overlay_counts = layer.overlay_count_vertex_count;
        let icons = layer.icon_quad_vertex_count;
        let overlay_icons = layer.overlay_icon_quad_vertex_count;
        if counts > 0 {
            self.draw_solid(pass, layer, 0..counts);
        }
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
                continue;
            };
            pass.set_bind_group(0, bind, &[]);
            pass.set_vertex_buffer(0, hud.vbuf.slice(..));
            pass.draw(0..hud.vertex_count, 0..1);
        }
    }

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
