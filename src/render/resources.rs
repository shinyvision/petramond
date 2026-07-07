use crate::atlas::decode_atlas_mips;
use crate::chunk::SectionPos;
use crate::mesh::{ChunkMesh, ModelVertex, Vertex};
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
    let texture = device.create_texture(&wgpu::TextureDescriptor {
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
    });
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
    let texture = device.create_texture(&wgpu::TextureDescriptor {
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
    });
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

pub struct GpuSectionMesh {
    /// World-space minimum corner `(x, y, z)` of this section.
    pub origin: (i32, i32, i32),
    pub opaque_index_start: u32,
    pub opaque_idx_count: u32,
    pub far_opaque_index_start: u32,
    pub far_opaque_idx_count: u32,
    pub transparent_index_start: u32,
    pub transparent_idx_count: u32,
    pub model_index_start: u32,
    pub model_idx_count: u32,
}

pub struct GpuColumnMesh {
    pub opaque_vbuf: Option<wgpu::Buffer>,
    pub opaque_ibuf: Option<wgpu::Buffer>,
    pub opaque_idx_count: u32,
    pub far_opaque_vbuf: Option<wgpu::Buffer>,
    pub far_opaque_ibuf: Option<wgpu::Buffer>,
    pub transparent_vbuf: Option<wgpu::Buffer>,
    pub transparent_ibuf: Option<wgpu::Buffer>,
    pub model_vbuf: Option<wgpu::Buffer>,
    pub model_ibuf: Option<wgpu::Buffer>,
    pub model_idx_count: u32,
    pub sections: Vec<(SectionPos, GpuSectionMesh)>,
}

#[derive(Default)]
pub(super) struct ColumnUploadScratch {
    opaque: Vec<Vertex>,
    opaque_idx: Vec<u32>,
    far_opaque: Vec<Vertex>,
    far_opaque_idx: Vec<u32>,
    transparent: Vec<Vertex>,
    transparent_idx: Vec<u32>,
    model: Vec<ModelVertex>,
    model_idx: Vec<u32>,
}

impl ColumnUploadScratch {
    fn clear(&mut self) {
        self.opaque.clear();
        self.opaque_idx.clear();
        self.far_opaque.clear();
        self.far_opaque_idx.clear();
        self.transparent.clear();
        self.transparent_idx.clear();
        self.model.clear();
        self.model_idx.clear();
    }

    fn reserve_for(&mut self, meshes: &[(SectionPos, &ChunkMesh)]) {
        self.opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.opaque.len()).sum());
        self.opaque_idx
            .reserve(meshes.iter().map(|(_, mesh)| mesh.opaque_idx.len()).sum());
        self.far_opaque
            .reserve(meshes.iter().map(|(_, mesh)| mesh.far_opaque.len()).sum());
        self.far_opaque_idx.reserve(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.far_opaque_idx.len())
                .sum(),
        );
        self.transparent
            .reserve(meshes.iter().map(|(_, mesh)| mesh.transparent.len()).sum());
        self.transparent_idx.reserve(
            meshes
                .iter()
                .map(|(_, mesh)| mesh.transparent_idx.len())
                .sum(),
        );
        self.model
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model.len()).sum());
        self.model_idx
            .reserve(meshes.iter().map(|(_, mesh)| mesh.model_idx.len()).sum());
    }
}

