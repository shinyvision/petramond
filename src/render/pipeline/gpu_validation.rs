use super::*;
use crate::render::renderer::instance_descriptor;
use crate::render::uniforms::ShaderParams;

/// Headless validation that the REAL pipeline factory produces internally
/// consistent pipelines: WGSL parses + passes naga validation, each pass's
/// vertex attribute formats/locations match its shader's `VsIn`, and the
/// bind-group layouts match the shaders' declared bindings. This calls the
/// production `create_pipeline_resources` under a validation error scope and
/// asserts nothing was reported — so it can never drift from the runtime
/// pipelines the way a hand-copied descriptor would. Skips cleanly on
/// machines/CI with no GPU adapter (the interactive demo is where final
/// visual confirmation happens).
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

    // Fabricate the minimal external resources `create_pipeline_resources`
    // binds: a 1x1 Rgba8UnormSrgb texture view for both the block atlas and
    // the gui atlas (matches the real `Float { filterable: true }` / D2 BGLs),
    // a filtering sampler, and a uniform buffer sized to `Uniforms`. The
    // factory never samples or reads these in this test — it only builds bind
    // groups + pipelines — so 1x1 placeholders are sufficient to validate.
    let tex = device.create_texture(&wgpu::TextureDescriptor {
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
    });
    let atlas_view = tex.create_view(&wgpu::TextureViewDescriptor::default());
    // A 1-layer D2Array view for the terrain pipeline's tile-array bind (matches the
    // real `D2Array` BGL). A D2 texture with one layer views fine as D2Array.
    let array_tex = device.create_texture(&wgpu::TextureDescriptor {
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
    });
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

    // Build EVERY real pipeline through the production factory. Any
    // shader/layout/vertex-attribute/blend/depth mismatch surfaces as a
    // captured validation error below.
    let _resources = create_pipeline_resources(
        &device,
        &queue,
        wgpu::TextureFormat::Rgba8UnormSrgb,
        1,
        &uniform_buf,
        &shader_params_buf,
        &atlas_view,
        &sampler,
        &array_view,
        &sampler,
    );

    let err = pollster::block_on(device.pop_error_scope());
    assert!(err.is_none(), "real-pipeline validation error: {err:?}");
    // Confirm the assumption baked into the packing: tile ids fit in 8 bits
    // (also enforced by the atlas loader at composition time).
    assert!(Tile::count() <= 256);
    // Stride sanity: the compressed block vertex is exactly 24 bytes
    // (unorm8 tint + two packed u32 words).
    assert_eq!(std::mem::size_of::<Vertex>(), 24);
    assert_eq!(std::mem::size_of::<crate::mesh::TerrainVertex>(), 20);
    // item3d vertex stride must match its declared attribute layout
    // (pos f32x3 @0, uv f32x2 @12, shade f32 @20, tint f32x3 @24 = 36 bytes).
    assert_eq!(
        std::mem::size_of::<crate::render::item_model::ItemVertex>(),
        36
    );
}
