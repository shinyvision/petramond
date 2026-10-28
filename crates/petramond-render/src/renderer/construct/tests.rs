use super::*;
use petramond_util::test_dirs::TestScratchDir;
use petramond_world::assets::{PackRoots, PackSet};
use petramond_world::content::{self, Content, ContentRegistry};

#[test]
fn content_preparation_preserves_the_live_renderer_until_commit_and_on_failure() {
    let roots = PackRoots::with_mods([]);
    let source = ContentRegistry::load(PackSet::discover(&roots)).unwrap();
    let _pin = content::pin(source);
    let candidate = ContentRegistry::load(PackSet::discover(&roots)).unwrap();
    let layers = 2 * petramond_world::tile::Tile::count() as u32;
    let instance = wgpu::Instance::new(&instance_descriptor());
    let adapter = match pollster::block_on(request_adapter(&instance, None)) {
        Ok(adapter) => adapter,
        Err(_) => {
            eprintln!("[skip] no wgpu adapter; renderer content transition not run");
            return;
        }
    };
    let mut limits = wgpu::Limits::default().using_alignment(adapter.limits());
    limits.max_texture_array_layers = layers;
    limits.max_texture_dimension_2d = adapter.limits().max_texture_dimension_2d;
    limits.max_buffer_size = adapter.limits().max_buffer_size;
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("content transition test"),
        required_features: wgpu::Features::empty(),
        required_limits: limits,
        experimental_features: wgpu::ExperimentalFeatures::disabled(),
        memory_hints: wgpu::MemoryHints::Performance,
        trace: wgpu::Trace::Off,
    }))
    .unwrap();
    let health = DeviceHealth::watch(&device);
    let config = wgpu::SurfaceConfiguration {
        usage: wgpu::TextureUsages::RENDER_ATTACHMENT,
        format: wgpu::TextureFormat::Rgba8UnormSrgb,
        width: 32,
        height: 32,
        present_mode: wgpu::PresentMode::Fifo,
        desired_maximum_frame_latency: 2,
        alpha_mode: wgpu::CompositeAlphaMode::Auto,
        view_formats: vec![],
    };
    let mut renderer = new_renderer_inner(None, device, queue, config, 1, health).unwrap();
    let prepared = renderer.prepare_content(candidate).unwrap().unwrap();
    assert!(Content::current().same(source));
    assert!(renderer.content().same(source));
    renderer.commit_content(prepared);
    assert!(renderer.content().same(candidate));
    assert!(renderer.prepare_content(candidate).unwrap().is_none());

    let scratch = TestScratchDir::new("renderer-content-limit");
    let mods = scratch.join("mods");
    let pack = mods.join("limitfixture");
    std::fs::create_dir_all(pack.join("textures")).unwrap();
    std::fs::write(
        pack.join("pack.json"),
        r#"{"id":"limitfixture","name":"Content limit fixture"}"#,
    )
    .unwrap();
    std::fs::write(
        pack.join("textures/atlas.json"),
        serde_json::json!({"tiles":[{
            "name":"limitfixture:extra",
            "file":petramond_world::tile::cells()[0].file
        }]})
        .to_string(),
    )
    .unwrap();
    let oversized = content::test_support::with_mods(&mods);
    renderer
        .device
        .push_error_scope(wgpu::ErrorFilter::Validation);
    assert!(matches!(
        renderer.prepare_content(oversized),
        Err(RenderInitError::ContentLimits(_))
    ));
    assert!(renderer.content().same(candidate));
    assert!(Content::current().same(source));
    assert!(renderer.failure().is_none());
    assert!(pollster::block_on(renderer.device.pop_error_scope()).is_none());
}
