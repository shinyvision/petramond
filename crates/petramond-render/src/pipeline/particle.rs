use super::builders::{color_target, cull_back, shader_module, world_pipeline, DepthPreset};

pub(super) struct ParticlePipelineResources {
    pub(super) pipe: wgpu::RenderPipeline,
    pub(super) emitter_pipe: wgpu::RenderPipeline,
}

/// Particle pipelines (tiny 3D cubes). Mining/break particles use alpha cutout and
/// depth writes. Block-row emitter particles use solid vertex colors, alpha blending,
/// depth read-only, and back-face culling.
pub(super) fn create_particle_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
) -> ParticlePipelineResources {
    let particle_shader = shader_module(
        device,
        "particle shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/particles.wgsl")
        ),
    );
    let particle_vbuf_attrs = [
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
            format: wgpu::VertexFormat::Float32x3,
            offset: 20,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 32,
            shader_location: 3,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 36,
            shader_location: 4,
        },
    ];
    let particle_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::particles::ParticleVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &particle_vbuf_attrs,
    };
    // Opaque cubes (cutout discard handles transparency) — no blend. Cubes carry
    // their own per-face winding; disabling cull is robust (and the cutout discard
    // means we never rely on backface rejection for the look). Depth Less + write.
    let particle_targets = color_target(format, None, wgpu::ColorWrites::ALL);
    let particle_pipe = world_pipeline(
        device,
        "particle pipe",
        layout,
        &particle_shader,
        "vs_particle",
        "fs_particle",
        std::slice::from_ref(&particle_vbuf_layout),
        &particle_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    let emitter_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let emitter_pipe = world_pipeline(
        device,
        "emitter particle pipe",
        layout,
        &particle_shader,
        "vs_particle",
        "fs_particle_transparent",
        std::slice::from_ref(&particle_vbuf_layout),
        &emitter_targets,
        cull_back(),
        Some(DepthPreset::ReadLess),
        sample_count,
    );
    ParticlePipelineResources {
        pipe: particle_pipe,
        emitter_pipe,
    }
}
