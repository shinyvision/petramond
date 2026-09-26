//! Each pass building itself from the pipeline resources it owns — the
//! pieces the renderer's constructor used to spell out as one struct literal.

use super::super::*;
use super::actors::{build_mob_gpu, build_player_gpu};
use super::hud::build_hud_layers;
use crate::pipeline::{EnvScalers, SampledPipeline};

impl TerrainPass {
    pub(super) fn new(device: &wgpu::Device, queue: &wgpu::Queue, pipes: TerrainPipes) -> Self {
        Self {
            pipes,
            columns: ColumnStore::default(),
            column_origins: ColumnOrigins::new(device),
            geometry: crate::resources::TerrainArenas::default(),
            quad_index: crate::resources::QuadIndexBuffer::new(device, queue),
            uploads: UploadQueue::default(),
            plan: TerrainPlan::default(),
            draws: TerrainDraws::new(device),
            sort_scratch: Vec::new(),
            sorted_scratch: Vec::new(),
            gpu_revision: 0,
            planned_gpu_revision: u64::MAX,
            view_key: TerrainViewKey {
                view_proj: [0; 16],
                cam: [0; 3],
                fog: 0,
            },
            planned_view_key: None,
            cull_index: Vec::new(),
            cull_regions: Vec::new(),
            cull_index_revision: u64::MAX,
            occlusion: SectionOcclusion::default(),
        }
    }
}

impl ViewState {
    /// Before the first `update_uniforms`: nothing culled, origin at zero.
    pub(super) fn initial() -> Self {
        Self {
            frustum: Frustum::permissive(),
            cam_pos: petramond_math::world_pos::WorldPos::ZERO,
            render_origin: glam::IVec3::ZERO,
            visual_time: 0.0,
            proj_y_scale: 1.0,
        }
    }
}

/// The sky's share of the pipeline build.
pub(super) struct SkyParts {
    pub(super) pipe: SampledPipeline,
    pub(super) bind: wgpu::BindGroup,
    pub(super) texture_bind: wgpu::BindGroup,
    pub(super) shader_param_keys: Vec<String>,
    pub(super) light_param_key: Option<String>,
    pub(super) env_passes: Vec<EnvPassResources>,
    pub(super) env_scaler: EnvScalers,
}

/// The half-res volumetric targets and the binds that read the frame depth
/// — everything of the environment chain that a scene-target rebuild
/// invalidates.
struct EnvTargets {
    color: wgpu::TextureView,
    depth: wgpu::TextureView,
    down_bind: wgpu::BindGroup,
    comp_bind: wgpu::BindGroup,
}

/// The environment targets at half the scene dims, bound over
/// `frame_depth` through the scaler for `samples`. Env passes march at half
/// resolution against a downsampled depth; the composite lifts the result
/// back (see `pipeline::EnvScaler`).
fn env_targets(
    device: &wgpu::Device,
    scalers: &EnvScalers,
    samples: u32,
    frame_depth: &wgpu::TextureView,
    (w, h): (u32, u32),
    format: wgpu::TextureFormat,
) -> EnvTargets {
    let scaler = scalers.get(samples);
    let (env_w, env_h) = (w.div_ceil(2), h.div_ceil(2));
    let color = create_scene_color(device, env_w, env_h, format);
    let depth = crate::resources::create_depth(device, env_w, env_h);
    let down_bind = crate::pipeline::create_env_down_bind(device, &scaler.down_bgl, frame_depth);
    let comp_bind = crate::pipeline::create_env_comp_bind(
        device,
        &scaler.comp_bgl,
        &color,
        &scaler.samp,
        &depth,
        frame_depth,
    );
    EnvTargets {
        color,
        depth,
        down_bind,
        comp_bind,
    }
}

impl SkyPass {
    pub(super) fn new(
        device: &wgpu::Device,
        parts: SkyParts,
        uniform_buf: &wgpu::Buffer,
        targets: &SceneTargets,
        scene: (u32, u32),
        format: wgpu::TextureFormat,
        (fog_start, fog_end): (f32, f32),
    ) -> Self {
        let env = env_targets(
            device,
            &parts.env_scaler,
            targets.anti_aliasing.sample_count(),
            &targets.depth,
            scene,
            format,
        );
        let env_passes = parts
            .env_passes
            .into_iter()
            .map(|res| EnvPass {
                bind: crate::pipeline::create_environment_bind(
                    device,
                    &res.bgl,
                    uniform_buf,
                    &res.params_buf,
                    &env.depth,
                ),
                res,
                dormant: false,
            })
            .collect();
        Self {
            pipe: parts.pipe,
            bind: parts.bind,
            texture_bind: parts.texture_bind,
            shader_param_keys: parts.shader_param_keys,
            light_param_key: parts.light_param_key,
            env_passes,
            env_scaler: parts.env_scaler,
            env_color: env.color,
            env_depth: env.depth,
            env_down_bind: env.down_bind,
            env_comp_bind: env.comp_bind,
            fog_start,
            fog_end,
            scale: 1.0,
            color: [1.0, 1.0, 1.0],
            clear_color: [0.60, 0.82, 1.00],
        }
    }