pub(super) fn create_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (mips, w, h) = decode_atlas_mips();
    let texture = device.create_texture(&wgpu::TextureDescriptor {
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
    });
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
    let texture = device.create_texture(&wgpu::TextureDescriptor {
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
    });
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
    let tex = device.create_texture(&wgpu::TextureDescriptor {
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
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(super) fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    let tex = device.create_texture(&wgpu::TextureDescriptor {
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
    });
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

/// Upload `data` into `prev`, REUSING its GPU allocation when it is large enough
/// (`queue.write_buffer`), otherwise (re)allocating a rounded-up buffer. Empty data drops
/// `prev` (frees it) and returns `None`.
///
/// Reuse is the point: a section re-meshes constantly while streaming (a freshly loaded
/// section re-lights its neighbours, each of which remeshes), and allocating fresh GPU
/// buffers for every one of those re-uploads — then freeing the old ones — churns the
/// driver allocator on the render thread and stalls the frame. `write_buffer` into an
/// existing, big-enough buffer avoids the allocation entirely; buffers only ever grow.
fn upload_layer(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    prev: Option<wgpu::Buffer>,
    data: &[u8],
    usage: wgpu::BufferUsages,
) -> Option<wgpu::Buffer> {
    if data.is_empty() {
        return None;
    }
    if let Some(buf) = prev {
        if buf.size() as usize >= data.len() {
            queue.write_buffer(&buf, 0, data);
            return Some(buf);
        }
    }
    // Round the capacity up so the next slightly-larger remesh reuses this allocation
    // rather than churning a new one. `COPY_DST` lets later frames `write_buffer` into it.
    let cap = (data.len() as u64).next_power_of_two().max(1024);
    let buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: None,
        size: cap,
        usage: usage | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    queue.write_buffer(&buf, 0, data);
    Some(buf)
}

fn append_indexed_layer<V: Copy>(
    verts: &mut Vec<V>,
    indices: &mut Vec<u32>,
    src_verts: &[V],
    src_indices: &[u32],
) -> (u32, u32) {
    let start = indices.len() as u32;
    let base = verts.len() as u32;
    verts.extend_from_slice(src_verts);
    indices.extend(src_indices.iter().map(|&i| i + base));
    (start, src_indices.len() as u32)
}

pub(super) fn upload_column_mesh(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    meshes: &[(SectionPos, &ChunkMesh)],
    prev: Option<GpuColumnMesh>,
    scratch: &mut ColumnUploadScratch,
) -> GpuColumnMesh {
    let (p_ov, p_oi, p_fov, p_foi, p_tv, p_ti, p_mv, p_mi, mut sections) = match prev {
        Some(g) => (
            g.opaque_vbuf,
            g.opaque_ibuf,
            g.far_opaque_vbuf,
            g.far_opaque_ibuf,
            g.transparent_vbuf,
            g.transparent_ibuf,
            g.model_vbuf,
            g.model_ibuf,
            g.sections,
        ),
        None => (None, None, None, None, None, None, None, None, Vec::new()),
    };

    scratch.clear();
    scratch.reserve_for(meshes);
    sections.clear();
    sections.reserve(meshes.len());

    for &(sp, mesh) in meshes {
        let (opaque_index_start, opaque_idx_count) = append_indexed_layer(
            &mut scratch.opaque,
            &mut scratch.opaque_idx,
            &mesh.opaque,
            &mesh.opaque_idx,
        );
        let (far_opaque_index_start, far_opaque_idx_count) = append_indexed_layer(
            &mut scratch.far_opaque,
            &mut scratch.far_opaque_idx,
            &mesh.far_opaque,
            &mesh.far_opaque_idx,
        );
        let (transparent_index_start, transparent_idx_count) = append_indexed_layer(
            &mut scratch.transparent,
            &mut scratch.transparent_idx,
            &mesh.transparent,
            &mesh.transparent_idx,
        );
        let (model_index_start, model_idx_count) = append_indexed_layer(
            &mut scratch.model,
            &mut scratch.model_idx,
            &mesh.model,
            &mesh.model_idx,
        );
        sections.push((
            sp,
            GpuSectionMesh {
                origin: (sp.cx * 16, sp.cy * 16, sp.cz * 16),
                opaque_index_start,
                opaque_idx_count,
                far_opaque_index_start,
                far_opaque_idx_count,
                transparent_index_start,
                transparent_idx_count,
                model_index_start,
                model_idx_count,
            },
        ));
    }

    let vtx = wgpu::BufferUsages::VERTEX;
    let idx = wgpu::BufferUsages::INDEX;
    GpuColumnMesh {
        opaque_vbuf: upload_layer(
            device,
            queue,
            p_ov,
            bytemuck::cast_slice(&scratch.opaque),
            vtx,
        ),
        opaque_ibuf: upload_layer(
            device,
            queue,
            p_oi,
            bytemuck::cast_slice(&scratch.opaque_idx),
            idx,
        ),
        opaque_idx_count: scratch.opaque_idx.len() as u32,
        far_opaque_vbuf: upload_layer(
            device,
            queue,
            p_fov,
            bytemuck::cast_slice(&scratch.far_opaque),
            vtx,
        ),
        far_opaque_ibuf: upload_layer(
            device,
            queue,
            p_foi,
            bytemuck::cast_slice(&scratch.far_opaque_idx),
            idx,
        ),
        transparent_vbuf: upload_layer(
            device,
            queue,
            p_tv,
            bytemuck::cast_slice(&scratch.transparent),
            vtx,
        ),
        transparent_ibuf: upload_layer(
            device,
            queue,
            p_ti,
            bytemuck::cast_slice(&scratch.transparent_idx),
            idx,
        ),
        model_vbuf: upload_layer(
            device,
            queue,
            p_mv,
            bytemuck::cast_slice(&scratch.model),
            vtx,
        ),
        model_ibuf: upload_layer(
            device,
            queue,
            p_mi,
            bytemuck::cast_slice(&scratch.model_idx),
            idx,
        ),
        model_idx_count: scratch.model_idx.len() as u32,
        sections,
    }
}
