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
    new_renderer_inner(instance, surface, width, height).await
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

async fn new_renderer_inner(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    // Try a high-performance adapter first. If it fails entirely (no adapter
    // compatible with the surface), retry with force_fallback_adapter to accept
    // the software/lowest-tier adapter rather than panicking.
    let adapter = match instance
        .request_adapter(&wgpu::RequestAdapterOptions {
            power_preference: wgpu::PowerPreference::HighPerformance,
            compatible_surface: Some(&surface),
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
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: true,
                })
                .await
                .expect("no compatible wgpu adapter available")
        }
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: wgpu::Limits::default().using_alignment(adapter.limits()),
            experimental_features: wgpu::ExperimentalFeatures::disabled(),
            memory_hints: wgpu::MemoryHints::Performance,
            trace: wgpu::Trace::Off,
        })
        .await
        .expect("device");

    let config = surface
        .get_default_config(&adapter, width, height)
        .expect("surface config");
    let format = config.format;
    let sample_count = 1u32;
    surface.configure(&device, &config);

    let (_atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let (_atlas_array_texture, atlas_array_view, atlas_array_sampler) =
        create_atlas_array(&device, &queue);
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [FOG_START, FOG_END, 0.0, 0.0],
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            render_origin: [0.0; 4],
            water_anim: crate::atlas::water_anim_uniform(),
            // White sky colour at init = identity; the icon-atlas bake reads
            // this buffer, so baked UI icons stay untinted.
            sky_color: [1.0, 1.0, 1.0, 0.0],
            // Late-morning sun at full daylight until the sim writes llama:time.
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
    let grade_bind =
        super::super::pipeline::create_grade_bind(&device, &pipelines.grade_bgl, &scene_color);

    // Item entities + chests draw through the EXISTING opaque pipeline; clone its
    // (Arc-backed) handle so each `DynamicDraw` issues a byte-identical draw while
    // the `opaque_pipe` field below still owns the original.
    let item_entity_pipe = pipelines.opaque_pipe.clone();
    let chest_pipe = pipelines.opaque_pipe.clone();
    let door_pipe = pipelines.opaque_pipe.clone();

    // Build per-species mob render resources by iterating the mob registry: load each
    // species' `.bbmodel` (geometry + walk animation + embedded texture), upload its
    // texture as a dedicated atlas, build its group(1) bind, and give it its own
    // dynamic-draw buffers over the shared mob pipeline. Adding a species is a row in
    // `mobs.json` — no renderer edit. A model parse failure degrades to an empty
    // model (that species just doesn't draw) rather than crashing the renderer.
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
                visible: Vec::new(),
                verts: Vec::new(),
                indices: Vec::new(),
            }
        })
        .collect();

    // Third-person player body: the precached player model gets the same shape of
    // resources as one mob species (own skin texture bind + dynamic draw over the
    // shared mob pipeline), plus two small held-item draws attached to its hand:
    // an explicit-UV stream (extruded sprite / bbmodel item) and a packed
    // block-vertex stream (held block mini-cube on the opaque pipeline).
    const PLAYER_ITEM_VERTICES: u64 = 8192;
    const PLAYER_ITEM_INDICES: u64 = 12288;
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
    let player_block_item_draw = DynamicDraw::new(
        pipelines.opaque_pipe.clone(),
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
    let hearts_bind = load_gui_bind("textures/gui/hearts.png");
    let ui_hearts_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui hearts vbuf"),
        size: crate::render::pipeline::MAX_UI_VERTICES * std::mem::size_of::<UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
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
    let ui_effects_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui effects vbuf"),
        size: crate::render::pipeline::MAX_UI_VERTICES * std::mem::size_of::<UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    // Hurt vignette: four gradient bands = 24 vertices; a small fixed buffer.
    let ui_vignette_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui vignette vbuf"),
        size: 24 * std::mem::size_of::<UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    Renderer {
        surface,
        device,
        queue,
        config,
        sky_pipe: pipelines.sky_pipe,
        sky_bind: pipelines.sky_bind,
        sky_texture_bind: pipelines.sky_texture_bind,
        sky_shader_param_keys: pipelines.sky_shader_param_keys,
        sky_light_param_key: pipelines.sky_light_param_key,
        underwater: false,
        sky_scale: 1.0,
        sky_color: [1.0, 1.0, 1.0],
        opaque_pipe: pipelines.opaque_pipe,
        transparent_pipe: pipelines.transparent_pipe,
        scene_color,
        grade_pipe: pipelines.grade_pipe,
        grade_bgl: pipelines.grade_bgl,
        grade_bind,
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
        uniform_buf,
        shader_params_buf,
        uniform_bind: pipelines.uniform_bind,
        atlas_bind: pipelines.atlas_bind,
        atlas_array_bind: pipelines.atlas_array_bind,
        model3d_hand_pipe: pipelines.model3d_hand_pipe,
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
        hand_index_count: 0,
        hand_verts: Vec::new(),
        hand_indices: Vec::new(),
        break_draw: DynamicDraw::new(
            pipelines.break_pipe,
            pipelines.break_vbuf,
            pipelines.break_ibuf,
            crate::render::pipeline::MAX_BREAK_VERTICES,
            crate::render::pipeline::MAX_BREAK_INDICES,
        ),
        item_entity_draw: DynamicDraw::new(
            item_entity_pipe,
            pipelines.item_entity_vbuf,
            pipelines.item_entity_ibuf,
            crate::render::pipeline::MAX_ITEM_ENTITY_VERTICES,
            crate::render::pipeline::MAX_ITEM_ENTITY_INDICES,
        ),
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
        mob_gpu,
        player_gpu,
        player_view: None,
        player_item_draw,
        player_item_is_model: false,
        player_item_verts: Vec::new(),
        player_item_indices: Vec::new(),
        player_block_item_draw,
        model_pipe: model_pipe.clone(),
        world_model_pipe,
        model_atlas_bind,
        item_model_entity_draw: DynamicDraw::new(
            model_pipe,
            item_model_entity_vbuf,
            item_model_entity_ibuf,
            crate::render::pipeline::MAX_MOB_VERTICES,
            crate::render::pipeline::MAX_MOB_INDICES,
        ),
        item_model_entity_verts: Vec::new(),
        item_model_entity_indices: Vec::new(),
        emitter_particle_draw: DynamicVertexDraw::new(
            pipelines.emitter_particle_pipe,
            pipelines.emitter_particle_vbuf,
            pipelines.particle_ibuf.clone(),
            crate::render::particles::MAX_PARTICLE_VERTICES as u64,
        ),
        particle_draw: DynamicVertexDraw::new(
            pipelines.particle_pipe,
            pipelines.particle_vbuf,
            pipelines.particle_ibuf,
            crate::render::particles::MAX_PARTICLE_VERTICES as u64,
        ),
        depth,
        terrain_columns: HashMap::new(),
        terrain_upload_order: Vec::new(),
        terrain_upload_scratch: ColumnUploadScratch::default(),
        draw_order: Vec::new(),
        opaque_column_order: Vec::new(),
        model_column_order: Vec::new(),
        frustum: Frustum::permissive(),
        cam_pos: glam::Vec3::ZERO,
        render_origin: glam::Vec3::ZERO,
        visual_time: 0.0,
        far_leaf_lod_state: HashMap::new(),
        clear_color: [0.60, 0.82, 1.00],
        last_stats: RenderStats::default(),
        break_overlay: None,
        held_item: HeldItemView::default(),
        hand_visible: false,
        hand_shake: [0.0, 0.0],
        held_item_anim: HeldItemAnimator::default(),
        held_item_skylight: crate::render::lighting::FULL_SKYLIGHT,
        held_item_blocklight: 0,
        held_item_warm: 0,
        item_entities: Vec::new(),
        particles: Vec::new(),
        model_particles: Vec::new(),
        particle_emitters: Vec::new(),
        particle_emitter_visible: Vec::new(),
        particle_block_vertex_count: 0,
        ui: UiSnapshot::default(),
        billboard_basis: BillboardBasis {
            right: glam::Vec3::X,
            up: glam::Vec3::Y,
        },
        item_entity_verts: Vec::new(),
        item_entity_indices: Vec::new(),
        item_entity_visible: Vec::new(),
        chests: Vec::new(),
        chest_visible: Vec::new(),
        doors: Vec::new(),
        door_visible: Vec::new(),
        mobs: Vec::new(),
        particle_verts: Vec::new(),
        emitter_particle_verts: Vec::new(),
        emitter_particle_scratch: Vec::new(),
        ui_pipe: pipelines.ui_pipe,
        ui_texture_bgl: pipelines.atlas_bgl.clone(),
        hearts_bind,
        doc_ui: super::doc_ui::DocUi::default(),
        ui_solid_vbuf: pipelines.ui_vbuf,
        ui_count_vertex_count: 0,
        ui_drag_count_vertex_count: 0,
        ui_hearts_vbuf,
        ui_hearts_vertex_count: 0,
        effects_bind,
        ui_effects_vbuf,
        ui_effects_vertex_count: 0,
        ui_vignette_vbuf,
        ui_vignette_vertex_count: 0,
        icon_atlas,
        icon_quad_vbuf,
        icon_quad_verts: Vec::new(),
        icon_quad_vertex_count: 0,
        drag_icon_quad_vertex_count: 0,
        ui_build: UiBuild::default(),
        hand_vertex_count: 0,
    }
}

impl Renderer {
    /// The current surface size in physical pixels `(width, height)` — the same
    /// coordinate space the UI layout (`render::ui`) and cursor hit-testing use.
    #[inline]
    pub fn screen_size(&self) -> (u32, u32) {
        (self.config.width, self.config.height)
    }

    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = create_depth(&self.device, width, height);
        self.scene_color = create_scene_color(&self.device, width, height, self.config.format);
        self.grade_bind = super::super::pipeline::create_grade_bind(
            &self.device,
            &self.grade_bgl,
            &self.scene_color,
        );
        self.crosshair_drawn_size = (0, 0);
    }
}
