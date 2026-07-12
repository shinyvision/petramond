//! Per-frame UI geometry build + upload for [`Renderer`].
//!
//! Runs `build_ui` into the reusable `UiBuild` scratch and uploads each quad
//! group (solid counts, hearts, per-slot icon quads) to its own buffer range
//! so the UI pass binds the right texture per group. Screen chrome is not
//! built here — the GUI-document draw list (`doc_ui`) owns it.

use super::*;

impl Renderer {
    /// Validate and prepare every UI layer as one viewport-stamped transaction.
    /// A resize-stale document rejects the whole packet before any layer state
    /// is cleared or uploaded.
    pub(crate) fn prepare_ui_frame(&mut self, frame: UiFrame<'_>) -> bool {
        if !frame.matches_viewport(self.ui_viewport()) {
            return false;
        }
        let screen = frame.viewport.size;
        let scale = frame.viewport.scale as f32;
        let slots = frame.document.as_ref().map(|document| document.slots);
        let hooks = frame.document.as_ref().map(|document| document.hooks);
        self.prepare_doc_ui(frame.document.as_ref(), screen);
        self.prepare_client_overlays(frame.client_overlays, screen, frame.client_overlay_dim);
        self.build_ui_frame(frame.content, screen, scale, slots, hooks);
        self.prepared_ui_viewport = frame.viewport;
        true
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
    fn build_ui_frame(
        &mut self,
        content: &UiSnapshot,
        screen: (u32, u32),
        scale: f32,
        slots: Option<&[crate::gui::DocSlot]>,
        hooks: Option<&[crate::gui::DocHook]>,
    ) {
        self.ui_count_vertex_count = 0;
        self.ui_drag_count_vertex_count = 0;
        self.icon_quad_vertex_count = 0;
        self.drag_icon_quad_vertex_count = 0;

        build_ui(content, screen, scale, slots, hooks, &mut self.ui_build);
        let cap = crate::render::pipeline::MAX_UI_VERTICES as usize;
        let vsize = std::mem::size_of::<UiVertex>();

        // Solid quads packed into one buffer: normal stack counts, then the
        // cursor-held count (drawn after the cursor icon).
        let counts = &self.ui_build.counts;
        let drag_counts = &self.ui_build.drag_counts;
        if !counts.is_empty() && counts.len() <= cap {
            self.queue
                .write_buffer(&self.ui_solid_vbuf, 0, bytemuck::cast_slice(counts));
            self.ui_count_vertex_count = counts.len() as u32;
        }
        let off = self.ui_count_vertex_count as usize;
        if !drag_counts.is_empty() && off + drag_counts.len() <= cap {
            self.queue.write_buffer(
                &self.ui_solid_vbuf,
                (off * vsize) as u64,
                bytemuck::cast_slice(drag_counts),
            );
            self.ui_drag_count_vertex_count = drag_counts.len() as u32;
        }

        // HUD chrome layers: each layer's UiBuild vec to its own buffer.
        for layer in &mut self.hud_layers {
            layer.vertex_count = 0;
            let verts = (layer.source)(&self.ui_build);
            if !verts.is_empty() && verts.len() <= cap {
                self.queue
                    .write_buffer(&layer.vbuf, 0, bytemuck::cast_slice(verts));
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
            for &(item, r) in &self.ui_build.icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                crate::render::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            // Greyed (semi-transparent) icons — workbench results not yet craftable.
            // Same icon-atlas quad, drawn at reduced alpha so the panel shows through.
            for &(item, r) in &self.ui_build.dim_icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                crate::render::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 0.35],
                );
            }
            for icon in &self.ui_build.hook_icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(icon.item);
                let Some((visible, uv_tl, uv_br)) =
                    clipped_icon(icon.rect, icon.clip, [u0, v0, u1, v1])
                else {
                    continue;
                };
                crate::render::ui::push_quad_uv(
                    &mut verts,
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
            let normal_icon_vertex_count = verts.len() as u32;
            for &(item, r) in &self.ui_build.drag_icon_quads {
                let [u0, v0, u1, v1] = self.icon_atlas.cell_uv(item);
                crate::render::ui::push_quad_uv(
                    &mut verts,
                    screen,
                    r.x,
                    r.y,
                    r.w,
                    r.h,
                    [u0, v0],
                    [u1, v1],
                    [1.0, 1.0, 1.0, 1.0],
                );
            }
            self.icon_quad_vertex_count = normal_icon_vertex_count;
            self.drag_icon_quad_vertex_count = verts.len() as u32 - normal_icon_vertex_count;
        }
        if !verts.is_empty() {
            // Icon-quad geometry is bounded by the visible slots but GROW the buffer to
            // fit rather than capping — a fixed cap that drops the batch when exceeded
            // would blank EVERY icon at once. Grow to the next power of two so it
            // doesn't reallocate every frame.
            let bytes = bytemuck::cast_slice::<_, u8>(verts.as_slice()).len() as u64;
            if bytes > self.icon_quad_vbuf.size() {
                self.icon_quad_vbuf = self.device.create_buffer(&wgpu::BufferDescriptor {
                    label: Some("icon quad vbuf"),
                    size: bytes.next_power_of_two(),
                    usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                    mapped_at_creation: false,
                });
            }
            self.queue
                .write_buffer(&self.icon_quad_vbuf, 0, bytemuck::cast_slice(&verts));
        }
        self.icon_quad_verts = verts;
    }
}

fn clipped_icon(
    rect: crate::gui::SlotRect,
    clip: Option<crate::gui::SlotRect>,
    uv: [f32; 4],
) -> Option<(crate::gui::SlotRect, [f32; 2], [f32; 2])> {
    let visible = clip.map_or(Some(rect), |clip| {
        crate::render::ui::intersect_rect(rect, clip)
    })?;
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
