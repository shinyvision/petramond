use super::builders::{
    buffer_bind_group, color_target, cull_back, pipeline_layout, shader_module, uniform_entry,
    world_pipeline, DepthPreset,
};
use crate::crosshair::MAX_CROSSHAIR_VERTICES;
use crate::uniforms::Uniforms;
use petramond_mesh::ContactShadowVertex;

/// Selection-outline pipeline.
/// Its own minimal bind-group layout (Uniforms at binding 0 only) so it
/// doesn't couple to the block pipelines' uv_rects layout. Reuses the same
/// uniform buffer for view_proj.
pub(super) fn create_selection_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
) -> (wgpu::RenderPipeline, wgpu::BindGroup, wgpu::Buffer) {
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
        array_stride: 12, // vec3<f32>
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
        sample_count,
    );
    // Selection outline vertices x vec3<f32>; grown by the renderer to fit
    // whatever outline a target resolves to.
    let outline_vbuf = crate::renderer::dynamic_draw::new_buffer(
        device,
        wgpu::BufferUsages::VERTEX,
        "outline vbuf",
    );
    (outline_pipe, outline_bind, outline_vbuf)
}

/// Center crosshair pipeline.
/// The fragment shader outputs white and the color blend computes
/// `white * (1 - dst) + dst * 0`, which inverts the pixels under the
/// crosshair instead of drawing a fixed light/dark color.
pub(super) fn create_crosshair_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    crosshair_shader: &wgpu::ShaderModule,
) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let crosshair_layout = pipeline_layout(device, "crosshair layout", &[]);
    let crosshair_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: 8, // vec2<f32>
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
    // write_mask = COLOR (not ALL): the invert blend must leave the alpha channel
    // untouched.
    let crosshair_targets = color_target(format, Some(invert_blend), wgpu::ColorWrites::COLOR);
    let crosshair_pipe = world_pipeline(
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
        sample_count,
    );
    let crosshair_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("crosshair vbuf"),
        size: (MAX_CROSSHAIR_VERTICES * 8) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (crosshair_pipe, crosshair_vbuf)
}

/// Break-overlay pipeline (the destroy crack).
/// Reuses the block `uniform_bgl` (group0: view_proj + uv_rects) + `atlas_bgl`
/// (group1) so it binds the renderer's existing `uniform_bind` / `atlas_bind`
/// unchanged. Same 32-byte vertex as the block pipe. MULTIPLY-blended; depth
/// LessEqual / no-write; the cube is built coincident with the block faces and a
/// small polygon offset (BREAK_DEPTH_BIAS) wins the depth tie on the surface, so
/// the crack reads cleanly with no inflation and no z-fighting.
pub(super) fn create_break_overlay_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    vbuf_layout: &wgpu::VertexBufferLayout,
) -> wgpu::RenderPipeline {
    let break_shader = shader_module(
        device,
        "break overlay shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/break_overlay.wgsl")
        ),
    );
    // MULTIPLY blend (result = src.rgb * dst.rgb): the crack fragment outputs WHITE
    // where the destroy tile is transparent (×1 = no change) and dark where the
    // crack texels are, so the cracks darken the block face instead of
    // alpha-compositing a flat overlay. `color = Dst * src + Zero * dst = src*dst`.
    // Alpha is preserved (Zero/One) — the colour target keeps its existing alpha.
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
    let break_targets = color_target(format, Some(multiply_blend), wgpu::ColorWrites::ALL);
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
        sample_count,
    );
    break_pipe
}

/// Model→terrain contact-shadow pipeline: the chunk `ContactShadowVertex`
/// stream (16-byte `{pos, darken}`, non-indexed), MULTIPLY-blended like the
/// break overlay, depth `LessEqual` read-only with its OWN coplanar bias
/// (`DepthPreset::ReadLessEqualContactBiased`). Drawn between the opaque and
/// sky passes — see `passes.rs` for why that order is a safety contract. Culling
/// is off: the stamp only shows where terrain was drawn under it, and a facing
/// rotation must never be able to wind it away.
///
/// Reuses the block group-0 layout (`layout` = the shared `[uniform_bgl,
/// atlas_bgl]` pipeline layout's group 0) via its own single-group pipeline
/// layout, so the pass binds the renderer's existing `uniform_bind` unchanged.
pub(super) fn create_contact_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
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
        std::slice::from_ref(&contact_vbuf_layout),
        &contact_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::ReadLessEqualContactBiased),
        sample_count,
    )
}

/// Entity blob-shadow pipeline: one horizontal quad per entity under the
/// contact-shadow pass's rules — MULTIPLY blend, depth `LessEqual` read-only
/// with the same coplanar bias, culling off. The static ibuf holds the quad
/// pattern repeated for every cap slot (indices are absolute into the shared
/// vbuf), so a frame draws its whole shadow batch in one call.
///
/// Reuses the block group-0 uniform layout like the contact pass; binds only
/// the renderer's existing `uniform_bind`.
pub(super) fn create_entity_shadow_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
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
        sample_count,
    );
    pipe
}
