//! GPU texture uploads: atlases, model sheets, GUI panels, the scene colour
//! and depth targets.

use crate::atlas::decode_atlas_mips;
use petramond_util::texture_mips::build_cutout_mips;

/// Upload a standalone GUI PNG (e.g. the HUD heart atlas) as its own
/// texture + nearest sampler (sRGB, like the gui atlas). Arbitrary size —
/// each PNG is its own image, not a fixed atlas slot.
pub(crate) fn create_gui_panel(
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
pub(crate) fn create_sky_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    bytes: &[u8],
) -> Option<(wgpu::Texture, wgpu::TextureView, wgpu::Sampler)> {
    let img = image::load_from_memory(bytes).ok()?.to_rgba8();
    Some(create_rgba_nearest(device, queue, &img, "sky texture"))
}

/// Upload a single fallback pixel for fixed bind slots whose pack texture is
/// absent or invalid.
pub(crate) fn create_solid_rgba_texture(
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
pub(crate) fn create_rgba_nearest(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    img: &image::RgbaImage,
    label: &str,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (w, h) = (img.width(), img.height());
    let texture = crate::gpu_mem::create_texture(
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
/// faces carry arbitrary sub-rectangle UVs into this sheet (see `petramond_world::bbmodel`).
/// Mips use cutout-alpha expansion so thin transparent decals, like the workbench's
/// tabletop grid, stay stable at distance under the shader's alpha test.
///
/// `w`/`h` of 0 are clamped to 1 so a missing/empty texture still yields a valid 1×1
/// binding.
pub(crate) fn create_model_texture(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    rgba: &[u8],
    w: u32,
    h: u32,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let w = w.max(1);
    let h = h.max(1);
    let mips = build_cutout_mips(rgba, w, h);
    let texture = crate::gpu_mem::create_texture(
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

/// The offscreen scene-colour target the world renders into before the grade
/// pass reads it back (same format as the swapchain, so every world pipeline
/// renders to it unchanged). Recreated with the depth texture on resize.
pub(crate) fn create_scene_color(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
) -> wgpu::TextureView {
    create_scene_color_sampled(device, w, h, format, 1)
}

pub(crate) fn create_scene_color_sampled(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    format: wgpu::TextureFormat,
    samples: u32,
) -> wgpu::TextureView {
    let tex = crate::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("scene color"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT
                | if samples == 1 {
                    wgpu::TextureUsages::TEXTURE_BINDING
                } else {
                    wgpu::TextureUsages::empty()
                },
            view_formats: &[],
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_depth(device: &wgpu::Device, w: u32, h: u32) -> wgpu::TextureView {
    create_depth_sampled(device, w, h, 1)
}

pub(crate) fn create_depth_sampled(
    device: &wgpu::Device,
    w: u32,
    h: u32,
    samples: u32,
) -> wgpu::TextureView {
    let tex = crate::gpu_mem::create_texture(
        device,
        &wgpu::TextureDescriptor {
            label: Some("depth"),
            size: wgpu::Extent3d {
                width: w,
                height: h,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: samples,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Depth32Float,
            usage: wgpu::TextureUsages::RENDER_ATTACHMENT | wgpu::TextureUsages::TEXTURE_BINDING,
            view_formats: &[],
        },
    );
    tex.create_view(&wgpu::TextureViewDescriptor::default())
}

pub(crate) fn create_atlas(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (mips, w, h) = decode_atlas_mips();
    let texture = crate::gpu_mem::create_texture(
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
pub(crate) fn create_atlas_array(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
) -> (wgpu::Texture, wgpu::TextureView, wgpu::Sampler) {
    let (levels, tile, layers) = crate::atlas::decode_atlas_array();
    let texture = crate::gpu_mem::create_texture(
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
