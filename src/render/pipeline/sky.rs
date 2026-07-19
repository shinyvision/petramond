use super::builders::{
    buffer_bind_group, color_target, pipeline_layout, shader_module, texture_sampler_bind_entries,
    texture_sampler_layout_entries, uniform_entry, world_pipeline, DepthPreset,
};
use crate::render::uniforms::{ShaderParams, Uniforms};

/// Load a pack shader row's declared texture paths into the four fixed
/// slots, blank-filling missing/undecodable slots, and bind them at
/// `slot*2`/`slot*2+1`. Shared by the sky and environment hooks.
pub(super) fn create_shader_texture_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    texture_bgl: &wgpu::BindGroupLayout,
    kind: &str,
    paths: &[String],
) -> wgpu::BindGroup {
    let mut slots = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS);
    for slot in 0..super::shader_pack::SKY_TEXTURE_SLOTS {
        let loaded = paths.get(slot).and_then(|rel| {
            let Some((bytes, path)) = crate::assets::read_bytes(rel) else {
                log::warn!("{kind} texture slot {slot} asset '{rel}' not found; using blank slot");
                return None;
            };
            match super::resources::create_sky_texture(device, queue, &bytes) {
                Some(texture) => {
                    log::info!("{kind} texture slot {slot} loaded from {}", path.display());
                    Some(texture)
                }
                None => {
                    log::warn!(
                        "{kind} texture slot {slot} asset '{}' is not a decodable PNG; using blank slot",
                        path.display()
                    );
                    None
                }
            }
        });
        slots.push(loaded.unwrap_or_else(|| {
            super::resources::create_solid_rgba_texture(
                device,
                queue,
                [0, 0, 0, 0],
                "blank shader texture",
            )
        }));
    }
    let mut entries = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS * 2);
    for (slot, (_, view, sampler)) in slots.iter().enumerate() {
        entries.extend(texture_sampler_bind_entries(
            (slot * 2) as u32,
            view,
            sampler,
        ));
    }
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("pack shader texture bg"),
        layout: texture_bgl,
        entries: &entries,
    })
}

/// The values the sky pass hands back to [`PipelineResources`].
pub(super) struct SkyResources {
    pub(super) pipe: wgpu::RenderPipeline,
    pub(super) bind: wgpu::BindGroup,
    pub(super) texture_bind: wgpu::BindGroup,
    pub(super) shader_param_keys: Vec<String>,
    pub(super) light_param_key: Option<String>,
}

/// Sky-background pipeline.
/// Uses a sky-specific group 0 (frame uniforms + mod shader params) and a
/// fixed sky-texture group 1. It does not use terrain atlas resources or the
/// block pipeline's uv-rect table.
pub(super) fn create_sky_pipeline(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    sample_count: u32,
    uniform_buf: &wgpu::Buffer,
    shader_params_buf: &wgpu::Buffer,
) -> SkyResources {
    let sky_spec = super::shader_pack::active_sky_shader();
    let sky_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<Uniforms>() as u64,
            ),
            uniform_entry(
                1,
                wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<ShaderParams>() as u64,
            ),
        ],
    });
    let sky_bind = buffer_bind_group(
        device,
        "sky bg",
        &sky_bgl,
        &[uniform_buf, shader_params_buf],
    );
    let mut sky_texture_bgl_entries = Vec::with_capacity(super::shader_pack::SKY_TEXTURE_SLOTS * 2);
    for slot in 0..super::shader_pack::SKY_TEXTURE_SLOTS {
        sky_texture_bgl_entries.extend(texture_sampler_layout_entries(
            (slot * 2) as u32,
            wgpu::TextureViewDimension::D2,
        ));
    }
    let sky_texture_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("sky texture bgl"),
        entries: &sky_texture_bgl_entries,
    });
    let sky_texture_paths: &[String] = sky_spec
        .as_ref()
        .map_or(&[], |spec| spec.textures.as_slice());
    let sky_texture_bind =
        create_shader_texture_bind(device, queue, &sky_texture_bgl, "sky", sky_texture_paths);
    let sky_layout = pipeline_layout(device, "sky layout", &[&sky_bgl, &sky_texture_bgl]);
    let sky_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let builtin_sky_shader = shader_module(
        device,
        "built-in sky shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/sky.wgsl")
        ),
    );
    let sky_pipe_for = |shader: &wgpu::ShaderModule| {
        world_pipeline(
            device,
            "sky pipe",
            &sky_layout,
            shader,
            "vs_sky",
            "fs_sky",
            &[],
            &sky_targets,
            wgpu::PrimitiveState::default(),
            // The sky draws AFTER opaque terrain at exactly the far plane
            // (vs_sky emits z = 1.0), so LessEqual shades only the pixels no
            // terrain covered — the expensive sky fs skips the overdrawn ~80–90%.
            Some(DepthPreset::ReadLessEqual),
            sample_count,
        )
    };
    let sky_pipe = if let Some(spec) = sky_spec.as_ref() {
        device.push_error_scope(wgpu::ErrorFilter::Validation);
        let custom = shader_module(device, "pack sky shader", spec.source.clone());
        let pipe = sky_pipe_for(&custom);
        match pollster::block_on(device.pop_error_scope()) {
            Some(err) => {
                log::warn!(
                    "ignoring pack sky shader {}: validation failed: {err}",
                    spec.path.display()
                );
                sky_pipe_for(&builtin_sky_shader)
            }
            None => {
                log::info!("using pack sky shader {}", spec.path.display());
                pipe
            }
        }
    } else {
        sky_pipe_for(&builtin_sky_shader)
    };
    let sky_shader_param_keys = sky_spec
        .as_ref()
        .map_or_else(Vec::new, |spec| spec.params.clone());
    let sky_light_param_key = sky_spec
        .as_ref()
        .and_then(|spec| spec.sky_light_param.clone());

    SkyResources {
        pipe: sky_pipe,
        bind: sky_bind,
        texture_bind: sky_texture_bind,
        shader_param_keys: sky_shader_param_keys,
        light_param_key: sky_light_param_key,
    }
}
