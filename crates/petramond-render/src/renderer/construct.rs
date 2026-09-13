//! Renderer construction + surface lifecycle.
//!
//! Owns wgpu instance/adapter/device/surface bring-up, per-species + model
//! atlas resources, the icon-atlas bake, the big `Renderer { .. }` initializer,
//! and `screen_size` / `resize`. Split out of the renderer god-file; behavior is
//! byte-for-byte identical. The `new_renderer_from_target` / `instance_descriptor`
//! external paths are preserved via re-exports in the parent module.

use super::*;

mod actors;
mod hud;
use actors::{build_mob_gpu, build_player_gpu};
use hud::build_hud_layers;

pub async fn new_renderer_from_target(
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
) -> Renderer {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let surface = instance.create_surface(target).expect("create surface");
    let adapter = request_adapter(&instance, Some(&surface)).await;
    let (device, queue) = request_device(&adapter).await;
    let config = surface
        .get_default_config(&adapter, width, height)
        .expect("surface config");
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
/// first, then the forced fallback (software) one rather than panicking.
/// `surface` is `None` for a surfaceless renderer, which constrains nothing.
pub(super) async fn request_adapter(
    instance: &wgpu::Instance,
    surface: Option<&wgpu::Surface<'static>>,
) -> wgpu::Adapter {
    match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: surface,
            force_fallback_adapter: false,
        })
        .await
    {
        Ok(a) => a,
        Err(_) => {
            eprintln!("wgpu: primary adapter unavailable; trying fallback");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: surface,
                    force_fallback_adapter: true,
                })
                .await
                .expect("no compatible wgpu adapter available")
        }
    }
}

/// The device every renderer needs. The terrain tile array holds every tile
/// PLUS its dye-base twin (2 × tile count layers), which exceeds the default
/// 256-layer limit — request what the tile array actually needs, capped to what
/// the adapter offers, so an adapter that can't fit it fails `create_texture`
/// with a clear count instead of silently truncating.
pub(super) async fn request_device(adapter: &wgpu::Adapter) -> (wgpu::Device, wgpu::Queue) {
    let mut required_limits = wgpu::Limits::default().using_alignment(adapter.limits());
    required_limits.max_texture_array_layers = (2 * petramond_world::tile::Tile::count() as u32)
        .max(required_limits.max_texture_array_layers)
        .min(adapter.limits().max_texture_array_layers);
    // Adapter-specific format features expose supported 8x MSAA; timestamps
    // remain opt-in for the GPU-timing instrument.
    let mut required_features =
        adapter.features() & wgpu::Features::TEXTURE_ADAPTER_SPECIFIC_FORMAT_FEATURES;
    if super::super::gpu_timer::GpuTimer::wanted() {
        required_features |= adapter.features() & wgpu::Features::TIMESTAMP_QUERY;
    }
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
        .expect("device")
}

