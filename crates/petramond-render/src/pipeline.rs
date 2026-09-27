use crate::atlas::tile_uv;
use petramond_mesh::{TerrainVertex, Vertex};
use petramond_world::tile::Tile;

use wgpu::util::DeviceExt;

use super::uniforms::Uniforms;
use super::{item_model, particles, resources, shader_pack, ui};

mod builders;
mod sampling;
pub(crate) use builders::{texture_sampler_bind_entries, texture_sampler_layout_entries};
pub(crate) use sampling::SampledPipeline;
mod entity_models;
mod environment;
mod flipbook;
mod fluid_media;
#[cfg(test)]
mod gpu_validation;
mod grade;
mod model3d;
mod overlays;
mod particle;
pub(crate) mod prelude;
mod sky;
mod terrain;
mod transition;
mod ui_icons;
mod variation;

#[cfg(test)]
pub(crate) use environment::scaler_sources;
pub(crate) use environment::scene_depth_source;

pub(crate) const GRADE_SHADER: &str = include_str!("../shaders/grade.wgsl");

pub(super) use self::environment::{
    create_env_comp_bind, create_env_down_bind, create_environment_bind, EnvPassResources,
    EnvScalers,
};
pub(super) use self::grade::create_grade_bind;

use self::builders::{pipeline_layout, shader_module, texture_sampler_bgl_bind, uniform_entry};
use self::entity_models::{
    create_bone_palette_bgl, create_mob_pipeline, create_model_break_pipeline,
    create_skinned_pipeline, create_world_model_pipeline,
};
use self::environment::{create_env_scaler, create_environment_pipelines};
use self::grade::create_grade_pipeline;
use self::model3d::{create_item3d_pipeline, create_model3d_pipelines};
use self::overlays::{
    create_break_overlay_pipeline, create_contact_pipeline, create_crosshair_pipeline,
    create_entity_shadow_pipeline, create_selection_pipeline,
};
use self::particle::create_particle_pipeline;
use self::sky::create_sky_pipeline;
use self::terrain::create_terrain_pipelines;
use self::ui_icons::{create_model_icon_pipeline, create_ui_pipeline};