    /// Rebuild the environment targets for a new scene size or sample count,
    /// and every bind that references them or the recreated frame depth.
    pub(in crate::renderer) fn recreate_env_targets(
        &mut self,
        device: &wgpu::Device,
        uniform_buf: &wgpu::Buffer,
        frame_depth: &wgpu::TextureView,
        scene: (u32, u32),
        format: wgpu::TextureFormat,
        samples: u32,
    ) {
        let env = env_targets(
            device,
            &self.env_scaler,
            samples,
            frame_depth,
            scene,
            format,
        );
        for pass in &mut self.env_passes {
            pass.bind = crate::pipeline::create_environment_bind(
                device,
                &pass.res.bgl,
                uniform_buf,
                &pass.res.params_buf,
                &env.depth,
            );
        }
        self.env_color = env.color;
        self.env_depth = env.depth;
        self.env_down_bind = env.down_bind;
        self.env_comp_bind = env.comp_bind;
    }
}

impl ChromePass {
    pub(super) fn new(
        outline_pipe: SampledPipeline,
        outline_bind: wgpu::BindGroup,
        outline_vbuf: wgpu::Buffer,
        crosshair_pipe: wgpu::RenderPipeline,
        crosshair_vbuf: wgpu::Buffer,
    ) -> Self {
        Self {
            outline_pipe,
            outline_bind,
            outline_vbuf,
            outline_vertex_count: 0,
            crosshair_pipe,
            crosshair_vbuf,
            crosshair_vertex_count: 0,
            crosshair_drawn_size: (0, 0),
            crosshair_visible: false,
            selection: None,
            selection_drawn: None,
        }
    }
}

/// The first-person hand's share of the pipeline build.
pub(super) struct HandParts {
    pub(super) model3d_pipe: SampledPipeline,
    pub(super) model3d_mvp_buf: wgpu::Buffer,
    pub(super) model3d_mvp_bind: wgpu::BindGroup,
    pub(super) model3d_vbuf: wgpu::Buffer,
    pub(super) model3d_ibuf: wgpu::Buffer,
    pub(super) item3d_pipe: SampledPipeline,
    pub(super) item3d_mvp_bind: wgpu::BindGroup,
    pub(super) item3d_vbuf: wgpu::Buffer,
}

impl HandPass {
    pub(super) fn new(
        device: &wgpu::Device,
        parts: HandParts,
        break_pipe: SampledPipeline,
    ) -> Self {
        Self {
            model3d_pipe: parts.model3d_pipe,
            model3d_mvp_buf: parts.model3d_mvp_buf,
            model3d_mvp_bind: parts.model3d_mvp_bind,
            model3d_vbuf: parts.model3d_vbuf,
            model3d_ibuf: parts.model3d_ibuf,
            item3d_pipe: parts.item3d_pipe,
            item3d_mvp_bind: parts.item3d_mvp_bind,
            item3d_vbuf: parts.item3d_vbuf,
            item3d_verts: Vec::new(),
            item3d_vertex_count: 0,
            held_is_model: false,
            index_count: 0,
            verts: Vec::new(),
            indices: Vec::new(),
            model_scratch_verts: Vec::new(),
            model_scratch_indices: Vec::new(),
            off_item3d_scratch: Vec::new(),
            break_draw: DynamicDraw::new(device, break_pipe, "break overlay"),
            break_overlays: Vec::new(),
            held_item: HeldItemView::default(),
            visible: false,
            shake: [0.0, 0.0],
            screen_shake: true,
            held_item_skylight: crate::lighting::FULL_SKYLIGHT,
            held_item_blocklight: petramond_world::light::BlockLight6::DARK,
            vertex_count: 0,
            off_item: HeldItemView::default(),
            off_item3d_start: 0,
            off_item3d_count: 0,
            off_is_model: false,
            first_person: crate::first_person::FirstPersonHand::shipped(),
            arm_start: 0,
            arm_count: 0,
        }
    }
}

impl UiPass {
    /// `texture_bgl` is the texture+sampler layout every UI texture binds
    /// with; `solid_vbuf` the pipeline build's stack-count buffer.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        pipe: wgpu::RenderPipeline,
        texture_bgl: &wgpu::BindGroupLayout,
        solid_vbuf: wgpu::Buffer,
        icon_atlas: IconAtlas,
    ) -> Self {
        Self {
            viewport_generation: 1,
            prepared_viewport: UiViewport::default(),
            pipe,
            texture_bgl: texture_bgl.clone(),
            doc_ui: super::super::doc_ui::DocUi::default(),
            client_overlays: super::super::client_overlay::ClientOverlays::default(),
            solid_vbuf,
            solid_verts: Vec::new(),
            count_vertex_count: 0,
            overlay_count_vertex_count: 0,
            drag_count_vertex_count: 0,
            hud_layers: build_hud_layers(device, queue, texture_bgl),
            icon_atlas,
            // Reusable dynamic vbuf for the per-frame icon quads (6 UiVertex
            // per filled slot), grown to fit.
            icon_quad_vbuf: dynamic_draw::new_buffer(
                device,
                wgpu::BufferUsages::VERTEX,
                "icon quad vbuf",
            ),
            icon_quad_verts: Vec::new(),
            icon_quad_vertex_count: 0,
            overlay_icon_quad_vertex_count: 0,
            drag_icon_quad_vertex_count: 0,
            build: UiBuild::default(),
        }
    }
}

