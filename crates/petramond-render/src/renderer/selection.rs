//! The region selection as it is drawn: the union's green outline, the amber
//! pending box, the hovered or dragged face, and the brightness lift on the
//! selected world surfaces. Everything uploads on change only; none of it
//! scales with the selected volume.

use super::{dynamic_draw, Renderer};
use glam::{IVec3, Mat4};
use petramond::schematic::{Selection, SelectionFace};

struct Layer {
    pipeline: crate::pipeline::SampledPipeline,
    vertices: wgpu::Buffer,
    uniform: wgpu::Buffer,
    bind: wgpu::BindGroup,
    count: u32,
    origin: IVec3,
    color: [f32; 4],
}
impl Layer {
    fn new(r: &Renderer, lines: bool, color: [f32; 4]) -> Self {
        let (pipeline, bgl) = crate::pipeline::world_overlay::flat(
            &r.device,
            r.config.format,
            r.targets.max_samples,
            lines,
        );
        let uniform = r.device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("selection overlay camera"),
            size: 80,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind = r.device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("selection overlay camera"),
            layout: &bgl,
            entries: &[wgpu::BindGroupEntry {
                binding: 0,
                resource: uniform.as_entire_binding(),
            }],
        });
        Self {
            pipeline,
            vertices: dynamic_draw::new_buffer(
                &r.device,
                wgpu::BufferUsages::VERTEX,
                "selection overlay",
            ),
            uniform,
            bind,
            count: 0,
            origin: IVec3::ZERO,
            color,
        }
    }
    fn upload(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, vertices: &[[f32; 3]]) {
        self.count = vertices.len() as u32;
        if !vertices.is_empty() {
            dynamic_draw::upload(
                device,
                queue,
                &mut self.vertices,
                vertices,
                wgpu::BufferUsages::VERTEX,
                "selection overlay",
            );
        }
    }
    fn camera(&self, queue: &wgpu::Queue, vp: Mat4, origin: IVec3) {
        let mvp = vp * Mat4::from_translation((self.origin - origin).as_vec3());
        let mut data = [0f32; 20];
        data[..16].copy_from_slice(&mvp.to_cols_array());
        data[16..].copy_from_slice(&self.color);
        queue.write_buffer(&self.uniform, 0, bytemuck::cast_slice(&data));
    }
    fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        if self.count == 0 {
            return;
        }
        pass.set_pipeline(self.pipeline.get(samples));
        pass.set_bind_group(0, &self.bind, &[]);
        pass.set_vertex_buffer(0, self.vertices.slice(..));
        pass.draw(0..self.count, 0..1);
    }
}

struct Gpu {
    highlight: crate::selection_highlight::Highlight,
    outline: Layer,
    region: Layer,
    face: Layer,
    active_face: Option<SelectionFace>,
    revision: Option<u64>,
    corners: Option<([i32; 3], [i32; 3])>,
}

impl Gpu {
    fn new(r: &Renderer) -> Self {
        Self {
            highlight: crate::selection_highlight::Highlight::new(
                &r.device,
                r.opaque_pipe.get(1).get_bind_group_layout(0),
                r.uniform_buf.clone(),
            ),
            outline: Layer::new(r, true, [0.2, 1.0, 0.7, 0.85]),
            region: Layer::new(r, true, [1.0, 0.7, 0.1, 1.0]),
            face: Layer::new(r, false, [0.65, 1.0, 0.9, 0.32]),
            active_face: None,
            revision: None,
            corners: None,
        }
    }

    fn layers(&self) -> [&Layer; 3] {
        [&self.face, &self.outline, &self.region]
    }
}

#[derive(Default)]
pub(super) struct SelectionPass {
    gpu: Option<Gpu>,
    shown: bool,
}

impl SelectionPass {
    pub(super) fn clear_world(&mut self) {
        *self = Self::default();
    }

    fn shown(&self) -> Option<&Gpu> {
        self.gpu.as_ref().filter(|_| self.shown)
    }