pub(super) struct PipelineResources {
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub atlas_array_bind: wgpu::BindGroup,
    pub atlas_bgl: wgpu::BindGroupLayout,
    pub sky_pipe: crate::pipeline::SampledPipeline,
    pub sky_bind: wgpu::BindGroup,
    pub sky_texture_bind: wgpu::BindGroup,
    pub sky_shader_param_keys: Vec<String>,
    pub sky_light_param_key: Option<String>,
    pub env_passes: Vec<EnvPassResources>,
    pub env_scaler: EnvScalers,
    pub opaque_pipe: crate::pipeline::SampledPipeline,
    pub translucent_pipe: crate::pipeline::SampledPipeline,
    pub transparent_pipe: crate::pipeline::SampledPipeline,
    pub transparent_two_sided_pipe: crate::pipeline::SampledPipeline,
    pub dynamic_opaque_pipe: crate::pipeline::SampledPipeline,
    pub grade_pipe: wgpu::RenderPipeline,
    pub grade_bgl: wgpu::BindGroupLayout,
    pub outline_pipe: crate::pipeline::SampledPipeline,
    pub outline_bind: wgpu::BindGroup,
    pub outline_vbuf: wgpu::Buffer,
    pub crosshair_pipe: wgpu::RenderPipeline,
    pub crosshair_vbuf: wgpu::Buffer,
    /// model3d pipeline: per-draw MVP (dynamic offset) + block atlas, full-bright,
    /// NO depth. Serves the isometric slot icons in the depthless UI pass.
    pub model3d_pipe: wgpu::RenderPipeline,
    pub model3d_hand_pipe: crate::pipeline::SampledPipeline,
    pub model3d_mvp_buf: wgpu::Buffer,
    pub model3d_mvp_bind: wgpu::BindGroup,
    /// The model3d group(0) bind-group LAYOUT (dynamic-offset MVP at binding 0 +
    /// uv_rects at binding 1), exposed so the renderer can build a SEPARATE,
    /// icon-count-sized MVP buffer + bind for the one-time icon-atlas bake (Pass A
    /// needs one live MVP slot per cube/sprite icon simultaneously — more than the
    /// per-frame [`MODEL3D_MVP_SLOTS`]).
    pub model3d_mvp_bgl: wgpu::BindGroupLayout,
    pub uv_rects_buf: wgpu::Buffer,
    pub model3d_vbuf: wgpu::Buffer,
    pub model3d_ibuf: wgpu::Buffer,
    /// item3d pipeline: the EXTRUDED first-person held item (flowers / tools).
    /// Explicit per-vertex (pos, uv, shade); group(0) = a dynamic-offset MVP over
    /// the shared `model3d_mvp_buf`; group(1) = the block atlas. Full-bright,
    /// alpha-cutout, double-sided, depth test + write (the hand pass clears depth)
    /// so the front/back/side-wall faces self-sort instead of overdrawing.
    pub item3d_pipe: crate::pipeline::SampledPipeline,
    pub item3d_mvp_bind: wgpu::BindGroup,
    pub item3d_vbuf: wgpu::Buffer,
    pub mob_pipe: crate::pipeline::SampledPipeline,
    pub skinned_pipe: crate::pipeline::SampledPipeline,
    pub bone_palette_bgl: wgpu::BindGroupLayout,
    /// World-model pipeline: the chunk's bbmodel-block stream (`ModelVertex`,
    /// model atlas at group1). Same layout/blend/depth as `mob_pipe`, but its
    /// vertices carry (sky, block) light separately and the shader applies the
    /// sim's day/night sky scale at draw time, so placed models darken at
    /// night like terrain (their meshes don't rebake when the sun sets).
    pub world_model_pipe: crate::pipeline::SampledPipeline,
    pub world_model_blend_pipe: crate::pipeline::SampledPipeline,
    pub model_break_pipe: crate::pipeline::SampledPipeline,
    pub model_break_bgl: wgpu::BindGroupLayout,
    pub break_pipe: crate::pipeline::SampledPipeline,
    pub contact_pipe: crate::pipeline::SampledPipeline,
    pub entity_shadow_pipe: crate::pipeline::SampledPipeline,
    pub particle_pipe: crate::pipeline::SampledPipeline,
    pub emitter_particle_pipe: crate::pipeline::SampledPipeline,
    pub ui_pipe: wgpu::RenderPipeline,
    pub model_icon_pipe: wgpu::RenderPipeline,
}

fn block_shader_source(media: &[petramond_world::fluid::FluidMedium]) -> String {
    let source = transition::declarations()
        + &variation::declarations()
        + &flipbook::declarations()
        + &fluid_media::declarations(media)
        + concat!(
            include_str!("../shaders/cel.wgsl"),
            include_str!("../shaders/atmosphere.wgsl"),
            include_str!("../shaders/sheen.wgsl"),
            include_str!("../shaders/texture_transition.wgsl"),
            include_str!("../shaders/tile_variation.wgsl"),
            include_str!("../shaders/selection_highlight.wgsl"),
            include_str!("../shaders/block.wgsl")
        );
    prelude::compose(&source)
        .unwrap_or_else(|e| panic!("terrain shader: {e}"))
        .into_owned()
}

