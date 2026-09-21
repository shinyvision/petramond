use crate::renderer::{dynamic_draw, Renderer};
use crate::schematic::Geometry;
use crate::{pipeline::SampledPipeline, uniforms::Uniforms};
use glam::{IVec3, Mat4, Vec3};
use wgpu::util::DeviceExt;

/// The two ghost pipelines and their atlases, compiled once and shared by
/// every ghost mesh.
#[derive(Clone)]
pub(super) struct GhostPipelines {
    uniform_layout: wgpu::BindGroupLayout,
    blocks: (SampledPipeline, wgpu::BindGroup),
    models: (SampledPipeline, wgpu::BindGroup),
}

impl GhostPipelines {
    pub fn new(r: &Renderer) -> Self {
        let uniform_layout = r
            .block_entity
            .chest_draw
            .pipeline
            .get(1)
            .get_bind_group_layout(0);
        let pipeline = |model| {
            crate::pipeline::world_overlay::ghost(
                &r.device,
                r.config.format,
                r.targets.max_samples,
                &uniform_layout,
                &if model {
                    r.world_model_pipe.get(1)
                } else {
                    r.block_entity.chest_draw.pipeline.get(1)
                }
                .get_bind_group_layout(1),
                model,
            )
        };
        Self {
            blocks: (pipeline(false), r.atlas_array_bind.clone()),
            models: (pipeline(true), r.model_atlas_bind.clone()),
            uniform_layout,
        }
    }
}

struct Mesh {
    pipeline: SampledPipeline,
    atlas: wgpu::BindGroup,
    vertices: wgpu::Buffer,
    indices: wgpu::Buffer,
    count: u32,
}

/// One alpha-blended schematic mesh (block + model geometry) at an integer
/// world origin.
pub(super) struct GhostMesh {
    blocks: Mesh,
    models: Mesh,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    instance: wgpu::Buffer,
    pub origin: IVec3,
}

impl GhostMesh {
    pub fn new(device: &wgpu::Device, pipelines: &GhostPipelines) -> Self {
        let mesh = |(pipeline, atlas): &(SampledPipeline, wgpu::BindGroup)| Mesh {
            pipeline: pipeline.clone(),
            atlas: atlas.clone(),
            vertices: dynamic_draw::new_buffer(
                device,
                wgpu::BufferUsages::VERTEX,
                "ghost vertices",
            ),
            indices: dynamic_draw::new_buffer(device, wgpu::BufferUsages::INDEX, "ghost indices"),
            count: 0,
        };
        let uniform = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ghost camera"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let uv = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("ghost uv table"),
            size: (crate::uniforms::UV_RECTS_LEN * 16) as u64,
            usage: wgpu::BufferUsages::UNIFORM,
            mapped_at_creation: false,
        });
        let bind = crate::selection_highlight::inactive_bind(
            device,
            &pipelines.uniform_layout,
            &uniform,
            &uv,
        );
        let instance = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("ghost origin"),
            contents: bytemuck::cast_slice(&[0i32; 4]),
            usage: wgpu::BufferUsages::VERTEX,
        });
        Self {
            blocks: mesh(&pipelines.blocks),
            models: mesh(&pipelines.models),
            uniform,
            bind,
            instance,
            origin: IVec3::ZERO,
        }
    }
    pub fn clear(&mut self) {
        self.blocks.count = 0;
        self.models.count = 0;
    }
    pub fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, geometry: &Geometry) {
        for (mesh, vertices, indices) in [
            (
                &mut self.blocks,
                bytemuck::cast_slice::<_, u8>(&geometry.blocks),
                &geometry.block_indices,
            ),
            (
                &mut self.models,
                bytemuck::cast_slice::<_, u8>(&geometry.models),
                &geometry.model_indices,
            ),
        ] {
            mesh.count = indices.len() as u32;
            if mesh.count == 0 {
                continue;
            }
            dynamic_draw::upload(
                device,
                queue,
                &mut mesh.vertices,
                vertices,
                wgpu::BufferUsages::VERTEX,
                "ghost vertices",
            );
            dynamic_draw::upload(
                device,
                queue,
                &mut mesh.indices,
                indices,
                wgpu::BufferUsages::INDEX,
                "ghost indices",
            );
        }
    }
    pub fn camera(&self, queue: &wgpu::Queue, world: &Uniforms) {
        let mut u = *world;
        let offset = (self.origin
            - IVec3::new(u.render_origin[0], u.render_origin[1], u.render_origin[2]))
        .as_vec3();
        let vp = Mat4::from_cols_array_2d(&u.view_proj) * Mat4::from_translation(offset);
        u.view_proj = vp.to_cols_array_2d();
        u.inv_view_proj = vp.inverse().to_cols_array_2d();
        u.cam_pos = (Vec3::from_slice(&u.cam_pos) - offset)
            .extend(0.0)
            .to_array();
        u.render_origin = [0; 4];
        queue.write_buffer(&self.uniform, 0, bytemuck::bytes_of(&u));
    }
    pub fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        pass.set_bind_group(0, &self.bind, &[]);
        for mesh in [&self.blocks, &self.models] {
            if mesh.count == 0 {
                continue;
            }
            pass.set_pipeline(mesh.pipeline.get(samples));
            pass.set_bind_group(1, &mesh.atlas, &[]);
            pass.set_vertex_buffer(0, mesh.vertices.slice(..));
            pass.set_vertex_buffer(1, self.instance.slice(..));
            pass.set_index_buffer(mesh.indices.slice(..), wgpu::IndexFormat::Uint32);
            pass.draw_indexed(0..mesh.count, 0, 0..1);
        }
    }
}
