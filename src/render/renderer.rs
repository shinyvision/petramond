use crate::camera::{Camera, Frustum};
use crate::chunk::{ChunkPos, CHUNK_SY, SECTION_COUNT, SECTION_SIZE};
use crate::mathh::SelectionShape;
use crate::mesh::MeshIndexSection;
use crate::world::World;

use std::collections::HashMap;
use wgpu::util::DeviceExt;

use super::crosshair::crosshair_vertices;
use super::pipeline::create_pipeline_resources;
use super::resources::{create_atlas, create_depth, upload_mesh, GpuMesh};
use super::section_cull::SectionVisibilityCache;
use super::selection::outline_vertices;
use super::uniforms::{Uniforms, FOG_END, FOG_START, UNDERWATER_FOG_END, UNDERWATER_FOG_START};

const FAR_LEAF_LOD_FADE_START: f32 = 128.0;
const FAR_LEAF_LOD_FADE_END: f32 = 192.0;
const MIN_SECTION_CULL_INDEX_SAVINGS: u32 = 2_048;

#[derive(Copy, Clone, Debug, Default)]
pub struct RenderStats {
    pub frustum_chunks: u32,
    pub drawn_chunks: u32,
    pub visible_sections: u32,
    pub opaque_draws: u32,
    pub transparent_draws: u32,
    pub opaque_indices: u64,
    pub transparent_indices: u64,
    pub section_culled_indices: u64,
    pub section_culling_active: bool,
}

