use super::*;
use crate::renderer::instance_descriptor;
use crate::uniforms::ShaderParams;

#[test]
fn packed_vertex_pipeline_validates() {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let adapter = match pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::LowPower,
        compatible_surface: None,
        force_fallback_adapter: false,
    })) {
        Ok(a) => a,
        Err(_) => {
            eprintln!("[skip] no wgpu adapter; pipeline validation not run");
            return;
        }
    };
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: None,
        required_features: wgpu::Features::empty(),
        required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .expect("device");

    let tex = crate::gpu_mem::create_texture(
        &device,
        &wgpu::TextureDescriptor {
            label: Some("test atlas"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
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
    let atlas_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    let array_tex = crate::gpu_mem::create_texture(
        &device,
        &wgpu::TextureDescriptor {
            label: Some("test atlas array"),
            size: wgpu::Extent3d {
                width: 1,
                height: 1,
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
    let array_view = array_tex.create_view(&wgpu::TextureViewDescriptor {
        dimension: Some(wgpu::TextureViewDimension::D2Array),
        ..Default::default()
    });
    let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
        label: Some("test sampler"),
        ..Default::default()
    });
    let uniform_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test uniforms"),
        size: std::mem::size_of::<Uniforms>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let shader_params_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("test shader params"),
        size: std::mem::size_of::<ShaderParams>() as u64,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    device.push_error_scope(wgpu::ErrorFilter::Validation);
    for lines in [false, true] {
        let (p, _) =
            super::world_overlay::flat(&device, wgpu::TextureFormat::Rgba8UnormSrgb, 1, lines);
        let _ = p.get(1);
    }

    let resources = create_pipeline_resources(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        if adapter
            .get_downlevel_capabilities()
            .flags
            .contains(wgpu::DownlevelFlags::MULTISAMPLED_SHADING)
        {
            4
        } else {
            1
        },
        &uniform_buf,
        &shader_params_buf,
        &atlas_view,
        &sampler,
        &array_view,
        &sampler,
    );

    for model in [false, true] {
        let source = if model {
            &resources.world_model_pipe
        } else {
            &resources.dynamic_opaque_pipe
        };
        let ghost = super::world_overlay::ghost(
            &device,
            wgpu::TextureFormat::Rgba8UnormSrgb,
            1,
            &source.get(1).get_bind_group_layout(0),
            &source.get(1).get_bind_group_layout(1),
            model,
        );
        let _ = ghost.get(1);
    }

    let _ = resources.model_break_pipe.get(1);
    let _ = resources.skinned_pipe.get(1);
    let _ = resources.particle_pipe.get(1);
    let _ = resources.emitter_particle_pipe.get(1);

    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "real-pipeline validation error: {err:?}");
    assert!(Tile::count() <= petramond_mesh::MAX_TILES);
    assert_eq!(std::mem::size_of::<Vertex>(), 24);
    assert_eq!(std::mem::size_of::<petramond_mesh::TerrainVertex>(), 20);
    assert_eq!(std::mem::size_of::<crate::item_model::ItemVertex>(), 36);
    assert_eq!(std::mem::size_of::<petramond_mesh::ModelVertex>(), 32);
    assert_eq!(std::mem::offset_of!(petramond_mesh::ModelVertex, light), 24);
    assert_eq!(std::mem::offset_of!(petramond_mesh::ModelVertex, tint), 28);
}