    pub(super) fn is_empty(&self) -> bool {
        self.shown().is_none()
    }

    pub(super) fn camera(&self, queue: &wgpu::Queue, u: &crate::uniforms::Uniforms) {
        let vp = Mat4::from_cols_array_2d(&u.view_proj);
        let origin = IVec3::new(u.render_origin[0], u.render_origin[1], u.render_origin[2]);
        for layer in self.gpu.iter().flat_map(Gpu::layers) {
            layer.camera(queue, vp, origin);
        }
    }

    pub(super) fn draw(&self, pass: &mut wgpu::RenderPass<'_>, samples: u32) {
        for layer in self.shown().into_iter().flat_map(Gpu::layers) {
            layer.draw(pass, samples);
        }
    }
}

impl Renderer {
    pub(super) fn selected_blocks_bind(&self) -> &wgpu::BindGroup {
        self.selection
            .shown()
            .filter(|gpu| gpu.outline.count > 0)
            .map(|gpu| &gpu.highlight.bind)
            .unwrap_or(&self.uniform_bind)
    }

    /// Show `selection` with its pending two-corner box and active face;
    /// `None` hides the whole overlay.
    pub fn set_selection_overlay(
        &mut self,
        selection: Option<&Selection>,
        corners: Option<([i32; 3], [i32; 3])>,
        face: Option<&SelectionFace>,
    ) {
        self.selection.shown = selection.is_some();
        let Some(selection) = selection else {
            return;
        };
        let mut gpu = match self.selection.gpu.take() {
            Some(gpu) => gpu,
            None => Gpu::new(self),
        };
        if gpu.revision != Some(selection.revision()) {
            gpu.revision = Some(selection.revision());
            gpu.highlight.upload(&self.device, &self.queue, selection);
            let anchor = IVec3::from_array(selection.regions().first().map_or([0; 3], |r| r.lo));
            gpu.outline.origin = anchor;
            let vertices: Vec<_> = selection
                .outline()
                .into_iter()
                .flatten()
                .map(|p| (IVec3::from_array(p) - anchor).as_vec3().to_array())
                .collect();
            gpu.outline.upload(&self.device, &self.queue, &vertices);
        }
        if gpu.corners != corners {
            gpu.corners = corners;
            let mut vertices = Vec::new();
            if let Some((a, b)) = corners {
                let lo = IVec3::from_array(a).min(IVec3::from_array(b));
                let hi = IVec3::from_array(a).max(IVec3::from_array(b));
                gpu.region.origin = lo;
                box_edges(
                    &mut vertices,
                    glam::Vec3::ZERO,
                    (hi - lo).as_vec3() + glam::Vec3::ONE,
                );
            }
            gpu.region.upload(&self.device, &self.queue, &vertices);
        }
        if gpu.active_face.as_ref() != face {
            gpu.active_face = face.cloned();
            let mut vertices = Vec::new();
            if let Some(face) = face {
                let anchor = face.quads().next().map_or([0; 3], |quad| quad[0]);
                gpu.face.origin = IVec3::from_array(anchor);
                for quad in face.quads() {
                    let quad = quad.map(|p| {
                        let mut p: [f32; 3] = std::array::from_fn(|i| {
                            (i64::from(p[i]) - i64::from(anchor[i])) as f32
                        });
                        p[face.axis] += face.normal as f32 * 0.003;
                        p
                    });
                    vertices.extend([quad[0], quad[1], quad[2], quad[0], quad[2], quad[3]]);
                }
            }
            gpu.face.upload(&self.device, &self.queue, &vertices);
        }
        self.selection.gpu = Some(gpu);
    }
}

fn box_edges(out: &mut Vec<[f32; 3]>, lo: glam::Vec3, hi: glam::Vec3) {
    let vertices = crate::selection::outline_vertices(
        petramond_math::math::SelectionShape::Box {
            origin: IVec3::ZERO,
            min: lo,
            max: hi,
        },
        IVec3::ZERO,
    );
    out.extend(vertices.vertices);
}

#[cfg(test)]
mod tests;
