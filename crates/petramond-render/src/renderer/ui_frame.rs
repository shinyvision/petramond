//! Per-frame UI geometry build + upload for [`Renderer`].
//!
//! Runs `build_ui` into the reusable `UiBuild` scratch and uploads each quad
//! group (solid counts, hearts, per-slot icon quads) to its own buffer range
//! so the UI pass binds the right texture per group. Screen chrome is not
//! built here — the GUI-document draw list (`doc_ui`) owns it.

use super::*;

impl Renderer {
    /// Validate and prepare both UI layers as one viewport-stamped
    /// transaction: the scene's at the frame's size, the window's at the
    /// window's. A resize-stale document rejects the whole packet before any
    /// layer state is cleared or uploaded.
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

    /// Build + upload this frame's game-owned UI geometry from the [`UiBuild`]
    /// that [`build_ui`] fills:
    /// - `ui_solid_vbuf`: stack counts `[0, counts)`, then drag counts — all
    ///   solid-color, drawn with the icon-atlas bind (the solid sentinel skips
    ///   the sampler).
    /// - each `hud_layers` entry (vignette, hearts, effects, …): its `UiBuild`
    ///   vec to its own buffer.
    /// - `icon_quad_vbuf`: one textured quad per filled slot sampling the item's
    ///   pre-baked icon-atlas cell — normal icons then cursor-held icons.
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

        // Solid quads packed into one buffer in draw order: normal stack
        // counts, the tooltip's counts (after the overlay chrome), then the
        // cursor-held count (drawn after the cursor icon). One upload, the
        // buffer grown to fit.
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

        // HUD chrome layers: each layer's UiBuild vec to its own buffer.
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

        // Per-slot item icons: resolve each recorded `(item, slot rect)` to the item's
        // pre-baked icon-atlas cell and emit a textured quad (6 verts) — slot rect →
        // NDC, cell rect → uv, white tint (so the quad samples the atlas, not the solid
        // sentinel). Normal icons draw in the UI pass; cursor-held icons are appended
        // to the same buffer but drawn later, after normal stack-count overlays.
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
