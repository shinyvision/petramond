use super::builders::{
    buffer_bind_group, color_target, cull_back, pipeline_layout, shader_module, single_pipeline,
    uniform_entry, world_pipeline, DepthPreset,
};
use crate::crosshair::MAX_CROSSHAIR_VERTICES;
use crate::uniforms::Uniforms;
use petramond_mesh::ContactShadowVertex;

pub(super) fn create_selection_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    uniform_buf: &wgpu::Buffer,
) -> (
    crate::pipeline::SampledPipeline,
    wgpu::BindGroup,
    wgpu::Buffer,
) {
    let outline_shader = shader_module(
        device,
        "outline shader",
        include_str!("../../shaders/outline.wgsl"),
    );
    let outline_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("outline bgl"),
        entries: &[uniform_entry(
            0,
            wgpu::ShaderStages::VERTEX,
            std::mem::size_of::<Uniforms>() as u64,
        )],
    });
    let outline_bind = buffer_bind_group(device, "outline bg", &outline_bgl, &[uniform_buf]);
    let outline_layout = pipeline_layout(device, "outline layout", &[&outline_bgl]);
    let outline_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 12,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        }],
    };
    let outline_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    // Depth-test against terrain so edges behind blocks are hidden, but don't
    // write depth. The box is inflated slightly outward (see `outline_vertices`)
    // so visible front edges win the LessEqual test.
    let outline_pipe = world_pipeline(
        device,
        "outline pipe",
        &outline_layout,
        &outline_shader,
        "vs_outline",
        "fs_outline",
        &[outline_vbuf_layout],
        &outline_targets,
        wgpu::PrimitiveState {
            topology: wgpu::PrimitiveTopology::LineList,
            ..Default::default()
        },
        Some(DepthPreset::ReadLessEqual),
        max_samples,
    );
    let outline_vbuf = crate::renderer::dynamic_draw::new_buffer(
        device,
        wgpu::BufferUsages::VERTEX,
        "outline vbuf",
    );
    (outline_pipe, outline_bind, outline_vbuf)
}

/// Crosshair pipeline. Shader outputs white, blend `white * (1 - dst) + dst * 0` inverts
/// what's under it.
pub(super) fn create_crosshair_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    crosshair_shader: &wgpu::ShaderModule,
) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let crosshair_layout = pipeline_layout(device, "crosshair layout", &[]);
    let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 8,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &[wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        }],
    };
    let invert_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::OneMinusDst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let crosshair_targets = color_target(format, Some(invert_blend), wgpu::ColorWrites::COLOR);
    let crosshair_pipe = single_pipeline(
        device,
        "crosshair pipe",
        &crosshair_layout,
        crosshair_shader,
        "vs_crosshair",
        "fs_crosshair",
        &[crosshair_vbuf_layout],
        &crosshair_targets,
        wgpu::PrimitiveState::default(),
        None,
    );
    let crosshair_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("crosshair vbuf"),
        size: (MAX_CROSSHAIR_VERTICES * 8) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (crosshair_pipe, crosshair_vbuf)
}

pub(super) const MULTIPLY_BLEND: wgpu::BlendState = wgpu::BlendState {
    color: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Dst,
        dst_factor: wgpu::BlendFactor::Zero,
        operation: wgpu::BlendOperation::Add,
    },
    alpha: wgpu::BlendComponent {
        src_factor: wgpu::BlendFactor::Zero,
        dst_factor: wgpu::BlendFactor::One,
        operation: wgpu::BlendOperation::Add,
    },
};

