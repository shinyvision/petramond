use super::builders::{color_target, shader_module, world_pipeline, DepthPreset};
use crate::mesh::Vertex;

/// Max vertices / indices in the reusable `mob` dynamic buffers (animated entity
/// models), drawn by the dedicated mob pipeline with the explicit-UV `ItemVertex`.
/// Sized per SPECIES (each gets its own buffers): a 16-cube sheep is ≤384 verts,
/// so this covers ~200 simultaneously-visible sheep — far above what worldgen
/// herds put in the streamed area. The bake also truncates to whole instances,
/// closest first, so exceeding the budget drops the farthest mobs instead of
/// blanking the species for the frame (see `dynamic_bake`).
pub(in crate::render) const MAX_MOB_VERTICES: u64 = 81920;
pub(in crate::render) const MAX_MOB_INDICES: u64 = 122880;

/// Max vertices in the item-entity dynamic vbuf. A stack draws up to 5 layered
/// copies (120 verts per cube), so this is sized 5× the old single-copy budget
/// to still cover ~170 simultaneously-visible dropped items (more when they're
/// single, unstacked drops) without the bake overflowing and dropping every
/// item entity that frame. Also sizes the separate extruded-sprite item stream
/// (an extruded flower is a few hundred `ItemVertex` per layer, so that stream
/// covers dozens of visible sprite drops before its bake bails for a frame).
pub(in crate::render) const MAX_ITEM_ENTITY_VERTICES: u64 = 20480;
/// Max indices in the item-entity dynamic ibuf (up to 180 per cube for a
/// 5-layer stack), matching [`MAX_ITEM_ENTITY_VERTICES`].
pub(in crate::render) const MAX_ITEM_ENTITY_INDICES: u64 = 30720;
/// Max vertices in the chest dynamic vbuf. Each chest is a body box + lid box = 48
/// verts, so this covers ~512 simultaneously-visible chests before the bake bails
/// for that frame. Separate from the item-entity budget so a wall of chests can't
/// make dropped items vanish.
pub(in crate::render) const MAX_CHEST_VERTICES: u64 = 24576;
/// Max indices in the chest dynamic ibuf (72 per chest), matching
/// [`MAX_CHEST_VERTICES`].
pub(in crate::render) const MAX_CHEST_INDICES: u64 = 36864;
/// Max vertices in the door dynamic vbuf. Each door is two boxes (lower + upper half)
/// = 48 verts, so this covers ~512 simultaneously-visible doors before the bake bails.
/// Separate from the chest budget so a wall of doors can't make chests vanish.
pub(in crate::render) const MAX_DOOR_VERTICES: u64 = 24576;
/// Max indices in the door dynamic ibuf (72 per door), matching [`MAX_DOOR_VERTICES`].
pub(in crate::render) const MAX_DOOR_INDICES: u64 = 36864;

