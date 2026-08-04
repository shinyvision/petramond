//! Renderer construction + surface lifecycle.
//!
//! Owns wgpu instance/adapter/device/surface bring-up, per-species + model
//! atlas resources, the icon-atlas bake, the big `Renderer { .. }` initializer,
//! and `screen_size` / `resize`. Split out of the renderer god-file; behavior is
//! byte-for-byte identical. The `new_renderer_from_target` / `instance_descriptor`
//! external paths are preserved via re-exports in the parent module.

use super::*;

pub(crate) async fn new_renderer_from_target(
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
    new_renderer_inner(Some(surface), device, queue, config)
}

/// Instance descriptor selecting native backends (Vulkan/Metal/DX12/GL).
///
/// Honors `WGPU_BACKEND` (`vulkan` | `gl`) to pin a single backend; unset = all.
/// This matters on a hybrid-GPU Wayland session: the discrete NVIDIA GPU's Vulkan
/// WSI can't present to a Wayland surface it isn't driving (it reports
/// `VK_KHR_wayland_surface` present = false), so wgpu's surface-compatible pick
/// falls back to the Intel iGPU. Its EGL/GLES path *can* present there, so
/// `WGPU_BACKEND=gl` (with the EGL vendor pointed at NVIDIA) renders on the dGPU.
pub(in crate::render) fn instance_descriptor() -> wgpu::InstanceDescriptor {
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
    required_limits.max_texture_array_layers = (2 * crate::atlas::Tile::count() as u32)
        .max(required_limits.max_texture_array_layers)
        .min(adapter.limits().max_texture_array_layers);
    // Timestamp queries are requested only when the GPU-timing instrument is
    // switched on, so an ordinary run asks for no optional feature at all.
    let mut required_features = wgpu::Features::empty();
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
) -> Renderer {
    let (width, height) = (config.width, config.height);
    let format = config.format;
    let sample_count = 1u32;

    let (_atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let (_atlas_array_texture, atlas_array_view, atlas_array_sampler) =
        create_atlas_array(&device, &queue);
    // Overridden by `set_render_distance` at host wiring; the default keeps the
    // icon-atlas bake (which reads this buffer) fog-free at any distance.
    let default_fog = crate::render::uniforms::fog_range(crate::world::RENDER_DIST);
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [default_fog.0, default_fog.1, 0.0, 0.0],
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            render_origin: [0.0; 4],
            atlas_anim: crate::atlas::atlas_anim_uniform(),
            // White sky colour at init = identity; the icon-atlas bake reads
            // this buffer, so baked UI icons stay untinted.
            sky_color: [1.0, 1.0, 1.0, 0.0],
            // Late-morning sun at full daylight until the sim writes petramond:time.
            sun_dir: super::frame_state::sun_uniform(None),
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
        sample_count,
        &uniform_buf,
        &shader_params_buf,
        &atlas_view,
        &atlas_sampler,
        &atlas_array_view,
        &atlas_array_sampler,
    );
    let depth = create_depth(&device, width, height);
    let scene_color = create_scene_color(&device, width, height, format);
    let mood_buf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("grade mood"),
        size: 16,
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let grade_bind = super::super::pipeline::create_grade_bind(
        &device,
        &pipelines.grade_bgl,
        &scene_color,
        &mood_buf,
    );
    // Half-res environment targets: env passes march at half the scene dims
    // against a downsampled depth; the composite lifts the result back (see
    // pipeline::EnvScaler).
    let (env_w, env_h) = (width.div_ceil(2), height.div_ceil(2));
    let env_color = create_scene_color(&device, env_w, env_h, format);
    let env_depth = super::super::resources::create_depth(&device, env_w, env_h);
    let env_down_bind = super::super::pipeline::create_env_down_bind(
        &device,
        &pipelines.env_scaler.down_bgl,
        &depth,
    );
    let env_comp_bind = super::super::pipeline::create_env_comp_bind(
        &device,
        &pipelines.env_scaler.comp_bgl,
        &env_color,
        &pipelines.env_scaler.samp,
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

    // Build per-species mob render resources by iterating the mob registry: load each
    // species' `.bbmodel` (geometry + walk animation + embedded texture), upload its
    // texture as a dedicated atlas, build its group(1) bind, and give it its own
    // dynamic-draw buffers over the shared mob pipeline. Adding a species is a row in
    // `mobs.json` — no renderer edit. A model parse failure degrades to an empty
    // model (that species just doesn't draw) rather than crashing the renderer.
    // World-space margin added around each species' rest-pose cull bounds.
    const MOB_CULL_SLACK: f32 = 0.5;
    let mob_gpu: Vec<MobGpu> = crate::mob::defs()
        .iter()
        .map(|d| {
            let kind = d.mob;
            // Borrow this species' precached model (compiled once on startup, shared with
            // the simulation — see `crate::mob::model`). The renderer never reads a
            // `.bbmodel`: at runtime the `.llmob` + this in-memory `Model` are golden.
            let model = crate::mob::model(kind);
            let (_texture, view, sampler) = create_model_texture(
                &device,
                &queue,
                &model.texture_rgba,
                model.tex_w,
                model.tex_h,
            );
            let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("mob atlas bg"),
                layout: &pipelines.atlas_bgl,
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
            });
            let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mob vbuf"),
                size: crate::render::pipeline::MAX_MOB_VERTICES
                    * std::mem::size_of::<ItemVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
                label: Some("mob ibuf"),
                size: crate::render::pipeline::MAX_MOB_INDICES * 4,
                usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            });
            // Cull volume from the rest-posed model bounds × render scale. The
            // horizontal radius takes the farthest posed corner (yaw can point
            // it in any direction); MOB_CULL_SLACK absorbs what the rest pose
            // cannot know — walk/idle limb swing, head-look, the interpolation
            // between replicated positions. Conservative slack costs a few
            // early-visible instances; too tight pops mobs at screen edges.
            let (bmin, bmax) = model.rest_bounds();
            let r = [bmin.x, bmax.x]
                .into_iter()
                .flat_map(|x| [bmin.z, bmax.z].map(|z| (x * x + z * z).sqrt()))
                .fold(0.0f32, f32::max);
            MobGpu {
                model,
                scale: d.scale,
                bind,
                draw: DynamicDraw::new(
                    pipelines.mob_pipe.clone(),
                    vbuf,
                    ibuf,
                    crate::render::pipeline::MAX_MOB_VERTICES,
                    crate::render::pipeline::MAX_MOB_INDICES,
                ),
                cull_r: r * d.scale + MOB_CULL_SLACK,
                cull_y0: bmin.y * d.scale - MOB_CULL_SLACK,
                cull_y1: bmax.y * d.scale + MOB_CULL_SLACK,
                visible: Vec::new(),
                verts: Vec::new(),
                indices: Vec::new(),
            }
        })
        .collect();

    // Player bodies: the precached player model gets the same shape of
    // resources as one mob species (own skin texture bind + dynamic draw over
    // the shared mob pipeline), plus three held-item draws attached to the
    // posed hands: an extruded-sprite stream (2D atlas), a bbmodel-item stream
    // (model atlas), and a packed block-vertex stream (held mini-cube on the
    // opaque pipeline). EVERY connected player's body appends
    // into the one stream, so the caps cover a full LAN party of bodies
    // (~300 verts each), not just the local one.
    const PLAYER_ITEM_VERTICES: u64 = 16384;
    const PLAYER_ITEM_INDICES: u64 = 24576;
    let player_gpu = {
        let model = crate::player::model::player_model();
        let (_texture, view, sampler) = create_model_texture(
            &device,
            &queue,
            &model.texture_rgba,
            model.tex_w,
            model.tex_h,
        );
        let bind = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("player atlas bg"),
            layout: &pipelines.atlas_bgl,
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
        });
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player vbuf"),
            size: PLAYER_ITEM_VERTICES * std::mem::size_of::<ItemVertex>() as u64,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("player ibuf"),
            size: PLAYER_ITEM_INDICES * 4,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        PlayerGpu {
            model,
            bind,
            draw: DynamicDraw::new(
                pipelines.mob_pipe.clone(),
                vbuf,
                ibuf,
                PLAYER_ITEM_VERTICES,
                PLAYER_ITEM_INDICES,
            ),
            verts: Vec::new(),
            indices: Vec::new(),
        }
    };
    let player_dyn_buffers = |label: &str, vert_size: u64| {
        let vbuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: PLAYER_ITEM_VERTICES * vert_size,
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let ibuf = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: PLAYER_ITEM_INDICES * 4,
            usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        (vbuf, ibuf)
    };
    let (player_item_vbuf, player_item_ibuf) =
        player_dyn_buffers("player item", std::mem::size_of::<ItemVertex>() as u64);
    let (player_model_item_vbuf, player_model_item_ibuf) = player_dyn_buffers(
        "player model item",
        std::mem::size_of::<ItemVertex>() as u64,
    );
    let (player_block_item_vbuf, player_block_item_ibuf) = player_dyn_buffers(
        "player block item",
        std::mem::size_of::<crate::mesh::Vertex>() as u64,
    );
    let player_item_draw = DynamicDraw::new(
        pipelines.mob_pipe.clone(),
        player_item_vbuf,
        player_item_ibuf,
        PLAYER_ITEM_VERTICES,
        PLAYER_ITEM_INDICES,
    );
    let player_model_item_draw = DynamicDraw::new(
        pipelines.mob_pipe.clone(),
        player_model_item_vbuf,
        player_model_item_ibuf,
        PLAYER_ITEM_VERTICES,
        PLAYER_ITEM_INDICES,
    );
    let player_block_item_draw = DynamicDraw::new(
        pipelines.dynamic_opaque_pipe.clone(),
        player_block_item_vbuf,
        player_block_item_ibuf,
        PLAYER_ITEM_VERTICES,
        PLAYER_ITEM_INDICES,
    );

    // bbmodel-block ("model") render resources: the combined model atlas (all kinds'
    // textures packed into one sheet — see `block_model::atlas`) uploaded as its own GPU
    // texture, bound at group(1) over the same atlas layout the mob pass uses, and the
    // mob pipeline reused for the model pass (the chunk's `ModelVertex` stream shares the
    // mob `ItemVertex` layout). The mesher bakes geometry into each chunk's model stream;
    // this pass just draws it with full-block lighting already baked in.
    let model_atlas = crate::block_model::atlas();
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
    // Dropped bbmodel item-entities ride the model pipeline (world-space ItemVertex,
    // model atlas) in their OWN buffers, sized like the packed item-entity buffers.
    let item_model_entity_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item model entity vbuf"),
        size: crate::render::pipeline::MAX_MOB_VERTICES * std::mem::size_of::<ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let item_model_entity_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item model entity ibuf"),
        size: crate::render::pipeline::MAX_MOB_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Dropped SPRITE item-entities extruded into pixel-perfect 3D slabs ride the
    // same mob-layout pipeline over the 2D BLOCK atlas (their side walls sample
    // single boundary texels) in their own buffers, sized like the packed
    // item-entity buffers.
    let item_sprite_entity_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item sprite entity vbuf"),
        size: crate::render::pipeline::MAX_ITEM_ENTITY_VERTICES
            * std::mem::size_of::<ItemVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let item_sprite_entity_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item sprite entity ibuf"),
        size: crate::render::pipeline::MAX_ITEM_ENTITY_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // A custom-shape block's inventory icon is its baked ITEM geometry (a chair,
    // not a plank cube), which comes from the pack's WASM — bake all installed
    // custom item shapes into the item cache NOW, before the icon atlas reads it.
    crate::modding::client::bake_installed_custom_item_geometry();

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
    // slot). Sized for the open inventory + craft/chest slots with headroom; grown
    // to fit if ever exceeded (never a hard cap that drops the batch).
    let icon_quad_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("icon quad vbuf"),
        size: crate::render::pipeline::MAX_UI_VERTICES * std::mem::size_of::<UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // HUD heart atlas (empty | half | full, side by side). One texture for the whole
    // health bar; the UI pass selects a cell per heart by UV. Resolved through the
    // asset overlay (a pack can reskin it) into its own bind group (reusing the
    // gui-atlas bind layout).
    let load_gui_bind = |rel: &str| -> Option<wgpu::BindGroup> {
        let (bytes, _path) = crate::assets::read_bytes(rel)?;
        let (_tex, view, sampler) = create_gui_panel(&device, &queue, &bytes);
        Some(device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("gui texture bind"),
            layout: &pipelines.atlas_bgl,
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
        }))
    };
    // The HUD chrome layers, in draw order. Adding a HUD element = one
    // `UiBuild` vec + one entry here (see `HudLayer`).
    let hud_layer = |label: &'static str,
                     source: fn(&crate::render::ui::UiBuild) -> &[UiVertex],
                     texture: super::HudLayerTexture,
                     under_chrome: bool| {
        super::HudLayer {
            source,
            texture,
            under_chrome,
            vbuf: device.create_buffer(&wgpu::BufferDescriptor {
                label: Some(label),
                size: crate::render::pipeline::MAX_UI_VERTICES
                    * std::mem::size_of::<UiVertex>() as u64,
                usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
                mapped_at_creation: false,
            }),
            vertex_count: 0,
        }
    };
    // Status-effect icon strip: composed on the CPU from the shared frame +
    // each registered effect's icon (engine and pack rows alike), uploaded
    // once — the HUD indexes cells by effect id like hearts index their atlas.
    let effects_bind = crate::render::effect_icons::compose_atlas().map(|img| {
        let (_tex, view, sampler) =
            crate::render::resources::create_rgba_nearest(&device, &queue, &img, "effect icons");
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("effect icons bind"),
            layout: &pipelines.atlas_bgl,
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
    });
    let hud_layers = vec![
        // Hurt-flash red edge vignette, under all chrome (solid gradient quads).
        hud_layer(
            "hud vignette",
            |b| &b.vignette,
            super::HudLayerTexture::Solid,
            true,
        ),
        // HUD hearts (bottom-left health bar), from the heart atlas.
        hud_layer(
            "hud hearts",
            |b| &b.hearts,
            super::HudLayerTexture::Texture(load_gui_bind("textures/gui/hearts.png")),
            false,
        ),
        // Status-effect icons (framed row above the hearts), from the strip.
        hud_layer(
            "hud effects",
            |b| &b.effects,
            super::HudLayerTexture::Texture(effects_bind),
            false,
        ),
    ];

    let gpu_timer = super::super::gpu_timer::GpuTimer::new(&device, &queue);
    let column_origins = super::super::resources::ColumnOrigins::new(&device);
    let quad_index = super::super::resources::QuadIndexBuffer::new(&device, &queue);

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
        model_pipe: model_pipe.clone(),
        world_model_pipe,
        world_model_blend_pipe,
        contact_pipe,
        model_atlas_bind,
        item_entity: ItemEntityPass {
            block_draws: Vec::new(),
            block_draws_visible: Vec::new(),
            draw: DynamicDraw::new(
                item_entity_pipe,
                pipelines.item_entity_vbuf,
                pipelines.item_entity_ibuf,
                crate::render::pipeline::MAX_ITEM_ENTITY_VERTICES,
                crate::render::pipeline::MAX_ITEM_ENTITY_INDICES,
            ),
            model_draw: DynamicDraw::new(
                model_pipe,
                item_model_entity_vbuf,
                item_model_entity_ibuf,
                crate::render::pipeline::MAX_MOB_VERTICES,
                crate::render::pipeline::MAX_MOB_INDICES,
            ),
            model_verts: Vec::new(),
            model_indices: Vec::new(),
            sprite_draw: DynamicDraw::new(
                pipelines.mob_pipe.clone(),
                item_sprite_entity_vbuf,
                item_sprite_entity_ibuf,
                crate::render::pipeline::MAX_ITEM_ENTITY_VERTICES,
                crate::render::pipeline::MAX_ITEM_ENTITY_INDICES,
            ),
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
            chest_draw: DynamicDraw::new(
                chest_pipe,
                pipelines.chest_vbuf,
                pipelines.chest_ibuf,
                crate::render::pipeline::MAX_CHEST_VERTICES,
                crate::render::pipeline::MAX_CHEST_INDICES,
            ),
            door_draw: DynamicDraw::new(
                door_pipe,
                pipelines.door_vbuf,
                pipelines.door_ibuf,
                crate::render::pipeline::MAX_DOOR_VERTICES,
                crate::render::pipeline::MAX_DOOR_INDICES,
            ),
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
            break_draw: DynamicDraw::new(
                pipelines.break_pipe,
                pipelines.break_vbuf,
                pipelines.break_ibuf,
                crate::render::pipeline::MAX_BREAK_VERTICES,
                crate::render::pipeline::MAX_BREAK_INDICES,
            ),
            break_overlays: Vec::new(),
            held_item: HeldItemView::default(),
            visible: false,
            shake: [0.0, 0.0],
            held_item_anim: HeldItemAnimator::default(),
            held_item_skylight: crate::render::lighting::FULL_SKYLIGHT,
            held_item_blocklight: crate::light::BlockLight6::DARK,
            vertex_count: 0,
        },
        ui: UiPass {
            viewport_generation: 1,
            prepared_viewport: UiViewport::default(),
            pipe: pipelines.ui_pipe,
            texture_bgl: pipelines.atlas_bgl.clone(),
            doc_ui: super::doc_ui::DocUi::default(),
            client_overlays: super::client_overlay::ClientOverlays::default(),
            solid_vbuf: pipelines.ui_vbuf,
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
            underwater: false,
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
            scene_color,
            grade_pipe: pipelines.grade_pipe,
            grade_bgl: pipelines.grade_bgl,
            grade_bind,
            mood_buf,
            mood: [0.0, 0.0],
            depth,
        },
        view: ViewState {
            frustum: Frustum::permissive(),
            cam_pos: glam::Vec3::ZERO,
            render_origin: glam::Vec3::ZERO,
            visual_time: 0.0,
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
            emitter_draw: DynamicVertexDraw::new(
                pipelines.emitter_particle_pipe,
                pipelines.emitter_particle_vbuf,
                pipelines.particle_ibuf.clone(),
                crate::render::particles::MAX_PARTICLE_VERTICES as u64,
            ),
            draw: DynamicVertexDraw::new(
                pipelines.particle_pipe,
                pipelines.particle_vbuf,
                pipelines.particle_ibuf,
                crate::render::particles::MAX_PARTICLE_VERTICES as u64,
            ),
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

    pub(crate) fn ui_viewport(&self) -> UiViewport {
        UiViewport::new(self.screen_size(), self.ui.viewport_generation)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.ui.viewport_generation = self.ui.viewport_generation.wrapping_add(1).max(1);
        if let Some(surface) = &self.surface {
            surface.configure(&self.device, &self.config);
        }
        self.recreate_scene_targets();
        self.chrome.crosshair_drawn_size = (0, 0);
        // A real size change earns a fresh suboptimal-retry (render()).
        self.suboptimal_retried = false;
    }

    /// (Re)build the world-pass targets at the current `render_scale` (and the
    /// grade bind that reads them). Called on resize and scale changes.
    pub(super) fn recreate_scene_targets(&mut self) {
        let (w, h) = self.scene_dims();
        self.targets.depth = create_depth(&self.device, w, h);
        self.targets.scene_color = create_scene_color(&self.device, w, h, self.config.format);
        self.targets.grade_bind = super::super::pipeline::create_grade_bind(
            &self.device,
            &self.targets.grade_bgl,
            &self.targets.scene_color,
            &self.targets.mood_buf,
        );
        // Environment half-res targets and every bind that references the
        // recreated views.
        let (env_w, env_h) = (w.div_ceil(2), h.div_ceil(2));
        self.sky.env_color = create_scene_color(&self.device, env_w, env_h, self.config.format);
        self.sky.env_depth = super::super::resources::create_depth(&self.device, env_w, env_h);
        self.sky.env_down_bind = super::super::pipeline::create_env_down_bind(
            &self.device,
            &self.sky.env_scaler.down_bgl,
            &self.targets.depth,
        );
        self.sky.env_comp_bind = super::super::pipeline::create_env_comp_bind(
            &self.device,
            &self.sky.env_scaler.comp_bgl,
            &self.sky.env_color,
            &self.sky.env_scaler.samp,
            &self.sky.env_depth,
            &self.targets.depth,
        );
        for pass in &mut self.sky.env_passes {
            pass.bind = super::super::pipeline::create_environment_bind(
                &self.device,
                &pass.res.bgl,
                &self.uniform_buf,
                &pass.res.params_buf,
                &self.sky.env_depth,
            );
        }
    }
}