/// Break-overlay pipeline (the destroy crack).
/// Reuses the block's `uniform_bgl`/`atlas_bgl` groups so it binds the existing
/// `uniform_bind`/`atlas_bind` as-is. Same 32-byte vertex as the block pipe.
/// Multiply blend, depth LessEqual/no-write. The cube sits coincident with the block
/// faces and `BREAK_DEPTH_BIAS` wins the depth tie, so the crack shows without
/// inflating the geometry or z-fighting.
pub(super) fn create_break_overlay_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    layout: &wgpu::PipelineLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> crate::pipeline::SampledPipeline {
    let break_shader = shader_module(
        device,
        "break overlay shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/break_overlay.wgsl")
        ),
    );
    let break_targets = color_target(format, Some(MULTIPLY_BLEND), wgpu::ColorWrites::ALL);
    // group0 = block uniform layout (Uniforms + uv_rects); group1 = atlas. Same
    // layout object as the opaque/transparent pipes (`layout`). Depth `LessEqual`,
    // NO write, with the BREAK_DEPTH_BIAS polygon offset (DepthPreset::
    // ReadLessEqualBiased): the crack cube is COINCIDENT with the block faces, but
    // the chunk mesher flips each face's triangulation diagonal per-AO (see
    // `should_flip` in mesh::face) while this cube always splits 0->2. Two
    // triangulations of the same plane interpolate depth a ULP apart per pixel,
    // which speckle-fights under a plain LessEqual; the small offset toward the
    // camera makes the crack win that tie everywhere, with no geometric inflation
    // to misalign the decal at glancing angles.
    let break_pipe = world_pipeline(
        device,
        "break overlay pipe",
        layout,
        &break_shader,
        "vs_break",
        "fs_break",
        std::slice::from_ref(vbuf_layout),
        &break_targets,
        cull_back(),
        Some(DepthPreset::ReadLessEqualBiased),
        max_samples,
    );
    break_pipe
}

/// Model to terrain contact-shadow pipeline. Uses the chunk `ContactShadowVertex`
/// stream (16-byte `{pos, darken}`, non-indexed), multiply blend like the break
/// overlay, depth LessEqual read-only with its own coplanar bias
/// (`DepthPreset::ReadLessEqualContactBiased`).
/// Drawn between opaque and sky passes - see passes.rs, the ordering there is a
/// safety contract. Culling off on purpose: the stamp only shows where terrain
/// was drawn under it, so a facing rotation can't wind it away.
/// Reuses the block's group-0 layout (from the shared `[uniform_bgl, atlas_bgl]`
/// pipeline layout) via its own single-group layout, so it binds the existing
/// `uniform_bind` unchanged.
pub(super) fn create_contact_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    uniform_bgl: &wgpu::BindGroupLayout,
) -> crate::pipeline::SampledPipeline {
    let contact_shader = shader_module(
        device,
        "contact shadow shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/contact.wgsl")
        ),
    );
    let multiply_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let contact_targets = color_target(format, Some(multiply_blend), wgpu::ColorWrites::ALL);
    let contact_layout = pipeline_layout(device, "contact layout", &[uniform_bgl]);
    let contact_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 12,
            shader_location: 1,
        },
    ];
    let contact_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<ContactShadowVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &contact_vbuf_attrs,
    };
    world_pipeline(
        device,
        "contact shadow pipe",
        &contact_layout,
        &contact_shader,
        "vs_contact",
        "fs_contact",
        &[contact_vbuf_layout, crate::resources::COLUMN_ORIGIN_LAYOUT],
        &contact_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::ReadLessEqualContactBiased),
        max_samples,
    )
}

pub(super) fn create_entity_shadow_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    uniform_bgl: &wgpu::BindGroupLayout,
) -> crate::pipeline::SampledPipeline {
    let shadow_shader = shader_module(
        device,
        "entity shadow shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/entity_shadow.wgsl")
        ),
    );
    let multiply_blend = wgpu::BlendState {
        color: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Dst,
            dst_factor: wgpu::BlendFactor::Zero,
            operation: wgpu::BlendOperation::Add,
        },
        alpha: wgpu::BlendComponent {
            src_factor: wgpu::BlendFactor::Zero,
            dst_factor: wgpu::BlendFactor::One,
            operation: wgpu::BlendOperation::Add,
        },
    };
    let shadow_targets = color_target(format, Some(multiply_blend), wgpu::ColorWrites::ALL);
    let shadow_layout = pipeline_layout(device, "entity shadow layout", &[uniform_bgl]);
    let shadow_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 20,
            shader_location: 2,
        },
    ];
    let shadow_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<crate::entity_shadow::ShadowVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &shadow_vbuf_attrs,
    };
    let pipe = world_pipeline(
        device,
        "entity shadow pipe",
        &shadow_layout,
        &shadow_shader,
        "vs_entity_shadow",
        "fs_entity_shadow",
        std::slice::from_ref(&shadow_vbuf_layout),
        &shadow_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::ReadLessEqualContactBiased),
        max_samples,
    );
    pipe
}
