use crate::atlas::decode_atlas_mips;
use crate::chunk::{ChunkPos, SECTION_COUNT};
use crate::mesh::{ChunkMesh, MeshIndexSection};
use crate::texture_mips::build_cutout_mips;

use wgpu::util::DeviceExt;

/// Upload a baked data-driven GUI panel PNG (from the `gui-builder`) as its own
/// texture + nearest sampler (sRGB, like the gui atlas). Arbitrary size — each
/// baked panel is its own image, not a fixed atlas slot. See `crate::gui`.
pub(super) fn create_gui_panel(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    png: &[u8],
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let img = image::load_from_memory(png)
        .expect("decode gui panel png")
        .to_rgba8();
    let (w, h) = (img.width(), img.height());
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("gui panel"),
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
        label: Some("gui panel sampler"),
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

pub struct GpuMesh {
    pub opaque_vbuf: Option<wgpu::Buffer>,
    pub opaque_ibuf: Option<wgpu::Buffer>,
    pub opaque_idx_count: u32,
    pub opaque_sections: [MeshIndexSection; SECTION_COUNT],
    pub far_opaque_vbuf: Option<wgpu::Buffer>,
    pub far_opaque_ibuf: Option<wgpu::Buffer>,
    pub far_opaque_idx_count: u32,
    pub far_opaque_sections: [MeshIndexSection; SECTION_COUNT],
    pub transparent_vbuf: Option<wgpu::Buffer>,
    pub transparent_ibuf: Option<wgpu::Buffer>,
    pub transparent_idx_count: u32,
    pub transparent_sections: [MeshIndexSection; SECTION_COUNT],
    /// bbmodel-block geometry (explicit-UV [`ModelVertex`], sampling the model atlas),
    /// drawn in the model pass. `None`/`0` for the common chunk.
    pub model_vbuf: Option<wgpu::Buffer>,
    pub model_ibuf: Option<wgpu::Buffer>,
    pub model_idx_count: u32,
    pub pos: ChunkPos,
    pub origin: (i32, i32),
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

pub(super) fn upload_mesh(device: &wgpu::Device, mesh: &ChunkMesh, pos: ChunkPos) -> GpuMesh {
    let opaque_vbuf = if mesh.opaque.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.opaque),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let opaque_ibuf = if mesh.opaque_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.opaque_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let far_opaque_vbuf = if mesh.far_opaque.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.far_opaque),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let far_opaque_ibuf = if mesh.far_opaque_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.far_opaque_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let transparent_vbuf = if mesh.transparent.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.transparent),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let transparent_ibuf = if mesh.transparent_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.transparent_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    let model_vbuf = if mesh.model.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.model),
                usage: wgpu::BufferUsages::VERTEX,
            }),
        )
    };
    let model_ibuf = if mesh.model_idx.is_empty() {
        None
    } else {
        Some(
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: None,
                contents: bytemuck::cast_slice(&mesh.model_idx),
                usage: wgpu::BufferUsages::INDEX,
            }),
        )
    };
    GpuMesh {
        opaque_vbuf,
        opaque_ibuf,
        opaque_idx_count: mesh.opaque_idx.len() as u32,
        opaque_sections: mesh.opaque_sections,
        far_opaque_vbuf,
        far_opaque_ibuf,
        far_opaque_idx_count: mesh.far_opaque_idx.len() as u32,
        far_opaque_sections: mesh.far_opaque_sections,
        transparent_vbuf,
        transparent_ibuf,
        transparent_idx_count: mesh.transparent_idx.len() as u32,
        transparent_sections: mesh.transparent_sections,
        model_vbuf,
        model_ibuf,
        model_idx_count: mesh.model_idx.len() as u32,
        pos,
        origin: (pos.cx * 16, pos.cz * 16),
    }
}
