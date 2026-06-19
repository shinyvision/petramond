use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY};
use crate::world::World;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

use super::pipeline::create_pipeline_resources;
use super::resources::{create_atlas, create_depth, upload_mesh, GpuMesh};
use super::selection::outline_vertices;
use super::uniforms::{Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START};

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas_texture: wgpu::Texture,
    pub atlas_view: wgpu::TextureView,
    pub atlas_sampler: wgpu::Sampler,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    /// Pipeline for the targeted-block wireframe (LineList, black, view_proj only).
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    /// 24 line vertices (12 cube edges) for the selection box; rewritten only
    /// when the selected block changes (see `selection` / `selection_drawn`).
    pub outline_vbuf: wgpu::Buffer,
    /// Currently-targeted block (min corner), or None when nothing is targeted.
    pub selection: Option<glam::IVec3>,
    /// The block whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<glam::IVec3>,
    pub uniform_buf: wgpu::Buffer,
    pub uniform_bind: wgpu::BindGroup,
    pub atlas_bind: wgpu::BindGroup,
    pub depth: wgpu::TextureView,
    pub chunk_meshes: HashMap<ChunkPos, GpuMesh>,
    /// Camera frustum for viewspace culling, refreshed each frame in
    /// `update_uniforms`; chunk meshes outside it are skipped in `render`.
    pub frustum: Frustum,
    /// Camera world position, refreshed in `update_uniforms`; used to sort
    /// chunk draws front-to-back (opaque) / back-to-front (transparent).
    pub cam_pos: glam::Vec3,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    pub clear_color: [f32; 3],
}

pub async fn new_renderer_from_target(
    target: impl Into<wgpu::SurfaceTarget<'static>>,
    width: u32,
    height: u32,
) -> Renderer {
    let instance = wgpu::Instance::new(&instance_descriptor());
    let surface = instance.create_surface(target).expect("create surface");
    new_renderer_inner(instance, surface, width, height).await
}

pub async fn new_renderer_with_instance(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    new_renderer_inner(instance, surface, width, height).await
}

/// Instance descriptor that picks appropriate backends per platform:
/// native = all (Vulkan/Metal/DX12/GL); web = WebGPU with WebGL fallback.
pub fn instance_descriptor() -> wgpu::InstanceDescriptor {
    #[cfg(target_arch = "wasm32")]
    {
        wgpu::InstanceDescriptor {
            backends: wgpu::Backends::BROWSER_WEBGPU | wgpu::Backends::GL,
            flags: wgpu::InstanceFlags::default(),
            ..Default::default()
        }
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        wgpu::InstanceDescriptor::default()
    }
}

pub async fn new_renderer(surface: wgpu::Surface<'static>, width: u32, height: u32) -> Renderer {
    // NOTE: surface must be created from a wgpu::Instance that is *not*
    // dropped before this call. We create a fresh instance here which means
    // the caller must have created the surface from this same runtime. In
    // practice, prefer `new_renderer_from_target` so the surface and adapter
    // share the same instance.
    let instance = wgpu::Instance::new(&instance_descriptor());
    new_renderer_inner(instance, surface, width, height).await
}

