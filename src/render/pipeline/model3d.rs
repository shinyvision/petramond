use std::num::NonZeroU64;

use super::builders::{
    color_target, cull_back, pipeline_layout, shader_module, uniform_entry, world_pipeline,
    DepthPreset,
};
use crate::mesh::Vertex;
use crate::render::uniforms::{Uniforms, UV_RECTS_LEN};

/// Size of one MVP slot in the model3d dynamic-offset uniform buffer. A `mat4`
/// is 64 bytes but dynamic offsets must be a multiple of the device's
/// `min_uniform_buffer_offset_alignment` (256 on WebGL2 / the WebGPU minimum),
/// so each per-draw MVP occupies a 256-byte aligned slot.
pub(super) const MODEL3D_MVP_SLOT_SIZE: u64 = 256;
/// Number of 256-byte MVP slots in the model3d uniform buffer. The hand uses
/// slot 0; the isometric inventory icons (Layer 4 UI) cycle through the rest, so
/// 64 slots covers the open inventory's 36 visible cube icons with headroom.
pub(super) const MODEL3D_MVP_SLOTS: u64 = 64;
/// Max vertices in the reusable model3d dynamic vertex buffer. A textured cube is
/// 24 verts; this covers the hand plus a batch of icon cubes drawn in one buffer.
pub(super) const MAX_MODEL3D_VERTICES: u64 = 4096;
/// Max indices in the reusable model3d dynamic index buffer (36 per cube).
pub(super) const MAX_MODEL3D_INDICES: u64 = 8192;
/// Max vertices in the reusable item3d dynamic vertex buffer (the extruded held
/// item). A 16×16 sprite extrudes to front+back + boundary walls; a dense flower
/// silhouette is well under this (non-indexed triangle list, 6 verts/quad).
pub(in crate::render) const MAX_ITEM3D_VERTICES: u64 = 4096;

/// The dynamic-offset per-draw MVP uniform entry: one mat4 (64 bytes) per
/// draw, selected by a 256-aligned dynamic offset (see
/// [`MODEL3D_MVP_SLOT_SIZE`]).
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

/// The matching MVP resource: a 64-byte mat4 window over the slot buffer; the
/// per-draw dynamic offset selects the slot.
fn mvp_slot_binding(buf: &wgpu::Buffer) -> wgpu::BindingResource<'_> {
    wgpu::BindingResource::Buffer(wgpu::BufferBinding {
        buffer: buf,
        offset: 0,
        size: NonZeroU64::new(64),
    })
}

/// The values the model3d pass hands back to [`PipelineResources`].
pub(super) struct Model3dResources {
    pub(super) pipe: wgpu::RenderPipeline,
    pub(super) hand_pipe: wgpu::RenderPipeline,
    pub(super) mvp_buf: wgpu::Buffer,
    pub(super) mvp_bind: wgpu::BindGroup,
    pub(super) mvp_bgl: wgpu::BindGroupLayout,
    pub(super) vbuf: wgpu::Buffer,
    pub(super) ibuf: wgpu::Buffer,
}

/// model3d pipeline (isometric slot icons + first-person held block).
/// group(0): a per-draw MVP mat4 via a DYNAMIC-OFFSET uniform (binding 0) plus
/// the shared uv_rects table (binding 1, same as the block pipeline). group(1):
/// the block atlas (reuse the atlas bgl shape). Full-bright, back-face culled,
/// alpha-blended so flat sprite items cut out. Built in TWO depth variants from
/// the SAME shader/layout: `model3d_pipe` (NO depth) for the depthless UI icon
/// pass, and `model3d_hand_pipe` (depth test + write) for the hand pass, which
/// now carries a cleared depth buffer so the held block self-sorts.
pub(super) fn create_model3d_pipelines(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
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
            uniform_entry(1, wgpu::ShaderStages::VERTEX, (UV_RECTS_LEN * 16) as u64),
            // The frame `Uniforms` buffer: model3d reads only fog_color.w (the
            // sim's sky scale) so the held block dims in step with terrain.
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
    let model3d_pipe = world_pipeline(
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
        sample_count,
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
        sample_count,
    );
    let model3d_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d vbuf"),
        size: MAX_MODEL3D_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let model3d_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("model3d ibuf"),
        size: MAX_MODEL3D_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

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
    sample_count: u32,
    atlas_bgl: &wgpu::BindGroupLayout,
    model3d_mvp_buf: &wgpu::Buffer,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer) {
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
    // Double-sided (cull None): the back face + inward walls must never cull.
    // Depth-test + write against the hand pass's own (cleared) depth buffer so the
    // extruded mesh self-sorts: front, stepped side walls and back no longer
    // overdraw each other in submission order. The hand pass clears depth, so this
    // stays isolated from the world (the item still draws over terrain).
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
        sample_count,
    );
    let item3d_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item3d vbuf"),
        size: MAX_ITEM3D_VERTICES * std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (item3d_pipe, item3d_mvp_bind, item3d_vbuf)
}
