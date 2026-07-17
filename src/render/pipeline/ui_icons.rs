use super::builders::{
    color_target, pipeline_layout, shader_module, texture_sampler_bgl, world_pipeline, DepthPreset,
};

/// Max vertices in each reusable UI dynamic vbuf (gui quads, stack-count digits,
/// icon quads, and text quads). Shell labels are drawn from runtime text atlases,
/// so this no longer needs to cover one solid quad per text bitmap cell.
pub(in crate::render) const MAX_UI_VERTICES: u64 = 16384;

/// UI pipeline (2D HUD / inventory).
/// group(0) is the SEPARATE gui sprite atlas (texture + sampler) — NOT the
/// block atlas. Vertices are NDC pos (vec2) + uv (vec2) + color (vec4); the
/// fragment shader outputs the vertex color for the solid sentinel (uv.x < 0)
/// and otherwise samples the gui atlas * color. Alpha-blended, NO depth, drawn
/// LAST so it sits over every world / hand / crosshair pass.
pub(super) fn create_ui_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
) -> (wgpu::RenderPipeline, wgpu::Buffer) {
    let ui_shader = shader_module(device, "ui shader", include_str!("../../shaders/ui.wgsl"));
    let ui_bgl = texture_sampler_bgl(device, "ui bgl", wgpu::TextureViewDimension::D2);
    let ui_layout = pipeline_layout(device, "ui layout", &[&ui_bgl]);
    let ui_vbuf_attrs = [
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 0,
            shader_location: 0,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 8,
            shader_location: 1,
        },
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x4,
            offset: 16,
            shader_location: 2,
        },
    ];
    let ui_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::ui::UiVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &ui_vbuf_attrs,
    };
    // UI quads are CPU-emitted CCW but disabling cull is robust against either
    // winding; no depth (last pass). Alpha blend.
    let ui_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    let ui_pipe = world_pipeline(
        device,
        "ui pipe",
        &ui_layout,
        &ui_shader,
        "vs_ui",
        "fs_ui",
        std::slice::from_ref(&ui_vbuf_layout),
        &ui_targets,
        wgpu::PrimitiveState::default(),
        None,
        sample_count,
    );
    let ui_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("ui vbuf"),
        size: MAX_UI_VERTICES * std::mem::size_of::<super::ui::UiVertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    (ui_pipe, ui_vbuf)
}

/// model-icon pipeline (bbmodel-block icon-atlas cells).
/// Pass-through `ItemVertex` (positions already in clip space, the MVP baked in by
/// `build_block_model_icon`) sampling the MODEL atlas at group(0). Depth test +
/// WRITE: the double-sided model self-sorts by depth (the faces are also emitted
/// far→near as a tiebreak). The same `item3d`/`mob` ItemVertex layout (pos f32x3 @0,
/// uv f32x2 @12, shade f32 @20, tint f32x3 @24) feeds it, so the model-atlas
/// validation test covers it. Used only to bake the model cells of the icon atlas.
pub(super) fn create_model_icon_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    atlas_bgl: &wgpu::BindGroupLayout,
) -> wgpu::RenderPipeline {
    let model_icon_shader = shader_module(
        device,
        "model icon shader",
        include_str!("../../shaders/model_icon.wgsl"),
    );
    let model_icon_layout = pipeline_layout(device, "model icon layout", &[atlas_bgl]);
    let model_icon_vbuf_attrs = [
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
    let model_icon_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<super::item_model::ItemVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &model_icon_vbuf_attrs,
    };
    let model_icon_targets = color_target(
        format,
        Some(wgpu::BlendState::ALPHA_BLENDING),
        wgpu::ColorWrites::ALL,
    );
    // Depth test + WRITE against the model-icon pass's OWN cleared depth buffer so the
    // (double-sided) model self-sorts — the panels/drawers can't be ordered by a painter
    // sort alone, exactly like the in-world block, which also leans on depth.
    let model_icon_pipe = world_pipeline(
        device,
        "model icon pipe",
        &model_icon_layout,
        &model_icon_shader,
        "vs_model_icon",
        "fs_model_icon",
        std::slice::from_ref(&model_icon_vbuf_layout),
        &model_icon_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    model_icon_pipe
}
