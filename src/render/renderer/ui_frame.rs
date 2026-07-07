//! Per-frame UI geometry build + upload for [`Renderer`].
//!
//! Runs `build_ui` into the reusable `UiBuild` scratch and uploads each quad
//! group (solid counts, hearts, per-slot icon quads) to its own buffer range
//! so the UI pass binds the right texture per group. Screen chrome is not
//! built here — the GUI-document draw list (`doc_ui`) owns it.

use super::*;

impl Renderer {
    /// Build + upload this frame's game-owned UI geometry from the [`UiBuild`]
    /// that [`build_ui`] fills:
    /// - `ui_solid_vbuf`: stack counts `[0, counts)`, then drag counts — all
    ///   solid-color, drawn with the icon-atlas bind (the solid sentinel skips
    ///   the sampler).
    /// - `ui_hearts_vbuf`: the HUD heart bar quads (heart atlas).
    /// - `icon_quad_vbuf`: one textured quad per filled slot sampling the item's
    ///   pre-baked icon-atlas cell — normal icons then cursor-held icons.
    pub(super) fn build_ui_frame(&mut self) {
        self.ui_count_vertex_count = 0;
        self.ui_drag_count_vertex_count = 0;
        self.ui_hearts_vertex_count = 0;
        self.ui_effects_vertex_count = 0;
        self.ui_vignette_vertex_count = 0;
        self.icon_quad_vertex_count = 0;
        self.drag_icon_quad_vertex_count = 0;

        // Disjoint-field borrow: `build_ui` reads the snapshot and writes the
        // scratch `UiBuild`, both distinct from the GPU buffers used below.
        build_ui(&self.ui, &mut self.ui_build);

        let screen = self.ui.screen;
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

        let hearts = &self.ui_build.hearts;
        if !hearts.is_empty() && hearts.len() <= cap {
            self.queue
                .write_buffer(&self.ui_hearts_vbuf, 0, bytemuck::cast_slice(hearts));
            self.ui_hearts_vertex_count = hearts.len() as u32;
        }

        let effects = &self.ui_build.effects;
        if !effects.is_empty() && effects.len() <= cap {
            self.queue
                .write_buffer(&self.ui_effects_vbuf, 0, bytemuck::cast_slice(effects));
            self.ui_effects_vertex_count = effects.len() as u32;
        }

        // Hurt vignette (fixed 24-vertex frame; the buffer is sized for it).
        let vignette = &self.ui_build.vignette;
        let vignette_cap = (self.ui_vignette_vbuf.size() / vsize as u64) as usize;
        if !vignette.is_empty() && vignette.len() <= vignette_cap {
            self.queue
                .write_buffer(&self.ui_vignette_vbuf, 0, bytemuck::cast_slice(vignette));
            self.ui_vignette_vertex_count = vignette.len() as u32;
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
