use super::geometry_arena::{GeometryArena, LayerAlloc};
use crate::atlas::decode_atlas_mips;
use crate::chunk::SectionPos;
use crate::mesh::{ChunkMesh, ContactShadowVertex, ModelVertex, TerrainVertex, Vertex};
use crate::texture_mips::build_cutout_mips;

/// Upload a standalone GUI PNG (e.g. the HUD heart atlas) as its own
/// texture + nearest sampler (sRGB, like the gui atlas). Arbitrary size —
/// each PNG is its own image, not a fixed atlas slot.
pub(super) fn create_gui_panel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    png: &[u8],
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let img = image::load_from_memory(png)
        .expect("decode gui panel png")
        .to_rgba8();
    create_rgba_nearest(device, queue, &img, "gui panel")
}

/// Upload one pack sky texture for a shader texture slot.
pub(super) fn create_sky_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
) -> Option<(wgpu::Texture, wgpu::TextureView, wgpu::Sampler)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(create_rgba_nearest(device, queue, &img, "sky texture"))
}

/// Upload a single fallback pixel for fixed bind slots whose pack texture is
/// absent or invalid.
pub(super) fn create_solid_rgba_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: [u8; 4],
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let img = image::RgbaImage::from_pixel(1, 1, image::Rgba(rgba));
    create_rgba_nearest(device, queue, &img, label)
}