pub struct Renderer {
    pub surface: wgpu::Surface<'static>,
    pub device: wgpu::Device,
    pub queue: wgpu::Queue,
    pub config: wgpu::SurfaceConfiguration,
    pub atlas_texture: wgpu::Texture,
    pub atlas_view: wgpu::TextureView,
    pub atlas_sampler: wgpu::Sampler,
    pub sky_pipe: wgpu::RenderPipeline,
    pub sky_bind: wgpu::BindGroup,
    pub opaque_pipe: wgpu::RenderPipeline,
    pub transparent_pipe: wgpu::RenderPipeline,
    /// Pipeline for the targeted-block wireframe (LineList, black, view_proj only).
    pub outline_pipe: wgpu::RenderPipeline,
    pub outline_bind: wgpu::BindGroup,
    /// Line vertices for the selection outline; rewritten only when the selected
    /// target changes (see `selection` / `selection_drawn`).
    pub outline_vbuf: wgpu::Buffer,
    pub outline_vertex_count: u32,
    pub crosshair_pipe: wgpu::RenderPipeline,
    pub crosshair_vbuf: wgpu::Buffer,
    pub crosshair_vertex_count: u32,
    crosshair_drawn_size: (u32, u32),
    /// Currently-targeted outline shape, or None when nothing is targeted.
    pub selection: Option<SelectionShape>,
    /// The target whose geometry currently sits in `outline_vbuf`.
    selection_drawn: Option<SelectionShape>,
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
    section_visibility: SectionVisibilityCache,
    /// Background clear colour, kept in sync with the fog colour each frame (sky/
    /// biome fog above water, deep blue when submerged) so the horizon matches the
    /// fog the terrain fades into.
    pub clear_color: [f32; 3],
    pub last_stats: RenderStats,
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
            fog_color: [0.60, 0.82, 1.00, 1.0],
            inv_view_proj: glam::Mat4::IDENTITY.to_cols_array_2d(),
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
        sky_pipe: pipelines.sky_pipe,
        sky_bind: pipelines.sky_bind,
        opaque_pipe: pipelines.opaque_pipe,
        transparent_pipe: pipelines.transparent_pipe,
        outline_pipe: pipelines.outline_pipe,
        outline_bind: pipelines.outline_bind,
        outline_vbuf: pipelines.outline_vbuf,
        outline_vertex_count: 0,
        crosshair_pipe: pipelines.crosshair_pipe,
        crosshair_vbuf: pipelines.crosshair_vbuf,
        crosshair_vertex_count: 0,
        crosshair_drawn_size: (0, 0),
        selection: None,
        selection_drawn: None,
        uniform_buf,
        uniform_bind: pipelines.uniform_bind,
        atlas_bind: pipelines.atlas_bind,
        depth,
        chunk_meshes: HashMap::new(),
        frustum: Frustum::permissive(),
        cam_pos: glam::Vec3::ZERO,
        section_visibility: SectionVisibilityCache::default(),
        clear_color: [0.60, 0.82, 1.00],
        last_stats: RenderStats::default(),
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
        self.crosshair_drawn_size = (0, 0);
    }

    pub fn update_uniforms(
        &mut self,
        cam: &Camera,
        fog_color: [f32; 3],
        time: f32,
        underwater: bool,
    ) {
        let view_proj = cam.view_proj();
        let inv_view_proj = view_proj.inverse();
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
            inv_view_proj: inv_view_proj.to_cols_array_2d(),
        };
        self.queue
            .write_buffer(&self.uniform_buf, 0, bytemuck::cast_slice(&[u]));
    }

    /// Set (or clear) the target highlighted by the selection outline. Cheap: the
    /// vertex buffer is only re-uploaded in `render` when the target changes.
    pub fn set_selection(&mut self, shape: Option<SelectionShape>) {
        self.selection = shape;
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

    pub fn update_section_visibility(&mut self, world: &mut World) {
        self.section_visibility.update(world, self.cam_pos);
    }

    pub fn render(&mut self) {
        let frame = match self.surface.get_current_texture() {
            Ok(t) => t,
            Err(_) => return,
        };
        let view = frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        if self.crosshair_drawn_size != (self.config.width, self.config.height) {
            let verts = crosshair_vertices(self.config.width, self.config.height);
            self.crosshair_vertex_count = verts.count;
            if verts.count > 0 {
                self.queue.write_buffer(
                    &self.crosshair_vbuf,
                    0,
                    bytemuck::cast_slice(&verts.vertices[..verts.count as usize]),
                );
            }
            self.crosshair_drawn_size = (self.config.width, self.config.height);
        }

        // Refresh the outline vertex buffer only when the target changed.
        if self.selection != self.selection_drawn {
            self.outline_vertex_count = 0;
            if let Some(shape) = self.selection {
                let outline = outline_vertices(shape);
                self.outline_vertex_count = outline.count;
                if outline.count > 0 {
                    self.queue.write_buffer(
                        &self.outline_vbuf,
                        0,
                        bytemuck::cast_slice(&outline.vertices[..outline.count as usize]),
                    );
                }
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
        let section_culling_active = self.section_visibility.is_active();
        let mut frustum_chunks = 0u32;
        let mut order: Vec<(f32, &GpuMesh)> = self
            .chunk_meshes
            .values()
            .filter(|gm| {
                if !self.chunk_visible(gm) {
                    return false;
                }
                frustum_chunks += 1;
                !section_culling_active || self.section_visibility.chunk_mask(gm.pos).is_some()
            })
            .map(|gm| {
                let (ox, oz) = gm.origin;
                let c = glam::Vec3::new(ox as f32 + 8.0, CHUNK_SY as f32 * 0.5, oz as f32 + 8.0);
                ((cam - c).length_squared(), gm)
            })
            .collect();
        order.sort_by(|a, b| a.0.total_cmp(&b.0));
        let mut stats = RenderStats {
            frustum_chunks,
            drawn_chunks: order.len() as u32,
            visible_sections: self.section_visibility.visible_section_count(),
            section_culling_active,
            ..Default::default()
        };
        let cc = self.clear_color;
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("sky pass"),
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
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.sky_pipe);
            pass.set_bind_group(0, &self.sky_bind, &[]);
            pass.draw(0..3, 0..1);
        }
        {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("opaque pass"),
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
            for (dist_sq, gm) in order.iter() {
                // near -> far (early-Z)
                let use_far_leaf_lod =
                    far_leaf_lod_active(*dist_sq, gm.origin, gm.far_opaque_idx_count > 0);
                let (vbuf, ibuf, idx_count, sections) = if use_far_leaf_lod {
                    (
                        &gm.far_opaque_vbuf,
                        &gm.far_opaque_ibuf,
                        gm.far_opaque_idx_count,
                        &gm.far_opaque_sections,
                    )
                } else {
                    (
                        &gm.opaque_vbuf,
                        &gm.opaque_ibuf,
                        gm.opaque_idx_count,
                        &gm.opaque_sections,
                    )
                };
                if idx_count == 0 {
                    continue;
                }
                if let (Some(vb), Some(ib)) = (vbuf, ibuf) {
                    pass.set_vertex_buffer(0, vb.slice(..));
                    pass.set_index_buffer(ib.slice(..), wgpu::IndexFormat::Uint32);
                    if let Some(mask) = self.section_visibility.chunk_mask(gm.pos) {
                        let ranges =
                            section_draw_ranges(self.frustum, gm.origin, idx_count, sections, mask);
                        if ranges.is_empty() {
                            continue;
                        }
                        for (start, end) in ranges.iter() {
                            stats.opaque_draws += 1;
                            pass.draw_indexed(start..end, 0, 0..1);
                        }
                        stats.opaque_indices += ranges.submitted as u64;
                        stats.section_culled_indices += (idx_count - ranges.submitted) as u64;
                    } else {
                        stats.opaque_draws += 1;
                        stats.opaque_indices += idx_count as u64;
                        pass.draw_indexed(0..idx_count, 0, 0..1);
                    }
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
                    if let Some(mask) = self.section_visibility.chunk_mask(gm.pos) {
                        let ranges = section_draw_ranges(
                            self.frustum,
                            gm.origin,
                            gm.transparent_idx_count,
                            &gm.transparent_sections,
                            mask,
                        );
                        if ranges.is_empty() {
                            continue;
                        }
                        for (start, end) in ranges.iter() {
                            stats.transparent_draws += 1;
                            pass.draw_indexed(start..end, 0, 0..1);
                        }
                        stats.transparent_indices += ranges.submitted as u64;
                        stats.section_culled_indices +=
                            (gm.transparent_idx_count - ranges.submitted) as u64;
                    } else {
                        stats.transparent_draws += 1;
                        stats.transparent_indices += gm.transparent_idx_count as u64;
                        pass.draw_indexed(0..gm.transparent_idx_count, 0, 0..1);
                    }
                }
            }
        }
        // Selection outline, last: load color + depth, depth-test (no write) so
        // it draws over terrain/water at the targeted block but stays occluded
        // behind nearer geometry.
        if self.selection.is_some() && self.outline_vertex_count > 0 {
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
            pass.draw(0..self.outline_vertex_count, 0..1);
        }
        if self.crosshair_vertex_count > 0 {
            let mut pass = enc.begin_render_pass(&wgpu::RenderPassDescriptor {
                label: Some("crosshair pass"),
                color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                    view: &view,
                    depth_slice: None,
                    resolve_target: None,
                    ops: wgpu::Operations {
                        load: wgpu::LoadOp::Load,
                        store: wgpu::StoreOp::Store,
                    },
                })],
                depth_stencil_attachment: None,
                timestamp_writes: None,
                occlusion_query_set: None,
            });
            pass.set_pipeline(&self.crosshair_pipe);
            pass.set_vertex_buffer(0, self.crosshair_vbuf.slice(..));
            pass.draw(0..self.crosshair_vertex_count, 0..1);
        }
        self.queue.submit(std::iter::once(enc.finish()));
        self.last_stats = stats;
        frame.present();
    }
}

