use super::builders::{color_target, cull_back, shader_module, world_pipeline, DepthPreset};

pub(super) struct ParticlePipelineResources {
    pub(super) pipe: crate::pipeline::SampledPipeline,
    pub(super) emitter_pipe: crate::pipeline::SampledPipeline,
}

/// `ParticleRow`'s attributes, in field order (centre / half / right / stretch
/// / up / alpha / uv_min / uv_max / tint / quad).
const PARTICLE_ROW_ATTRS: [wgpu::VertexAttribute; 10] = wgpu::vertex_attr_array![
    0 => Float32x3,
    1 => Float32,
    2 => Float32x3,
    3 => Float32,
    4 => Float32x3,
    5 => Float32,
    6 => Float32x2,
    7 => Float32x2,
    8 => Float32x3,
    9 => Uint32,
];

/// The particle module: the helpers, the face table generated from
/// `particles::FACES`, then `particles.wgsl`.
fn particle_shader_source() -> String {
    [
        include_str!("../../shaders/cel.wgsl"),
        include_str!("../../shaders/atmosphere.wgsl"),
        &super::particles::wgsl_faces(),
        include_str!("../../shaders/particles.wgsl"),
    ]
    .concat()
}

/// Particle pipelines (tiny 3D cubes, one instance per particle). Mining/break
/// particles use alpha cutout and depth writes. Block-row emitter particles use
/// solid colors, alpha blending, depth read-only, and back-face culling.
pub(super) fn create_particle_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    max_samples: u32,
    layout: &wgpu::PipelineLayout,
) -> ParticlePipelineResources {
    let particle_shader = shader_module(device, "particle shader", particle_shader_source());
    // One instance-stepped buffer and no per-vertex one: the vertex stage
    // expands each row from `vertex_index`.
    let particle_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::particles::ParticleRow>() as u64,
        step_mode: wgpu::VertexStepMode::Instance,
        attributes: &PARTICLE_ROW_ATTRS,
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
        max_samples,
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
        max_samples,
    );
    ParticlePipelineResources {
        pipe: particle_pipe,
        emitter_pipe,
    }
}

#[cfg(test)]
mod tests {
    /// The particle module (with its generated face table) must parse and
    /// validate, with the entry points the pipelines name.
    #[test]
    fn particle_shader_validates() {
        let source = super::particle_shader_source();
        let module = naga::front::wgsl::parse_str(&source).expect("particle shader parses");
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::default(),
        )
        .validate(&module)
        .expect("particle shader validates");
        for entry in ["vs_particle", "fs_particle", "fs_particle_transparent"] {
            assert!(
                module.entry_points.iter().any(|e| e.name == entry),
                "no `{entry}` entry point"
            );
        }
    }
}