#[allow(clippy::too_many_arguments)]
pub(super) fn create_pipeline_resources(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    format: wgpu::TextureFormat,
    max_samples: u32,
    uniform_buf: &wgpu::Buffer,
    shader_params_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    array_view: &wgpu::TextureView,
    array_sampler: &wgpu::Sampler,
) -> PipelineResources {
    let block_source = block_shader_source(&fluid_media::registered());
    let shader = shader_module(device, "block shader", block_source);
    let crosshair_shader = shader_module(
        device,
        "crosshair shader",
        include_str!("../shaders/crosshair.wgsl"),
    );

    let shared = create_shared_bindings(
        device,
        uniform_buf,
        atlas_view,
        atlas_sampler,
        array_view,
        array_sampler,
    );

    let vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 16,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 20,
            shader_location: 3,
        },
    ];
    let vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<Vertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &vbuf_attrs,
    };
    // Terrain 20-byte vertex: i16×3 pos + pad + tint + packed×2, plus instance
    // column origin at location 4.
    // Sint16x4 = pos.xyz + pad (wgpu has no Sint16x3).
    let terrain_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Sint16x4,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Unorm8x4,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 12,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Uint32,
            offset: 16,
            shader_location: 3,
        },
    ];
    let terrain_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<TerrainVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &terrain_vbuf_attrs,
    };

    let item3d_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 12,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32,
            offset: 20,
            shader_location: 2,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x3,
            offset: 24,
            shader_location: 3,
        },
    ];
    let item3d_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &item3d_vbuf_attrs,
    };

    let (opaque_pipe, translucent_pipe, transparent_pipe, transparent_two_sided_pipe) =
        create_terrain_pipelines(
            device,
            format,
            max_samples,
            &shader,
            &shared.array_layout,
            &[terrain_vbuf_layout, crate::resources::COLUMN_ORIGIN_LAYOUT],
        );
    let dynamic_opaque_targets = builders::color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let dynamic_opaque_pipe = builders::world_pipeline(
        device,
        "dynamic opaque pipe",
        &shared.array_layout,
        &shader,
        "vs_main",
        "fs_opaque",
        std::slice::from_ref(&vbuf_layout),
        &dynamic_opaque_targets,
        builders::cull_back(),
        Some(builders::DepthPreset::WriteLess),
        max_samples,
    );
    let sky = create_sky_pipeline(
        device,
        queue,
        format,
        max_samples,
        uniform_buf,
        shader_params_buf,
    );
    let env_passes = create_environment_pipelines(device, queue, format);
    let env_scaler = create_env_scaler(device, format, max_samples);
    let (outline_pipe, outline_bind, outline_vbuf) =
        create_selection_pipeline(device, format, max_samples, uniform_buf);
    let (crosshair_pipe, crosshair_vbuf) =
        create_crosshair_pipeline(device, format, &crosshair_shader);
    let model3d = create_model3d_pipelines(
        device,
        format,
        max_samples,
        uniform_buf,
        &shared.uv_rects_buf,
        &shared.atlas_bgl,
        &vbuf_layout,
    );
    let (item3d_pipe, item3d_mvp_bind, item3d_vbuf) = create_item3d_pipeline(
        device,
        format,
        max_samples,
        &shared.atlas_bgl,
        &model3d.mvp_buf,
        &item3d_vbuf_layout,
    );
    let (mob_pipe, mob_shader) = create_mob_pipeline(
        device,
        format,
        max_samples,
        &shared.layout,
        &item3d_vbuf_layout,
    );
    let bone_palette_bgl = create_bone_palette_bgl(device);
    let skinned_layout = pipeline_layout(
        device,
        "skinned pipe layout",
        &[&shared.uniform_bgl, &shared.atlas_bgl, &bone_palette_bgl],
    );
    let skinned_pipe = create_skinned_pipeline(device, format, max_samples, &skinned_layout);
    let world_model_pipe = create_world_model_pipeline(
        device,
        format,
        max_samples,
        &shared.layout,
        &mob_shader,
        false,
    );
    let world_model_blend_pipe = create_world_model_pipeline(
        device,
        format,
        max_samples,
        &shared.layout,
        &mob_shader,
        true,
    );
    let model_break_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("model break bgl"),
        entries: &crate::model_break::layout_entries(),
    });
    let model_break_layout = pipeline_layout(
        device,
        "model break pipe layout",
        &[&shared.uniform_bgl, &shared.atlas_bgl, &model_break_bgl],
    );
    let model_break_pipe = create_model_break_pipeline(
        device,
        format,
        max_samples,
        &model_break_layout,
        &mob_shader,
    );
    let break_pipe =
        create_break_overlay_pipeline(device, format, max_samples, &shared.layout, &vbuf_layout);
    let contact_pipe = create_contact_pipeline(device, format, max_samples, &shared.uniform_bgl);
    let entity_shadow_pipe =
        create_entity_shadow_pipeline(device, format, max_samples, &shared.uniform_bgl);
    let particles = create_particle_pipeline(device, format, max_samples, &shared.layout);
    let ui_pipe = create_ui_pipeline(device, format);
    let model_icon_pipe = create_model_icon_pipeline(device, format, &shared.atlas_bgl);
    let (grade_pipe, grade_bgl) = create_grade_pipeline(device, format);

    PipelineResources {
        atlas_array_bind: shared.atlas_array_bind,
        uniform_bind: shared.uniform_bind,
        atlas_bind: shared.atlas_bind,
        atlas_bgl: shared.atlas_bgl,
        sky_pipe: sky.pipe,
        sky_bind: sky.bind,
        sky_texture_bind: sky.texture_bind,
        sky_shader_param_keys: sky.shader_param_keys,
        sky_light_param_key: sky.light_param_key,
        env_passes,
        env_scaler,
        opaque_pipe,
        translucent_pipe,
        transparent_pipe,
        transparent_two_sided_pipe,
        dynamic_opaque_pipe,
        grade_pipe,
        grade_bgl,
        outline_pipe,
        outline_bind,
        outline_vbuf,
        crosshair_pipe,
        crosshair_vbuf,
        model3d_pipe: model3d.pipe,
        model3d_hand_pipe: model3d.hand_pipe,
        model3d_mvp_buf: model3d.mvp_buf,
        model3d_mvp_bind: model3d.mvp_bind,
        model3d_mvp_bgl: model3d.mvp_bgl,
        uv_rects_buf: shared.uv_rects_buf,
        model3d_vbuf: model3d.vbuf,
        model3d_ibuf: model3d.ibuf,
        item3d_pipe,
        item3d_mvp_bind,
        item3d_vbuf,
        mob_pipe,
        skinned_pipe,
        bone_palette_bgl,
        world_model_pipe,
        world_model_blend_pipe,
        model_break_pipe,
        model_break_bgl,
        break_pipe,
        contact_pipe,
        entity_shadow_pipe,
        particle_pipe: particles.pipe,
        emitter_particle_pipe: particles.emitter_pipe,
        ui_pipe,
        model_icon_pipe,
    }
}