struct SectionDrawRanges {
    ranges: [(u32, u32); SECTION_COUNT],
    len: usize,
    submitted: u32,
}

impl SectionDrawRanges {
    fn new() -> Self {
        Self {
            ranges: [(0, 0); SECTION_COUNT],
            len: 0,
            submitted: 0,
        }
    }

    fn full(index_count: u32) -> Self {
        let mut out = Self::new();
        if index_count > 0 {
            out.ranges[0] = (0, index_count);
            out.len = 1;
            out.submitted = index_count;
        }
        out
    }

    fn push(&mut self, start: u32, end: u32) {
        if start >= end {
            return;
        }
        if self.len > 0 && self.ranges[self.len - 1].1 == start {
            self.ranges[self.len - 1].1 = end;
        } else {
            self.ranges[self.len] = (start, end);
            self.len += 1;
        }
        self.submitted += end - start;
    }

    fn is_empty(&self) -> bool {
        self.len == 0
    }

    fn iter(&self) -> impl Iterator<Item = (u32, u32)> + '_ {
        self.ranges[..self.len].iter().copied()
    }
}

fn section_draw_ranges(
    frustum: Frustum,
    origin: (i32, i32),
    full_idx_count: u32,
    sections: &[MeshIndexSection; SECTION_COUNT],
    visible_mask: u16,
) -> SectionDrawRanges {
    let mut out = SectionDrawRanges::new();
    for (section_idx, section) in sections.iter().enumerate() {
        if visible_mask & (1u16 << section_idx) == 0 || section.index_count == 0 {
            continue;
        }
        if !section_visible(frustum, origin, section_idx) {
            continue;
        }
        out.push(
            section.first_index,
            section.first_index + section.index_count,
        );
    }

    if out.is_empty() || out.submitted >= full_idx_count {
        return out;
    }
    if out.len == 1 {
        return out;
    }

    let saved = full_idx_count - out.submitted;
    let saves_enough_indices = saved >= MIN_SECTION_CULL_INDEX_SAVINGS;
    let saves_enough_ratio = (out.submitted as u64) * 4 <= (full_idx_count as u64) * 3;
    if saves_enough_indices && saves_enough_ratio {
        out
    } else {
        SectionDrawRanges::full(full_idx_count)
    }
}

