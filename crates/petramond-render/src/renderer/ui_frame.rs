use super::*;

impl Renderer {
    pub fn prepare_ui_frame(&mut self, frame: UiLayers<'_>) -> bool {
        if !frame.scene.matches_viewport(self.scene_ui_viewport())
            || !frame.window.matches_viewport(self.window_ui_viewport())
        {
            return false;
        }
        let Renderer {
            device, queue, ui, ..
        } = self;
        let gpu = super::doc_ui::UiGpu {
            device,
            queue,
            texture_bgl: &ui.texture_bgl,
        };
        for (layer, frame) in [
            (&mut ui.scene, &frame.scene),
            (&mut ui.window, &frame.window),
        ] {
            layer.prepare(&gpu, &mut ui.theme, &ui.icon_atlas, frame);
        }
        true
    }
}

impl UiLayer {
    fn prepare(
        &mut self,
        gpu: &super::doc_ui::UiGpu<'_>,
        theme: &mut Option<super::doc_ui::ThemeBinds>,
        icon_atlas: &IconAtlas,
        frame: &UiFrame<'_>,
    ) {
        let screen = frame.viewport.size;
        let scale = frame.viewport.scale as f32;
        let slots = frame.document.as_ref().map(|document| document.slots);
        let hooks = frame.document.as_ref().map(|document| document.hooks);
        self.doc_ui
            .prepare(gpu, theme, frame.document.as_ref(), screen);
        self.client_overlays.prepare(
            gpu,
            theme,
            frame.client_overlays,
            screen,
            frame.client_overlay_dim,
        );
        self.build_frame(gpu, icon_atlas, frame.content, screen, scale, slots, hooks);
        self.prepared_viewport = frame.viewport;
    }

    #[allow(clippy::too_many_arguments)]
    fn build_frame(
        &mut self,
        gpu: &super::doc_ui::UiGpu<'_>,
        icon_atlas: &IconAtlas,
        content: &UiSnapshot,
        screen: (u32, u32),
        scale: f32,
        slots: Option<&[petramond::gui::DocSlot]>,
        hooks: Option<&[petramond::gui::DocHook]>,
    ) {
        self.count_vertex_count = 0;
        self.overlay_count_vertex_count = 0;
        self.drag_count_vertex_count = 0;
        self.icon_quad_vertex_count = 0;
        self.overlay_icon_quad_vertex_count = 0;
        self.drag_icon_quad_vertex_count = 0;

        build_ui(content, screen, scale, slots, hooks, &mut self.build);

        let counts = &self.build.counts;
        let overlay_counts = &self.build.overlay_counts;
        let drag_counts = &self.build.drag_counts;
        let mut solid = std::mem::take(&mut self.solid_verts);
        solid.clear();
        solid.extend_from_slice(counts);
        solid.extend_from_slice(overlay_counts);
        solid.extend_from_slice(drag_counts);
        if !solid.is_empty() {
            super::dynamic_draw::upload(
                gpu.device,
                gpu.queue,
                &mut self.solid_vbuf,
                &solid,
                wgpu::BufferUsages::VERTEX,
                "ui solid vbuf",
            );
            self.count_vertex_count = counts.len() as u32;
            self.overlay_count_vertex_count = overlay_counts.len() as u32;
            self.drag_count_vertex_count = drag_counts.len() as u32;
        }
        self.solid_verts = solid;

        for layer in &mut self.hud_layers {
            layer.vertex_count = 0;
            let verts = (layer.source)(&self.build);
            if !verts.is_empty() {
                super::dynamic_draw::upload(
                    gpu.device,
                    gpu.queue,
                    &mut layer.vbuf,
                    verts,
                    wgpu::BufferUsages::VERTEX,
                    "hud layer vbuf",
                );
                layer.vertex_count = verts.len() as u32;
            }
        }

        let mut verts = std::mem::take(&mut self.icon_quad_verts);
        verts.clear();
        if screen.0 != 0 && screen.1 != 0 {
            for &(item, r, color, dyed) in &self.build.icon_quads {
                let [u0, v0, u1, v1] = if dyed {
                    icon_atlas.cell_uv_dyed(item)
                } else {
                    icon_atlas.cell_uv(item)
                };
                crate::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    color,
                );
            }
            let push_hooks = |verts: &mut Vec<UiVertex>, icons: &[crate::ui::HookIconQuad]| {
                for icon in icons {
                    let [u0, v0, u1, v1] = icon_atlas.cell_uv(icon.item);
                    let Some((visible, uv_tl, uv_br)) =
                        clipped_icon(icon.rect, icon.clip, [u0, v0, u1, v1])
                    else {
                        continue;
                    };
                    crate::ui::push_quad_uv(
                        verts,
                        screen,
                        visible.x,
                        visible.y,
                        visible.w,
                        visible.h,
                        uv_tl,
                        uv_br,
                        [1.0, 1.0, 1.0, if icon.dim { 0.35 } else { 1.0 }],
                    );
                }
            };
            push_hooks(&mut verts, &self.build.hook_icon_quads);
            let normal_icon_vertex_count = verts.len() as u32;
            push_hooks(&mut verts, &self.build.overlay_icon_quads);
            self.overlay_icon_quad_vertex_count = verts.len() as u32 - normal_icon_vertex_count;
            for &(item, r, color, dyed) in &self.build.drag_icon_quads {
                let [u0, v0, u1, v1] = if dyed {
                    icon_atlas.cell_uv_dyed(item)
                } else {
                    icon_atlas.cell_uv(item)
                };
                crate::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    color,
                );
            }
            self.icon_quad_vertex_count = normal_icon_vertex_count;
            self.drag_icon_quad_vertex_count =
                verts.len() as u32 - normal_icon_vertex_count - self.overlay_icon_quad_vertex_count;
        }
        if !verts.is_empty() {
            super::dynamic_draw::upload(
                gpu.device,
                gpu.queue,
                &mut self.icon_quad_vbuf,
                &verts,
                wgpu::BufferUsages::VERTEX,
                "icon quad vbuf",
            );
        }
        self.icon_quad_verts = verts;
    }
}

fn clipped_icon(
    rect: petramond::gui::SlotRect,
    clip: Option<petramond::gui::SlotRect>,
    uv: [f32; 4],
) -> Option<(petramond::gui::SlotRect, [f32; 2], [f32; 2])> {
    let visible = clip.map_or(Some(rect), |clip| crate::ui::intersect_rect(rect, clip))?;
    let fx0 = (visible.x - rect.x) / rect.w;
    let fy0 = (visible.y - rect.y) / rect.h;
    let fx1 = (visible.x + visible.w - rect.x) / rect.w;
    let fy1 = (visible.y + visible.h - rect.y) / rect.h;
    let du = uv[2] - uv[0];
    let dv = uv[3] - uv[1];
    Some((
        visible,
        [uv[0] + du * fx0, uv[1] + dv * fy0],
        [uv[0] + du * fx1, uv[1] + dv * fy1],
    ))
}