/// mob pipeline (in-world animated entity models).
/// Reuses the BLOCK pipeline layout (`layout` = [uniform_bgl, atlas_bgl]): group0
/// is the world `view_proj` uniform (the shader reads only view_proj; the uv_rects
/// binding in the layout is simply unused), group1 is an atlas-shaped texture+
/// sampler — bound by the renderer to the ENTITY texture, not the block atlas.
/// Same explicit-UV `ItemVertex` layout as item3d (the model carries arbitrary
/// sub-rect UVs). REPLACE blend + cutout (opaque creature), depth test + WRITE,
/// double-sided (cull off) so flat mob sub-cubes show from both sides.
///
/// The mob pipeline is shared across species; each species' own vbuf/ibuf +
/// bind group + DynamicDraw are built in the renderer by iterating `mob::defs()`
/// (each species has a distinct texture, so geometry can't share one buffer).
/// Also returns the mob shader module, which the world-model pipeline shares.
pub(super) fn create_mob_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    item3d_vbuf_layout: &wgpu::VertexBufferLayout,
) -> (wgpu::RenderPipeline, wgpu::ShaderModule) {
    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let mob_shader = shader_module(
        device,
        "mob shader",
        concat!(
            include_str!("../../shaders/cel.wgsl"),
            include_str!("../../shaders/atmosphere.wgsl"),
            include_str!("../../shaders/mob.wgsl")
        ),
    );
    let mob_pipe = world_pipeline(
        device,
        "mob pipe",
        layout,
        &mob_shader,
        "vs_mob",
        "fs_mob",
        std::slice::from_ref(item3d_vbuf_layout),
        &opaque_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    (mob_pipe, mob_shader)
}

/// world-model pipeline (chunk bbmodel-block stream).
/// `ModelVertex`: the ItemVertex attributes + a (sky, block) light pair at
/// @location(4), so `fs_world_model` can scale the sky term by the sim's
/// day/night state at draw time (chunk meshes don't rebake at sunset).
pub(super) fn create_world_model_pipeline(
    device: &wgpu::Device,
    format: wgpu::TextureFormat,
    sample_count: u32,
    layout: &wgpu::PipelineLayout,
    mob_shader: &wgpu::ShaderModule,
) -> wgpu::RenderPipeline {
    let opaque_targets = color_target(
        format,
        Some(wgpu::BlendState::REPLACE),
        wgpu::ColorWrites::ALL,
    );
    let world_model_vbuf_attrs = [
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
        wgpu::VertexAttribute {
            format: wgpu::VertexFormat::Float32x2,
            offset: 36,
            shader_location: 4,
        },
    ];
    let world_model_vbuf_layout = wgpu::VertexBufferLayout {
        array_stride: std::mem::size_of::<crate::mesh::ModelVertex>() as u64,
        step_mode: wgpu::VertexStepMode::Vertex,
        attributes: &world_model_vbuf_attrs,
    };
    let world_model_pipe = world_pipeline(
        device,
        "world model pipe",
        layout,
        mob_shader,
        "vs_world_model",
        "fs_world_model",
        std::slice::from_ref(&world_model_vbuf_layout),
        &opaque_targets,
        wgpu::PrimitiveState::default(),
        Some(DepthPreset::WriteLess),
        sample_count,
    );
    world_model_pipe
}

/// Dynamic vertex/index buffers for item-entity, chest, and door models (all
/// drawn by the opaque pipeline; separate budgets so one kind can't starve
/// another).
pub(super) struct EntityModelBuffers {
    pub(super) item_entity_vbuf: wgpu::Buffer,
    pub(super) item_entity_ibuf: wgpu::Buffer,
    pub(super) chest_vbuf: wgpu::Buffer,
    pub(super) chest_ibuf: wgpu::Buffer,
    pub(super) door_vbuf: wgpu::Buffer,
    pub(super) door_ibuf: wgpu::Buffer,
}

pub(super) fn create_entity_model_buffers(device: &wgpu::Device) -> EntityModelBuffers {
    // Item-entity dynamic buffers (drawn by the opaque pipeline; separate from the
    // hand's model3d buffers so the hand pass doesn't clobber them).
    let item_entity_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item entity vbuf"),
        size: MAX_ITEM_ENTITY_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let item_entity_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("item entity ibuf"),
        size: MAX_ITEM_ENTITY_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    // Chest model dynamic buffers (drawn by the opaque pipeline, like item entities;
    // separate so a chest wall and the dropped-item budget don't fight).
    let chest_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chest vbuf"),
        size: MAX_CHEST_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let chest_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chest ibuf"),
        size: MAX_CHEST_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let door_vbuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("door vbuf"),
        size: MAX_DOOR_VERTICES * std::mem::size_of::<Vertex>() as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });
    let door_ibuf = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("door ibuf"),
        size: MAX_DOOR_INDICES * 4,
        usage: wgpu::BufferUsages::INDEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    });

    EntityModelBuffers {
        item_entity_vbuf,
        item_entity_ibuf,
        chest_vbuf,
        chest_ibuf,
        door_vbuf,
        door_ibuf,
    }
}