fn section_visible(frustum: Frustum, origin: (i32, i32), section_idx: usize) -> bool {
    let (ox, oz) = origin;
    let y0 = (section_idx * SECTION_SIZE) as f32;
    let y1 = ((section_idx + 1) * SECTION_SIZE).min(CHUNK_SY) as f32;
    let min = glam::Vec3::new(ox as f32, y0, oz as f32);
    let max = glam::Vec3::new((ox + 16) as f32, y1, (oz + 16) as f32);
    frustum.aabb_visible(min, max)
}

fn far_leaf_lod_active(dist_sq: f32, origin: (i32, i32), has_far_lod: bool) -> bool {
    if !has_far_lod {
        return false;
    }

    let dist = dist_sq.sqrt();
    if dist <= FAR_LEAF_LOD_FADE_START {
        return false;
    }
    if dist >= FAR_LEAF_LOD_FADE_END {
        return true;
    }

    let t = (dist - FAR_LEAF_LOD_FADE_START) / (FAR_LEAF_LOD_FADE_END - FAR_LEAF_LOD_FADE_START);
    let smooth = t * t * (3.0 - 2.0 * t);
    smooth >= chunk_lod_threshold(origin)
}

fn chunk_lod_threshold(origin: (i32, i32)) -> f32 {
    let mut h =
        (origin.0 as u32).wrapping_mul(0x9E37_79B1) ^ (origin.1 as u32).wrapping_mul(0x85EB_CA77);
    h ^= h >> 16;
    h = h.wrapping_mul(0x7FEB_352D);
    h ^= h >> 15;
    h = h.wrapping_mul(0x846C_A68B);
    h ^= h >> 16;
    ((h & 0xFFFF) as f32 + 0.5) / 65_536.0
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn section_draw_ranges_keep_single_visible_section() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[2] = MeshIndexSection {
            first_index: 120,
            index_count: 60,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 480, &sections, 1u16 << 2);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(120, 180)]);
        assert_eq!(ranges.submitted, 60);
    }

    #[test]
    fn section_draw_ranges_fall_back_when_fragmented_savings_are_small() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 100,
        };
        sections[2] = MeshIndexSection {
            first_index: 200,
            index_count: 100,
        };

        let ranges = section_draw_ranges(frustum, (0, 0), 360, &sections, 0b0101);

        assert_eq!(ranges.iter().collect::<Vec<_>>(), vec![(0, 360)]);
        assert_eq!(ranges.submitted, 360);
    }

    #[test]
    fn section_draw_ranges_keep_fragmented_ranges_when_savings_are_large() {
        let frustum = Frustum::permissive();
        let mut sections = [MeshIndexSection::default(); SECTION_COUNT];
        sections[0] = MeshIndexSection {
            first_index: 0,
            index_count: 600,
        };
        sections[8] = MeshIndexSection {
            first_index: 8_000,
            index_count: 600,
        };

        let ranges = section_draw_ranges(
            frustum,
            (0, 0),
            12_000,
            &sections,
            (1u16 << 0) | (1u16 << 8),
        );

        assert_eq!(
            ranges.iter().collect::<Vec<_>>(),
            vec![(0, 600), (8_000, 8_600)]
        );
        assert_eq!(ranges.submitted, 1_200);
    }

    #[test]
    fn far_leaf_lod_stays_near_and_converges_far() {
        assert!(!far_leaf_lod_active(200.0 * 200.0, (0, 0), false));
        assert!(!far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_START * FAR_LEAF_LOD_FADE_START,
            (0, 0),
            true
        ));
        assert!(far_leaf_lod_active(
            FAR_LEAF_LOD_FADE_END * FAR_LEAF_LOD_FADE_END,
            (0, 0),
            true
        ));
    }

    #[test]
    fn far_leaf_lod_transition_is_staggered_by_chunk() {
        let mid = ((FAR_LEAF_LOD_FADE_START + FAR_LEAF_LOD_FADE_END) * 0.5).powi(2);
        let mut near_count = 0;
        let mut far_count = 0;
        for z in -8..=8 {
            for x in -8..=8 {
                if far_leaf_lod_active(mid, (x * 16, z * 16), true) {
                    far_count += 1;
                } else {
                    near_count += 1;
                }
            }
        }

        assert!(near_count > 0);
        assert!(far_count > 0);
    }
}
