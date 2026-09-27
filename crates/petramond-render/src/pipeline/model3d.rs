use std::num::NonZeroU64;

use super::builders::{
    color_target, cull_back, pipeline_layout, shader_module, single_pipeline, uniform_entry,
    world_pipeline, DepthPreset,
};
use crate::renderer::dynamic_draw::new_buffer;
use crate::uniforms::Uniforms;

pub(super) const MODEL3D_MVP_SLOT_SIZE: u64 = 256;
pub(super) const MODEL3D_MVP_SLOTS: u64 = 64;
fn mvp_slot_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::VERTEX,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: true,
            min_binding_size: NonZeroU64::new(64),
        },
        count: None,
    }
}

fn mvp_slot_binding(buf: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer: buf,
        offset: 0,
        size: NonZeroU64::new(64),
    })
}

pub(super) struct Model3dResources {
    pub(super) pipe: wgpu::RenderPipeline,
    pub(super) hand_pipe: crate::pipeline::SampledPipeline,
    pub(super) mvp_buf: wgpu::Buffer,
    pub(super) mvp_bind: wgpu::BindGroup,
    pub(super) mvp_bgl: wgpu::BindGroupLayout,
    pub(super) vbuf: wgpu::Buffer,
    pub(super) ibuf: wgpu::Buffer,
}

/// model3d pipeline for isometric slot icons and the first-person held block.
///
/// group(0) holds a per-draw MVP mat4 in a dynamic-offset uniform (binding 0) and the shared
/// `uv_rects` table (binding 1, as in the block pipeline). group(1) is the block atlas.
/// Full-bright, back-face culled, and alpha-blended so flat sprite items cut out.
///
/// Built in two depth variants from the same shader and layout. `model3d_pipe` has no depth, for
/// the UI icon pass. `model3d_hand_pipe` tests and writes depth in the hand pass, which has a
/// cleared depth buffer, so the held block self-sorts.
pub(super) fn create_model3d_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    uniform_buf: &wgpu::Buffer,
    uv_rects_buf: &wgpu::Buffer,
    atlas_bgl: &wgpu::BindGroupLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> Model3dResources {
    let model3d_shader = shader_module(
        device,
        "model3d shader",
        include_str!("../../shaders/model3d.wgsl"),
    );
    let model3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model3d mvp bgl"),
        entries: &[
            mvp_slot_entry(0),
            crate::uniforms::uv_rects_entry(1),
            uniform_entry(
                2,
                wgpu::ShaderStages::VERTEX,
                std::mem::size_of::<Uniforms>() as u64,
            ),
        ],
    });
    let model3d_mvp_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d mvp"),
        size: MODEL3D_MVP_SLOTS * MODEL3D_MVP_SLOT_SIZE,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let model3d_mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model3d mvp bg"),
        layout: &model3d_mvp_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: mvp_slot_binding(&model3d_mvp_buf),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: uv_rects_buf.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: uniform_buf.as_entire_binding(),
            },
        ],
    });
    let model3d_layout = pipeline_layout(device, "model3d layout", &[&model3d_mvp_bgl, atlas_bgl]);
    let model3d_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // The two model3d pipelines share shader/layout/blend/cull and differ ONLY in
    // the depth attachment: `model3d_pipe` is depthless (the iso icons draw in the
    // depthless UI pass); `model3d_hand_pipe` adds depth Less + write so the
    // first-person held block self-sorts against the hand pass's cleared depth
    // buffer (a single pipeline cannot serve both passes).
    let model3d_pipe = single_pipeline(
        device,
        "model3d pipe",
        &model3d_layout,
        &model3d_shader,
        "vs_model",
        "fs_model",
        std::slice::from_ref(vbuf_layout),
        &model3d_targets,
        cull_back(),
        None,
    );
    let model3d_hand_pipe = world_pipeline(
        device,
        "model3d hand pipe",
        &model3d_layout,
        &model3d_shader,
        "vs_model",
        "fs_model",
        std::slice::from_ref(vbuf_layout),
        &model3d_targets,
        cull_back(),
        Some(DepthPreset::WriteLess),
        max_samples,
    );
    let model3d_vbuf = new_buffer(device, wgpu::BufferUsages::VERTEX, "model3d vbuf");
    let model3d_ibuf = new_buffer(device, wgpu::BufferUsages::INDEX, "model3d ibuf");

    Model3dResources {
        pipe: model3d_pipe,
        hand_pipe: model3d_hand_pipe,
        mvp_buf: model3d_mvp_buf,
        mvp_bind: model3d_mvp_bind,
        mvp_bgl: model3d_mvp_bgl,
        vbuf: model3d_vbuf,
        ibuf: model3d_ibuf,
    }
}

/// item3d pipeline (extruded first-person held item).
/// group(0) = a per-draw MVP via a DYNAMIC-OFFSET uniform (binding 0) over the
/// shared `model3d_mvp_buf` (reuses its 256-byte-slot pattern). group(1) = the
/// block atlas (reuse the atlas bgl). Explicit per-vertex (pos, uv, shade) so
/// the side walls can sample a single boundary texel's sub-UV (the model3d
/// packed-vertex shader can only SELECT whole-tile UV corners). Full-bright,
/// alpha-cutout, DOUBLE-SIDED (cull off so the back face + inner walls show),
/// NO depth (drawn over the world in the hand pass), alpha-blended.
pub(super) fn create_item3d_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    atlas_bgl: &wgpu::BindGroupLayout,
    model3d_mvp_buf: &wgpu::Buffer,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (
    crate::pipeline::SampledPipeline,
    wgpu::BindGroup,
    wgpu::Buffer,
) {
    let item3d_shader = shader_module(
        device,
        "item3d shader",
        include_str!("../../shaders/item3d.wgsl"),
    );
    let item3d_mvp_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("item3d mvp bgl"),
        entries: &[mvp_slot_entry(0)],
    });
    let item3d_mvp_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("item3d mvp bg"),
        layout: &item3d_mvp_bgl,
        entries: &[wgpu::BindGroupEntry {
            binding: 0,
            resource: mvp_slot_binding(model3d_mvp_buf),
        }],
    });
    let item3d_layout = pipeline_layout(device, "item3d layout", &[&item3d_mvp_bgl, atlas_bgl]);
    let item3d_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // Cull None: back face and inward walls must never cull.
    // Depth test + write against hand pass's own cleared depth buffer, so front,
    // stepped side walls, and back self-sort instead of overdrawing by submission
    // order. Hand pass clears depth so this stays isolated from world depth (item
    // still draws over terrain).
    let item3d_pipe = world_pipeline(
        device,
        "item3d pipe",
        &item3d_layout,
        &item3d_shader,
        "vs_item",
        "fs_item",
        std::slice::from_ref(item3d_vbuf_layout),
        &item3d_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        max_samples,
    );
    let item3d_vbuf = new_buffer(device, wgpu::BufferUsages::VERTEX, "item3d vbuf");
    (item3d_pipe, item3d_mvp_bind, item3d_vbuf)
}