async fn new_renderer_inner(
    instance: wgpu::Instance,
    surface: wgpu::Surface<'static>,
    width: u32,
    height: u32,
) -> Renderer {
    // Try a high-performance adapter first; on browsers without WebGPU this
    // may still succeed via the WebGL fallback. If it fails entirely (no
    // adapter compatible with the surface), retry with force_fallback_adapter
    // to accept the software/lowest-tier adapter rather than panicking.
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
            #[cfg(target_arch = "wasm32")]
            web_sys::console::warn_1(&"wgpu: primary adapter unavailable; trying fallback".into());
            #[cfg(not(target_arch = "wasm32"))]
            eprintln!("wgpu: primary adapter unavailable; trying fallback");
            instance
                .request_adapter(&wgpu::RequestAdapterOptions {
                    power_preference: wgpu::PowerPreference::LowPower,
                    compatible_surface: Some(&surface),
                    force_fallback_adapter: true,
                })
                .await
                .expect("no wgpu adapter available (WebGPU/WebGL both failed)")
        }
    };
    let (device, queue) = adapter
        .request_device(&wgpu::DeviceDescriptor {
            label: None,
            required_features: wgpu::Features::empty(),
            required_limits: {
                #[cfg(target_arch = "wasm32")]
                {
                    wgpu::Limits::downlevel_webgl2_defaults().using_alignment(adapter.limits())
                }
                #[cfg(not(target_arch = "wasm32"))]
                {
                    wgpu::Limits::default().using_alignment(adapter.limits())
                }
            },
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

    let (atlas_texture, atlas_view, atlas_sampler) = create_atlas(&device, &queue);
    let uniform_buf = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
        label: Some("uniforms"),
        contents: bytemuck::cast_slice(&[Uniforms {
            view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
            cam_pos: [0.0; 4],
            fog: [FOG_START, FOG_END, 0.0, 0.0],
            fog_color: [0.62, 0.78, 0.95, 1.0],
        }]),
        usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
    });
    let pipelines = create_pipeline_resources(
        &device,
        format,
        sample_count,
        &uniform_buf,
        &atlas_view,
        &atlas_sampler,
    );
    let depth = create_depth(&device, width, height);

    Renderer {
        surface,
        device,
        queue,
        config,
        atlas_texture,
        atlas_view,
        atlas_sampler,
        opaque_pipe: pipelines.opaque_pipe,
        transparent_pipe: pipelines.transparent_pipe,
        outline_pipe: pipelines.outline_pipe,
        outline_bind: pipelines.outline_bind,
        outline_vbuf: pipelines.outline_vbuf,
        selection: None,
        selection_drawn: None,
        uniform_buf,
        uniform_bind: pipelines.uniform_bind,
        atlas_bind: pipelines.atlas_bind,
        depth,
        chunk_meshes: HashMap::new(),
        frustum: Frustum::permissive(),
        cam_pos: glam::Vec3::ZERO,
        clear_color: [0.62, 0.78, 0.95],
    }
}

