//! Renderer construction + surface lifecycle.
//!
//! Owns wgpu instance/adapter/device/surface bring-up — every failure a
//! typed [`RenderInitError`], never a panic — the shared frame resources
//! (uniforms, atlases, the icon-atlas bake), and the assembly of the
//! `Renderer` from its passes, each of which builds itself from the pipeline
//! resources it owns (`construct/passes.rs`, with the per-species actor
//! resources in `construct/actors.rs` and the HUD layers in
//! `construct/hud.rs`).

use super::*;

mod actors;
mod hud;
mod passes;
use passes::{HandParts, SkyParts};

/// A renderer presenting to `target` at `width` × `height`, or why the
/// platform cannot give it one: no surface, no adapter, no device, or an
/// adapter that cannot present to the surface.
pub async fn new_renderer_from_target(
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
) -> Result<Renderer, RenderInitError> {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let surface = instance
        .create_surface(target)
        .map_err(RenderInitError::CreateSurface)?;
    let adapter = request_adapter(&instance, Some(&surface)).await?;
    let (device, queue) = request_device(&adapter).await?;
    let config = surface
        .get_default_config(&adapter, width, height)
        .ok_or(RenderInitError::SurfaceUnsupported)?;
    surface.configure(&device, &config);
    let samples = max_scene_samples(&adapter, config.format);
    new_renderer_inner(Some(surface), device, queue, config, samples)
}

/// Instance descriptor selecting native backends (Vulkan/Metal/DX12/GL).
///
/// Honors `WGPU_BACKEND` (`vulkan` | `gl`) to pin a single backend; unset = all.
/// This matters on a hybrid-GPU Wayland session: the discrete NVIDIA GPU's Vulkan
/// WSI can't present to a Wayland surface it isn't driving (it reports
/// `VK_KHR_wayland_surface` present = false), so wgpu's surface-compatible pick
/// falls back to the Intel iGPU. Its EGL/GLES path *can* present there, so
/// `WGPU_BACKEND=gl` (with the EGL vendor pointed at NVIDIA) renders on the dGPU.
pub(crate) fn instance_descriptor() -> wgpu::InstanceDescriptor {
    let mut desc = wgpu::InstanceDescriptor::default();
    if let Ok(name) = std::env::var("WGPU_BACKEND") {
        match name.trim().to_ascii_lowercase().as_str() {
            "vulkan" | "vk" => desc.backends = wgpu::Backends::VULKAN,
            "gl" | "gles" | "opengl" => desc.backends = wgpu::Backends::GL,
            _ => {}
        }
    }
    desc
}

/// Adapter pick shared by every renderer bring-up: a high-performance adapter
/// first, then the forced fallback (software) one, and an error only when
/// neither exists. `surface` is `None` for a surfaceless renderer, which
/// constrains nothing.
pub(super) async fn request_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> Result<wgpu::Adapter, RenderInitError> {
    match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(adapter) => Ok(adapter),
        Err(e) => {
            log::warn!("wgpu: primary adapter unavailable ({e}); trying the fallback adapter");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: surface,
                    force_fallback_adapter: true,
                })
                .await
                .map_err(RenderInitError::NoAdapter)
        }
    }
}

/// The device every renderer needs. The terrain tile array holds every tile
/// PLUS its dye-base twin (2 × tile count layers), which exceeds the default
/// 256-layer limit — request what the tile array actually needs, capped to what
/// the adapter offers; content that overflows the resulting limits is reported
/// by `content_limits::check` with the offending counts before the device
/// exists.
pub(super) async fn request_device(
    adapter: &wgpu::Adapter,
) -> Result<(wgpu::Device, wgpu::Queue), RenderInitError> {
    let mut required_limits = wgpu::Limits::default().using_alignment(adapter.limits());
    required_limits.max_texture_array_layers = (2 * petramond_world::tile::Tile::count() as u32)
        .max(required_limits.max_texture_array_layers)
        .min(adapter.limits().max_texture_array_layers);
    // The limits the device will have: content past them is named here.
    crate::content_limits::check(&required_limits).map_err(RenderInitError::ContentLimits)?;
    // Adapter-specific format features expose supported 8x MSAA; timestamps
    // remain opt-in for the GPU-timing instrument.
    let mut required_features =
        adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    if gpu_timer::GpuTimer::wanted() {
        required_features |= adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    }
    required_features |= super::draw_plan::terrain_draw_features(adapter);
    adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features,
            required_limits,
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .map_err(RenderInitError::RequestDevice)
}