impl ParticlePass {
    pub(super) fn new(
        device: &wgpu::Device,
        particle_pipe: SampledPipeline,
        emitter_pipe: SampledPipeline,
    ) -> Self {
        Self {
            draw: DynamicInstanceDraw::new(
                device,
                particle_pipe,
                "particle",
                &crate::particles::CUBE_INDEX_PATTERN,
            ),
            emitter_draw: DynamicInstanceDraw::new(
                device,
                emitter_pipe,
                "emitter particle",
                &crate::particles::CUBE_INDEX_PATTERN,
            ),
            instances: Vec::new(),
            model_instances: Vec::new(),
            solid_instances: Vec::new(),
            emitters: Vec::new(),
            density: 1.0,
            block_count: 0,
            rows: Vec::new(),
            emitter_rows: Vec::new(),
            emitter_scratch: Vec::new(),
        }
    }
}

impl ShadowPass {
    pub(super) fn new(device: &wgpu::Device, pipe: SampledPipeline) -> Self {
        Self {
            draw: DynamicVertexDraw::new(
                device,
                pipe,
                "entity shadow",
                crate::entity_shadow::VERTS_PER_SHADOW,
                &crate::entity_shadow::QUAD_INDEX_PATTERN,
            ),
            verts: Vec::new(),
            instances: Vec::new(),
        }
    }
}

impl ItemEntityPass {
    /// Cubes draw through the absolute-vertex opaque pipeline (a clone of
    /// its Arc-backed handle over this pass's own buffers); bbmodel items and
    /// extruded sprites through the mob-layout one, each in its own stream.
    pub(super) fn new(
        device: &wgpu::Device,
        dynamic_opaque: &SampledPipeline,
        mob: &SampledPipeline,
    ) -> Self {
        Self {
            draw: DynamicDraw::new(device, dynamic_opaque.clone(), "item entity"),
            verts: Vec::new(),
            indices: Vec::new(),
            visible: Vec::new(),
            instances: Vec::new(),
            block_draws: Vec::new(),
            block_draws_visible: Vec::new(),
            model_draw: DynamicDraw::new(device, mob.clone(), "item model entity"),
            model_verts: Vec::new(),
            model_indices: Vec::new(),
            sprite_draw: DynamicDraw::new(device, mob.clone(), "item sprite entity"),
            sprite_verts: Vec::new(),
            sprite_indices: Vec::new(),
            sprite_scratch: Vec::new(),
        }
    }
}

impl BlockEntityPass {
    pub(super) fn new(device: &wgpu::Device, dynamic_opaque: SampledPipeline) -> Self {
        Self {
            draw: DynamicDraw::new(device, dynamic_opaque, "block entity"),
            instances: Vec::new(),
            visible: Vec::new(),
            baked: Vec::new(),
            baked_origin: glam::IVec3::MIN,
        }
    }
}

impl ActorPass {
    /// Every species' resources from the mob registry, the player body's,
    /// the frame's skin batch (skinned pipeline + bone-palette layout), and
    /// the three held-item streams attached to posed hands.
    pub(super) fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas_bgl: &wgpu::BindGroupLayout,
        skinned: SampledPipeline,
        bone_palette_bgl: wgpu::BindGroupLayout,
        mob: &SampledPipeline,
        dynamic_opaque: &SampledPipeline,
    ) -> Self {
        Self {
            mob_gpu: build_mob_gpu(device, queue, atlas_bgl),
            skin: SkinFrame::new(device, skinned, bone_palette_bgl),
            mobs: Vec::new(),
            mob_arena: Default::default(),
            anim_names: Default::default(),
            player_gpu: build_player_gpu(device, queue, atlas_bgl),
            bodies: Vec::new(),
            body_poses: Vec::new(),
            player_visible: Vec::new(),
            item_draw: DynamicDraw::new(device, mob.clone(), "player item"),
            item_verts: Vec::new(),
            item_indices: Vec::new(),
            sprite_verts: Vec::new(),
            model_item_draw: DynamicDraw::new(device, mob.clone(), "player model item"),
            model_item_verts: Vec::new(),
            model_item_indices: Vec::new(),
            block_item_draw: DynamicDraw::new(device, dynamic_opaque.clone(), "player block item"),
        }
    }
}