impl Renderer {
    pub fn resize(&mut self, width: u32, height: u32) {
        if width == 0 || height == 0 {
            return;
        }
        self.config.width = width;
        self.config.height = height;
        self.surface.configure(&self.device, &self.config);
        self.depth = create_depth(&self.device, width, height);
    }

    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        underwater: bool,
    ) {
        let view_proj = cam.view_proj();
        // Refresh the culling frustum from the same matrix the GPU will use.
        self.frustum = Frustum::from_view_proj(view_proj);
        self.cam_pos = cam.pos;
        self.clear_color = fog_color;
        let (fog_start, fog_end) = if underwater {
            (UNDERWATER_FOG_START, UNDERWATER_FOG_END)
        } else {
            (FOG_START, FOG_END)
        };
        let u = Uniforms {
            view_proj: view_proj.to_cols_array_2d(),
            cam_pos: [cam.pos.x, cam.pos.y, cam.pos.z, 0.0],
            // fog.z = animation time (caustics), fog.w = underwater flag.
            fog: [fog_start, fog_end, time, if underwater { 1.0 } else { 0.0 }],
            fog_color: [fog_color[0], fog_color[1], fog_color[2], 1.0],
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    /// Set (or clear) the block highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, block: Option<glam::IVec3>) {
        self.selection = block;
    }

    /// Is this chunk mesh's bounding box inside the current view frustum?
    #[inline]
    fn chunk_visible(&self, gm: &GpuMesh) -> bool {
        let (ox, oz) = gm.origin;
        let min = glam::Vec3::new(ox as f32, 0.0, oz as f32);
        let max = glam::Vec3::new((ox + 16) as f32, CHUNK_SY as f32, (oz + 16) as f32);
        self.frustum.aabb_visible(min, max)
    }

    /// Synchronize GPU meshes with the World's CPU meshes.
    pub fn sync_meshes(&mut self, world: &mut World) {
        // Upload only meshes marked dirty by the world (newly built/changed).
        // Existing unchanged meshes are left on the GPU untouched.
        let mut keep: std::collections::HashSet<ChunkPos> = std::collections::HashSet::new();
        for (pos, mesh) in world.iter_meshes() {
            keep.insert(pos);
            let need_upload = match self.chunk_meshes.get(&pos) {
                None => true,
                Some(_) => mesh.mesh_dirty,
            };
            if need_upload {
                let gm = upload_mesh(&self.device, mesh, pos);
                self.chunk_meshes.insert(pos, gm);
            }
        }
        // Drop removed.
        self.chunk_meshes.retain(|p, _| keep.contains(p));
        // Clear CPU-side dirty flags now that uploads are done.
        for pos in keep {
            if let Some(m) = world.meshes.get_mut(&pos) {
                m.mesh_dirty = false;
            }
        }
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        // Refresh the outline vertex buffer only when the target changed.
        if self.selection != self.selection_drawn {
            if let Some(b) = self.selection {
                let verts = outline_vertices(b);
                self.queue
                    .write_buffer(&self.outline_vbuf, 0, bytemuck::cast_slice(&verts));
            }
            self.selection_drawn = self.selection;
        }

        let mut enc = self
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("frame"),
            });
        // Frustum-cull + depth-sort the visible chunks once. The opaque pass
        // draws nearest-first so the GPU's early-Z rejects occluded fragments
        // before the (texture + tint + fog) fragment shader runs, cutting
        // overdraw, which is the dominant GPU cost in dense voxel terrain. The
        // transparent pass draws farthest-first for correct back-to-front alpha.
        let cam = self.cam_pos;
        let mut order: Vec<(f32, &GpuMesh)> = self
            .chunk_meshes
            .values()
            .filter(|gm| self.chunk_visible(gm))
            .map(|gm| {
                let (ox, oz) = gm.origin;
                let c = glam::Vec3::new(ox as f32 + 8.0, CHUNK_SY as f32 * 0.5, oz as f32 + 8.0);
                ((cam - c).length_squared(), gm)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let cc = self.clear_color;
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Clear(wgpu::Color {
                            r: cc[0] as f64,
                            g: cc[1] as f64,
                            b: cc[2] as f64,
                            a: 1.0,
                        }),
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Clear(1.0),
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.opaque_pipe);
            for (_, gm) in order.iter() {
                // near -> far (early-Z)
                if let (Some(vb), Some(ib)) = (&gm.opaque_vbuf, &gm.opaque_ibuf) {
                    if gm.opaque_idx_count == 0 {
                        continue;
                    }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.opaque_idx_count, 0, 0..1);
                }
            }
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("transparent pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_bind_group(0, &self.uniform_bind, &[]);
            pass.set_bind_group(1, &self.atlas_bind, &[]);
            pass.set_pipeline(&self.transparent_pipe);
            for (_, gm) in order.iter().rev() {
                // far -> near (alpha order)
                if let (Some(vb), Some(ib)) = (&gm.transparent_vbuf, &gm.transparent_ibuf) {
                    if gm.transparent_idx_count == 0 {
                        continue;
                    }
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    pass.draw_indexed(0..gm.transparent_idx_count, 0, 0..1);
                }
            }
        }
        // Selection outline, last: load color + depth, depth-test (no write) so
        // it draws over terrain/water at the targeted block but stays occluded
        // behind nearer geometry.
        if self.selection.is_some() {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("outline pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: Some(wgpu::RenderPassDepthStencilAttachment {
                    view: &self.depth,
                    depth_ops: Some(wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    }),
                    stencil_ops: None,
                }),
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.outline_pipe);
            pass.set_bind_group(0, &self.outline_bind, &[]);
            pass.set_vertex_buffer(0, self.outline_vbuf.slice(..));
            pass.draw(0..24, 0..1);
        }
        self.queue.submit(std::iter::once(enc.finish()));
        frame.present();
    }
}