/// Shared single-mip sRGB upload + nearest ClampToEdge sampler for arbitrary
/// standalone RGBA images.
pub(super) fn create_rgba_nearest(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &image::RgbaImage,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (w, h) = (img.width(), img.height());
    let texture = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some(label),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: &texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        img.as_raw(),
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some(label),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Nearest,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// Upload an entity/model RGBA texture (decoded from a `.bbmodel`) as its own GPU
/// texture + nearest sampler — a SEPARATE atlas from the block atlas, because model
/// faces carry arbitrary sub-rectangle UVs into this sheet (see `crate::bbmodel`).
/// Mips use cutout-alpha expansion so thin transparent decals, like the workbench's
/// tabletop grid, stay stable at distance under the shader's alpha test.
///
/// `w`/`h` of 0 are clamped to 1 so a missing/empty texture still yields a valid 1×1
/// binding.
pub(super) fn create_model_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let w = w.max(1);
    let h = h.max(1);
    let mips = build_cutout_mips(rgba, w, h);
    let texture = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("entity model texture"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    for (level, mip) in mips.iter().enumerate() {
        let level_w = (w >> level).max(1);
        let level_h = (h >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            mip,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level_w * 4),
                rows_per_image: Some(level_h),
            },
            wgpu::Extent3d {
                width: level_w,
                height: level_h,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("entity model sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: (mips.len() - 1) as f32,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// The process-wide index buffer every implied-triangulation terrain draw uses
/// (see [`crate::mesh::QuadIdx`]): `0,1,2, 0,2,3` repeated, so a draw is
/// `draw_indexed(0..6*quads, base_vertex = the section's first vertex)`.
///
/// It replaces a per-column index allocation for the opaque and far-LOD streams
/// — 122 MiB of VRAM at render distance 32, and the same again on the CPU side,
/// for one buffer of a couple of megabytes.
pub(super) struct QuadIndexBuffer {
    buf: wgpu::Buffer,
    quads: u32,
}

/// Quads the shared index buffer covers on creation. Sized so an ordinary
/// column's whole-column opaque draw never has to grow it.
const QUAD_INDEX_INITIAL: u32 = 1 << 16;

impl QuadIndexBuffer {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue) -> Self {
        Self {
            buf: Self::build(device, queue, QUAD_INDEX_INITIAL),
            quads: QUAD_INDEX_INITIAL,
        }
    }

    fn build(device: &wgpu::Device, queue: &wgpu::Queue, quads: u32) -> wgpu::Buffer {
        let mut data: Vec<u32> = Vec::with_capacity(quads as usize * 6);
        for q in 0..quads {
            let b = q * 4;
            data.extend_from_slice(&[b, b + 1, b + 2, b, b + 2, b + 3]);
        }
        let buf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("shared quad index buffer"),
            size: (data.len() * 4) as u64,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        queue.write_buffer(&buf, 0, bytemuck::cast_slice(&data));
        buf
    }

    /// Guarantee the buffer covers a draw of `quads` quads.
    pub(super) fn ensure(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, quads: u32) {
        if quads <= self.quads {
            return;
        }
        let grown = quads.next_power_of_two().max(self.quads * 2);
        self.buf = Self::build(device, queue, grown);
        self.quads = grown;
    }

    pub(super) fn slice(&self) -> wgpu::BufferSlice<'_> {
        self.buf.slice(..)
    }
}

pub struct GpuSectionMesh {
    /// World-space minimum corner `(x, y, z)` of this section.
    pub origin: (i32, i32, i32),
    pub opaque_vertex_start: u32,
    pub opaque_vertex_count: u32,
    pub far_opaque_vertex_start: u32,
    pub far_opaque_vertex_count: u32,
    pub transparent_vertex_start: u32,
    pub transparent_vertex_count: u32,
    /// The cull-none water-top stream (see [`crate::mesh::ChunkMesh`]).
    pub transparent_ts_vertex_start: u32,
    pub transparent_ts_vertex_count: u32,
    pub translucent_vertex_start: u32,
    pub translucent_vertex_count: u32,
    pub model_index_start: u32,
    pub model_idx_count: u32,
    pub model_vertex_start: u32,
    pub model_vertex_count: u32,
    /// Contact-shadow VERTEX range (the stream is non-indexed). Kept per section
    /// only so `plan_draw_order` can decide column contact visibility from the
    /// VISIBLE sections — the draw itself is whole-column. A section may hold
    /// contact vertices with `model_idx_count == 0` (a multi-cell model whose
    /// cuboids all render from a sibling cell), so contact visibility must NOT
    /// be inferred from the model range.
    pub contact_vertex_start: u32,
    pub contact_vertex_count: u32,
    /// Fingerprint of the section-local index streams (see
    /// [`section_index_hash`]). Guards the vertex-only patch path: equal layer
    /// counts pin every start offset, but NOT the index topology — plant quads
    /// (12 indices per 4 vertices) interleave with inline cube faces in cell
    /// order, and faces migrate between the inline and deferred-greedy
    /// partitions as their merge keys change, so two meshes with identical
    /// counts can wire vertices differently. Patching vertices under stale
    /// indices collapses such quads to degenerate triangles.
    pub index_hash: u64,
}

pub struct GpuColumnMesh {
    pub opaque_vbuf: Option<Layer>,
    /// Quads in the column's opaque stream — the draw's index count is six per
    /// quad against the shared quad index buffer.
    pub opaque_quads: u32,
    pub far_opaque_vbuf: Option<Layer>,
    pub transparent_vbuf: Option<Layer>,
    pub transparent_ts_vbuf: Option<Layer>,
    pub translucent_vbuf: Option<Layer>,
    pub model_vbuf: Option<Layer>,
    pub model_ibuf: Option<Layer>,
    pub model_idx_count: u32,
    /// The column's whole contact-shadow stream (non-indexed 16-byte
    /// `ContactShadowVertex`), drawn once per visible contact-bearing column.
    pub contact_vbuf: Option<Layer>,
    pub contact_vertex_count: u32,
    /// This column's slot in the shared instance-step origin table
    /// ([`ColumnOrigins`]); `vs_terrain` reads `[ox, 0, oz, 0]` from it via the
    /// draw's `first_instance`.
    pub origin_slot: ColumnOriginSlot,
    pub col_ox: i32,
    pub col_oz: i32,
    pub sections: Vec<(SectionPos, GpuSectionMesh)>,
    /// `(min_cy, max_cy)` over `sections`, or `(i32::MAX, i32::MIN)` when the
    /// column holds none. The whole-column cull AABB derives from it, so it is
    /// stored rather than re-folded over the section list every frame.
    pub cy_span: (i32, i32),
}

#[derive(Default)]
pub(super) struct ColumnUploadScratch {
    opaque: Vec<TerrainVertex>,
    far_opaque: Vec<TerrainVertex>,
    transparent: Vec<TerrainVertex>,
    transparent_two_sided: Vec<TerrainVertex>,
    translucent: Vec<TerrainVertex>,
    model: Vec<ModelVertex>,
    model_idx: Vec<u32>,
    contact: Vec<ContactShadowVertex>,
}

impl ColumnUploadScratch {
    fn clear(&mut self) {
        self.opaque.clear();
        self.far_opaque.clear();
        self.transparent.clear();
        self.transparent_two_sided.clear();
        self.translucent.clear();
        self.model.clear();
        self.model_idx.clear();
        self.contact.clear();
    }

    fn reserve_for(&mut self, meshes: &[(SectionPos, &ChunkMesh)]) {
        self.opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.opaque.len()).sum());
        self.far_opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.far_opaque.len()).sum());
        self.transparent
            .reserve(meshes.iter().map(|(_, mesh)| mesh.transparent.len()).sum());
        self.transparent_two_sided.reserve(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.transparent_two_sided.len())
                .sum(),
        );
        self.translucent
            .reserve(meshes.iter().map(|(_, mesh)| mesh.translucent.len()).sum());
        self.model
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model.len()).sum());
        self.model_idx
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model_idx.len()).sum());
        self.contact
            .reserve(meshes.iter().map(|(_, mesh)| mesh.contact.len()).sum());
    }
}