struct SharedBindings {
    uv_rects_buf: wgpu::Buffer,
    uniform_bgl: wgpu::BindGroupLayout,
    uniform_bind: wgpu::BindGroup,
    atlas_bgl: wgpu::BindGroupLayout,
    atlas_bind: wgpu::BindGroup,
    layout: wgpu::PipelineLayout,
    atlas_array_bind: wgpu::BindGroup,
    array_layout: wgpu::PipelineLayout,
}

fn create_shared_bindings(
    device: &wgpu::Device,
    uniform_buf: &wgpu::Buffer,
    atlas_view: &wgpu::TextureView,
    atlas_sampler: &wgpu::Sampler,
    array_view: &wgpu::TextureView,
    array_sampler: &wgpu::Sampler,
) -> SharedBindings {
    let mut uv_rects = vec![[0f32; 4]; Tile::count().max(1)];
    for t in Tile::all() {
        uv_rects[t.index()] = tile_uv(t);
    }
    let uv_rects_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uv_rects"),
        contents: bytemuck::cast_slice(&uv_rects),
        usage: wgpu::BufferUsages::STORAGE,
    });

    let uniform_bgl = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("uniform bgl"),
        entries: &[
            uniform_entry(
                0,
                wgpu::ShaderStages::VERTEX | wgpu::ShaderStages::FRAGMENT,
                std::mem::size_of::<Uniforms>() as u64,
            ),
            crate::uniforms::uv_rects_entry(1),
            crate::selection_highlight::layout_entries()[0],
            crate::selection_highlight::layout_entries()[1],
        ],
    });
    let uniform_bind =
        crate::selection_highlight::inactive_bind(device, &uniform_bgl, uniform_buf, &uv_rects_buf);

    let (atlas_bgl, atlas_bind) = texture_sampler_bgl_bind(
        device,
        "atlas",
        atlas_view,
        atlas_sampler,
        wgpu::TextureViewDimension::D2,
    );
    let layout = pipeline_layout(device, "pipe layout", &[&uniform_bgl, &atlas_bgl]);

    // Terrain block pipelines bind a tile array in group 1: one layer per tile, REPEAT wrapping, so
    // a greedy-meshed quad tiles its layer. Models, break cracks, particles and mobs keep the 2D
    // atlas above.
    let (array_bgl, atlas_array_bind) = texture_sampler_bgl_bind(
        device,
        "atlas array",
        array_view,
        array_sampler,
        wgpu::TextureViewDimension::D2Array,
    );
    let array_layout = pipeline_layout(device, "array pipe layout", &[&uniform_bgl, &array_bgl]);

    SharedBindings {
        uv_rects_buf,
        uniform_bgl,
        uniform_bind,
        atlas_bgl,
        atlas_bind,
        layout,
        atlas_array_bind,
        array_layout,
    }
}

pub(crate) mod world_overlay;