/// Build every pipeline/atlas/model resource and assemble the `Renderer`.
/// `config` carries the frame geometry + colour format; `surface` is `None`
/// for a surfaceless renderer, which changes nothing else.
pub(super) fn new_renderer_inner(
    surface: Option<wgpu::Surface<'static>>,
    device: wgpu::Device,
    queue: wgpu::Queue,
    config: wgpu::SurfaceConfiguration,
    max_samples: u32,
) -> Renderer {
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
    let (scene_w, scene_h) = (width * sample_axis, height * sample_axis);
    let format = config.format;

    let (_atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let (_atlas_array_texture, atlas_array_view, atlas_array_sampler) =
        create_atlas_array(&device, &queue);
    // Overridden by `set_render_distance` at host wiring; the default keeps the
    // icon-atlas bake (which reads this buffer) fog-free at any distance.
    let default_fog = crate::uniforms::fog_range(petramond::world::RENDER_DIST);
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [default_fog.0, default_fog.1, 0.0, 0.0],
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            render_origin: [0; 4],
            atlas_layout: crate::atlas::atlas_layout_uniform(),
            // White sky colour at init = identity; the icon-atlas bake reads
            // this buffer, so baked UI icons stay untinted.
            sky_color: [1.0, 1.0, 1.0, 0.0],
            // Late-morning sun at full daylight until the sim writes petramond:time.
            sun_dir: super::frame_state::sun_uniform(None),
            volume_tint: [1.0, 1.0, 1.0, 0.0],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let shader_params_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("shader params"),
        contents: bytemuck::cast_slice(&[super::super::uniforms::ShaderParams {
            values: [[0.0; 4]; super::super::uniforms::SHADER_PARAM_SLOTS],
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
    let depth = crate::resources::create_depth_sampled(
        &device,
        scene_w,
        scene_h,
        anti_aliasing.sample_count(),
    );
    let multisample_color = super::post_process::create_multisample_color(
        &device,
        scene_w,
        scene_h,
        format,
        anti_aliasing.sample_count(),
    );
    let scene_color = create_scene_color(&device, scene_w, scene_h, format);
    let post_process_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("post-process controls"),
        contents: bytemuck::cast_slice(&[0.0_f32, 0.0, sample_axis as f32, 1.0]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let grade_bind = super::super::pipeline::create_grade_bind(
        &device,
        &pipelines.grade_bgl,
        &scene_color,
        &post_process_buf,
    );
    // Half-res environment targets: env passes march at half the scene dims
    // against a downsampled depth; the composite lifts the result back (see
    // pipeline::EnvScaler).
    let (env_w, env_h) = (scene_w.div_ceil(2), scene_h.div_ceil(2));
    let env_color = create_scene_color(&device, env_w, env_h, format);
    let env_depth = super::super::resources::create_depth(&device, env_w, env_h);
    let env_down_bind = super::super::pipeline::create_env_down_bind(
        &device,
        &pipelines
            .env_scaler
            .get(anti_aliasing.sample_count())
            .down_bgl,
        &depth,
    );
    let env_comp_bind = super::super::pipeline::create_env_comp_bind(
        &device,
        &pipelines
            .env_scaler
            .get(anti_aliasing.sample_count())
            .comp_bgl,
        &env_color,
        &pipelines.env_scaler.get(anti_aliasing.sample_count()).samp,
        &env_depth,
        &depth,
    );
    let env_passes = pipelines
        .env_passes
        .into_iter()
        .map(|res| {
            let bind = super::super::pipeline::create_environment_bind(
                &device,
                &res.bgl,
                &uniform_buf,
                &res.params_buf,
                &env_depth,
            );
            super::EnvPass {
                res,
                bind,
                dormant: false,
            }
        })
        .collect();

    // Item entities + chests draw through the EXISTING opaque pipeline; clone its
    // (Arc-backed) handle so each `DynamicDraw` issues a byte-identical draw while
    // Terrain `opaque_pipe` is quantized; dynamic bakes need absolute Vertex.
    let item_entity_pipe = pipelines.dynamic_opaque_pipe.clone();
    let chest_pipe = pipelines.dynamic_opaque_pipe.clone();
    let door_pipe = pipelines.dynamic_opaque_pipe.clone();

    let mob_gpu = build_mob_gpu(&device, &queue, &pipelines.atlas_bgl, &pipelines.mob_pipe);
    let player_gpu = build_player_gpu(&device, &queue, &pipelines.atlas_bgl, &pipelines.mob_pipe);
    let player_item_draw = DynamicDraw::new(&device, pipelines.mob_pipe.clone(), "player item");
    let player_model_item_draw =
        DynamicDraw::new(&device, pipelines.mob_pipe.clone(), "player model item");
    let player_block_item_draw = DynamicDraw::new(
        &device,
        pipelines.dynamic_opaque_pipe.clone(),
        "player block item",
    );

    // bbmodel-block ("model") render resources: the combined model atlas (all kinds'
    // textures packed into one sheet — see `block_model::atlas`) uploaded as its own GPU
    // texture, bound at group(1) over the same atlas layout the mob pass uses, and the
    // mob pipeline reused for the model pass (the chunk's `ModelVertex` stream shares the
    // mob `ItemVertex` layout). The mesher bakes geometry into each chunk's model stream;
    // this pass just draws it with full-block lighting already baked in.
    let model_atlas = petramond_world::block_model::atlas();
    let (matlas_rgba, matlas_w, matlas_h) = model_atlas.texture();
    let (_model_atlas_texture, model_atlas_view, model_atlas_sampler) =
        create_model_texture(&device, &queue, matlas_rgba, matlas_w, matlas_h);
    let model_atlas_bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("model atlas bg"),
        layout: &pipelines.atlas_bgl,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: wgpu::BindingResource::TextureView(&model_atlas_view),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: wgpu::BindingResource::Sampler(&model_atlas_sampler),
            },
        ],
    });
    let model_pipe = pipelines.mob_pipe.clone();
    let world_model_pipe = pipelines.world_model_pipe.clone();
    let world_model_blend_pipe = pipelines.world_model_blend_pipe.clone();
    let contact_pipe = pipelines.contact_pipe.clone();
    // A custom-shape block's inventory icon is its baked ITEM geometry (a chair,
    // not a plank cube), which comes from the pack's WASM — bake all installed
    // custom item shapes into the item cache NOW, before the icon atlas reads it.
    petramond::modding::client::bake_installed_custom_item_geometry();

    // Bake every item's inventory icon into the icon atlas ONCE, here at init: the
    // cube/sprite icons through the depthless `model3d_pipe` and the bbmodel-block
    // icons through the depth-tested `model_icon_pipe` (these two pipelines are used
    // only by this bake now — see `icon_atlas`). The atlas color format MUST match
    // the surface (sRGB) so sampling/store cancel like the gui atlas (no double
    // gamma). The per-slot UI pass then draws a textured quad sampling this.
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
    // Reusable dynamic vbuf for the per-frame icon quads (6 UiVertex per filled
    // slot), grown to fit.
    let icon_quad_vbuf =
        super::dynamic_draw::new_buffer(&device, wgpu::BufferUsages::VERTEX, "icon quad vbuf");

    let hud_layers = build_hud_layers(&device, &queue, &pipelines.atlas_bgl);

    let gpu_timer = super::super::gpu_timer::GpuTimer::new(&device, &queue);
    let column_origins = super::super::resources::ColumnOrigins::new(&device);
    let quad_index = super::super::resources::QuadIndexBuffer::new(&device, &queue);

    // Every dynamic draw owns growing buffers; built here, before the
    // renderer takes the device.
    let item_entity_draw = DynamicDraw::new(&device, item_entity_pipe, "item entity");
    let item_model_entity_draw = DynamicDraw::new(&device, model_pipe.clone(), "item model entity");
    let item_sprite_entity_draw =
        DynamicDraw::new(&device, pipelines.mob_pipe.clone(), "item sprite entity");
    let chest_draw = DynamicDraw::new(&device, chest_pipe, "chest");
    let door_draw = DynamicDraw::new(&device, door_pipe, "door");
    let break_draw = DynamicDraw::new(&device, pipelines.break_pipe, "break overlay");
    let emitter_particle_draw = DynamicVertexDraw::new(
        &device,
        pipelines.emitter_particle_pipe,
        "emitter particle",
        crate::particles::VERTS_PER_CUBE as u32,
        &crate::particles::CUBE_INDEX_PATTERN,
    );
    let particle_draw = DynamicVertexDraw::new(
        &device,
        pipelines.particle_pipe,
        "particle",
        4,
        &[0, 1, 2, 0, 2, 3],
    );
    let entity_shadow_draw = DynamicVertexDraw::new(
        &device,
        pipelines.entity_shadow_pipe,
        "entity shadow",
        crate::entity_shadow::VERTS_PER_SHADOW,
        &crate::entity_shadow::QUAD_INDEX_PATTERN,
    );

    Renderer {
        surface,
        device,
        queue,
        config,
        gpu_timer,
        offscreen_target: None,
        suboptimal_retried: false,
        opaque_pipe: pipelines.opaque_pipe,
        translucent_pipe: pipelines.translucent_pipe,
        transparent_pipe: pipelines.transparent_pipe,
        transparent_two_sided_pipe: pipelines.transparent_two_sided_pipe,
        uniform_buf,
        shader_params_buf,
        uniform_bind: pipelines.uniform_bind,
        atlas_bind: pipelines.atlas_bind,
        atlas_array_bind: pipelines.atlas_array_bind,
        model_pipe,
        world_model_pipe,
        world_model_blend_pipe,
        contact_pipe,
        model_atlas_bind,
        item_entity: ItemEntityPass {
            block_draws: Vec::new(),
            block_draws_visible: Vec::new(),
            draw: item_entity_draw,
            // Dropped bbmodel items ride the model pipeline (world-space
            // ItemVertex, model atlas) in their own stream.
            model_draw: item_model_entity_draw,
            model_verts: Vec::new(),
            model_indices: Vec::new(),
            // Dropped SPRITE items extruded into pixel-perfect 3D slabs ride
            // the same mob-layout pipeline over the 2D BLOCK atlas (their side
            // walls sample single boundary texels) in their own stream.
            sprite_draw: item_sprite_entity_draw,
            sprite_verts: Vec::new(),
            sprite_indices: Vec::new(),
            sprite_scratch: Vec::new(),
            instances: Vec::new(),
            verts: Vec::new(),
            indices: Vec::new(),
            visible: Vec::new(),
        },
        actor: ActorPass {
            mob_gpu,
            player_gpu,
            item_draw: player_item_draw,
            model_item_draw: player_model_item_draw,
            block_item_draw: player_block_item_draw,
            player_view: None,
            remote_players: Vec::new(),
            bone_offsets: Vec::new(),
            player_visible: Vec::new(),
            body_verts: Vec::new(),
            body_indices: Vec::new(),
            item_verts: Vec::new(),
            item_indices: Vec::new(),
            sprite_verts: Vec::new(),
            model_item_verts: Vec::new(),
            model_item_indices: Vec::new(),
            mobs: Vec::new(),
        },
        block_entity: BlockEntityPass {
            chest_draw,
            door_draw,
            chests: Vec::new(),
            chest_visible: Vec::new(),
            doors: Vec::new(),
            door_visible: Vec::new(),
        },
        hand: HandPass {
            model3d_pipe: pipelines.model3d_hand_pipe,
            model3d_mvp_buf: pipelines.model3d_mvp_buf,
            model3d_mvp_bind: pipelines.model3d_mvp_bind,
            model3d_vbuf: pipelines.model3d_vbuf,
            model3d_ibuf: pipelines.model3d_ibuf,
            item3d_pipe: pipelines.item3d_pipe,
            item3d_mvp_bind: pipelines.item3d_mvp_bind,
            item3d_vbuf: pipelines.item3d_vbuf,
            item3d_verts: Vec::new(),
            item3d_vertex_count: 0,
            held_is_model: false,
            index_count: 0,
            verts: Vec::new(),
            indices: Vec::new(),
            off_verts: Vec::new(),
            off_indices: Vec::new(),
            model_scratch_verts: Vec::new(),
            model_scratch_indices: Vec::new(),
            off_item3d_scratch: Vec::new(),
            break_draw,
            break_overlays: Vec::new(),
            held_item: HeldItemView::default(),
            visible: false,
            shake: [0.0, 0.0],
            held_item_anim: HeldItemAnimator::default(),
            held_item_skylight: crate::lighting::FULL_SKYLIGHT,
            held_item_blocklight: petramond_world::light::BlockLight6::DARK,
            vertex_count: 0,
            off_item: HeldItemView::default(),
            off_item_anim: HeldItemAnimator::default(),
            off_index_count: 0,
            off_item3d_start: 0,
            off_item3d_count: 0,
            off_is_model: false,
        },
        ui: UiPass {
            viewport_generation: 1,
            prepared_viewport: UiViewport::default(),
            pipe: pipelines.ui_pipe,
            texture_bgl: pipelines.atlas_bgl.clone(),
            doc_ui: super::doc_ui::DocUi::default(),
            client_overlays: super::client_overlay::ClientOverlays::default(),
            solid_vbuf: pipelines.ui_vbuf,
            solid_verts: Vec::new(),
            count_vertex_count: 0,
            overlay_count_vertex_count: 0,
            drag_count_vertex_count: 0,
            hud_layers,
            icon_atlas,
            icon_quad_vbuf,
            icon_quad_verts: Vec::new(),
            icon_quad_vertex_count: 0,
            overlay_icon_quad_vertex_count: 0,
            drag_icon_quad_vertex_count: 0,
            build: UiBuild::default(),
        },
        sky: SkyPass {
            pipe: pipelines.sky_pipe,
            bind: pipelines.sky_bind,
            texture_bind: pipelines.sky_texture_bind,
            shader_param_keys: pipelines.sky_shader_param_keys,
            env_passes,
            env_scaler: pipelines.env_scaler,
            env_color,
            env_depth,
            env_down_bind,
            env_comp_bind,
            light_param_key: pipelines.sky_light_param_key,
            fog_start: default_fog.0,
            fog_end: default_fog.1,
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            clear_color: [0.60, 0.82, 1.00],
        },
        chrome: ChromePass {
            outline_pipe: pipelines.outline_pipe,
            outline_bind: pipelines.outline_bind,
            outline_vbuf: pipelines.outline_vbuf,
            outline_vertex_count: 0,
            crosshair_pipe: pipelines.crosshair_pipe,
            crosshair_vbuf: pipelines.crosshair_vbuf,
            crosshair_vertex_count: 0,
            crosshair_drawn_size: (0, 0),
            crosshair_visible: false,
            selection: None,
            selection_drawn: None,
        },
        targets: SceneTargets {
            render_scale: 1.0,
            grade_enabled: true,
            anti_aliasing,
            scene_color,
            multisample_color,
            max_samples,
            grade_pipe: pipelines.grade_pipe,
            grade_bgl: pipelines.grade_bgl,
            grade_bind,
            post_process_buf,
            mood: [0.0, 0.0],
            depth,
        },
        view: ViewState {
            frustum: Frustum::permissive(),
            cam_pos: petramond_math::world_pos::WorldPos::ZERO,
            render_origin: glam::IVec3::ZERO,
            visual_time: 0.0,
            proj_y_scale: 1.0,
        },
        terrain: TerrainPass {
            columns: HashMap::new(),
            column_origins,
            geometry: super::super::geometry_arena::GeometryArena::new(),
            quad_index,
            upload_pending: HashMap::new(),
            upload_heap: BinaryHeap::new(),
            upload_frame: 0,
            upload_scratch: ColumnUploadScratch::default(),
            draw_order: Vec::new(),
            opaque_column_order: Vec::new(),
            model_column_order: Vec::new(),
            contact_column_order: Vec::new(),
            gpu_revision: 0,
            planned_gpu_revision: u64::MAX,
            view_key: TerrainViewKey {
                view_proj: [0; 16],
                cam: [0; 3],
                fog: 0,
            },
            planned_view_key: None,
            plan_any_model: false,
            plan_any_transparent: false,
            far_leaf_lod_state: HashMap::new(),
        },
        particle: ParticlePass {
            emitter_draw: emitter_particle_draw,
            draw: particle_draw,
            instances: Vec::new(),
            model_instances: Vec::new(),
            solid_instances: Vec::new(),
            emitters: Vec::new(),
            density: 1.0,
            block_vertex_count: 0,
            verts: Vec::new(),
            emitter_verts: Vec::new(),
            emitter_scratch: Vec::new(),
        },
        shadow: ShadowPass {
            draw: entity_shadow_draw,
            verts: Vec::new(),
            instances: Vec::new(),
        },
        last_stats: RenderStats::default(),
    }
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