pub(super) fn create_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (mips, w, h) = decode_atlas_mips();
    let texture = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("atlas"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: mips.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    for (level, rgba) in mips.iter().enumerate() {
        let level_w = (w >> level).max(1);
        let level_h = (h >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            rgba,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(level_w * 4),
                rows_per_image: Some(level_h),
            },
            wgpu::Extent3d {
                width: level_w,
                height: level_h,
                depth_or_array_layers: 1,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("atlas sampler"),
        address_mode_u: wgpu::AddressMode::ClampToEdge,
        address_mode_v: wgpu::AddressMode::ClampToEdge,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: (mips.len() - 1) as f32,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// The terrain pipeline's tile texture ARRAY (one layer per tile, per-layer mips), with a
/// REPEAT sampler so a greedy-meshed quad can tile its layer across a wide/tall face without
/// the atlas cross-tile bleed. Parallel to [`create_atlas`]: the 2D atlas stays for the model
/// / break-overlay / particle / mob passes; only the block terrain pipeline binds this.
pub(super) fn create_atlas_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (levels, tile, layers) = crate::atlas::decode_atlas_array();
    let texture = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("atlas array"),
            size: wgpu::Extent3d {
                width: tile,
                height: tile,
                depth_or_array_layers: layers,
            },
            mip_level_count: levels.len() as u32,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8UnormSrgb,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
            view_formats: &[],
        },
    );
    for (level, data) in levels.iter().enumerate() {
        let tw = (tile >> level).max(1);
        queue.write_texture(
            wgpu::TexelCopyTextureInfo {
                texture: &texture,
                mip_level: level as u32,
                origin: wgpu::Origin3d::ZERO,
                aspect: wgpu::TextureAspect::All,
            },
            data,
            wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(tw * 4),
                rows_per_image: Some(tw),
            },
            wgpu::Extent3d {
                width: tw,
                height: tw,
                depth_or_array_layers: layers,
            },
        );
    }
    let view = texture.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("atlas array sampler"),
        address_mode_u: wgpu::AddressMode::Repeat,
        address_mode_v: wgpu::AddressMode::Repeat,
        address_mode_w: wgpu::AddressMode::ClampToEdge,
        mag_filter: wgpu::FilterMode::Nearest,
        min_filter: wgpu::FilterMode::Nearest,
        mipmap_filter: wgpu::FilterMode::Linear,
        lod_max_clamp: (levels.len() - 1) as f32,
        ..Default::default()
    });
    (texture, view, sampler)
}