/// Build every pipeline, atlas and pass and assemble the `Renderer`.
/// `config` carries the frame geometry + colour format; `surface` is `None`
/// for a surfaceless renderer, which changes nothing else.
pub(super) fn new_renderer_inner(
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    max_samples: u32,
) -> Result<Renderer, RenderInitError> {
    // First, so the rest of the bring-up already reports through it.
    let health = DeviceHealth::watch(&device);
    let graph =
        super::passes::frame_graph().map_err(|e| RenderInitError::PassGraph(e.to_string()))?;
    let (width, height) = (config.width, config.height);
    let anti_aliasing = super::post_process::supported_mode(
        petramond::save::client::AntiAliasing::default(),
        super::post_process::max_multiplier(
            (width, height),
            device.limits().max_texture_dimension_2d,
        ),
        max_samples,
    );
    let sample_axis = anti_aliasing.resolution_multiplier();
    let scene = (width * sample_axis, height * sample_axis);
    let format = config.format;

    let (_atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let (_atlas_array_texture, atlas_array_view, atlas_array_sampler) =
        create_atlas_array(&device, &queue);
    // Overridden by `set_render_distance` at host wiring; the default keeps the
    // icon-atlas bake (which reads this buffer) fog-free at any distance.
    let default_fog = crate::uniforms::fog_range(petramond::world::RENDER_DIST);
    let uniform_buf = create_uniform_buffer(&device, default_fog);
    let shader_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("shader params"),
        contents: bytemuck::cast_slice(&[crate::uniforms::ShaderParams {
            values: [[0.0; 4]; crate::uniforms::SHADER_PARAM_SLOTS],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let pipelines = create_pipeline_resources(
        &device,
        &queue,
        format,
        max_samples,
        &uniform_buf,
        &shader_params_buf,
        &atlas_view,
        &atlas_sampler,
        &atlas_array_view,
        &atlas_array_sampler,
    );
    let model_atlas_bind = create_model_atlas_bind(&device, &queue, &pipelines.atlas_bgl);
    // The bbmodel break crack draws the same model stream a second time; its own
    // group(2) holds the frame's crack masks and the BLOCK atlas (the destroy
    // tiles live there, not in the model atlas).
    let model_break = crate::model_break::ModelBreak::new(
        &device,
        pipelines.model_break_pipe,
        &pipelines.model_break_bgl,
        &atlas_view,
        &atlas_sampler,
    );
    // A custom-shape block's inventory icon is its baked ITEM geometry (a chair,
    // not a plank cube), which comes from the pack's WASM — bake all installed
    // custom item shapes into the item cache NOW, before the icon atlas reads it.
    petramond::modding::client::bake_installed_custom_item_geometry();
    // Bake every item's inventory icon into the icon atlas ONCE, here at init: the
    // cube/sprite icons through the depthless `model3d_pipe` and the bbmodel-block
    // icons through the depth-tested `model_icon_pipe` (these two pipelines are used
    // only by this bake — see `icon_atlas`). The atlas color format MUST match the
    // surface (sRGB) so sampling/store cancel like the gui atlas (no double gamma).
    // The per-slot UI node then draws a textured quad sampling this.
    let icon_atlas = icon_atlas::bake(
        &device,
        &queue,
        format,
        &pipelines.atlas_bgl,
        &pipelines.atlas_bind,
        &model_atlas_bind,
        &pipelines.model3d_pipe,
        &pipelines.model_icon_pipe,
        &pipelines.model3d_mvp_bgl,
        &pipelines.uv_rects_buf,
        &uniform_buf,
    );

    let targets = SceneTargets::new(
        &device,
        format,
        scene,
        anti_aliasing,
        max_samples,
        pipelines.grade_pipe,
        pipelines.grade_bgl,
    );
    let sky = SkyPass::new(
        &device,
        SkyParts {
            pipe: pipelines.sky_pipe,
            bind: pipelines.sky_bind,
            texture_bind: pipelines.sky_texture_bind,
            shader_param_keys: pipelines.sky_shader_param_keys,
            light_param_key: pipelines.sky_light_param_key,
            env_passes: pipelines.env_passes,
            env_scaler: pipelines.env_scaler,
        },
        &uniform_buf,
        &targets,
        scene,
        format,
        default_fog,
    );
    let terrain = TerrainPass::new(
        &device,
        &queue,
        TerrainPipes {
            opaque: pipelines.opaque_pipe,
            translucent: pipelines.translucent_pipe,
            transparent: pipelines.transparent_pipe,
            transparent_two_sided: pipelines.transparent_two_sided_pipe,
            world_model: pipelines.world_model_pipe,
            world_model_blend: pipelines.world_model_blend_pipe,
            contact: pipelines.contact_pipe,
        },
    );
    let hand = HandPass::new(
        &device,
        HandParts {
            model3d_pipe: pipelines.model3d_hand_pipe,
            model3d_mvp_buf: pipelines.model3d_mvp_buf,
            model3d_mvp_bind: pipelines.model3d_mvp_bind,
            model3d_vbuf: pipelines.model3d_vbuf,
            model3d_ibuf: pipelines.model3d_ibuf,
            item3d_pipe: pipelines.item3d_pipe,
            item3d_mvp_bind: pipelines.item3d_mvp_bind,
            item3d_vbuf: pipelines.item3d_vbuf,
        },
        pipelines.break_pipe,
    );
    let ui = UiPass::new(
        &device,
        &queue,
        pipelines.ui_pipe,
        &pipelines.atlas_bgl,
        pipelines.ui_vbuf,
        icon_atlas,
    );
    let chrome = ChromePass::new(
        pipelines.outline_pipe,
        pipelines.outline_bind,
        pipelines.outline_vbuf,
        pipelines.crosshair_pipe,
        pipelines.crosshair_vbuf,
    );
    let actor = ActorPass::new(
        &device,
        &queue,
        &pipelines.atlas_bgl,
        pipelines.skinned_pipe,
        pipelines.bone_palette_bgl,
        &pipelines.mob_pipe,
        &pipelines.dynamic_opaque_pipe,
    );
    let item_entity =
        ItemEntityPass::new(&device, &pipelines.dynamic_opaque_pipe, &pipelines.mob_pipe);
    let block_entity = BlockEntityPass::new(&device, pipelines.dynamic_opaque_pipe);
    let particle = ParticlePass::new(
        &device,
        pipelines.particle_pipe,
        pipelines.emitter_particle_pipe,
    );
    let shadow = ShadowPass::new(&device, pipelines.entity_shadow_pipe);
    let binds = SharedBinds {
        uniform_buf,
        shader_params_buf,
        uniform: pipelines.uniform_bind,
        atlas: pipelines.atlas_bind,
        atlas_array: pipelines.atlas_array_bind,
        model_atlas: model_atlas_bind,
    };

    Ok(Renderer {
        ghosts: Default::default(),
        selection: Default::default(),
        gpu_timer: gpu_timer::GpuTimer::new(&device, &queue),
        surface,
        device,
        queue,
        config,
        offscreen_target: None,
        suboptimal_retried: false,
        health,
        graph,
        frame_plan: FramePlan::default(),
        binds,
        model_break,
        terrain,
        view: ViewState::initial(),
        targets,
        chrome,
        sky,
        ui,
        hand,
        particle,
        item_entity,
        actor,
        shadow,
        block_entity,
        last_stats: RenderStats::default(),
    })
}

/// The frame uniforms before the first `update_uniforms`: identity view, the
/// default fog band, white sky, late-morning sun. The icon-atlas bake reads
/// this buffer, so these are also what the baked icons see.
fn create_uniform_buffer(device: &wgpu::Device, (fog_start, fog_end): (f32, f32)) -> wgpu::Buffer {
    device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [fog_start, fog_end, 0.0, 0.0],
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            render_origin: [0; 4],
            atlas_layout: crate::atlas::atlas_layout_uniform(),
            // White sky colour at init = identity, so baked UI icons stay
            // untinted.
            sky_color: [1.0, 1.0, 1.0, 0.0],
            // Late-morning sun at full daylight until the sim writes petramond:time.
            sun_dir: super::frame_state::sun_uniform(None),
            volume_tint: [1.0, 1.0, 1.0, 0.0],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    })
}

/// The combined bbmodel-block atlas (every kind's textures packed into one
/// sheet — see `block_model::atlas`) as its own GPU texture, bound over the
/// same atlas layout the mob pipeline uses. The mesher bakes model geometry
/// into each chunk's model stream; the model nodes just draw it over this.
fn create_model_atlas_bind(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> wgpu::BindGroup {
    let atlas = petramond_world::block_model::atlas();
    let (rgba, w, h) = atlas.texture();
    let (_texture, view, sampler) = create_model_texture(device, queue, rgba, w, h);
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model atlas bg"),
        layout: atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&sampler),
            },
        ],
    })
}

impl Renderer {
    /// The current surface size in physical pixels `(width, height)` — the same
    /// coordinate space the UI layout (`render::ui`) and cursor hit-testing use.
    #[inline]
    pub fn screen_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn ui_viewport(&self) -> UiViewport {
        UiViewport::new(self.screen_size(), self.ui.viewport_generation)
    }
}

pub(super) fn max_scene_samples(adapter: &wgpu::Adapter, format: wgpu::TextureFormat) -> u32 {
    // Cloud occlusion reads scene depth per coverage sample.
    if !adapter
        .get_downlevel_capabilities()
        .flags
        .contains(wgpu::DownlevelFlags::MULTISAMPLED_SHADING)
    {
        return 1;
    }
    let features = adapter.features();
    let flags = |format: wgpu::TextureFormat| {
        if features.contains(wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES) {
            adapter.get_texture_format_features(format).flags
        } else {
            format.guaranteed_format_features(features).flags
        }
    };
    let color = flags(format);
    let depth = flags(wgpu::TextureFormat::Depth32Float);
    [8, 4, 1]
        .into_iter()
        .find(|n| color.sample_count_supported(*n) && depth.sample_count_supported(*n))
        .unwrap_or(1)
}
