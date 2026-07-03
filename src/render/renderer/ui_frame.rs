//! Per-frame UI geometry build + upload for [`Renderer`].
//!
//! Splits `build_ui_frame` out of the renderer god-file: it runs `build_ui`
//! into the reusable `UiBuild` scratch and uploads each quad group (solid
//! counts/dim, panel, overlays, hover, text, per-slot icon quads) to its own
//! buffer range so the UI pass binds the right texture per group.

use super::*;

impl Renderer {
    /// Build + upload this frame's UI geometry from the [`UiBuild`] that
    /// [`build_ui`] fills. Each quad group goes to its own buffer / range so the UI
    /// pass binds the right texture per group:
    /// - `ui_solid_vbuf`: dim backdrop `[0, dim)`, then stack counts, then drag
    ///   counts — all solid-color, drawn with the icon-atlas bind (the solid
    ///   sentinel skips the sampler).
    /// - `ui_panel_vbuf` / `ui_overlay_vbuf` / `ui_hover_vbuf`: the baked panel,
    ///   the dynamic overlays, and the hover highlight (each its own texture).
    /// - `ui_static_text_vbuf` / `ui_glyph_text_vbuf`: runtime text atlas quads,
    ///   split by whole-run rasterized labels and editable glyph-atlas text.
    /// - `icon_quad_vbuf`: one textured quad per filled slot sampling the item's
    ///   pre-baked icon-atlas cell — normal icons then cursor-held icons.
    pub(super) fn build_ui_frame(&mut self) {
        self.ui_dim_vertex_count = 0;
        self.ui_count_vertex_count = 0;
        self.ui_drag_count_vertex_count = 0;
        self.ui_panel_vertex_count = 0;
        self.ui_overlay_vertex_count = 0;
        self.ui_hover_vertex_count = 0;
        self.ui_shell_vertex_count = 0;
        self.ui_shell_scroll_thumb_vertex_count = 0;
        self.ui_hearts_vertex_count = 0;
        self.ui_static_text_vertex_count = 0;
        self.ui_glyph_text_vertex_count = 0;
        self.icon_quad_vertex_count = 0;
        self.drag_icon_quad_vertex_count = 0;

        // Disjoint-field borrow: `build_ui` reads the snapshot and writes the
        // scratch `UiBuild`, both distinct from the GPU buffers used below.
        build_ui(&self.ui, &mut self.ui_build);

        let screen = self.ui.screen;
        let cap = crate::render::pipeline::MAX_UI_VERTICES as usize;
        let vsize = std::mem::size_of::<UiVertex>();

        // Solid quads packed into one buffer: dim backdrop, then normal stack
        // counts, then the cursor-held count (drawn after the cursor icon).
        let dim = &self.ui_build.dim;
        let counts = &self.ui_build.counts;
        let drag_counts = &self.ui_build.drag_counts;
        if !dim.is_empty() && dim.len() <= cap {
            self.queue
                .write_buffer(&self.ui_solid_vbuf, 0, bytemuck::cast_slice(dim));
            self.ui_dim_vertex_count = dim.len() as u32;
        }
        let mut off = self.ui_dim_vertex_count as usize;
        if !counts.is_empty() && off + counts.len() <= cap {
            self.queue.write_buffer(
                &self.ui_solid_vbuf,
                (off * vsize) as u64,
                bytemuck::cast_slice(counts),
            );
            self.ui_count_vertex_count = counts.len() as u32;
            off += counts.len();
        }
        if !drag_counts.is_empty() && off + drag_counts.len() <= cap {
            self.queue.write_buffer(
                &self.ui_solid_vbuf,
                (off * vsize) as u64,
                bytemuck::cast_slice(drag_counts),
            );
            self.ui_drag_count_vertex_count = drag_counts.len() as u32;
        }

        // Baked panel + shell skin + dynamic overlays + hover highlight, each its own buffer.
        let panel = &self.ui_build.panel;
        if !panel.is_empty() && panel.len() <= cap {
            self.queue
                .write_buffer(&self.ui_panel_vbuf, 0, bytemuck::cast_slice(panel));
            self.ui_panel_vertex_count = panel.len() as u32;
        }
        let shell_skin = &self.ui_build.shell_skin;
        if !shell_skin.is_empty() && shell_skin.len() <= cap {
            self.queue
                .write_buffer(&self.ui_shell_vbuf, 0, bytemuck::cast_slice(shell_skin));
            self.ui_shell_vertex_count = shell_skin.len() as u32;
        }
        let shell_scroll_thumb = &self.ui_build.shell_scroll_thumb;
        if !shell_scroll_thumb.is_empty() && shell_scroll_thumb.len() <= cap {
            self.queue.write_buffer(
                &self.ui_shell_scroll_thumb_vbuf,
                0,
                bytemuck::cast_slice(shell_scroll_thumb),
            );
            self.ui_shell_scroll_thumb_vertex_count = shell_scroll_thumb.len() as u32;
        }
        let overlays = &self.ui_build.overlays;
        if !overlays.is_empty() && overlays.len() <= cap {
            self.queue
                .write_buffer(&self.ui_overlay_vbuf, 0, bytemuck::cast_slice(overlays));
            self.ui_overlay_vertex_count = overlays.len() as u32;
        }
        let hover = &self.ui_build.hover;
        if !hover.is_empty() && hover.len() <= cap {
            self.queue
                .write_buffer(&self.ui_hover_vbuf, 0, bytemuck::cast_slice(hover));
            self.ui_hover_vertex_count = hover.len() as u32;
        }
        let hearts = &self.ui_build.hearts;
        if !hearts.is_empty() && hearts.len() <= cap {
            self.queue
                .write_buffer(&self.ui_hearts_vbuf, 0, bytemuck::cast_slice(hearts));
            self.ui_hearts_vertex_count = hearts.len() as u32;
        }

        let mut static_text_verts = std::mem::take(&mut self.static_text_verts);
        self.static_text_atlas.prepare(
            &self.device,
            &self.queue,
            &self.ui_texture_bgl,
            screen,
            &self.ui_build.raster_text_runs,
            &mut static_text_verts,
        );
        self.ui_static_text_vertex_count = upload_ui_vertices(
            &self.device,
            &self.queue,
            &mut self.ui_static_text_vbuf,
            "ui static text vbuf",
            &static_text_verts,
        );
        self.static_text_verts = static_text_verts;

        let mut glyph_text_verts = std::mem::take(&mut self.glyph_text_verts);
        self.glyph_text_atlas.prepare(
            &self.device,
            &self.queue,
            &self.ui_texture_bgl,
            screen,
            &self.ui_build.glyph_text_runs,
            &mut glyph_text_verts,
        );
        self.ui_glyph_text_vertex_count = upload_ui_vertices(
            &self.device,
            &self.queue,
            &mut self.ui_glyph_text_vbuf,
            "ui glyph text vbuf",
            &glyph_text_verts,
        );
        self.glyph_text_verts = glyph_text_verts;

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

fn upload_ui_vertices(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    buffer: &mut wgpu::Buffer,
    label: &str,
    verts: &[UiVertex],
) -> u32 {
    if verts.is_empty() {
        return 0;
    }
    let bytes = bytemuck::cast_slice::<_, u8>(verts).len() as u64;
    if bytes > buffer.size() {
        *buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes.next_power_of_two(),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(verts));
    verts.len() as u32
}