/// The offscreen scene-colour target the world renders into before the grade
/// pass reads it back (same format as the swapchain, so every world pipeline
/// renders to it unchanged). Recreated with the depth texture on resize.
pub(super) fn create_scene_color(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    let tex = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("scene color"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = crate::render::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Allocation size for a layer holding `len` bytes: 25% headroom rounded up to
/// 1 KiB. The headroom absorbs remesh-to-remesh size jitter (so consecutive
/// uploads reuse the allocation) with bounded slack — the previous
/// `next_power_of_two()` rounding averaged ~40% wasted VRAM (up to 2×) across
/// every loaded column's up-to-8 buffers.
/// Fresh arena suballocations since process start. A sizing-policy change
/// trades VRAM against this number, so it is measurable rather than argued.
pub(super) static TERRAIN_SUBALLOCS: std::sync::atomic::AtomicU64 =
    std::sync::atomic::AtomicU64::new(0);

/// Growth headroom on a terrain layer buffer: a quarter of its size, but never
/// more than [`LAYER_HEADROOM_CAP`].
///
/// The headroom exists so a column that remeshes slightly larger writes into
/// the buffer it already has instead of reallocating on the render thread. That
/// argument is about the ABSOLUTE growth a remesh can produce, not a
/// proportion — and a proportional headroom on the big layers is what terrain
/// VRAM is mostly made of (measured RD32: opaque vertices are 61% of the
/// packed columns, and a flat 25% left 111 MiB of the 540 MiB unused).
/// One column layer's live geometry: where it sits in the arena and how many
/// bytes of it are in use (the allocation's class capacity is usually larger).
pub struct Layer {
    pub alloc: LayerAlloc,
    pub len: u64,
}

/// Upload `data` into the arena, REUSING `prev`'s suballocation when the data
/// still fits its size class.
///
/// Reuse is the point: a section re-meshes constantly while streaming (a
/// freshly loaded section re-lights its neighbours, each of which remeshes),
/// and re-suballocating for every one of those re-uploads churns the free
/// lists on the render thread. Writing into the allocation it already has
/// avoids that. The class rounding IS the growth headroom; an allocation is
/// released only when the data no longer fits it or has shrunk past a 4×
/// hysteresis, so a dug-out column returns its VRAM but size jitter never
/// churns.
fn upload_layer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    arena: &mut GeometryArena,
    prev: Option<Layer>,
    data: &[u8],
) -> Option<Layer> {
    let len = data.len() as u64;
    if data.is_empty() {
        return None;
    }
    if let Some(p) = prev {
        let cap = p.alloc.capacity();
        // Keep allocations that fit, unless they are now wildly oversized (the
        // player mined out most of the column / far LOD replaced dense
        // foliage).
        let oversized = cap > 16 * 1024 && cap / 4 > len;
        if cap >= len && !oversized {
            arena.write(queue, &p.alloc, 0, data);
            return Some(Layer {
                alloc: p.alloc,
                len,
            });
        }
    }
    TERRAIN_SUBALLOCS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let alloc = arena.alloc(device, len);
    arena.write(queue, &alloc, 0, data);
    Some(Layer { alloc, len })
}

fn append_indexed_layer<V: Copy>(
    verts: &mut Vec<V>,
    indices: &mut Vec<u32>,
    src_verts: &[V],
    src_indices: &[u32],
) -> (u32, u32, u32, u32) {
    let index_start = indices.len() as u32;
    let vertex_start = verts.len() as u32;
    verts.extend_from_slice(src_verts);
    if vertex_start == 0 {
        indices.extend_from_slice(src_indices);
    } else {
        indices.extend(src_indices.iter().map(|&i| i + vertex_start));
    }
    (
        index_start,
        src_indices.len() as u32,
        vertex_start,
        src_verts.len() as u32,
    )
}

/// Append an IMPLIED-triangulation layer (see [`crate::mesh::QuadIdx`]): only
/// the vertices travel, and the draw reads the shared quad index buffer with
/// the section's vertex start as `base_vertex`. Returns
/// `(vertex_start, vertex_count)`.
fn append_quad_layer(
    verts: &mut Vec<TerrainVertex>,
    src_verts: &[Vertex],
    col_ox: i32,
    col_oz: i32,
) -> (u32, u32) {
    let vertex_start = verts.len() as u32;
    verts.extend(
        src_verts
            .iter()
            .map(|v| TerrainVertex::from_world(v, col_ox, col_oz)),
    );
    (vertex_start, src_verts.len() as u32)
}

/// `(min_cy, max_cy)` over a column's installed sections, inverted when empty.
fn cy_span(sections: &[(SectionPos, GpuSectionMesh)]) -> (i32, i32) {
    sections
        .iter()
        .fold((i32::MAX, i32::MIN), |(lo, hi), (sp, _)| {
            (lo.min(sp.cy), hi.max(sp.cy))
        })
}

/// The shared instance-step table of per-column world XZ origins.
///
/// `vs_terrain` reconstructs absolute positions from a column-local vertex plus
/// this origin. It used to be a 16-byte GPU buffer PER COLUMN, which cost a
/// `set_vertex_buffer` on every single terrain draw — a quarter of the frame's
/// recorded commands at high render distance, and thousands of tiny buffer
/// objects for the driver and wgpu's submit-time resource tracker to carry.
/// Now every column indexes ONE array and the draw selects its row through
/// `first_instance`, so the bind happens once per pass.
pub struct ColumnOrigins {
    buf: wgpu::Buffer,
    /// CPU mirror, so growing the buffer is one write of everything live.
    values: Vec<[f32; 4]>,
    free: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
}

/// A column's row in [`ColumnOrigins`], returned to the free list on drop —
/// which is what keeps the table bounded across every path that can drop a
/// column (retain, remove, clear).
pub struct ColumnOriginSlot {
    index: u32,
    free: std::sync::Arc<std::sync::Mutex<Vec<u32>>>,
}

impl ColumnOriginSlot {
    #[inline]
    pub fn index(&self) -> u32 {
        self.index
    }
}

impl Drop for ColumnOriginSlot {
    fn drop(&mut self) {
        if let Ok(mut free) = self.free.lock() {
            free.push(self.index);
        }
    }
}

/// Rows the table starts with; it doubles from here.
const COLUMN_ORIGIN_INITIAL: u32 = 2048;

impl ColumnOrigins {
    pub(super) fn new(device: &wgpu::Device) -> Self {
        Self {
            buf: Self::create(device, COLUMN_ORIGIN_INITIAL),
            values: Vec::new(),
            free: std::sync::Arc::new(std::sync::Mutex::new(Vec::new())),
        }
    }

    fn create(device: &wgpu::Device, rows: u32) -> wgpu::Buffer {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("column origins"),
            size: u64::from(rows) * 16,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    }

    pub fn buffer(&self) -> &wgpu::Buffer {
        &self.buf
    }

    /// Claim (or refresh) the row holding `(col_ox, col_oz)`.
    fn slot(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        prev: Option<ColumnOriginSlot>,
        col_ox: i32,
        col_oz: i32,
    ) -> ColumnOriginSlot {
        let value = [col_ox as f32, 0.0, col_oz as f32, 0.0];
        let slot = match prev {
            Some(s) => s,
            None => {
                let reused = self.free.lock().ok().and_then(|mut f| f.pop());
                let index = match reused {
                    Some(i) => i,
                    None => {
                        let i = self.values.len() as u32;
                        self.values.push(value);
                        i
                    }
                };
                ColumnOriginSlot {
                    index,
                    free: std::sync::Arc::clone(&self.free),
                }
            }
        };
        let i = slot.index as usize;
        if i >= self.values.len() {
            self.values.resize(i + 1, [0.0; 4]);
        }
        self.values[i] = value;
        let rows = (self.buf.size() / 16) as u32;
        if slot.index >= rows {
            let mut grown = rows.max(1);
            while slot.index >= grown {
                grown *= 2;
            }
            self.buf = Self::create(device, grown);
            queue.write_buffer(&self.buf, 0, bytemuck::cast_slice(&self.values));
        } else {
            queue.write_buffer(
                &self.buf,
                u64::from(slot.index) * 16,
                bytemuck::bytes_of(&value),
            );
        }
        slot
    }
}

fn patch_terrain_verts(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    buf: &Option<Layer>,
    vertex_start: u32,
    src: &[Vertex],
    col_ox: i32,
    col_oz: i32,
) -> bool {
    if src.is_empty() {
        return true;
    }
    let quantized: Vec<TerrainVertex> = src
        .iter()
        .map(|v| TerrainVertex::from_world(v, col_ox, col_oz))
        .collect();
    patch_verts(queue, arena, buf, vertex_start, &quantized)
}

/// FNV-1a over a section's SECTION-LOCAL index streams, all indexed layers in a
/// fixed order. With per-layer counts already matched, equal hashes mean the
/// column-buffer indices retained on the GPU (section-local + a vertex-start
/// offset that count equality pins) are still valid for the new vertex data.
fn section_index_hash(mesh: &ChunkMesh) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |stream: &[u32]| {
        for &i in stream {
            h ^= i as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
        // Layer separator: streams of different layers must not concatenate
        // into the same digest position.
        h ^= 0xff;
        h = h.wrapping_mul(0x0000_0100_0000_01b3);
    };
    eat(&mesh.model_idx);
    h
}

fn layer_sizes_match(mesh: &ChunkMesh, gpu: &GpuSectionMesh) -> bool {
    mesh.opaque.len() as u32 == gpu.opaque_vertex_count
        && mesh.far_opaque.len() as u32 == gpu.far_opaque_vertex_count
        && mesh.transparent.len() as u32 == gpu.transparent_vertex_count
        && mesh.transparent_two_sided.len() as u32 == gpu.transparent_ts_vertex_count
        && mesh.translucent.len() as u32 == gpu.translucent_vertex_count
        && mesh.model.len() as u32 == gpu.model_vertex_count
        && mesh.model_idx.len() as u32 == gpu.model_idx_count
        && mesh.contact.len() as u32 == gpu.contact_vertex_count
}

fn patch_verts<V: bytemuck::Pod>(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    buf: &Option<Layer>,
    vertex_start: u32,
    src: &[V],
) -> bool {
    if src.is_empty() {
        return true;
    }
    let Some(buf) = buf else {
        return false;
    };
    let offset = vertex_start as u64 * std::mem::size_of::<V>() as u64;
    let bytes = bytemuck::cast_slice(src);
    arena.write(queue, &buf.alloc, offset, bytes)
}

/// When every section keeps the same vertex/index counts as the installed GPU
/// column, rewrite only vertex attributes in place (light/AO remeshes). Indices
/// and sibling CPU packing are skipped entirely.
fn try_patch_column_verts(
    queue: &wgpu::Queue,
    arena: &GeometryArena,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: &GpuColumnMesh,
) -> bool {
    if meshes.len() != prev.sections.len() {
        return false;
    }
    for (&(sp, mesh), &(psp, ref gpu)) in meshes.iter().zip(&prev.sections) {
        if sp != psp || !layer_sizes_match(mesh, gpu) || section_index_hash(mesh) != gpu.index_hash
        {
            return false;
        }
    }
    let (ox, oz) = (prev.col_ox, prev.col_oz);
    for (&(_, mesh), (_, gpu)) in meshes.iter().zip(&prev.sections) {
        if !patch_terrain_verts(
            queue,
            arena,
            &prev.opaque_vbuf,
            gpu.opaque_vertex_start,
            &mesh.opaque,
            ox,
            oz,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.far_opaque_vbuf,
            gpu.far_opaque_vertex_start,
            &mesh.far_opaque,
            ox,
            oz,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.transparent_vbuf,
            gpu.transparent_vertex_start,
            &mesh.transparent,
            ox,
            oz,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.transparent_ts_vbuf,
            gpu.transparent_ts_vertex_start,
            &mesh.transparent_two_sided,
            ox,
            oz,
        ) || !patch_terrain_verts(
            queue,
            arena,
            &prev.translucent_vbuf,
            gpu.translucent_vertex_start,
            &mesh.translucent,
            ox,
            oz,
        ) || !patch_verts(
            queue,
            arena,
            &prev.model_vbuf,
            gpu.model_vertex_start,
            &mesh.model,
        ) || !patch_verts(
            queue,
            arena,
            &prev.contact_vbuf,
            gpu.contact_vertex_start,
            &mesh.contact,
        ) {
            return false;
        }
    }
    true
}

pub(super) fn upload_column_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: Option<GpuColumnMesh>,
    scratch: &mut ColumnUploadScratch,
    origins: &mut ColumnOrigins,
    arena: &mut GeometryArena,
    quad_index: &mut QuadIndexBuffer,
) -> GpuColumnMesh {
    let col_ox = meshes.first().map(|(sp, _)| sp.cx * 16).unwrap_or(0);
    let col_oz = meshes.first().map(|(sp, _)| sp.cz * 16).unwrap_or(0);

    let (p_ov, p_fov, p_tv, p_ti, p_lv, p_mv, p_mi, p_cv, p_origin, mut sections) = match prev {
        Some(g) if try_patch_column_verts(queue, arena, meshes, &g) => {
            // Layout unchanged: reuse the GPU column (buffers + section ranges).
            return g;
        }
        Some(g) => (
            g.opaque_vbuf,
            g.far_opaque_vbuf,
            g.transparent_vbuf,
            g.transparent_ts_vbuf,
            g.translucent_vbuf,
            g.model_vbuf,
            g.model_ibuf,
            g.contact_vbuf,
            Some(g.origin_slot),
            g.sections,
        ),
        None => (
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Vec::new(),
        ),
    };

    scratch.clear();
    scratch.reserve_for(meshes);
    sections.clear();
    sections.reserve(meshes.len());

    for &(sp, mesh) in meshes {
        let (opaque_vertex_start, opaque_vertex_count) =
            append_quad_layer(&mut scratch.opaque, &mesh.opaque, col_ox, col_oz);
        let (far_opaque_vertex_start, far_opaque_vertex_count) =
            append_quad_layer(&mut scratch.far_opaque, &mesh.far_opaque, col_ox, col_oz);
        let (transparent_vertex_start, transparent_vertex_count) =
            append_quad_layer(&mut scratch.transparent, &mesh.transparent, col_ox, col_oz);
        let (transparent_ts_vertex_start, transparent_ts_vertex_count) = append_quad_layer(
            &mut scratch.transparent_two_sided,
            &mesh.transparent_two_sided,
            col_ox,
            col_oz,
        );
        let (translucent_vertex_start, translucent_vertex_count) =
            append_quad_layer(&mut scratch.translucent, &mesh.translucent, col_ox, col_oz);
        let (model_index_start, model_idx_count, model_vertex_start, model_vertex_count) =
            append_indexed_layer(
                &mut scratch.model,
                &mut scratch.model_idx,
                &mesh.model,
                &mesh.model_idx,
            );
        let contact_vertex_start = scratch.contact.len() as u32;
        let contact_vertex_count = mesh.contact.len() as u32;
        scratch.contact.extend_from_slice(&mesh.contact);
        sections.push((
            sp,
            GpuSectionMesh {
                origin: (sp.cx * 16, sp.cy * 16, sp.cz * 16),
                opaque_vertex_start,
                opaque_vertex_count,
                far_opaque_vertex_start,
                far_opaque_vertex_count,
                transparent_vertex_start,
                transparent_vertex_count,
                transparent_ts_vertex_start,
                transparent_ts_vertex_count,
                translucent_vertex_start,
                translucent_vertex_count,
                model_index_start,
                model_idx_count,
                model_vertex_start,
                model_vertex_count,
                contact_vertex_start,
                contact_vertex_count,
                index_hash: section_index_hash(mesh),
            },
        ));
    }

    // The largest implied-triangulation draw this column can submit: the whole
    // column's opaque stream, or one section's far-LOD stream.
    quad_index.ensure(
        device,
        queue,
        (scratch
            .opaque
            .len()
            .max(scratch.far_opaque.len())
            .max(scratch.transparent.len())
            .max(scratch.transparent_two_sided.len())
            .max(scratch.translucent.len())
            / 4) as u32,
    );

    GpuColumnMesh {
        opaque_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_ov,
            bytemuck::cast_slice(&scratch.opaque),
        ),
        opaque_quads: (scratch.opaque.len() / 4) as u32,
        far_opaque_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_fov,
            bytemuck::cast_slice(&scratch.far_opaque),
        ),
        transparent_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_tv,
            bytemuck::cast_slice(&scratch.transparent),
        ),
        transparent_ts_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_ti,
            bytemuck::cast_slice(&scratch.transparent_two_sided),
        ),
        translucent_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_lv,
            bytemuck::cast_slice(&scratch.translucent),
        ),
        model_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_mv,
            bytemuck::cast_slice(&scratch.model),
        ),
        model_ibuf: upload_layer(
            device,
            queue,
            arena,
            p_mi,
            bytemuck::cast_slice(&scratch.model_idx),
        ),
        model_idx_count: scratch.model_idx.len() as u32,
        contact_vbuf: upload_layer(
            device,
            queue,
            arena,
            p_cv,
            bytemuck::cast_slice(&scratch.contact),
        ),
        contact_vertex_count: scratch.contact.len() as u32,
        origin_slot: origins.slot(device, queue, p_origin, col_ox, col_oz),
        col_ox,
        col_oz,
        cy_span: cy_span(&sections),
        sections,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The vertex-only patch path must refuse a mesh whose model index TOPOLOGY
    /// changed even when every layer count matches — count equality alone let
    /// stale GPU indices rewire a rebaked model's triangles.
    #[test]
    fn index_hash_distinguishes_equal_count_topologies() {
        let mut a = crate::mesh::ChunkMesh::empty();
        let mut b = crate::mesh::ChunkMesh::empty();
        a.model_idx = vec![0, 1, 2, 0, 2, 3, 4, 5, 6, 4, 6, 7];
        b.model_idx = vec![4, 5, 6, 4, 6, 7, 0, 1, 2, 0, 2, 3];
        assert_ne!(section_index_hash(&a), section_index_hash(&b));
        assert_eq!(section_index_hash(&a), section_index_hash(&a));
    }
}
